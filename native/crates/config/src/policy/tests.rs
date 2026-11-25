use serde_json::{Value, json};

use super::{policy_model, read_policy_file};
use crate::model::ConfigurationModel;
use crate::registry::Registry;

/// policyConfiguration.test.ts `policyConfigurationNode`.
fn registry() -> Registry {
    let policy = |name: &str| json!({ "name": name, "category": "Extensions", "minimumVersion": "1.0.0" });
    let mut registry = Registry::default();
    registry.register_configuration(
        "policyConfiguration",
        &json!({ "contributes": { "configuration": { "id": "policyConfiguration", "title": "a", "properties": {
            "policy.settingA": { "type": "string", "default": "defaultValueA", "policy": policy("PolicySettingA") },
            "policy.settingB": { "type": "string", "default": "defaultValueB", "policy": policy("PolicySettingB") },
            "policy.objectSetting": { "type": "object", "default": {}, "policy": policy("PolicyObjectSetting") },
            "policy.arraySetting": { "type": "object", "default": [], "policy": policy("PolicyArraySetting") },
            "policy.booleanSetting": { "type": "boolean", "default": true, "policy": policy("PolicyBooleanSetting") },
            "policy.internalSetting": { "type": "string", "default": "defaultInternalValue", "included": false, "policy": policy("PolicyInternalSetting") },
            "policy.ownerSetting": { "type": "boolean", "default": true, "policy": policy("PolicyShared") },
            "policy.referenceSetting": { "type": "boolean", "default": true, "policyReference": { "name": "PolicyShared" } },
            "policy.orphanReferenceSetting": { "type": "boolean", "default": true, "policyReference": { "name": "PolicyOrphanReference" } },
            "nonPolicy.setting": { "type": "boolean", "default": true },
        } } } }),
    );
    registry
}

/// Writes the policy file and reads it back as FilePolicyService does, then builds the policy layer.
fn model_for_file(registry: &Registry, content: Value) -> ConfigurationModel {
    let dir = std::env::temp_dir().join(format!("config-policy-{}-{:?}", std::process::id(), std::thread::current().id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("policy.json");
    std::fs::write(&file, content.to_string()).unwrap();
    let values = read_policy_file(&file, registry).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    policy_model(registry, &values)
}

fn keys(model: &ConfigurationModel) -> Vec<String> {
    let mut keys = model.keys.clone();
    keys.sort();
    keys
}

#[test]
fn with_policies() {
    let model = model_for_file(&registry(), json!({ "PolicySettingA": "policyValueA" }));
    assert_eq!(model.get_value(Some("policy.settingA")), Some(json!("policyValueA")));
    assert_eq!(model.get_value(Some("policy.settingB")), None);
    assert_eq!(model.get_value(Some("nonPolicy.setting")), None);
    assert_eq!(keys(&model), ["policy.settingA"]);
    assert!(model.overrides.is_empty());
}

#[test]
fn no_policies() {
    let registry = registry();
    let missing = std::env::temp_dir().join("config-policy-missing/policy.json");
    let model = policy_model(&registry, &read_policy_file(&missing, &registry).unwrap());
    assert!(model.keys.is_empty());
    assert!(model.overrides.is_empty());
}

#[test]
fn with_policies_but_not_registered() {
    let model = model_for_file(&registry(), json!({ "PolicySettingA": "policyValueA", "PolicySettingB": "policyValueB", "PolicySettingC": "policyValueC" }));
    assert_eq!(model.get_value(Some("policy.settingA")), Some(json!("policyValueA")));
    assert_eq!(model.get_value(Some("policy.settingB")), Some(json!("policyValueB")));
    assert_eq!(keys(&model), ["policy.settingA", "policy.settingB"]);
}

#[test]
fn object_type_policy_is_parsed_from_its_string() {
    let expected = json!({ "microsoft": true, "github": "stable", "other": 1, "complex": { "key": "value" }, "array": [1, 2, 3] });
    let model = model_for_file(&registry(), json!({ "PolicyObjectSetting": expected.to_string() }));
    assert_eq!(model.get_value(Some("policy.objectSetting")), Some(expected));
}

#[test]
fn object_type_policy_keeps_proto_as_own_data() {
    let model = model_for_file(&registry(), json!({ "PolicyObjectSetting": r#"{ "__proto__": { "polluted": true } }"# }));
    assert_eq!(model.get_value(Some("policy.objectSetting")), Some(json!({ "__proto__": { "polluted": true } })));
}

#[test]
fn array_and_boolean_type_policies() {
    let model = model_for_file(&registry(), json!({ "PolicyArraySetting": "[1]", "PolicyBooleanSetting": false }));
    assert_eq!(model.get_value(Some("policy.arraySetting")), Some(json!([1])));
    assert_eq!(model.get_value(Some("policy.booleanSetting")), Some(json!(false)));
    let model = model_for_file(&registry(), json!({ "PolicyBooleanSetting": true }));
    assert_eq!(model.get_value(Some("policy.booleanSetting")), Some(json!(true)));
}

#[test]
fn object_type_policy_is_ignored_when_invalid_or_with_duplicate_keys() {
    let model = model_for_file(&registry(), json!({ "PolicyObjectSetting": r#"{"a": "b", "hello": }"# }));
    assert_eq!(model.get_value(Some("policy.objectSetting")), None);
    let model = model_for_file(&registry(), json!({ "PolicyObjectSetting": r#"{"microsoft": true, "microsoft": false }"# }));
    assert_eq!(model.get_value(Some("policy.objectSetting")), None);
}

#[test]
fn an_owning_policy_applies_to_the_owner_and_its_references() {
    let model = model_for_file(&registry(), json!({ "PolicyShared": false }));
    assert_eq!(model.get_value(Some("policy.ownerSetting")), Some(json!(false)));
    assert_eq!(model.get_value(Some("policy.referenceSetting")), Some(json!(false)));
    assert_eq!(keys(&model), ["policy.ownerSetting", "policy.referenceSetting"]);
}

#[test]
fn a_reference_resolves_without_its_owner() {
    let model = model_for_file(&registry(), json!({ "PolicyOrphanReference": false }));
    assert_eq!(model.get_value(Some("policy.orphanReferenceSetting")), Some(json!(false)));
    assert_eq!(keys(&model), ["policy.orphanReferenceSetting"]);
}

#[test]
fn nullable_boolean_reference_keeps_an_explicit_false() {
    let mut registry = registry();
    registry.register_configuration(
        "_test_nullable_policy_reference",
        &json!({ "contributes": { "configuration": { "properties": {
            "policy.nullableReference": { "type": ["boolean", "null"], "default": null, "policyReference": { "name": "PolicyShared" } },
        } } } }),
    );
    let model = model_for_file(&registry, json!({ "PolicyShared": false }));
    assert_eq!(model.get_value(Some("policy.nullableReference")), Some(json!(false)));
}

#[test]
fn internal_policies_apply() {
    let model = model_for_file(&registry(), json!({ "PolicyInternalSetting": "internalValue" }));
    assert_eq!(model.get_value(Some("policy.internalSetting")), Some(json!("internalValue")));
}

#[test]
fn a_policy_file_that_is_not_an_object_is_an_error() {
    let registry = registry();
    let dir = std::env::temp_dir().join(format!("config-policy-array-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("policy.json"), "[]").unwrap();
    assert_eq!(read_policy_file(&dir.join("policy.json"), &registry), Err("Policy file isn't a JSON object".to_string()));
    std::fs::remove_dir_all(dir).unwrap();
}
