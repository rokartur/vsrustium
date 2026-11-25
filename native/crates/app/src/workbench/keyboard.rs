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
    /// The rules of keybindings.json, kept to rebuild the resolver when the keyboard layout changes.
    overrides: Vec<Item>,
    /// `<user data>/User/keybindings.json`.
    pub(super) user_file: Option<PathBuf>,
    pub resolver: Resolver,
    /// Counts the resolver's rebuilds, so the Keyboard Shortcuts editor knows its rows are stale.
    pub(super) generation: u64,
    /// Chords pressed so far in multi-chord mode, with their labels.
    chords: Vec<(Chord, String)>,
    chord_checker: Option<Task<()>>,
    system_wide: SystemWide,
    /// `_logging`, toggled by Toggle Keyboard Shortcuts Troubleshooting.
    pub(super) logging: bool,
}

impl Default for Keybindings {
    fn default() -> Self {
        Keybindings { defaults: Vec::new(), overrides: Vec::new(), user_file: None, resolver: Resolver::new(Vec::new(), Vec::new()), generation: 0, chords: Vec::new(), chord_checker: None, system_wide: SystemWide::default(), logging: false }
    }
}

impl Workbench {
    /// Sets the default and user keybindings, and reloads the user's file when it changes on disk.
    pub fn load_keybindings(&mut self, defaults: Vec<Item>, user_file: PathBuf, cx: &mut Context<Self>) {
        self.keybindings.user_file = Some(user_file.clone());
        self.set_default_keybindings(defaults);
        let menu = std::mem::take(&mut self.quick_access.menu);
        self.set_command_palette_menu(menu);
        self.set_user_keybindings(keybindings::user_keybindings(&user_file));
        // shortcut: the file is polled once a second where the reference has a file watcher; no watcher crate is in the port.
        cx.spawn(async move |workbench, cx| {
            let modified = |path: &PathBuf| std::fs::metadata(path).and_then(|metadata| metadata.modified()).ok();
            let mut watched = user_file;
            let mut seen: Option<SystemTime> = modified(&watched);
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                // A profile switch moves the file; its change was applied then.
                let Ok(Some(user_file)) = workbench.read_with(cx, |workbench, _| workbench.keybindings.user_file.clone()) else { return };
                if user_file != watched {
                    seen = modified(&user_file);
                    watched = user_file;
                    continue;
                }
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

    /// The keybindings the core and extensions contribute; `set_user_keybindings` puts them in the resolver.
    pub fn set_default_keybindings(&mut self, defaults: Vec<Item>) {
        self.keybindings.defaults = self.with_user_keybindings_resource(defaults);
    }

    /// The default rules with the reference profile's `resource ==` clauses naming the port's keybindings.json.
    pub(super) fn with_user_keybindings_resource(&self, defaults: Vec<Item>) -> Vec<Item> {
        let resource = self.keybindings.user_file.as_ref().map(|file| crate::editor::uri_string("file", file)).unwrap_or_default();
        defaults
            .into_iter()
            .map(|item| {
                let when = keybindings::with_keybindings_resource(item.when.clone(), &resource);
                let when_text = if when == item.when { item.when_text } else { None };
                Item { when, when_text, ..item }
            })
            .collect()
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
        for warning in self.keybindings.system_wide.set_keybindings(&overrides) {
            self.notify_warning(warning);
        }
        self.keybindings.overrides = overrides;
        self.rebuild_resolver();
    }

    fn rebuild_resolver(&mut self) {
        self.keybindings.resolver = Resolver::new(self.keybindings.defaults.clone(), self.keybindings.overrides.clone());
        self.keybindings.generation += 1;
    }

    /// nativeKeyboardLayout.ts: a new keyboard mapper when the input source or `keyboard.dispatch` and
    /// `keyboard.mapAltGrToCtrlAlt` change, and the keybindings resolved again on it.
    pub fn refresh_keyboard_mapper(&mut self) {
        let setting = |key| self.settings.lock().unwrap().get_value(Some(key), &Default::default());
        let dispatch_key_code = setting("keyboard.dispatch") == Some(json!("keyCode"));
        let map_alt_gr_to_ctrl_alt = setting("keyboard.mapAltGrToCtrlAlt") == Some(json!(true));
        if crate::keyboard_mapper::refresh(dispatch_key_code, map_alt_gr_to_ctrl_alt) {
            self.rebuild_resolver();
        }
    }

    /// Registers the system-wide keybindings with macOS and runs the command of each press, as the
    /// `vscode:runAction` request of globalKeybindingsMainService.ts.
    pub fn start_system_wide_keybindings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, mut presses) = futures::channel::mpsc::unbounded();
        if let Some(warning) = self.keybindings.system_wide.start(HotKeys::new(sender)) {
            self.notify_warning(warning);
        }
        cx.spawn_in(window, async move |workbench, cx| {
            while let Some(id) = presses.next().await {
                let ran = workbench.update_in(cx, |workbench, window, cx| {
                    let Some(binding) = workbench.keybindings.system_wide.pressed(id) else { return };
                    let (command, args) = (binding.command.clone(), binding.args.clone().into_iter().collect());
                    let outcome = workbench.execute_command(&command, args, window, cx);
                    report_failure(outcome, cx);
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
        let native_event = crate::keyboard_layout::current_key_event();
        if native_event.is_some() {
            self.refresh_keyboard_mapper();
        }
        let chord = crate::keyboard_mapper::event_chord(keystroke);
        if self.keybindings.logging {
            let m = &keystroke.modifiers;
            let modifiers: Vec<&str> = [(m.control, "ctrl"), (m.shift, "shift"), (m.alt, "alt"), (m.platform, "meta")].into_iter().filter(|(on, _)| *on).map(|(_, name)| name).collect();
            // shortcut: GPUI keystrokes carry no DOM keyCode (`e.keyCode` of printKeyboardEvent), so it is left out.
            let code = native_event.map_or("", |event| event.code);
            self.log_keybinding(&format!("/ Received  keydown event - modifiers: [{}], code: {code}, key: {}", modifiers.join(","), keystroke.key));
            self.log_keybinding(&format!("| Converted keydown event - modifiers: [{}], code: {code}, keyCode: {} ('{}')", modifiers.join(","), chord.key, crate::keybindings::key_codes::KEY_CODES[chord.key].0));
        }
        // isKeyInComposition: the IME owns the keys while it composes.
        if self.marked_text.is_some() || self.input_composing() {
            self.log_keybinding("+ Ignoring keybinding dispatch because an IME composition is in progress.");
            return;
        }
        // The menu's own key handlers run before the window's keybindings.
        if self.context_menu_key(keystroke, window, cx) || self.custom_dialog_key(keystroke, cx) || self.accessible_view_key(keystroke, window, cx) || self.icon_select_box_key(keystroke, window, cx) {
            cx.stop_propagation();
            return;
        }
        self.find_input_key_down(keystroke, window, cx);
        if chord.is_modifier_key() {
            self.log_keybinding("\\ Keyboard event cannot be dispatched in keydown phase.");
            return;
        }
        let current: Vec<Chord> = self.keybindings.chords.iter().map(|(chord, _)| *chord).collect();
        let in_chord_mode = !current.is_empty();
        let mut log = Vec::new();
        let resolution = self.keybindings.resolver.resolve_logged(&self.key_context(window), &current, chord, &mut log);
        for line in &log {
            self.log_keybinding(line);
        }
        if !in_chord_mode && self.keybindings_editor_record_key(chord, &resolution, window, cx) {
            cx.stop_propagation();
            return;
        }
        if self.modal_blocks_command(&resolution, window) {
            cx.stop_propagation();
            return;
        }
        if !in_chord_mode && self.terminal_key(keystroke, &resolution, window, cx) {
            cx.stop_propagation();
            return;
        }
        // The list's own keydown listener runs before the window's keybindings.
        if !in_chord_mode && self.list_type_navigation(keystroke, resolution == Resolution::NoMatchingKb, window, cx) {
            cx.stop_propagation();
            return;
        }
        if self.editor_focus.is_focused(window) {
            self.content_hover_key_down(&resolution, cx);
        }
        match resolution {
            Resolution::MoreChordsNeeded => {
                cx.stop_propagation();
                self.expect_another_chord(chord, window, cx);
                self.log_keybinding(if current.is_empty() { "+ Entering multi-chord mode..." } else { "+ Continuing multi-chord mode..." });
            }
            Resolution::KbFound { command, args, bubble } if !command.is_empty() => {
                if in_chord_mode {
                    self.leave_chord_mode(cx);
                }
                if !bubble {
                    cx.stop_propagation();
                }
                self.log_keybinding(&format!("+ Invoking command {command}."));
                let outcome = self.execute_command(&command, args.into_iter().collect(), window, cx);
                report_failure(outcome, cx);
            }
            // Nothing bound, or bound to no command: only a chord sequence in progress ends with a message.
            _ if in_chord_mode => {
                let mode = if resolution == Resolution::NoMatchingKb { "multi-chord mode" } else { "chord mode" };
                self.log_keybinding(&format!("+ Leaving {mode}: Nothing bound to \"{}, {}\".", self.chord_labels(), chord.label()));
                let message = format!("The key combination ({}, {}) is not a command.", self.chord_labels(), chord.label());
                self.show_status(message, Duration::from_secs(10), window, cx);
                self.leave_chord_mode(cx);
                cx.stop_propagation();
            }
            _ if keystroke.key == "tab" && !(keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt) && self.move_tab_focus(keystroke.modifiers.shift, window, cx) => {
                cx.stop_propagation();
                self.dismiss_hover(cx);
            }
            // hoverService.ts `_keyDown` with hideOnKeyDown, which every workbench hover delegate sets.
            _ => self.dismiss_hover(cx),
        }
    }

    /// abstractKeybindingService.ts `_log`: an Info line of the Window log while troubleshooting.
    fn log_keybinding(&self, line: &str) {
        if self.keybindings.logging {
            self.output.log_window(3, &format!("[KeybindingService]: {line}"));
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
        // contextKeyService.ts `ConfigAwareContextValuesContainer`: `config.<setting>` is the setting's value.
        let mut context = HashMap::new();
        let configuration = self.settings.lock().unwrap().get_value(None, &config::configuration::Overrides::default());
        if let Some(Value::Object(sections)) = configuration {
            insert_config_keys("config", sections, &mut context);
        }
        context.extend(self.context_keys.clone());
        // list.contribution.ts `ListContext`; listSupportsKeyboardNavigation is the deprecated name.
        for key in ["listSupportsTypeNavigation", "listSupportsKeyboardNavigation"] {
            context.entry(key.into()).or_insert(json!(true));
        }
        let editor_focused = self.editor_focus.is_focused(window);
        // The sticky lines and the find widget are part of the editor's DOM, so their focus is the editor's but not its text's.
        let (find_focused, replace_focused) = (self.find.find_focus.is_focused(window), self.find.replace_focus.is_focused(window));
        let button_focused = self.find.button_focus.values().any(|focus| focus.is_focused(window));
        context.insert("editorFocus".into(), json!(editor_focused || self.sticky_scroll.is_focused(window) || find_focused || replace_focused || button_focused || self.rename_input_focused(window)));
        context.insert("editorTextFocus".into(), json!(editor_focused));
        context.insert("textInputFocus".into(), json!(editor_focused));
        self.notifications_context(window, &mut context);
        self.layout_context(&mut context);
        self.editor_groups_context(&mut context);
        self.history_context(&mut context);
        self.list_scroll_context(window, &mut context);
        context.insert("editorIsOpen".into(), json!(self.editors.active().is_some()));
        // viewDescriptorService.ts `<view id>.active`: the Changes view has no `when`, so it is always active.
        context.insert("workbench.scm.active".into(), json!(true));
        context.insert("scm.providerCount".into(), json!(self.scm.model.providers.len()));
        context.insert("dirtyWorkingCopies".into(), json!(self.editors.editors.iter().any(|editor| editor.is_dirty())));
        // toggleMultiCursorModifier.ts `MultiCursorModifierContextKeyController`.
        context.insert("multiCursorModifier".into(), json!(if self.multi_cursor_modifier_is_cmd() { "ctrlCmd" } else { "altKey" }));
        self.word_wrap_context(&mut context);
        self.problems_context(&mut context);
        // contextkeys.ts WorkbenchContextKeysHandler: the desktop build supports every workspace kind.
        let workbench_state = match self.workspace() {
            None => "empty",
            Some(workspace) if workspace.file.is_some() => "workspace",
            Some(_) => "folder",
        };
        context.insert("workbenchState".into(), json!(workbench_state));
        context.insert("workspaceFolderCount".into(), json!(self.workspace().map_or(0, |workspace| workspace.folders.len())));
        // RemoteNameContext of a local window.
        context.insert("remoteName".into(), json!(""));
        // workspace.contribution.ts WorkspaceTrustContextKeys.
        context.insert("isWorkspaceTrustEnabled".into(), json!(self.workspace_trust.enabled));
        context.insert("isWorkspaceTrusted".into(), json!(self.workspace_trust.is_trusted()));
        for key in ["openFolderWorkspaceSupport", "enterMultiRootWorkspaceSupport", "emptyWorkspaceSupport"] {
            context.insert(key.into(), json!(true));
        }
        // viewDescriptorService.ts: the Explorer, Search and Extensions containers always hold their views.
        context.insert("viewContainer.workbench.view.extensions.enabled".into(), json!(true));
        for (container, view) in [("workbench.view.explorer", "workbench.explorer.fileView"), ("workbench.view.search", "workbench.view.search")] {
            context.insert(format!("viewContainer.{container}.enabled"), json!(true));
            context.insert(format!("{view}.active"), json!(true));
        }
        if let Some(index) = self.editors.active_index() {
            context.insert("activeEditorIsLastInGroup".into(), json!(index == self.editors.editors.len() - 1));
        }
        if let Some(editor) = self.editors.active() {
            let pane = if editor.document().scheme == super::welcome_page::WALKTHROUGH_SCHEME { "gettingStartedPage" } else { "workbench.editor.files" };
            context.insert("activeEditor".into(), json!(pane));
            self.insert_diff_editor_context(&mut context);
            // The port opens only files, never untitled editors (editorGroupView.ts).
            context.insert("activeEditorCanRevert".into(), json!(true));
            let readonly = self.active_editor_readonly().is_some();
            context.insert("editorReadonly".into(), json!(readonly));
            context.insert("activeEditorIsReadonly".into(), json!(readonly));
            context.insert("activeEditorCanToggleReadonly".into(), json!(editor.document().scheme != "untitled"));
            context.insert("messageVisible".into(), json!(self.editor_message_visible()));
            context.insert("markersNavigationVisible".into(), json!(self.markers_navigation_visible()));
            self.goto_symbol_context(&mut context);
            self.references_peek_context(window, &mut context);
            self.hierarchy_peek_context(editor, window, &mut context);
            context.insert("editorHoverVisible".into(), json!(self.content_hover_visible()));
            self.suggest_context(&mut context);
            self.snippet_context(&mut context);
            self.parameter_hints_context(&mut context);
            self.code_actions_context(&mut context);
            self.action_widget_context(&mut context);
            self.code_lens_context(&mut context);
            self.rename_context(&mut context);
            self.format_context(&mut context);
            self.linked_editing_context(&mut context);
            context.insert("editorHasSelection".into(), json!(editor.selections.iter().any(|selection| !selection.is_empty())));
            context.insert("editorHasMultipleSelections".into(), json!(editor.selections.len() > 1));
            context.insert("selectionAnchorSet".into(), json!(editor.selection_anchor.is_some()));
            context.insert("editorTabMovesFocus".into(), json!(self.tab_focus_mode()));
            context.insert("editorColumnSelection".into(), self.setting("editor.columnSelection"));
            context.insert("foldingEnabled".into(), self.setting("editor.folding"));
            context.insert("editorLangId".into(), json!(editor.document().language_id));
            context.insert("resourceLangId".into(), json!(editor.document().language_id));
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
            context.insert("isFileSystemResource".into(), json!(editor.document().scheme == "file"));
            let path = &editor.document().path;
            context.insert("resource".into(), json!(crate::editor::uri_string(&editor.document().scheme, path)));
            context.insert("resourceScheme".into(), json!(editor.document().scheme));
            context.insert("resourceFilename".into(), json!(path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default()));
            context.insert("resourceExtname".into(), json!(path.extension().map(|ext| format!(".{}", ext.to_string_lossy())).unwrap_or_default()));
        }
        self.quick_input_context(window, &mut context);
        match self.side_bar_view {
            SideBarView::Explorer => self.explorer_context(window, &mut context),
            SideBarView::Search => self.search_context(window, &mut context),
            SideBarView::Scm => {}
            SideBarView::Extensions => self.extensions_context(window, &mut context),
            SideBarView::Debug | SideBarView::Testing => {}
        }
        self.scm_key_context(window, &mut context);
        self.debug_context(window, &mut context);
        self.keybindings_editor_context(window, &mut context);
        self.settings_editor_context(window, &mut context);
        self.search_editor_context(window, &mut context);
        self.output_context(window, &mut context);
        self.terminal_context(window, &mut context);
        self.accessibility_context(&mut context);
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
        self.commands.register("workbench.action.inspectKeyMappings", |workbench, _, window, cx| {
            let resolved = keybindings::dump_resolve_keybinding_debug_info(&workbench.keybindings.defaults, &workbench.keybindings.overrides);
            workbench.open_untitled_with_contents(&crate::keyboard_layout::dump_debug_info(&resolved), window, cx);
            Outcome::done()
        });
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
        // contextKeyService.ts `getContextKeyInfo`: the reference's RawContextKey.all() (port/tools/ref-commands).
        self.commands.register("getContextKeyInfo", |_, _, _, _| Outcome::Ready(Ok(serde_json::from_str(include_str!("../contextkey/reference-context-keys.json")).expect("valid JSON"))));
        // mainThreadLogService.ts: `loggerService.setLogLevel(parseLogLevel(level))` and `logService.getLevel()`.
        self.commands.register("_extensionTests.setLogLevel", |workbench, args, _, _| {
            if let Some(level) = args.first().and_then(Value::as_str).and_then(crate::output::log_levels::parse_log_level) {
                workbench.set_log_level(level);
            }
            Outcome::done()
        });
        self.commands.register("_extensionTests.getLogLevel", |workbench, _, _, _| Outcome::Ready(Ok(Value::from(crate::output::log_level_name(workbench.output.log_level)))));
    }
}

/// `notificationService.warn(err)` of a failed keybinding command; the caller holds the workbench, so the warning waits a tick.
pub(super) fn report_failure(outcome: Outcome, cx: &mut Context<Workbench>) {
    cx.spawn(async move |workbench, cx| {
        let result = match outcome {
            Outcome::Ready(result) => result,
            Outcome::Pending(pending) => pending.await.unwrap_or(Ok(Value::Null)),
        };
        if let Err(error) = result {
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.notify_warning(error["message"].as_str().unwrap_or_default());
                cx.notify();
            });
        }
    })
    .detach();
}

/// The settings below `prefix` as `prefix.<section>.<key>` context keys, one per value that is not an object.
fn insert_config_keys(prefix: &str, sections: serde_json::Map<String, Value>, context: &mut HashMap<String, Value>) {
    for (key, value) in sections {
        let key = format!("{prefix}.{key}");
        match value {
            Value::Object(children) => insert_config_keys(&key, children, context),
            value => drop(context.insert(key, value)),
        }
    }
}

#[cfg(test)]
mod tests;
