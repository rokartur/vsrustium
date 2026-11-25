//! Keybinding dispatch (platform/keybinding/common/abstractKeybindingService.ts `_doDispatch`): every key
//! down of the window resolves against the keybindings in the context of the focused part, before the
//! focused element sees it. Multi-chord mode, its status messages and the context keys live here too.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use futures::StreamExt;
use gpui::{Context, Keystroke, Task, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::search_view::SideBarView;
use crate::command_palette::MenuItem;
use crate::commands::Outcome;
use crate::editor::TextDocument;
use crate::keybindings::{self, Chord, Item, Resolution, Resolver};
use crate::system_wide_keybindings::{HotKeys, SystemWide};

/// The keybindings and the dispatch state of `AbstractKeybindingService`.
pub struct Keybindings {
    pub(super) defaults: Vec<Item>,
    /// `<user data>/User/keybindings.json`.
    pub(super) user_file: Option<PathBuf>,
    pub resolver: Resolver,
    /// Counts the resolver's rebuilds, so the Keyboard Shortcuts editor knows its rows are stale.
    pub(super) generation: u64,
    /// Chords pressed so far in multi-chord mode, with their labels.
    chords: Vec<(Chord, String)>,
    chord_checker: Option<Task<()>>,
    system_wide: SystemWide,
}

impl Default for Keybindings {
    fn default() -> Self {
        Keybindings { defaults: Vec::new(), user_file: None, resolver: Resolver::new(Vec::new(), Vec::new()), generation: 0, chords: Vec::new(), chord_checker: None, system_wide: SystemWide::default() }
    }
}

impl Workbench {
    /// Sets the default and user keybindings, and reloads the user's file when it changes on disk.
    pub fn load_keybindings(&mut self, defaults: Vec<Item>, user_file: PathBuf, cx: &mut Context<Self>) {
        let resource = crate::editor::uri_string("file", &user_file);
        self.keybindings.defaults = defaults.into_iter().map(|item| Item { when: keybindings::with_keybindings_resource(item.when, &resource), ..item }).collect();
        self.keybindings.user_file = Some(user_file.clone());
        let menu = std::mem::take(&mut self.quick_access.menu);
        self.set_command_palette_menu(menu);
        self.set_user_keybindings(keybindings::user_keybindings(&user_file));
        // shortcut: the file is polled once a second where the reference has a file watcher; no watcher crate is in the port.
        cx.spawn(async move |workbench, cx| {
            let modified = |path: &PathBuf| std::fs::metadata(path).and_then(|metadata| metadata.modified()).ok();
            let mut seen: Option<SystemTime> = modified(&user_file);
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let now = modified(&user_file);
                if now == seen {
                    continue;
                }
                seen = now;
                let overrides = keybindings::user_keybindings(&user_file);
                let updated = workbench.update(cx, |workbench, cx| {
                    workbench.set_user_keybindings(overrides);
                    cx.notify();
                });
                if updated.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// The Command Palette's items, whose `resource ==` clauses of the reference's profile name the port's
    /// keybindings.json once it is known (`with_keybindings_resource`).
    pub fn set_command_palette_menu(&mut self, menu: Vec<MenuItem>) {
        let Some(file) = &self.keybindings.user_file else {
            self.quick_access.menu = menu;
            return;
        };
        let resource = crate::editor::uri_string("file", file);
        let map = |when| keybindings::with_keybindings_resource(when, &resource);
        self.quick_access.menu = menu.into_iter().map(|item| MenuItem { when: map(item.when), precondition: map(item.precondition), ..item }).collect();
    }

    /// Rebuilds the resolver over the defaults and the user's keybindings, and syncs the system-wide ones.
    pub(super) fn set_user_keybindings(&mut self, overrides: Vec<Item>) {
        self.keybindings.system_wide.set_keybindings(&overrides);
        self.keybindings.resolver = Resolver::new(self.keybindings.defaults.clone(), overrides);
        self.keybindings.generation += 1;
    }

    /// Registers the system-wide keybindings with macOS and runs the command of each press, as the
    /// `vscode:runAction` request of globalKeybindingsMainService.ts.
    pub fn start_system_wide_keybindings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, mut presses) = futures::channel::mpsc::unbounded();
        self.keybindings.system_wide.start(HotKeys::new(sender));
        cx.spawn_in(window, async move |workbench, cx| {
            while let Some(id) = presses.next().await {
                let ran = workbench.update_in(cx, |workbench, window, cx| {
                    let Some(binding) = workbench.keybindings.system_wide.pressed(id) else { return };
                    let (command, args) = (binding.command.clone(), binding.args.clone().into_iter().collect());
                    let outcome = workbench.execute_command(&command, args, window, cx);
                    report_failure(command, outcome, cx);
                });
                if ran.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// `_doDispatch`: runs the command bound to the key and keeps the key from the focused element.
    pub(super) fn dispatch_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        // isKeyInComposition: the IME owns the keys while it composes.
        if self.marked_text.is_some() || self.input_composing() {
            return;
        }
        // The menu's own key handlers run before the window's keybindings.
        if self.context_menu_key(keystroke, window, cx) {
            cx.stop_propagation();
            return;
        }
        let chord = Chord::from_keystroke(keystroke);
        if chord.is_modifier_key() {
            return;
        }
        let current: Vec<Chord> = self.keybindings.chords.iter().map(|(chord, _)| *chord).collect();
        let in_chord_mode = !current.is_empty();
        let resolution = self.keybindings.resolver.resolve(&self.key_context(window), &current, chord);
        if !in_chord_mode && self.keybindings_editor_record_key(chord, &resolution, window, cx) {
            cx.stop_propagation();
            return;
        }
        if self.modal_blocks_command(&resolution, window) {
            cx.stop_propagation();
            return;
        }
        match resolution {
            Resolution::MoreChordsNeeded => {
                cx.stop_propagation();
                self.expect_another_chord(chord, window, cx);
            }
            Resolution::KbFound { command, args, bubble } if !command.is_empty() => {
                if in_chord_mode {
                    self.leave_chord_mode(cx);
                }
                if !bubble {
                    cx.stop_propagation();
                }
                let outcome = self.execute_command(&command, args.into_iter().collect(), window, cx);
                report_failure(command, outcome, cx);
            }
            // Nothing bound, or bound to no command: only a chord sequence in progress ends with a message.
            _ if in_chord_mode => {
                let message = format!("The key combination ({}, {}) is not a command.", self.chord_labels(), chord.label());
                self.show_status(message, Duration::from_secs(10), window, cx);
                self.leave_chord_mode(cx);
                cx.stop_propagation();
            }
            // hoverService.ts `_keyDown` with hideOnKeyDown, which every workbench hover delegate sets.
            _ => self.dismiss_hover(cx),
        }
    }

    fn chord_labels(&self) -> String {
        self.keybindings.chords.iter().map(|(_, label)| label.as_str()).collect::<Vec<_>>().join(", ")
    }

    /// The status bar message of multi-chord mode, shown while it lasts.
    pub(super) fn chord_status(&self) -> Option<String> {
        match self.keybindings.chords.len() {
            0 => None,
            1 => Some(format!("({}) was pressed. Waiting for second key of chord...", self.chord_labels())),
            _ => Some(format!("({}) was pressed. Waiting for next key of chord...", self.chord_labels())),
        }
    }

    fn expect_another_chord(&mut self, chord: Chord, window: &mut Window, cx: &mut Context<Self>) {
        self.keybindings.chords.push((chord, chord.label()));
        cx.notify();
        // _scheduleLeaveChordMode: leave when the window loses focus or 5s after the last chord.
        let entered = Instant::now();
        self.keybindings.chord_checker = Some(cx.spawn_in(window, async move |workbench, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(500)).await;
                let left = workbench.update_in(cx, |workbench, window, cx| {
                    let leave = !window.is_window_active() || entered.elapsed() > Duration::from_secs(5);
                    if leave {
                        workbench.leave_chord_mode(cx);
                    }
                    leave
                });
                if left.unwrap_or(true) {
                    return;
                }
            }
        }));
    }

    fn leave_chord_mode(&mut self, cx: &mut Context<Self>) {
        self.keybindings.chords.clear();
        // Dropping the task cancels it; it may be the task running this call, which ends right after.
        self.keybindings.chord_checker.take();
        cx.notify();
    }

    /// `INotificationService.status` with `hideAfter`: a message at the far right of the status bar's left side.
    pub(super) fn show_status(&mut self, message: String, hide_after: Duration, window: &mut Window, cx: &mut Context<Self>) {
        let hide = cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(hide_after).await;
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.status_message = None;
                cx.notify();
            });
        });
        self.status_message = Some((message, hide));
        cx.notify();
    }

    /// The context keys the keybindings' `when` clauses see: the keys extensions set, then the focused part's.
    pub fn key_context(&self, window: &Window) -> HashMap<String, Value> {
        let mut context = self.context_keys.clone();
        let editor_focused = self.editor_focus.is_focused(window);
        // The sticky lines and the find widget are part of the editor's DOM, so their focus is the editor's but not its text's.
        let (find_focused, replace_focused) = (self.find.find_focus.is_focused(window), self.find.replace_focus.is_focused(window));
        let button_focused = self.find.button_focus.values().any(|focus| focus.is_focused(window));
        context.insert("editorFocus".into(), json!(editor_focused || self.sticky_scroll.is_focused(window) || find_focused || replace_focused || button_focused));
        context.insert("editorTextFocus".into(), json!(editor_focused));
        context.insert("textInputFocus".into(), json!(editor_focused));
        context.insert("editorIsOpen".into(), json!(self.editors.active().is_some()));
        context.insert("dirtyWorkingCopies".into(), json!(self.editors.editors.iter().any(|editor| editor.is_dirty())));
        // viewDescriptorService.ts: the Explorer and Search containers always hold their view.
        for (container, view) in [("workbench.view.explorer", "workbench.explorer.fileView"), ("workbench.view.search", "workbench.view.search")] {
            context.insert(format!("viewContainer.{container}.enabled"), json!(true));
            context.insert(format!("{view}.active"), json!(true));
        }
        if let Some(index) = self.editors.active_index() {
            context.insert("activeEditorIsLastInGroup".into(), json!(index == self.editors.editors.len() - 1));
        }
        if let Some(editor) = self.editors.active() {
            context.insert("activeEditor".into(), json!("workbench.editor.files"));
            // The port opens only files, never untitled editors (editorGroupView.ts).
            context.insert("activeEditorCanRevert".into(), json!(true));
            context.insert("editorReadonly".into(), json!(false));
            context.insert("editorHasSelection".into(), json!(editor.selections.iter().any(|selection| !selection.is_empty())));
            context.insert("editorHasMultipleSelections".into(), json!(editor.selections.len() > 1));
            context.insert("editorColumnSelection".into(), self.setting("editor.columnSelection"));
            context.insert("foldingEnabled".into(), self.setting("editor.folding"));
            context.insert("editorLangId".into(), json!(editor.document.language_id));
            context.insert("resourceLangId".into(), json!(editor.document.language_id));
            self.sticky_scroll_context(window, &mut context);
            // findModel.ts CONTEXT_FIND_WIDGET_VISIBLE, CONTEXT_FIND_INPUT_FOCUSED, CONTEXT_REPLACE_INPUT_FOCUSED.
            context.insert("findWidgetVisible".into(), json!(self.find.controller.state.is_revealed));
            context.insert("findInputFocussed".into(), json!(find_focused));
            context.insert("replaceInputFocussed".into(), json!(replace_focused));
            if find_focused || replace_focused {
                context.insert("inputFocus".into(), json!(true));
                // contextScopedHistoryWidget.ts: the inputs are history widgets, navigable both ways.
                context.insert("historyNavigationWidgetFocus".into(), json!(true));
                context.insert("historyNavigationForwardsEnabled".into(), json!(true));
                context.insert("historyNavigationBackwardsEnabled".into(), json!(true));
            }
            let path = &editor.document.path;
            context.insert("resource".into(), json!(crate::editor::uri_string(&editor.document.scheme, path)));
            context.insert("resourceScheme".into(), json!(editor.document.scheme));
            context.insert("resourceFilename".into(), json!(path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default()));
            context.insert("resourceExtname".into(), json!(path.extension().map(|ext| format!(".{}", ext.to_string_lossy())).unwrap_or_default()));
        }
        self.quick_input_context(window, &mut context);
        match self.side_bar_view {
            SideBarView::Explorer => self.explorer_context(window, &mut context),
            SideBarView::Search => self.search_context(window, &mut context),
        }
        self.keybindings_editor_context(window, &mut context);
        context
    }

    /// `openGlobalKeybindingSettings(textual: true)`: opens keybindings.json, created with upstream's header first.
    pub(super) fn open_keybindings_file(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(file) = self.keybindings.user_file.clone() else { return Outcome::error("no keybindings.json") };
        if !file.exists() {
            let created = file.parent().map_or(Ok(()), std::fs::create_dir_all);
            if let Err(error) = created.and_then(|_| std::fs::write(&file, "// Place your key bindings in this file to override the defaults\n[\n]")) {
                return Outcome::error(format!("writing {}: {error}", file.display()));
            }
        }
        self.open_file(file, true, window, cx);
        Outcome::done()
    }

    /// `editorService.openEditor({ resource: undefined, contents, options: { pinned: true } })` of inspectKeybindings.ts.
    pub(super) fn open_untitled_with_contents(&mut self, contents: &str, window: &mut Window, cx: &mut Context<Self>) {
        let path = (1..).map(|n| PathBuf::from(format!("Untitled-{n}"))).find(|path| self.editors.model("untitled", path).is_none()).expect("a free name");
        let document = TextDocument::untitled(&path, contents, None, &self.languages, &self.settings.lock().unwrap());
        self.editors.open(document, false);
        window.focus(&self.editor_focus, cx);
        self.editors_changed(cx);
    }

    /// The `_setContext` command (extensions call it as `setContext`): `createKey(String(key), value)`.
    pub(super) fn register_context_commands(&mut self) {
        self.commands.register("workbench.action.openGlobalKeybindingsFile", |workbench, _, window, cx| workbench.open_keybindings_file(window, cx));
        self.commands.register("workbench.action.inspectKeyMappingsJSON", |workbench, _, window, cx| {
            workbench.open_untitled_with_contents(&crate::keyboard_layout::dump_debug_info_json(), window, cx);
            Outcome::done()
        });
        self.commands.register("_setContext", |workbench, args, _, _| {
            let mut args = args.into_iter();
            let key = match args.next() {
                Some(Value::String(key)) => key,
                Some(key) => key.to_string(),
                None => return Outcome::error("setContext needs a key"),
            };
            workbench.context_keys.insert(key, args.next().unwrap_or(Value::Null));
            Outcome::done()
        });
        // mainThreadLogService.ts; the level starts at Info, the init data's logLevel 3.
        self.commands.register("_extensionTests.setLogLevel", |workbench, args, _, _| {
            if let Some(Value::String(level)) = args.first() {
                workbench.log_level = level.clone();
            }
            Outcome::done()
        });
        self.commands.register("_extensionTests.getLogLevel", |workbench, _, _, _| Outcome::Ready(Ok(Value::from(workbench.log_level.clone()))));
    }
}

/// `notificationService.warn(err)` of a failed keybinding command.
/// shortcut: logged, since the port has no notifications toasts.
pub(super) fn report_failure(command: String, outcome: Outcome, cx: &mut Context<Workbench>) {
    let log = move |result: Result<Value, Value>| {
        if let Err(error) = result {
            eprintln!("keybinding command {command} failed: {}", error["message"].as_str().unwrap_or_default());
        }
    };
    match outcome {
        Outcome::Ready(result) => log(result),
        Outcome::Pending(pending) => cx.background_executor().spawn(async move { log(pending.await.unwrap_or(Ok(Value::Null))) }).detach(),
    }
}
