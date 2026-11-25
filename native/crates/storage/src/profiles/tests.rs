//! The cases of platform/userDataProfile/test/common/userDataProfileService.test.ts and
//! electron-main/userDataProfileMainService.test.ts, on a temporary user data folder.

use std::path::PathBuf;

use serde_json::json;

use super::*;

struct Fixture {
    root: PathBuf,
    state: FileStorage,
    profiles: Profiles,
}

fn fixture(test: &str) -> Fixture {
    let root = std::env::temp_dir().join(format!("vsctests-profiles-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let state = FileStorage::open(&root.join("storage.json"));
    let profiles = Profiles::load(&state, &paths(&root));
    Fixture { root, state, profiles }
}

fn paths(root: &Path) -> Paths {
    Paths { user_home: root.join("User"), cache_home: root.to_path_buf(), extensions_dir: None }
}

fn transient() -> Options {
    Options { transient: true, ..Options::default() }
}

fn flags(resource: &str) -> Options {
    Options { use_default_flags: Some(UseDefaultFlags::from([(resource.to_string(), true)])), ..Options::default() }
}

fn workspace(path: &str) -> WorkspaceId {
    WorkspaceId::Uri(format!("file://{path}"))
}

#[test]
fn default_profile() {
    let Fixture { root, profiles, .. } = fixture("default");
    let default = profiles.default_profile();
    let user = root.join("User");
    assert!(default.is_default);
    assert_eq!(default.use_default_flags, None);
    assert_eq!(default.location, user);
    assert_eq!(default.global_storage_home, user.join("globalStorage"));
    assert_eq!(default.keybindings_resource, user.join("keybindings.json"));
    assert_eq!(default.settings_resource, user.join("settings.json"));
    assert_eq!(default.snippets_home, user.join("snippets"));
    assert_eq!(default.tasks_resource, user.join("tasks.json"));
    assert_eq!(default.extensions_resource, user.join("extensions.json"));
    assert_eq!(profiles.profiles().len(), 1);
}

#[test]
fn create_profile_with_id_and_transient() {
    let Fixture { mut state, mut profiles, root } = fixture("create");
    let (profile, change) = profiles.create_profile("id", "name", Options::default(), None, &mut state).unwrap();
    assert_eq!((profile.id.as_str(), profile.name.as_str(), profile.is_transient), ("id", "name", false));
    assert_eq!(change.added, [profile.clone()]);
    assert_eq!(profile.location, root.join("User/profiles/id"));
    assert!(profile.location.is_dir());
    assert_eq!(profile.cache_home, root.join("CachedProfilesData/id"));
    let (temporary, _) = profiles.create_profile("id2", "name2", transient(), None, &mut state).unwrap();
    assert!(temporary.is_transient);
    let ids: Vec<String> = profiles.profiles().into_iter().map(|profile| profile.id).collect();
    assert_eq!(ids, [DEFAULT_PROFILE_ID, "id", "id2"]);
    assert_eq!(profiles.create_profile("id3", "name", Options::default(), None, &mut state).unwrap_err(), "Profile with name name already exists");
}

#[test]
fn create_transient_profiles() {
    let Fixture { mut state, mut profiles, .. } = fixture("transient");
    let names: Vec<String> = (0..3).map(|_| profiles.create_transient_profile(None, &mut state).unwrap().0.name).collect();
    assert_eq!(names, ["Temp 1", "Temp 2", "Temp 3"]);
    profiles.create_profile("id", "name", transient(), None, &mut state).unwrap();
    assert_eq!(profiles.profiles().len(), 5);
    assert!(profiles.profiles()[1..].iter().all(|profile| profile.is_transient));
}

#[test]
fn create_transient_profile_when_a_normal_profile_with_temp_is_already_created() {
    let Fixture { mut state, mut profiles, .. } = fixture("temp-named");
    profiles.create_named_profile("Temp 1", Options::default(), None, &mut state).unwrap();
    let (profile, _) = profiles.create_transient_profile(None, &mut state).unwrap();
    assert_eq!(profile.name, "Temp 2");
    assert!(profile.is_transient);
}

#[test]
fn removing_a_transient_profile_leaves_the_default_profile() {
    let Fixture { mut state, mut profiles, .. } = fixture("remove");
    let (profile, _) = profiles.create_transient_profile(None, &mut state).unwrap();
    profiles.remove_profile(&profile.id, &mut state).unwrap();
    assert_eq!(profiles.profiles().len(), 1);
    assert!(profiles.profiles()[0].is_default);
    assert_eq!(profiles.remove_profile(DEFAULT_PROFILE_ID, &mut state).unwrap_err(), "Cannot remove default profile");
}

#[test]
fn update_named_profile_and_persist_transient_profiles() {
    let Fixture { mut state, mut profiles, .. } = fixture("update");
    let (named, _) = profiles.create_named_profile("name", Options::default(), None, &mut state).unwrap();
    profiles.update_profile(&named.id, UpdateOptions { name: Some("name changed".into()), ..UpdateOptions::default() }, &mut state).unwrap();
    assert_eq!(profiles.profiles()[1].name, "name changed");
    assert_eq!(profiles.profiles()[1].id, named.id);

    let (temporary, _) = profiles.create_transient_profile(None, &mut state).unwrap();
    profiles.update_profile(&temporary.id, UpdateOptions { name: Some("saved".into()), ..UpdateOptions::default() }, &mut state).unwrap();
    assert!(profiles.profiles()[2].is_transient, "save transient profile keeps it transient");
    let persist = UpdateOptions { name: Some("saved".into()), transient: Some(false), ..UpdateOptions::default() };
    profiles.update_profile(&temporary.id, persist, &mut state).unwrap();
    let saved = &profiles.profiles()[2];
    assert_eq!((saved.id.as_str(), saved.name.as_str(), saved.is_transient), (temporary.id.as_str(), "saved", false));
}

#[test]
fn profiles_using_the_default_profile_resources() {
    let Fixture { mut state, mut profiles, .. } = fixture("flags");
    let default = profiles.default_profile().clone();
    let cases: [(&str, fn(&Profile) -> &PathBuf); 7] = [
        ("settings", |profile| &profile.settings_resource),
        ("keybindings", |profile| &profile.keybindings_resource),
        ("snippets", |profile| &profile.snippets_home),
        ("tasks", |profile| &profile.tasks_resource),
        ("globalState", |profile| &profile.global_storage_home),
        ("extensions", |profile| &profile.extensions_resource),
        ("languageModels", |profile| &profile.language_models_resource),
    ];
    for (index, (resource, path)) in cases.into_iter().enumerate() {
        let (profile, _) = profiles.create_named_profile(&format!("name{index}"), flags(resource), None, &mut state).unwrap();
        assert!(!profile.is_default);
        assert_eq!(profile.use_default_flags, flags(resource).use_default_flags);
        assert_eq!(path(&profile), path(&default), "{resource}");
        assert_ne!(profile.settings_resource == default.settings_resource, resource != "settings", "only {resource} is shared");
    }
    let (plain, _) = profiles.create_named_profile("plain", Options::default(), None, &mut state).unwrap();
    let (updated, _) = profiles.update_profile(&plain.id, UpdateOptions { use_default_flags: flags("keybindings").use_default_flags, ..UpdateOptions::default() }, &mut state).unwrap();
    assert_eq!(updated.keybindings_resource, default.keybindings_resource);
}

#[test]
fn workspace_associations() {
    let Fixture { mut state, mut profiles, .. } = fixture("workspaces");
    let first = workspace("/workspace1");
    let uri = "file:///workspace1".to_string();
    let (profile1, _) = profiles.create_profile("id", "name", Options::default(), Some(&first), &mut state).unwrap();
    assert_eq!(profile1.workspaces, Some(vec![uri.clone()]));
    assert_eq!(profiles.set_profile_for_workspace(&first, "id", &mut state).unwrap(), Change::default(), "no duplicate");

    profiles.create_profile("id1", "name1", Options::default(), None, &mut state).unwrap();
    let change = profiles.set_profile_for_workspace(&first, "id1", &mut state).unwrap();
    let updated: Vec<(String, Option<Vec<String>>)> = change.updated.into_iter().map(|profile| (profile.id, profile.workspaces)).collect();
    assert_eq!(updated, [("id".into(), None), ("id1".into(), Some(vec![uri.clone()]))]);
    assert_eq!(profiles.profile_for_workspace(&first).map(|profile| profile.id.as_str()), Some("id1"));

    let change = profiles.unset_workspace(&first, false, &mut state);
    assert_eq!(change.updated.len(), 1);
    assert_eq!(change.updated[0].workspaces, None);
    assert_eq!(profiles.profile_for_workspace(&first), None);
}

#[test]
fn update_profile_workspaces() {
    let Fixture { mut state, mut profiles, .. } = fixture("update-workspaces");
    let (profile, _) = profiles.create_named_profile("name", Options::default(), None, &mut state).unwrap();
    let set = |workspaces: &[&str]| UpdateOptions { workspaces: Some(workspaces.iter().map(|workspace| workspace.to_string()).collect()), ..UpdateOptions::default() };
    let (profile, _) = profiles.update_profile(&profile.id, set(&["file:///workspace1"]), &mut state).unwrap();
    assert_eq!(profile.workspaces, Some(vec!["file:///workspace1".to_string()]));
    let (profile, _) = profiles.update_profile(&profile.id, set(&["file:///workspace2"]), &mut state).unwrap();
    assert_eq!(profile.workspaces, Some(vec!["file:///workspace2".to_string()]));
    let (profile, _) = profiles.update_profile(&profile.id, set(&[]), &mut state).unwrap();
    assert_eq!(profile.workspaces, None);

    profiles.update_profile(DEFAULT_PROFILE_ID, set(&["file:///workspace1"]), &mut state).unwrap();
    assert_eq!(profiles.profiles().len(), 2);
    assert!(profiles.default_profile().is_default);
    assert_eq!(profiles.default_profile().workspaces, Some(vec!["file:///workspace1".to_string()]));
}

#[test]
fn can_create_transient_and_persistent_profiles_with_same_name() {
    let Fixture { mut state, mut profiles, .. } = fixture("same-name");
    let (profile1, _) = profiles.create_named_profile("name", transient(), None, &mut state).unwrap();
    let (profile2, _) = profiles.create_named_profile("name", transient(), None, &mut state).unwrap();
    let (profile3, _) = profiles.create_named_profile("name", Options::default(), None, &mut state).unwrap();
    let ids: Vec<String> = profiles.profiles().into_iter().skip(1).map(|profile| profile.id).collect();
    assert_eq!(ids, [profile3.id, profile1.id, profile2.id]);
}

#[test]
fn profiles_and_associations_survive_a_restart_in_upstream_s_storage_json_shape() {
    let Fixture { root, mut state, mut profiles } = fixture("restart");
    let options = Options { icon: Some("rocket".into()), ..flags("settings") };
    profiles.create_profile("abc", "Work", options, Some(&workspace("/w")), &mut state).unwrap();
    profiles.create_transient_profile(None, &mut state).unwrap();
    profiles.set_profile_for_workspace(&WorkspaceId::EmptyWindow("1234".into()), "abc", &mut state).unwrap();

    let stored: serde_json::Value = serde_json::from_str(&fs::read_to_string(root.join("storage.json")).unwrap()).unwrap();
    assert_eq!(stored["userDataProfiles"], json!([{ "location": "abc", "name": "Work", "icon": "rocket", "useDefaultFlags": { "settings": true } }]));
    assert_eq!(stored["profileAssociations"], json!({ "workspaces": { "file:///w": "abc" }, "emptyWindows": { "1234": "abc" } }));

    let reloaded = Profiles::load(&FileStorage::open(&root.join("storage.json")), &paths(&root));
    let restored = &reloaded.profiles()[1];
    assert_eq!(reloaded.profiles().len(), 2, "transient profiles are not stored");
    assert_eq!((restored.id.as_str(), restored.name.as_str(), restored.icon.as_deref()), ("abc", "Work", Some("rocket")));
    assert_eq!(restored.settings_resource, root.join("User/settings.json"));
    assert_eq!(reloaded.profile_for_workspace(&WorkspaceId::EmptyWindow("1234".into())).map(|profile| profile.id.as_str()), Some("abc"));
}

#[test]
fn clean_up_removes_unused_folders_and_invalid_stored_profiles() {
    let Fixture { root, mut state, .. } = fixture("clean-up");
    state.set_item("userDataProfiles", Some(json!([{ "location": "kept", "name": "Kept" }, { "location": "x", "name": "" }])));
    let mut profiles = Profiles::load(&state, &paths(&root));
    for folder in ["kept", "stale", "builtin"] {
        fs::create_dir_all(root.join("User/profiles").join(folder)).unwrap();
    }
    assert_eq!(profiles.profiles().len(), 2, "the invalid profile is skipped");
    profiles.clean_up(&mut state);
    assert!(root.join("User/profiles/kept").is_dir());
    assert!(root.join("User/profiles/builtin").is_dir());
    assert!(!root.join("User/profiles/stale").exists());
    assert_eq!(state.get_item("userDataProfiles"), Some(&json!([{ "location": "kept", "name": "Kept" }])));
}

#[test]
fn hash_hex_matches_hash_ts() {
    assert_eq!(hash_hex(""), format!("{:x}", 149417));
    assert_eq!(hash_hex("a"), format!("{:x}", 149417 * 31 + 97));
    assert!(new_profile_id().len() <= 9);
}
