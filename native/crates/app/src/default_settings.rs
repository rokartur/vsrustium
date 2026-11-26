//! The default settings of preferencesModels.ts: `DefaultSettings.getRegisteredGroups`, the groups the Settings
//! editor and the default settings JSON are built from, and `DefaultRawSettingsEditorModel.content`, the text of
//! `vscode://defaultsettings/defaultSettings.jsonc` (Preferences: Open Default Settings (JSON)).

use std::cmp::Ordering;

use config::model::is_override_key;
use config::registry::{FOLDER_SCOPES, Property, Registry, Scope, WORKSPACE_SCOPES};
use config::service::Target;
use serde::Serialize;
use serde_json::Value;

use crate::command_palette::locale_compare;

/// `ISettingsGroup` with its one section.
pub struct SettingsGroup {
    pub id: String,
    pub title: String,
    pub order: Option<f64>,
    pub extension_id: Option<String>,
    /// The section title, the title of the setting that created the group.
    pub section_title: Option<String>,
    pub settings: Vec<Setting>,
}

/// `ISetting`: the default settings content and the Settings editor read it.
#[derive(Clone, Default)]
pub struct Setting {
    pub key: String,
    pub value: Value,
    pub description: Vec<String>,
    pub description_is_markdown: bool,
    /// The registered schema, for the type, enum, items and properties the Settings editor reads.
    pub schema: Value,
    pub scope: Option<Scope>,
    pub restricted: bool,
    /// `extensionInfo.id`; `None` for core settings.
    pub extension_id: Option<String>,
    pub deprecation_message: Option<String>,
    /// `deprecationMessageSeverity: 'info'`: an info icon in the foreground color instead of the error look.
    pub deprecation_is_info: bool,
    /// `deprecationMessageShowInSettings`: the tree keeps the setting while it is not configured.
    pub deprecation_show_in_settings: bool,
    pub enum_values: Option<Vec<Value>>,
    pub enum_descriptions: Option<Vec<Value>>,
    /// `parseOverrideSettings`: the settings a `[lang]` default sets.
    pub overrides: Vec<Setting>,
}

/// `DefaultSettings.getRegisteredGroups` for `target`.
pub fn registered_groups(registry: &Registry, target: Target) -> Vec<SettingsGroup> {
    let mut groups = parse_properties(registry, target);
    groups.retain(|group| !group.settings.is_empty());
    sort_groups(&mut groups);
    for group in &mut groups {
        group.settings.sort_by(|a, b| locale_compare(&a.key, &b.key));
    }
    groups
}

/// `DefaultSettings.getContentWithoutMostCommonlyUsed`: the registered groups of `target` as `SettingsContentBuilder`
/// writes them, each an object of one JSON array, two spaces indented, the last setting of a group without its comma.
pub fn default_settings_content(registry: &Registry, target: Target) -> String {
    let groups = registered_groups(registry, target);
    let mut lines = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        lines.push(if index == 0 { "[{" } else { "{" }.to_string());
        if let Some(title) = group.section_title.as_deref().filter(|title| !title.is_empty()) {
            lines.push(format!("  // {title}"));
        }
        for setting in &group.settings {
            push_setting(&mut lines, setting, "  ");
        }
        // `push_setting` ends a setting with its comma, then an empty line.
        let comma = lines.len() - 2;
        lines[comma].pop();
        lines.push(if index == groups.len() - 1 { "}]" } else { "}," }.to_string());
    }
    lines.join("\n")
}

/// `DefaultRawSettingsEditorModel.content`: every registered group, tab indented, inside one object.
pub fn raw_default_settings(registry: &Registry) -> String {
    let mut lines = vec!["{".to_string()];
    for group in registered_groups(registry, Target::UserLocal) {
        if let Some(title) = group.section_title.as_deref().filter(|title| !title.is_empty()) {
            lines.push(format!("\t// {title}"));
        }
        for setting in &group.settings {
            push_setting(&mut lines, setting, "\t");
        }
    }
    lines.push("}".to_string());
    lines.join("\n")
}

/// `parseProperties`: a group per section title (or id when untitled) and contributing extension. The `[lang]`
/// defaults come first: their node is the first of configurationRegistry.ts's `configurationContributors`.
fn parse_properties(registry: &Registry, target: Target) -> Vec<SettingsGroup> {
    let mut groups: Vec<SettingsGroup> = Vec::new();
    let (language_defaults, others): (Vec<&String>, Vec<&String>) = registry.keys().iter().partition(|key| is_override_key(key));
    for key in language_defaults.into_iter().chain(others) {
        let property = registry.property(key).expect("registered key");
        let section = &property.schema["section"];
        if !section.is_object() {
            continue;
        }
        let title = section["title"].as_str().filter(|title| !title.is_empty());
        let id = section["id"].as_str().filter(|id| !id.is_empty());
        let extension_id = section["extensionInfo"]["id"].as_str();
        let same_extension = |group: &SettingsGroup| group.extension_id.as_deref() == extension_id;
        let mut index = title.and_then(|title| groups.iter().position(|group| group.title == title && same_extension(group)));
        if index.is_none()
            && let Some(id) = id
        {
            index = groups.iter().position(|group| group.id == id && same_extension(group) && group.title.is_empty());
            if let (Some(index), Some(title)) = (index, title) {
                groups[index].title = title.to_string();
            }
        }
        let index = index.unwrap_or_else(|| {
            groups.push(SettingsGroup {
                id: id.unwrap_or_default().to_string(),
                title: title.unwrap_or_default().to_string(),
                order: section["order"].as_f64(),
                extension_id: extension_id.map(str::to_string),
                section_title: title.map(str::to_string),
                settings: Vec::new(),
            });
            groups.len() - 1
        });
        if let Some(setting) = parse_setting(key, property, target) {
            groups[index].settings.push(setting);
        }
    }
    groups
}

/// `compareGroups`: by order, then title; groups without an order last. Upstream's comparator puts an unordered
/// group after any other, so V8's stable sort keeps the unordered ones in registration order.
fn sort_groups(groups: &mut [SettingsGroup]) {
    groups.sort_by(|a, b| match (a.order, b.order) {
        (Some(a_order), Some(b_order)) if a_order == b_order => locale_compare(&a.title, &b.title),
        (Some(a_order), Some(b_order)) => a_order.partial_cmp(&b_order).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
}

/// `parseSetting`: `None` when the setting's scope does not belong to the target.
fn parse_setting(key: &str, property: &Property, target: Target) -> Option<Setting> {
    let matches_scope = match target {
        Target::WorkspaceFolder => FOLDER_SCOPES.contains(&property.scope),
        Target::Workspace => WORKSPACE_SCOPES.contains(&property.scope),
        _ => true,
    };
    if !matches_scope {
        return None;
    }
    let schema = &property.schema;
    let text = |key: &str| schema[key].as_str().filter(|text| !text.is_empty());
    let description = text("markdownDescription").or(text("description")).unwrap_or_default();
    let overrides = match property.default.as_object() {
        Some(values) if is_override_key(key) => values.iter().map(|(key, value)| Setting { key: key.clone(), value: value.clone(), ..Setting::default() }).collect(),
        _ => Vec::new(),
    };
    let mut enum_values = schema["enum"].as_array().cloned();
    let mut enum_descriptions = enum_descriptions_of(schema);
    let items = &schema["items"];
    if schema["type"] == "array" && items.is_object() && !items["type"].is_null() && items["enum"].is_array() {
        enum_values = items["enum"].as_array().cloned();
        enum_descriptions = enum_descriptions_of(items);
    }
    Some(Setting {
        key: key.to_string(),
        value: property.default.clone(),
        description: description.split('\n').map(str::to_string).collect(),
        description_is_markdown: text("markdownDescription").is_some(),
        schema: schema.clone(),
        scope: Some(property.scope),
        restricted: property.restricted,
        extension_id: property.source.clone(),
        deprecation_message: text("markdownDeprecationMessage").or(text("deprecationMessage")).map(str::to_string),
        deprecation_is_info: schema["deprecationMessageSeverity"] == "info",
        deprecation_show_in_settings: schema["deprecationMessageShowInSettings"] == true,
        enum_values,
        enum_descriptions,
        overrides,
    })
}

/// `markdownEnumDescriptions ?? enumDescriptions`.
fn enum_descriptions_of(schema: &Value) -> Option<Vec<Value>> {
    let markdown = schema.get("markdownEnumDescriptions").filter(|descriptions| !descriptions.is_null());
    markdown.or(schema.get("enumDescriptions")).and_then(Value::as_array).cloned()
}

/// `SettingsContentBuilder.pushSetting`: the description comments, `"key": value,` and an empty line.
fn push_setting(lines: &mut Vec<String>, setting: &Setting, indent: &str) {
    let deprecation = setting.deprecation_message.as_deref().map(|message| message.split('\n').collect()).unwrap_or_else(Vec::new);
    for line in deprecation.into_iter().chain(setting.description.iter().map(String::as_str)) {
        lines.push(format!("{indent}// {}", fix_setting_links(line)));
    }
    let enum_descriptions = setting.enum_descriptions.as_deref().unwrap_or_default();
    if let Some(enum_values) = &setting.enum_values
        && enum_descriptions.iter().any(is_truthy)
    {
        for (index, description) in enum_descriptions.iter().enumerate() {
            let display_enum = js_string(enum_values.get(index)).replace('\n', "\\n").replace('\r', "\\r");
            let line = match description.as_str().filter(|description| !description.is_empty()) {
                Some(description) => format!("{display_enum}: {}", fix_setting_links(description)),
                None => display_enum,
            };
            for (number, part) in line.split('\n').enumerate() {
                let bullet = if number == 0 { " - " } else { "" };
                lines.push(format!("{indent}// {bullet}{part}"));
            }
        }
    }
    let key = serde_json::to_string(&setting.key).expect("a string serializes");
    if setting.overrides.is_empty() {
        let value = stringify(&setting.value, indent);
        let mut value_lines = value.split('\n');
        lines.push(format!("{indent}{key}: {}", value_lines.next().unwrap_or_default()));
        lines.extend(value_lines.map(|line| format!("{indent}{line}")));
    } else {
        // `pushValue` appends ` {` to `"key": `, so the brace follows two spaces.
        lines.push(format!("{indent}{key}:  {{"));
        for sub_setting in &setting.overrides {
            push_setting(lines, sub_setting, &indent.repeat(2));
            lines.pop();
        }
        let last = lines.last_mut().expect("the last override line");
        last.pop();
        lines.push(format!("{indent}}}"));
    }
    let last = lines.last_mut().expect("the value line");
    last.push(',');
    lines.push(String::new());
}

/// `fixSettingLinks`: `` `#setting.id#` `` becomes `` `setting.id` ``.
pub fn fix_setting_links(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("`#") {
        let after = &rest[start + 2..];
        match after.find(['#', '`']) {
            Some(end) if after[end..].starts_with("#`") => {
                result.push_str(&rest[..start]);
                result.push('`');
                result.push_str(&after[..end]);
                result.push('`');
                rest = &after[end + 2..];
            }
            _ => {
                result.push_str(&rest[..start + 1]);
                rest = &rest[start + 1..];
            }
        }
    }
    result.push_str(rest);
    result
}

/// `JSON.stringify(value, null, indent)`.
fn stringify(value: &Value, indent: &str) -> String {
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    js_numbers(value).serialize(&mut serializer).expect("a JSON value serializes");
    String::from_utf8(out).expect("JSON is UTF-8")
}

/// Whole floats as integers, as JavaScript prints numbers.
fn js_numbers(value: &Value) -> Value {
    match value {
        Value::Number(number) if number.is_f64() => {
            let float = number.as_f64().expect("an f64 number");
            if float.fract() == 0.0 && float.abs() < 1e21 { Value::from(float as i64) } else { value.clone() }
        }
        Value::Array(items) => Value::Array(items.iter().map(js_numbers).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(key, value)| (key.clone(), js_numbers(value))).collect()),
        _ => value.clone(),
    }
}

/// `String(value)` of a JSON value; `undefined` for a missing one.
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items.iter().map(|item| if item.is_null() { String::new() } else { js_string(Some(item)) }).collect::<Vec<_>>().join(","),
        Some(Value::Object(_)) => "[object Object]".to_string(),
        Some(value) => stringify(&js_numbers(value), ""),
    }
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        _ => true,
    }
}

#[cfg(test)]
mod tests;
