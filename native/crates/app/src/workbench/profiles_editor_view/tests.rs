use std::collections::BTreeSet;

use serde_json::json;

use super::{copied_template, copy_flags};

#[test]
fn a_template_copies_what_it_has_and_a_profile_everything() {
    let template = json!({ "name": "T", "settings": "{}", "extensions": "[]" });
    assert_eq!(copy_flags(None, Some(&template)), BTreeSet::from(["settings", "extensions"]));
    assert_eq!(copy_flags(Some("id"), None).len(), 6);
    assert!(copy_flags(None, None).is_empty());
}

#[test]
fn the_created_profile_keeps_only_copied_resources_not_taken_from_the_default_profile() {
    let template = json!({ "name": "T", "settings": "{}", "keybindings": "{}", "tasks": "{}", "globalState": "{}" });
    let copied = copied_template(template, &BTreeSet::from(["settings", "keybindings"]), &BTreeSet::from(["keybindings"]));
    assert_eq!(copied, json!({ "name": "T", "settings": "{}", "globalState": "{}" }));
}
