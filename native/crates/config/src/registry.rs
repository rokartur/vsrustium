//! platform/configuration/common/configurationRegistry.ts and the `configuration` and `configurationDefaults`
//! extension points of workbench/api/common/configurationExtensionPoint.ts: every registered setting with its
//! schema, scope and default, and the defaults model built from them.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::model::{ConfigurationModel, is_override_key};

/// `ConfigurationScope`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    Application = 1,
    Machine = 2,
    ApplicationMachine = 3,
    Window = 4,
    Resource = 5,
    LanguageOverridable = 6,
    MachineOverridable = 7,
}

impl Scope {
    fn from_number(value: &Value) -> Option<Scope> {
        Some(match value.as_u64()? {
            1 => Scope::Application,
            2 => Scope::Machine,
            3 => Scope::ApplicationMachine,
            4 => Scope::Window,
            5 => Scope::Resource,
            6 => Scope::LanguageOverridable,
            7 => Scope::MachineOverridable,
            _ => return None,
        })
    }

    /// `parseScope`: what an extension's package.json may say; anything else, `application-machine` included, is WINDOW.
    fn parse(scope: &str) -> Scope {
        match scope {
            "application" => Scope::Application,
            "machine" => Scope::Machine,
            "resource" => Scope::Resource,
            "machine-overridable" => Scope::MachineOverridable,
            "language-overridable" => Scope::LanguageOverridable,
            _ => Scope::Window,
        }
    }
}

/// `APPLICATION_SCOPES`, `WORKSPACE_SCOPES` and `FOLDER_SCOPES` of workbench/services/configuration/common/configuration.ts.
pub const APPLICATION_SCOPES: [Scope; 2] = [Scope::Application, Scope::ApplicationMachine];
pub const WORKSPACE_SCOPES: [Scope; 4] = [Scope::Window, Scope::Resource, Scope::LanguageOverridable, Scope::MachineOverridable];
pub const FOLDER_SCOPES: [Scope; 3] = [Scope::Resource, Scope::LanguageOverridable, Scope::MachineOverridable];

/// `IRegisteredConfigurationPropertySchema`: the schema as registered, plus what the registry derives from it.
#[derive(Clone, Debug)]
pub struct Property {
    pub schema: Value,
    /// WINDOW for `[lang]` properties, whose scope upstream leaves unset.
    pub scope: Scope,
    pub restricted: bool,
    pub default: Value,
    /// `defaultDefaultValue`: the default before any `configurationDefaults` contribution.
    default_default: Option<Value>,
    /// The contributing extension id; `None` for core settings.
    pub source: Option<String>,
}

#[derive(Default)]
pub struct Registry {
    keys: Vec<String>,
    properties: HashMap<String, Property>,
    /// `configurationDefaultsOverrides`: the merged value each key's `configurationDefaults` contributions give it.
    default_overrides: Vec<(String, Value, Option<String>)>,
    /// `registerOverrideIdentifiers`: the language ids languagesRegistry.ts registers.
    pub override_identifiers: Vec<String>,
}

/// The core settings of VS Code 1.147.0 as its registry holds them, dumped with port/tools/dump-settings.
const CORE_SETTINGS: &str = include_str!("../core-settings.json");

impl Registry {
    /// The core settings and the language defaults of editor/common/languages/modesRegistry.ts.
    pub fn core() -> Registry {
        let mut registry = Registry::default();
        let Value::Object(settings) = serde_json::from_str(CORE_SETTINGS).expect("core-settings.json is JSON") else {
            unreachable!("core-settings.json holds an object")
        };
        for (key, schema) in settings {
            let scope = Scope::from_number(&schema["scope"]).unwrap_or(Scope::Window);
            let restricted = schema["restricted"].as_bool().unwrap_or(false);
            let default = schema.get("default").cloned();
            registry.insert(key, Property { scope, restricted, default: Value::Null, default_default: default, source: None, schema });
        }
        registry.register_defaults(
            &json!({
                "[plaintext]": { "editor.unicodeHighlight.ambiguousCharacters": false, "editor.unicodeHighlight.invisibleCharacters": false },
                "[go]": { "editor.insertSpaces": false },
                "[makefile]": { "editor.insertSpaces": false },
                "[shellscript]": { "files.eol": "\n" },
                "[yaml]": { "editor.insertSpaces": true, "editor.tabSize": 2 },
            }),
            None,
        );
        registry
    }

    /// The core settings plus what each extension (id, package.json) contributes, defaults resolved.
    pub fn with_extensions(extensions: &[(&str, &Value)]) -> Registry {
        let mut registry = Registry::core();
        for (id, description) in extensions {
            registry.register_configuration(id, description);
        }
        for (id, description) in extensions {
            registry.register_configuration_defaults(id, description);
        }
        // The core plaintext language (modesRegistry.ts), then each `contributes.languages` id.
        registry.override_identifiers.push("plaintext".into());
        for (_, description) in extensions {
            for language in description["contributes"]["languages"].as_array().into_iter().flatten() {
                if let Some(id) = language["id"].as_str().filter(|id| !registry.override_identifiers.iter().any(|known| known == id)) {
                    registry.override_identifiers.push(id.into());
                }
            }
        }
        registry.finish();
        registry
    }

    /// The `configuration` contribution of one extension's package.json (`configurationExtPoint` handler).
    pub fn register_configuration(&mut self, extension_id: &str, description: &Value) {
        let display_name = description["displayName"].as_str();
        let restricted: Vec<&str> = match &description["capabilities"]["untrustedWorkspaces"] {
            limited if limited["supported"] == "limited" => limited["restrictedConfigurations"].as_array().into_iter().flatten().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        let nodes = match &description["contributes"]["configuration"] {
            Value::Array(nodes) => nodes.iter().collect(),
            node @ Value::Object(_) => vec![node],
            _ => Vec::new(),
        };
        for node in nodes {
            let section = json!({
                "id": node["id"].as_str().unwrap_or(extension_id),
                "title": node["title"].as_str().or(display_name).unwrap_or(extension_id),
                "order": node["order"],
                "extensionInfo": { "id": extension_id, "displayName": display_name },
            });
            for (key, schema) in node["properties"].as_object().into_iter().flatten() {
                // validateProperty, and the extension point's object check.
                if key.trim().is_empty() || is_override_key(key) || self.properties.contains_key(key) || !schema.is_object() {
                    continue;
                }
                // `included: false` goes to the excluded properties, which only policies and experiments read.
                if schema["included"] == false {
                    continue;
                }
                let mut schema = schema.clone();
                schema["section"] = section.clone();
                let scope = schema["scope"].as_str().map_or(Scope::Window, Scope::parse);
                let restricted = schema["restricted"].as_bool().unwrap_or_else(|| restricted.contains(&key.as_str()));
                let default = schema.get("default").cloned();
                self.insert(key.clone(), Property { scope, restricted, default: Value::Null, default_default: default, source: Some(extension_id.to_owned()), schema });
            }
        }
    }

    /// The `configurationDefaults` contribution of one extension (`defaultConfigurationExtPoint` handler).
    pub fn register_configuration_defaults(&mut self, extension_id: &str, description: &Value) {
        let Some(overrides) = description["contributes"]["configurationDefaults"].as_object() else { return };
        const ALLOWED: [Scope; 4] = [Scope::MachineOverridable, Scope::Window, Scope::Resource, Scope::LanguageOverridable];
        let mut allowed = Map::new();
        for (key, value) in overrides {
            let property = self.properties.get(key);
            if property.is_some_and(|p| p.schema["disallowConfigurationDefault"] == true) {
                continue;
            }
            if !is_override_key(key) && property.is_some_and(|p| !ALLOWED.contains(&p.scope)) {
                continue;
            }
            allowed.insert(key.clone(), value.clone());
        }
        self.register_defaults(&Value::Object(allowed), Some(extension_id));
    }

    /// `doRegisterDefaultConfigurations`: `[lang]` blocks merge key by key, object settings merge their keys, the rest replace.
    fn register_defaults(&mut self, overrides: &Value, source: Option<&str>) {
        for (key, value) in overrides.as_object().into_iter().flatten() {
            let existing = self.default_overrides.iter().position(|(k, _, _)| k == key);
            let previous = existing.map(|index| self.default_overrides[index].1.clone());
            let merged = if is_override_key(key) {
                let mut merged = previous.and_then(|p| p.as_object().cloned()).unwrap_or_default();
                for (property, property_value) in value.as_object().into_iter().flatten() {
                    if let (Some(Value::Object(base)), Value::Object(add)) = (merged.get_mut(property), property_value) {
                        base.extend(add.clone());
                        continue;
                    }
                    merged.insert(property.clone(), property_value.clone());
                }
                Value::Object(merged)
            } else {
                let property = self.properties.get(key);
                let existing_default = previous.or_else(|| property.and_then(|p| p.default_default.clone()));
                let is_object_setting = value.is_object()
                    && match property {
                        Some(property) => property.schema["type"] == "object",
                        None => existing_default.as_ref().is_none_or(Value::is_object),
                    };
                match (is_object_setting, existing_default) {
                    (true, Some(Value::Object(mut base))) => {
                        base.extend(value.as_object().cloned().unwrap_or_default());
                        Value::Object(base)
                    }
                    _ => value.clone(),
                }
            };
            let entry = (key.clone(), merged, source.map(str::to_owned));
            match existing {
                Some(index) => self.default_overrides[index] = entry,
                None => self.default_overrides.push(entry),
            }
        }
    }

    fn insert(&mut self, key: String, property: Property) {
        if !self.properties.contains_key(&key) {
            self.keys.push(key.clone());
        }
        self.properties.insert(key, property);
    }

    /// Resolves every default (`updatePropertyDefaultValue`) and adds a `[lang]` property per language default
    /// (`updateDefaultOverrideProperty`). Call once all contributions are in.
    pub fn finish(&mut self) {
        for (key, value, source) in self.default_overrides.clone() {
            if is_override_key(&key) {
                // `getLanguageTagSettingPlainKey`: `[a][b]` reads `a, b`.
                let plain_key = key.strip_prefix('[').and_then(|key| key.strip_suffix(']')).expect("an override key").replace("][", ", ");
                let schema = json!({
                    "type": "object",
                    "default": value,
                    "description": format!("Configure settings to be overridden for {plain_key}."),
                    "$ref": "vscode://schemas/settings/resourceLanguage",
                    "section": { "id": "defaultOverrides", "title": "Default Language Configuration Overrides" },
                });
                self.insert(key, Property { schema, scope: Scope::Window, restricted: false, default: Value::Null, default_default: Some(value), source });
            }
        }
        for property in self.properties.values_mut() {
            property.default = property.default_default.clone().unwrap_or_else(|| default_for_type(&property.schema["type"]));
        }
        for (key, value, source) in &self.default_overrides {
            let Some(property) = self.properties.get_mut(key) else { continue };
            if is_override_key(key) || property.schema["disallowConfigurationDefault"] != true || source.is_none() {
                property.default = value.clone();
            }
        }
    }

    pub fn property(&self, key: &str) -> Option<&Property> {
        self.properties.get(key)
    }

    /// Registered keys in registration order.
    pub fn keys(&self) -> &[String] {
        &self.keys
    }

    /// `DefaultConfiguration.configurationModel`: every property's default, `[lang]` defaults as overrides.
    pub fn default_model(&self) -> ConfigurationModel {
        let mut model = ConfigurationModel::default();
        for key in &self.keys {
            model.set_value(key, self.properties[key].default.clone());
        }
        model
    }

    /// `getScopes()`: what `$initializeConfiguration` sends as `configurationScopes` in a development build.
    pub fn scopes(&self) -> Value {
        let mut scopes: Vec<Value> = self
            .keys
            .iter()
            .map(|key| match is_override_key(key) {
                true => json!([key, null]),
                false => json!([key, self.properties[key].scope as u8]),
            })
            .collect();
        scopes.push(json!(["launch", Scope::Resource as u8]));
        scopes.push(json!(["task", Scope::Resource as u8]));
        Value::Array(scopes)
    }
}

/// `getDefaultValue(type)`.
fn default_for_type(type_: &Value) -> Value {
    let first = match type_ {
        Value::Array(types) => types.first().and_then(Value::as_str),
        other => other.as_str(),
    };
    match first {
        Some("boolean") => json!(false),
        Some("integer" | "number") => json!(0),
        Some("string") => json!(""),
        Some("array") => json!([]),
        Some("object") => json!({}),
        _ => Value::Null,
    }
}

/// The default-override cases of platform/configuration/test/common/configurationRegistry.test.ts, fed through
/// package.json contributions. The policy and hidden-experiment cases are left out: the port has no policies.
#[cfg(test)]
mod tests {
    use super::*;

    fn registry(properties: Value, defaults: &[Value]) -> Registry {
        let mut registry = Registry::default();
        registry.register_configuration("test", &json!({ "contributes": { "configuration": { "id": "_test_default", "type": "object", "properties": properties } } }));
        for (index, overrides) in defaults.iter().enumerate() {
            registry.register_configuration_defaults(&format!("defaults{index}"), &json!({ "contributes": { "configurationDefaults": overrides } }));
        }
        registry.finish();
        registry
    }

    #[test]
    fn configuration_override() {
        let registry = registry(json!({ "config": { "type": "object" } }), &[json!({ "config": { "a": 1, "b": 2 } }), json!({ "[lang]": { "a": 2, "c": 3 } })]);
        assert_eq!(registry.property("config").unwrap().default, json!({ "a": 1, "b": 2 }));
        assert_eq!(registry.property("[lang]").unwrap().default, json!({ "a": 2, "c": 3 }));
    }

    #[test]
    fn override_defaults_cannot_override_a_disallowed_default() {
        let properties = json!({ "config.preventDefaultValueOverride": { "type": "object", "default": { "a": 0 }, "disallowConfigurationDefault": true } });
        let registry = registry(properties, &[json!({ "config.preventDefaultValueOverride": { "a": 1, "b": 2 } })]);
        assert_eq!(registry.property("config.preventDefaultValueOverride").unwrap().default, json!({ "a": 0 }));
    }

    #[test]
    fn override_defaults_merge() {
        let languages = registry(json!({}), &[json!({ "[lang]": { "a": 1, "b": 2 } }), json!({ "[lang]": { "a": 2, "c": 3 } })]);
        assert_eq!(languages.property("[lang]").unwrap().default, json!({ "a": 2, "b": 2, "c": 3 }));
        let registry = registry(json!({ "config": { "type": "object" } }), &[json!({ "config": { "a": 1, "b": 2 } }), json!({ "config": { "a": 2, "c": 3 } })]);
        assert_eq!(registry.property("config").unwrap().default, json!({ "a": 2, "b": 2, "c": 3 }));
    }

    #[test]
    fn contributed_defaults_respect_scope() {
        let properties = json!({ "app": { "type": "string", "default": "a", "scope": "application" }, "win": { "type": "string", "default": "a" } });
        let registry = registry(properties, &[json!({ "app": "b", "win": "b" })]);
        assert_eq!((&registry.property("app").unwrap().default, &registry.property("win").unwrap().default), (&json!("a"), &json!("b")));
    }

    #[test]
    fn core_registry_has_upstream_defaults() {
        let registry = Registry::with_extensions(&[]);
        assert_eq!(registry.property("editor.tabSize").unwrap().default, json!(4));
        assert_eq!(registry.property("workbench.colorTheme").unwrap().default, json!("Dark 2026"));
        assert_eq!(registry.property("[yaml]").unwrap().default, json!({ "editor.insertSpaces": true, "editor.tabSize": 2 }));
    }
}
