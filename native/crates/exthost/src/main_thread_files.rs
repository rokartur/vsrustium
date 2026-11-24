//! MainThreadFileSystem and MainThreadFileSystemEventService (workbench/api/browser/mainThreadFileSystem.ts,
//! mainThreadFileSystemEventService.ts): `vscode.workspace.fs` calls the host does not serve itself, and the disk
//! changes of the workspace folder and of extension watchers (`$onFileEvent`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use files::watcher::{Change, ChangeType, Watcher};
use files::{FileError, Mode};
use serde_json::{Value, json};

use crate::rpc::{Arg, Message, Reply};
use crate::scan::file_uri;

/// diskFileSystemProviderClient.ts capabilities on macOS: FileReadWrite, FileOpenReadWriteClose, FileReadStream,
/// FileFolderCopy, FileWriteUnlock, FileAtomicRead/Write/Delete/Rename, FileAppend, FileClone, FileRealpath, Trash.
pub const DISK_CAPABILITIES: u32 = 2 | 4 | 16 | 8 | (1 << 13) | (1 << 14) | (1 << 15) | (1 << 16) | (1 << 20) | (1 << 19) | (1 << 17) | (1 << 18) | (1 << 12);

/// The id of the workspace folder's watch; extension sessions are numbered by the host from 0 up.
const WORKSPACE_WATCH: u64 = u64::MAX;

type Events = (Option<u64>, Vec<Change>);

pub struct MainThreadFiles {
    watcher: Watcher,
    /// Correlated extension watchers each get their own, so their events carry the session.
    correlated: HashMap<u64, Watcher>,
    sender: Sender<Events>,
    receiver: Receiver<Events>,
}

impl MainThreadFiles {
    /// `WorkspaceWatcher.watchWorkspace`: the folder, recursively, minus `files.watcherExclude` patterns set to true.
    pub fn new(folder: Option<&Path>, watcher_excludes: &[String]) -> anyhow::Result<Self> {
        let (sender, receiver) = channel();
        let workspace_sender = sender.clone();
        let mut watcher = Watcher::new(move |changes| {
            let _ = workspace_sender.send((None, changes));
        })?;
        if let Some(folder) = folder {
            watcher.watch(WORKSPACE_WATCH, folder, true, watcher_excludes)?;
        }
        Ok(MainThreadFiles { watcher, correlated: HashMap::new(), sender, receiver })
    }

    /// The `$onFileEvent` arguments of the changes seen since the last call.
    pub fn take_events(&mut self) -> Vec<Value> {
        let mut events = Vec::new();
        while let Ok((session, changes)) = self.receiver.try_recv() {
            let uris = |kind| changes.iter().filter(|change| change.kind == kind).map(|change| file_uri(&change.path)).collect::<Vec<_>>();
            let mut event = json!({ "created": uris(ChangeType::Added), "changed": uris(ChangeType::Updated), "deleted": uris(ChangeType::Deleted) });
            if let Some(session) = session {
                event["session"] = json!(session);
            }
            events.push(event);
        }
        events
    }

    pub fn serve(&mut self, req: u32, method: &str, args: &[Arg]) -> Message {
        match self.call(method, args) {
            Ok(value) => Message::ReplyOk { req, value },
            Err(err) => {
                let error = json!({ "$isError": true, "name": err.name, "message": err.message, "stack": format!("{}: {}", err.name, err.message) });
                Message::ReplyErr { req, error: Some(error) }
            }
        }
    }

    fn call(&mut self, method: &str, args: &[Arg]) -> Result<Reply, FileError> {
        let json = |index: usize| match args.get(index) {
            Some(Arg::Json(value)) => value.clone(),
            _ => Value::Null,
        };
        match method {
            "$stat" => {
                let stat = files::stat(&file_path(&json(0))?)?;
                Ok(Reply::Json(json!({ "type": stat.file_type, "ctime": stat.ctime, "mtime": stat.mtime, "size": stat.size, "permissions": stat.permissions })))
            }
            "$readdir" => {
                let path = file_path(&json(0))?;
                let stat = files::stat(&path)?;
                if stat.file_type & files::DIRECTORY == 0 {
                    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                    return Err(FileError { name: files::FILE_NOT_A_DIRECTORY, message: name });
                }
                Ok(Reply::Json(json!(files::read_dir(&path)?)))
            }
            "$readFile" => Ok(Reply::Buffer(files::read_file(&file_path(&json(0))?)?)),
            "$writeFile" => {
                let content: &[u8] = match args.get(1) {
                    Some(Arg::Buffer(bytes)) => bytes,
                    _ => &[],
                };
                files::write_file(&file_path(&json(0))?, content)?;
                Ok(Reply::Empty)
            }
            "$rename" | "$copy" => {
                let mode = if method == "$rename" { Mode::Move } else { Mode::Copy };
                let overwrite = json(2)["overwrite"].as_bool().unwrap_or(false);
                files::move_or_copy(&file_path(&json(0))?, &file_path(&json(1))?, mode, overwrite)?;
                Ok(Reply::Empty)
            }
            "$mkdir" => {
                files::create_folder(&file_path(&json(0))?)?;
                Ok(Reply::Empty)
            }
            "$delete" => {
                let options = json(1);
                files::delete(&file_path(&json(0))?, options["recursive"] == true, options["useTrash"] == true)?;
                Ok(Reply::Empty)
            }
            // Only the disk provider is registered, and it is active from the start.
            "$ensureActivation" => Ok(Reply::Empty),
            "$watch" => {
                self.watch(json(1).as_u64().unwrap_or_default(), &json(2), &json(3), json(4) == true);
                Ok(Reply::Empty)
            }
            "$unwatch" => {
                let session = json(0).as_u64().unwrap_or_default();
                self.watcher.unwatch(session);
                self.correlated.remove(&session);
                Ok(Reply::Empty)
            }
            // shortcut: extension file system providers are not served yet; their schemes answer ENOPRO.
            _ => Ok(Reply::Empty),
        }
    }

    /// `$watch`: a recursive request on a file becomes flat; a correlated flat one gets its own watcher.
    fn watch(&mut self, session: u64, resource: &Value, options: &Value, correlate: bool) {
        let Ok(path) = file_path(resource) else {
            eprintln!("MainThreadFileSystemEventService#$watch(): cannot watch resource as its scheme is not handled by the file service ({resource})");
            return;
        };
        let recursive = options["recursive"] == true && files::stat(&path).is_ok_and(|stat| stat.file_type & files::DIRECTORY != 0);
        let excludes: Vec<String> = options["excludes"].as_array().into_iter().flatten().filter_map(|pattern| pattern.as_str().map(str::to_owned)).collect();
        let watched = if correlate && !recursive {
            let sender = self.sender.clone();
            Watcher::new(move |changes| {
                let _ = sender.send((Some(session), changes));
            })
            .and_then(|mut watcher| {
                watcher.watch(session, &path, false, &excludes)?;
                self.correlated.insert(session, watcher);
                Ok(())
            })
        } else {
            self.watcher.watch(session, &path, recursive, &excludes)
        };
        if let Err(err) = watched {
            eprintln!("[File Watcher] {}: {err}", path.display());
        }
    }
}

/// The path of a `file` URI; any other scheme has no provider (`FileService.withProvider`).
fn file_path(uri: &Value) -> Result<PathBuf, FileError> {
    let scheme = uri["scheme"].as_str().unwrap_or_default();
    let path = uri["path"].as_str().unwrap_or_default();
    if scheme == "file" {
        return Ok(PathBuf::from(path));
    }
    let authority = uri["authority"].as_str().unwrap_or_default();
    let resource = if authority.is_empty() { format!("{scheme}:{path}") } else { format!("{scheme}://{authority}{path}") };
    Err(FileError { name: "CodeExpectedError", message: format!("ENOPRO: No file system provider found for resource '{resource}'") })
}

