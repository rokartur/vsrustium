//! MainThreadConfiguration (workbench/api/browser/mainThreadConfiguration.ts): the window's settings as the
//! extension host sees them, and the writes `WorkspaceConfiguration.update` makes.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use config::configuration::Overrides;
use config::model::Change;
use config::service::{Settings, Target};
use serde_json::{Value, json};

use crate::rpc::{Arg, Message, Reply};

pub struct MainThreadConfiguration {
    pub settings: Arc<Mutex<Settings>>,
    /// `configurationScopes` go to the host only in a development run, as upstream's `!isBuilt || isExtensionDevelopment`.
    pub send_scopes: bool,
}

impl MainThreadConfiguration {
    pub fn new(settings: Arc<Mutex<Settings>>, send_scopes: bool) -> Self {
        MainThreadConfiguration { settings, send_scopes }
    }

    /// `_getConfigurationData`: the argument of `$initializeConfiguration` and `$acceptConfigurationChanged`.
    pub fn configuration_data(&self) -> Value {
        configuration_data(&self.settings.lock().unwrap(), self.send_scopes)
    }

    /// Request `req` from the host: the `$acceptConfigurationChanged` arguments to send first if a setting
    /// changed, and the reply. A failed write rejects with the `ConfigurationEditingError` message.
    pub fn serve(&mut self, req: u32, method: &str, args: &[Arg]) -> (Option<Vec<Value>>, Message) {
        match self.handle(method, args) {
            Ok(change) => (change, Message::ReplyOk { req, value: Reply::Empty }),
            Err(message) => {
                let error = json!({ "$isError": true, "name": "CodeExpectedError", "message": message, "stack": "", "noTelemetry": true });
                (None, Message::ReplyErr { req, error: Some(error) })
            }
        }
    }

    /// `$updateConfigurationOption(target, key, value, overrides, scopeToLanguage)` and
    /// `$removeConfigurationOption(target, key, overrides, scopeToLanguage)`.
    fn handle(&mut self, method: &str, args: &[Arg]) -> Result<Option<Vec<Value>>, String> {
        let arg = |index: usize| match args.get(index) {
            Some(Arg::Json(value)) => Some(value.clone()),
            _ => None,
        };
        let (value, overrides, scope_to_language) = match method {
            "$updateConfigurationOption" => (arg(2), arg(3), arg(4)),
            _ => (None, arg(2), arg(3)),
        };
        let target = arg(0).and_then(|target| target.as_u64()).and_then(Target::from_number);
        let key = arg(1).and_then(|key| key.as_str().map(str::to_owned)).unwrap_or_default();
        let overrides = overrides.unwrap_or(Value::Null);
        let resource = file_path(&overrides["resource"]);
        let override_identifier = overrides["overrideIdentifier"].as_str().map(str::to_owned);
        let change = self.write_configuration(target, &key, value.as_ref(), resource, override_identifier, scope_to_language.and_then(|scope| scope.as_bool()))?;
        Ok((!change.keys.is_empty()).then(|| self.change_args(&change)))
    }

    /// `writeConfiguration` and `_updateValue`: a missing target is the workspace (`deriveConfigurationTarget` of
    /// a window with at most one folder), and the language block is written only when asked to or when the
    /// target already sets the key inside it.
    fn write_configuration(
        &mut self,
        target: Option<Target>,
        key: &str,
        value: Option<&Value>,
        resource: Option<PathBuf>,
        override_identifier: Option<String>,
        scope_to_language: Option<bool>,
    ) -> Result<Change, String> {
        let target = target.unwrap_or(Target::Workspace);
        let mut settings = self.settings.lock().unwrap();
        let inspected = settings.configuration.inspect(key, &Overrides { resource: resource.as_deref(), override_identifier: override_identifier.as_deref() });
        let overridden = match target {
            Target::WorkspaceFolder => inspected.workspace_folder.override_value,
            Target::Workspace => inspected.workspace.override_value,
            // Memory and remote user values are not ported; their override is always unset.
            Target::Memory | Target::UserRemote => None,
            _ => inspected.user_local.override_value,
        };
        let identifiers: Vec<String> = match (scope_to_language, override_identifier) {
            (Some(true), Some(identifier)) => vec![identifier],
            (None, Some(identifier)) if overridden.is_some() => vec![identifier],
            _ => Vec::new(),
        };
        settings.update_value(key, value, target, resource.as_deref(), &identifiers)
    }

    fn change_args(&self, change: &Change) -> Vec<Value> {
        changed_args(&self.settings.lock().unwrap(), self.send_scopes, change)
    }
}

fn configuration_data(settings: &Settings, send_scopes: bool) -> Value {
    let mut data = settings.configuration.to_data();
    data["configurationScopes"] = if send_scopes { settings.registry.scopes() } else { json!([]) };
    data
}

/// The `$acceptConfigurationChanged` arguments for a change the workbench wrote.
pub fn changed_args(settings: &Settings, send_scopes: bool, change: &Change) -> Vec<Value> {
    vec![configuration_data(settings, send_scopes), change.to_json()]
}

/// The file path of a `file:` `UriComponents`; other schemes are in no folder.
fn file_path(uri: &Value) -> Option<PathBuf> {
    (uri["scheme"] == "file").then(|| uri["path"].as_str().map(PathBuf::from)).flatten()
}
