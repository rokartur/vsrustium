//! The preferences service (src/vs/workbench/services/preferences/browser/preferencesService.ts) behind the
//! settings JSON commands of preferences.contribution.ts, and the read-only `vscode://defaultsettings` documents of
//! settingsFilesystemProvider.ts.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use config::configuration::Overrides;
use config::json::{NodeType, ParseOptions, Segment, parse_tree};
use config::json_edit::{FormattingOptions, set_property};
use config::model::is_override_key;
use config::service::Target;
use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::host_requests::uri_parts;
use crate::commands::Outcome;
use crate::default_settings::{default_settings_content, raw_default_settings};
use crate::editor::{EditReason, Position, Selection, TextDocument, VerticalReveal};
use crate::keybindings::{self, Item as KeybindingItem};
use crate::keybindings_editor;
use crate::quick_input::{Entry, Item, Kind, QuickInput, path_label};

pub(super) mod edit_setting;
mod unsupported_settings;

#[cfg(test)]
mod tests;

/// `defaultSettingsRawResource`, as `uri_parts` keys a `vscode://defaultsettings/...` URI.
const RAW_DEFAULT_SETTINGS: &str = "//defaultsettings/defaultSettings.jsonc";
/// `defaultKeybindingsResource`.
const DEFAULT_KEYBINDINGS: &str = "//defaultsettings/keybindings.json";

/// preferences.ts `getDefaultSettingsResource`: the default settings beside a split settings.json of `target`.
fn default_settings_resource(target: Target) -> &'static str {
    match target {
        Target::Workspace => "//defaultsettings/workspaceSettings.json",
        Target::WorkspaceFolder => "//defaultsettings/resourceSettings.json",
        _ => "//defaultsettings/settings.json",
    }
}

impl Workbench {
    /// preferencesContribution.ts `createEditorInput`: the default settings beside a settings file the split settings
    /// editor opens: the user's, a single folder's, or a folder's of a multi-root workspace.
    pub(super) fn split_settings_secondary(&self, path: &Path) -> Option<&'static str> {
        match unsupported_settings::settings_file_target(&self.settings.lock().unwrap(), path)? {
            (_, true) => None,
            (target, false) => Some(default_settings_resource(target)),
        }
    }
}

/// `sanitizeOpenSettingsArgs`: the query and the setting to reveal (`revealSetting.key`, `.edit`).
// shortcut: `openToSide` opens in the active group; editor groups are milestone 11.
#[derive(Default)]
struct OpenSettingsOptions {
    query: Option<String>,
    reveal_setting: Option<(String, bool)>,
}

impl OpenSettingsOptions {
    fn sanitize(args: &[Value]) -> Self {
        let args = args.first().filter(|args| args.is_object()).cloned().unwrap_or_default();
        let reveal = &args["revealSetting"];
        OpenSettingsOptions {
            query: args["query"].as_str().map(str::to_string),
            reveal_setting: reveal["key"].as_str().map(|key| (key.to_string(), reveal["edit"].as_bool().unwrap_or(false))),
        }
    }
}

impl Workbench {
    pub(super) fn register_preferences_commands(&mut self) {
        self.commands.register("workbench.action.openSettingsJson", |workbench, args, window, cx| workbench.open_settings_json(Target::UserLocal, None, OpenSettingsOptions::sanitize(&args), window, cx));
        // `openApplicationSettings`: the default profile's settings.json, the user settings in the default profile.
        self.commands.register("workbench.action.openApplicationSettingsJson", |workbench, args, window, cx| {
            workbench.open_settings_json(Target::Application, None, OpenSettingsOptions::sanitize(&args), window, cx)
        });
        self.commands.register("workbench.action.openWorkspaceSettingsFile", |workbench, args, window, cx| {
            if workbench.settings.lock().unwrap().workspace.is_none() {
                // shortcut: logged, since the port has no notification toasts.
                eprintln!("Open a folder or workspace first to create workspace or folder settings.");
                return Outcome::done();
            }
            workbench.open_settings_json(Target::Workspace, None, OpenSettingsOptions::sanitize(&args), window, cx)
        });
        // preferences.contribution.ts `_workbench.action.openFolderSettings`: the Explorer root's folder settings.
        self.commands.register("_workbench.action.openFolderSettings", |workbench, args, window, cx| {
            let folder = match args.first().map(uri_parts) {
                Some(Ok((_, path))) => path,
                _ => return Outcome::error("Folder resource required."),
            };
            workbench.open_settings_editor(None, Some((Target::WorkspaceFolder, Some(folder))), window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.openFolderSettingsFile", |workbench, _, window, cx| {
            let Some(picked) = workbench.pick_workspace_folder(&Value::Null, window, cx) else { return Outcome::done() };
            cx.spawn_in(window, async move |workbench, cx| {
                let Some(index) = picked.await.ok().and_then(Result::ok).and_then(|picked| picked.as_u64()) else { return };
                let _ = workbench.update_in(cx, |workbench, window, cx| {
                    let workspace = workbench.settings.lock().unwrap().workspace.clone();
                    let folder = workspace.and_then(|workspace| workspace.folders.get(index as usize).map(|folder| folder.path.clone()));
                    let outcome = workbench.open_settings_json(Target::WorkspaceFolder, folder.as_deref(), OpenSettingsOptions::default(), window, cx);
                    super::quick_access::show_failure("Preferences: Open Folder Settings (JSON)".into(), outcome, window, cx);
                });
            })
            .detach();
            Outcome::done()
        });
        self.commands.register("_workbench.pickWorkspaceFolder", |workbench, args, window, cx| {
            let options = args.into_iter().next().unwrap_or_default();
            let Some(picked) = workbench.pick_workspace_folder(&options, window, cx) else { return Outcome::done() };
            let folders = workbench.settings.lock().unwrap().workspace.as_ref().map(|workspace| workspace.folders.clone()).unwrap_or_default();
            let (sender, receiver) = futures::channel::oneshot::channel();
            cx.background_executor()
                .spawn(async move {
                    let index = picked.await.ok().and_then(Result::ok).and_then(|picked| picked.as_u64()).map(|index| index as usize);
                    let folder = index.and_then(|index| Some((index, folders.get(index)?)));
                    let folder = folder.map_or(Value::Null, |(index, folder)| json!({ "uri": crate::editor::uri_value("file", &folder.path), "name": folder.name, "index": index }));
                    let _ = sender.send(Ok(folder));
                })
                .detach();
            Outcome::Pending(receiver)
        });
        self.commands.register("workbench.action.openRawDefaultSettings", |workbench, _, window, cx| workbench.open_preferences_resource(RAW_DEFAULT_SETTINGS, window, cx));
        self.commands.register("workbench.action.openDefaultKeybindingsFile", |workbench, _, window, cx| workbench.open_preferences_resource(DEFAULT_KEYBINDINGS, window, cx));
    }

    /// The Default Keybindings editor in the active group, as `openDefaultKeybindingsFile` opens it.
    pub(super) fn open_default_keybindings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        drop(self.open_preferences_resource(DEFAULT_KEYBINDINGS, window, cx));
    }

    /// `openEditor` of a read-only `vscode://defaultsettings` resource: its open editor, its model, or a new model.
    pub(super) fn open_preferences_resource(&mut self, resource: impl AsRef<Path>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let path = resource.as_ref().to_path_buf();
        if let Some(index) = self.editors.editors.iter().position(|editor| editor.document().is("vscode", &path)) {
            self.editors.activate(index);
        } else {
            let index = match self.editors.models.iter().position(|model| model.document().is("vscode", &path)) {
                Some(index) => index,
                None => {
                    let text = self.preferences_content(&path).expect("a defaultsettings resource");
                    let document = TextDocument::from_text("vscode", &path, &text, files::encoding::UTF8.into(), None, &self.languages, &self.settings.lock().unwrap());
                    self.editors.open_model(document)
                }
            };
            self.editors.show_model(index, true);
        }
        window.focus(&self.editor_focus, cx);
        self.editors_changed(cx);
        Outcome::done()
    }

    /// `PreferencesService.resolveModel` for a `vscode://defaultsettings` path (`//authority/path` as `uri_parts` keys it).
    pub(super) fn preferences_content(&self, path: &Path) -> Option<String> {
        match path.to_str()? {
            RAW_DEFAULT_SETTINGS => Some(raw_default_settings(&self.settings.lock().unwrap().registry)),
            DEFAULT_KEYBINDINGS => Some(self.default_keybindings_content()),
            // `getDefaultSettingsContent` of a split JSON editor's default half.
            path => {
                let target = [Target::UserLocal, Target::Workspace, Target::WorkspaceFolder].into_iter().find(|target| default_settings_resource(*target) == path)?;
                Some(default_settings_content(&self.settings.lock().unwrap().registry, target))
            }
        }
    }

    /// `defaultKeybindingsContents`: the header, then `getDefaultKeybindingsContent` of the workbench's defaults.
    fn default_keybindings_content(&self) -> String {
        format!("// Override key bindings by placing them into your key bindings file.\n{}", self.keybindings_content(&self.keybindings.defaults))
    }

    /// keybindingsExport.contribution.ts `exportDefaultKeybindingsAndQuit`: writes `doc.keybindings.osx.json` into
    /// `folder`. shortcut: the Windows and Linux files are not written; the port has only the macOS default table.
    pub fn export_default_keybindings(&self, folder: &Path) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(folder)?;
        let path = folder.join("doc.keybindings.osx.json");
        std::fs::write(&path, self.keybindings_content(&self.with_user_keybindings_resource(keybindings::exported_default_keybindings(&self.extensions))))?;
        Ok(path)
    }

    /// `getDefaultKeybindingsContent`: `defaults` as `KeybindingIO.writeKeybindingItem` writes them, then the commands
    /// none of them binds (`getAllUnboundCommands`).
    fn keybindings_content(&self, defaults: &[KeybindingItem]) -> String {
        let bound: HashSet<&str> = defaults.iter().map(|item| item.command.as_str()).filter(|command| !command.is_empty() && !command.starts_with('-')).collect();
        let registered: Vec<&str> = self.commands.ids().collect();
        let mut unbound = keybindings_editor::unbound_command_ids(&self.quick_access.menu, &registered, &bound);
        unbound.sort();
        let mut lines = vec!["[".to_string()];
        for (index, item) in defaults.iter().enumerate() {
            let mut line = write_keybinding_item(item);
            line.push_str(if index + 1 < defaults.len() { "," } else { "" });
            lines.push(line);
        }
        lines.push("]".into());
        lines.push(String::new());
        lines.push(String::new());
        lines.push(format!("// Here are other available commands: \n// - {}", unbound.join("\n// - ")));
        lines.join("\n")
    }

    /// `open` with `jsonEditor: true`, then `openSettingsJson`: a query naming a setting reveals it.
    fn open_settings_json(&mut self, target: Target, folder: Option<&Path>, mut options: OpenSettingsOptions, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        if let Some(query) = options.query.take().filter(|_| options.reveal_setting.is_none()) {
            let query = query.trim();
            let key = match query.strip_prefix("@id:") {
                Some(id) => Some(id.trim().to_string()),
                None => self.settings.lock().unwrap().registry.property(query).map(|_| query.to_string()),
            };
            options.reveal_setting = key.map(|key| (key, false));
        }
        let created = self.settings.lock().unwrap().create_settings_if_not_exists(target, folder);
        let path = match created {
            Some(Ok(path)) => path,
            Some(Err(error)) => return Outcome::error(error),
            None => return Outcome::done(),
        };
        self.open_preferences_file(path, window, cx);
        // `doOpenSplitJSON`: the default settings of the target beside the file, read-only (`SplitSettingsEditorInput`).
        if self.setting("workbench.settings.useSplitJSON") == json!(true) || self.setting("workbench.settings.openDefaultSettings") == json!(true) {
            self.show_side_by_side(Path::new(default_settings_resource(target)), window, cx);
        }
        if let Some((key, edit)) = options.reveal_setting {
            self.reveal_setting(&key, edit, cx);
        }
        Outcome::done()
    }

    /// `revealSetting` in the active settings editor: the key, or with `edit` its value, added with its default when
    /// missing (`getPositionToReveal`).
    // shortcut: `triggerSuggest` after an edit is left out; suggestions are milestone 13.
    fn reveal_setting(&mut self, key: &str, edit: bool, cx: &mut Context<Self>) {
        let settings = self.settings.lock().unwrap();
        let schema = settings.registry.property(key).map(|property| property.schema.clone());
        let is_override = is_override_key(key);
        if schema.is_none() && !is_override {
            return;
        }
        let value_type = schema.as_ref().map_or(json!("object"), |schema| schema["type"].clone());
        let value_type = value_type.as_array().and_then(|types| types.first()).unwrap_or(&value_type).as_str().unwrap_or_default().to_string();
        let default_value = match value_type.as_str() {
            "object" | "array" => settings.configuration.inspect(key, &Overrides::default()).default_value,
            other => Some(default_of_type(other)),
        };
        let default_value = default_value.or_else(|| is_override.then(|| json!({})));
        let in_workspace_file = settings.workspace.as_ref().and_then(|workspace| workspace.file.as_deref()) == self.editors.active().map(|editor| editor.document().path.clone()).as_deref();
        drop(settings);
        let Some(editor) = self.editors.active() else { return };
        let mut path = vec![Segment::Key(key.to_string())];
        if in_workspace_file {
            path.insert(0, Segment::Key("settings".into()));
        }
        let mut setting = setting_ranges(&editor.document().lines, &path);
        if setting.is_none()
            && edit
            && let Some(value) = default_value
        {
            let text = editor.document().lines.join("\n");
            let indentation = editor.document().indentation;
            let options = FormattingOptions { tab_size: indentation.tab_size, insert_spaces: indentation.insert_spaces, eol: editor.document().eol.to_string() };
            let edits: Vec<_> = set_property(&text, &path, Some(&value), &options)
                .into_iter()
                .map(|edit| (position_at(&text, edit.offset), position_at(&text, edit.offset + edit.length), edit.content.replace("\r\n", "\n"), false))
                .collect();
            self.run_indenting(|editor, _| editor.execute_edits(edits, None, EditReason::Unknown("jsonEditing")).into_iter().collect(), cx);
            setting = setting_ranges(&self.editors.active().expect("still active").document().lines, &path);
        }
        let Some(setting) = setting else { return };
        let position = match edit {
            true if setting.value_is_container => {
                let after_bracket = Position::new(setting.value_start.line, setting.value_start.column + 1);
                self.set_active_position(after_bracket);
                self.run_indenting(|editor, auto| editor.line_break_insert(auto).into_iter().collect(), cx);
                let document = self.editors.active().expect("still active").document();
                let lines = &document.lines;
                let next = after_bracket.line + 1;
                let first_non_whitespace = lines[next].chars().position(|c| !c.is_whitespace());
                match first_non_whitespace {
                    Some(column) => {
                        drop(document);
                        self.set_active_position(Position::new(next, column));
                        self.run_indenting(|editor, auto| editor.line_break_insert(auto).into_iter().collect(), cx);
                        let lines = &self.editors.active().expect("still active").document().lines;
                        Position::new(next, lines[next].chars().count())
                    }
                    None => Position::new(next, lines[next].chars().count()),
                }
            }
            true => setting.value_end,
            false => setting.key_start,
        };
        self.set_active_position(position);
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("still active");
        editor.reveal_range(position, position, VerticalReveal::NearTop, viewport);
        self.host.send_selection(editor, "api");
        self.editor_scrolled(cx);
    }

    fn set_active_position(&mut self, position: Position) {
        self.editors.active_mut().expect("an active settings editor").set_selections(vec![Selection::caret(position)]);
    }

    /// workspaceCommands.ts `_workbench.pickWorkspaceFolder`: the workspace folders in a quick pick with `options`
    /// (`IPickOptions`); the receiver gets the picked folder's index, or null. None without folders.
    fn pick_workspace_folder(&mut self, options: &Value, window: &mut Window, cx: &mut Context<Self>) -> Option<futures::channel::oneshot::Receiver<Result<Value, Value>>> {
        let workspace = self.settings.lock().unwrap().workspace.clone();
        let workspace = workspace.filter(|workspace| !workspace.folders.is_empty())?;
        let items = workspace
            .folders
            .iter()
            .enumerate()
            .map(|(handle, folder)| {
                let parent = folder.path.parent().map(|parent| path_label(parent, Some(&workspace))).unwrap_or_default();
                let description = (parent != folder.name).then_some(parent);
                Entry::Item(Item { handle: handle as i64, label: folder.name.clone(), description, pickable: true, ..Item::default() })
            })
            .collect();
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some(options["placeHolder"].as_str().unwrap_or("Select workspace folder").to_string());
        input.match_on_description = options["matchOnDescription"].as_bool().unwrap_or(true);
        input.match_on_detail = options["matchOnDetail"].as_bool().unwrap_or(false);
        input.ignore_focus_out = options["ignoreFocusOut"].as_bool().unwrap_or(false);
        input.set_items(items);
        Some(self.choose(input, window, cx))
    }
}

/// The label `openDefaultKeybindingsFile` opens its resource with.
pub(super) fn editor_label(document: &TextDocument) -> Option<String> {
    document.is("vscode", Path::new(DEFAULT_KEYBINDINGS)).then(|| "Default Keybindings".to_string())
}

/// `KeybindingIO.writeKeybindingItem` of a default keybinding; an item without a keybinding writes nothing.
fn write_keybinding_item(item: &KeybindingItem) -> String {
    if item.keybinding.is_empty() {
        return String::new();
    }
    let key = format!("{},", Value::from(keybindings::user_settings_label(&item.keybinding)));
    // `rightPaddedString(str, 25)` pads with `new Array(25 - length).join(' ')`, one space short of 25.
    let mut out = format!("{{ \"key\": {key:<24} \"command\": {}", Value::from(item.command.as_str()));
    if let Some(when) = item.when_text.clone().or_else(|| item.when.as_ref().map(|when| when.serialize())) {
        out.push_str(&format!(",\n                                     \"when\": {}", Value::from(when)));
    }
    if let Some(args) = &item.args {
        out.push_str(&format!(",\n                                     \"args\": {args}"));
    }
    out.push_str(" }");
    out
}

/// `getDefaultValue(type)` of configurationRegistry.ts.
fn default_of_type(value_type: &str) -> Value {
    match value_type {
        "boolean" => json!(false),
        "integer" | "number" => json!(0),
        "string" => json!(""),
        "array" => json!([]),
        "object" => json!({}),
        _ => Value::Null,
    }
}

/// The ranges `SettingsEditorModel.getPreference` gives a setting: where its key starts, where its value starts and
/// ends, and whether the value is an object or an array.
#[derive(Debug, PartialEq)]
struct SettingRanges {
    key_start: Position,
    value_start: Position,
    value_end: Position,
    value_is_container: bool,
}

fn setting_ranges(lines: &[String], path: &[Segment]) -> Option<SettingRanges> {
    let text = lines.join("\n");
    let tree = parse_tree(&text, &mut Vec::new(), ParseOptions { allow_trailing_comma: true, ..ParseOptions::default() });
    let value = tree.node(tree.find(tree.root, path)?);
    let key = tree.node(tree.node(value.parent?).children[0]);
    Some(SettingRanges {
        key_start: position_at(&text, key.offset),
        value_start: position_at(&text, value.offset),
        value_end: position_at(&text, value.offset + value.length),
        value_is_container: matches!(value.kind, NodeType::Object | NodeType::Array),
    })
}

/// The line and character column of a byte offset in `text`, whose lines end with "\n".
fn position_at(text: &str, offset: usize) -> Position {
    let before = &text[..offset];
    let line = before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    Position::new(line, before[line_start..].chars().count())
}
