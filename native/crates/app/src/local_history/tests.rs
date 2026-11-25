use super::*;

fn limits() -> Limits {
    Limits { merge_window: 10., max_entries: 50 }
}

fn setup(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("local-history-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("notes.txt");
    std::fs::write(&file, "one").unwrap();
    (dir.join("History"), file, dir)
}

#[test]
fn history_folders_are_named_by_the_uri_hash() {
    assert_eq!(hash_hex("file:///a/b.txt"), "74731c7c");
    assert_eq!(hash_hex("file:///Users/artur/ws/notes.txt"), "-3aa898c0");
}

#[test]
fn saves_within_the_merge_window_replace_the_last_entry() {
    let (home, file, dir) = setup("merge");
    let first = add_entry(&home, "file:///notes.txt", &file, None, 1_000, &limits()).unwrap();
    std::fs::write(&file, "two").unwrap();
    let merged = add_entry(&home, "file:///notes.txt", &file, None, 11_000, &limits()).unwrap();
    assert_eq!(merged.id, first.id);
    std::fs::write(&file, "three").unwrap();
    let separate = add_entry(&home, "file:///notes.txt", &file, None, 21_001, &limits()).unwrap();
    assert_ne!(separate.id, first.id);
    let other_source = add_entry(&home, "file:///notes.txt", &file, Some("undoRedo.source"), 21_002, &limits()).unwrap();
    assert_ne!(other_source.id, separate.id);

    let folder = entries_folder(&home, "file:///notes.txt");
    assert!(regex::Regex::new(r"^[A-Za-z0-9]{4}\.txt$").unwrap().is_match(&first.id));
    assert_eq!(std::fs::read_to_string(folder.join(&first.id)).unwrap(), "two");
    let listing: serde_json::Value = serde_json::from_slice(&std::fs::read(folder.join("entries.json")).unwrap()).unwrap();
    assert_eq!(
        listing,
        serde_json::json!({ "version": 1, "resource": "file:///notes.txt", "entries": [
            { "id": first.id, "timestamp": 11_000 },
            { "id": separate.id, "timestamp": 21_001 },
            { "id": other_source.id, "source": "undoRedo.source", "timestamp": 21_002 },
        ] })
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn entries_past_the_limit_are_deleted_oldest_first() {
    let (home, file, dir) = setup("limit");
    let limits = Limits { merge_window: 0., max_entries: 2 };
    let ids: Vec<String> = (0..3).map(|index| add_entry(&home, "file:///notes.txt", &file, None, index * 1000, &limits).unwrap().id).collect();
    let folder = entries_folder(&home, "file:///notes.txt");
    let kept: Vec<String> = read_entries(&folder, &file).unwrap().into_iter().map(|entry| entry.id).collect();
    assert_eq!(kept, ids[1..]);
    assert!(!folder.join(&ids[0]).exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn excluded_large_or_disabled_files_are_not_tracked() {
    let (_, file, dir) = setup("tracked");
    let user = dir.join("User");
    std::fs::create_dir_all(&user).unwrap();
    let workspace = config::workspace::Workspace::folder(dir.clone());
    let load = |settings: serde_json::Value| {
        std::fs::write(user.join("settings.json"), settings.to_string()).unwrap();
        Settings::load(config::registry::Registry::with_extensions(&[]), user.clone(), Some(workspace.clone()))
    };
    let settings = load(serde_json::json!({}));
    let limits = tracked(&settings, &file, 256 * 1024).unwrap();
    assert_eq!((limits.merge_window, limits.max_entries), (10., 50));
    assert!(tracked(&settings, &file, 256 * 1024 + 1).is_none());
    assert!(tracked(&load(serde_json::json!({ "workbench.localHistory.enabled": false })), &file, 3).is_none());
    assert!(tracked(&load(serde_json::json!({ "workbench.localHistory.exclude": { "*.txt": true } })), &file, 3).is_none());
    assert!(tracked(&load(serde_json::json!({ "workbench.localHistory.exclude": { "*.md": true } })), &file, 3).is_some());
    let absolute = format!("{}/**", dir.display());
    assert!(tracked(&load(serde_json::json!({ "workbench.localHistory.exclude": { (absolute): true } })), &file, 3).is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn entry_files_missing_from_the_listing_still_count() {
    let (home, file, dir) = setup("unlisted");
    let folder = entries_folder(&home, "file:///notes.txt");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("AbC1.txt"), "old").unwrap();
    std::fs::write(folder.join("other.md"), "not an entry").unwrap();
    let entries = read_entries(&folder, &file).unwrap();
    assert_eq!(entries.iter().map(|entry| entry.id.as_str()).collect::<Vec<_>>(), ["AbC1.txt"]);
    std::fs::remove_dir_all(dir).unwrap();
}
