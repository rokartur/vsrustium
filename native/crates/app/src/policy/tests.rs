use std::process::Command;

use config::registry::Registry;
use serde_json::json;

use super::{read_native_policies, watch_policy_changes};

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

#[test]
fn a_file_written_under_the_watched_folder_reports_a_change_even_when_the_folder_came_later() {
    // FSEvents reports real paths: the temp dir sits behind the /var -> /private/var link.
    let root = std::env::temp_dir().canonicalize().unwrap().join(format!("policy-watch-{}", std::process::id()));
    let folder = root.join("Managed Preferences");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    watch_policy_changes(folder.to_str().unwrap(), move || {
        let _ = sender.send(());
    });
    std::thread::sleep(std::time::Duration::from_millis(500));
    std::fs::create_dir_all(folder.join("artur")).unwrap();
    std::fs::write(folder.join("artur/com.rokartur.vsrustium.plist"), "<plist/>").unwrap();
    let reported = receiver.recv_timeout(std::time::Duration::from_secs(10));
    std::fs::remove_dir_all(&root).unwrap();
    assert!(reported.is_ok(), "no FSEvents batch within 10s");
}
