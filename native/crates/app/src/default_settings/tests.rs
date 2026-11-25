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
