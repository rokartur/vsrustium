//! The `Configuration` of platform/configuration/common/configurationModels.ts: the layers of one window and
//! the merged view `getValue` reads. The port has no policies, remote, profiles or in-memory values yet, so
//! those layers are empty and left out; `to_data` still sends them as upstream's `toData` does.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::model::{Change, ConfigurationModel, compare, merge_changes};

#[derive(Clone, Debug, Default)]
pub struct Configuration {
    pub defaults: ConfigurationModel,
    pub local_user: ConfigurationModel,
    pub workspace: ConfigurationModel,
    /// The one folder of a folder window and its `.vscode` settings.
    pub folder: Option<(PathBuf, ConfigurationModel)>,
}

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
        self.consolidated(overrides).get_value(section)
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
            user_local: layer(Some(&self.local_user)),
            workspace: layer(self.folder.is_some().then_some(&self.workspace)),
            workspace_folder: layer(self.folder_model_for(overrides.resource)),
        }
    }

    /// `getConsolidatedConfigurationModel`: defaults, then user, workspace and the folder the resource is in,
    /// then the `[language]` block.
    fn consolidated(&self, overrides: &Overrides) -> ConfigurationModel {
        let mut layers = vec![self.local_user.clone(), self.workspace.clone()];
        if let Some(folder) = self.folder_model_for(overrides.resource) {
            layers.push(folder.clone());
        }
        let merged = self.defaults.merge(&layers);
        match overrides.override_identifier {
            Some(identifier) => merged.override_for(identifier),
            None => merged,
        }
    }

    /// `getFolderConfigurationModelForResource`.
    fn folder_model_for(&self, resource: Option<&Path>) -> Option<&ConfigurationModel> {
        let (folder, model) = self.folder.as_ref()?;
        resource.filter(|resource| resource.starts_with(folder)).map(|_| model)
    }

    /// `compareAndUpdateLocalUserConfiguration`.
    pub fn compare_and_update_local_user(&mut self, user: ConfigurationModel) -> Change {
        let change = compare(Some(&self.local_user), Some(&user));
        if !change.keys.is_empty() {
            self.local_user = user;
        }
        change
    }

    /// `onWorkspaceFolderConfigurationChanged` of a folder window: the folder model is also the workspace model.
    pub fn compare_and_update_folder(&mut self, folder_model: ConfigurationModel) -> Change {
        let Some((_, current)) = &mut self.folder else { return Change::default() };
        let folder_change = compare(Some(current), Some(&folder_model));
        if !folder_change.keys.is_empty() {
            *current = folder_model.clone();
        }
        let workspace_change = compare(Some(&self.workspace), Some(&folder_model));
        if !workspace_change.keys.is_empty() {
            self.workspace = folder_model;
        }
        merge_changes(&[folder_change, workspace_change])
    }

    /// `toData()`: the `IConfigurationData` `$initializeConfiguration` and `$acceptConfigurationChanged` carry.
    pub fn to_data(&self) -> Value {
        let empty = ConfigurationModel::default().to_json();
        let folders: Vec<Value> = self.folder.iter().map(|(folder, model)| json!([file_uri(folder), model.to_json()])).collect();
        json!({
            "defaults": self.defaults.to_json(),
            "policy": empty,
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
