use serde_json::{Value, json};

use super::{RESOURCE_LANGUAGE_SETTINGS_SCHEMA_ID, compressed_content, keybindings_schema, settings_schemas};
use crate::registry::Registry;

/// keybindingService.ts `updateSchema`: an extension's command is a command name and a removal; the code action
/// commands suggest the provided kinds of their family.
#[test]
fn the_keybindings_schema_adds_extension_commands_and_code_action_kinds() {
    let description = json!({ "contributes": { "commands": [{ "command": "port.hello", "title": "Hello" }] } });
    let mut registry = Registry::with_extensions(&[("port.ext", &description)]);
    registry.code_action_kinds = vec!["quickfix".into(), "refactor.extract".into(), "source.organizeImports".into()];
    let schema = keybindings_schema(&registry);
    let definitions = &schema["definitions"];
    assert!(definitions["commandNames"]["enum"].as_array().unwrap().contains(&json!("port.hello")));
    assert!(definitions["commandType"]["anyOf"][1]["enum"].as_array().unwrap().contains(&json!("-port.hello")));
    let kinds = |command: &str| {
        let all_of = definitions["commandsSchemas"]["allOf"].as_array().unwrap();
        let addition = all_of.iter().rev().find(|entry| entry["if"]["properties"]["command"]["const"] == command).unwrap();
        addition["then"]["properties"]["args"]["properties"]["kind"]["anyOf"][0]["enum"].clone()
    };
    assert_eq!(kinds("editor.action.codeAction"), json!(["quickfix", "refactor.extract", "source.organizeImports"]));
    assert_eq!(kinds("editor.action.refactor"), json!(["refactor.extract"]));
    assert_eq!(kinds("editor.action.sourceAction"), json!(["source.organizeImports"]));
}

/// The five `getCompressedContent` cases of base/test/common/jsonSchema.test.ts, as JSON.
const CASES: &str = include_str!("compressed-content-cases.json");

#[test]
fn compressed_content_matches_json_schema_test() {
    let cases: Vec<Value> = serde_json::from_str(CASES).unwrap();
    assert_eq!(cases.len(), 5);
    for (index, case) in cases.iter().enumerate() {
        assert_eq!(compressed_content(&case["schema"]), case["expected"].as_str().unwrap(), "getCompressedContent {}", index + 1);
    }
}

#[test]
fn settings_schemas_split_the_registry_by_scope() {
    let registry = Registry::with_extensions(&[]);
    let schemas = settings_schemas(&registry, false);
    let schema = |id: &str| schemas.iter().find(|(schema_id, _)| *schema_id == id).map(|(_, schema)| schema).unwrap();
    let has = |id: &str, key: &str| schema(id)["properties"].get(key).is_some();

    // window.restoreWindows is application, window.zoomLevel window, workbench.localHistory.enabled resource and
    // editor.tabSize language-overridable.
    assert!(has("vscode://schemas/settings/user", "window.restoreWindows"));
    assert!(!has("vscode://schemas/settings/workspace", "window.restoreWindows"));
    assert!(has("vscode://schemas/settings/workspace", "window.zoomLevel"));
    assert!(has("vscode://schemas/settings/folder", "workbench.localHistory.enabled"));
    assert!(has("vscode://schemas/settings/folder", "editor.tabSize"));
    assert!(has(RESOURCE_LANGUAGE_SETTINGS_SCHEMA_ID, "editor.tabSize"));
    assert!(!has(RESOURCE_LANGUAGE_SETTINGS_SCHEMA_ID, "workbench.localHistory.enabled"));
    assert_eq!(schema("vscode://schemas/settings/user")["properties"]["editor.tabSize"]["default"], json!(4));
    assert_eq!(schema("vscode://schemas/settings/user")["properties"]["[yaml]"]["$ref"], json!(RESOURCE_LANGUAGE_SETTINGS_SCHEMA_ID));
    assert_eq!(schema("vscode://schemas/settings/user")["patternProperties"][r"^(\[([^\]]+)\])+$"]["$ref"], json!(RESOURCE_LANGUAGE_SETTINGS_SCHEMA_ID));

    let multi_root = settings_schemas(&registry, true);
    let folder = &multi_root.iter().find(|(id, _)| *id == "vscode://schemas/settings/folder").unwrap().1;
    assert!(folder["properties"].get("window.zoomLevel").is_none(), "a multi-root folder takes resource settings only");
}

#[test]
fn each_registered_language_gets_an_override_property() {
    let rust = json!({ "contributes": { "languages": [{ "id": "rust" }] } });
    let registry = Registry::with_extensions(&[("vscode.rust", &rust)]);
    let schemas = settings_schemas(&registry, false);
    let user = &schemas.iter().find(|(id, _)| *id == "vscode://schemas/settings/user").unwrap().1;
    assert_eq!(user["properties"]["[rust]"]["$ref"], json!(RESOURCE_LANGUAGE_SETTINGS_SCHEMA_ID));
    assert_eq!(user["properties"]["[rust]"]["default"], json!({}));
    assert_eq!(user["properties"]["[plaintext]"]["$ref"], json!(RESOURCE_LANGUAGE_SETTINGS_SCHEMA_ID));
}
