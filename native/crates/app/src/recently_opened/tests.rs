use std::path::Path;

use serde_json::json;
use storage::Storage;

use super::*;

fn stored(shared: &Storage) -> Value {
    serde_json::from_str(shared.get(STORAGE_KEY).unwrap()).unwrap()
}

#[test]
fn added_entries_come_first_and_replace_stored_ones_ignoring_case() {
    let mut shared = Storage::in_memory();
    add(&mut shared, vec![entry(Kind::Folder, Path::new("/One"), None), entry(Kind::File, Path::new("/One/a.txt"), Some("~/a.txt".into()))]);
    add(&mut shared, vec![entry(Kind::Folder, Path::new("/Two"), None), entry(Kind::Folder, Path::new("/one"), None)]);
    let recent = load(&mut shared);
    let folders: Vec<&str> = recent.workspaces.iter().map(location).collect();
    assert_eq!(folders, ["file:///Two", "file:///one"]);
    assert_eq!(recent.files, [json!({ "fileUri": "file:///One/a.txt", "label": "~/a.txt" })]);
}

#[test]
fn a_new_entry_without_label_keeps_the_stored_label() {
    let mut shared = Storage::in_memory();
    add(&mut shared, vec![entry(Kind::File, Path::new("/a.txt"), Some("A".into()))]);
    add(&mut shared, vec![entry(Kind::File, Path::new("/a.txt"), None)]);
    assert_eq!(load(&mut shared).files, [json!({ "fileUri": "file:///a.txt", "label": "A" })]);
}

#[test]
fn labels_equal_to_the_path_and_git_files_are_not_stored() {
    let mut shared = Storage::in_memory();
    add(&mut shared, vec![entry(Kind::File, Path::new("/r/.git/COMMIT_EDITMSG"), None), entry(Kind::File, Path::new("/b c.txt"), Some("/b c.txt".into()))]);
    assert_eq!(stored(&shared), json!({ "entries": [{ "fileUri": "file:///b%20c.txt" }] }));
}

#[test]
fn remove_and_clear_keep_entries_of_other_schemes_until_cleared() {
    let mut shared = Storage::in_memory();
    let remote = json!({ "folderUri": "vscode-remote://ssh-remote%2Bhost/home", "remoteAuthority": "ssh-remote+host" });
    shared.set(STORAGE_KEY, json!({ "entries": [remote, { "folderUri": "file:///One" }, { "bogus": 1 }] }).to_string());
    remove(&mut shared, &["file:///ONE"]);
    assert_eq!(stored(&shared), json!({ "entries": [remote] }));
    clear(&mut shared);
    assert_eq!(stored(&shared), json!({ "entries": [] }));
}

#[test]
fn at_most_500_entries_of_each_kind() {
    let mut shared = Storage::in_memory();
    let folders = (0..501).map(|index| entry(Kind::Folder, Path::new(&format!("/f{index}")), None)).collect();
    add(&mut shared, folders);
    let recent = load(&mut shared);
    assert_eq!(recent.workspaces.len(), 500);
    assert_eq!(location(&recent.workspaces[0]), "file:///f0");
}

#[test]
fn workspace_entries_carry_the_workspace_id() {
    let workspace = entry(Kind::Workspace, Path::new("/w/project.code-workspace"), None);
    assert_eq!(kind(&workspace), Some(Kind::Workspace));
    assert_eq!(location(&workspace), "file:///w/project.code-workspace");
    assert_eq!(workspace["workspace"]["id"].as_str().map(str::len), Some(32));
}
