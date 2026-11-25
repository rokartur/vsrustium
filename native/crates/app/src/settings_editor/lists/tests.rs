use serde_json::{Value, json};

use super::*;
use crate::default_settings::Setting;

fn setting(key: &str, schema: Value) -> Setting {
    Setting {
        key: key.into(),
        value: schema["default"].clone(),
        description: Vec::new(),
        description_is_markdown: false,
        schema,
        scope: None,
        restricted: false,
        extension_id: None,
        deprecation_message: None,
        deprecation_is_info: false,
        deprecation_show_in_settings: false,
        enum_values: None,
        enum_descriptions: None,
        overrides: Vec::new(),
    }
}

fn inspection(default: Value, scope: Option<Value>) -> Inspection {
    Inspection { value: scope.clone().unwrap_or_else(|| default.clone()), default_value: default, is_configured: scope.is_some(), ..Default::default() }
}

#[test]
fn array_edits_return_nothing_when_back_at_the_default() {
    let state = inspection(json!(["a"]), Some(json!(["a", "b"])));
    assert_eq!(new_array_value(&state, false, &ListEdit::Remove { index: 1 }), None);
    assert_eq!(new_array_value(&state, true, &ListEdit::Remove { index: 1 }), Some(json!(["a"])));
    assert_eq!(new_array_value(&state, false, &ListEdit::Change { index: 0, key: "c".into(), value: None }), Some(json!(["c", "b"])));
    assert_eq!(new_array_value(&state, false, &ListEdit::Add { key: "d".into(), value: None }), Some(json!(["a", "b", "d"])));
}

#[test]
fn include_exclude_overrides_removed_defaults_with_false_and_sorts() {
    let state = inspection(json!({ "**/.git": true, "**/.DS_Store": true }), Some(json!({ "out": true })));
    let items = include_exclude_items(&state);
    assert_eq!(items.iter().map(|item| item.key.as_str()).collect::<Vec<_>>(), ["**/.git", "**/.DS_Store", "out"]);
    assert_eq!(new_include_exclude_value(&state, &items, &ListEdit::Remove { index: 0 }), Some(json!({ "**/.git": false, "out": true })));
    let when = ListEdit::Add { key: "*.js".into(), value: Some(json!("$(basename).ts")) };
    assert_eq!(new_include_exclude_value(&state, &items, &when), Some(json!({ "*.js": { "when": "$(basename).ts" }, "out": true })));
    assert_eq!(new_include_exclude_value(&state, &items, &ListEdit::Remove { index: 2 }), None);
}

#[test]
fn object_edits_drop_entries_equal_to_their_default_and_parse_numbers() {
    let setting = setting("x.sizes", json!({ "type": "object", "additionalProperties": { "type": "number" }, "default": { "a": 1 } }));
    let state = inspection(json!({ "a": 1 }), Some(json!({ "a": 2 })));
    let items = object_items(&setting, &state);
    assert_eq!(items[0].key, "a");
    assert!(items[0].resetable);
    assert_eq!(new_object_value(&setting, &state, &items, &ListEdit::Reset { index: 0 }), None);
    let add = ListEdit::Add { key: "b".into(), value: Some(json!("3")) };
    assert_eq!(new_object_value(&setting, &state, &items, &add), Some(json!({ "a": 2, "b": 3 })));
    let change = ListEdit::Change { index: 0, key: "a".into(), value: Some(json!("1")) };
    // Upstream compares the typed text with the default before parsing it, so "1" is written as 1.
    assert_eq!(new_object_value(&setting, &state, &items, &change), Some(json!({ "a": 1 })));
}

#[test]
fn unique_enum_arrays_offer_only_unused_options_and_the_edited_one() {
    let setting = setting("x.list", json!({ "type": "array", "uniqueItems": true, "items": { "type": "string", "enum": ["a", "b", "c"] } }));
    let keys = vec!["a".to_string(), "b".to_string()];
    assert_eq!(array_key_options(&setting, &keys, None), ["c"]);
    assert_eq!(array_key_options(&setting, &keys, Some(1)), ["b", "c"]);
}

#[test]
fn object_keys_suggest_their_schema_value() {
    let setting = setting(
        "x.map",
        json!({ "type": "object", "properties": { "on": { "type": "boolean", "default": false }, "mode": { "type": "string", "enum": ["x", "y"] } }, "additionalProperties": { "type": "string" } }),
    );
    assert_eq!(object_key_options(&setting, &["on".to_string()]), ["mode"]);
    let mode = object_value_suggestion(&setting, "mode").unwrap();
    assert_eq!((mode.kind, mode.text.as_str()), (EntryType::Enum, "x"));
    let on = object_value_suggestion(&setting, "on").unwrap();
    assert_eq!((on.kind, on.value()), (EntryType::Boolean, json!(false)));
    let typed = EditField::text("v".into());
    assert!(should_use_suggestion(false, &typed, &mode));
    assert!(!should_use_suggestion(false, &typed, &object_value_suggestion(&setting, "other").unwrap()));
    assert!(should_use_suggestion(true, &typed, &object_value_suggestion(&setting, "other").unwrap()));
}

#[test]
fn bool_objects_list_every_property_and_write_only_changes() {
    let setting = setting("x.flags", json!({ "type": "object", "properties": { "a": { "type": "boolean" }, "b": { "type": "boolean" } }, "additionalProperties": false }));
    let state = inspection(json!({ "a": true, "b": false }), None);
    let items = bool_object_items(&setting, &state);
    assert_eq!(items.len(), 2);
    assert_eq!(new_bool_object_value(&state, &items, 1, true), Some(json!({ "b": true })));
    assert_eq!(new_bool_object_value(&state, &items, 0, true), None);
}
