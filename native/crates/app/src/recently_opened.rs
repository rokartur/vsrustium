//! workspacesHistoryMainService.ts: the recently opened folders, workspaces and files, stored as
//! `history.recentlyOpenedPathsList` in the shared storage in workspaces.ts `toStoreData` form. Entries stay as
//! stored, so ones of schemes the port cannot open (remote) survive its writes.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use config::workspace::Workspace;
use serde_json::{Value, json};
use storage::Storage;

use crate::editor_history::{file_path, file_uri_string};
use crate::quick_input::path_label;

pub const STORAGE_KEY: &str = "history.recentlyOpenedPathsList";
const MAX_TOTAL_RECENT_ENTRIES: usize = 500;
/// `COMMON_FILES_FILTER`: files git opens that are no documents of the user's.
const COMMON_FILES_FILTER: [&str; 3] = ["COMMIT_EDITMSG", "MERGE_MSG", "git-rebase-todo"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Folder,
    Workspace,
    File,
}

/// `IRecentlyOpened` as `toStoreData` writes its entries.
#[derive(Debug, Default, PartialEq)]
pub struct RecentlyOpened {
    pub workspaces: Vec<Value>,
    pub files: Vec<Value>,
}

/// An `IRecent` in its serialized form: `{ folderUri }`, `{ workspace: { id, configPath } }` or `{ fileUri }`.
pub fn entry(kind: Kind, path: &Path, label: Option<String>) -> Value {
    let uri = file_uri_string(path);
    let mut entry = match kind {
        Kind::Folder => json!({ "folderUri": uri }),
        Kind::Workspace => json!({ "workspace": { "id": exthost::session::workspace_id(&Workspace::from_file(path.to_owned())), "configPath": uri } }),
        Kind::File => json!({ "fileUri": uri }),
    };
    // toStoreData `storeLabel`: only a label that differs from the path is kept.
    if let Some(label) = label.filter(|label| *label != path.to_string_lossy()) {
        entry["label"] = json!(label);
    }
    entry
}

pub fn kind(entry: &Value) -> Option<Kind> {
    if entry["workspace"]["id"].is_string() && entry["workspace"]["configPath"].is_string() {
        Some(Kind::Workspace)
    } else if entry["folderUri"].is_string() {
        Some(Kind::Folder)
    } else if entry["fileUri"].is_string() {
        Some(Kind::File)
    } else {
        None
    }
}

/// `location`: the folder, workspace file or file URI.
pub fn location(entry: &Value) -> &str {
    let uri = match kind(entry) {
        Some(Kind::Workspace) => &entry["workspace"]["configPath"],
        Some(Kind::Folder) => &entry["folderUri"],
        _ => &entry["fileUri"],
    };
    uri.as_str().unwrap_or_default()
}

/// The `file` path of an entry; `None` for other schemes.
pub fn path(entry: &Value) -> Option<PathBuf> {
    file_path(location(entry))
}

/// The label Open Recent shows: the stored one, else labelService `getWorkspaceLabel` with `Verbosity.LONG` for
/// folders and workspaces and `getUriLabel` for files, all tildified. Only for `file` entries.
pub fn label(entry: &Value) -> String {
    if let Some(label) = entry["label"].as_str() {
        return label.to_string();
    }
    let path = path(entry).expect("a `file` entry");
    match kind(entry) {
        Some(Kind::Workspace) => format!("{} (Workspace)", path_label(&path.with_extension(""), None)),
        _ => path_label(&path, None),
    }
}

/// `getRecentlyOpened`: read from the file, as other windows (processes) write it too.
pub fn load(shared: &mut Storage) -> RecentlyOpened {
    shared.refresh();
    let stored: Value = shared.get(STORAGE_KEY).and_then(|value| serde_json::from_str(value).ok()).unwrap_or_default();
    let mut recent = RecentlyOpened::default();
    for entry in stored["entries"].as_array().into_iter().flatten() {
        match kind(entry) {
            Some(Kind::File) => recent.files.push(entry.clone()),
            Some(_) => recent.workspaces.push(entry.clone()),
            None => {}
        }
    }
    recent
}

/// `addRecentlyOpened`: the new entries first, then the stored ones not among them, at most 500 of each.
pub fn add(shared: &mut Storage, entries: Vec<Value>) {
    let mut added = RecentlyOpened::default();
    for entry in entries {
        let is_file = kind(&entry) == Some(Kind::File);
        let list = if is_file { &mut added.files } else { &mut added.workspaces };
        let filtered = is_file && path(&entry).is_some_and(|path| path.file_name().is_some_and(|name| COMMON_FILES_FILTER.iter().any(|filter| name == *filter)));
        if !filtered && !contains(list, &entry) {
            list.push(entry);
        }
    }
    let stored = load(shared);
    merge(&mut added.workspaces, stored.workspaces);
    merge(&mut added.files, stored.files);
    added.workspaces.truncate(MAX_TOTAL_RECENT_ENTRIES);
    added.files.truncate(MAX_TOTAL_RECENT_ENTRIES);
    save(shared, &added);
}

/// `removeRecentlyOpened`.
pub fn remove(shared: &mut Storage, uris: &[&str]) {
    let mut recent = load(shared);
    let count = recent.workspaces.len() + recent.files.len();
    let keys: Vec<String> = uris.iter().map(|uri| comparison_key(uri)).collect();
    let keep = |entry: &Value| !keys.contains(&comparison_key(location(entry)));
    recent.workspaces.retain(keep);
    recent.files.retain(keep);
    if recent.workspaces.len() + recent.files.len() != count {
        save(shared, &recent);
    }
}

/// `clearRecentlyOpened`.
pub fn clear(shared: &mut Storage) {
    save(shared, &RecentlyOpened::default());
}

/// `mergeEntriesFromStorage`: a stored entry already present lends its label to the new one.
fn merge(entries: &mut Vec<Value>, stored: Vec<Value>) {
    for stored in stored {
        match entries.iter_mut().find(|entry| same(entry, &stored)) {
            Some(entry) if entry["label"].is_null() && !stored["label"].is_null() => entry["label"] = stored["label"].clone(),
            Some(_) => {}
            None => entries.push(stored),
        }
    }
}

fn contains(entries: &[Value], entry: &Value) -> bool {
    entries.iter().any(|known| same(known, entry))
}

fn same(a: &Value, b: &Value) -> bool {
    comparison_key(location(a)) == comparison_key(location(b))
}

/// `extUriBiasedIgnorePathCase.getComparisonKey`: `file` paths compare ignoring case, as the macOS disk does.
fn comparison_key(uri: &str) -> String {
    match file_path(uri) {
        Some(path) => path.to_string_lossy().to_lowercase(),
        None => uri.to_string(),
    }
}

fn save(shared: &mut Storage, recent: &RecentlyOpened) {
    let entries: Vec<&Value> = recent.workspaces.iter().chain(&recent.files).collect();
    shared.set(STORAGE_KEY, json!({ "entries": entries }).to_string());
}
