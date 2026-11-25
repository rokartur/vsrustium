use config::model::ConfigurationModel;
use config::registry::Registry;
use config::service::Target;
use serde_json::{Value, json};

use super::unsupported_settings_markers;

fn registry() -> Registry {
    let package = json!({
        "contributes": {
            "configuration": {
                "properties": {
                    "test.application": { "type": "string", "scope": "application" },
                    "test.machine": { "type": "string", "scope": "machine" },
                    "test.window": { "type": "string", "scope": "window" },
                    "test.resource": { "type": "string", "scope": "resource" },
                    "test.overridable": { "type": "string", "scope": "language-overridable" },
                    "test.preview": { "type": "string", "scope": "resource", "tags": ["preview"] },
                    "test.experimental": { "type": "string", "scope": "resource", "tags": ["experimental"] },
                    "test.policy": { "type": "string", "scope": "application", "policy": { "name": "TestPolicy", "minimumVersion": "1.0" } }
                }
            }
        }
    });
    Registry::with_extensions(&[("test.ext", &package)])
}

fn summary(markers: &[Value]) -> Vec<(u64, u64, u64, u64, String, bool)> {
    markers
        .iter()
        .map(|marker| {
            let number = |key: &str| marker[key].as_u64().unwrap();
            let unnecessary = marker["tags"] == json!([1]);
            (number("startLineNumber"), number("startColumn"), number("endLineNumber"), number("endColumn"), marker["message"].as_str().unwrap().to_string(), unnecessary)
        })
        .collect()
}

#[test]
fn unknown_settings_get_a_faded_hint_over_the_key_and_value() {
    let text = "{\n\t\"editor.fontSize\": 14,\n\t\"nope.unknown\": { \"a\": 1 },\n\t\"\u{e9}\": true\n}";
    let markers = unsupported_settings_markers(text, Target::UserLocal, false, &registry(), &ConfigurationModel::default());
    assert_eq!(
        summary(&markers),
        vec![
            (3, 2, 3, 28, "Unknown Configuration Setting".into(), true),
            (4, 2, 4, 11, "Unknown Configuration Setting".into(), true),
        ]
    );
    assert_eq!(markers[0]["severity"], 1, "MarkerSeverity.Hint");
}

#[test]
fn language_overrides_check_each_setting_inside() {
    let text = "{\n\t\"[plaintext]\": {\n\t\t\"test.overridable\": \"a\",\n\t\t\"test.window\": \"b\",\n\t\t\"nope\": 1\n\t}\n}";
    let markers = unsupported_settings_markers(text, Target::UserLocal, false, &registry(), &ConfigurationModel::default());
    assert_eq!(
        summary(&markers),
        vec![
            (4, 3, 4, 21, "This setting cannot be applied because it is not registered as language override setting.".into(), true),
            (5, 3, 5, 12, "Unknown Configuration Setting".into(), true),
        ]
    );
}

#[test]
fn scopes_that_cannot_apply_in_the_target_are_marked() {
    let text = "{\"test.application\": \"\", \"test.machine\": \"\", \"test.window\": \"\", \"test.resource\": \"\"}";
    let messages = |target| summary(&unsupported_settings_markers(text, target, false, &registry(), &ConfigurationModel::default())).into_iter().map(|marker| marker.4).collect::<Vec<_>>();
    assert_eq!(messages(Target::UserLocal), Vec::<String>::new());
    let application = "This setting has an application scope and can only be set in the settings file from the Default profile.";
    let machine = "This setting can only be applied in user settings in local window or in remote settings in remote window.";
    let window = "This setting cannot be applied in this workspace. It will be applied when you open the containing workspace folder directly.";
    assert_eq!(messages(Target::Workspace), vec![application, machine]);
    assert_eq!(messages(Target::WorkspaceFolder), vec![application, machine, window]);
}

#[test]
fn preview_and_experimental_settings_get_a_hint_that_does_not_fade() {
    let text = "{\"test.preview\": \"\", \"test.experimental\": \"\"}";
    let markers = summary(&unsupported_settings_markers(text, Target::UserLocal, false, &registry(), &ConfigurationModel::default()));
    assert!(markers[0].4.starts_with("Preview setting:") && !markers[0].5);
    assert!(markers[1].4.starts_with("Experimental setting:") && !markers[1].5);
}

#[test]
fn the_workspace_file_checks_its_settings_key_only() {
    let text = "{\"folders\": [], \"nope\": 1, \"settings\": {\"nope.too\": 1, \"mcp\": {}}}";
    let markers = summary(&unsupported_settings_markers(text, Target::Workspace, true, &registry(), &ConfigurationModel::default()));
    assert_eq!(markers, vec![(1, 41, 1, 54, "Unknown Configuration Setting".into(), true)]);
}

#[test]
fn a_key_without_a_value_is_not_a_setting() {
    let markers = unsupported_settings_markers("{\n\t\"nope\"\n}", Target::UserLocal, false, &registry(), &ConfigurationModel::default());
    assert!(markers.is_empty());
}

#[test]
fn a_setting_set_by_policy_gets_only_the_policy_hint() {
    let text = "{\n\t\"test.policy\": \"a\"\n}";
    let policy = config::model::parse_content(r#"{ "test.policy": "b" }"#, None, &config::model::ParseFilter::default()).model;
    let markers = summary(&unsupported_settings_markers(text, Target::Workspace, false, &registry(), &policy));
    assert_eq!(markers, vec![(2, 2, 2, 20, "This setting cannot be applied because it is configured in the system policy.".into(), true)]);
    let without_policy = summary(&unsupported_settings_markers(text, Target::Workspace, false, &registry(), &ConfigurationModel::default()));
    assert_eq!(without_policy[0].4, "This setting has an application scope and can only be set in the settings file from the Default profile.");
}
