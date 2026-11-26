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
use crate::policy::{PolicyValues, policy_model};
use crate::model::{Change, ConfigurationModel, ParseFilter, compare, is_override_key, key_from_override_identifiers, merge_changes, parse_content, parse_raw, to_values_tree};
use crate::registry::{APPLICATION_SCOPES, FOLDER_SCOPES, PROFILE_SCOPES, Registry, Scope, WORKSPACE_SCOPES};
use crate::workspace::Workspace;

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

/// Port decision, not upstream: a folder's configuration is read from both, `.vsrustium` winning per key.
pub const FOLDER_CONFIG_DIRS: [&str; 2] = [".vscode", ".vsrustium"];

/// Where UI writes a folder's configuration: `.vsrustium` when that folder exists, else `.vscode`.
pub fn folder_config_dir(folder: &Path) -> PathBuf {
    let preferred = folder.join(".vsrustium");
    if preferred.is_dir() { preferred } else { folder.join(".vscode") }
}

/// `TASKS_DEFAULT`: what configurationEditing.ts writes into a new tasks.json before editing it.
const TASKS_DEFAULT: &str = "{\n\t\"version\": \"2.0.0\",\n\t\"tasks\": []\n}";

/// `APPLY_ALL_PROFILES_SETTING`: the settings a profile with its own settings reads from the default profile.
pub const APPLY_ALL_PROFILES_SETTING: &str = "workbench.settings.applyToAllProfiles";

/// The current profile's user files (`userDataProfileService.currentProfile`). `default_settings` is the default
/// profile's settings.json when the profile keeps its own settings, read as `ApplicationConfiguration`.
#[derive(Clone, Debug, PartialEq)]
pub struct UserFiles {
    pub settings: PathBuf,
    pub tasks: PathBuf,
    pub mcp: PathBuf,
    pub default_settings: Option<PathBuf>,
}

impl UserFiles {
    /// The default profile's files in `<user data>/User`.
    pub fn default_profile(user_home: &Path) -> UserFiles {
        UserFiles { settings: user_home.join("settings.json"), tasks: user_home.join("tasks.json"), mcp: user_home.join("mcp.json"), default_settings: None }
    }
}

pub struct Settings {
    pub registry: Registry,
    pub configuration: Configuration,
    /// `<user data>/User`, the default profile's home.
    user_home: PathBuf,
    user_files: UserFiles,
    /// The window's folders and workspace file; `None` is the empty window.
    pub workspace: Option<Workspace>,
    /// Size and modification time of every file a layer reads, to notice edits made outside the port.
    stamps: Vec<(PathBuf, Option<(SystemTime, u64)>)>,
    /// `isWorkspaceTrusted`: an untrusted window skips restricted and unregistered workspace settings.
    trusted: bool,
    /// What the policy service reports, kept to rebuild the policy layer when the registry changes.
    policy_values: PolicyValues,
}

impl Settings {
    /// `ConfigurationService.initialize` for an empty window (`workspace` is `None`), a folder or a workspace file.
    pub fn load(registry: Registry, user_home: PathBuf, workspace: Option<Workspace>) -> Settings {
        let user_files = UserFiles::default_profile(&user_home);
        Settings::load_with_profile(registry, user_home, user_files, workspace)
    }

    /// `load` in a profile: its settings, tasks and mcp files.
    pub fn load_with_profile(registry: Registry, user_home: PathBuf, user_files: UserFiles, workspace: Option<Workspace>) -> Settings {
        let defaults = registry.default_model();
        let has_workspace = workspace.is_some();
        let configuration = Configuration { defaults, has_workspace, ..Configuration::default() };
        let mut settings = Settings { registry, configuration, user_home, user_files, workspace, stamps: Vec::new(), trusted: true, policy_values: PolicyValues::new() };
        settings.configuration.application = settings.load_application();
        settings.configuration.local_user = settings.load_user();
        let folders: Vec<PathBuf> = settings.workspace.iter().flat_map(|workspace| workspace.folders.iter().map(|folder| folder.path.clone())).collect();
        settings.configuration.folders = folders.into_iter().map(|folder| (folder.clone(), settings.load_folder(&folder))).collect();
        settings.configuration.workspace = settings.load_workspace();
        settings.configuration.layers_changed();
        settings.stamps = settings.watched_files().into_iter().map(|path| (path.clone(), stamp(&path))).collect();
        settings
    }

    /// `onDefaultConfigurationChanged` after extensions were added or removed: the defaults of the new registry.
    pub fn set_registry(&mut self, registry: Registry) -> Change {
        let defaults = registry.default_model();
        let change = compare(Some(&self.configuration.defaults), Some(&defaults));
        self.configuration.defaults = defaults;
        self.configuration.layers_changed();
        self.registry = registry;
        let policy = policy_model(&self.registry, &self.policy_values);
        let mut changes = vec![change, self.configuration.compare_and_update_policy(policy)];
        // The application and profile layers are parsed against the registry's scopes.
        if self.user_files.default_settings.is_some() {
            changes.push(self.reload_user_layers());
        }
        merge_changes(&changes)
    }

    /// `PolicyConfiguration.onDidChangePolicies`: the policy service's values, applied to the settings that
    /// declare those policies.
    pub fn set_policy_values(&mut self, values: PolicyValues) -> Change {
        self.policy_values = values;
        let policy = policy_model(&self.registry, &self.policy_values);
        self.configuration.compare_and_update_policy(policy)
    }

    /// `WorkspaceConfiguration.setFolders` then `onWorkspaceConfigurationChanged`: writes the workspace file's
    /// `folders` and applies them. Errors without a workspace file or when the file cannot be written.
    pub fn write_workspace_folders(&mut self, folders: &[Value]) -> Result<Change, String> {
        let file = self.workspace_file().ok_or("Unable to write into workspace configuration file.")?.to_path_buf();
        let content = std::fs::read_to_string(&file).map_err(|error| error.to_string())?;
        let options = formatting_options(&content, &self.configuration, &file);
        let edits = set_property(&content, &[Segment::Key("folders".into())], Some(&Value::Array(folders.to_vec())), &options);
        std::fs::write(&file, crate::json_edit::apply_edits(&content, &edits)).map_err(|error| error.to_string())?;
        Ok(self.set_workspace_folders(Workspace::from_file(file).folders))
    }

    /// `onFoldersChanged`: the settings layers of removed folders go, those of added folders are read.
    fn set_workspace_folders(&mut self, folders: Vec<crate::workspace::WorkspaceFolder>) -> Change {
        let Some(workspace) = self.workspace.as_mut() else { return Change::default() };
        workspace.folders = folders;
        let paths: Vec<PathBuf> = workspace.folders.iter().map(|folder| folder.path.clone()).collect();
        let mut old = std::mem::take(&mut self.configuration.folders);
        let mut changes: Vec<Change> = old.iter().filter(|(folder, _)| !paths.contains(folder)).map(|(_, model)| compare(Some(model), None)).collect();
        for path in paths {
            let model = match old.iter().position(|(folder, _)| *folder == path) {
                Some(index) => old.swap_remove(index).1,
                None => {
                    let model = self.load_folder(&path);
                    changes.push(compare(None, Some(&model)));
                    model
                }
            };
            self.configuration.folders.push((path, model));
        }
        self.configuration.layers_changed();
        self.stamps = self.watched_files().into_iter().map(|path| (path.clone(), stamp(&path))).collect();
        merge_changes(&changes)
    }

    /// `updateWorkspaceTrust`: reparses the folder and workspace settings and answers the keys whose folder or
    /// workspace layers gained or lost: the restricted and unregistered keys set there (upstream also lists the
    /// restricted keys of the user layers, whose values do not change).
    pub fn update_workspace_trust(&mut self, trusted: bool) -> Change {
        self.trusted = trusted;
        let before = self.configuration.clone();
        let folders: Vec<PathBuf> = self.configuration.folders.iter().map(|(folder, _)| folder.clone()).collect();
        self.configuration.folders = folders.into_iter().map(|folder| (folder.clone(), self.load_folder(&folder))).collect();
        self.configuration.workspace = self.load_workspace();
        self.configuration.layers_changed();
        let mut layers = vec![(&before.workspace, &self.configuration.workspace)];
        layers.extend(before.folders.iter().zip(&self.configuration.folders).map(|((_, from), (_, to))| (from, to)));
        let mut keys: Vec<String> = Vec::new();
        for (from, to) in layers {
            for key in from.keys.iter().filter(|key| !to.keys.contains(key)).chain(to.keys.iter().filter(|key| !from.keys.contains(key))) {
                if !keys.contains(key) {
                    keys.push(key.clone());
                }
            }
        }
        Change { keys, overrides: Vec::new() }
    }

    /// `onDidChangeCurrentProfile`: the user layers of the new profile and what changed.
    /// The change is `Configuration.compare`: the keys of either profile whose value differs.
    pub fn set_profile(&mut self, user_files: UserFiles) -> Change {
        let before = self.configuration.clone();
        self.user_files = user_files;
        self.stamps = self.watched_files().into_iter().map(|path| (path.clone(), stamp(&path))).collect();
        let overrides = self.reload_user_layers().overrides;
        let after = &self.configuration;
        let candidates = before.application.keys.iter().chain(&before.local_user.keys).chain(&after.application.keys).chain(&after.local_user.keys);
        let mut keys: Vec<String> = Vec::new();
        for key in candidates {
            if !keys.contains(key) && before.get_value(Some(key), &Overrides::default()) != after.get_value(Some(key), &Overrides::default()) {
                keys.push(key.clone());
            }
        }
        Change { keys, overrides }
    }

    pub fn user_files(&self) -> &UserFiles {
        &self.user_files
    }

    /// `isSettingAppliedForAllProfiles`: application scoped, or listed in `workbench.settings.applyToAllProfiles`.
    pub fn is_applied_for_all_profiles(&self, key: &str) -> bool {
        if self.registry.property(key).is_some_and(|property| APPLICATION_SCOPES.contains(&property.scope)) {
            return true;
        }
        let all_profiles = self.get_value(Some(APPLY_ALL_PROFILES_SETTING), &Overrides::default());
        all_profiles.as_ref().and_then(Value::as_array).is_some_and(|keys| keys.iter().any(|listed| listed == key))
    }

    /// `<user data>/User`, `environmentService.userRoamingDataHome`.
    pub fn user_home(&self) -> &Path {
        &self.user_home
    }

    /// `getValue(section, overrides)`.
    pub fn get_value(&self, section: Option<&str>, overrides: &Overrides) -> Option<Value> {
        self.configuration.get_value(section, overrides)
    }

    /// Reloads the layers whose files changed on disk since they were read, as the file watchers of
    /// `UserConfiguration` and `FolderConfiguration` do. `None` when no setting changed.
    pub fn reload_changed_files(&mut self) -> Option<Change> {
        let mut changed_files = Vec::new();
        for (path, last) in &mut self.stamps {
            let now = stamp(path);
            if now != *last {
                *last = now;
                changed_files.push(path.clone());
            }
        }
        let mut changes = Vec::new();
        if changed_files.iter().any(|path| self.is_user_file(path)) {
            changes.push(self.reload_user_layers());
        }
        if let Some(file) = self.workspace_file().filter(|file| changed_files.iter().any(|path| path == file)).map(Path::to_path_buf) {
            let folders = Workspace::from_file(file).folders;
            if self.workspace.as_ref().is_some_and(|workspace| workspace.folders != folders) {
                changes.push(self.set_workspace_folders(folders));
            }
            let workspace = self.load_workspace();
            changes.push(self.configuration.compare_and_update_workspace(workspace));
        }
        let single_folder = self.workspace_file().is_none();
        let folders: Vec<PathBuf> = self.configuration.folders.iter().map(|(folder, _)| folder.clone()).collect();
        for folder in folders {
            if changed_files.iter().any(|path| FOLDER_CONFIG_DIRS.iter().any(|dir| path.parent() == Some(folder.join(dir).as_path()))) {
                let model = self.load_folder(&folder);
                changes.push(self.configuration.compare_and_update_folder(&folder, model, single_folder));
            }
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

    /// `updateValue(key, value)` without a target, as the workbench calls it: written where the key is already
    /// set (`deriveConfigurationTargets`), else into user settings, where the default value removes the key.
    pub fn update_value_derived(&mut self, key: &str, value: Option<&Value>) -> Result<Change, String> {
        let inspected = self.configuration.inspect(key, &Overrides::default());
        if value.cloned() == inspected.value {
            return Ok(Change::default());
        }
        let mut targets = Vec::new();
        if inspected.workspace_folder.value.is_some() {
            targets.push(Target::WorkspaceFolder);
        }
        if inspected.workspace.value.is_some() {
            targets.push(Target::Workspace);
        }
        if inspected.user_local.value.is_some() {
            targets.push(Target::UserLocal);
        }
        if inspected.application.value.is_some() {
            targets.push(Target::Application);
        }
        if value.is_some() {
            targets = vec![targets.first().copied().unwrap_or(Target::User)];
        }
        let value = match targets[..] {
            [Target::User | Target::UserLocal] if value.cloned() == inspected.default_value => None,
            _ => value,
        };
        let mut changes = Vec::new();
        for target in targets {
            changes.push(self.update_value(key, value, target, None, &[])?);
        }
        Ok(merge_changes(&changes))
    }

    /// preferencesService.ts `getEditableSettingsURI` then `createSettingsIfNotExists`: the target's settings file,
    /// written as `{\n}` when missing, or the workspace file given a `settings` object. `None` without a folder.
    pub fn create_settings_if_not_exists(&self, target: Target, resource: Option<&Path>) -> Option<Result<PathBuf, String>> {
        let editable = match target {
            Target::Workspace => Editable::Workspace,
            Target::WorkspaceFolder => Editable::WorkspaceFolder,
            _ => Editable::UserLocal,
        };
        // `openApplicationSettings`: the default profile's settings.json.
        let path = match (target, &self.user_files.default_settings) {
            (Target::Application, Some(default_settings)) => default_settings.clone(),
            _ => self.file_for(editable, "settings.json", resource)?,
        };
        if self.is_workspace_file(Some(&path)) {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            let parsed = json::parse(&content, &mut Vec::new(), ParseOptions { allow_trailing_comma: true, allow_empty_content: true, ..ParseOptions::default() });
            if parsed.is_some_and(|workspace| workspace.get("settings").is_none()) {
                let options = formatting_options(&content, &self.configuration, &path);
                let edits = set_property(&content, &[Segment::Key("settings".into())], Some(&Value::Object(Map::new())), &options);
                if let Err(error) = write_file(&path, &crate::json_edit::apply_edits(&content, &edits)) {
                    return Some(Err(format!("Unable to create '{}' ({error}).", path.display())));
                }
            }
            return Some(Ok(path));
        }
        if !path.exists()
            && let Err(error) = self.write_settings_file(&path, "{\n}")
        {
            return Some(Err(format!("Unable to create '{}' ({error}).", path.display())));
        }
        Some(Ok(path))
    }

    fn reload_after_write(&mut self, editable: Editable) -> Change {
        for (path, last) in &mut self.stamps {
            *last = stamp(path);
        }
        if editable == Editable::UserLocal {
            return self.reload_user_layers();
        }
        let mut changes = Vec::new();
        if self.workspace_file().is_some() {
            let workspace = self.load_workspace();
            changes.push(self.configuration.compare_and_update_workspace(workspace));
        }
        let single_folder = self.workspace_file().is_none();
        let folders: Vec<PathBuf> = self.configuration.folders.iter().map(|(folder, _)| folder.clone()).collect();
        for folder in folders {
            let model = self.load_folder(&folder);
            changes.push(self.configuration.compare_and_update_folder(&folder, model, single_folder));
        }
        merge_changes(&changes)
    }

    /// `onApplicationConfigurationChanged` then `onLocalUserConfigurationChanged`: of the application layer's changed
    /// keys only the application scoped ones and those applied to all profiles count, plus the keys that joined or
    /// left `workbench.settings.applyToAllProfiles`.
    fn reload_user_layers(&mut self) -> Change {
        let previous_all = string_list(self.configuration.application.get_value(Some(APPLY_ALL_PROFILES_SETTING)));
        let application = self.load_application();
        let mut application_change = self.configuration.compare_and_update_application(application);
        let current_all = string_list(self.get_value(Some(APPLY_ALL_PROFILES_SETTING), &Overrides::default()));
        let mut keys = Vec::new();
        for key in application_change.keys {
            if !self.registry.property(&key).is_some_and(|property| APPLICATION_SCOPES.contains(&property.scope)) {
                if current_all.contains(&key) {
                    keys.push(key);
                }
                continue;
            }
            if key == APPLY_ALL_PROFILES_SETTING {
                keys.push(key);
                keys.extend(previous_all.iter().filter(|setting| !current_all.contains(setting)).cloned());
                keys.extend(current_all.iter().filter(|setting| !previous_all.contains(setting)).cloned());
                continue;
            }
            keys.push(key);
        }
        application_change.keys = keys;
        let user = self.load_user();
        merge_changes(&[application_change, self.configuration.compare_and_update_local_user(user)])
    }

    fn is_user_file(&self, path: &Path) -> bool {
        let files = &self.user_files;
        [&files.settings, &files.tasks, &files.mcp].into_iter().chain(&files.default_settings).any(|file| file == path)
    }

    fn workspace_file(&self) -> Option<&Path> {
        self.workspace.as_ref().and_then(|workspace| workspace.file.as_deref())
    }

    /// The folder of `resource`, as `IWorkspaceContextService.getWorkspaceFolder` finds it.
    fn folder_of(&self, resource: Option<&Path>) -> Option<&Path> {
        Some(self.workspace.as_ref()?.folder_of(resource?)?.path.as_path())
    }

    fn watched_files(&self) -> Vec<PathBuf> {
        let user_files = &self.user_files;
        let mut files: Vec<PathBuf> = [&user_files.settings, &user_files.tasks, &user_files.mcp].into_iter().chain(&user_files.default_settings).cloned().collect();
        files.extend(self.workspace_file().map(Path::to_path_buf));
        for (folder, _) in &self.configuration.folders {
            for dir in FOLDER_CONFIG_DIRS {
                files.extend(["settings.json", "tasks.json", "launch.json", "mcp.json"].iter().map(|name| folder.join(dir).join(name)));
            }
        }
        files
    }

    /// `ApplicationConfiguration`: in a profile with its own settings, the default profile's application scoped
    /// settings and those listed in `workbench.settings.applyToAllProfiles`; empty otherwise.
    fn load_application(&self) -> ConfigurationModel {
        let Some(default_settings) = &self.user_files.default_settings else { return ConfigurationModel::default() };
        let content = read_or_empty(default_settings);
        let filter = ParseFilter { scopes: Some(APPLICATION_SCOPES.to_vec()), skip_unregistered: true, ..ParseFilter::default() };
        let model = parse_content(&content, Some(&self.registry), &filter).model;
        let include = string_list(model.get_value(Some(APPLY_ALL_PROFILES_SETTING)));
        match include.is_empty() {
            true => model,
            false => parse_content(&content, Some(&self.registry), &ParseFilter { include, ..filter }).model,
        }
    }

    /// `UserConfiguration`: the profile's settings.json, every scope in the default profile and PROFILE_SCOPES less
    /// the settings applied to all profiles in another, plus tasks.json and mcp.json.
    fn load_user(&self) -> ConfigurationModel {
        let filter = match self.user_files.default_settings {
            Some(_) => ParseFilter { scopes: Some(PROFILE_SCOPES.to_vec()), exclude: string_list(self.configuration.application.get_value(Some(APPLY_ALL_PROFILES_SETTING))), ..ParseFilter::default() },
            None => ParseFilter::default(),
        };
        let settings = parse_content(&read_or_empty(&self.user_files.settings), Some(&self.registry), &filter).model;
        let standalone = [("tasks", &self.user_files.tasks), ("mcp", &self.user_files.mcp)].map(|(name, path)| standalone_model(name, path));
        settings.merge(&standalone.into_iter().flatten().collect::<Vec<_>>())
    }

    /// `FolderConfiguration` of each of FOLDER_CONFIG_DIRS, the later winning per key.
    fn load_folder(&self, folder: &Path) -> ConfigurationModel {
        let models: Vec<ConfigurationModel> = FOLDER_CONFIG_DIRS.iter().map(|dir| self.load_folder_dir(&folder.join(dir))).collect();
        models[0].merge(&models[1..])
    }

    /// `.vscode/settings.json` with WORKSPACE_SCOPES in a folder window and FOLDER_SCOPES in a workspace (the window
    /// is trusted, as the reference runs with workspace trust off), plus tasks.json, launch.json and mcp.json.
    fn load_folder_dir(&self, directory: &Path) -> ConfigurationModel {
        let scopes = if self.workspace_file().is_some() { FOLDER_SCOPES.to_vec() } else { WORKSPACE_SCOPES.to_vec() };
        let filter = ParseFilter { scopes: Some(scopes), skip_restricted: !self.trusted, skip_unregistered: !self.trusted, ..ParseFilter::default() };
        let settings = parse_content(&read_or_empty(&directory.join("settings.json")), Some(&self.registry), &filter).model;
        let standalone = ["tasks", "launch", "mcp"].map(|name| standalone_model(name, &directory.join(format!("{name}.json"))));
        settings.merge(&standalone.into_iter().flatten().collect::<Vec<_>>())
    }

    /// The workspace layer: the folder's model in a folder window; in a workspace, the workspace file's `settings`
    /// with WORKSPACE_SCOPES plus its `launch` and `tasks` (`WorkspaceConfigurationModelParser`).
    fn load_workspace(&self) -> ConfigurationModel {
        let Some(file) = self.workspace_file() else {
            return self.configuration.folders.first().map(|(_, model)| model.clone()).unwrap_or_default();
        };
        let content = std::fs::read_to_string(file).unwrap_or_default();
        let raw = match json::parse(&content, &mut Vec::new(), ParseOptions { allow_trailing_comma: true, allow_empty_content: true, ..ParseOptions::default() }) {
            Some(Value::Object(raw)) => raw,
            _ => Map::new(),
        };
        let filter = ParseFilter { scopes: Some(WORKSPACE_SCOPES.to_vec()), skip_restricted: !self.trusted, skip_unregistered: !self.trusted, ..ParseFilter::default() };
        let settings = raw.get("settings").and_then(Value::as_object).cloned().unwrap_or_default();
        let (settings, _) = parse_raw(settings, Some(&self.registry), &filter);
        let standalone = ["launch", "tasks"].map(|name| raw.get(name).and_then(Value::as_object).map(|section| standalone_section(name, section)));
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
            // In the workspace file the standalone sections are top-level keys.
            let section = match self.is_workspace_file(path.as_deref()) {
                true => vec![Segment::Key(name.to_string())],
                false => Vec::new(),
            };
            if key == *name {
                return Operation { key: key.to_owned(), json_path: section, path, standalone: Some(name) };
            }
            if let Some(rest) = key.strip_prefix(&format!("{name}.")) {
                let json_path = section.into_iter().chain([Segment::Key(rest.to_owned())]).collect();
                return Operation { key: rest.to_owned(), json_path, path, standalone: Some(name) };
            }
        }
        let mut json_path = vec![Segment::Key(key.to_owned())];
        if !override_identifiers.is_empty() {
            json_path.insert(0, Segment::Key(key_from_override_identifiers(override_identifiers)));
        }
        let path = match editable {
            Editable::UserLocal => match &self.user_files.default_settings {
                Some(default_settings) if self.is_applied_for_all_profiles(key) => Some(default_settings.clone()),
                _ => Some(self.user_files.settings.clone()),
            },
            _ => self.file_for(editable, "settings.json", resource),
        };
        if self.is_workspace_file(path.as_deref()) {
            json_path.insert(0, Segment::Key("settings".into()));
        }
        Operation { key: key.to_owned(), json_path, path, standalone: None }
    }

    /// `getConfigurationFileResource`: the user file, the workspace file, or the file in a folder's `folder_config_dir`.
    fn file_for(&self, editable: Editable, name: &str, resource: Option<&Path>) -> Option<PathBuf> {
        match editable {
            Editable::UserLocal => Some(match name {
                "tasks.json" => self.user_files.tasks.clone(),
                "mcp.json" => self.user_files.mcp.clone(),
                _ => self.user_files.settings.clone(),
            }),
            Editable::Workspace => match self.workspace_file() {
                Some(file) => Some(file.to_path_buf()),
                None => self.configuration.folders.first().map(|(folder, _)| folder_config_dir(folder).join(name)),
            },
            Editable::WorkspaceFolder => self.folder_of(resource).map(|folder| folder_config_dir(folder).join(name)),
        }
    }

    /// `isWorkspaceConfigurationResource`.
    fn is_workspace_file(&self, path: Option<&Path>) -> bool {
        path.is_some() && path == self.workspace_file()
    }

    /// `ConfigurationEditing.validate`.
    fn validate(&self, editable: Editable, operation: &Operation, value: Option<&Value>, override_identifiers: &[String]) -> Result<(), String> {
        let key = &operation.key;
        if self.configuration.inspect(key, &Overrides::default()).policy_value.is_some() {
            return Err(self.error(Error::PolicyConfiguration, editable, key));
        }
        let scope = self.registry.property(key).map(|property| property.scope);
        if operation.standalone.is_none() && value.is_some() && !is_override_key(key) && !self.registry.keys().contains(key) {
            return Err(self.error(Error::UnknownKey, editable, key));
        }
        if operation.standalone.is_some_and(|name| name == "launch") && editable == Editable::UserLocal {
            return Err(self.error(Error::InvalidUserTarget, editable, key));
        }
        if editable != Editable::UserLocal && self.workspace.is_none() {
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
                self.write_settings_file(path, initial).map_err(|error| self.fs_error(editable, &error))?;
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
            self.write_settings_file(path, &new_content).map_err(|error| self.fs_error(editable, &error))?;
        }
        Ok(())
    }

    /// A profile's settings and tasks files are written atomically (`updateAtomicReadWritesResources`), the rest in place.
    fn write_settings_file(&self, path: &Path, content: &str) -> std::io::Result<()> {
        let atomic = path.starts_with(&self.user_home) && path.file_name().is_some_and(|name| name == "settings.json" || name == "tasks.json");
        match atomic {
            true => write_user_data_file(path, content.as_bytes()),
            false => write_file(path, content),
        }
    }

    /// `toErrorMessage` for the API constraint errors `validate` raises.
    fn error(&self, error: Error, editable: Editable, key: &str) -> String {
        match error {
            Error::PolicyConfiguration => format!("Unable to write {key} because it is configured in system policy."),
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
                let folder = operation.path.as_deref().and_then(|path| self.folder_of(Some(path)));
                let name = folder.and_then(|folder| folder.file_name()).map_or("<<unknown>>".into(), |name| name.to_string_lossy());
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
    PolicyConfiguration,
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
    Some(standalone_section(name, &raw))
}

/// A tasks, launch or mcp object as the `name` section.
fn standalone_section(name: &str, raw: &Map<String, Value>) -> ConfigurationModel {
    let keys = raw.keys().map(|key| format!("{name}.{key}")).collect();
    let mut contents = Map::new();
    contents.insert(name.to_owned(), Value::Object(to_values_tree(raw, &mut |_| {})));
    ConfigurationModel::new(contents, keys, Vec::new())
}

/// The text a settings file parses from: a missing or empty file reads as `{}`, as `UserSettings` and
/// `FileServiceBasedConfiguration.resolveContents` do.
fn string_list(value: Option<Value>) -> Vec<String> {
    let values = value.as_ref().and_then(Value::as_array).into_iter().flatten();
    values.filter_map(|value| value.as_str().map(str::to_owned)).collect()
}

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

/// fileUserDataProvider.ts `enforceAtomicWriteFile` (`.vsctmp`) through diskFileSystemProvider.ts `doWriteFileAtomic`:
/// a temporary file renamed over the target; a symbolic link is written in place (`canWriteFileAtomic`).
pub fn write_user_data_file(path: &Path, content: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return std::fs::write(path, content);
    }
    let temporary = PathBuf::from(format!("{}.vsctmp", path.display()));
    std::fs::write(&temporary, content)?;
    std::fs::rename(&temporary, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

/// `getFormattingOptions(model)`: the indentation of the file's text model (language `jsonc`) and its line
/// break, `\n` on macOS for a file with none.
pub fn formatting_options(content: &str, configuration: &Configuration, path: &Path) -> FormattingOptions {
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
/// temporary directory. Left out: dirty-file, Save and Retry cases (no text file model).
#[cfg(test)]
mod profile_tests;

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
            Fixture::with_workspace(name, |root| with_folder.then(|| Workspace::folder(root.join("folder"))))
        }

        fn with_workspace(name: &str, workspace: impl FnOnce(&Path) -> Option<Workspace>) -> Fixture {
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
                "configurationEditing.service.restrictedSetting": { "type": "string", "default": "isSet", "restricted": true },
                "configurationEditing.service.policySetting": { "type": "string", "default": "isSet", "policy": { "name": "configurationEditing.service.policySetting" } },
                "configurationEditing.service.policyObjectSetting": { "type": "object", "default": {}, "policy": { "name": "configurationEditing.service.policyObjectSetting" } },
            });
            registry.register_configuration("test", &json!({ "contributes": { "configuration": { "id": "_test", "properties": properties } } }));
            registry.finish();
            let settings = Settings::load(registry, root.join("User"), workspace(&root));
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
    fn user_settings_are_written_atomically_and_folder_settings_in_place() {
        use std::os::unix::fs::MetadataExt;
        let mut fixture = Fixture::new("atomic", true);
        write_file(&fixture.user("settings.json"), "{}");
        write_file(&fixture.folder("settings.json"), "{}");
        let inode = |path: &Path| std::fs::metadata(path).unwrap().ino();
        let (user, folder) = (inode(&fixture.user("settings.json")), inode(&fixture.folder("settings.json")));
        fixture.write("configurationEditing.service.testSetting", Some(json!("value")), Target::UserLocal).unwrap();
        fixture.write("configurationEditing.service.testSetting", Some(json!("value")), Target::Workspace).unwrap();
        assert_ne!(inode(&fixture.user("settings.json")), user, "renamed over");
        assert_eq!(inode(&fixture.folder("settings.json")), folder, "written in place");
        assert!(!fixture.user("settings.json.vsctmp").exists());

        let target = fixture.root.join("linked.json");
        write_file(&target, "{}");
        std::fs::remove_file(fixture.user("settings.json")).unwrap();
        std::os::unix::fs::symlink(&target, fixture.user("settings.json")).unwrap();
        fixture.write("configurationEditing.service.testSettingTwo", Some(json!("value")), Target::UserLocal).unwrap();
        assert!(std::fs::symlink_metadata(fixture.user("settings.json")).unwrap().file_type().is_symlink());
        assert_eq!(parsed(&target)["configurationEditing.service.testSettingTwo"], json!("value"));
    }

    #[test]
    fn a_policy_value_blocks_writes_and_overrides_every_layer() {
        let mut fixture = Fixture::new("policy", true);
        write_file(&fixture.user("settings.json"), r#"{ "configurationEditing.service.policyObjectSetting": {"b": true} }"#);
        fixture.settings.reload_changed_files();
        let values = PolicyValues::from([
            ("configurationEditing.service.policySetting".to_string(), json!("policyValue")),
            ("configurationEditing.service.policyObjectSetting".to_string(), json!(r#"{"a": true}"#)),
        ]);
        let mut keys = fixture.settings.set_policy_values(values).keys;
        keys.sort();
        assert_eq!(keys, ["configurationEditing.service.policyObjectSetting", "configurationEditing.service.policySetting"]);

        let policy_setting = fixture.settings.configuration.inspect("configurationEditing.service.policySetting", &Overrides::default());
        assert_eq!((policy_setting.value, policy_setting.policy_value), (Some(json!("policyValue")), Some(json!("policyValue"))));
        assert_eq!(fixture.settings.get_value(Some("configurationEditing.service.policyObjectSetting"), &Overrides::default()), Some(json!({ "a": true })));
        let error = fixture.write("configurationEditing.service.policySetting", Some(json!("value")), Target::UserLocal).unwrap_err();
        assert_eq!(error, "Unable to write configurationEditing.service.policySetting because it is configured in system policy.");

        let change = fixture.settings.set_policy_values(PolicyValues::new());
        assert_eq!(change.keys.len(), 2);
        assert_eq!(fixture.settings.get_value(Some("configurationEditing.service.policyObjectSetting"), &Overrides::default()), Some(json!({ "b": true })));
        fixture.write("configurationEditing.service.policySetting", Some(json!("value")), Target::UserLocal).unwrap();
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
    fn vsrustium_wins_over_vscode_and_takes_writes_when_present() {
        let mut fixture = Fixture::new("vsrustium", true);
        let (test, two) = (Some("configurationEditing.service.testSetting"), Some("configurationEditing.service.testSettingTwo"));
        write_file(&fixture.folder("settings.json"), r#"{ "configurationEditing.service.testSetting": "vscode", "configurationEditing.service.testSettingTwo": "vscode" }"#);
        fixture.settings.reload_changed_files().unwrap();
        fixture.write("configurationEditing.service.testSetting", Some(json!("written")), Target::Workspace).unwrap();
        assert_eq!(parsed(&fixture.folder("settings.json"))["configurationEditing.service.testSetting"], "written", "without .vsrustium writes go to .vscode");

        let vsrustium = fixture.root.join("folder/.vsrustium/settings.json");
        write_file(&vsrustium, r#"{ "configurationEditing.service.testSetting": "vsrustium" }"#);
        fixture.settings.reload_changed_files().expect("the .vsrustium file is watched");
        assert_eq!(fixture.settings.get_value(test, &Overrides::default()), Some(json!("vsrustium")));
        assert_eq!(fixture.settings.get_value(two, &Overrides::default()), Some(json!("vscode")));

        fixture.write("configurationEditing.service.testSettingTwo", Some(json!("written")), Target::Workspace).unwrap();
        assert_eq!(parsed(&vsrustium)["configurationEditing.service.testSettingTwo"], "written");
        assert_eq!(fixture.settings.get_value(two, &Overrides::default()), Some(json!("written")));
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
    fn a_write_without_target_goes_where_the_key_is_set() {
        let key = "configurationEditing.service.windowSetting";
        let mut fixture = Fixture::new("derived", true);
        fixture.settings.update_value_derived(key, Some(&json!("user"))).unwrap();
        assert_eq!(parsed(&fixture.user("settings.json"))[key], "user");
        let change = fixture.settings.update_value_derived(key, Some(&json!("isSet"))).unwrap();
        assert_eq!(change.keys, [key]);
        assert_eq!(parsed(&fixture.user("settings.json")), json!({}), "the default value removes the user key");

        write_file(&fixture.folder("settings.json"), &json!({ key: "folder" }).to_string());
        fixture.settings.reload_changed_files();
        fixture.settings.update_value_derived(key, Some(&json!("workspace"))).unwrap();
        assert_eq!(parsed(&fixture.folder("settings.json"))[key], "workspace");
        assert_eq!(parsed(&fixture.user("settings.json")), json!({}), "a key set in the folder is written there");
        assert!(fixture.settings.update_value_derived(key, Some(&json!("workspace"))).unwrap().keys.is_empty(), "an unchanged value writes nothing");
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

    /// platform/configuration's configurationService.test.ts cases that read one user settings file.
    #[test]
    fn platform_configuration_service_cases() {
        let root = std::env::temp_dir().join(format!("config-service-platform-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let load = || {
            let mut registry = Registry::core();
            let properties = json!({
                "configuration.service.testSetting": { "type": "string", "default": "isSet" },
                "lookup.service.testNullSetting": { "type": "null" },
            });
            registry.register_configuration("test", &json!({ "contributes": { "configuration": { "id": "_test", "properties": properties } } }));
            registry.finish();
            Settings::load(registry, root.join("User"), None)
        };
        let settings_file = root.join("User/settings.json");
        let value = |settings: &Settings, key: &str| settings.get_value(Some(key), &Overrides::default());

        let mut settings = load();
        assert_eq!(value(&settings, "configuration.service.testSetting"), Some(json!("isSet")), "missing file does not explode, model defaults");

        write_file(&settings_file, r#"{ "foo": "bar", "testworkbench.editor.tabs": true }"#);
        settings.reload_changed_files().expect("the created file fires a change");
        assert_eq!(value(&settings, "foo"), Some(json!("bar")));
        assert_eq!(value(&settings, "testworkbench"), Some(json!({ "editor": { "tabs": true } })), "config gets flattened");

        write_file(&settings_file, ",,,,");
        assert!(load().get_value(None, &Overrides::default()).is_some(), "error case does not explode");

        let missing = settings.configuration.inspect("something.missing", &Overrides::default());
        assert_eq!((missing.value, missing.default_value, missing.user_local.value), (None, None, None));

        write_file(&settings_file, r#"{ "configuration.service.testSetting": "bar" }"#);
        settings.reload_changed_files().unwrap();
        let lookup = settings.configuration.inspect("configuration.service.testSetting", &Overrides::default());
        assert_eq!((lookup.value, lookup.default_value, lookup.user_local.value), (Some(json!("bar")), Some(json!("isSet")), Some(json!("bar"))));

        let null = settings.configuration.inspect("lookup.service.testNullSetting", &Overrides::default());
        assert_eq!((null.value, null.default_value, null.user_local.value), (Some(Value::Null), Some(Value::Null), None));
        write_file(&settings_file, r#"{ "lookup.service.testNullSetting": null }"#);
        settings.reload_changed_files().unwrap();
        let null = settings.configuration.inspect("lookup.service.testNullSetting", &Overrides::default());
        assert_eq!((null.value, null.default_value, null.user_local.value), (Some(Value::Null), Some(Value::Null), Some(Value::Null)), "lookup with null");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_workspace_file_is_the_workspace_layer() {
        let mut fixture = Fixture::with_workspace("multi-root", |root| {
            let file = root.join("test.code-workspace");
            let settings = r#""settings": { "configurationEditing.service.windowSetting": "workspace" }"#;
            write_file(&file, &format!(r#"{{ "folders": [{{ "path": "folder" }}, {{ "path": "folder2" }}], {settings}, "launch": {{ "version": "0.2.0" }} }}"#));
            Some(Workspace::from_file(file))
        });
        let window = Some("configurationEditing.service.windowSetting");
        assert_eq!(fixture.settings.get_value(window, &Overrides::default()), Some(json!("workspace")));
        assert_eq!(fixture.settings.get_value(Some("launch.version"), &Overrides::default()), Some(json!("0.2.0")));

        let folder2 = fixture.root.join("folder2/.vscode/settings.json");
        write_file(&folder2, r#"{ "configurationEditing.service.windowSetting": "folder", "configurationEditing.service.languageSetting": "folder2" }"#);
        fixture.settings.reload_changed_files().expect("the second folder's file is noticed");
        let in_folder2 = Overrides { resource: Some(&folder2), override_identifier: None };
        assert_eq!(fixture.settings.get_value(window, &in_folder2), Some(json!("workspace")), "window settings are not read from a folder of a workspace");
        let language = Some("configurationEditing.service.languageSetting");
        assert_eq!(fixture.settings.get_value(language, &in_folder2), Some(json!("folder2")));
        assert_eq!(fixture.settings.get_value(language, &Overrides::default()), Some(json!("isSet")));

        fixture.write("configurationEditing.service.testSetting", Some(json!("value")), Target::Workspace).unwrap();
        fixture.write("tasks.version", Some(json!("2.0.0")), Target::Workspace).unwrap();
        let file = parsed(&fixture.root.join("test.code-workspace"));
        assert_eq!(file["settings"]["configurationEditing.service.testSetting"], "value");
        assert_eq!(file["tasks"]["version"], "2.0.0");
        assert_eq!(file["folders"][1]["path"], "folder2");
        assert_eq!(fixture.settings.get_value(Some("configurationEditing.service.testSetting"), &Overrides::default()), Some(json!("value")));

        let resource = fixture.root.join("folder2/a.txt");
        fixture.settings.update_value("configurationEditing.service.languageSetting", Some(&json!("two")), Target::WorkspaceFolder, Some(&resource), &[]).unwrap();
        assert_eq!(parsed(&folder2)["configurationEditing.service.languageSetting"], "two");
        let error = fixture.settings.update_value("configurationEditing.service.testSetting", Some(&json!("two")), Target::WorkspaceFolder, Some(&resource), &[]).unwrap_err();
        assert_eq!(error, "Unable to write to Folder Settings because configurationEditing.service.testSetting does not support the folder resource scope.");
    }

    #[test]
    fn an_untrusted_window_skips_restricted_and_unregistered_workspace_settings() {
        let mut fixture = Fixture::new("trust", true);
        let folder_settings = r#"{ "configurationEditing.service.restrictedSetting": "folder", "configurationEditing.service.testSetting": "folder", "unregistered.setting": "folder" }"#;
        write_file(&fixture.folder("settings.json"), folder_settings);
        write_file(&fixture.user("settings.json"), r#"{ "configurationEditing.service.restrictedSetting": "user" }"#);
        fixture.settings.reload_changed_files();
        let value = |fixture: &Fixture, key| fixture.settings.get_value(Some(key), &Overrides::default());
        assert_eq!(value(&fixture, "configurationEditing.service.restrictedSetting"), Some(json!("folder")));
        assert_eq!(value(&fixture, "unregistered.setting"), Some(json!("folder")));

        let change = fixture.settings.update_workspace_trust(false);
        assert!(change.keys.contains(&"configurationEditing.service.restrictedSetting".to_owned()));
        assert!(!change.keys.contains(&"configurationEditing.service.testSetting".to_owned()));
        assert_eq!(value(&fixture, "configurationEditing.service.restrictedSetting"), Some(json!("user")));
        assert_eq!(value(&fixture, "unregistered.setting"), None);
        assert_eq!(value(&fixture, "configurationEditing.service.testSetting"), Some(json!("folder")));

        fixture.settings.update_workspace_trust(true);
        assert_eq!(value(&fixture, "configurationEditing.service.restrictedSetting"), Some(json!("folder")));
    }
}
