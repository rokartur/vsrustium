//! taskDefinitionRegistry.ts and tasks.ts `TaskDefinition`/`KeyedTaskIdentifier`: the task types extensions
//! declare (`contributes.taskDefinitions`) and the key that names a task of a type by its identifying properties.

use serde_json::{Map, Value, json};

#[derive(Clone, Debug)]
pub struct TaskDefinition {
    pub extension_id: String,
    pub task_type: String,
    pub required: Vec<String>,
    /// The JSON schemas of the type's properties, in declaration order.
    pub properties: Map<String, Value>,
    /// The context the type's provider is enabled in (`_isTaskProviderEnabled`).
    pub when: Option<String>,
}

/// The declared task types, in extension order.
pub fn definitions(extensions: &[(&str, &Value)]) -> Vec<TaskDefinition> {
    let mut result = Vec::new();
    for (extension_id, contributes) in extensions {
        for definition in contributes["taskDefinitions"].as_array().into_iter().flatten() {
            let Some(task_type) = definition["type"].as_str() else { continue };
            result.push(TaskDefinition {
                extension_id: extension_id.to_string(),
                task_type: task_type.to_owned(),
                required: definition["required"].as_array().into_iter().flatten().filter_map(|value| value.as_str().map(str::to_owned)).collect(),
                properties: definition["properties"].as_object().cloned().unwrap_or_default(),
                when: definition["when"].as_str().map(str::to_owned),
            });
        }
    }
    result
}

/// `TaskDefinition.createTaskIdentifier`: the declared properties of `external` (required ones defaulted), keyed;
/// without a declaration the literal is kept as it is. `Err` is the missing required property.
pub fn create_task_identifier(external: &Value, definition: Option<&TaskDefinition>) -> Result<Value, String> {
    let Some(definition) = definition else {
        let mut copy = external.clone();
        if let Some(object) = copy.as_object_mut() {
            object.remove("_key");
        }
        return Ok(keyed(copy));
    };
    let mut literal = Map::new();
    literal.insert("type".into(), json!(definition.task_type));
    for (property, schema) in &definition.properties {
        let value = &external[property.as_str()];
        if !value.is_null() {
            literal.insert(property.clone(), value.clone());
        } else if definition.required.contains(property) {
            let default = match (&schema["default"], schema["type"].as_str()) {
                (default, _) if !default.is_null() => default.clone(),
                (_, Some("boolean")) => json!(false),
                (_, Some("number" | "integer")) => json!(0),
                (_, Some("string")) => json!(""),
                _ => return Err(property.clone()),
            };
            literal.insert(property.clone(), default);
        }
    }
    Ok(keyed(Value::Object(literal)))
}

/// `KeyedTaskIdentifier.create`.
fn keyed(value: Value) -> Value {
    let key = sorted_stringify(&value);
    let mut result = Map::new();
    result.insert("_key".into(), json!(key));
    if let Value::Object(object) = value {
        result.extend(object);
    }
    Value::Object(result)
}

/// `sortedStringify`: `key,value,` for the sorted keys, nested objects and arrays the same way, commas in
/// strings doubled.
fn sorted_stringify(value: &Value) -> String {
    let entries: Vec<(String, &Value)> = match value {
        Value::Object(object) => {
            let mut entries: Vec<(String, &Value)> = object.iter().map(|(key, value)| (key.clone(), value)).collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            entries
        }
        // `Object.keys` of an array: its indices, sorted as strings.
        Value::Array(items) => {
            let mut entries: Vec<(String, &Value)> = items.iter().enumerate().map(|(index, value)| (index.to_string(), value)).collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            entries
        }
        _ => Vec::new(),
    };
    let mut result = String::new();
    for (key, value) in entries {
        let text = match value {
            Value::Object(_) | Value::Array(_) => sorted_stringify(value),
            Value::String(text) => text.replace(',', ",,"),
            Value::Null => "null".into(),
            other => other.to_string(),
        };
        result.push_str(&key);
        result.push(',');
        result.push_str(&text);
        result.push(',');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn npm() -> TaskDefinition {
        let contributes = json!({ "taskDefinitions": [{ "type": "npm", "required": ["script"], "properties": { "script": { "type": "string" }, "path": { "type": "string" } } }] });
        definitions(&[("vscode.npm", &contributes)]).remove(0)
    }

    #[test]
    fn identifiers_keep_declared_properties_and_sort_the_key() {
        let identifier = create_task_identifier(&json!({ "type": "npm", "script": "build, all", "label": "x" }), Some(&npm())).unwrap();
        assert_eq!(identifier, json!({ "_key": "script,build,, all,type,npm,", "type": "npm", "script": "build, all" }));
        let defaulted = create_task_identifier(&json!({ "type": "npm" }), Some(&npm())).unwrap();
        assert_eq!(defaulted["script"], "");
        let undeclared = create_task_identifier(&json!({ "type": "custom", "flavor": { "b": 1, "a": [true] } }), None).unwrap();
        assert_eq!(undeclared["_key"], "flavor,a,0,true,,b,1,,type,custom,");
    }
}
