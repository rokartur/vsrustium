use std::process::Command;

use config::registry::Registry;
use serde_json::json;

use super::read_native_policies;

const DOMAIN: &str = "com.rokartur.vsrustium.policy-tests";

fn defaults(args: &[&str]) {
    assert!(Command::new("defaults").args(args).status().unwrap().success(), "defaults {args:?}");
}

#[test]
fn policies_read_from_the_bundle_preferences_by_their_setting_type() {
    let mut registry = Registry::default();
    let policy = |name: &str| json!({ "name": name });
    registry.register_configuration(
        "test",
        &json!({ "contributes": { "configuration": { "properties": {
            "policy.string": { "type": "string", "default": "", "policy": policy("PolicyString") },
            "policy.number": { "type": "number", "default": 0, "policy": policy("PolicyNumber") },
            "policy.boolean": { "type": "boolean", "default": true, "policy": policy("PolicyBoolean") },
            "policy.object": { "type": "object", "default": {}, "policy": policy("PolicyObject") },
            "policy.mismatch": { "type": "boolean", "default": true, "policy": policy("PolicyMismatch") },
            "policy.unset": { "type": "string", "default": "", "policy": policy("PolicyUnset") },
        } } } }),
    );
    defaults(&["write", DOMAIN, "PolicyString", "-string", "managed"]);
    defaults(&["write", DOMAIN, "PolicyNumber", "-int", "42"]);
    defaults(&["write", DOMAIN, "PolicyBoolean", "-bool", "false"]);
    defaults(&["write", DOMAIN, "PolicyObject", "-string", r#"{"a": true}"#]);
    defaults(&["write", DOMAIN, "PolicyMismatch", "-string", "yes"]);

    let values = read_native_policies(&registry, DOMAIN);
    defaults(&["delete", DOMAIN]);

    assert_eq!(
        serde_json::to_value(&values).unwrap(),
        json!({ "PolicyBoolean": false, "PolicyNumber": 42, "PolicyObject": r#"{"a": true}"#, "PolicyString": "managed" })
    );
}
