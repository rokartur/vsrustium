//! MainThreadFileSystem and MainThreadFileSystemEventService (workbench/api/browser/mainThreadFileSystem.ts,
//! mainThreadFileSystemEventService.ts): `vscode.workspace.fs` over the disk and the providers extensions register,
//! and the changes of the workspace folder, of extension watchers and of extension providers (`$onFileEvent`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use config::configuration::Overrides;
use config::service::Settings;
use files::disk::Disk;
use files::operations::{FileOperation, SourceTarget, UndoInfo, WorkingCopyFiles};
use files::watcher::{Change, ChangeType, Watcher};
use files::{FileError, FileSystem, Mode, Resource, Stat};
use serde_json::{Value, json};

use crate::rpc::{Arg, Message, Reply};
use crate::scan::{ScannedExtension, file_uri};
use crate::session::{CallResult, Session};

/// The id of the first workspace folder's watch, the next folders count down; extension sessions are numbered by
/// the host from 0 up.
const WORKSPACE_WATCH: u64 = u64::MAX;

/// shortcut: a provider call that takes longer fails instead of waiting forever, upstream has no limit; raise it if a
/// slow remote provider needs more.
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(300);

type Events = (Option<u64>, Vec<Change>);

#[derive(Clone, Copy)]
struct ExtensionProvider {
    handle: u64,
    capabilities: u32,
}

pub struct MainThreadFiles {
    watcher: Watcher,
    /// Correlated extension watchers each get their own, so their events carry the session.
    correlated: HashMap<u64, Watcher>,
    sender: Sender<Events>,
    receiver: Receiver<Events>,
    /// `RemoteFileSystemProvider`s by scheme.
    providers: HashMap<String, ExtensionProvider>,
    /// Watch session -> handle of the extension provider that watches it.
    provider_watches: HashMap<u64, u64>,
    /// Schemes some extension activates on (`onFileSystem:<scheme>`), and those already requested
    /// (`ExtensionHostManager._cachedActivationEvents`).
    activation_schemes: HashSet<String>,
    activated_schemes: HashSet<String>,
    settings: Arc<Mutex<Settings>>,
}

impl MainThreadFiles {
    /// `WorkspaceWatcher.watchWorkspace`: each folder recursively, minus its `files.watcherExclude` patterns set to true.
    pub fn new(folders: &[(PathBuf, Vec<String>)], extensions: &[ScannedExtension], settings: Arc<Mutex<Settings>>) -> anyhow::Result<Self> {
        let (sender, receiver) = channel();
        let workspace_sender = sender.clone();
        let mut watcher = Watcher::new(move |changes| {
            let _ = workspace_sender.send((None, changes));
        })?;
        for (index, (folder, excludes)) in folders.iter().enumerate() {
            watcher.watch(WORKSPACE_WATCH - index as u64, folder, true, excludes)?;
        }
        let mut activation_schemes = HashSet::new();
        for extension in extensions {
            for event in extension.description["activationEvents"].as_array().into_iter().flatten() {
                if let Some(scheme) = event.as_str().and_then(|event| event.strip_prefix("onFileSystem:")) {
                    activation_schemes.insert(scheme.to_owned());
                }
            }
        }
        Ok(MainThreadFiles {
            watcher,
            correlated: HashMap::new(),
            sender,
            receiver,
            providers: HashMap::new(),
            provider_watches: HashMap::new(),
            activation_schemes,
            activated_schemes: HashSet::new(),
            settings,
        })
    }

    /// The `$onFileEvent` arguments of the disk changes seen since the last call.
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
}

/// Answers a MainThreadFileSystem or MainThreadFileSystemEventService request; calls into extension providers
/// serve other host requests while they wait.
pub fn serve(session: &mut Session, req: u32, method: &str, args: &[Arg]) -> Message {
    match call(session, method, args) {
        Ok(value) => Message::ReplyOk { req, value },
        Err(err) => {
            let err = files::handle_error(err);
            let error = json!({ "$isError": true, "name": err.name, "message": err.message, "stack": format!("{}: {}", err.name, err.message) });
            Message::ReplyErr { req, error: Some(error) }
        }
    }
}

fn files(session: &mut Session) -> &mut MainThreadFiles {
    session.files.as_mut().expect("main_thread_files::serve runs only with files")
}

fn call(session: &mut Session, method: &str, args: &[Arg]) -> Result<Reply, FileError> {
    let json = |index: usize| match args.get(index) {
        Some(Arg::Json(value)) => value.clone(),
        _ => Value::Null,
    };
    let fs = &mut Providers { session };
    match method {
        "$stat" => {
            let stat = files::stat(fs, &resource(&json(0)))?;
            Ok(Reply::Json(json!({ "type": stat.file_type, "ctime": stat.ctime, "mtime": stat.mtime, "size": stat.size, "permissions": stat.permissions })))
        }
        "$readdir" => Ok(Reply::Json(json!(files::read_dir(fs, &resource(&json(0)))?))),
        "$readFile" => Ok(Reply::Buffer(files::read_file(fs, &resource(&json(0)))?)),
        "$writeFile" => {
            let content: &[u8] = match args.get(1) {
                Some(Arg::Buffer(bytes)) => bytes,
                _ => &[],
            };
            files::write_file(fs, &resource(&json(0)), content)?;
            Ok(Reply::Empty)
        }
        "$rename" | "$copy" => {
            let mode = if method == "$rename" { Mode::Move } else { Mode::Copy };
            files::move_or_copy(fs, &resource(&json(0)), &resource(&json(1)), mode, json(2)["overwrite"] == true)?;
            Ok(Reply::Empty)
        }
        "$mkdir" => {
            files::create_folder(fs, &resource(&json(0)))?;
            Ok(Reply::Empty)
        }
        "$delete" => {
            let options = json(1);
            files::delete(fs, &resource(&json(0)), options["recursive"] == true, options["useTrash"] == true)?;
            Ok(Reply::Empty)
        }
        "$ensureActivation" => {
            fs.capabilities(json(0).as_str().unwrap_or_default());
            Ok(Reply::Empty)
        }
        "$registerFileSystemProvider" => {
            let (handle, scheme, capabilities) = (json(0).as_u64().unwrap_or_default(), json(1).as_str().unwrap_or_default().to_owned(), json(2).as_u64().unwrap_or_default() as u32);
            files(fs.session).providers.insert(scheme.clone(), ExtensionProvider { handle, capabilities });
            fs.session.notify("ExtHostFileSystemInfo", "$acceptProviderInfos", vec![json!({ "$mid": 1, "scheme": scheme, "path": "/dummy" }), json!(capabilities)]);
            Ok(Reply::Empty)
        }
        "$unregisterProvider" => {
            let handle = json(0).as_u64();
            let providers = &mut files(fs.session).providers;
            let schemes: Vec<String> = providers.iter().filter(|(_, provider)| Some(provider.handle) == handle).map(|(scheme, _)| scheme.clone()).collect();
            for scheme in schemes {
                files(fs.session).providers.remove(&scheme);
                fs.session.notify("ExtHostFileSystemInfo", "$acceptProviderInfos", vec![json!({ "$mid": 1, "scheme": scheme, "path": "/dummy" }), Value::Null]);
            }
            Ok(Reply::Empty)
        }
        // `FileService.onDidFilesChange` of a provider's changes; `FileChangeType` UPDATED 0, ADDED 1, DELETED 2.
        "$onFileSystemChange" => {
            let changes = json(1);
            let of_type = |kind: u64| changes.as_array().into_iter().flatten().filter(|change| change["type"] == kind).map(|change| change["resource"].clone()).collect::<Vec<_>>();
            let event = json!({ "created": of_type(1), "changed": of_type(0), "deleted": of_type(2) });
            fs.session.notify("ExtHostFileSystemEventService", "$onFileEvent", vec![event]);
            Ok(Reply::Empty)
        }
        "$watch" => {
            watch(fs, json(1).as_u64().unwrap_or_default(), &json(2), &json(3), json(4) == true);
            Ok(Reply::Empty)
        }
        "$unwatch" => {
            let watch_session = json(0).as_u64().unwrap_or_default();
            let main_thread = files(fs.session);
            main_thread.watcher.unwatch(watch_session);
            main_thread.correlated.remove(&watch_session);
            if let Some(handle) = main_thread.provider_watches.remove(&watch_session) {
                fs.session.notify("ExtHostFileSystem", "$unwatch", vec![json!(handle), json!(watch_session)]);
            }
            Ok(Reply::Empty)
        }
        _ => Ok(Reply::Empty),
    }
}

/// `$watch`: a recursive request on a file becomes flat; a correlated flat one gets its own watcher; a resource of an
/// extension provider is watched by the provider (`RemoteFileSystemProvider.watch`).
fn watch(fs: &mut Providers, watch_session: u64, uri: &Value, options: &Value, correlate: bool) {
    let resource = resource(uri);
    if fs.capabilities(&resource.scheme).is_none() {
        eprintln!("MainThreadFileSystemEventService#$watch(): cannot watch resource as its scheme is not handled by the file service (path: {})", resource.for_error());
        return;
    }
    let recursive = options["recursive"] == true && files::stat(fs, &resource).is_ok_and(|stat| stat.file_type & files::DIRECTORY != 0);
    if resource.scheme != "file" {
        let handle = files(fs.session).providers[&resource.scheme].handle;
        files(fs.session).provider_watches.insert(watch_session, handle);
        let mut options = options.clone();
        options["recursive"] = json!(recursive);
        fs.session.notify("ExtHostFileSystem", "$watch", vec![json!(handle), json!(watch_session), uri.clone(), options]);
        return;
    }
    let path = PathBuf::from(&resource.path);
    let excludes: Vec<String> = options["excludes"].as_array().into_iter().flatten().filter_map(|pattern| pattern.as_str().map(str::to_owned)).collect();
    let main_thread = files(fs.session);
    let watched = if correlate && !recursive {
        let sender = main_thread.sender.clone();
        Watcher::new(move |changes| {
            let _ = sender.send((Some(watch_session), changes));
        })
        .and_then(|mut watcher| {
            watcher.watch(watch_session, &path, false, &excludes)?;
            main_thread.correlated.insert(watch_session, watcher);
            Ok(())
        })
    } else {
        main_thread.watcher.watch(watch_session, &path, recursive, &excludes)
    };
    if let Err(err) = watched {
        eprintln!("[File Watcher] {}: {err}", path.display());
    }
}

pub(crate) fn resource(uri: &Value) -> Resource {
    let part = |key: &str| uri[key].as_str().unwrap_or_default().to_owned();
    Resource { scheme: part("scheme"), authority: part("authority"), path: part("path") }
}

fn uri(resource: &Resource) -> Value {
    json!({ "$mid": 1, "scheme": resource.scheme, "authority": resource.authority, "path": resource.path })
}

/// The file service's providers: the disk for `file`, `ExtHostFileSystem` for the schemes extensions registered.
pub struct Providers<'a> {
    pub session: &'a mut Session,
}

impl Providers<'_> {
    /// `RemoteFileSystemProvider`: forwards the call with the provider's handle; the host's `FileSystemError` keeps its
    /// name (`EntryNotFound (FileSystemError)`).
    fn remote(&mut self, method: &str, resource: &Resource, mut args: Vec<Arg>) -> files::Result<Reply> {
        let handle = files(self.session).providers[&resource.scheme].handle;
        args.insert(0, Arg::Json(json!(handle)));
        args.insert(1, Arg::Json(uri(resource)));
        match self.session.request("ExtHostFileSystem", method, args, PROVIDER_TIMEOUT) {
            Ok(CallResult::Ok(reply)) => Ok(reply),
            Ok(CallResult::Err(error)) => {
                let error = error.unwrap_or_default();
                let text = |key: &str| error[key].as_str().unwrap_or_default().to_owned();
                Err(FileError { name: text("name"), message: text("message"), operation: None })
            }
            Err(err) => Err(FileError { name: "Error".into(), message: err.to_string(), operation: None }),
        }
    }

    fn setting(&mut self, key: &str, resource: Option<&Path>) -> Value {
        let settings = files(self.session).settings.lock().unwrap();
        settings.get_value(Some(key), &Overrides { resource, ..Overrides::default() }).unwrap_or(Value::Null)
    }

    fn remote_done(&mut self, method: &str, resource: &Resource, args: Vec<Arg>) -> files::Result<()> {
        self.remote(method, resource, args).map(|_| ())
    }
}

impl FileSystem for Providers<'_> {
    /// `FileService.activateProvider`: asks the host to activate `onFileSystem:<scheme>` extensions once, and waits
    /// for them only while the scheme has no provider yet.
    fn capabilities(&mut self, scheme: &str) -> Option<u32> {
        let registered = |main_thread: &MainThreadFiles| match scheme {
            "file" => Some(files::disk::CAPABILITIES),
            _ => main_thread.providers.get(scheme).map(|provider| provider.capabilities),
        };
        let main_thread = files(self.session);
        if main_thread.activation_schemes.contains(scheme) && main_thread.activated_schemes.insert(scheme.to_owned()) {
            let event = json!(format!("onFileSystem:{scheme}"));
            if registered(main_thread).is_some() {
                self.session.notify("ExtHostExtensionService", "$activateByEvent", vec![event, json!(0)]);
            } else if let Err(err) = self.session.request("ExtHostExtensionService", "$activateByEvent", vec![Arg::Json(event), Arg::Json(json!(0))], PROVIDER_TIMEOUT) {
                eprintln!("[exthost] activating onFileSystem:{scheme} failed: {err}");
            }
        }
        registered(files(self.session))
    }

    fn stat(&mut self, resource: &Resource) -> files::Result<Stat> {
        if resource.scheme == "file" {
            return Disk.stat(resource);
        }
        let Reply::Json(stat) = self.remote("$stat", resource, vec![])? else { return Err(files::provider_error(files::UNKNOWN, "$stat answered no stat")) };
        let number = |key: &str| stat[key].as_u64().unwrap_or_default();
        Ok(Stat { file_type: number("type") as u32, ctime: number("ctime"), mtime: number("mtime"), size: number("size"), permissions: stat["permissions"].as_u64().map(|bits| bits as u32) })
    }

    fn read_dir(&mut self, resource: &Resource) -> files::Result<Vec<(String, u32)>> {
        if resource.scheme == "file" {
            return Disk.read_dir(resource);
        }
        let Reply::Json(entries) = self.remote("$readdir", resource, vec![])? else { return Ok(Vec::new()) };
        let mut children = Vec::new();
        for entry in entries.as_array().into_iter().flatten() {
            children.push((entry[0].as_str().unwrap_or_default().to_owned(), entry[1].as_u64().unwrap_or_default() as u32));
        }
        Ok(children)
    }

    fn read_file(&mut self, resource: &Resource) -> files::Result<Vec<u8>> {
        if resource.scheme == "file" {
            return Disk.read_file(resource);
        }
        match self.remote("$readFile", resource, vec![])? {
            Reply::Buffer(bytes) => Ok(bytes),
            Reply::JsonWithBuffers { mut buffers, .. } if !buffers.is_empty() => Ok(buffers.swap_remove(0)),
            _ => Ok(Vec::new()),
        }
    }

    fn write_file(&mut self, resource: &Resource, content: &[u8], create: bool, overwrite: bool) -> files::Result<()> {
        if resource.scheme == "file" {
            return Disk.write_file(resource, content, create, overwrite);
        }
        let options = json!({ "create": create, "overwrite": overwrite, "unlock": false, "atomic": false });
        self.remote_done("$writeFile", resource, vec![Arg::Buffer(content.to_vec()), Arg::Json(options)])
    }

    fn mkdir(&mut self, resource: &Resource) -> files::Result<()> {
        if resource.scheme == "file" {
            return Disk.mkdir(resource);
        }
        self.remote_done("$mkdir", resource, vec![])
    }

    fn delete(&mut self, resource: &Resource, recursive: bool, use_trash: bool) -> files::Result<()> {
        if resource.scheme == "file" {
            return Disk.delete(resource, recursive, use_trash);
        }
        self.remote_done("$delete", resource, vec![Arg::Json(json!({ "recursive": recursive, "useTrash": use_trash, "atomic": false }))])
    }

    fn rename(&mut self, from: &Resource, to: &Resource, overwrite: bool) -> files::Result<()> {
        if from.scheme == "file" {
            return Disk.rename(from, to, overwrite);
        }
        self.remote_done("$rename", from, vec![Arg::Json(uri(to)), Arg::Json(json!({ "overwrite": overwrite }))])
    }

    fn copy(&mut self, from: &Resource, to: &Resource, overwrite: bool) -> files::Result<()> {
        if from.scheme == "file" {
            return Disk.copy(from, to, overwrite);
        }
        self.remote_done("$copy", from, vec![Arg::Json(uri(to)), Arg::Json(json!({ "overwrite": overwrite }))])
    }
}

/// The extension host's file operation participant (mainThreadFileSystemEventService.ts) and the working copy
/// file service's settings.
impl WorkingCopyFiles for Providers<'_> {
    /// `WorkingCopyFileOperationParticipant.participate` with `files.participants.timeout`, then the participant of
    /// mainThreadFileSystemEventService.ts: the host's `onWill*` edits, cancelled after the timeout.
    fn participate(&mut self, operation: FileOperation, files: &[SourceTarget], undo: UndoInfo) {
        let timeout = self.setting("files.participants.timeout", None).as_f64().unwrap_or_default();
        if timeout <= 0.0 {
            return;
        }
        let args = vec![json!(operation as u8), source_targets(files), json!(timeout)];
        let reply = self.session.request_cancellable("ExtHostFileSystemEventService", "$onWillRunFileOperation", args, Duration::from_millis(timeout as u64));
        let data = match reply {
            Some(CallResult::Ok(Reply::Json(data))) => data,
            Some(CallResult::Err(error)) => {
                eprintln!("[file participants] {}", error.unwrap_or_default());
                return;
            }
            _ => return,
        };
        if data["edit"]["edits"].as_array().is_none_or(Vec::is_empty) {
            return;
        }
        // Upstream asks first ("Extension '{0}' wants to make refactoring changes ...") unless extension tests run;
        // the dialog and the refactor preview come with the text edits of milestone 8 (port/notes.md).
        if !self.session.runs_extension_tests() {
            eprintln!("[onWill-handler] skipping additional workspace edit from extensions {}", data["extensionNames"]);
            return;
        }
        eprintln!("[onWill-handler] applying additional workspace edit from extensions {}", data["extensionNames"]);
        if let Err(error) = crate::main_thread_bulk_edits::apply(self, &data["edit"], undo.undo_redo_group_id) {
            eprintln!("[file participants] {error}");
        }
    }

    fn did_run(&mut self, operation: FileOperation, files: &[SourceTarget]) {
        self.session.notify("ExtHostFileSystemEventService", "$onDidRunFileOperation", vec![json!(operation as u8), source_targets(files)]);
    }

    fn enable_trash(&mut self) -> bool {
        self.setting("files.enableTrash", None) != false
    }

    /// `TextFileService.getEncodedReadable` of empty text: the BOM `getWriteEncoding` enforces for the encoding.
    fn empty_file(&mut self, resource: &Resource) -> Vec<u8> {
        let path = (resource.scheme == "file").then(|| Path::new(&resource.path));
        match self.setting("files.encoding", path).as_str() {
            Some("utf8bom") => vec![0xEF, 0xBB, 0xBF],
            Some("utf16be") => vec![0xFE, 0xFF],
            Some("utf16le") => vec![0xFF, 0xFE],
            _ => Vec::new(),
        }
    }
}

/// `SourceTargetPair[]` as the host revives them; a create or delete has no `source`.
fn source_targets(files: &[SourceTarget]) -> Value {
    let mut pairs = Vec::new();
    for pair in files {
        let mut value = json!({ "target": uri(&pair.target) });
        if let Some(source) = &pair.source {
            value["source"] = uri(source);
        }
        pairs.push(value);
    }
    Value::Array(pairs)
}
