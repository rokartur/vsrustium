//! Keybinding dispatch (platform/keybinding/common/abstractKeybindingService.ts `_doDispatch`): every key
//! down of the window resolves against the keybindings in the context of the focused part, before the
//! focused element sees it. Multi-chord mode, its status messages and the context keys live here too.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use gpui::{Context, Keystroke, Task, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::keybindings::{self, Chord, Item, Resolution, Resolver};

/// The keybindings and the dispatch state of `AbstractKeybindingService`.
pub struct Keybindings {
    defaults: Vec<Item>,
    /// `<user data>/User/keybindings.json`.
    user_file: Option<PathBuf>,
    pub resolver: Resolver,
    /// Chords pressed so far in multi-chord mode, with their labels.
    chords: Vec<(Chord, String)>,
    chord_checker: Option<Task<()>>,
}

impl Default for Keybindings {
    fn default() -> Self {
        Keybindings { defaults: Vec::new(), user_file: None, resolver: Resolver::new(Vec::new(), Vec::new()), chords: Vec::new(), chord_checker: None }
    }
}

impl Workbench {
    /// Sets the default and user keybindings, and reloads the user's file when it changes on disk.
    pub fn load_keybindings(&mut self, defaults: Vec<Item>, user_file: PathBuf, cx: &mut Context<Self>) {
        let overrides = keybindings::user_keybindings(&user_file);
        self.keybindings.resolver = Resolver::new(defaults.clone(), overrides);
        self.keybindings.defaults = defaults;
        self.keybindings.user_file = Some(user_file.clone());
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
                let updated = workbench.update(cx, |workbench, _| {
                    workbench.keybindings.resolver = Resolver::new(workbench.keybindings.defaults.clone(), overrides);
                });
                if updated.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// `_doDispatch`: runs the command bound to the key and keeps the key from the focused element.
    pub(super) fn dispatch_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        // isKeyInComposition: the IME owns the keys while it composes.
        if self.marked_text.is_some() || self.quick_inputs.marked.is_some() {
            return;
        }
        let chord = Chord::from_keystroke(keystroke);
        if chord.is_modifier_key() {
            return;
        }
        let current: Vec<Chord> = self.keybindings.chords.iter().map(|(chord, _)| *chord).collect();
        let in_chord_mode = !current.is_empty();
        match self.keybindings.resolver.resolve(&self.key_context(window), &current, chord) {
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
            _ => {}
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
        context.insert("editorFocus".into(), json!(editor_focused));
        context.insert("editorTextFocus".into(), json!(editor_focused));
        context.insert("textInputFocus".into(), json!(editor_focused));
        context.insert("editorIsOpen".into(), json!(self.editors.active().is_some()));
        if let Some(editor) = self.editors.active() {
            context.insert("activeEditor".into(), json!("workbench.editor.files"));
            context.insert("editorReadonly".into(), json!(false));
            context.insert("editorHasSelection".into(), json!(!editor.selection.is_empty()));
            context.insert("editorLangId".into(), json!(editor.document.language_id));
            context.insert("resourceLangId".into(), json!(editor.document.language_id));
            context.insert("resourceScheme".into(), json!("file"));
            let path = &editor.document.path;
            context.insert("resourceFilename".into(), json!(path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default()));
            context.insert("resourceExtname".into(), json!(path.extension().map(|ext| format!(".{}", ext.to_string_lossy())).unwrap_or_default()));
        }
        self.quick_input_context(window, &mut context);
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

    /// The `setContext` command: `createKey(String(key), value)`.
    pub(super) fn register_context_commands(&mut self) {
        self.commands.register("workbench.action.openGlobalKeybindingsFile", |workbench, _, window, cx| workbench.open_keybindings_file(window, cx));
        self.commands.register("setContext", |workbench, args, _, _| {
            let mut args = args.into_iter();
            let key = match args.next() {
                Some(Value::String(key)) => key,
                Some(key) => key.to_string(),
                None => return Outcome::error("setContext needs a key"),
            };
            workbench.context_keys.insert(key, args.next().unwrap_or(Value::Null));
            Outcome::done()
        });
    }
}

/// `notificationService.warn(err)` of a failed keybinding command.
/// shortcut: logged, since the port has no notifications toasts.
fn report_failure(command: String, outcome: Outcome, cx: &mut Context<Workbench>) {
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
