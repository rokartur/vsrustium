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

/// A resolved backup: the working copy's resource, its meta (without `typeId`) and the backed up text.
#[derive(Debug, PartialEq)]
pub struct Backup {
    pub resource: Uri,
    pub meta: Option<Value>,
    pub content: String,
}

/// `hashIdentifier` of a working copy without a type id: the hash of the file system path for `file` and
/// `untitled` resources, of the URI for the others, in hex as JavaScript's `toString(16)` writes an int32.
pub fn hash_identifier(resource: &Uri) -> String {
    let text = match resource.scheme.as_str() {
        "file" | "untitled" => resource.path.clone(),
        _ => resource.to_string(false),
    };
    let hash = string_hash(&text);
    match hash < 0 {
        true => format!("-{:x}", hash.unsigned_abs()),
        false => format!("{hash:x}"),
    }
}

/// `toBackupResource`: `<home>/<scheme>/<hash>`.
pub fn backup_path(home: &Path, resource: &Uri) -> PathBuf {
    home.join(&resource.scheme).join(hash_identifier(resource))
}

/// `backup`: the preamble with the meta (left out when the preamble would get too long), then the content,
/// written to a temporary file that replaces the backup.
pub fn write(home: &Path, resource: &Uri, meta: Option<&Value>, content: &str) -> std::io::Result<()> {
    let path = backup_path(home, resource);
    let mut preamble = preamble(resource, meta);
    if preamble.encode_utf16().count() >= PREAMBLE_MAX_LENGTH {
        preamble = self::preamble(resource, None);
    }
    std::fs::create_dir_all(path.parent().expect("a backup has a scheme folder"))?;
    let temporary = PathBuf::from(format!("{}{TEMP_SUFFIX}", path.display()));
    std::fs::write(&temporary, preamble + content)?;
    std::fs::rename(&temporary, &path)
}

fn preamble(resource: &Uri, meta: Option<&Value>) -> String {
    let mut meta = meta.cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
    meta["typeId"] = json!("");
    format!("{} {meta}\n", resource.to_string(false))
}

/// `resolve`: `None` for a file without a preamble end (a write cut short) or with a resource that does not parse.
pub fn resolve(path: &Path) -> Option<Backup> {
    let text = std::fs::read_to_string(path).ok()?;
    let (preamble, content) = text.split_once('\n')?;
    let (resource, meta) = preamble.split_once(' ').unwrap_or((preamble, ""));
    Some(Backup { resource: Uri::parse(resource).ok()?, meta: parse_meta(meta), content: content.to_owned() })
}

/// `parsePreambleMeta`: the JSON without its `typeId`, `None` when that leaves nothing or it does not parse.
fn parse_meta(raw: &str) -> Option<Value> {
    let mut meta: Value = serde_json::from_str(raw).ok()?;
    if meta["typeId"].is_string() {
        meta.as_object_mut()?.remove("typeId");
    }
    meta.as_object().is_some_and(|meta| !meta.is_empty()).then_some(meta)
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
pub fn discard(home: &Path, resource: &Uri) {
    let _ = std::fs::remove_file(backup_path(home, resource));
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
