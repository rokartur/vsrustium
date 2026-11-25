//! extensionEnablementService.ts (workbench and platform): the user's enablement of each extension, kept as
//! identifier lists in storage: `extensionsIdentifiers/disabled` in the profile storage, `extensionsIdentifiers/disabled`
//! and `extensionsIdentifiers/enabled` in the workspace storage. A workspace entry wins over the profile one.

use serde_json::{Value, json};
use storage::Storage;

pub const DISABLED_EXTENSIONS_STORAGE_PATH: &str = "extensionsIdentifiers/disabled";
pub const ENABLED_EXTENSIONS_STORAGE_PATH: &str = "extensionsIdentifiers/enabled";

/// `EnablementState`, the states the port can reach.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EnablementState {
    DisabledByEnvironment,
    EnabledByEnvironment,
    DisabledByInvalidExtension,
    DisabledByExtensionDependency,
    DisabledGlobally,
    DisabledWorkspace,
    EnabledGlobally,
    EnabledWorkspace,
}

impl EnablementState {
    /// `isEnabledEnablementState`.
    pub fn is_enabled(self) -> bool {
        matches!(self, EnablementState::EnabledByEnvironment | EnablementState::EnabledWorkspace | EnablementState::EnabledGlobally)
    }
}

/// `IExtensionIdentifier`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Identifier {
    pub id: String,
    pub uuid: Option<String>,
}

impl Identifier {
    /// `areSameExtensions`: by uuid when both have one, else by id ignoring case.
    pub fn same(&self, other: &Identifier) -> bool {
        match (&self.uuid, &other.uuid) {
            (Some(a), Some(b)) => a == b,
            _ => self.id.eq_ignore_ascii_case(&other.id),
        }
    }
}

/// What the enablement of one extension depends on: its identifier, manifest and where it comes from.
pub struct EnablementInput<'a> {
    pub identifier: Identifier,
    pub manifest: &'a Value,
    /// `--disable-extension` names it, or `--disable-extensions` and it is not built in.
    pub disabled_in_env: bool,
    /// The extension under development (`--extensionDevelopmentPath`).
    pub enabled_in_env: bool,
    pub is_valid: bool,
}

/// The storage lists behind the enablement; `workspace` is `None` without a folder (`hasWorkspace`).
pub struct Enablement<'a> {
    pub profile: &'a mut Storage,
    pub workspace: Option<&'a mut Storage>,
}

impl Enablement<'_> {
    /// `_getUserEnablementState`.
    pub fn user_state(&self, identifier: &Identifier) -> EnablementState {
        if let Some(workspace) = &self.workspace {
            if identifiers(workspace, ENABLED_EXTENSIONS_STORAGE_PATH).iter().any(|e| e.same(identifier)) {
                return EnablementState::EnabledWorkspace;
            }
            if identifiers(workspace, DISABLED_EXTENSIONS_STORAGE_PATH).iter().any(|e| e.same(identifier)) {
                return EnablementState::DisabledWorkspace;
            }
        }
        if self.is_disabled_globally(identifier) {
            return EnablementState::DisabledGlobally;
        }
        EnablementState::EnabledGlobally
    }

    pub fn is_disabled_globally(&self, identifier: &Identifier) -> bool {
        identifiers(self.profile, DISABLED_EXTENSIONS_STORAGE_PATH).iter().any(|e| e.same(identifier))
    }

    /// `_computeEnablementState` for `extensions[index]`, without malicious, allowlist, virtual workspace, trust,
    /// extension kind and sessions window states the port has no source for.
    pub fn state(&self, extensions: &[EnablementInput], index: usize) -> EnablementState {
        self.compute(extensions, index, &mut Vec::new())
    }

    fn compute(&self, extensions: &[EnablementInput], index: usize, visiting: &mut Vec<usize>) -> EnablementState {
        let extension = &extensions[index];
        let user_state = self.user_state(&extension.identifier);
        let enabled = user_state.is_enabled();
        if enabled && !extension.is_valid {
            return EnablementState::DisabledByInvalidExtension;
        }
        if extension.disabled_in_env {
            return EnablementState::DisabledByEnvironment;
        }
        if enabled && self.disabled_by_dependency(extensions, index, visiting) {
            return EnablementState::DisabledByExtensionDependency;
        }
        if !enabled && extension.enabled_in_env {
            return EnablementState::EnabledByEnvironment;
        }
        user_state
    }

    /// `_isDisabledByExtensionDependency`: a dependency that is installed and not enabled; a cycle counts as enabled.
    fn disabled_by_dependency(&self, extensions: &[EnablementInput], index: usize, visiting: &mut Vec<usize>) -> bool {
        let Some(dependencies) = extensions[index].manifest["extensionDependencies"].as_array() else { return false };
        visiting.push(index);
        let disabled = extensions.iter().enumerate().any(|(other, extension)| {
            let is_dependency = dependencies.iter().filter_map(Value::as_str).any(|id| extension.identifier.id.eq_ignore_ascii_case(id));
            is_dependency && !visiting.contains(&other) && !self.compute(extensions, other, visiting).is_enabled()
        });
        visiting.pop();
        disabled
    }

    /// `throwErrorIfCannotChangeEnablement`: the reason `extensions[index]` cannot be enabled or disabled.
    pub fn cannot_change(&self, extensions: &[EnablementInput], index: usize) -> Option<String> {
        let extension = &extensions[index];
        let name = extension.manifest["displayName"].as_str().unwrap_or(&extension.identifier.id).to_owned();
        if extension.manifest["contributes"]["localizations"].as_array().is_some_and(|localizations| !localizations.is_empty()) {
            return Some(format!("Cannot change enablement of {name} extension because it contributes language packs."));
        }
        if extension.enabled_in_env {
            return Some(format!("Cannot change enablement of {name} extension because it is enabled in environment"));
        }
        match self.state(extensions, index) {
            EnablementState::DisabledByEnvironment => Some(format!("Cannot change enablement of {name} extension because it is disabled in environment")),
            EnablementState::DisabledByInvalidExtension => Some(format!("Cannot change enablement of {name} extension because of it is invalid")),
            // Upstream throws for the first dependency that is not enabled.
            EnablementState::DisabledByExtensionDependency => {
                let dependencies = extension.manifest["extensionDependencies"].as_array().cloned().unwrap_or_default();
                let (_, dependency) = extensions.iter().enumerate().find(|(other, dependency)| {
                    dependencies.iter().filter_map(Value::as_str).any(|id| dependency.identifier.id.eq_ignore_ascii_case(id)) && !self.state(extensions, *other).is_enabled()
                })?;
                let dependency_name = dependency.manifest["displayName"].as_str().unwrap_or(&dependency.identifier.id);
                Some(format!("Cannot enable '{name}' extension because it depends on '{dependency_name}' extension that cannot be enabled"))
            }
            _ => None,
        }
    }

    /// `setEnablement` for one extension and the disabled dependencies and pack members an enable takes along
    /// (`getExtensionsToEnableRecursively`). Answers the indexes whose user state changed.
    pub fn set_enablement(&mut self, extensions: &[EnablementInput], index: usize, new_state: EnablementState) -> Result<Vec<usize>, String> {
        let workspace = matches!(new_state, EnablementState::DisabledWorkspace | EnablementState::EnabledWorkspace);
        let mut targets = vec![index];
        if new_state.is_enabled() {
            self.add_to_enable_recursively(extensions, &mut targets);
        }
        for &target in &targets {
            if workspace && self.workspace.is_none() {
                return Err("No workspace.".into());
            }
            if let Some(error) = self.cannot_change(extensions, target) {
                return Err(error);
            }
        }
        Ok(targets.into_iter().filter(|&target| self.set_user_state(&extensions[target].identifier, new_state)).collect())
    }

    fn add_to_enable_recursively(&self, extensions: &[EnablementInput], targets: &mut Vec<usize>) {
        let mut next = 0;
        while next < targets.len() {
            let manifest = extensions[targets[next]].manifest;
            next += 1;
            let wanted: Vec<&str> = ["extensionDependencies", "extensionPack"].iter().flat_map(|key| manifest[*key].as_array().into_iter().flatten()).filter_map(Value::as_str).collect();
            for (other, extension) in extensions.iter().enumerate() {
                if targets.contains(&other) || !wanted.iter().any(|id| extension.identifier.id.eq_ignore_ascii_case(id)) {
                    continue;
                }
                if !self.state(extensions, other).is_enabled() {
                    targets.push(other);
                }
            }
        }
    }

    /// `_setUserEnablementState`: answers whether the stored state changed.
    pub fn set_user_state(&mut self, identifier: &Identifier, new_state: EnablementState) -> bool {
        if self.user_state(identifier) == new_state {
            return false;
        }
        match new_state {
            EnablementState::EnabledGlobally => {
                self.remove_from_workspace(DISABLED_EXTENSIONS_STORAGE_PATH, identifier);
                self.remove_from_workspace(ENABLED_EXTENSIONS_STORAGE_PATH, identifier);
                remove(self.profile, DISABLED_EXTENSIONS_STORAGE_PATH, identifier);
            }
            EnablementState::DisabledGlobally => {
                self.remove_from_workspace(DISABLED_EXTENSIONS_STORAGE_PATH, identifier);
                self.remove_from_workspace(ENABLED_EXTENSIONS_STORAGE_PATH, identifier);
                add(self.profile, DISABLED_EXTENSIONS_STORAGE_PATH, identifier);
            }
            EnablementState::EnabledWorkspace => {
                self.remove_from_workspace(DISABLED_EXTENSIONS_STORAGE_PATH, identifier);
                if let Some(workspace) = self.workspace.as_deref_mut() {
                    add(workspace, ENABLED_EXTENSIONS_STORAGE_PATH, identifier);
                }
            }
            EnablementState::DisabledWorkspace => {
                if let Some(workspace) = self.workspace.as_deref_mut() {
                    add(workspace, DISABLED_EXTENSIONS_STORAGE_PATH, identifier);
                }
                self.remove_from_workspace(ENABLED_EXTENSIONS_STORAGE_PATH, identifier);
            }
            _ => return false,
        }
        true
    }

    /// `_reset` after an uninstall: the extension leaves every list.
    pub fn reset(&mut self, identifier: &Identifier) {
        self.remove_from_workspace(DISABLED_EXTENSIONS_STORAGE_PATH, identifier);
        self.remove_from_workspace(ENABLED_EXTENSIONS_STORAGE_PATH, identifier);
        remove(self.profile, DISABLED_EXTENSIONS_STORAGE_PATH, identifier);
    }

    fn remove_from_workspace(&mut self, key: &str, identifier: &Identifier) {
        if let Some(workspace) = self.workspace.as_deref_mut() {
            remove(workspace, key, identifier);
        }
    }
}

/// `StorageManager.get`: the stored JSON list, `[]` when missing.
pub fn identifiers(storage: &Storage, key: &str) -> Vec<Identifier> {
    let value: Value = storage.get(key).and_then(|text| serde_json::from_str(text).ok()).unwrap_or(json!([]));
    let entries = value.as_array().cloned().unwrap_or_default();
    entries
        .iter()
        .filter_map(|entry| Some(Identifier { id: entry["id"].as_str()?.to_owned(), uuid: entry["uuid"].as_str().map(str::to_owned) }))
        .collect()
}

/// `StorageManager.set`: `{id, uuid}` entries, the key removed once the list is empty.
fn write(storage: &mut Storage, key: &str, list: &[Identifier]) {
    if list.is_empty() {
        return storage.delete(key);
    }
    let entries: Vec<Value> = list
        .iter()
        .map(|identifier| match &identifier.uuid {
            Some(uuid) => json!({ "id": identifier.id, "uuid": uuid }),
            None => json!({ "id": identifier.id }),
        })
        .collect();
    storage.set(key, Value::Array(entries).to_string());
}

fn add(storage: &mut Storage, key: &str, identifier: &Identifier) {
    let mut list = identifiers(storage, key);
    if list.iter().all(|entry| !entry.same(identifier)) {
        list.push(identifier.clone());
        write(storage, key, &list);
    }
}

fn remove(storage: &mut Storage, key: &str, identifier: &Identifier) {
    let mut list = identifiers(storage, key);
    if let Some(index) = list.iter().position(|entry| entry.same(identifier)) {
        list.remove(index);
        write(storage, key, &list);
    }
}

#[cfg(test)]
mod tests;
