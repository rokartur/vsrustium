//! Working copy backups on disk: workingCopyBackupService.ts (the backup files under `Backups/<workspace id>`, one per
//! working copy, a `<resource> <meta>\n` preamble before the content) and backupMainService.ts (`Backups/workspaces.json`,
//! the folders, workspaces and empty windows that have a backup folder).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::main_thread_status_bar::string_hash;
use crate::uri::Uri;

const PREAMBLE_MAX_LENGTH: usize = 10000;
/// `WORKING_COPY_BACKUP_TEMP_FILE_SUFFIX`: written first, then renamed over the backup.
const TEMP_SUFFIX: &str = ".vsctmp";

/// A resolved backup: the working copy's resource and type id, its meta (without `typeId`) and the backed up bytes.
#[derive(Debug, PartialEq)]
pub struct Backup {
    pub resource: Uri,
    pub type_id: String,
    pub meta: Option<Value>,
    pub content: Vec<u8>,
}

/// `hashIdentifier`: the hash of the file system path for `file` and `untitled` resources, of the URI for the
/// others; a type id (empty for text files and untitled editors) joins its own hash to the resource path first.
pub fn hash_identifier(resource: &Uri, type_id: &str) -> String {
    let typed;
    let resource = match type_id.is_empty() {
        true => resource,
        false => {
            let type_hash = hash_string(type_id);
            typed = resource
                .with(|uri| {
                    uri.path = match uri.path.is_empty() {
                        true => type_hash,
                        false => format!("{}/{type_hash}", uri.path.trim_end_matches('/')),
                    }
                })
                .expect("a path segment keeps the URI valid");
            &typed
        }
    };
    hash_string(&match resource.scheme.as_str() {
        "file" | "untitled" => resource.path.clone(),
        _ => resource.to_string(false),
    })
}

/// `hash(str).toString(16)`: hex as JavaScript writes an int32.
fn hash_string(text: &str) -> String {
    let hash = string_hash(text);
    match hash < 0 {
        true => format!("-{:x}", hash.unsigned_abs()),
        false => format!("{hash:x}"),
    }
}

/// `toBackupResource`: `<home>/<scheme>/<hash>`.
pub fn backup_path(home: &Path, resource: &Uri, type_id: &str) -> PathBuf {
    home.join(&resource.scheme).join(hash_identifier(resource, type_id))
}

/// `backup`: the preamble with the meta (left out when the preamble would get too long), then the content,
/// written to a temporary file that replaces the backup.
pub fn write(home: &Path, resource: &Uri, type_id: &str, meta: Option<&Value>, content: &[u8]) -> std::io::Result<()> {
    let path = backup_path(home, resource, type_id);
    let mut preamble = preamble(resource, type_id, meta);
    if preamble.encode_utf16().count() >= PREAMBLE_MAX_LENGTH {
        preamble = self::preamble(resource, type_id, None);
    }
    std::fs::create_dir_all(path.parent().expect("a backup has a scheme folder"))?;
    let temporary = PathBuf::from(format!("{}{TEMP_SUFFIX}", path.display()));
    std::fs::write(&temporary, [preamble.as_bytes(), content].concat())?;
    std::fs::rename(&temporary, &path)
}

fn preamble(resource: &Uri, type_id: &str, meta: Option<&Value>) -> String {
    let mut meta = meta.cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
    meta["typeId"] = json!(type_id);
    format!("{} {meta}\n", resource.to_string(false))
}

/// `resolve`: `None` for a file without a preamble end (a write cut short) or with a resource that does not parse.
pub fn resolve(path: &Path) -> Option<Backup> {
    let bytes = std::fs::read(path).ok()?;
    let end = bytes.iter().position(|byte| *byte == b'\n')?;
    let preamble = std::str::from_utf8(&bytes[..end]).ok()?;
    let (resource, meta) = preamble.split_once(' ').unwrap_or((preamble, ""));
    let (type_id, meta) = parse_meta(meta);
    Some(Backup { resource: Uri::parse(resource).ok()?, type_id, meta, content: bytes[end + 1..].to_vec() })
}

/// `parsePreambleMeta`: the type id (empty without one) and the JSON without it, `None` when that leaves nothing
/// or it does not parse.
fn parse_meta(raw: &str) -> (String, Option<Value>) {
    let Ok(Value::Object(mut meta)) = serde_json::from_str::<Value>(raw) else { return (String::new(), None) };
    let type_id = match meta.get("typeId") {
        Some(Value::String(type_id)) => type_id.clone(),
        _ => String::new(),
    };
    if meta["typeId"].is_string() {
        meta.remove("typeId");
    }
    (type_id, (!meta.is_empty()).then_some(Value::Object(meta)))
}

/// The backups in `home` (WorkingCopyBackupsModel `resolve`), without temporary files.
pub fn list(home: &Path) -> Vec<PathBuf> {
    let mut backups = Vec::new();
    for scheme in std::fs::read_dir(home).into_iter().flatten().flatten().filter(|entry| entry.path().is_dir()) {
        for backup in std::fs::read_dir(scheme.path()).into_iter().flatten().flatten() {
            if !backup.file_name().to_string_lossy().ends_with(TEMP_SUFFIX) {
                backups.push(backup.path());
            }
        }
    }
    backups.sort();
    backups
}

/// `discardBackup`.
pub fn discard(home: &Path, resource: &Uri, type_id: &str) {
    let _ = std::fs::remove_file(backup_path(home, resource, type_id));
}

/// `Backups/workspaces.json` (backupMainService.ts `ISerializedBackupWorkspaces`).
pub fn workspaces_file(user_data: &Path) -> PathBuf {
    user_data.join("Backups/workspaces.json")
}

/// backupMainService.ts `registerEmptyWindowBackup`, `registerFolderBackup` and `registerWorkspaceBackup`: adds the
/// entry under `key` unless it is there.
pub fn register(user_data: &Path, key: &str, entry: Value) {
    let file = workspaces_file(user_data);
    let mut workspaces = read_workspaces(&file);
    let list = workspaces[key].as_array_mut().expect("read_workspaces gives arrays");
    if !list.contains(&entry) {
        list.push(entry);
        let _ = std::fs::create_dir_all(file.parent().expect("in Backups"));
        let _ = std::fs::write(&file, workspaces.to_string());
    }
}

/// backupMainService.ts `getEmptyWindowBackups`: the backup folders of empty windows that still hold backups.
pub fn empty_window_backups(user_data: &Path) -> Vec<String> {
    let workspaces = read_workspaces(&workspaces_file(user_data));
    let folders = workspaces["emptyWindows"].as_array().into_iter().flatten().filter_map(|entry| entry["backupFolder"].as_str());
    folders.filter(|folder| !list(&user_data.join("Backups").join(folder)).is_empty()).map(str::to_owned).collect()
}

fn read_workspaces(file: &Path) -> Value {
    let mut workspaces: Value = std::fs::read(file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    for key in ["rootURIWorkspaces", "folders", "emptyWindows"] {
        if !workspaces[key].is_array() {
            workspaces[key] = json!([]);
        }
    }
    workspaces
}

#[cfg(test)]
mod tests;
