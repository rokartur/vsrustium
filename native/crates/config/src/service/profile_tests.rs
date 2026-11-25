//! The `WorkspaceConfigurationService - Profiles` suite of configurationService.test.ts: a window in a profile with
//! its own settings reads the default profile's application settings and those applied to all profiles.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{APPLY_ALL_PROFILES_SETTING, Settings, Target, UserFiles};
use crate::configuration::Overrides;
use crate::registry::Registry;

const APPLICATION: &str = "configurationService.profiles.applicationSetting";
const TEST: &str = "configurationService.profiles.testSetting";
const APPLICATION_2: &str = "configurationService.profiles.applicationSetting2";
const TEST_2: &str = "configurationService.profiles.testSetting2";

struct Fixture {
    root: PathBuf,
    settings: Settings,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn registry(extra: Value) -> Registry {
    let mut properties = json!({
        APPLY_ALL_PROFILES_SETTING: { "type": "array", "default": [], "scope": "application" },
        APPLICATION: { "type": "string", "default": "isSet", "scope": "application" },
        TEST: { "type": "string", "default": "isSet" },
        APPLICATION_2: { "type": "string", "default": "isSet", "scope": "application" },
        TEST_2: { "type": "string", "default": "isSet" },
    });
    properties.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
    let mut registry = Registry::core();
    registry.register_configuration("test", &json!({ "contributes": { "configuration": { "id": "_test", "properties": properties } } }));
    registry.finish();
    registry
}

fn profile_files(user_home: &Path, id: &str) -> UserFiles {
    let location = user_home.join("profiles").join(id);
    UserFiles { settings: location.join("settings.json"), tasks: location.join("tasks.json"), mcp: location.join("mcp.json"), default_settings: Some(user_home.join("settings.json")) }
}

impl Fixture {
    /// The setup: the default profile and the current `temp` profile each set `applicationSetting2` and `testSetting2`.
    fn new(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!("config-profiles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let user_home = root.join("User");
        let fixture_files = profile_files(&user_home, "temp");
        write(&user_home.join("settings.json"), &json!({ APPLICATION_2: "applicationValue", TEST_2: "userValue" }));
        write(&fixture_files.settings, &json!({ APPLICATION_2: "profileValue", TEST_2: "profileValue" }));
        let settings = Settings::load_with_profile(registry(json!({})), user_home, fixture_files, None);
        Fixture { root, settings }
    }

    fn user_home(&self) -> PathBuf {
        self.root.join("User")
    }

    fn default_settings(&self) -> PathBuf {
        self.user_home().join("settings.json")
    }

    fn profile_settings(&self) -> PathBuf {
        self.settings.user_files().settings.clone()
    }

    fn value(&self, key: &str) -> Option<Value> {
        self.settings.get_value(Some(key), &Overrides::default())
    }

    /// `reloadConfiguration` after writing both settings files.
    fn write_both(&mut self, default: Value, profile: Value) {
        write(&self.default_settings(), &default);
        write(&self.profile_settings(), &profile);
        let files = self.settings.user_files().clone();
        self.settings.set_profile(files);
    }

    fn apply_to_all_profiles(&mut self, keys: &[&str]) -> Vec<String> {
        self.settings.update_value(APPLY_ALL_PROFILES_SETTING, Some(&json!(keys)), Target::UserLocal, None, &[]).unwrap().keys
    }
}

fn write(path: &Path, value: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, value.to_string()).unwrap();
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn initialize() {
    let fixture = Fixture::new("initialize");
    assert_eq!(fixture.value(APPLICATION_2), Some(json!("applicationValue")));
    assert_eq!(fixture.value(TEST_2), Some(json!("profileValue")));
}

#[test]
fn inspect() {
    let mut fixture = Fixture::new("inspect");
    let actual = fixture.settings.configuration.inspect(APPLICATION, &Overrides::default());
    assert_eq!((actual.default_value, actual.application.value, actual.user_local.value, actual.value), (Some(json!("isSet")), None, None, Some(json!("isSet"))));

    fixture.write_both(json!({ APPLICATION: "applicationValue" }), json!({ APPLICATION: "profileValue" }));
    let actual = fixture.settings.configuration.inspect(APPLICATION, &Overrides::default());
    // The profile's settings.json is parsed with PROFILE_SCOPES, so an application setting in it is dropped.
    assert_eq!((actual.application.value, actual.user_local.value, actual.value), (Some(json!("applicationValue")), None, Some(json!("applicationValue"))));

    fixture.write_both(json!({ TEST: "applicationValue" }), json!({ TEST: "profileValue" }));
    let actual = fixture.settings.configuration.inspect(TEST, &Overrides::default());
    assert_eq!((actual.application.value, actual.user_local.value, actual.value), (None, Some(json!("profileValue")), Some(json!("profileValue"))));
}

#[test]
fn update_application_scope_settings_writes_the_default_profile() {
    let mut fixture = Fixture::new("update-application");
    // `application-machine` in a manifest parses as WINDOW (`parseScope`), so only the application case is ported.
    fixture.settings.update_value_derived(APPLICATION, Some(&json!("applicationValue"))).unwrap();
    assert_eq!(read(&fixture.default_settings())[APPLICATION], "applicationValue");
    assert_eq!(read(&fixture.profile_settings()).get(APPLICATION), None);
    assert_eq!(fixture.value(APPLICATION), Some(json!("applicationValue")));
}

#[test]
fn update_normal_setting_writes_the_profile() {
    let mut fixture = Fixture::new("update-normal");
    fixture.settings.update_value_derived(TEST, Some(&json!("profileValue"))).unwrap();
    assert_eq!(read(&fixture.profile_settings()), json!({ TEST_2: "profileValue", APPLICATION_2: "profileValue", TEST: "profileValue" }));
    assert_eq!(fixture.value(TEST), Some(json!("profileValue")));
}

#[test]
fn registering_settings_after_init() {
    let mut fixture = Fixture::new("register-later");
    write(&fixture.default_settings(), &json!({ "configurationService.profiles.testSetting3": "defaultProfile", "configurationService.profiles.applicationSetting3": "defaultProfile" }));
    let files = fixture.settings.user_files().clone();
    fixture.settings.set_profile(files);
    let extra = json!({
        "configurationService.profiles.testSetting3": { "type": "string", "default": "isSet" },
        "configurationService.profiles.applicationSetting3": { "type": "string", "default": "isSet", "scope": "application" },
    });
    fixture.settings.set_registry(registry(extra));
    assert_eq!(fixture.value("configurationService.profiles.testSetting3"), Some(json!("isSet")));
    assert_eq!(fixture.value("configurationService.profiles.applicationSetting3"), Some(json!("defaultProfile")));
}

#[test]
fn non_registered_setting_is_not_read_from_the_default_profile() {
    let mut fixture = Fixture::new("non-registered");
    fixture.write_both(json!({ "configurationService.profiles.nonregistered": "defaultProfile" }), json!({}));
    assert_eq!(fixture.value("configurationService.profiles.nonregistered"), None);
}

#[test]
fn settings_applied_to_all_profiles() {
    let mut fixture = Fixture::new("apply-all");
    assert!(fixture.settings.is_applied_for_all_profiles(APPLICATION_2));
    assert!(!fixture.settings.is_applied_for_all_profiles(TEST_2));
    let changed = fixture.apply_to_all_profiles(&[TEST_2]);
    assert_eq!(changed, [APPLY_ALL_PROFILES_SETTING, TEST_2]);
    assert!(fixture.settings.is_applied_for_all_profiles(TEST_2));
    assert_eq!(fixture.value(TEST_2), Some(json!("userValue")));

    let user_home = fixture.user_home();
    let reloaded = Settings::load_with_profile(registry(json!({})), user_home.clone(), profile_files(&user_home, "temp"), None);
    assert_eq!(reloaded.get_value(Some(APPLICATION_2), &Overrides::default()), Some(json!("applicationValue")));
    assert_eq!(reloaded.get_value(Some(TEST_2), &Overrides::default()), Some(json!("userValue")));

    let change = fixture.settings.update_value(TEST_2, Some(&json!("updatedValue")), Target::UserLocal, None, &[]).unwrap();
    assert_eq!(change.keys, [TEST_2]);
    assert_eq!(read(&fixture.default_settings())[TEST_2], "updatedValue");
    assert_eq!(fixture.value(TEST_2), Some(json!("updatedValue")));
}

#[test]
fn changing_application_settings_in_a_profile_reports_only_application_settings() {
    let mut fixture = Fixture::new("application-change");
    fixture.write_both(json!({}), json!({}));
    write(&fixture.default_settings(), &json!({ APPLICATION: "applicationValue", TEST: "applicationValue" }));
    let change = fixture.settings.reload_changed_files().unwrap();
    assert_eq!(change.keys, [APPLICATION]);
    assert_eq!(fixture.value(TEST), Some(json!("isSet")));
}

#[test]
fn switch_profiles() {
    let mut fixture = Fixture::new("switch");
    fixture.write_both(json!({ APPLICATION: "applicationValue", TEST: "userValue" }), json!({ APPLICATION: "profileValue", TEST: "profileValue" }));
    let user_home = fixture.user_home();

    let change = fixture.settings.set_profile(UserFiles::default_profile(&user_home));
    assert_eq!(change.keys, [TEST]);
    assert_eq!((fixture.value(APPLICATION), fixture.value(TEST)), (Some(json!("applicationValue")), Some(json!("userValue"))));

    let custom2 = profile_files(&user_home, "custom2");
    write(&custom2.settings, &json!({ APPLICATION: "profileValue2", TEST: "profileValue2" }));
    let change = fixture.settings.set_profile(custom2);
    assert_eq!(change.keys, [TEST]);
    assert_eq!((fixture.value(APPLICATION), fixture.value(TEST)), (Some(json!("applicationValue")), Some(json!("profileValue2"))));
}

#[test]
fn switch_to_a_profile_using_the_default_profile_settings() {
    let mut fixture = Fixture::new("switch-use-default");
    fixture.write_both(json!({ APPLICATION: "applicationValue", TEST: "userValue" }), json!({ APPLICATION: "profileValue", TEST: "profileValue" }));
    // useDefaultFlags.settings: the profile's settings resource is the default profile's, read as the default profile.
    write(&fixture.default_settings(), &json!({ APPLICATION: "applicationValue2", TEST: "profileValue2" }));
    let change = fixture.settings.set_profile(UserFiles::default_profile(&fixture.user_home()));
    assert_eq!(change.keys, [APPLICATION, TEST]);
    assert_eq!((fixture.value(APPLICATION), fixture.value(TEST)), (Some(json!("applicationValue2")), Some(json!("profileValue2"))));
}

#[test]
fn switch_with_settings_applied_to_all_profiles() {
    let mut fixture = Fixture::new("switch-apply-all");
    fixture.apply_to_all_profiles(&[TEST_2]);
    let user_home = fixture.user_home();
    fixture.settings.set_profile(UserFiles::default_profile(&user_home));
    assert_eq!((fixture.value(APPLICATION_2), fixture.value(TEST_2)), (Some(json!("applicationValue")), Some(json!("userValue"))));

    let custom2 = profile_files(&user_home, "custom2");
    write(&custom2.settings, &json!({ TEST: "profileValue", TEST_2: "profileValue2" }));
    let change = fixture.settings.set_profile(custom2);
    assert_eq!(change.keys, [TEST]);
    assert_eq!((fixture.value(APPLICATION_2), fixture.value(TEST_2), fixture.value(TEST)), (Some(json!("applicationValue")), Some(json!("userValue")), Some(json!("profileValue"))));
}
