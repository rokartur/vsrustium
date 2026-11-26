use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use config::workspace::Workspace;
use serde_json::{Value, json};
use storage::Storage;

use super::{STORAGE_KEY, Startup, WorkspaceTrust};

fn open(shared: Storage, workspace: Option<&Workspace>, workspace_storage: &mut Storage, empty_window_trusted: bool) -> WorkspaceTrust {
    WorkspaceTrust::new(Arc::new(Mutex::new(shared)), Startup { enabled: true, workspace, empty_window_trusted, workspace_storage })
}

fn shared_with(paths: &[&str]) -> Storage {
    let mut shared = Storage::in_memory();
    let entries: Vec<Value> = paths.iter().map(|path| json!({ "uri": { "$mid": 1, "path": path, "scheme": "file" }, "trusted": true })).collect();
    shared.set(STORAGE_KEY, json!({ "uriTrustInfo": entries }).to_string());
    shared
}

#[test]
fn disabled_trust_trusts_every_window() {
    let workspace = Workspace::folder(PathBuf::from("/Folder"));
    let trust = WorkspaceTrust::new(Arc::new(Mutex::new(Storage::in_memory())), Startup { enabled: false, workspace: Some(&workspace), empty_window_trusted: false, workspace_storage: &mut Storage::in_memory() });
    assert!(trust.is_trusted());
    assert_eq!(trust.uri_trust_info(Path::new("/Other")), (true, PathBuf::from("/Other")));
}

#[test]
fn empty_workspace_follows_the_empty_window_setting_then_its_memento() {
    let mut workspace_storage = Storage::in_memory();
    assert!(open(Storage::in_memory(), None, &mut workspace_storage, true).is_trusted());
    // initializeWorkspaceTrust saved the first answer, so the setting no longer decides.
    assert!(open(Storage::in_memory(), None, &mut workspace_storage, false).is_trusted());
    assert!(!open(Storage::in_memory(), None, &mut Storage::in_memory(), false).is_trusted());
}

#[test]
fn a_folder_is_trusted_by_itself_or_a_parent_ignoring_case() {
    let workspace = Workspace::folder(PathBuf::from("/Users/a/Projects/app"));
    assert!(!open(Storage::in_memory(), Some(&workspace), &mut Storage::in_memory(), true).is_trusted());
    let trust = open(shared_with(&["/users/a/projects", "/Users/a/Projects/app"]), Some(&workspace), &mut Storage::in_memory(), true);
    assert!(trust.is_trusted());
    // The longest trusted path wins.
    assert_eq!(trust.uri_trust_info(Path::new("/Users/a/Projects/app/src")), (true, PathBuf::from("/Users/a/Projects/app")));
    assert!(!trust.uri_trust_info(Path::new("/Users/a/Projects-old")).0);
}

#[test]
fn untrusted_and_unparsable_entries_are_dropped_on_load() {
    let workspace = Workspace::folder(PathBuf::from("/Folder"));
    let mut shared = Storage::in_memory();
    shared.set(STORAGE_KEY, json!({ "uriTrustInfo": [{ "uri": { "$mid": 1, "path": "/Folder", "scheme": "file" }, "trusted": false }] }).to_string());
    assert!(!open(shared, Some(&workspace), &mut Storage::in_memory(), true).is_trusted());
    let mut shared = Storage::in_memory();
    shared.set(STORAGE_KEY, "{".into());
    assert!(!open(shared, Some(&workspace), &mut Storage::in_memory(), true).is_trusted());
}

#[test]
fn trusting_a_folder_stores_its_uri_and_untrusting_removes_it() {
    let workspace = Workspace::folder(PathBuf::from("/Folder a"));
    let mut workspace_storage = Storage::in_memory();
    let mut trust = open(Storage::in_memory(), Some(&workspace), &mut workspace_storage, true);
    assert!(trust.set_workspace_trust(true, &mut workspace_storage, true));
    assert!(trust.is_trusted());
    let stored: Value = serde_json::from_str(trust.shared.lock().unwrap().get(STORAGE_KEY).unwrap()).unwrap();
    assert_eq!(stored, json!({ "uriTrustInfo": [{ "uri": { "$mid": 1, "fsPath": "/Folder a", "external": "file:///Folder%20a", "path": "/Folder a", "scheme": "file" }, "trusted": true }] }));
    // Trusting again changes nothing.
    assert!(!trust.set_workspace_trust(true, &mut workspace_storage, true));
    assert!(trust.set_workspace_trust(false, &mut workspace_storage, true));
    assert_eq!(trust.shared.lock().unwrap().get(STORAGE_KEY), Some(r#"{"uriTrustInfo":[]}"#));
}

#[test]
fn trusting_the_parent_folder_stores_the_parent() {
    let workspace = Workspace::folder(PathBuf::from("/Parent/Folder"));
    let mut workspace_storage = Storage::in_memory();
    let mut trust = open(Storage::in_memory(), Some(&workspace), &mut workspace_storage, true);
    assert_eq!(trust.parent_folder(), Some(Path::new("/Parent")));
    assert!(trust.set_parent_folder_trust(true, &mut workspace_storage, true));
    assert!(trust.is_trusted());
    assert_eq!(trust.trusted_paths().collect::<Vec<_>>(), vec![PathBuf::from("/Parent")]);
}

#[test]
fn a_workspace_file_needs_its_folders_and_itself_trusted() {
    let dir = std::env::temp_dir().join(format!("trust-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.code-workspace");
    std::fs::write(&file, r#"{ "folders": [{ "path": "/One" }, { "path": "/Two" }] }"#).unwrap();
    let workspace = Workspace::from_file(file);
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(!open(shared_with(&["/One", "/Two"]), Some(&workspace), &mut Storage::in_memory(), true).is_trusted());
    let dir = dir.to_string_lossy();
    let trust = open(shared_with(&["/One", "/Two", &dir]), Some(&workspace), &mut Storage::in_memory(), true);
    assert!(trust.is_trusted());
    assert_eq!(trust.parent_folder(), None);
}

#[test]
fn an_empty_window_keeps_its_choice_in_the_memento() {
    let mut workspace_storage = Storage::in_memory();
    let mut trust = open(Storage::in_memory(), None, &mut workspace_storage, true);
    assert!(trust.set_workspace_trust(false, &mut workspace_storage, true));
    let memento: Value = serde_json::from_str(workspace_storage.get("memento/workspaceTrust").unwrap()).unwrap();
    assert_eq!(memento, json!({ "acceptsOutOfWorkspaceFiles": false, "isEmptyWorkspaceTrusted": false }));
    assert!(!open(Storage::in_memory(), None, &mut workspace_storage, true).is_trusted());
}

#[test]
fn an_extension_under_development_is_never_disabled_by_trust() {
    let workspace = Workspace::folder(PathBuf::from("/Folder"));
    let extension = |description: Value| exthost::scan::ScannedExtension { id: "port.tests".into(), location: PathBuf::from("/Folder/tests"), description };
    assert!(super::disabled_when_untrusted(&extension(json!({ "main": "./extension.js" })), Some(&workspace), &Value::Null));
    assert!(!super::disabled_when_untrusted(&extension(json!({ "main": "./extension.js", "isUnderDevelopment": true })), Some(&workspace), &Value::Null));
}
