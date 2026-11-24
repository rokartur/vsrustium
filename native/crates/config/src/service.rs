//! The configuration service of one window (workbench/services/configuration/browser/configurationService.ts
//! and configuration.ts): it loads the user and folder settings files, reloads them when they change on disk,
//! and writes values into them as common/configurationEditing.ts does.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{Map, Value};

use crate::configuration::{Configuration, Overrides};
use crate::indentation::{creation_options, model_indentation};
use crate::json::{self, ParseOptions, Segment};
use crate::json_edit::{FormattingOptions, set_property};
use crate::model::{Change, ConfigurationModel, ParseFilter, is_override_key, key_from_override_identifiers, merge_changes, parse_content, to_values_tree};
use crate::registry::{APPLICATION_SCOPES, FOLDER_SCOPES, Registry, Scope, WORKSPACE_SCOPES};

/// `ConfigurationTarget` of platform/configuration/common/configuration.ts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Application = 1,
    User = 2,
    UserLocal = 3,
    UserRemote = 4,
    Workspace = 5,
    WorkspaceFolder = 6,
    Default = 7,
    Memory = 8,
}

impl Target {
    pub fn from_number(value: u64) -> Option<Target> {
        Some(match value {
            1 => Target::Application,
            2 => Target::User,
            3 => Target::UserLocal,
            4 => Target::UserRemote,
            5 => Target::Workspace,
            6 => Target::WorkspaceFolder,
            7 => Target::Default,
            8 => Target::Memory,
            _ => return None,
        })
    }
}

/// `EditableConfigurationTarget`, without the remote user settings the port has no remote for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Editable {
    UserLocal,
    Workspace,
    WorkspaceFolder,
}

/// `TASKS_DEFAULT`: what configurationEditing.ts writes into a new tasks.json before editing it.
const TASKS_DEFAULT: &str = "{\n\t\"version\": \"2.0.0\",\n\t\"tasks\": []\n}";

pub struct Settings {
    pub registry: Registry,
    pub configuration: Configuration,
    /// `<user data>/User`, the default profile's home: settings.json, tasks.json, mcp.json.
    user_home: PathBuf,
    /// Size and modification time of every file a layer reads, to notice edits made outside the port.
    stamps: Vec<(PathBuf, Option<(SystemTime, u64)>)>,
}

impl Settings {
    /// `ConfigurationService.initialize` for an empty window (`folder` is `None`) or a folder window.
    pub fn load(registry: Registry, user_home: PathBuf, folder: Option<PathBuf>) -> Settings {
        let defaults = registry.default_model();
        let mut settings = Settings { registry, configuration: Configuration { defaults, ..Configuration::default() }, user_home, stamps: Vec::new() };
        settings.configuration.local_user = settings.load_user();
        if let Some(folder) = folder {
            let model = settings.load_folder(&folder);
            settings.configuration.workspace = model.clone();
            settings.configuration.folder = Some((folder, model));
        }
        settings.stamps = settings.watched_files().into_iter().map(|path| (path.clone(), stamp(&path))).collect();
        settings
    }

    /// `getValue(section, overrides)`.
    pub fn get_value(&self, section: Option<&str>, overrides: &Overrides) -> Option<Value> {
        self.configuration.get_value(section, overrides)
    }

    /// Reloads the layers whose files changed on disk since they were read, as the file watchers of
    /// `UserConfiguration` and `FolderConfiguration` do. `None` when no setting changed.
    pub fn reload_changed_files(&mut self) -> Option<Change> {
        let mut user_changed = false;
        let mut folder_changed = false;
        for (path, last) in &mut self.stamps {
            let now = stamp(path);
            if now == *last {
                continue;
            }
            *last = now;
            match path.starts_with(&self.user_home) {
                true => user_changed = true,
                false => folder_changed = true,
            }
        }
        let mut changes = Vec::new();
        if user_changed {
            let user = self.load_user();
            changes.push(self.configuration.compare_and_update_local_user(user));
        }
        if folder_changed && let Some((folder, _)) = &self.configuration.folder {
            let model = self.load_folder(&folder.clone());
            changes.push(self.configuration.compare_and_update_folder(model));
        }
        let change = merge_changes(&changes);
        (!change.keys.is_empty()).then_some(change)
    }

    /// `updateValue(key, value, overrides, target)` with a target, as MainThreadConfiguration calls it:
    /// writes the value (`None` removes it) into that target's file, reloads the file and returns what changed.
    pub fn update_value(&mut self, key: &str, value: Option<&Value>, target: Target, resource: Option<&Path>, override_identifiers: &[String]) -> Result<Change, String> {
        let editable = match target {
            Target::Application | Target::User | Target::UserLocal => Editable::UserLocal,
            Target::Workspace => Editable::Workspace,
            Target::WorkspaceFolder => Editable::WorkspaceFolder,
            Target::UserRemote | Target::Default | Target::Memory => return Err("Invalid configuration target".into()),
        };
        let operation = self.edit_operation(editable, key, resource, override_identifiers);
        self.validate(editable, &operation, value, override_identifiers)?;
        let path = operation.path.as_ref().ok_or_else(|| self.error(Error::InvalidFolderTarget, editable, key))?;
        self.write(path, &operation, value, editable)?;
        Ok(self.reload_after_write(editable))
    }

    fn reload_after_write(&mut self, editable: Editable) -> Change {
        for (path, last) in &mut self.stamps {
            *last = stamp(path);
        }
        match (editable, &self.configuration.folder) {
            (Editable::UserLocal, _) => {
                let user = self.load_user();
                self.configuration.compare_and_update_local_user(user)
            }
            (_, Some((folder, _))) => {
                let model = self.load_folder(&folder.clone());
                self.configuration.compare_and_update_folder(model)
            }
            (_, None) => Change::default(),
        }
    }

    fn watched_files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = ["settings.json", "tasks.json", "mcp.json"].iter().map(|name| self.user_home.join(name)).collect();
        if let Some((folder, _)) = &self.configuration.folder {
            files.extend(["settings.json", "tasks.json", "launch.json", "mcp.json"].iter().map(|name| folder.join(".vscode").join(name)));
        }
        files
    }

    /// `UserConfiguration`: settings.json of the default profile with every scope, plus tasks.json and mcp.json.
    fn load_user(&self) -> ConfigurationModel {
        let settings = parse_content(&read_or_empty(&self.user_home.join("settings.json")), Some(&self.registry), &ParseFilter::default()).model;
        let standalone = ["tasks", "mcp"].map(|name| standalone_model(name, &self.user_home.join(format!("{name}.json"))));
        settings.merge(&standalone.into_iter().flatten().collect::<Vec<_>>())
    }

    /// `FolderConfiguration` of a folder window: `.vscode/settings.json` with WORKSPACE_SCOPES (the window is
    /// trusted, as the reference runs with workspace trust off) plus tasks.json, launch.json and mcp.json.
    fn load_folder(&self, folder: &Path) -> ConfigurationModel {
        let directory = folder.join(".vscode");
        let filter = ParseFilter { scopes: Some(WORKSPACE_SCOPES.to_vec()), ..ParseFilter::default() };
        let settings = parse_content(&read_or_empty(&directory.join("settings.json")), Some(&self.registry), &filter).model;
        let standalone = ["tasks", "launch", "mcp"].map(|name| standalone_model(name, &directory.join(format!("{name}.json"))));
        settings.merge(&standalone.into_iter().flatten().collect::<Vec<_>>())
    }

    /// `getConfigurationEditOperation`.
    fn edit_operation(&self, editable: Editable, key: &str, resource: Option<&Path>, override_identifiers: &[String]) -> Operation {
        let standalone: &[&str] = match editable {
            Editable::UserLocal => &["tasks", "mcp"],
            _ => &["tasks", "launch", "mcp"],
        };
        for name in standalone {
            let path = self.file_for(editable, &format!("{name}.json"), resource);
            if key == *name {
                return Operation { key: key.to_owned(), json_path: Vec::new(), path, standalone: Some(name) };
            }
            if let Some(rest) = key.strip_prefix(&format!("{name}.")) {
                return Operation { key: rest.to_owned(), json_path: vec![Segment::Key(rest.to_owned())], path, standalone: Some(name) };
            }
        }
        let mut json_path = vec![Segment::Key(key.to_owned())];
        if !override_identifiers.is_empty() {
            json_path.insert(0, Segment::Key(key_from_override_identifiers(override_identifiers)));
        }
        let path = match editable {
            Editable::UserLocal => Some(self.user_home.join("settings.json")),
            _ => self.file_for(editable, "settings.json", resource),
        };
        Operation { key: key.to_owned(), json_path, path, standalone: None }
    }

    /// `getConfigurationFileResource`: the user file, or the file in the folder's `.vscode`.
    fn file_for(&self, editable: Editable, name: &str, resource: Option<&Path>) -> Option<PathBuf> {
        let folder = self.configuration.folder.as_ref().map(|(folder, _)| folder);
        match editable {
            Editable::UserLocal => Some(self.user_home.join(name)),
            Editable::Workspace => folder.map(|folder| folder.join(".vscode").join(name)),
            Editable::WorkspaceFolder => folder.filter(|folder| resource.is_some_and(|resource| resource.starts_with(folder))).map(|folder| folder.join(".vscode").join(name)),
        }
    }

    /// `ConfigurationEditing.validate`.
    fn validate(&self, editable: Editable, operation: &Operation, value: Option<&Value>, override_identifiers: &[String]) -> Result<(), String> {
        let key = &operation.key;
        let scope = self.registry.property(key).map(|property| property.scope);
        if operation.standalone.is_none() && value.is_some() && !is_override_key(key) && !self.registry.keys().contains(key) {
            return Err(self.error(Error::UnknownKey, editable, key));
        }
        if operation.standalone.is_some_and(|name| name == "launch") && editable == Editable::UserLocal {
            return Err(self.error(Error::InvalidUserTarget, editable, key));
        }
        if editable != Editable::UserLocal && self.configuration.folder.is_none() {
            return Err(self.error(Error::NoWorkspaceOpened, editable, key));
        }
        let plain_setting = operation.standalone.is_none() && !is_override_key(key);
        if editable == Editable::Workspace && plain_setting {
            if scope.is_some_and(|scope| APPLICATION_SCOPES.contains(&scope)) {
                return Err(self.error(Error::InvalidWorkspaceConfigurationApplication, editable, key));
            }
            if scope == Some(Scope::Machine) {
                return Err(self.error(Error::InvalidWorkspaceConfigurationMachine, editable, key));
            }
        }
        if editable == Editable::WorkspaceFolder {
            if operation.path.is_none() {
                return Err(self.error(Error::InvalidFolderTarget, editable, key));
            }
            if plain_setting && scope.is_some_and(|scope| !FOLDER_SCOPES.contains(&scope)) {
                return Err(self.error(Error::InvalidFolderConfiguration, editable, key));
            }
        }
        if !override_identifiers.is_empty() && scope != Some(Scope::LanguageOverridable) {
            return Err(self.error(Error::InvalidResourceLanguageConfiguration, editable, key));
        }
        Ok(())
    }

    /// `resolveModelReference`, `updateConfiguration` and `save`: creates the file if missing, edits it in place
    /// with the file's own indentation and line breaks, and saves only when the text changed.
    fn write(&self, path: &Path, operation: &Operation, value: Option<&Value>, editable: Editable) -> Result<(), String> {
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(_) => {
                let initial = if operation.standalone == Some("tasks") { TASKS_DEFAULT } else { "{}" };
                write_file(path, initial).map_err(|error| self.fs_error(editable, &error))?;
                initial.to_owned()
            }
        };
        let whole_file = operation.standalone.is_some() && operation.json_path.is_empty();
        let mut errors = Vec::new();
        json::parse(&content, &mut errors, ParseOptions { allow_trailing_comma: true, allow_empty_content: true, ..ParseOptions::default() });
        if !whole_file && !errors.is_empty() {
            return Err(self.invalid_configuration_error(editable, operation));
        }
        let options = formatting_options(&content, &self.configuration, path);
        let new_content = match whole_file {
            true => stringify(value.unwrap_or(&Value::Null), &options),
            false => match set_property(&content, &operation.json_path, value, &options).first() {
                Some(edit) => format!("{}{}{}", &content[..edit.offset], edit.content, &content[edit.offset + edit.length..]),
                None => return Ok(()),
            },
        };
        if new_content != content {
            write_file(path, &new_content).map_err(|error| self.fs_error(editable, &error))?;
        }
        Ok(())
    }

    /// `toErrorMessage` for the API constraint errors `validate` raises.
    fn error(&self, error: Error, editable: Editable, key: &str) -> String {
        match error {
            Error::UnknownKey => format!("Unable to write to {} because {key} is not a registered configuration.", target_name(editable)),
            Error::InvalidWorkspaceConfigurationApplication | Error::InvalidWorkspaceConfigurationMachine => {
                format!("Unable to write {key} to Workspace Settings. This setting can be written only into User settings.")
            }
            Error::InvalidFolderConfiguration => format!("Unable to write to Folder Settings because {key} does not support the folder resource scope."),
            Error::InvalidUserTarget => format!("Unable to write to User Settings because {key} does not support for global scope."),
            Error::InvalidFolderTarget => "Unable to write to Folder Settings because no resource is provided.".into(),
            Error::InvalidResourceLanguageConfiguration => format!("Unable to write to Language Settings because {key} is not a resource language setting."),
            Error::NoWorkspaceOpened => {
                format!("Unable to write to {} because no workspace is opened. Please open a workspace first and try again.", target_name(editable))
            }
        }
    }

    /// `ERROR_INVALID_CONFIGURATION`: the file does not parse, so an edit could corrupt it.
    fn invalid_configuration_error(&self, editable: Editable, operation: &Operation) -> String {
        match (operation.standalone, editable) {
            (Some("tasks"), _) => "Unable to write into the tasks configuration file. Please open it to correct errors/warnings in it and try again.".into(),
            (Some("launch"), _) => "Unable to write into the launch configuration file. Please open it to correct errors/warnings in it and try again.".into(),
            (Some("mcp"), _) => "Unable to write into the MCP configuration file. Please open it to correct errors/warnings in it and try again.".into(),
            (_, Editable::UserLocal) => "Unable to write into user settings. Please open the user settings to correct errors/warnings in it and try again.".into(),
            (_, Editable::Workspace) => "Unable to write into workspace settings. Please open the workspace settings to correct errors/warnings in the file and try again.".into(),
            (_, Editable::WorkspaceFolder) => {
                let name = self.configuration.folder.as_ref().and_then(|(folder, _)| folder.file_name()).map_or("<<unknown>>".into(), |name| name.to_string_lossy());
                format!("Unable to write into folder settings. Please open the '{name}' folder settings to correct errors/warnings in it and try again.")
            }
        }
    }

    fn fs_error(&self, editable: Editable, error: &std::io::Error) -> String {
        format!("Error while writing to {}. {error}", target_name(editable))
    }
}

/// `ConfigurationEditingErrorCode`s `validate` raises.
enum Error {
    UnknownKey,
    InvalidWorkspaceConfigurationApplication,
    InvalidWorkspaceConfigurationMachine,
    InvalidFolderConfiguration,
    InvalidUserTarget,
    InvalidFolderTarget,
    InvalidResourceLanguageConfiguration,
    NoWorkspaceOpened,
}

/// `IConfigurationEditOperation`: `key` is the last segment written, `standalone` the tasks/launch/mcp file.
struct Operation {
    key: String,
    json_path: Vec<Segment>,
    path: Option<PathBuf>,
    standalone: Option<&'static str>,
}

/// `stringifyTarget`.
fn target_name(editable: Editable) -> &'static str {
    match editable {
        Editable::UserLocal => "User Settings",
        Editable::Workspace => "Workspace Settings",
        Editable::WorkspaceFolder => "Folder Settings",
    }
}

/// `StandaloneConfigurationModelParser`: a tasks, launch or mcp file as the `name` section; `None` when missing.
fn standalone_model(name: &str, path: &Path) -> Option<ConfigurationModel> {
    let content = std::fs::read_to_string(path).ok()?;
    let raw = match json::parse(&content, &mut Vec::new(), ParseOptions::default()) {
        Some(Value::Object(raw)) => raw,
        _ => Map::new(),
    };
    let keys = raw.keys().map(|key| format!("{name}.{key}")).collect();
    let mut contents = Map::new();
    contents.insert(name.to_owned(), Value::Object(to_values_tree(&raw, &mut |_| {})));
    Some(ConfigurationModel::new(contents, keys, Vec::new()))
}

/// The text a settings file parses from: a missing or empty file reads as `{}`, as `UserSettings` and
/// `FileServiceBasedConfiguration.resolveContents` do.
fn read_or_empty(path: &Path) -> String {
    std::fs::read_to_string(path).ok().filter(|content| !content.is_empty()).unwrap_or_else(|| "{}".into())
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

fn write_file(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)
}

/// `getFormattingOptions(model)`: the indentation of the file's text model (language `jsonc`) and its line
/// break, `\n` on macOS for a file with none.
fn formatting_options(content: &str, configuration: &Configuration, path: &Path) -> FormattingOptions {
    let lines: Vec<&str> = content.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)).collect();
    let (tab_size, insert_spaces) = model_indentation(&lines, &creation_options(configuration, path, "jsonc"));
    let crlf = content.matches("\r\n").count();
    let eol = if crlf > 0 && crlf * 2 > content.matches('\n').count() { "\r\n" } else { "\n" };
    FormattingOptions { tab_size, insert_spaces, eol: eol.into() }
}

/// `JSON.stringify(value, null, indent)` for a whole standalone file.
fn stringify(value: &Value, options: &FormattingOptions) -> String {
    let indent = match options.insert_spaces && options.tab_size > 0 {
        true => " ".repeat(options.tab_size),
        false => "\t".into(),
    };
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    serde::Serialize::serialize(value, &mut serializer).expect("a JSON value serializes");
    String::from_utf8(out).expect("serde_json writes UTF-8")
}

/// The write cases of workbench/services/configuration/test/browser/configurationEditing.test.ts, on a real
/// temporary directory. Left out: dirty-file, Save and Retry and profile cases (no text file model or profiles
/// yet) and the policy cases (no policies).
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Fixture {
        root: PathBuf,
        settings: Settings,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl Fixture {
        fn new(name: &str, with_folder: bool) -> Fixture {
            let root = std::env::temp_dir().join(format!("config-service-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("folder")).unwrap();
            let mut registry = Registry::core();
            let setting = json!({ "type": "string", "default": "isSet" });
            let properties = json!({
                "configurationEditing.service.testSetting": setting,
                "configurationEditing.service.testSettingTwo": setting,
                "configurationEditing.service.applicationSetting": { "type": "string", "default": "isSet", "scope": "application" },
                "configurationEditing.service.windowSetting": { "type": "string", "default": "isSet", "scope": "window" },
                "configurationEditing.service.languageSetting": { "type": "string", "default": "isSet", "scope": "language-overridable" },
            });
            registry.register_configuration("test", &json!({ "contributes": { "configuration": { "id": "_test", "properties": properties } } }));
            registry.finish();
            let folder = with_folder.then(|| root.join("folder"));
            let settings = Settings::load(registry, root.join("User"), folder);
            Fixture { root, settings }
        }

        fn user(&self, name: &str) -> PathBuf {
            self.root.join("User").join(name)
        }

        fn folder(&self, name: &str) -> PathBuf {
            self.root.join("folder/.vscode").join(name)
        }

        fn write(&mut self, key: &str, value: Option<Value>, target: Target) -> Result<Change, String> {
            self.settings.update_value(key, value.as_ref(), target, None, &[])
        }
    }

    fn write_file(path: &Path, content: &str) {
        super::write_file(path, content).unwrap();
    }

    fn parsed(path: &Path) -> Value {
        json::parse(&std::fs::read_to_string(path).unwrap(), &mut Vec::new(), ParseOptions::default()).unwrap()
    }

    #[test]
    fn error_cases() {
        let mut fixture = Fixture::new("errors", true);
        let error = fixture.write("unknown.key", Some(json!("value")), Target::Workspace).unwrap_err();
        assert_eq!(error, "Unable to write to Workspace Settings because unknown.key is not a registered configuration.");
        let error = fixture.write("configurationEditing.service.applicationSetting", Some(json!("value")), Target::Workspace).unwrap_err();
        assert_eq!(error, "Unable to write configurationEditing.service.applicationSetting to Workspace Settings. This setting can be written only into User settings.");
        let error = fixture.settings.update_value("configurationEditing.service.testSetting", Some(&json!("value")), Target::User, None, &["md".into()]).unwrap_err();
        assert_eq!(error, "Unable to write to Language Settings because configurationEditing.service.testSetting is not a resource language setting.");

        write_file(&fixture.user("settings.json"), ",,,,,,,,,,,,,,");
        let error = fixture.write("configurationEditing.service.testSetting", Some(json!("value")), Target::User).unwrap_err();
        assert_eq!(error, "Unable to write into user settings. Please open the user settings to correct errors/warnings in it and try again.");

        write_file(&fixture.user("tasks.json"), ",,,,,,,,,,,,,,");
        let error = fixture.write("tasks.version", Some(json!("1.0.0")), Target::User).unwrap_err();
        assert_eq!(error, "Unable to write into the tasks configuration file. Please open it to correct errors/warnings in it and try again.");

        let mut empty_window = Fixture::new("no-workspace", false);
        let error = empty_window.write("configurationEditing.service.testSetting", Some(json!("value")), Target::Workspace).unwrap_err();
        assert_eq!(error, "Unable to write to Workspace Settings because no workspace is opened. Please open a workspace first and try again.");
    }

    #[test]
    fn write_and_remove_settings() {
        let mut fixture = Fixture::new("settings", true);
        fixture.write("configurationEditing.service.testSetting", Some(json!("value")), Target::User).unwrap();
        assert_eq!(parsed(&fixture.user("settings.json")), json!({ "configurationEditing.service.testSetting": "value" }));

        write_file(&fixture.user("settings.json"), r#"{ "my.super.setting": "my.super.value" }"#);
        fixture.write("configurationEditing.service.testSetting", Some(json!("value")), Target::User).unwrap();
        assert_eq!(parsed(&fixture.user("settings.json")), json!({ "my.super.setting": "my.super.value", "configurationEditing.service.testSetting": "value" }));

        write_file(&fixture.user("settings.json"), r#"{ "my.super.setting": "my.super.value", "configurationEditing.service.testSetting": "value" }"#);
        fixture.write("configurationEditing.service.testSetting", None, Target::User).unwrap();
        assert_eq!(parsed(&fixture.user("settings.json")), json!({ "my.super.setting": "my.super.value" }));
        fixture.write("configurationEditing.service.testSetting", None, Target::User).unwrap();
        assert_eq!(parsed(&fixture.user("settings.json")), json!({ "my.super.setting": "my.super.value" }));
    }

    #[test]
    fn write_overridable_settings() {
        let mut fixture = Fixture::new("overridable", true);
        let value = json!({ "configurationEditing.service.testSetting": "overridden value" });
        fixture.write("[language]", Some(value.clone()), Target::User).unwrap();
        assert_eq!(parsed(&fixture.user("settings.json"))["[language]"], value);
        fixture.write("[language]", Some(value.clone()), Target::Workspace).unwrap();
        assert_eq!(parsed(&fixture.folder("settings.json"))["[language]"], value);
        let resource = fixture.folder("settings.json");
        fixture.settings.update_value("[language]", Some(&value), Target::WorkspaceFolder, Some(&resource), &[]).unwrap();
        assert_eq!(parsed(&fixture.folder("settings.json"))["[language]"], value);
    }

    #[test]
    fn write_standalone_settings() {
        let mut fixture = Fixture::new("standalone", true);
        fixture.write("tasks.service.testSetting", Some(json!("value")), Target::Workspace).unwrap();
        assert_eq!(parsed(&fixture.folder("tasks.json"))["service.testSetting"], "value");
        fixture.write("tasks.service.testSetting", Some(json!("value")), Target::User).unwrap();
        assert_eq!(parsed(&fixture.user("tasks.json"))["service.testSetting"], "value");

        for (path, target) in [(fixture.folder("tasks.json"), Target::Workspace), (fixture.user("tasks.json"), Target::User), (fixture.user("mcp.json"), Target::User)] {
            write_file(&path, r#"{ "my.super.setting": "my.super.value" }"#);
            let key = if path.ends_with("mcp.json") { "mcp.service.testSetting" } else { "tasks.service.testSetting" };
            fixture.write(key, Some(json!("value")), target).unwrap();
            assert_eq!(parsed(&path), json!({ "my.super.setting": "my.super.value", "service.testSetting": "value" }));
        }

        let tasks = json!({ "version": "1.0.0", "tasks": [{ "taskName": "myTask" }] });
        for (path, target) in [(fixture.folder("tasks.json"), Target::Workspace), (fixture.user("tasks.json"), Target::User)] {
            for existing in [None, Some(r#"{ "my.super.setting": "my.super.value" }"#), Some(r#"{ "my.super.setting": "#)] {
                match existing {
                    Some(content) => write_file(&path, content),
                    None => std::fs::remove_file(&path).unwrap(),
                }
                fixture.write("tasks", Some(tasks.clone()), target).unwrap();
                assert_eq!(parsed(&path)["version"], "1.0.0");
                assert_eq!(parsed(&path)["tasks"][0]["taskName"], "myTask");
            }
            write_file(&path, "{\n\t\"version\": \"1.0.0\",\n\t\"tasks\": [\n\t\t{\n\t\t\t\"taskName\": \"myTask1\"\n\t\t},\n\t\t{\n\t\t\t\"taskName\": \"myTask2\"\n\t\t}\n\t]\n}");
            fixture.write("tasks", Some(json!({ "version": "1.0.0", "tasks": [{ "taskName": "myTask1" }] })), target).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\n\t\"version\": \"1.0.0\",\n\t\"tasks\": [\n\t\t{\n\t\t\t\"taskName\": \"myTask1\"\n\t\t}\n\t]\n}");
        }
    }

    #[test]
    fn writes_keep_comments_and_layout() {
        let mut fixture = Fixture::new("layout", false);
        write_file(&fixture.user("settings.json"), "{\n    // keep me\n    \"a.b\": 1\n}\n");
        fixture.write("configurationEditing.service.testSetting", Some(json!("value")), Target::User).unwrap();
        assert_eq!(std::fs::read_to_string(fixture.user("settings.json")).unwrap(), "{\n    // keep me\n    \"a.b\": 1,\n    \"configurationEditing.service.testSetting\": \"value\"\n}\n");
    }

    #[test]
    fn a_new_file_is_indented_with_the_configured_tab_size() {
        let mut fixture = Fixture::new("tab-size", true);
        fixture.write("editor.tabSize", Some(json!(3)), Target::User).unwrap();
        fixture.write("configurationEditing.service.testSetting", Some(json!("value")), Target::Workspace).unwrap();
        assert_eq!(std::fs::read_to_string(fixture.folder("settings.json")).unwrap(), "{\n   \"configurationEditing.service.testSetting\": \"value\"\n}");
    }

    #[test]
    fn values_layer_and_report_changes() {
        let mut fixture = Fixture::new("layers", true);
        let get = |settings: &Settings, overrides: &Overrides| settings.get_value(Some("configurationEditing.service.languageSetting"), overrides);
        assert_eq!(get(&fixture.settings, &Overrides::default()), Some(json!("isSet")));

        let change = fixture.write("configurationEditing.service.languageSetting", Some(json!("user")), Target::User).unwrap();
        assert_eq!(change.keys, ["configurationEditing.service.languageSetting"]);
        assert_eq!(get(&fixture.settings, &Overrides::default()), Some(json!("user")));

        fixture.write("configurationEditing.service.languageSetting", Some(json!("workspace")), Target::Workspace).unwrap();
        assert_eq!(get(&fixture.settings, &Overrides::default()), Some(json!("workspace")));

        let change = fixture.settings.update_value("configurationEditing.service.languageSetting", Some(&json!("markdown")), Target::User, None, &["markdown".into()]).unwrap();
        assert_eq!(change.overrides, [("markdown".to_string(), vec!["configurationEditing.service.languageSetting".to_string()])]);
        let markdown = Overrides { resource: None, override_identifier: Some("markdown") };
        assert_eq!(get(&fixture.settings, &markdown), Some(json!("markdown")), "a user language value beats a workspace value");
    }

    #[test]
    fn edits_on_disk_are_reloaded() {
        let mut fixture = Fixture::new("reload", true);
        assert!(fixture.settings.reload_changed_files().is_none());
        write_file(&fixture.user("settings.json"), r#"{ "configurationEditing.service.testSetting": "edited" }"#);
        let change = fixture.settings.reload_changed_files().expect("the new file is noticed");
        assert_eq!(change.keys, ["configurationEditing.service.testSetting"]);
        assert_eq!(fixture.settings.get_value(Some("configurationEditing.service.testSetting"), &Overrides::default()), Some(json!("edited")));

        write_file(&fixture.folder("settings.json"), r#"{ "configurationEditing.service.testSettingTwo": "folder", "configurationEditing.service.applicationSetting": "ignored" }"#);
        let change = fixture.settings.reload_changed_files().expect("the folder file is noticed");
        // Upstream compares the raw models, so the ignored key is still reported as changed.
        assert_eq!(change.keys, ["configurationEditing.service.testSettingTwo", "configurationEditing.service.applicationSetting"]);
        let application = fixture.settings.get_value(Some("configurationEditing.service.applicationSetting"), &Overrides::default());
        assert_eq!(application, Some(json!("isSet")), "application settings are not read from a folder");
        assert!(fixture.settings.reload_changed_files().is_none());
    }
}
