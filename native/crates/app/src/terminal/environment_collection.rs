//! environmentVariableService.ts and environmentVariableCollection.ts: the extensions' environment variable
//! collections (`ExtensionContext.environmentVariableCollection`), merged and applied to a new terminal's
//! environment; persistent ones are kept in workspace storage.

use std::collections::HashMap;

use serde_json::{Value, json};

/// `TerminalStorageKeys.EnvironmentVariableCollections`.
pub const STORAGE_KEY: &str = "terminal.integrated.environmentVariableCollectionsV2";

/// `EnvironmentVariableMutatorType`.
const REPLACE: u64 = 1;
const APPEND: u64 = 2;
const PREPEND: u64 = 3;

/// Only this extension may set the Python activation variables (`blockPythonActivationVar`).
const PYTHON_ENV_EXTENSION_ID: &str = "ms-python.vscode-python-envs";

struct Collection {
    persistent: bool,
    /// `ISerializableEnvironmentVariableCollection`: `[key, IEnvironmentVariableMutator]` pairs.
    entries: Vec<(String, Value)>,
    description: Value,
}

/// The collections by extension id, in the order they were first set (a JS `Map`).
#[derive(Default)]
pub struct EnvironmentCollections {
    collections: Vec<(String, Collection)>,
}

impl EnvironmentCollections {
    /// The persisted collections, as the service's constructor reads them.
    pub fn load(stored: Option<&str>) -> Self {
        let mut collections = EnvironmentCollections::default();
        let parsed: Value = stored.and_then(|stored| serde_json::from_str(stored).ok()).unwrap_or(Value::Null);
        for item in parsed.as_array().into_iter().flatten() {
            let Some(id) = item["extensionIdentifier"].as_str() else { continue };
            let entries = pairs(&item["collection"]);
            collections.collections.push((id.to_owned(), Collection { persistent: true, entries, description: item["description"].clone() }));
        }
        collections
    }

    /// `$setEnvironmentVariableCollection`: `None` deletes the extension's collection.
    pub fn set(&mut self, extension_id: &str, persistent: bool, collection: Option<&Value>, description: &Value) {
        let Some(collection) = collection else {
            self.collections.retain(|(id, _)| id != extension_id);
            return;
        };
        let collection = Collection { persistent, entries: pairs(collection), description: description.clone() };
        match self.collections.iter_mut().find(|(id, _)| id == extension_id) {
            Some((_, existing)) => *existing = collection,
            None => self.collections.push((extension_id.to_owned(), collection)),
        }
    }

    /// `_persistCollections`: the persistent collections as workspace storage keeps them.
    pub fn serialize_persistent(&self) -> String {
        let items: Vec<Value> = self
            .collections
            .iter()
            .filter(|(_, collection)| collection.persistent)
            .map(|(id, collection)| json!({ "extensionIdentifier": id, "collection": serialize_pairs(&collection.entries), "description": collection.description }))
            .collect();
        Value::Array(items).to_string()
    }

    /// `$initEnvironmentVariableCollections`'s argument.
    pub fn serialized(&self) -> Value {
        Value::Array(self.collections.iter().map(|(id, collection)| json!([id, serialize_pairs(&collection.entries)])).collect())
    }

    pub fn is_empty(&self) -> bool {
        self.collections.is_empty()
    }

    /// `MergedEnvironmentVariableCollection.applyToProcessEnvironment` for a terminal in the workspace folder
    /// with this index.
    pub fn apply(&self, env: &mut HashMap<String, String>, workspace_folder: Option<u64>) {
        for (variable, mutators) in self.variable_map(workspace_folder) {
            for mutator in mutators {
                let value = mutator["value"].as_str().unwrap_or_default();
                let kind = mutator["type"].as_u64();
                // Default: true
                if mutator["options"]["applyAtProcessCreation"].as_bool() != Some(false) {
                    let current = env.get(&variable).cloned().unwrap_or_default();
                    let next = match kind {
                        Some(APPEND) => Some(current + value),
                        Some(PREPEND) => Some(format!("{value}{current}")),
                        Some(REPLACE) => Some(value.to_owned()),
                        _ => None,
                    };
                    if let Some(next) = next {
                        env.insert(variable.clone(), next);
                    }
                }
                // Default: false
                if mutator["options"]["applyAtShellIntegration"].as_bool() == Some(true) {
                    let label = match kind {
                        Some(APPEND) => "APPEND",
                        Some(PREPEND) => "PREPEND",
                        _ => "REPLACE",
                    };
                    let key = format!("VSCODE_ENV_{label}");
                    let entry = format!("{variable}={}", value.replace(':', "\\x3a"));
                    let next = match env.get(&key) {
                        Some(existing) => format!("{existing}:{entry}"),
                        None => entry,
                    };
                    env.insert(key, next);
                }
            }
        }
    }

    /// The constructor's merge then `getVariableMap(scope)`: mutators per key in reverse order of creation, a
    /// replace first dropping the rest; the result keyed by variable name.
    fn variable_map(&self, workspace_folder: Option<u64>) -> Vec<(String, Vec<Value>)> {
        let mut merged: Vec<(String, Vec<Value>)> = Vec::new();
        for (extension_id, collection) in &self.collections {
            for (key, mutator) in &collection.entries {
                let variable = mutator["variable"].as_str().unwrap_or(key);
                if is_python_activation_variable(variable) && extension_id != PYTHON_ENV_EXTENSION_ID {
                    continue;
                }
                let index = match merged.iter().position(|(known, _)| known == key) {
                    Some(index) => index,
                    None => {
                        merged.push((key.clone(), Vec::new()));
                        merged.len() - 1
                    }
                };
                let entry = &mut merged[index].1;
                if entry.first().is_some_and(|first| first["type"].as_u64() == Some(REPLACE)) {
                    continue;
                }
                entry.insert(0, mutator.clone());
            }
        }
        let mut result: Vec<(String, Vec<Value>)> = Vec::new();
        for (_, mutators) in merged {
            let filtered: Vec<Value> = mutators.into_iter().filter(|mutator| in_scope(mutator, workspace_folder)).collect();
            let Some(first) = filtered.first() else { continue };
            let variable = first["variable"].as_str().unwrap_or_default().to_owned();
            match result.iter_mut().find(|(known, _)| *known == variable) {
                Some((_, existing)) => *existing = filtered,
                None => result.push((variable, filtered)),
            }
        }
        result
    }
}

/// `filterScope`: an unscoped mutator applies everywhere, a scoped one only in its workspace folder.
fn in_scope(mutator: &Value, workspace_folder: Option<u64>) -> bool {
    let scope = &mutator["scope"];
    if scope.is_null() {
        return true;
    }
    let index = scope["workspaceFolder"]["index"].as_u64();
    index.is_some() && index == workspace_folder
}

/// `PYTHON_ACTIVATION_VARS_PATTERN`.
fn is_python_activation_variable(variable: &str) -> bool {
    let Some(rest) = variable.strip_prefix("VSCODE_PYTHON_") else { return false };
    ["PWSH", "ZSH", "BASH", "FISH"].iter().any(|shell| rest.strip_prefix(shell).is_some_and(|rest| rest.starts_with("_ACTIVATE")))
}

fn pairs(value: &Value) -> Vec<(String, Value)> {
    let mut pairs = Vec::new();
    for pair in value.as_array().into_iter().flatten() {
        if let (Some(key), Some(mutator)) = (pair[0].as_str(), pair.get(1)) {
            pairs.push((key.to_owned(), mutator.clone()));
        }
    }
    pairs
}

fn serialize_pairs(entries: &[(String, Value)]) -> Value {
    Value::Array(entries.iter().map(|(key, mutator)| json!([key, mutator])).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mutator(variable: &str, value: &str, kind: u64) -> Value {
        json!([variable, { "variable": variable, "value": value, "type": kind }])
    }

    /// environmentVariableCollection.test.ts 'applyToProcessEnvironment' and the api-tests expectations.
    #[test]
    fn mutators_replace_append_and_prepend() {
        let mut collections = EnvironmentCollections::default();
        collections.set("ext", true, Some(&json!([mutator("A", "~a2~", REPLACE), mutator("B", "~b2~", APPEND), mutator("C", "~c2~", PREPEND), mutator("D", "d", APPEND)])), &Value::Null);
        let mut env = HashMap::from([("A".to_owned(), "a1".to_owned()), ("B".into(), "b1".into()), ("C".into(), "c1".into())]);
        collections.apply(&mut env, None);
        assert_eq!(env["A"], "~a2~");
        assert_eq!(env["B"], "b1~b2~");
        assert_eq!(env["C"], "~c2~c1");
        assert_eq!(env["D"], "d");
    }

    #[test]
    fn later_extensions_apply_first_and_a_replace_shadows_them() {
        let mut collections = EnvironmentCollections::default();
        collections.set("ext1", true, Some(&json!([mutator("A", "a1", APPEND)])), &Value::Null);
        collections.set("ext2", true, Some(&json!([mutator("A", "a2", APPEND)])), &Value::Null);
        let mut env = HashMap::new();
        collections.apply(&mut env, None);
        assert_eq!(env["A"], "a2a1");

        let mut collections = EnvironmentCollections::default();
        collections.set("ext1", true, Some(&json!([mutator("A", "r", REPLACE)])), &Value::Null);
        collections.set("ext2", true, Some(&json!([mutator("A", "x", APPEND)])), &Value::Null);
        let mut env = HashMap::new();
        collections.apply(&mut env, None);
        assert_eq!(env["A"], "r");
    }

    #[test]
    fn scoped_mutators_apply_in_their_folder_only_and_persistence_round_trips() {
        let scoped = json!([["A-folder", { "variable": "A", "value": "s", "type": REPLACE, "scope": { "workspaceFolder": { "index": 1 } } }]]);
        let mut collections = EnvironmentCollections::default();
        collections.set("ext", true, Some(&scoped), &Value::Null);
        collections.set("transient", false, Some(&json!([mutator("B", "b", REPLACE)])), &Value::Null);
        let mut env = HashMap::new();
        collections.apply(&mut env, Some(0));
        assert!(!env.contains_key("A"));
        collections.apply(&mut env, Some(1));
        assert_eq!(env["A"], "s");

        let restored = EnvironmentCollections::load(Some(&collections.serialize_persistent()));
        assert_eq!(restored.serialized(), json!([["ext", scoped]]));
        collections.set("ext", true, None, &Value::Null);
        assert_eq!(collections.serialized(), json!([["transient", [mutator("B", "b", REPLACE)]]]));
    }
}
