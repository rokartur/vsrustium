//! The list setting widgets' data: settingsTree.ts `getListDisplayValue`, `getIncludeExcludeDisplayValue`,
//! `getObjectDisplayValue`, `getBoolObjectDisplayValue`, and the values their edits write (`computeNewList`,
//! `onDidChangeIncludeExclude`, `onDidChangeObject`).

#[cfg(test)]
mod tests;

use serde_json::{Map, Value};

use super::Inspection;
use crate::default_settings::Setting;

/// One row of a list widget: the item (or object key), its sibling or value, and the row's actions.
#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    pub key: String,
    /// The `when` sibling of an include/exclude pattern, the value of an object entry.
    pub value: Option<Value>,
    /// `keyDescription`, shown as the row's hover.
    pub description: Option<String>,
    pub removable: bool,
    pub resetable: bool,
}

/// A `SettingListEvent`; `index` is the row's position in the shown items.
#[derive(Clone, Debug, PartialEq)]
pub enum ListEdit {
    Add { key: String, value: Option<Value> },
    Change { index: usize, key: String, value: Option<Value> },
    Remove { index: usize },
    Reset { index: usize },
}

/// The kind of value an object entry holds: `getObjectValueType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryType {
    String,
    Boolean,
    Enum,
}

fn object(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// `{ ...default, ...scope }` when the scope sets the setting, else the default.
fn shown_object(inspection: &Inspection) -> Map<String, Value> {
    let mut data = object(&inspection.default_value);
    if inspection.is_configured {
        data.extend(object(&inspection.value));
    }
    data
}

fn scope_object(inspection: &Inspection) -> Map<String, Value> {
    if inspection.is_configured { object(&inspection.value) } else { Map::new() }
}

/// `getListDisplayValue`.
pub fn array_items(inspection: &Inspection) -> Vec<ListItem> {
    let Some(values) = inspection.value.as_array() else { return Vec::new() };
    values.iter().map(|value| ListItem { key: display(value), value: None, description: None, removable: true, resetable: false }).collect()
}

/// `getShowAddButtonList`: an enum list with unique items stops at its option count.
pub fn array_shows_add_button(setting: &Setting, items: usize) -> bool {
    let options = setting.schema["items"]["enum"].as_array().map(Vec::len);
    match options {
        Some(options) if setting.schema["uniqueItems"] == true => options > items,
        _ => true,
    }
}

/// `computeNewList`; `None` writes nothing in the user tab when the list equals the default.
pub fn new_array_value(inspection: &Inspection, workspace_target: bool, edit: &ListEdit) -> Option<Value> {
    let mut values: Vec<String> = inspection.value.as_array().map(|values| values.iter().map(display).collect()).unwrap_or_default();
    match edit {
        ListEdit::Remove { index } | ListEdit::Reset { index } => {
            values.remove(*index);
        }
        ListEdit::Change { index, key, .. } if *index < values.len() => values[*index] = key.clone(),
        ListEdit::Change { key, .. } | ListEdit::Add { key, .. } => values.push(key.clone()),
    }
    let default: Option<Vec<String>> = inspection.default_value.as_array().map(|values| values.iter().map(display).collect());
    if !workspace_target && default.as_ref() == Some(&values) {
        return None;
    }
    Some(Value::Array(values.into_iter().map(Value::String).collect()))
}

/// `getIncludeExcludeDisplayValue`: the patterns set to true or to a `when` clause.
pub fn include_exclude_items(inspection: &Inspection) -> Vec<ListItem> {
    let data = shown_object(inspection);
    let enabled = data.into_iter().filter(|(_, value)| is_truthy(value));
    enabled.map(|(key, value)| ListItem { key, value: value.get("when").cloned(), description: None, removable: true, resetable: false }).collect()
}

/// `onDidChangeIncludeExclude`: a removed default pattern is overridden with false; keys come out sorted.
pub fn new_include_exclude_value(inspection: &Inspection, items: &[ListItem], edit: &ListEdit) -> Option<Value> {
    let defaults = object(&inspection.default_value);
    let mut value = scope_object(inspection);
    let original = match edit {
        ListEdit::Add { .. } => None,
        ListEdit::Change { index, .. } | ListEdit::Remove { index } | ListEdit::Reset { index } => Some(&items[*index].key),
    };
    if let Some(original) = original {
        if defaults.contains_key(original) {
            value.insert(original.clone(), Value::Bool(false));
        } else {
            value.remove(original);
        }
    }
    if let ListEdit::Add { key, value: sibling } | ListEdit::Change { key, value: sibling, .. } = edit {
        let sibling = sibling.as_ref().and_then(Value::as_str).filter(|sibling| !sibling.is_empty());
        match sibling {
            None if defaults.contains_key(key) => {
                value.remove(key);
            }
            None => {
                value.insert(key.clone(), Value::Bool(true));
            }
            Some(when) => {
                value.insert(key.clone(), serde_json::json!({ "when": when }));
            }
        }
    }
    if value.is_empty() {
        return None;
    }
    let mut sorted: Vec<(String, Value)> = value.into_iter().collect();
    sorted.sort_by(|(a, _), (b, _)| crate::command_palette::locale_compare(a, b));
    Some(Value::Object(sorted.into_iter().collect()))
}

/// `objectSettingSupportsRemoveDefaultValue`.
fn supports_remove_default(key: &str) -> bool {
    key == "workbench.editor.customLabels.patterns"
}

/// `getObjectValueType`.
pub fn entry_type(schema: &Value) -> EntryType {
    if let Some(any_of) = schema["anyOf"].as_array() {
        return if any_of.iter().any(|schema| entry_type(schema) == EntryType::Enum) { EntryType::Enum } else { EntryType::String };
    }
    if schema["type"] == "boolean" {
        EntryType::Boolean
    } else if schema["type"] == "string" && schema["enum"].as_array().is_some_and(|options| !options.is_empty()) {
        EntryType::Enum
    } else {
        EntryType::String
    }
}

/// The schema of an object entry: its property, the first matching pattern, or `additionalProperties`.
pub fn entry_schema<'a>(setting: &'a Setting, key: &str) -> Option<&'a Value> {
    let schema = &setting.schema;
    if let Some(property) = schema["properties"].get(key) {
        return Some(property);
    }
    let patterns = schema["patternProperties"].as_object();
    let pattern = patterns.into_iter().flatten().find(|(pattern, _)| regex::Regex::new(pattern).is_ok_and(|pattern| pattern.is_match(key)));
    if let Some((_, schema)) = pattern {
        return Some(schema);
    }
    schema["additionalProperties"].is_object().then(|| &schema["additionalProperties"])
}

/// `getObjectDisplayValue`: each key with its value as the widget shows it; null entries are hidden.
pub fn object_items(setting: &Setting, inspection: &Inspection) -> Vec<ListItem> {
    let defaults = object(&inspection.default_value);
    let mut items = Vec::new();
    for (key, value) in shown_object(inspection) {
        if value.is_null() {
            continue;
        }
        let default = defaults.get(&key);
        let schema = entry_schema(setting, &key);
        let description = schema.and_then(|schema| schema["description"].as_str()).map(str::to_string);
        let value = match schema.map(entry_type) {
            Some(EntryType::Boolean) => Value::Bool(is_truthy(&value)),
            _ => Value::String(display(&value)),
        };
        let (removable, resetable) = match setting.schema["properties"].get(&key) {
            Some(_) => (default.is_none_or(Value::is_null), default.is_some_and(|default| !default.is_null())),
            None => (default.is_none() || supports_remove_default(&setting.key), default.is_some_and(|default| is_truthy(default) && display(default) != display(&value))),
        };
        items.push(ListItem { key, value: Some(value), description, removable, resetable });
    }
    items
}

/// `ObjectSettingDropdownWidget` `showAddButton`: closed objects stop once each property is listed.
pub fn object_shows_add_button(setting: &Setting, items: &[ListItem]) -> bool {
    let schema = &setting.schema;
    if schema["additionalProperties"] != false {
        return true;
    }
    let properties = schema["properties"].as_object().map(|properties| properties.keys().collect::<Vec<_>>()).unwrap_or_default();
    !properties.iter().all(|property| items.iter().any(|item| &&item.key == property)) || !schema["patternProperties"].is_null()
}

/// `onDidChangeObject` then `parseNumericObjectValues`: the scope's object after the edit, entries equal to their
/// default dropped.
pub fn new_object_value(setting: &Setting, inspection: &Inspection, items: &[ListItem], edit: &ListEdit) -> Option<Value> {
    let defaults = object(&inspection.default_value);
    let scope = scope_object(inspection);
    let removes_default = supports_remove_default(&setting.key);
    let mut value = scope.clone();
    for (index, item) in items.iter().enumerate() {
        match edit {
            ListEdit::Change { index: changed, key, value: new } if *changed == index => {
                if &item.key != key && removes_default && defaults.contains_key(&item.key) {
                    value.insert(item.key.clone(), Value::Null);
                } else {
                    value.remove(&item.key);
                }
                value.insert(key.clone(), new.clone().unwrap_or(Value::Null));
            }
            ListEdit::Change { key, .. } if key == &item.key => {}
            _ => {
                value.insert(item.key.clone(), item.value.clone().unwrap_or(Value::Null));
            }
        }
    }
    match edit {
        ListEdit::Remove { index } | ListEdit::Reset { index } => {
            let item = &items[*index];
            let removing_default = matches!(edit, ListEdit::Remove { .. }) && removes_default && defaults.get(&item.key).is_some_and(|default| Some(default) == item.value.as_ref());
            if removing_default {
                value.insert(item.key.clone(), Value::Null);
            } else {
                value.remove(&item.key);
            }
        }
        ListEdit::Add { key, value: new } => {
            value.insert(key.clone(), new.clone().unwrap_or(Value::Null));
        }
        ListEdit::Change { .. } => {}
    }
    value.retain(|key, entry| !(scope.get(key) != Some(entry) && defaults.get(key) == Some(entry) && !(removes_default && entry.is_null())));
    if value.is_empty() {
        return None;
    }
    for (key, entry) in value.iter_mut() {
        let numeric = entry_schema(setting, key).is_some_and(|schema| schema["type"] == "number" || schema["type"] == "integer");
        if let (true, Some(text)) = (numeric, entry.as_str()) {
            // `Number(text)`: integers stay integers in settings.json, text that is not a number becomes null.
            *entry = match (text.trim().parse::<i64>(), text.trim().parse::<f64>()) {
                (Ok(integer), _) => Value::from(integer),
                (_, Ok(number)) => serde_json::Number::from_f64(number).map_or(Value::Null, Value::Number),
                _ => Value::Null,
            };
        }
    }
    Some(Value::Object(value))
}

/// `getBoolObjectDisplayValue`: every declared property with its checkbox.
pub fn bool_object_items(setting: &Setting, inspection: &Inspection) -> Vec<ListItem> {
    let data = shown_object(inspection);
    let properties = setting.schema["properties"].as_object().into_iter().flatten();
    properties
        .map(|(key, schema)| ListItem {
            key: key.clone(),
            value: Some(Value::Bool(data.get(key).is_some_and(is_truthy))),
            description: schema["description"].as_str().map(str::to_string),
            removable: false,
            resetable: true,
        })
        .collect()
}

/// `SettingBoolObjectRenderer.onDidChangeObject` for a toggled checkbox.
pub fn new_bool_object_value(inspection: &Inspection, items: &[ListItem], index: usize, checked: bool) -> Option<Value> {
    let defaults = object(&inspection.default_value);
    let scope = scope_object(inspection);
    let mut value = scope.clone();
    for (position, item) in items.iter().enumerate() {
        let entry = if position == index { Value::Bool(checked) } else { item.value.clone().unwrap_or(Value::Null) };
        value.insert(item.key.clone(), entry);
    }
    value.retain(|key, entry| !(scope.get(key) != Some(entry) && defaults.get(key) == Some(entry)));
    if value.is_empty() { None } else { Some(Value::Object(value)) }
}

/// A key or value in an edit row: typed text, or one of `options` (`true`/`false` for a boolean).
#[derive(Clone, Debug, PartialEq)]
pub struct EditField {
    pub kind: EntryType,
    pub text: String,
    pub options: Vec<String>,
}

impl EditField {
    pub fn text(text: String) -> Self {
        EditField { kind: EntryType::String, text, options: Vec::new() }
    }

    pub fn value(&self) -> Value {
        match self.kind {
            EntryType::Boolean => Value::Bool(self.text == "true"),
            _ => Value::String(self.text.clone()),
        }
    }
}

/// `getEnumOptionsFromSchema`.
fn enum_options(schema: &Value) -> Vec<String> {
    if let Some(any_of) = schema["anyOf"].as_array() {
        return any_of.iter().flat_map(enum_options).collect();
    }
    schema["enum"].as_array().into_iter().flatten().map(display).collect()
}

/// `createArraySuggester`: the item enum's options; with `uniqueItems` only unused ones and the edited item's.
pub fn array_key_options(setting: &Setting, keys: &[String], index: Option<usize>) -> Vec<String> {
    let unique = setting.schema["uniqueItems"] == true;
    let edited = index.and_then(|index| keys.get(index));
    let options = enum_options(&setting.schema["items"]);
    options.into_iter().filter(|option| !unique || Some(option) == edited || !keys.contains(option)).collect()
}

/// `createObjectKeySuggester`: the declared properties not set yet.
pub fn object_key_options(setting: &Setting, keys: &[String]) -> Vec<String> {
    let properties = setting.schema["properties"].as_object().into_iter().flatten();
    properties.map(|(key, _)| key.clone()).filter(|key| !keys.contains(key)).collect()
}

/// `createObjectValueSuggester`: the edit field for a key's value, from its property, pattern or `additionalProperties`.
pub fn object_value_suggestion(setting: &Setting, key: &str) -> Option<EditField> {
    let schema = entry_schema(setting, key)?;
    let kind = entry_type(schema);
    let default = schema.get("default");
    Some(match kind {
        EntryType::Boolean => EditField { kind, text: default.map_or("true".into(), display), options: vec!["true".into(), "false".into()] },
        EntryType::Enum => {
            let options = enum_options(schema);
            EditField { kind, text: default.map_or_else(|| options[0].clone(), display), options }
        }
        EntryType::String => EditField::text(default.map_or_else(String::new, display)),
    })
}

/// The edit field of a shown object entry: `getObjectEntryValueDisplayValue` with the entry's options.
pub fn object_value_field(setting: &Setting, item: &ListItem) -> EditField {
    let kind = entry_schema(setting, &item.key).map_or(EntryType::String, entry_type);
    let text = item.value.as_ref().map(display).unwrap_or_default();
    match kind {
        EntryType::Boolean => EditField { kind, text, options: vec!["true".into(), "false".into()] },
        EntryType::Enum => EditField { kind, text, options: entry_schema(setting, &item.key).map(enum_options).unwrap_or_default() },
        EntryType::String => EditField::text(text),
    }
}

/// `ObjectSettingDropdownWidget.shouldUseSuggestion`: a changed key replaces the value field with its suggestion.
pub fn should_use_suggestion(original_empty: bool, previous: &EditField, suggested: &EditField) -> bool {
    if suggested.kind != EntryType::Enum && suggested.kind == previous.kind && suggested.text == previous.text {
        return false;
    }
    if original_empty {
        return true;
    }
    if previous.kind == suggested.kind && suggested.kind != EntryType::Enum {
        return false;
    }
    !(previous.kind == EntryType::Enum && suggested.kind == EntryType::Enum && previous.options.iter().all(|option| suggested.options.contains(option)))
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64() != Some(0.),
        Value::String(text) => !text.is_empty(),
        _ => true,
    }
}

/// `'' + data`: strings bare, others as JSON.
pub fn display(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
