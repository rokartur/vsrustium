//! `getCompressedContent` of base/common/jsonSchema.ts and the settings schemas of the JSON contribution registry
//! (platform/jsonschemas/common/jsonContributionRegistry.ts) that configurationRegistry.ts and
//! configurationService.ts `registerConfigurationSchemas` register.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::model::is_override_key;
use crate::registry::{Registry, Scope};

/// `getCompressedContent`: the schema as JSON with each subschema that occurs more than once, and is 30 characters or
/// longer, moved to `$defs` and replaced by a `$ref`. Subschemas are found by `traverseNodes`, and the first one
/// found, in its stack order, is the one kept.
pub fn compressed_content(schema: &Value) -> String {
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut group_by_text: HashMap<String, usize> = HashMap::new();
    let mut group_of: HashMap<String, usize> = HashMap::new();
    let mut has_dups = false;
    let mut to_walk = vec![String::new()];
    while let Some(pointer) = to_walk.pop() {
        let node = schema.pointer(&pointer).expect("a walked pointer");
        if !pointer.is_empty() {
            let text = node.to_string();
            if text.encode_utf16().count() >= 30 {
                if let Some(&group) = group_by_text.get(&text) {
                    groups[group].push(pointer.clone());
                    group_of.insert(pointer, group);
                    has_dups = true;
                    continue;
                }
                group_by_text.insert(text, groups.len());
                group_of.insert(pointer.clone(), groups.len());
                groups.push(vec![pointer.clone()]);
            }
        }
        collect_subschemas(node, &pointer, &mut to_walk);
    }
    if !has_dups {
        return schema.to_string();
    }
    let mut def_node_name = "$defs".to_string();
    while schema.get(&def_node_name).is_some() {
        def_node_name.push('_');
    }
    let mut writer = Writer { schema, group_of: &group_of, groups: &groups, def_node_name: &def_node_name, ids: HashMap::new(), definitions: Vec::new() };
    let mut out = String::new();
    writer.write(&mut out, "", true);
    let mut def_strings = Vec::new();
    let mut index = 0;
    while index < writer.definitions.len() {
        let mut definition = format!("\"_{index}\":");
        let pointer = writer.definitions[index].clone();
        writer.write(&mut definition, &pointer, true);
        def_strings.push(definition);
        index += 1;
    }
    out.pop();
    format!("{out},\"{def_node_name}\":{{{}}}}}", def_strings.join(","))
}

/// `traverseNodes`: the subschemas of `node` pushed in upstream's order, so the walk pops them last first.
fn collect_subschemas(node: &Value, pointer: &str, to_walk: &mut Vec<String>) {
    let child = |key: &str| format!("{pointer}/{}", escape(key));
    for key in ["additionalItems", "additionalProperties", "not", "contains", "propertyNames", "if", "then", "else", "unevaluatedItems", "unevaluatedProperties"] {
        if node[key].is_object() {
            to_walk.push(child(key));
        }
    }
    for key in ["definitions", "$defs", "properties", "patternProperties", "dependencies", "dependentSchemas"] {
        for (name, entry) in node[key].as_object().into_iter().flatten() {
            if entry.is_object() {
                to_walk.push(format!("{}/{}", child(key), escape(name)));
            }
        }
    }
    for key in ["anyOf", "allOf", "oneOf", "prefixItems", "items"] {
        match &node[key] {
            Value::Array(entries) => {
                for (index, entry) in entries.iter().enumerate() {
                    if entry.is_object() {
                        to_walk.push(format!("{}/{index}", child(key)));
                    }
                }
            }
            Value::Object(_) if key == "items" => to_walk.push(child(key)),
            _ => {}
        }
    }
}

/// A JSON pointer reference token (RFC 6901).
fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// The `stringify` of `getCompressedContent`: `JSON.stringify` with a replacer that turns a duplicated subschema into
/// a `$ref` to its definition, numbering definitions as it meets them.
struct Writer<'a> {
    schema: &'a Value,
    group_of: &'a HashMap<String, usize>,
    groups: &'a [Vec<String>],
    def_node_name: &'a str,
    ids: HashMap<usize, usize>,
    definitions: Vec<String>,
}

impl Writer<'_> {
    fn write(&mut self, out: &mut String, pointer: &str, is_root: bool) {
        if !is_root && let Some(&group) = self.group_of.get(pointer).filter(|&&group| self.groups[group].len() > 1) {
            let id = match self.ids.get(&group) {
                Some(&id) => id,
                None => {
                    let id = self.definitions.len();
                    self.ids.insert(group, id);
                    self.definitions.push(self.groups[group][0].clone());
                    id
                }
            };
            out.push_str(&format!("{{\"$ref\":\"#/{}/_{id}\"}}", self.def_node_name));
            return;
        }
        match self.schema.pointer(pointer).expect("a written pointer") {
            Value::Object(entries) => {
                out.push('{');
                for (index, key) in entries.keys().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(&Value::from(key.as_str()).to_string());
                    out.push(':');
                    self.write(out, &format!("{pointer}/{}", escape(key)), false);
                }
                out.push('}');
            }
            Value::Array(entries) => {
                out.push('[');
                for index in 0..entries.len() {
                    if index > 0 {
                        out.push(',');
                    }
                    self.write(out, &format!("{pointer}/{index}"), false);
                }
                out.push(']');
            }
            leaf => out.push_str(&leaf.to_string()),
        }
    }
}

pub const RESOURCE_LANGUAGE_SETTINGS_SCHEMA_ID: &str = "vscode://schemas/settings/resourceLanguage";
const OVERRIDE_PROPERTY_PATTERN: &str = r"^(\[([^\]]+)\])+$";

/// The settings schemas by id, as `registerConfigurationSchemas` and configurationRegistry.ts register them.
/// `in_workspace` is a multi-root workspace (`WorkbenchState.WORKSPACE`), whose folders take resource settings only.
// The port has no workspace trust, so `checkAndFilterPropertiesRequiringTrust` keeps every property.
pub fn settings_schemas(registry: &Registry, in_workspace: bool) -> Vec<(&'static str, Value)> {
    let language_override = json!({
        "type": "object",
        "description": "Configure editor settings to be overridden for a language.",
        "errorMessage": "This setting does not support per-language configuration.",
        "$ref": RESOURCE_LANGUAGE_SETTINGS_SCHEMA_ID,
    });
    let pattern_properties = json!({ OVERRIDE_PROPERTY_PATTERN: language_override });
    let mut all = Map::new();
    let mut by_scope: HashMap<Scope, Map<String, Value>> = HashMap::new();
    let mut resource_language = Map::new();
    for key in registry.keys() {
        let property = registry.property(key).expect("a registered key");
        if is_override_key(key) {
            insert_language_override(&mut all, &mut by_scope, key, &language_override, property.default.clone());
            continue;
        }
        let mut schema = property.schema.clone();
        schema["default"] = property.default.clone();
        all.insert(key.clone(), schema.clone());
        if property.scope == Scope::LanguageOverridable {
            resource_language.insert(key.clone(), schema.clone());
            by_scope.entry(Scope::Resource).or_default().insert(key.clone(), schema);
        } else {
            by_scope.entry(property.scope).or_default().insert(key.clone(), schema);
        }
    }
    for id in &registry.override_identifiers {
        let key = format!("[{id}]");
        if !all.contains_key(&key) {
            // `updatePropertyDefaultValue` without a configurationDefaults value: the default of type object.
            insert_language_override(&mut all, &mut by_scope, &key, &language_override, json!({}));
        }
    }
    let properties = |scopes: &[Scope]| {
        let mut properties = Map::new();
        for scope in scopes {
            properties.extend(by_scope.get(scope).cloned().unwrap_or_default());
        }
        Value::Object(properties)
    };
    let settings_schema = |properties: Value| json!({ "properties": properties, "patternProperties": pattern_properties, "additionalProperties": true, "allowTrailingCommas": true, "allowComments": true });
    let workspace = settings_schema(properties(&[Scope::MachineOverridable, Scope::Window, Scope::Resource]));
    let folder = match in_workspace {
        true => settings_schema(properties(&[Scope::MachineOverridable, Scope::Resource])),
        false => workspace.clone(),
    };
    let default_properties = |properties: &Map<String, Value>| {
        let mut defaults = Map::new();
        for (key, schema) in properties {
            // `Object.assign({ deprecationMessage: undefined }, schema)` moves a deprecation message first.
            let mut ordered = Map::new();
            if let Some(message) = schema.get("deprecationMessage") {
                ordered.insert("deprecationMessage".into(), message.clone());
            }
            ordered.extend(schema.as_object().expect("a schema object").clone());
            defaults.insert(key.clone(), Value::Object(ordered));
        }
        Value::Object(defaults)
    };
    let defaults_schema = json!({
        "properties": default_properties(&all),
        "patternProperties": default_properties(pattern_properties.as_object().expect("an object")),
        "additionalProperties": true,
        "allowTrailingCommas": true,
        "allowComments": true,
    });
    let overridable = |scopes: &[Scope]| {
        let mut properties = Map::new();
        for scope in scopes {
            for (key, schema) in by_scope.get(scope).into_iter().flatten() {
                if schema["disallowConfigurationDefault"] != json!(true) {
                    properties.insert(key.clone(), schema.clone());
                }
            }
        }
        Value::Object(properties)
    };
    let config_defaults = json!({
        "type": "object",
        "description": "Contribute defaults for configurations",
        "properties": overridable(&[Scope::MachineOverridable, Scope::Window, Scope::Resource]),
        "patternProperties": { OVERRIDE_PROPERTY_PATTERN: { "type": "object", "default": {}, "$ref": RESOURCE_LANGUAGE_SETTINGS_SCHEMA_ID } },
        "additionalProperties": false,
    });
    vec![
        (RESOURCE_LANGUAGE_SETTINGS_SCHEMA_ID, json!({ "properties": resource_language, "patternProperties": {}, "additionalProperties": true, "allowTrailingCommas": true, "allowComments": true })),
        ("vscode://schemas/settings/default", defaults_schema),
        ("vscode://schemas/settings/user", settings_schema(Value::Object(all))),
        ("vscode://schemas/settings/profile", settings_schema(properties(&[Scope::Machine, Scope::MachineOverridable, Scope::Window, Scope::Resource]))),
        ("vscode://schemas/settings/machine", settings_schema(properties(&[Scope::ApplicationMachine, Scope::Machine, Scope::MachineOverridable, Scope::Window, Scope::Resource]))),
        ("vscode://schemas/settings/workspace", workspace),
        ("vscode://schemas/settings/folder", folder),
        ("vscode://schemas/settings/configurationDefaults", config_defaults),
    ]
}

/// `updateOverridePropertyPatternKey`: a `[lang]` key takes the override schema in every scope but language-overridable.
fn insert_language_override(all: &mut Map<String, Value>, by_scope: &mut HashMap<Scope, Map<String, Value>>, key: &str, language_override: &Value, default: Value) {
    let mut schema = language_override.clone();
    schema["default"] = default;
    all.insert(key.to_string(), schema.clone());
    for scope in [Scope::Application, Scope::ApplicationMachine, Scope::Machine, Scope::MachineOverridable, Scope::Window, Scope::Resource] {
        by_scope.entry(scope).or_default().insert(key.to_string(), schema.clone());
    }
}

#[cfg(test)]
mod tests;
