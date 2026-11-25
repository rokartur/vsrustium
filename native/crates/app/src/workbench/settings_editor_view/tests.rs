use serde_json::json;

use super::*;

#[test]
fn filter_toggles_add_and_remove_their_word() {
    assert_eq!(apply_filter_action("", "modifiedSettingsSearch"), "@modified");
    assert_eq!(apply_filter_action("editor", "modifiedSettingsSearch"), "editor @modified");
    assert_eq!(apply_filter_action("editor @modified", "modifiedSettingsSearch"), "editor");
    assert_eq!(apply_filter_action("editor ", "extSettingsSearch"), "editor @ext:");
}

#[test]
fn stable_preview_and_experimental_exclude_each_other() {
    assert_eq!(apply_filter_action("font @tag:preview", "stableSettingsSearch"), "font @stable");
    assert_eq!(apply_filter_action("@stable", "experimentalSettingsSearch"), "@tag:experimental");
}

#[test]
fn filter_menu_checks_the_toggles_in_the_query() {
    let checked: Vec<String> = filter_menu_actions("@modified @tag:advanced")
        .into_iter()
        .filter_map(|action| match action {
            MenuAction::Command { id, checked: true, .. } => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(checked, ["modifiedSettingsSearch", "advancedSettingsSearch"]);
}

#[test]
fn typed_numbers_parse_and_empty_nullable_numbers_are_null() {
    assert_eq!(parse_typed_value(ValueType::Integer, " 12 "), Ok(json!(12)));
    assert_eq!(parse_typed_value(ValueType::Integer, "1.5"), Err("Value must be an integer."));
    assert_eq!(parse_typed_value(ValueType::Number, "1.5"), Ok(json!(1.5)));
    assert_eq!(parse_typed_value(ValueType::NullableNumber, ""), Ok(json!(null)));
    assert_eq!(parse_typed_value(ValueType::String, " a "), Ok(json!(" a ")));
}

#[test]
fn setting_as_json_indents_by_two_spaces() {
    assert_eq!(setting_as_json("editor.fontSize", &json!(14)), "\"editor.fontSize\": 14");
    assert_eq!(setting_as_json("files.exclude", &json!({ "**/.git": true })), "\"files.exclude\": {\n  \"**/.git\": true\n}");
}
