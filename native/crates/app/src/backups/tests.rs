//! Ported from workingCopyBackupService.test.ts (electron-browser).

use super::*;

fn file(path: &str) -> Uri {
    Uri::new("file", "", path, "", "").unwrap()
}

fn untitled(name: &str) -> Uri {
    Uri::new("untitled", "", name, "", "").unwrap()
}

fn home(name: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("vsrustium-backups-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    home
}

#[test]
fn hash_identifier_matches_the_upstream_hashes() {
    // "If these hashes change people will lose their backed up files".
    assert_eq!(hash_identifier(&untitled("Untitled-1"), ""), "-7f9c1a2e");
    assert_eq!(hash_identifier(&file("/foo"), ""), "20eb3560");
    assert_eq!(hash_identifier(&Uri::new("vscode-custom", "", "somePath", "", "").unwrap(), ""), "-44972d98");
}

#[test]
fn typed_hash_identifiers_match_the_upstream_hashes() {
    assert_eq!(hash_identifier(&untitled("Untitled-1"), "hashTest"), "-8ad5f4f");
    assert_eq!(hash_identifier(&file("/foo"), "hashTest"), "51e56bf");
    assert_eq!(hash_identifier(&Uri::new("vscode-custom", "", "somePath", "", "").unwrap(), "hashTest"), "502149c7");
    assert_eq!(hash_identifier(&Uri::new("vscode-fragment", "", "", "", "frag").unwrap(), "hashTest"), "6e82ca57");
}

#[test]
fn a_typed_backup_keeps_its_type_id_and_bytes() {
    let home = home("typed");
    let bytes = [b'{', 0xff, b'\n', b'}'];
    write(&home, &file("/a.ipynb"), "notebook/jupyter-notebook/jupyter-notebook", None, &bytes).unwrap();
    let backup = resolve(&backup_path(&home, &file("/a.ipynb"), "notebook/jupyter-notebook/jupyter-notebook")).unwrap();
    assert_eq!(backup, Backup { resource: file("/a.ipynb"), type_id: "notebook/jupyter-notebook/jupyter-notebook".into(), meta: None, content: bytes.to_vec() });
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn backup_paths_are_home_scheme_hash() {
    let home = Path::new("/backups/ws");
    assert_eq!(backup_path(home, &file("/foo"), ""), home.join("file/20eb3560"));
    assert_eq!(backup_path(home, &untitled("Untitled-1"), ""), home.join("untitled/-7f9c1a2e"));
}

#[test]
fn a_backup_holds_the_preamble_then_the_text() {
    let home = home("write");
    let resource = file("/foo");
    write(&home, &resource, "", None, b"").unwrap();
    assert_eq!(std::fs::read_to_string(backup_path(&home, &resource, "")).unwrap(), "file:///foo {\"typeId\":\"\"}\n");
    write(&home, &resource, "", Some(&json!({ "etag": "678", "orphaned": true })), b"test").unwrap();
    assert_eq!(std::fs::read_to_string(backup_path(&home, &resource, "")).unwrap(), "file:///foo {\"etag\":\"678\",\"orphaned\":true,\"typeId\":\"\"}\ntest");
    assert_eq!(list(&home), vec![backup_path(&home, &resource, "")]);
    discard(&home, &resource, "");
    assert!(list(&home).is_empty());
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn resolve_gives_the_text_and_the_meta_back() {
    let home = home("resolve");
    let contents = "Lorem ipsum dolor öäü sit amet \nadipiscing ßß elit\nconsectetur ";
    let meta = json!({ "mtime": 1, "size": 888, "etag": "x", "orphaned": false });
    write(&home, &file("/foo bar.txt"), "", Some(&meta), contents.as_bytes()).unwrap();
    let backup = resolve(&backup_path(&home, &file("/foo bar.txt"), "")).unwrap();
    assert_eq!(backup, Backup { resource: file("/foo bar.txt"), type_id: String::new(), meta: Some(meta), content: contents.into() });
    write(&home, &untitled("Untitled-1"), "", None, b"").unwrap();
    assert_eq!(resolve(&backup_path(&home, &untitled("Untitled-1"), "")).unwrap().meta, None);
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn too_large_meta_is_left_out() {
    let home = home("large");
    let meta = json!({ "etag": "Large String".repeat(100 * 1024), "size": 888 });
    write(&home, &file("/foo"), "", Some(&meta), b"text").unwrap();
    let backup = resolve(&backup_path(&home, &file("/foo"), "")).unwrap();
    assert_eq!((backup.meta, backup.content.as_slice()), (None, b"text".as_slice()));
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn broken_meta_and_invalid_backups() {
    let home = home("broken");
    let path = backup_path(&home, &file("/foo"), "");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "file:///foo {broken\ntext").unwrap();
    assert_eq!(resolve(&path).unwrap(), Backup { resource: file("/foo"), type_id: String::new(), meta: None, content: "text".into() });
    std::fs::write(&path, "").unwrap();
    assert_eq!(resolve(&path), None, "empty file");
    std::fs::write(&path, "file:///foo").unwrap();
    assert_eq!(resolve(&path), None, "no preamble end");
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn empty_window_backups_are_the_registered_folders_with_backups() {
    let user_data = home("workspaces");
    register(&user_data, "emptyWindows", json!({ "backupFolder": "1" }));
    register(&user_data, "emptyWindows", json!({ "backupFolder": "2" }));
    register(&user_data, "emptyWindows", json!({ "backupFolder": "2" }));
    write(&user_data.join("Backups/2"), &untitled("Untitled-1"), "", None, b"x").unwrap();
    assert_eq!(empty_window_backups(&user_data), vec!["2".to_owned()]);
    let workspaces: Value = serde_json::from_slice(&std::fs::read(workspaces_file(&user_data)).unwrap()).unwrap();
    assert_eq!(workspaces["emptyWindows"], json!([{ "backupFolder": "1" }, { "backupFolder": "2" }]));
    std::fs::remove_dir_all(&user_data).unwrap();
}
