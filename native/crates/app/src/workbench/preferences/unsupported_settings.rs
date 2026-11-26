//! `UnsupportedSettingsRenderer` of workbench/contrib/preferences/browser/preferencesRenderers.ts: hint markers on the
//! settings of an open settings.json that are unknown or cannot apply in that file.

use std::path::Path;
use std::time::Duration;

use config::json::{NodeType, ParseOptions, Tree, parse_tree};
use config::model::{ConfigurationModel, is_override_key};
use config::registry::{APPLICATION_SCOPES, Registry, Scope};
use config::configuration::Overrides;
use config::service::{APPLY_ALL_PROFILES_SETTING, FOLDER_CONFIG_DIRS, Settings, Target};
use gpui::Context;
use serde_json::{Value, json};

use super::Workbench;
use crate::markers::{HINT, ReadOptions, UNNECESSARY};

const OWNER: &str = "UnsupportedSettingsRenderer";
/// `renderingDelayer`.
const RENDER_DELAY: Duration = Duration::from_millis(200);
const PREVIEW_INDICATOR_DESCRIPTION: &str =
    "Preview setting: this setting controls a new feature that is still under refinement yet ready to use. Feedback is welcome.";
const EXPERIMENTAL_INDICATOR_DESCRIPTION: &str =
    "Experimental setting: this setting controls a new feature that is actively being developed and may be unstable. It is subject to change or removal.";

/// `ISetting` as `parse` reads it: the key, and its range from the key's quote to the value's end, 1-based with
/// UTF-16 columns.
pub(super) struct ParsedSetting {
    pub(super) key: String,
    pub(super) range: [usize; 4],
    /// `valueRange`'s start and end.
    pub(super) value_range: [usize; 4],
    pub(super) overrides: Vec<ParsedSetting>,
}

impl Workbench {
    /// `delayedRender` of every open settings file's renderer; files no longer open lose their markers (`dispose`).
    pub(crate) fn schedule_unsupported_settings_markers(&mut self, cx: &mut Context<Self>) {
        self.unsupported_settings_render = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(RENDER_DELAY).await;
            let _ = workbench.update(cx, |workbench, cx| workbench.render_unsupported_settings_markers(cx));
        }));
    }

    fn render_unsupported_settings_markers(&mut self, cx: &mut Context<Self>) {
        let settings = self.settings.lock().unwrap();
        let mut rendered = Vec::new();
        for editor in &self.editors.editors {
            let document = &editor.document();
            if document.scheme != "file" {
                continue;
            }
            let Some((target, in_workspace_file)) = settings_file_target(&settings, &document.path) else { continue };
            let text = document.lines.join("\n");
            let profile_file = profile_file(&settings, &document.path);
            let markers = unsupported_settings_markers(&text, target, in_workspace_file, &settings.registry, &settings.configuration.policy, &profile_file);
            rendered.push((document.path.clone(), markers));
        }
        drop(settings);
        let open: Vec<String> = rendered.iter().map(|(path, _)| crate::editor::uri_string("file", path)).collect();
        let stale: Vec<String> = self
            .diagnostics
            .service
            .read(ReadOptions { owner: Some(OWNER), ..Default::default() })
            .iter()
            .map(|marker| marker.resource.clone())
            .filter(|resource| !open.contains(resource))
            .collect();
        for resource in stale {
            self.remove_workbench_markers(OWNER, &resource, cx);
        }
        for (path, markers) in rendered {
            let uri = json!({ "$mid": 1, "scheme": "file", "authority": "", "path": path.to_string_lossy(), "query": "", "fragment": "" });
            self.change_workbench_markers(OWNER, uri, &markers, cx);
        }
    }
}

/// The settings target of `path` and whether its settings sit under the workspace file's `settings` key; `None` for
/// any other file. A single folder's settings file is the workspace's, as `getEditableSettingsURI` has it.
pub(super) fn settings_file_target(settings: &Settings, path: &Path) -> Option<(Target, bool)> {
    let user_files = settings.user_files();
    if path == user_files.settings || user_files.default_settings.as_deref() == Some(path) {
        return Some((Target::UserLocal, false));
    }
    let workspace = settings.workspace.as_ref()?;
    if workspace.file.as_deref() == Some(path) {
        return Some((Target::Workspace, true));
    }
    let in_folder = workspace.folders.iter().any(|folder| FOLDER_CONFIG_DIRS.iter().any(|dir| folder.path.join(dir).join("settings.json") == path));
    match (in_folder, workspace.file.is_some()) {
        (false, _) => None,
        (true, true) => Some((Target::WorkspaceFolder, false)),
        (true, false) => Some((Target::Workspace, false)),
    }
}

/// Which user settings file the markers are for, in a window whose profile keeps its own settings
/// (`handleLocalUserConfiguration`); `all_profiles` is `workbench.settings.applyToAllProfiles`.
pub enum ProfileFile {
    Shared,
    DefaultWhileOtherActive { all_profiles: Vec<String> },
    Profile { all_profiles: Vec<String> },
}

fn profile_file(settings: &Settings, path: &Path) -> ProfileFile {
    let user_files = settings.user_files();
    if user_files.default_settings.is_none() {
        return ProfileFile::Shared;
    }
    let all_profiles = settings.get_value(Some(APPLY_ALL_PROFILES_SETTING), &Overrides::default());
    let all_profiles = all_profiles.as_ref().and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
    match user_files.default_settings.as_deref() == Some(path) {
        true => ProfileFile::DefaultWhileOtherActive { all_profiles },
        false => ProfileFile::Profile { all_profiles },
    }
}

/// `generateMarkerData`; `policy` holds the values set by system policy. shortcut: no untrusted-workspace markers
/// until workspace trust reaches the settings files; remote windows are removed.
pub fn unsupported_settings_markers(text: &str, target: Target, in_workspace_file: bool, registry: &Registry, policy: &ConfigurationModel, profile_file: &ProfileFile) -> Vec<Value> {
    let mut markers = Vec::new();
    for setting in parse_settings(text, in_workspace_file) {
        if is_override_key(&setting.key) {
            for setting in &setting.overrides {
                match registry.property(&setting.key) {
                    Some(property) if property.scope != Scope::LanguageOverridable => markers.push(hint(
                        &setting.range,
                        "This setting cannot be applied because it is not registered as language override setting.",
                        true,
                    )),
                    Some(_) => {}
                    None => markers.push(unknown_configuration_marker(&setting.range)),
                }
            }
            continue;
        }
        if setting.key == "mcp" && in_workspace_file {
            continue;
        }
        let Some(property) = registry.property(&setting.key) else {
            markers.push(unknown_configuration_marker(&setting.range));
            continue;
        };
        let tags = property.schema["tags"].as_array();
        let has_tag = |tag: &str| tags.is_some_and(|tags| tags.iter().any(|value| value == tag));
        if has_tag("preview") {
            markers.push(hint(&setting.range, PREVIEW_INDICATOR_DESCRIPTION, false));
        } else if has_tag("experimental") {
            markers.push(hint(&setting.range, EXPERIMENTAL_INDICATOR_DESCRIPTION, false));
        }
        // `handlePolicyConfiguration`: a policy value hides the scope checks.
        if !property.schema["policy"].is_null() && policy.get_value(Some(&setting.key)).is_some() {
            markers.push(hint(&setting.range, "This setting cannot be applied because it is configured in the system policy.", true));
            continue;
        }
        let scope = property.scope;
        if target == Target::UserLocal {
            let applied_for_all = |all_profiles: &[String]| APPLICATION_SCOPES.contains(&scope) || all_profiles.contains(&setting.key);
            match profile_file {
                ProfileFile::DefaultWhileOtherActive { all_profiles } if !applied_for_all(all_profiles) => markers.push(hint(
                    &setting.range,
                    "This setting cannot be applied while a non-default profile is active. It will be applied when the default profile is active.",
                    true,
                )),
                ProfileFile::Profile { .. } if APPLICATION_SCOPES.contains(&scope) => {
                    markers.push(hint(&setting.range, "This setting has an application scope and can only be set in the settings file from the Default profile.", true));
                }
                ProfileFile::Profile { all_profiles } if applied_for_all(all_profiles) => markers.push(hint(
                    &setting.range,
                    &format!("This setting cannot be applied because it is configured to be applied in all profiles using setting {APPLY_ALL_PROFILES_SETTING}. Value from the default profile will be used instead."),
                    true,
                )),
                _ => {}
            }
        }
        if matches!(target, Target::Workspace | Target::WorkspaceFolder) {
            if APPLICATION_SCOPES.contains(&scope) {
                markers.push(hint(&setting.range, "This setting has an application scope and can only be set in the settings file from the Default profile.", true));
            }
            if scope == Scope::Machine {
                markers.push(hint(&setting.range, "This setting can only be applied in user settings in local window or in remote settings in remote window.", true));
            }
        }
        if target == Target::WorkspaceFolder && scope == Scope::Window {
            markers.push(hint(
                &setting.range,
                "This setting cannot be applied in this workspace. It will be applied when you open the containing workspace folder directly.",
                true,
            ));
        }
    }
    markers
}

fn unknown_configuration_marker(range: &[usize; 4]) -> Value {
    hint(range, "Unknown Configuration Setting", true)
}

fn hint(range: &[usize; 4], message: &str, unnecessary: bool) -> Value {
    let mut marker = json!({
        "severity": HINT,
        "startLineNumber": range[0],
        "startColumn": range[1],
        "endLineNumber": range[2],
        "endColumn": range[3],
        "message": message,
    });
    if unnecessary {
        marker["tags"] = json!([UNNECESSARY]);
    }
    marker
}

/// `parse` of `SettingsEditorModel`, or of `WorkspaceConfigurationEditorModel` whose settings sit under `settings`:
/// the properties of the settings object with the overrides of their `[language]` keys. A property without a value
/// is dropped, as `onError` drops a setting whose value range is unset.
pub(super) fn parse_settings(text: &str, in_workspace_file: bool) -> Vec<ParsedSetting> {
    let tree = parse_tree(text, &mut Vec::new(), ParseOptions { allow_trailing_comma: true, ..ParseOptions::default() });
    let root = match in_workspace_file {
        true => tree.find(tree.root, &[config::json::Segment::Key("settings".into())]),
        false => tree.root,
    };
    let Some(root) = root.filter(|&root| tree.node(root).kind == NodeType::Object) else { return Vec::new() };
    let mut settings = properties(&tree, root, text);
    for setting in &mut settings {
        if !is_override_key(&setting.key) {
            continue;
        }
        let value = tree.find(Some(root), &[config::json::Segment::Key(setting.key.clone())]);
        if let Some(value) = value.filter(|&value| tree.node(value).kind == NodeType::Object) {
            setting.overrides = properties(&tree, value, text);
        }
    }
    settings
}

fn properties(tree: &Tree, object: usize, text: &str) -> Vec<ParsedSetting> {
    let mut settings = Vec::new();
    for &property in &tree.node(object).children {
        let property = tree.node(property);
        let (Some(&key), Some(&value)) = (property.children.first(), property.children.get(1)) else { continue };
        let (key, value) = (tree.node(key), tree.node(value));
        let Some(name) = key.value.as_str() else { continue };
        let (start_line, start_column) = position_at(text, key.offset);
        let (value_line, value_column) = position_at(text, value.offset);
        let (end_line, end_column) = position_at(text, value.offset + value.length);
        let (range, value_range) = ([start_line, start_column, end_line, end_column], [value_line, value_column, end_line, end_column]);
        settings.push(ParsedSetting { key: name.to_string(), range, value_range, overrides: Vec::new() });
    }
    settings
}

/// `getPositionAt`: the 1-based line and UTF-16 column of a byte offset in `text`, whose lines end with "\n".
fn position_at(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset];
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    (before.matches('\n').count() + 1, before[line_start..].encode_utf16().count() + 1)
}

#[cfg(test)]
mod tests;
