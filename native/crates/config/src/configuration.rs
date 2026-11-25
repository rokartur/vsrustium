//! The `Configuration` of platform/configuration/common/configurationModels.ts: the layers of one window and
//! the merged view `getValue` reads. The port has no policies, remote, profiles or in-memory values yet, so
//! those layers are empty and left out; `to_data` still sends them as upstream's `toData` does.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::model::{Change, ConfigurationModel, compare, merge_changes};

#[derive(Clone, Debug, Default)]
pub struct Configuration {
    pub defaults: ConfigurationModel,
    pub local_user: ConfigurationModel,
    pub workspace: ConfigurationModel,
    /// Each workspace folder and its `.vscode` settings; a folder window has one.
    pub folders: Vec<(PathBuf, ConfigurationModel)>,
    /// The values policies force (`PolicyConfiguration`), over every other layer.
    pub policy: ConfigurationModel,
    /// `WorkbenchState` is not EMPTY: a folder or a workspace file is open, so the workspace layer counts.
    pub has_workspace: bool,
    /// `_workspaceConsolidatedConfiguration`, `_foldersConsolidatedConfigurations` and `overrideConfigurations`:
    /// the merged model per folder and language. A write to a layer field must call `layers_changed`.
    pub(crate) consolidated: RefCell<HashMap<FolderAndLanguage, ConfigurationModel>>,
}

type FolderAndLanguage = (Option<PathBuf>, Option<String>);

/// `IConfigurationOverrides`: a file the value is read for, and a language.
#[derive(Clone, Debug, Default)]
pub struct Overrides<'a> {
    pub resource: Option<&'a Path>,
    pub override_identifier: Option<&'a str>,
}

/// `IConfigurationValue`: what each layer holds for one key, as `inspect` reports it.
#[derive(Clone, Debug, Default)]
pub struct Inspected {
    pub value: Option<Value>,
    pub default_value: Option<Value>,
    pub policy_value: Option<Value>,
    pub user_local: Layer,
    pub workspace: Layer,
    pub workspace_folder: Layer,
}

/// One layer's value for a key, and its value inside the `[language]` block asked for.
#[derive(Clone, Debug, Default)]
pub struct Layer {
    pub value: Option<Value>,
    pub override_value: Option<Value>,
}

impl Configuration {
    /// `getValue(section, overrides)`.
    pub fn get_value(&self, section: Option<&str>, overrides: &Overrides) -> Option<Value> {
        let folder = self.folder_for(overrides.resource).map(Path::to_path_buf);
        let key = (folder, overrides.override_identifier.map(str::to_string));
        let mut consolidated = self.consolidated.borrow_mut();
        consolidated.entry(key).or_insert_with(|| self.consolidated(overrides)).get_value(section)
    }

    /// Drops the merged models after a layer changed.
    pub fn layers_changed(&mut self) {
        self.consolidated.get_mut().clear();
    }

    /// `inspect(key, overrides)`, the layers MainThreadConfiguration and `updateValue` decide a write target from.
    pub fn inspect(&self, key: &str, overrides: &Overrides) -> Inspected {
        let layer = |model: Option<&ConfigurationModel>| {
            let Some(model) = model else { return Layer::default() };
            let override_value = overrides.override_identifier.and_then(|identifier| model.get_override_value(Some(key), identifier));
            let value = match overrides.override_identifier {
                Some(identifier) => model.override_for(identifier).get_value(Some(key)),
                None => model.get_value(Some(key)),
            };
            Layer { value, override_value }
        };
        Inspected {
            value: self.get_value(Some(key), overrides),
            default_value: layer(Some(&self.defaults)).value,
            policy_value: self.policy.get_value(Some(key)),
            user_local: layer(Some(&self.local_user)),
            workspace: layer(self.has_workspace.then_some(&self.workspace)),
            workspace_folder: layer(self.folder_model_for(overrides.resource)),
        }
    }

    /// `getConsolidatedConfigurationModel`: defaults, then user, workspace and the folder the resource is in,
    /// then the `[language]` block, then the policy values.
    fn consolidated(&self, overrides: &Overrides) -> ConfigurationModel {
        let mut layers = vec![self.local_user.clone(), self.workspace.clone()];
        if let Some(folder) = self.folder_model_for(overrides.resource) {
            layers.push(folder.clone());
        }
        let merged = self.defaults.merge(&layers);
        let mut merged = match overrides.override_identifier {
            Some(identifier) => merged.override_for(identifier),
            None => merged,
        };
        for key in &self.policy.keys {
            merged.set_value(key, self.policy.get_value(Some(key)).expect("a model key has a value"));
        }
        merged
    }

    /// `getFolderConfigurationModelForResource`: the model of the folder closest to the resource.
    fn folder_model_for(&self, resource: Option<&Path>) -> Option<&ConfigurationModel> {
        let folder = self.folder_for(resource)?;
        self.folders.iter().find(|(path, _)| path == folder).map(|(_, model)| model)
    }

    fn folder_for(&self, resource: Option<&Path>) -> Option<&Path> {
        let resource = resource?;
        self.folders.iter().map(|(folder, _)| folder.as_path()).filter(|folder| resource.starts_with(folder)).max_by_key(|folder| folder.components().count())
    }

    /// `compareAndUpdatePolicyConfiguration`.
    pub fn compare_and_update_policy(&mut self, policy: ConfigurationModel) -> Change {
        let change = compare(Some(&self.policy), Some(&policy));
        if !change.keys.is_empty() {
            self.policy = policy;
        }
        change
    }

    /// `compareAndUpdateLocalUserConfiguration`.
    pub fn compare_and_update_local_user(&mut self, user: ConfigurationModel) -> Change {
        let change = compare(Some(&self.local_user), Some(&user));
        if !change.keys.is_empty() {
            self.local_user = user;
            self.layers_changed();
        }
        change
    }

    /// `compareAndUpdateFolderConfiguration`; in a folder window (`single_folder`) the folder model is also the
    /// workspace model, as `onWorkspaceFolderConfigurationChanged` does for WorkbenchState.FOLDER.
    pub fn compare_and_update_folder(&mut self, folder: &Path, folder_model: ConfigurationModel, single_folder: bool) -> Change {
        let Some((_, current)) = self.folders.iter_mut().find(|(path, _)| path == folder) else { return Change::default() };
        let folder_change = compare(Some(current), Some(&folder_model));
        if !folder_change.keys.is_empty() {
            *current = folder_model.clone();
            self.layers_changed();
        }
        if !single_folder {
            return folder_change;
        }
        merge_changes(&[folder_change, self.compare_and_update_workspace(folder_model)])
    }

    /// `compareAndUpdateWorkspaceConfiguration`.
    pub fn compare_and_update_workspace(&mut self, workspace: ConfigurationModel) -> Change {
        let change = compare(Some(&self.workspace), Some(&workspace));
        if !change.keys.is_empty() {
            self.workspace = workspace;
            self.layers_changed();
        }
        change
    }

    /// `toData()`: the `IConfigurationData` `$initializeConfiguration` and `$acceptConfigurationChanged` carry.
    pub fn to_data(&self) -> Value {
        let empty = ConfigurationModel::default().to_json();
        let folders: Vec<Value> = self.folders.iter().map(|(folder, model)| json!([file_uri(folder), model.to_json()])).collect();
        json!({
            "defaults": self.defaults.to_json(),
            "policy": self.policy.to_json(),
            "application": empty,
            "userLocal": self.local_user.to_json_with_raw(),
            "userRemote": empty,
            "workspace": self.workspace.to_json(),
            "folders": folders,
        })
    }
}

/// `UriComponents` of a `file:` URI, as the extension host revives it.
fn file_uri(path: &Path) -> Value {
    json!({ "$mid": 1, "scheme": "file", "authority": "", "path": path.to_string_lossy(), "query": "", "fragment": "" })
}
