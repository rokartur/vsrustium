//! Cases of extensionEnablementService.test.ts the port's states reach.

use serde_json::{Value, json};
use storage::Storage;

use super::*;

fn input<'a>(id: &str, manifest: &'a Value) -> EnablementInput<'a> {
    EnablementInput { identifier: Identifier { id: id.into(), uuid: None }, manifest, disabled_in_env: false, enabled_in_env: false, is_valid: true }
}

struct Fixture {
    profile: Storage,
    workspace: Storage,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture { profile: Storage::in_memory(), workspace: Storage::in_memory() }
    }

    fn enablement(&mut self) -> Enablement<'_> {
        Enablement { profile: &mut self.profile, workspace: Some(&mut self.workspace) }
    }
}

fn id(id: &str) -> Identifier {
    Identifier { id: id.into(), uuid: None }
}

#[test]
fn disable_an_extension_globally_then_again() {
    let mut fixture = Fixture::new();
    let manifest = json!({});
    let extensions = [input("pub.a", &manifest)];
    assert_eq!(fixture.enablement().set_enablement(&extensions, 0, EnablementState::DisabledGlobally), Ok(vec![0]));
    assert_eq!(fixture.enablement().set_enablement(&extensions, 0, EnablementState::DisabledGlobally), Ok(vec![]));
    assert_eq!(fixture.enablement().state(&extensions, 0), EnablementState::DisabledGlobally);
    assert_eq!(fixture.profile.get(DISABLED_EXTENSIONS_STORAGE_PATH), Some(r#"[{"id":"pub.a"}]"#));
}

#[test]
fn workspace_states_win_over_the_global_one() {
    let mut fixture = Fixture::new();
    let mut enablement = fixture.enablement();
    enablement.set_user_state(&id("pub.a"), EnablementState::DisabledWorkspace);
    assert_eq!(enablement.user_state(&id("pub.a")), EnablementState::DisabledWorkspace);
    enablement.set_user_state(&id("pub.a"), EnablementState::DisabledGlobally);
    assert_eq!(enablement.user_state(&id("pub.a")), EnablementState::DisabledGlobally);
    enablement.set_user_state(&id("pub.a"), EnablementState::EnabledWorkspace);
    assert_eq!(enablement.user_state(&id("pub.a")), EnablementState::EnabledWorkspace);
    // Disabled globally and enabled in the workspace: enabled here.
    assert!(enablement.is_disabled_globally(&id("pub.a")));
    enablement.set_user_state(&id("pub.a"), EnablementState::EnabledGlobally);
    assert_eq!(enablement.user_state(&id("pub.a")), EnablementState::EnabledGlobally);
    assert_eq!(fixture.workspace.get(ENABLED_EXTENSIONS_STORAGE_PATH), None);
    assert_eq!(fixture.profile.get(DISABLED_EXTENSIONS_STORAGE_PATH), None);
}

#[test]
fn state_of_multiple_extensions() {
    let mut fixture = Fixture::new();
    let manifest = json!({});
    let extensions: Vec<_> = ["pub.a", "pub.b", "pub.c", "pub.d", "pub.e"].iter().map(|name| input(name, &manifest)).collect();
    let mut enablement = fixture.enablement();
    for (index, state) in [EnablementState::DisabledGlobally, EnablementState::DisabledWorkspace, EnablementState::EnabledWorkspace, EnablementState::EnabledGlobally].into_iter().enumerate() {
        enablement.set_enablement(&extensions, index, state).unwrap();
    }
    let states: Vec<_> = (0..5).map(|index| enablement.state(&extensions, index)).collect();
    assert_eq!(states, [EnablementState::DisabledGlobally, EnablementState::DisabledWorkspace, EnablementState::EnabledWorkspace, EnablementState::EnabledGlobally, EnablementState::EnabledGlobally]);
}

#[test]
fn workspace_enablement_needs_a_workspace() {
    let mut profile = Storage::in_memory();
    let manifest = json!({});
    let mut enablement = Enablement { profile: &mut profile, workspace: None };
    assert_eq!(enablement.set_enablement(&[input("pub.a", &manifest)], 0, EnablementState::DisabledWorkspace), Err("No workspace.".into()));
}

#[test]
fn enabling_takes_dependencies_and_pack_members_along() {
    for key in ["extensionDependencies", "extensionPack"] {
        let mut fixture = Fixture::new();
        let (target, dependency) = (json!({ key: ["pub.b"] }), json!({}));
        let extensions = [input("pub.a", &target), input("pub.b", &dependency)];
        let mut enablement = fixture.enablement();
        enablement.set_user_state(&id("pub.b"), EnablementState::DisabledGlobally);
        enablement.set_user_state(&id("pub.a"), EnablementState::DisabledGlobally);
        assert_eq!(enablement.set_enablement(&extensions, 0, EnablementState::EnabledGlobally), Ok(vec![0, 1]));
        assert_eq!(enablement.state(&extensions, 1), EnablementState::EnabledGlobally);
    }
}

#[test]
fn disabled_dependency_disables_its_dependents() {
    let mut fixture = Fixture::new();
    let (dependency, dependent) = (json!({}), json!({ "extensionDependencies": ["pub.a"] }));
    let extensions = [input("pub.a", &dependency), input("pub.b", &dependent)];
    let mut enablement = fixture.enablement();
    enablement.set_enablement(&extensions, 0, EnablementState::DisabledWorkspace).unwrap();
    assert_eq!(enablement.state(&extensions, 1), EnablementState::DisabledByExtensionDependency);
    assert_eq!(enablement.cannot_change(&extensions, 1), Some("Cannot enable 'pub.b' extension because it depends on 'pub.a' extension that cannot be enabled".into()));
    // A missing dependency does not disable.
    assert_eq!(enablement.state(&extensions[1..], 0), EnablementState::EnabledGlobally);
}

#[test]
fn invalid_and_environment_states() {
    let mut fixture = Fixture::new();
    let manifest = json!({});
    let mut invalid = input("pub.a", &manifest);
    invalid.is_valid = false;
    let mut env_disabled = input("pub.b", &manifest);
    env_disabled.disabled_in_env = true;
    let mut env_enabled = input("pub.c", &manifest);
    env_enabled.enabled_in_env = true;
    let extensions = [invalid, env_disabled, env_enabled];
    let mut enablement = fixture.enablement();
    enablement.set_user_state(&id("pub.c"), EnablementState::DisabledGlobally);
    assert_eq!(enablement.state(&extensions, 0), EnablementState::DisabledByInvalidExtension);
    assert_eq!(enablement.state(&extensions, 1), EnablementState::DisabledByEnvironment);
    assert_eq!(enablement.state(&extensions, 2), EnablementState::EnabledByEnvironment);
    assert!(enablement.cannot_change(&extensions, 1).is_some());
    assert!(enablement.cannot_change(&extensions, 2).is_some());
}

#[test]
fn language_packs_cannot_change_enablement() {
    let mut fixture = Fixture::new();
    let manifest = json!({ "displayName": "German", "contributes": { "localizations": [{ "languageId": "de" }] } });
    assert_eq!(fixture.enablement().cannot_change(&[input("pub.a", &manifest)], 0), Some("Cannot change enablement of German extension because it contributes language packs.".into()));
}

#[test]
fn uninstall_resets_the_lists() {
    let mut fixture = Fixture::new();
    let mut enablement = fixture.enablement();
    enablement.set_user_state(&id("pub.a"), EnablementState::DisabledWorkspace);
    enablement.set_user_state(&id("pub.a"), EnablementState::DisabledGlobally);
    enablement.reset(&id("pub.a"));
    assert_eq!(enablement.user_state(&id("pub.a")), EnablementState::EnabledGlobally);
}

#[test]
fn identifiers_match_by_uuid_first() {
    let a = Identifier { id: "pub.a".into(), uuid: Some("1".into()) };
    assert!(a.same(&Identifier { id: "PUB.A".into(), uuid: None }));
    assert!(!a.same(&Identifier { id: "pub.a".into(), uuid: Some("2".into()) }));
    assert!(a.same(&Identifier { id: "pub.renamed".into(), uuid: Some("1".into()) }));
}
