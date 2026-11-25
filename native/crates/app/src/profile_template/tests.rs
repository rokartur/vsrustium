use serde_json::json;
use storage::profiles::{Options, to_profile};

use super::*;

fn registry() -> Registry {
    let package = json!({ "contributes": { "configuration": { "properties": {
        "test.window": { "type": "string" },
        "test.machine": { "type": "string", "scope": "machine" },
    } } } });
    Registry::with_extensions(&[("test.ext", &package)])
}

fn profile(root: &Path, id: &str) -> Profile {
    to_profile(id, id, &root.join(id), &root.join("cache"), &Options::default(), None)
}

#[test]
fn a_template_carries_the_profile_without_machine_settings_and_applies_to_another() {
    let root = std::env::temp_dir().join(format!("app-profile-template-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (from, to) = (profile(&root, "from"), profile(&root, "to"));
    let configuration = config::service::Settings::load(registry(), root.join("User"), None).configuration;
    write(&from.settings_resource, "{\n\t\"test.window\": \"a\",\n\t\"test.machine\": \"here\"\n}").unwrap();
    write(&from.keybindings_resource, "[{ \"key\": \"cmd+k\", \"command\": \"x\" }]").unwrap();
    write(&from.snippets_home.join("rust.json"), "{}").unwrap();
    let mut state = Storage::in_memory();
    state.set("user.key", "1".into());
    state.set("machine.key", "2".into());
    state.set(TARGET_KEY, json!({ "user.key": 0, "machine.key": 1 }).to_string());
    let extensions = [json!({ "identifier": { "id": "port.sample" }, "displayName": "Sample" })];

    let template = export_template(&from, "From", Flags::ALL, &registry(), &configuration, &state, &extensions);
    let settings: Value = serde_json::from_str(template["settings"].as_str().unwrap()).unwrap();
    assert_eq!(settings["settings"], "{\n\t\"test.window\": \"a\"\n}");
    assert_eq!(serde_json::from_str::<Value>(template["tasks"].as_str().unwrap()).unwrap(), json!({ "tasks": null }));
    assert_eq!(serde_json::from_str::<Value>(template["globalState"].as_str().unwrap()).unwrap(), json!({ "storage": { "user.key": "1" } }));
    let template = parse_template(&template.to_string()).unwrap();

    write(&to.settings_resource, "{ \"test.machine\": \"there\" }").unwrap();
    let mut target_state = Storage::in_memory();
    let listed = apply_template(&template, &to, &registry(), &configuration, &mut target_state).unwrap();
    let settings: Value = serde_json::from_str(&std::fs::read_to_string(&to.settings_resource).unwrap()).unwrap();
    assert_eq!(settings, json!({ "test.window": "a", "test.machine": "there" }));
    assert_eq!(std::fs::read_to_string(&to.keybindings_resource).unwrap(), "[{ \"key\": \"cmd+k\", \"command\": \"x\" }]");
    assert!(to.snippets_home.join("rust.json").is_file());
    assert!(!to.tasks_resource.exists());
    assert_eq!(target_state.get("user.key"), Some("1"));
    assert_eq!(listed, extensions);
    assert!(parse_template("{\"settings\": 1}").is_err());
    std::fs::remove_dir_all(&root).unwrap();
}
