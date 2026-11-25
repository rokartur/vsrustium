//! The editor status bar's pickers (workbench/browser/parts/editor/editorStatus.ts: the indentation picker,
//! ChangeLanguageAction, ChangeEOLAction, ChangeEncodingAction) and the indentation actions they run
//! (editor/contrib/indentation/browser/indentation.ts, linesOperations.ts `TrimTrailingWhitespaceAction`).

use std::path::Path;

use config::configuration::Overrides;
use config::indentation::creation_options;
use config::service::Target;
use files::encoding::{self, SUPPORTED_ENCODINGS};
use gpui::{Context, PromptLevel, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::explorer_actions::confirm;
use super::quick_access::show_failure;
use crate::commands::Outcome;
use crate::editor::{Indentation, OpenEditor};
use crate::languages::Languages;
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Entry, Event, Item, Kind, QuickInput};

/// `changeEditorIndentation${targetWindowId}` of the main window, whose `vscodeWindowId` is 1.
pub const CHANGE_INDENTATION: &str = "changeEditorIndentation1";
const NO_EDITOR: &str = "No text editor active at this time";

/// What accepting a row of a status picker does, by item handle.
#[derive(Clone, Debug, PartialEq)]
pub enum StatusPick {
    Command(&'static str),
    /// A `Command` an extension gave (codelens.showLensesInCurrentLine): its id and arguments.
    ExtensionCommand(String, Vec<Value>),
    TabSize { size: usize, insert_spaces: bool, display_only: bool },
    Eol(u64),
    AutoDetectLanguage,
    ConfigureFileAssociation,
    ConfigureLanguageSettings,
    Language(String),
    Associate(String),
    EncodingAction { reopen: bool },
    Encoding { id: String, reopen: bool },
    /// A user setting's new value (unicodeHighlighter.ts `ShowExcludeOptions`).
    UserSetting { key: &'static str, value: Value },
    Nothing,
}

/// The rows of the indentation picker, in `showIndentationPicker` order, before its separators.
const INDENTATION_ACTIONS: [(&str, &str); 7] = [
    ("editor.action.indentUsingSpaces", "Indent Using Spaces"),
    ("editor.action.indentUsingTabs", "Indent Using Tabs"),
    ("editor.action.changeTabDisplaySize", "Change Tab Display Size"),
    ("editor.action.detectIndentation", "Detect Indentation from Content"),
    ("editor.action.indentationToSpaces", "Convert Indentation to Spaces"),
    ("editor.action.indentationToTabs", "Convert Indentation to Tabs"),
    ("editor.action.trimTrailingWhitespace", "Trim Trailing Whitespace"),
];

impl Workbench {
    pub(super) fn register_editor_status_commands(&mut self) {
        self.commands.register(CHANGE_INDENTATION, |workbench, _, window, cx| {
            workbench.show_indentation_picker(window, cx);
            Outcome::done()
        });
        let tab_size_actions = [("editor.action.indentUsingSpaces", true, false), ("editor.action.indentUsingTabs", false, false), ("editor.action.changeTabDisplaySize", true, true)];
        for (id, insert_spaces, display_only) in tab_size_actions {
            self.commands.register(id, move |workbench, _, window, cx| {
                workbench.show_tab_size_picker(insert_spaces, display_only, window, cx);
                Outcome::done()
            });
        }
        self.commands.register("editor.action.detectIndentation", |workbench, _, _, cx| {
            if workbench.editors.active().is_none() {
                return Outcome::done();
            }
            let created = workbench.active_creation_options();
            workbench.update_active_indentation(|editor| editor.detect_indentation(created.insert_spaces, created.tab_size), cx);
            Outcome::done()
        });
        for (id, to_spaces) in [("editor.action.indentationToSpaces", true), ("editor.action.indentationToTabs", false)] {
            self.commands.register(id, move |workbench, _, _, cx| {
                let before = workbench.editors.active().map(|editor| editor.document().indentation);
                let outcome = workbench.run_on_active_editor(|editor| editor.convert_indentation(to_spaces), cx);
                workbench.send_indentation_if_changed(before);
                outcome
            });
        }
        self.commands.register("editor.action.trimTrailingWhitespace", |workbench, _, _, cx| {
            let Some(editor) = workbench.editors.active() else { return Outcome::done() };
            let document = editor.document();
            let overrides = Overrides { resource: Some(&document.path), override_identifier: Some(&document.language_id) };
            let trim_in_regex_and_strings = workbench.settings.lock().unwrap().get_value(Some("files.trimTrailingWhitespaceInRegexAndStrings"), &overrides) != Some(json!(false));
            drop(document);
            workbench.run_on_active_editor(|editor| editor.trim_trailing_whitespace(trim_in_regex_and_strings), cx)
        });
        self.commands.register("editor.action.toggleTabFocusMode", |workbench, _, _, cx| {
            // shortcut: GPUI has no screen reader announcements, so "Pressing Tab will now ..." is not read out; add one with them.
            workbench.tab_focus_toggle = Some((workbench.tab_focus_configured(), !workbench.tab_focus_mode()));
            cx.notify();
            Outcome::done()
        });
        self.commands.register("editor.action.toggleOvertypeInsertMode", |workbench, _, _, cx| {
            workbench.overtype = !workbench.overtype;
            cx.notify();
            Outcome::done()
        });
        self.commands.register("workbench.action.editor.changeEOL", |workbench, _, window, cx| {
            workbench.show_eol_picker(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.editor.changeLanguageMode", |workbench, args, window, cx| {
            // A string argument is picked as `{ label }` without an id, which `createById` turns into plain text.
            match args.first() {
                Some(Value::String(_)) => workbench.accept_status_pick(StatusPick::Language(crate::languages::PLAINTEXT.into()), window, cx),
                _ => workbench.show_language_picker(window, cx),
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.editor.changeEncoding", |workbench, _, window, cx| {
            workbench.show_encoding_action_picker(window, cx);
            Outcome::done()
        });
    }

    fn active_creation_options(&self) -> config::indentation::CreationOptions {
        let editor = self.editors.active().expect("an indentation action needs an editor");
        creation_options(&self.settings.lock().unwrap().configuration, &editor.document().path, &editor.document().language_id)
    }

    /// `model.updateOptions` on the active editor's model; the host hears the new options.
    fn update_active_indentation(&mut self, update: impl FnOnce(&mut OpenEditor), cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active_mut() else { return };
        let before = editor.document().indentation;
        update(editor);
        self.send_indentation_if_changed(Some(before));
        cx.notify();
    }

    fn send_indentation_if_changed(&self, before: Option<Indentation>) {
        let Some(editor) = self.editors.active() else { return };
        if before.is_some_and(|before| before != editor.document().indentation) {
            self.host.send_options(editor);
        }
    }

    /// `showIndentationPicker`: the indentation actions under "change view" and "convert file".
    // shortcut: no read-only editors exist in the port, so the "The active code editor is read-only." pick never shows.
    fn show_indentation_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editors.active().is_none() {
            return self.show_no_editor_pick(window, cx);
        }
        let mut rows: Vec<(Entry, StatusPick)> = INDENTATION_ACTIONS.iter().map(|(id, label)| (Entry::Item(label_item(label)), StatusPick::Command(id))).collect();
        rows.insert(3, (Entry::Separator(Some("convert file".into())), StatusPick::Nothing));
        rows.insert(0, (Entry::Separator(Some("change view".into())), StatusPick::Nothing));
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select Action".into());
        input.match_on_detail = true;
        self.show_status_picker(input, rows, window, cx);
    }

    /// `ChangeIndentationSizeAction.run`: sizes 1 to 8, the model's and the configured one described.
    fn show_tab_size_picker(&mut self, insert_spaces: bool, display_only: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let current = editor.document().indentation.tab_size;
        let configured = self.active_creation_options().tab_size;
        let rows = (1..=8)
            .map(|size| {
                let description = match (size == configured, size == current) {
                    (true, true) => Some("Configured Tab Size"),
                    (true, false) => Some("Default Tab Size"),
                    (false, true) => Some("Current Tab Size"),
                    (false, false) => None,
                };
                let item = Item { description: description.map(str::to_string), ..label_item(&size.to_string()) };
                (Entry::Item(item), StatusPick::TabSize { size, insert_spaces, display_only })
            })
            .collect();
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select Tab Size for Current File".into());
        let id = self.show_status_picker(input, rows, window, cx);
        self.activate_status_pick(id, current.min(8) - 1, window, cx);
    }

    /// `ChangeEOLAction.run`.
    fn show_eol_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else {
            return self.show_no_editor_pick(window, cx);
        };
        let selected = if editor.document().eol == "\n" { 0 } else { 1 };
        let rows = vec![(Entry::Item(label_item("LF")), StatusPick::Eol(0)), (Entry::Item(label_item("CRLF")), StatusPick::Eol(1))];
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select End of Line Sequence".into());
        let id = self.show_status_picker(input, rows, window, cx);
        self.activate_status_pick(id, selected, window, cx);
    }

    /// `ChangeLanguageAction.run` without an argument.
    fn show_language_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else {
            return self.show_no_editor_pick(window, cx);
        };
        let document = editor.document();
        let current_name = self.languages.name(&document.language_id);
        let mut rows = Vec::new();
        // Auto Detect only for a document with text.
        if document.lines.len() > 1 || !document.lines[0].is_empty() {
            rows.push((Entry::Item(label_item("Auto Detect")), StatusPick::AutoDetectLanguage));
        }
        // An untitled document without an associated file has no settings to configure (e.g. "Untitled-1").
        if document.scheme != "untitled" || editor.text_model().associated_file {
            rows.push((Entry::Item(label_item(&format!("Configure File Association for '{}'...", association_name(&document.path)))), StatusPick::ConfigureFileAssociation));
            rows.push((Entry::Item(label_item(&format!("Configure '{current_name}' language based settings..."))), StatusPick::ConfigureLanguageSettings));
        }
        rows.push((Entry::Separator(Some("languages (identifier)".into())), StatusPick::Nothing));
        for (name, id) in self.languages.sorted_names() {
            let description = if name == current_name { format!("({id}) - Configured Language") } else { format!("({id})") };
            let item = Item { description: Some(description), language_icon: Some(id.to_string()), ..label_item(name) };
            rows.push((Entry::Item(item), StatusPick::Language(id.to_string())));
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select Language Mode".into());
        input.match_on_description = true;
        drop(document);
        self.show_status_picker(input, rows, window, cx);
    }

    /// `configureFileAssociation`: the languages, the one the file name maps to described.
    fn show_file_association_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let base = editor.document().path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let associations = self.settings.lock().unwrap().get_value(Some("files.associations"), &Overrides::default()).unwrap_or_default();
        let current = Languages::configured_language(&associations, Path::new(&base)).or_else(|| self.languages.guess_language(Path::new(&base), "").map(str::to_string));
        let rows = self
            .languages
            .sorted_names()
            .into_iter()
            .map(|(name, id)| {
                let description = (current.as_deref() == Some(id)).then(|| "Current Association".to_string());
                (Entry::Item(Item { description, language_icon: Some(id.to_string()), ..label_item(name) }), StatusPick::Associate(id.to_string()))
            })
            .collect();
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some(format!("Select Language Mode to Associate with '{}'", association_name(&editor.document().path)));
        self.show_status_picker(input, rows, window, cx);
    }

    /// `ChangeEncodingAction.run` up to the action: an untitled document saves with the encoding.
    fn show_encoding_action_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else {
            return self.show_no_editor_pick(window, cx);
        };
        if editor.document().scheme == "untitled" {
            return self.show_encoding_picker(false, window, cx);
        }
        let rows = vec![(Entry::Item(label_item("Reopen with Encoding")), StatusPick::EncodingAction { reopen: true }), (Entry::Item(label_item("Save with Encoding")), StatusPick::EncodingAction { reopen: false })];
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select Action".into());
        input.match_on_detail = true;
        self.show_status_picker(input, rows, window, cx);
    }

    /// The encodings of `ChangeEncodingAction`: the configured one first, then by order, the guessed one on top.
    fn show_encoding_picker(&mut self, reopen: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let document = editor.document();
        let settings = self.settings.lock().unwrap();
        let resource = (document.scheme == "file").then_some(document.path.as_path());
        let configured = encoding::configured(&settings, resource);
        // `readStream` with `autoGuessEncoding`.
        // shortcut: the guess is the BOM and UTF-16 detection only; jschardet is not ported (files crate `detect`).
        let guessed = resource.and_then(|path| std::fs::read(path).ok()).map(|bytes| encoding::read_encoding(&settings, resource, None, encoding::detect(&bytes).encoding));
        drop(settings);
        let mut sorted: Vec<_> = SUPPORTED_ENCODINGS.iter().collect();
        sorted.sort_by_key(|supported| supported.id != configured);
        sorted.retain(|supported| !(guessed.as_deref() == Some(supported.id) && supported.id != configured) && !(reopen && supported.encode_only));
        let direct = sorted.iter().position(|supported| supported.id == document.encoding);
        let alias = sorted.iter().position(|supported| supported.alias == Some(document.encoding.as_str()));
        let mut rows: Vec<(Entry, StatusPick)> = sorted
            .iter()
            .map(|supported| {
                let item = Item { description: Some(supported.id.to_string()), ..label_item(supported.label_long) };
                (Entry::Item(item), StatusPick::Encoding { id: supported.id.to_string(), reopen })
            })
            .collect();
        let mut active = direct.or(alias);
        if let Some(guessed) = guessed.filter(|guessed| *guessed != configured).and_then(|guessed| SUPPORTED_ENCODINGS.iter().find(|supported| supported.id == guessed)) {
            let item = Item { description: Some("Guessed from content".into()), ..label_item(guessed.label_long) };
            rows.insert(0, (Entry::Separator(None), StatusPick::Nothing));
            rows.insert(0, (Entry::Item(item), StatusPick::Encoding { id: guessed.id.to_string(), reopen }));
            active = active.map(|index| index + 2);
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some(if reopen { "Select File Encoding to Reopen File" } else { "Select File Encoding to Save with" }.into());
        drop(document);
        let id = self.show_status_picker(input, rows, window, cx);
        if let Some(active) = active {
            self.activate_status_pick(id, active, window, cx);
        }
    }

    /// The single "No text editor active at this time" pick.
    fn show_no_editor_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_status_picker(QuickInput::new(0, Kind::QuickPick), vec![(Entry::Item(label_item(NO_EDITOR)), StatusPick::Nothing)], window, cx);
    }

    /// Shows a picker whose items run `picks`; separators take a pick so the handles stay row indices.
    pub(super) fn show_status_picker(&mut self, mut input: QuickInput, rows: Vec<(Entry, StatusPick)>, window: &mut Window, cx: &mut Context<Self>) -> i64 {
        let mut entries = Vec::new();
        let mut picks = Vec::new();
        for (entry, pick) in rows {
            if let Entry::Item(mut item) = entry {
                item.handle = picks.len() as i64;
                entries.push(Entry::Item(item));
                picks.push(pick);
            } else {
                entries.push(entry);
            }
        }
        input.set_items(entries);
        let id = self.quick_inputs.add(input, Owner::EditorStatus(picks));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
        id
    }

    /// `activeItem`: the item at `handle` starts focused.
    fn activate_status_pick(&mut self, id: i64, handle: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = self.quick_inputs.get_mut(id) {
            input.set_active_items(&[handle as i64]);
        }
        self.flush_quick_input_events(window, cx);
    }

    pub fn editor_status_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Accept(_) => {
                let Some(input) = self.quick_inputs.get(id) else { return };
                let Some(&handle) = input.selected_handles().first() else { return };
                let Some(Owner::EditorStatus(picks)) = self.quick_inputs.owner(id) else { return };
                let pick = picks[handle as usize].clone();
                self.hide_quick_input_by_id(id, window, cx);
                self.accept_status_pick(pick, window, cx);
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }

    fn accept_status_pick(&mut self, pick: StatusPick, window: &mut Window, cx: &mut Context<Self>) {
        match pick {
            StatusPick::Command(command) => {
                let outcome = self.execute_command(command, Vec::new(), window, cx);
                show_failure(command.into(), outcome, window, cx);
            }
            StatusPick::ExtensionCommand(id, args) => self.run_code_lens_command(&id, args, window, cx),
            StatusPick::TabSize { size, insert_spaces, display_only } => self.update_active_indentation(
                |editor| {
                    let indentation = editor.document().indentation;
                    editor.set_indentation(match display_only {
                        true => Indentation { tab_size: size, ..indentation },
                        false => Indentation { tab_size: size, indent_size: Some(size), insert_spaces },
                    });
                },
                cx,
            ),
            StatusPick::Eol(sequence) => {
                self.run_on_active_editor(
                    |editor| {
                        editor.push_undo_stop();
                        let change = editor.push_eol(sequence);
                        editor.push_undo_stop();
                        change
                    },
                    cx,
                );
            }
            StatusPick::AutoDetectLanguage => {
                let Some(editor) = self.editors.active() else { return };
                let (scheme, path) = (editor.document().scheme.clone(), editor.document().path.clone());
                let guessed = self.languages.guess_language(&path, &editor.document().lines[0]).map(str::to_string);
                if let Some(language_id) = guessed {
                    return self.set_active_language(&language_id, cx);
                }
                let detection = self.detect_language(&scheme, &path);
                cx.spawn(async move |workbench, cx| {
                    let detection = detection.await;
                    let _ = workbench.update(cx, |workbench, cx| {
                        let Some(language_id) = workbench.detected_language(detection) else { return };
                        if workbench.editors.active().is_some_and(|editor| editor.document().scheme == scheme && editor.document().path == path) {
                            workbench.set_active_language(&language_id, cx);
                        }
                    });
                })
                .detach();
            }
            StatusPick::Language(language_id) => self.set_active_language(&language_id, cx),
            StatusPick::ConfigureFileAssociation => self.show_file_association_picker(window, cx),
            StatusPick::ConfigureLanguageSettings => {
                let Some(editor) = self.editors.active() else { return };
                // `preferencesService.openUserSettings` runs as the command the preferences contribution registers.
                let args = vec![json!({ "revealSetting": { "key": format!("[{}]", editor.document().language_id), "edit": true } })];
                let outcome = self.execute_command("workbench.action.openSettingsJson", args, window, cx);
                show_failure("Preferences: Open User Settings (JSON)".into(), outcome, window, cx);
            }
            StatusPick::Associate(language_id) => self.write_file_association(&language_id, cx),
            StatusPick::EncodingAction { reopen } => self.show_encoding_picker(reopen, window, cx),
            StatusPick::Encoding { id, reopen } => self.set_active_encoding(id, reopen, window, cx),
            StatusPick::UserSetting { key, value } => {
                let mut settings = self.settings.lock().unwrap();
                match settings.update_value(key, Some(&value), Target::User, None, &[]) {
                    Ok(change) if !change.is_empty() => self.host.send_configuration_change(&settings, &change),
                    Ok(_) => {}
                    Err(error) => eprintln!("writing {key}: {error}"),
                }
                drop(settings);
                cx.notify();
            }
            StatusPick::Nothing => {}
        }
    }

    /// `setLanguageId` of the active editor's input.
    pub(super) fn set_active_language(&mut self, language_id: &str, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let (scheme, path) = (editor.document().scheme.clone(), editor.document().path.clone());
        self.text_files.preferred_languages.insert((scheme.clone(), path.clone()));
        self.set_model_language(&scheme, &path, language_id, cx);
    }

    /// `TextModel.setLanguage`: the host hears the language and, for the visible editor, new indentation options.
    pub(super) fn set_model_language(&mut self, scheme: &str, path: &Path, language_id: &str, cx: &mut Context<Self>) {
        let model = self.editors.model_mut(scheme, path).expect("the caller found the model");
        if model.document().language_id == language_id {
            return;
        }
        let before = model.document().indentation;
        model.set_language(language_id, &self.languages, &self.settings.lock().unwrap());
        self.host.send("ExtHostDocuments", "$acceptModelLanguageChanged", vec![model.document().uri(), json!(language_id)]);
        let active = self.editors.active().is_some_and(|editor| editor.document().scheme == scheme && editor.document().path == path);
        if active {
            self.send_indentation_if_changed(Some(before));
            self.refresh_language_status(cx);
            self.linked_editing_reinitialize(true, cx);
        }
        cx.notify();
    }

    /// `configureFileAssociation`'s write: `*.ext`, or the file name when it has no extension or starts with a dot,
    /// into workspace settings when they already associate that key, else user settings.
    fn write_file_association(&mut self, language_id: &str, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let key = association_key(&editor.document().path);
        let mut settings = self.settings.lock().unwrap();
        let inspected = settings.configuration.inspect("files.associations", &Overrides::default());
        let in_workspace = inspected.workspace.value.as_ref().is_some_and(|value| value.get(&key).is_some_and(|value| !value.is_null() && value != false && value != ""));
        let (target, layer) = if in_workspace { (Target::Workspace, inspected.workspace.value) } else { (Target::User, inspected.user_local.value) };
        let mut associations = layer.filter(Value::is_object).unwrap_or_else(|| json!({}));
        associations[key] = json!(language_id);
        match settings.update_value("files.associations", Some(&associations), target, None, &[]) {
            Ok(change) if !change.is_empty() => {
                self.host.send_configuration_change(&settings, &change);
                drop(settings);
                self.files_associations_changed(cx);
            }
            Ok(_) => {}
            Err(error) => eprintln!("writing files.associations: {error}"),
        }
    }

    /// The end of `ChangeEncodingAction.run`: reopening a dirty editor asks to revert it first.
    fn set_active_encoding(&mut self, encoding: String, reopen: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.editors.active_index() else { return };
        if reopen && self.editors.editors[index].is_dirty() {
            cx.spawn_in(window, async move |workbench, cx| {
                let message = "Do you want to revert the active text editor and reopen with a different encoding?".to_string();
                if !confirm(&workbench, cx, PromptLevel::Warning, message, Some("This will discard any unsaved changes.".into()), "Discard Changes and Reopen").await {
                    return;
                }
                let _ = workbench.update_in(cx, |workbench, window, cx| {
                    workbench.revert_editor(index, cx);
                    workbench.set_editor_encoding(index, &encoding, reopen, window, cx);
                });
            })
            .detach();
            return;
        }
        self.set_editor_encoding(index, &encoding, reopen, window, cx);
    }

    /// `setEncoding(encoding, EncodingMode.Decode)` reads the file again in the encoding; `Encode` saves with it.
    fn set_editor_encoding(&mut self, index: usize, encoding: &str, reopen: bool, window: &mut Window, cx: &mut Context<Self>) {
        let document = self.editors.editors[index].document();
        let (uri, scheme, path) = (document.uri(), document.scheme.clone(), document.path.clone());
        if reopen {
            drop(document);
            if let Err(error) = self.set_model_encoding(&uri, &scheme, &path, encoding, cx) {
                show_failure("Change File Encoding".into(), Outcome::error(error), window, cx);
            }
            return;
        }
        drop(document);
        let editor = &mut self.editors.editors[index];
        if editor.document().encoding == encoding {
            return;
        }
        editor.document_mut().encoding = encoding.to_string();
        self.host.send("ExtHostDocuments", "$acceptEncodingChanged", vec![uri, json!(encoding)]);
        cx.notify();
        if scheme != "untitled" {
            drop(self.save_editor(index, cx));
        }
    }

    fn tab_focus_configured(&self) -> bool {
        self.setting("editor.tabFocusMode") == json!(true)
    }

    /// `TabFocus` as editorStatus.ts `TabFocusMode` drives it: `editor.tabFocusMode` at start and on each change of
    /// the setting, Toggle Tab Key Moves Focus in between.
    pub(super) fn tab_focus_mode(&self) -> bool {
        let configured = self.tab_focus_configured();
        match self.tab_focus_toggle {
            Some((configured_then, mode)) if configured_then == configured => mode,
            _ => configured,
        }
    }

    /// The browser's Tab and Shift+Tab where nothing handles them: with tab focus mode on, Tab leaves the editor's
    /// text for the status bar, the next focusable part of the workbench, and Shift+Tab comes back.
    pub(super) fn move_tab_focus(&mut self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.editor_focus.is_focused(window) && self.tab_focus_mode() {
            if !backwards {
                window.focus(&self.status_bar_focus, cx);
            }
            return true;
        }
        if self.status_bar_focus.is_focused(window) {
            if backwards {
                window.focus(&self.editor_focus, cx);
            }
            return true;
        }
        false
    }
}

pub(super) fn label_item(label: &str) -> Item {
    Item { label: label.to_string(), pickable: true, ..Item::default() }
}

/// `extname(resource) || basename(resource)`.
fn association_name(path: &Path) -> String {
    let base = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    extname(&base).map_or(base.clone(), str::to_string)
}

/// The `files.associations` key `configureFileAssociation` writes.
fn association_key(path: &Path) -> String {
    let base = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    match extname(&base) {
        Some(extension) if !base.starts_with('.') => format!("*{extension}"),
        _ => base,
    }
}

/// `resources.extname`: from the last dot of the file name, empty for a name without one.
fn extname(base: &str) -> Option<&str> {
    base.rfind('.').map(|dot| &base[dot..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use exthost::scan::{ScannedExtension, scan_extension};
    use gpui::TestAppContext;

    use crate::host_sync::HostSync;
    use crate::theme::load_theme;

    #[gpui::test]
    fn status_pickers_change_indentation_eol_language_and_encoding(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-editor-status-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "\tx  \n\ty\n").unwrap();
        let rust = ScannedExtension { id: "port.rust".into(), location: dir.clone(), description: json!({ "contributes": { "languages": [{ "id": "rust", "aliases": ["Rust"], "extensions": [".rs"] }] } }) };
        let (sender, receiver) = std::sync::mpsc::channel();
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[rust]), crate::default_settings(), HostSync::new(sender), cx);
            workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
            workbench.open_file(file.clone(), true, window, cx);
            workbench
        });
        cx.run_until_parked();
        receiver.try_iter().count();
        let run = |command: &str, cx: &mut gpui::VisualTestContext| {
            workbench.update_in(cx, |workbench, window, cx| workbench.execute_command(command, vec![], window, cx));
            cx.run_until_parked();
        };
        let labels = |cx: &mut gpui::VisualTestContext| {
            workbench.read_with(cx, |workbench, _| {
                let input = workbench.quick_inputs.current().unwrap();
                input.order.iter().map(|row| &input.rows[*row]).map(|row| format!("{}{}", row.separator.as_ref().map(|label| format!("--{label}--")).unwrap_or_default(), row.item.label)).collect::<Vec<_>>()
            })
        };
        let pick = |label: &str, cx: &mut gpui::VisualTestContext| {
            workbench.update_in(cx, |workbench, window, cx| {
                let input = workbench.quick_inputs.find_mut(|owner| matches!(owner, Owner::EditorStatus(_))).unwrap();
                let handle = input.rows.iter().find(|row| row.item.label == label).unwrap().item.handle;
                input.set_active_items(&[handle]);
                workbench.flush_quick_input_events(window, cx);
            });
            cx.simulate_keystrokes("enter");
            cx.run_until_parked();
        };
        let document = |cx: &mut gpui::VisualTestContext| {
            workbench.read_with(cx, |workbench, _| {
                let document = &workbench.editors.active().unwrap().document();
                (document.lines.join("|"), document.indentation.insert_spaces, document.eol, document.language_id.clone(), document.encoding.clone())
            })
        };
        assert_eq!(document(cx), ("\tx  |\ty|".into(), false, "\n".into(), "plaintext".into(), "utf8".into()));

        run(CHANGE_INDENTATION, cx);
        assert_eq!(
            labels(cx),
            [
                "--change view--Indent Using Spaces",
                "Indent Using Tabs",
                "Change Tab Display Size",
                "--convert file--Detect Indentation from Content",
                "Convert Indentation to Spaces",
                "Convert Indentation to Tabs",
                "Trim Trailing Whitespace"
            ]
        );
        pick("Convert Indentation to Spaces", cx);
        assert_eq!(document(cx).0, "    x  |    y|");
        run(CHANGE_INDENTATION, cx);
        pick("Trim Trailing Whitespace", cx);
        assert_eq!(document(cx).0, "    x|    y|");
        run("editor.action.indentUsingTabs", cx);
        assert_eq!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().unwrap().placeholder.clone()).as_deref(), Some("Select Tab Size for Current File"));
        pick("2", cx);
        let indentation = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().indentation);
        assert_eq!((indentation.tab_size, indentation.indent_size(), indentation.insert_spaces), (2, 2, false));

        run("workbench.action.editor.changeEOL", cx);
        assert_eq!(labels(cx), ["LF", "CRLF"]);
        pick("CRLF", cx);
        assert_eq!(document(cx).2, "\r\n");

        run("workbench.action.editor.changeLanguageMode", cx);
        assert!(labels(cx).contains(&"--languages (identifier)--Plain Text".to_string()));
        pick("Rust", cx);
        assert_eq!(document(cx).3, "rust");
        let methods: Vec<_> = receiver.try_iter().map(|message| message.method).collect();
        assert!(methods.contains(&"$acceptModelLanguageChanged"));
        workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("workbench.action.editor.changeLanguageMode", vec![json!("rust")], window, cx));
        assert_eq!(document(cx).3, "plaintext", "a string argument has no id, so createById picks plain text");
        run("workbench.action.editor.changeLanguageMode", cx);
        pick("Rust", cx);

        cx.simulate_keystrokes("cmd-s");
        run("workbench.action.editor.changeEncoding", cx);
        assert_eq!(labels(cx), ["Reopen with Encoding", "Save with Encoding"]);
        pick("Save with Encoding", cx);
        pick("UTF-16 LE", cx);
        assert_eq!(document(cx).4, "utf16le");
        assert_eq!(std::fs::read(&file).unwrap()[..4], [0xFF, 0xFE, b' ', 0]);
        run("workbench.action.editor.changeEncoding", cx);
        pick("Reopen with Encoding", cx);
        pick("UTF-8", cx);
        assert_eq!(document(cx).4, "utf8");
        assert!(document(cx).0.starts_with("\u{FFFD}\u{FFFD} \0"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn association_keys_like_configure_file_association() {
        assert_eq!(association_key(Path::new("/a/main.rs")), "*.rs");
        assert_eq!(association_key(Path::new("/a/Dockerfile")), "Dockerfile");
        assert_eq!(association_key(Path::new("/a/.gitignore")), ".gitignore");
        assert_eq!(association_name(Path::new("/a/.gitignore")), ".gitignore");
        assert_eq!(association_name(Path::new("/a/Dockerfile")), "Dockerfile");
    }
}
