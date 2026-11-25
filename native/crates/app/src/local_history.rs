//! Local history of saved files: workingCopyHistoryService.ts `WorkingCopyHistoryModel` for the native service, which
//! keeps every entry in `User/History/<hash of the resource>/` with an `entries.json` listing.

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use config::configuration::Overrides;
use config::service::Settings;
use files::glob;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const ENTRIES_FILE: &str = "entries.json";

/// An entry of `entries.json` (`ISerializedWorkingCopyHistoryModelEntry`); `source` is left out for "File Saved".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_description: Option<String>,
    pub timestamp: i64,
}

#[derive(Serialize, Deserialize)]
struct Listing {
    version: u32,
    resource: String,
    entries: Vec<Entry>,
}

/// The settings `addEntry` reads for the resource.
pub struct Limits {
    /// `workbench.localHistory.mergeWindow`, in seconds.
    pub merge_window: f64,
    /// `workbench.localHistory.maxFileEntries`.
    pub max_entries: usize,
}

/// workingCopyHistoryTracker.ts `shouldTrackHistory` for a saved file: the limits to add its entry with, or None when
/// it is larger than `workbench.localHistory.maxFileSize` KB, history is off, or `workbench.localHistory.exclude`
/// matches it (ResourceGlobMatcher: relative to its workspace folder, and absolute when a pattern is absolute).
pub fn tracked(settings: &Settings, file: &Path, size: u64) -> Option<Limits> {
    let overrides = Overrides { resource: Some(file), ..Overrides::default() };
    let value = |key: &str| settings.get_value(Some(key), &overrides).unwrap_or(Value::Null);
    // Number settings come back as floats when they hold a fraction or were written with one.
    if size as f64 > 1024. * value("workbench.localHistory.maxFileSize").as_f64()? || value("workbench.localHistory.enabled") == Value::Bool(false) {
        return None;
    }
    if let Some(exclude) = value("workbench.localHistory.exclude").as_object().filter(|exclude| !exclude.is_empty()) {
        let expression = glob::parse_expression(exclude, glob::Options::default());
        let absolute = file.to_string_lossy();
        let folder = settings.workspace.as_ref().and_then(|workspace| workspace.folder_of(file));
        let relative = folder.map(|folder| file.strip_prefix(&folder.path).expect("folder_of contains the file").to_string_lossy());
        if relative.as_ref().is_some_and(|relative| expression.matches(relative, None, None).is_some()) {
            return None;
        }
        let has_absolute_path = exclude.keys().any(|key| key.starts_with('/'));
        if has_absolute_path && relative.as_deref() != Some(&*absolute) && expression.matches(&absolute, None, None).is_some() {
            return None;
        }
    }
    Some(Limits { merge_window: value("workbench.localHistory.mergeWindow").as_f64()?, max_entries: value("workbench.localHistory.maxFileEntries").as_f64()? as usize })
}

/// `toHistoryEntriesFolder`: the folder of a resource's entries, named by the hash of its URI string.
pub fn entries_folder(history_home: &Path, resource: &str) -> PathBuf {
    history_home.join(hash_hex(resource))
}

/// `WorkingCopyHistoryModel.addEntry` followed by `store`: copies `file` into the history as a new entry, or over the
/// last one when it has the same source and is at most `merge_window` seconds older, then drops the oldest entries
/// past `max_entries` and writes `entries.json`.
pub fn add_entry(history_home: &Path, resource: &str, file: &Path, source: Option<&str>, timestamp: i64, limits: &Limits) -> io::Result<Entry> {
    // Saves run on the background executor; one at a time keeps a listing from being read while it is written.
    static WRITING: Mutex<()> = Mutex::new(());
    let _writing = WRITING.lock().unwrap();
    let folder = entries_folder(history_home, resource);
    let mut entries = read_entries(&folder, file)?;
    let merges_last = entries.last().is_some_and(|last| last.source.as_deref() == source && (timestamp - last.timestamp) as f64 <= limits.merge_window * 1000.);
    let entry = match merges_last {
        true => {
            let last = entries.last_mut().expect("checked above");
            last.source_description = None;
            last.timestamp = timestamp;
            last.clone()
        }
        false => {
            let entry = Entry { id: format!("{}{}", random_name(4), extname(file)), source: source.map(str::to_string), source_description: None, timestamp };
            entries.push(entry.clone());
            entry
        }
    };
    std::fs::create_dir_all(&folder)?;
    std::fs::copy(file, folder.join(&entry.id))?;
    if entries.len() > limits.max_entries {
        for removed in entries.drain(..entries.len() - limits.max_entries) {
            // deleteEntry only traces a failed delete.
            let _ = std::fs::remove_file(folder.join(removed.id));
        }
    }
    let listing = Listing { version: 1, resource: resource.to_string(), entries };
    std::fs::write(folder.join(ENTRIES_FILE), serde_json::to_vec(&listing).expect("entries serialize"))?;
    Ok(entry)
}

/// `resolveEntriesFromDisk`: every entry file in the folder with its modification time, then the listing's timestamp
/// and source for the ids it has, oldest first.
pub fn read_entries(folder: &Path, file: &Path) -> io::Result<Vec<Entry>> {
    let Ok(children) = std::fs::read_dir(folder) else { return Ok(Vec::new()) };
    let name_matcher = regex::Regex::new(&format!("[A-Za-z0-9]{{4}}{}", regex::escape(&extname(file)))).expect("escaped pattern");
    let mut entries = HashMap::new();
    for child in children {
        let child = child?;
        let name = child.file_name().to_string_lossy().into_owned();
        if name == ENTRIES_FILE || !name_matcher.is_match(&name) {
            continue;
        }
        let modified = child.metadata()?.modified()?;
        let timestamp = modified.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64);
        entries.insert(name.clone(), Entry { id: name, source: None, source_description: None, timestamp });
    }
    let listing = std::fs::read(folder.join(ENTRIES_FILE)).ok().and_then(|bytes| serde_json::from_slice::<Listing>(&bytes).ok());
    for listed in listing.map(|listing| listing.entries).unwrap_or_default() {
        if let Some(entry) = entries.get_mut(&listed.id) {
            entry.timestamp = listed.timestamp;
            entry.source = listed.source.or(entry.source.take());
            entry.source_description = listed.source_description.or(entry.source_description.take());
        }
    }
    let mut entries: Vec<Entry> = entries.into_values().collect();
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

/// hash.ts `hash(string).toString(16)`: the int32 string hash, negative values with a minus sign.
fn hash_hex(value: &str) -> String {
    let mut hash: i32 = 0;
    for unit in std::iter::once(149417).chain(value.encode_utf16().map(i32::from)) {
        hash = hash.wrapping_shl(5).wrapping_sub(hash).wrapping_add(unit);
    }
    match hash < 0 {
        true => format!("-{:x}", -(hash as i64)),
        false => format!("{hash:x}"),
    }
}

/// extpath.ts `randomPath(undefined, undefined, length)`.
fn random_name(length: usize) -> String {
    const PATH_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    (0..length)
        .map(|_| {
            let random = std::collections::hash_map::RandomState::new().build_hasher().finish();
            PATH_CHARS[(random % PATH_CHARS.len() as u64) as usize] as char
        })
        .collect()
}

/// resources.ts `extname`: the last `.` and what follows, empty for a name without one or starting with it.
fn extname(file: &Path) -> String {
    file.extension().map(|extension| format!(".{}", extension.to_string_lossy())).unwrap_or_default()
}
