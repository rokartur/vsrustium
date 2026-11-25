use config::registry::Registry;
use config::service::Target;
use serde_json::json;

use super::{fix_setting_links, raw_default_settings, registered_groups};

fn registry() -> Registry {
    let mut registry = Registry::default();
    let contribution = json!({ "contributes": { "configuration": { "title": "Sample", "properties": {
        "sample.b": { "type": "string", "default": "x", "description": "Second.\nSee `#sample.a#`.", "scope": "resource" },
        "sample.a": { "type": "string", "enum": ["one", "two"], "enumDescriptions": ["First one", ""], "default": "one", "markdownDescription": "First.", "scope": "application" },
        "sample.c": { "type": "object", "default": { "size": 2.0 }, "description": "Third.", "deprecationMessage": "Gone." }
    } } } });
    registry.register_configuration("pub.sample", &contribution);
    registry.finish();
    registry
}

#[test]
fn the_raw_default_settings_list_each_setting_with_its_comments_sorted_by_key() {
    let expected = [
        "{",
        "\t// Sample",
        "\t// First.",
        "\t//  - one: First one",
        "\t//  - two",
        "\t\"sample.a\": \"one\",",
        "",
        "\t// Second.",
        "\t// See `sample.a`.",
        "\t\"sample.b\": \"x\",",
        "",
        "\t// Gone.",
        "\t// Third.",
        "\t\"sample.c\": {",
        "\t\t\"size\": 2",
        "\t},",
        "",
        "}",
    ];
    assert_eq!(raw_default_settings(&registry()), expected.join("\n"));
}

/// preferencesModels.test.ts `DefaultSettings`, through the extension point: it titles an untitled node with the
/// extension's name (configurationExtensionPoint.ts `handleConfiguration`), so "merges untitled group into titled
/// group if id matches" has no extension counterpart.
#[test]
fn default_settings_group_cases() {
    let groups_of = |contributions: &[(&str, serde_json::Value)]| {
        let mut registry = Registry::default();
        for (extension, nodes) in contributions {
            registry.register_configuration(extension, &json!({ "contributes": { "configuration": nodes } }));
        }
        registry.finish();
        registered_groups(&registry, Target::UserLocal)
    };
    let node = |id: &str, title: Option<&str>, key: &str| {
        let mut node = json!({ "id": id, "properties": { key: { "type": "string", "default": "value" } } });
        if let Some(title) = title {
            node["title"] = json!(title);
        }
        node
    };
    let summary = |groups: Vec<super::SettingsGroup>| -> Vec<(String, String, Vec<String>)> {
        groups.into_iter().map(|group| (group.id, group.title, group.settings.into_iter().map(|setting| setting.key).collect())).collect()
    };
    let strings = |items: &[&str]| items.iter().map(|item| item.to_string()).collect::<Vec<_>>();

    let deprecated = json!({ "id": "deprecatedSettings", "title": "Deprecated Settings", "properties": { "test.deprecatedSetting": {
        "type": "boolean", "default": false, "deprecationMessage": "Use the replacement setting.", "deprecationMessageSeverity": "info", "deprecationMessageShowInSettings": true
    } } });
    let setting = &groups_of(&[("test.extension", json!([deprecated]))])[0].settings[0];
    assert_eq!((setting.deprecation_is_info, setting.deprecation_show_in_settings), (true, true), "preserves deprecation message presentation");

    let groups = groups_of(&[("test.extension", json!([node("config1", Some("Group 1"), "test.setting1"), node("config2", Some("Group 2"), "test.setting2")]))]);
    assert_eq!(summary(groups), [("config1".into(), "Group 1".into(), strings(&["test.setting1"])), ("config2".into(), "Group 2".into(), strings(&["test.setting2"]))], "by title");

    let groups = groups_of(&[("test.extension", json!([node("group1", None, "test.setting1"), node("group1", None, "test.setting2")]))]);
    assert_eq!(summary(groups), [("group1".into(), "test.extension".into(), strings(&["test.setting1", "test.setting2"]))], "by id without a title");

    let groups = groups_of(&[("test.extension", json!([node("group1", Some("Title 1"), "test.setting1"), node("group1", Some("Title 2"), "test.setting2")]))]);
    assert_eq!(groups.iter().map(|group| group.title.as_str()).collect::<Vec<_>>(), ["Title 1", "Title 2"], "same id, different titles");

    for title in [Some("Title 1"), None] {
        let groups = groups_of(&[("test.extension1", json!([node("group1", title, "test.setting1")])), ("test.extension2", json!([node("group1", title, "test.setting2")]))]);
        let extensions: Vec<_> = groups.iter().map(|group| group.extension_id.as_deref()).collect();
        assert_eq!(extensions, [Some("test.extension1"), Some("test.extension2")], "same id and title {title:?}, different extensions");
    }

    let groups = groups_of(&[("test.extension", json!([node("test.extension", Some("Group 1"), "test.setting1"), node("test.extension", Some("Group 2"), "test.setting2")]))]);
    assert_eq!(groups.iter().map(|group| group.title.as_str()).collect::<Vec<_>>(), ["Group 1", "Group 2"], "extension id as group id");

    let mut first = node("group1", Some("Group 1"), "test.setting1");
    first["order"] = json!(2);
    let mut second = node("group2", Some("Group 2"), "test.setting2");
    second["order"] = json!(1);
    let groups = groups_of(&[("test.extension", json!([first, second]))]);
    assert_eq!(groups.iter().map(|group| group.title.as_str()).collect::<Vec<_>>(), ["Group 2", "Group 1"], "sorts groups by order");
}

#[test]
fn folder_targets_keep_only_resource_scoped_settings() {
    let groups = registered_groups(&registry(), Target::WorkspaceFolder);
    let keys: Vec<_> = groups.iter().flat_map(|group| group.settings.iter().map(|setting| setting.key.as_str())).collect();
    assert_eq!(keys, ["sample.b"]);
}

#[test]
fn language_defaults_come_first_with_each_override_on_its_own_line() {
    let mut registry = Registry::default();
    let contribution = json!({ "contributes": {
        "configuration": { "title": "Sample", "properties": { "sample.a": { "type": "string", "default": "x", "description": "A." } } },
        "configurationDefaults": { "[lang]": { "editor.tabSize": 2.0, "editor.quickSuggestions": { "strings": "on" } } }
    } });
    registry.register_configuration("pub.sample", &contribution);
    registry.register_configuration_defaults("pub.sample", &contribution);
    registry.finish();
    let expected = [
        "{",
        "\t// Default Language Configuration Overrides",
        "\t// Configure settings to be overridden for lang.",
        "\t\"[lang]\":  {",
        "\t\t\"editor.tabSize\": 2,",
        "\t\t\"editor.quickSuggestions\": {",
        "\t\t\t\t\"strings\": \"on\"",
        "\t\t}",
        "\t},",
        "",
        "\t// Sample",
        "\t// A.",
        "\t\"sample.a\": \"x\",",
        "",
        "}",
    ];
    assert_eq!(raw_default_settings(&registry), expected.join("\n"));
}

#[test]
fn setting_links_lose_their_hashes_only_when_closed() {
    assert_eq!(fix_setting_links("a `#b.c#` and `#d`"), "a `b.c` and `#d`");
}
