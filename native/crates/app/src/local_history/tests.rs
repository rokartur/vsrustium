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
    let first = add_entry(&home, "file:///notes.txt", &file, None, None, 1_000, &limits()).unwrap();
    std::fs::write(&file, "two").unwrap();
    let merged = add_entry(&home, "file:///notes.txt", &file, None, None, 11_000, &limits()).unwrap();
    assert_eq!(merged.id, first.id);
    std::fs::write(&file, "three").unwrap();
    let separate = add_entry(&home, "file:///notes.txt", &file, None, None, 21_001, &limits()).unwrap();
    assert_ne!(separate.id, first.id);
    let other_source = add_entry(&home, "file:///notes.txt", &file, Some("undoRedo.source"), None, 21_002, &limits()).unwrap();
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
    let ids: Vec<String> = (0..3).map(|index| add_entry(&home, "file:///notes.txt", &file, None, None, index * 1000, &limits).unwrap().id).collect();
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

#[test]
fn entries_follow_a_renamed_file_and_a_moved_folder() {
    let (home, file, dir) = setup("move");
    let resource = uri_string("file", &file);
    let saved = add_entry(&home, &resource, &file, None, None, 1_000, &limits()).unwrap();
    let renamed = dir.join("renamed.txt");
    std::fs::rename(&file, &renamed).unwrap();
    let moved = move_entries(&home, &file, &renamed, 50_000, |_| Some(limits())).unwrap();
    assert_eq!(moved, [renamed.clone()]);
    assert!(!entries_folder(&home, &resource).exists());
    let folder = entries_folder(&home, &uri_string("file", &renamed));
    let entries = read_entries(&folder, &renamed).unwrap();
    assert_eq!(entries[0].id, saved.id);
    assert_eq!(entries[1].source.as_deref(), Some("renamed.source"));
    assert_eq!(entries[1].source_description.as_deref(), Some(&*crate::quick_input::path_label(&file, None)));

    let (sub, other) = (dir.join("sub"), dir.join("other"));
    std::fs::create_dir(&sub).unwrap();
    let in_sub = sub.join("renamed.txt");
    std::fs::rename(&renamed, &in_sub).unwrap();
    move_entries(&home, &renamed, &in_sub, 60_000, |_| Some(limits())).unwrap();
    let entries = read_entries(&entries_folder(&home, &uri_string("file", &in_sub)), &in_sub).unwrap();
    assert_eq!(entries.last().unwrap().source.as_deref(), Some("moved.source"));
    // A moved folder takes the entries of the files under it.
    std::fs::rename(&sub, &other).unwrap();
    // Past the 10 s merge window of the "File Moved" entry at 60 000.
    let moved = move_entries(&home, &sub, &other, 90_000, |_| Some(limits())).unwrap();
    let in_other = other.join("renamed.txt");
    assert_eq!(moved, [in_other.clone()]);
    assert_eq!(read_entries(&entries_folder(&home, &uri_string("file", &in_other)), &in_other).unwrap().len(), 4);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn all_files_lists_the_files_with_entries_until_all_are_removed() {
    let (home, file, dir) = setup("all");
    add_entry(&home, &uri_string("file", &file), &file, None, None, 1_000, &limits()).unwrap();
    let empty = entries_folder(&home, "file:///empty.txt");
    std::fs::create_dir_all(&empty).unwrap();
    write_listing(&empty, "file:///empty.txt", Vec::new()).unwrap();
    assert_eq!(all_files(&home), [file.clone()]);
    remove_all(&home).unwrap();
    assert!(all_files(&home).is_empty());
    remove_all(&home).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn date_labels_read_as_the_english_intl_format() {
    use objc2_foundation::{NSDate, NSTimeZone};
    // 2025-11-05 13:04 UTC, shifted so it reads 13:04 in the local time zone.
    let utc = 1_762_347_840_000_i64;
    let offset = NSTimeZone::localTimeZone().secondsFromGMTForDate(&NSDate::dateWithTimeIntervalSince1970(utc as f64 / 1000.));
    assert_eq!(date_label(utc - offset as i64 * 1000), "November 5, 2025 at 1:04 PM");
}
