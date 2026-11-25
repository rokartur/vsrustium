use std::path::PathBuf;

use super::*;

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!("app-profiles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("folder")).unwrap();
        Fixture { root }
    }

    fn resolve(&self, force: Option<&str>, development: bool) -> ProfileState {
        let workspace = Workspace::folder(self.root.join("folder"));
        ProfileState::resolve(&self.root.join("data"), &self.root.join("data/extensions"), Some(&workspace), force, false, development)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_new_workspace_opens_in_the_default_profile_and_is_associated() {
    let fixture = Fixture::new("default");
    let state = fixture.resolve(None, false);
    assert!(state.current.is_default);
    assert_eq!(state.user_files(), UserFiles::default_profile(&fixture.root.join("data/User")));
    let stored: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(fixture.root.join("data/User/globalStorage/storage.json")).unwrap()).unwrap();
    let uri = crate::editor_history::file_uri_string(&fixture.root.join("folder"));
    assert_eq!(stored["profileAssociations"]["workspaces"][&uri], "__default__profile__");
}

#[test]
fn the_profile_flag_creates_the_profile_and_the_workspace_keeps_it() {
    let fixture = Fixture::new("forced");
    let state = fixture.resolve(Some("Work"), false);
    assert_eq!(state.current.name, "Work");
    let location = state.current.location.clone();
    assert_eq!(
        state.user_files(),
        UserFiles { settings: location.join("settings.json"), tasks: location.join("tasks.json"), mcp: location.join("mcp.json"), default_settings: Some(fixture.root.join("data/User/settings.json")) }
    );
    assert_eq!(fixture.resolve(None, false).current.id, state.current.id);
}

#[test]
fn a_window_with_no_association_opens_in_the_new_window_profile() {
    let fixture = Fixture::new("new-window");
    fixture.resolve(Some("Work"), true);
    std::fs::write(fixture.root.join("data/User/settings.json"), r#"{ "window.newWindowProfile": "Work" }"#).unwrap();
    assert_eq!(fixture.resolve(None, false).current.name, "Work");
}

#[test]
fn an_extension_development_window_does_not_change_the_association() {
    let fixture = Fixture::new("development");
    fixture.resolve(Some("Work"), true);
    assert!(fixture.resolve(None, false).current.is_default);
}
