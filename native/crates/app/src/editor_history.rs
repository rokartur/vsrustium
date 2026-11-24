//! The editor history of historyService.ts that Quick Open lists: files most recently active first, at most
//! `MAX_HISTORY_ITEMS`, kept as `history.entries` in the workspace storage. Left out: editors without a file
//! (the port has none) and updates on file moves and deletes.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const MAX_HISTORY_ITEMS: usize = 200;
pub const HISTORY_STORAGE_KEY: &str = "history.entries";

#[derive(Default)]
pub struct EditorHistory {
    pub entries: Vec<PathBuf>,
}

impl EditorHistory {
    /// `loadHistory` from the stored entries; the port restores no editors before, so none are merged in.
    pub fn load(stored: Option<&str>, include: impl Fn(&Path) -> bool) -> EditorHistory {
        let mut history = EditorHistory::default();
        let stored: Vec<Value> = stored.and_then(|raw| serde_json::from_str(raw).ok()).unwrap_or_default();
        for entry in stored {
            // A string from 1.67 on, `UriComponents` before.
            let resource = &entry["editor"]["resource"];
            let path = match resource.as_str() {
                Some(uri) => file_path(uri),
                None => (resource["scheme"] == "file").then(|| resource["path"].as_str().map(PathBuf::from)).flatten(),
            };
            let Some(path) = path else { continue };
            if include(&path) && !history.entries.contains(&path) {
                history.entries.push(path);
                history.entries.truncate(MAX_HISTORY_ITEMS);
            }
        }
        history
    }

    /// `handleActiveEditorChangeInHistory`: the active editor moves to the front.
    pub fn add(&mut self, path: &Path) {
        self.remove(path);
        self.entries.insert(0, path.to_path_buf());
        self.entries.truncate(MAX_HISTORY_ITEMS);
    }

    pub fn remove(&mut self, path: &Path) -> bool {
        let count = self.entries.len();
        self.entries.retain(|entry| entry != path);
        self.entries.len() != count
    }

    /// `saveState`: what `toUntyped` of a file editor gives, the resource as `URI.toString()`.
    pub fn serialize(&self) -> String {
        let entries: Vec<Value> = self.entries.iter().map(|path| json!({ "editor": { "resource": file_uri_string(path), "forceFile": true, "options": { "override": "default" } } })).collect();
        Value::Array(entries).to_string()
    }
}

/// `URI.file(path).toString()`: path characters other than unreserved ones and `/` percent-encoded.
fn file_uri_string(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => uri.push(byte as char),
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

/// The path of a `file:` URI string; `None` for other schemes.
fn file_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = (bytes[index] == b'%').then(|| encoded.get(index + 1..index + 3)).flatten().and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match hex {
            Some(byte) => {
                decoded.push(byte);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    Some(PathBuf::from(String::from_utf8(decoded).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_editors_move_to_the_front_and_the_history_keeps_200() {
        let mut history = EditorHistory::default();
        for index in 0..205 {
            history.add(Path::new(&format!("/w/{index}.txt")));
        }
        assert_eq!(history.entries.len(), 200);
        assert_eq!(history.entries[0], Path::new("/w/204.txt"));
        history.add(Path::new("/w/100.txt"));
        assert_eq!(history.entries[..2], [PathBuf::from("/w/100.txt"), PathBuf::from("/w/204.txt")]);
        assert_eq!(history.entries.len(), 200);
        assert!(history.remove(Path::new("/w/100.txt")));
        assert!(!history.remove(Path::new("/w/100.txt")));
    }

    #[test]
    fn stored_entries_round_trip_without_excluded_ones() {
        let mut stored = EditorHistory::default();
        stored.entries = vec![PathBuf::from("/w/a b.txt"), PathBuf::from("/w/ü.ts"), PathBuf::from("/w/excluded/x.ts")];
        let raw = stored.serialize();
        assert!(raw.contains("\"file:///w/a%20b.txt\""));
        assert!(raw.contains("\"file:///w/%C3%BC.ts\""));
        let history = EditorHistory::load(Some(&raw), |path| !path.starts_with("/w/excluded"));
        assert_eq!(history.entries, [PathBuf::from("/w/a b.txt"), PathBuf::from("/w/ü.ts")]);
        let legacy = r#"[{"editor":{"resource":{"scheme":"file","path":"/w/old.ts"}}},{"editor":{}},{"editor":{"resource":"untitled:Untitled-1"}}]"#;
        assert_eq!(EditorHistory::load(Some(legacy), |_| true).entries, [PathBuf::from("/w/old.ts")]);
        assert!(EditorHistory::load(Some("not json"), |_| true).entries.is_empty());
    }
}
