//! The profile template (`.code-profile`) of userDataProfileImportExportService.ts and the profile resources of
//! services/userDataProfile/browser: settings (machine scoped settings left out), keybindings, tasks, snippets,
//! global state (the keys stored for the user) and extensions, read from one profile and applied to another.

use std::path::{Path, PathBuf};

use config::configuration::Configuration;
use config::json::{self, ParseOptions, Segment};
use config::json_edit::{apply_edits, set_property};
use config::registry::{Registry, Scope};
use config::service::{formatting_options, write_user_data_file};
use serde_json::{Map, Value, json};
use storage::Storage;
use storage::profiles::Profile;

/// `PROFILE_EXTENSION`.
pub const PROFILE_EXTENSION: &str = "code-profile";
/// storage.ts `TARGET_KEY`: the targets of the stored keys, `StorageTarget.USER` is 0.
const TARGET_KEY: &str = "__$__targetStorageMarker";
/// platform.ts `Platform.Mac`, the `platform` of exported keybindings.
const PLATFORM_MAC: u8 = 1;

/// Which resources a template carries (`ProfileResourceTypeFlags`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flags {
    pub settings: bool,
    pub keybindings: bool,
    pub snippets: bool,
    pub tasks: bool,
    pub global_state: bool,
    pub extensions: bool,
}

impl Flags {
    pub const ALL: Flags = Flags { settings: true, keybindings: true, snippets: true, tasks: true, global_state: true, extensions: true };
}

/// `getProfileTemplate`: each resource's content as the JSON string its `getContent` returns, `extensions` the
/// list `ExtensionsResource.getContent` gives for `extensions`.
pub fn export_template(profile: &Profile, name: &str, flags: Flags, registry: &Registry, configuration: &Configuration, global_state: &Storage, extensions: &[Value]) -> Value {
    let mut template = Map::new();
    template.insert("name".into(), json!(name));
    if let Some(icon) = &profile.icon {
        template.insert("icon".into(), json!(icon));
    }
    if flags.settings {
        let settings = read(&profile.settings_resource).map(|local| update_ignored_settings(&local, "{}", &ignored_settings(registry), configuration, &profile.settings_resource));
        template.insert("settings".into(), json!(json!({ "settings": settings }).to_string()));
    }
    if flags.keybindings {
        let keybindings = read(&profile.keybindings_resource);
        template.insert("keybindings".into(), json!(json!({ "keybindings": keybindings, "platform": PLATFORM_MAC }).to_string()));
    }
    if flags.tasks {
        template.insert("tasks".into(), json!(json!({ "tasks": read(&profile.tasks_resource) }).to_string()));
    }
    if flags.snippets {
        let mut snippets = Map::new();
        for (key, path) in snippet_files(&profile.snippets_home) {
            snippets.insert(key, json!(read(&path).unwrap_or_default()));
        }
        template.insert("snippets".into(), json!(json!({ "snippets": snippets }).to_string()));
    }
    if flags.global_state {
        let storage = user_storage(global_state);
        if !storage.is_empty() {
            template.insert("globalState".into(), json!(json!({ "storage": storage }).to_string()));
        }
    }
    if flags.extensions && !extensions.is_empty() {
        template.insert("extensions".into(), json!(Value::Array(extensions.to_vec()).to_string()));
    }
    Value::Object(template)
}

/// `isUserDataProfileTemplate` after `JSON.parse`.
pub fn parse_template(content: &str) -> Result<Value, String> {
    let invalid = || "Invalid profile content.".to_owned();
    let template: Value = serde_json::from_str(content).map_err(|_| invalid())?;
    let string_or_missing = |key: &str| template.get(key).is_none_or(Value::is_string);
    match template.is_object() && ["settings", "globalState", "extensions", "mcp"].into_iter().all(string_or_missing) {
        true => Ok(template),
        false => Err(invalid()),
    }
}

/// `applyProfileTemplate`: each resource's `apply` on `profile`. Answers the extensions the template lists, which
/// the caller installs from the gallery and enables or disables.
pub fn apply_template(template: &Value, profile: &Profile, registry: &Registry, configuration: &Configuration, global_state: &mut Storage) -> Result<Vec<Value>, String> {
    let resource = |key: &str| template[key].as_str().map(|content| serde_json::from_str::<Value>(content).unwrap_or_default());
    if let Some(content) = resource("settings")
        && let Some(settings) = content["settings"].as_str()
    {
        let local = read(&profile.settings_resource).unwrap_or_else(|| "{}".into());
        let updated = update_ignored_settings(settings, &local, &ignored_settings(registry), configuration, &profile.settings_resource);
        write(&profile.settings_resource, &updated)?;
    }
    if let Some(content) = resource("keybindings")
        && let Some(keybindings) = content["keybindings"].as_str()
    {
        write(&profile.keybindings_resource, keybindings)?;
    }
    if let Some(content) = resource("tasks")
        && let Some(tasks) = content["tasks"].as_str()
    {
        write(&profile.tasks_resource, tasks)?;
    }
    if let Some(content) = resource("snippets") {
        for (key, snippet) in content["snippets"].as_object().into_iter().flatten() {
            // `toSnippetResource`: a key that leaves the snippets folder is ignored.
            let escapes = Path::new(key).components().any(|component| !matches!(component, std::path::Component::Normal(_)));
            if escapes {
                eprintln!("SnippetsResource: Ignoring snippet with key '{key}' as it escapes the snippets folder.");
                continue;
            }
            write(&profile.snippets_home.join(key), snippet.as_str().unwrap_or_default())?;
        }
    }
    if let Some(content) = resource("globalState") {
        let storage = content["storage"].as_object().cloned().unwrap_or_default();
        let mut targets: Map<String, Value> = global_state.get(TARGET_KEY).and_then(|targets| serde_json::from_str(targets).ok()).unwrap_or_default();
        for (key, value) in &storage {
            global_state.set(key, value.as_str().map_or_else(|| value.to_string(), str::to_owned));
            targets.insert(key.clone(), json!(0));
        }
        if !storage.is_empty() {
            global_state.set(TARGET_KEY, Value::Object(targets).to_string());
        }
    }
    let extensions = template["extensions"].as_str().map(|content| serde_json::from_str::<Vec<Value>>(content).unwrap_or_default());
    Ok(extensions.unwrap_or_default())
}

/// `SettingsResource.getIgnoredSettings`: machine scoped settings stay with the machine.
fn ignored_settings(registry: &Registry) -> Vec<String> {
    let machine = |key: &&String| registry.property(key).is_some_and(|property| matches!(property.scope, Scope::Machine | Scope::ApplicationMachine | Scope::MachineOverridable));
    registry.keys().iter().filter(machine).cloned().collect()
}

/// settingsMerge.ts `updateIgnoredSettings`: the ignored settings of `target` take their value from `source`, or
/// are removed when `source` has none.
fn update_ignored_settings(target: &str, source: &str, ignored: &[String], configuration: &Configuration, path: &Path) -> String {
    let parse = |text: &str| json::parse(text, &mut Vec::new(), ParseOptions { allow_trailing_comma: true, allow_empty_content: true, ..ParseOptions::default() });
    let Some(Value::Object(target_values)) = parse(target) else { return target.to_owned() };
    let source_values = parse(source).and_then(|source| source.as_object().cloned()).unwrap_or_default();
    let options = formatting_options(target, configuration, path);
    let mut text = target.to_owned();
    for key in ignored {
        let value = match (source_values.get(key), target_values.contains_key(key)) {
            (None, true) => None,
            (Some(value), _) => Some(value),
            (None, false) => continue,
        };
        let edits = set_property(&text, &[Segment::Key(key.clone())], value, &options);
        text = apply_edits(&text, &edits);
    }
    text
}

/// `GlobalStateResource.getGlobalState`: the keys stored for the user.
fn user_storage(storage: &Storage) -> Map<String, Value> {
    let targets: Map<String, Value> = storage.get(TARGET_KEY).and_then(|targets| serde_json::from_str(targets).ok()).unwrap_or_default();
    let mut user = Map::new();
    for (key, target) in targets {
        if target == 0
            && let Some(value) = storage.get(&key)
        {
            user.insert(key, json!(value));
        }
    }
    user
}

/// `getSnippetsResources`: the `.json` and `.code-snippets` files of the snippets folder, by relative path.
fn snippet_files(home: &Path) -> Vec<(String, PathBuf)> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(home).into_iter().flatten().flatten() {
        let path = entry.path();
        let snippet = path.extension().is_some_and(|extension| extension == "json" || extension == "code-snippets");
        if path.is_file() && snippet {
            files.push((entry.file_name().to_string_lossy().into_owned(), path));
        }
    }
    files.sort();
    files
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn write(path: &Path, content: &str) -> Result<(), String> {
    write_user_data_file(path, content.as_bytes()).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests;
