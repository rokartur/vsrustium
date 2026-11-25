//! Main-thread side of one extension host session: init data, the startup calls the
//! workbench's MainThread* actors make, and a dispatcher for requests from the host.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use config::configuration::Configuration;
use config::registry::Registry;
use config::workspace::Workspace;
use serde_json::{Value, json};

use crate::console::format_console_entry;
use crate::main_thread::{MainThread, Waiting};
use crate::main_thread_configuration::MainThreadConfiguration;
use crate::main_thread_files::MainThreadFiles;
use crate::main_thread_workspace::{MainThreadWorkspace, Outgoing};
use crate::main_thread_secret_state::MainThreadSecretState;
use crate::main_thread_storage::MainThreadStorage;
use crate::rpc::{self, Arg, Message, Reply};
use crate::scan::{ScannedExtension, extension_identifier, file_uri};
use crate::utc::logs_folder_name;
use crate::{ExtensionHost, Incoming, proxy_id, proxy_name};

pub struct SessionPaths<'a> {
    pub app_root: &'a Path,
    pub user_data: &'a Path,
    pub workspace: Option<&'a Workspace>,
}

/// environmentService.windowLogsPath: <userData>/logs/<session start>/window1, one session start per process.
pub fn window_logs_path(user_data: &Path) -> PathBuf {
    static SESSION_START: OnceLock<String> = OnceLock::new();
    user_data.join("logs").join(SESSION_START.get_or_init(|| logs_folder_name(SystemTime::now()))).join("window1")
}

/// `IExtensionHostInitData` for a desktop window. Telemetry identifiers are placeholders the app replaces with
/// `MainThreadStorage::telemetry_info`; exthost-probe keeps them.
pub fn init_data(paths: &SessionPaths, extensions: &[ScannedExtension], version: &str) -> Value {
    let descriptions: Vec<&Value> = extensions.iter().map(|e| &e.description).collect();
    let ids: Vec<Value> = extensions.iter().map(|e| extension_identifier(&e.id)).collect();
    // ImplicitActivationEvents.createActivationEventsMap: lowercase ids, extensions without events left out.
    let mut activation_events = serde_json::Map::new();
    for extension in extensions {
        let events = crate::activation_events::read_activation_events(&extension.id, &extension.description);
        if !events.is_empty() {
            activation_events.insert(extension.id.to_lowercase(), json!(events));
        }
    }
    let workspace = paths.workspace.map(|workspace| {
        let configuration = workspace.file.as_deref().map(file_uri);
        json!({ "id": workspace_id(workspace), "name": workspace.label(), "transient": workspace.transient, "configuration": configuration, "isUntitled": false })
    });
    json!({
        "version": version,
        "quality": null,
        "parentPid": std::process::id(),
        "environment": {
            "isExtensionDevelopmentDebug": false,
            "appName": "Code - OSS",
            "appHost": "desktop",
            "appRoot": file_uri(paths.app_root),
            "appLanguage": "en",
            "isExtensionTelemetryLoggingOnly": false,
            "appUriScheme": URL_PROTOCOL,
            "globalStorageHome": file_uri(&paths.user_data.join("User/globalStorage")),
            "workspaceStorageHome": file_uri(&paths.user_data.join("User/workspaceStorage")),
            "extensionLogLevel": [],
        },
        "workspace": workspace,
        "extensions": {
            "versionId": 1,
            "allExtensions": descriptions,
            "activationEvents": activation_events,
            "myExtensions": ids,
        },
        "telemetryInfo": {
            "sessionId": "someValue.sessionId",
            "machineId": "someValue.machineId",
            "sqmId": "",
            "devDeviceId": "",
            "firstSessionDate": "",
        },
        "logLevel": 3,
        "loggers": [],
        "logsLocation": file_uri(&window_logs_path(paths.user_data).join("exthost")),
        // The local host starts EagerManualStart: the main side sends $startExtensionHost.
        "autoStart": false,
        "remote": { "isRemote": false, "authority": null, "connectionData": null },
        // A product without quality is not 'stable', so upstream forwards console stacks; extension tests turn both off.
        "consoleForward": { "includeStack": true, "logNative": false },
        "uiKind": 1,
    })
}

/// `MainThreadWorkspace.getWorkspaceData` for `$initializeWorkspace`, with the fields `init_data` gave the workspace.
pub fn workspace_data(init: &Value, workspace: &Workspace) -> Value {
    let mut data = init["workspace"].clone();
    let folders: Vec<Value> = workspace.folders.iter().enumerate().map(|(index, folder)| json!({ "uri": file_uri(&folder.path), "name": folder.name, "index": index })).collect();
    data["folders"] = json!(folders);
    data
}

/// `getWorkspaceIdentifier` for a workspace file: MD5 of the lowercased path (macOS is not Linux).
/// `getSingleFolderWorkspaceIdentifier` for a folder: MD5 of the path and the folder's birth time in
/// milliseconds, so a folder created again gets new workspace storage.
pub fn workspace_id(workspace: &Workspace) -> String {
    use md5::Digest;
    if let Some(file) = &workspace.file {
        let digest = md5::Md5::new().chain_update(file.to_string_lossy().to_lowercase().as_bytes()).finalize();
        return digest.iter().map(|byte| format!("{byte:02x}")).collect();
    }
    let folder = &workspace.folders[0].path;
    let birth_time = std::fs::metadata(folder).and_then(|metadata| metadata.created()).ok();
    let millis = birth_time.and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map(|age| age.as_millis().to_string());
    let digest = md5::Md5::new().chain_update(folder.to_string_lossy().as_bytes()).chain_update(millis.unwrap_or_default()).finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `IConfigurationInitData` of a session without settings files: the registered defaults and their scopes.
pub fn configuration_init_data(extensions: &[ScannedExtension]) -> Value {
    let described: Vec<(&str, &Value)> = extensions.iter().map(|extension| (extension.id.as_str(), &extension.description)).collect();
    let registry = Registry::with_extensions(&described);
    let mut data = Configuration { defaults: registry.default_model(), ..Configuration::default() }.to_data();
    data["configurationScopes"] = registry.scopes();
    data
}

pub enum CallResult {
    Ok(Reply),
    Err(Option<Value>),
}

/// What the main thread saw from the host during a session.
#[derive(Default)]
pub struct SessionLog {
    /// "Proxy.$method" -> number of requests from the host.
    pub host_calls: BTreeMap<String, u32>,
    /// Extension id -> activation error message.
    pub activation_errors: BTreeMap<String, String>,
    pub activated: Vec<String>,
    /// Lowercase ids of extensions whose activation started (`$onWillActivateExtension`).
    pub activation_started: HashSet<String>,
    pub console: Vec<String>,
}

pub struct Session {
    pub host: ExtensionHost,
    next_req: u32,
    results: HashMap<u32, CallResult>,
    pub log: SessionLog,
    /// `IExtensionDescription`s by lowercase id, for `$getExtension`.
    extensions: HashMap<String, Value>,
    /// `ExtensionDescriptionRegistry` version the host has, raised by each `$deltaExtensions`.
    extensions_version: u64,
    /// `_allRequestedActivateEvents`.
    requested_activation_events: HashSet<String>,
    extension_tests_req: Option<u32>,
    /// The window's settings; without it the host gets the registered defaults and writes are ignored.
    pub configuration: Option<MainThreadConfiguration>,
    /// Extension state on disk; without it `globalState` and `workspaceState` start empty and are not kept.
    pub storage: Option<MainThreadStorage>,
    /// Extension secrets; without it every MainThreadSecretState call answers `undefined`.
    pub secrets: Option<MainThreadSecretState>,
    /// The disk and its watchers; without it file calls answer `undefined` and no file events reach the host.
    pub files: Option<MainThreadFiles>,
    /// Workspace search; without it `findFiles` and `findTextInFiles` answer `undefined`.
    pub workspace: Option<MainThreadWorkspace>,
    forward: Option<Forward>,
    /// The workbench's handler for the MainThread* requests it answers while the session thread waits.
    pub main_thread: Option<Box<dyn MainThread>>,
    waiting: Waiting,
}

/// A host request the app answers later with `Session::reply` (MainThread* shapes the workbench implements).
#[derive(Debug)]
pub struct ForwardedRequest {
    pub req: u32,
    pub proxy: &'static str,
    pub method: String,
    pub args: Vec<Value>,
}

struct Forward {
    proxies: &'static [&'static str],
    send: Box<dyn FnMut(ForwardedRequest) + Send>,
    replies: std::sync::mpsc::Receiver<(u32, Result<Value, Value>)>,
    /// The ids of `ask_workbench` requests, counted down from `u32::MAX`, below which the host's ids never reach.
    asks: Vec<u32>,
}

impl Session {
    pub fn new(host: ExtensionHost) -> Self {
        Session {
            host,
            next_req: 0,
            results: HashMap::new(),
            log: SessionLog::default(),
            extensions: HashMap::new(),
            extensions_version: 1,
            requested_activation_events: HashSet::new(),
            extension_tests_req: None,
            configuration: None,
            storage: None,
            secrets: None,
            files: None,
            workspace: None,
            forward: None,
            main_thread: None,
            waiting: Waiting::default(),
        }
    }

    /// The calls the workbench makes once the handshake is done (MainThreadConfiguration,
    /// MainThreadWorkspace, MainThreadTelemetry constructors, then the extension service).
    pub fn start(&mut self, extensions: &[ScannedExtension], workspace: Option<Value>) -> anyhow::Result<()> {
        self.extensions = extensions.iter().map(|e| (e.id.to_lowercase(), e.description.clone())).collect();
        let configuration = self.configuration.as_ref().map_or_else(|| configuration_init_data(extensions), MainThreadConfiguration::configuration_data);
        self.notify("ExtHostConfiguration", "$initializeConfiguration", vec![configuration]);
        self.notify("ExtHostWorkspace", "$initializeWorkspace", vec![workspace.unwrap_or(Value::Null), json!(true)]);
        // TelemetryLevel.NONE and supportsTelemetry = false: the port has no telemetry.
        self.notify("ExtHostTelemetry", "$initializeTelemetryLevel", vec![json!(0), json!(false), json!({ "usage": false, "error": false })]);
        // MainThreadEditorTabs sends the model on construction; an empty window has one active group with no tabs.
        self.notify("ExtHostEditorTabs", "$acceptEditorTabModel", vec![json!([{ "isActive": true, "viewColumn": 0, "groupId": 0, "tabs": [] }])]);
        if self.files.is_some() {
            self.notify("ExtHostFileSystemInfo", "$acceptProviderInfos", vec![json!({ "$mid": 1, "scheme": "file", "path": "/dummy" }), json!(files::disk::CAPABILITIES)]);
        }
        let delta = json!({
            "versionId": 1, "toRemove": [], "toAdd": [], "addActivationEvents": {}, "myToRemove": [], "myToAdd": [],
        });
        self.call("ExtHostExtensionService", "$startExtensionHost", vec![Arg::Json(delta)], Duration::from_secs(60))?;
        Ok(())
    }

    /// The part of a delta `_deltaExtensions` applies: `canRemoveExtension` drops ids that are unknown or whose
    /// activation started, `canAddExtension` drops extensions the host already runs and keeps no removal for.
    pub fn applicable_delta<'a>(&self, to_add: &'a [ScannedExtension], to_remove: &[String]) -> (Vec<&'a ScannedExtension>, Vec<String>) {
        let to_remove: Vec<String> = to_remove
            .iter()
            .map(|id| id.to_lowercase())
            .filter(|id| self.extensions.contains_key(id) && !self.log.activation_started.contains(id))
            .collect();
        let to_add = to_add
            .iter()
            .filter(|extension| {
                let id = extension.id.to_lowercase();
                !self.extensions.contains_key(&id) || to_remove.contains(&id)
            })
            .collect();
        (to_add, to_remove)
    }

    /// `_updateExtensionsOnExtHosts` with a delta from `applicable_delta`, then `_activateAddedExtensionIfNeeded`.
    pub fn delta_extensions(&mut self, to_add: &[&ScannedExtension], to_remove: &[String]) -> anyhow::Result<()> {
        if to_add.is_empty() && to_remove.is_empty() {
            return Ok(());
        }
        let removed: Vec<Value> = to_remove.iter().map(|id| self.extensions.remove(id).expect("applicable_delta keeps known ids")["identifier"].clone()).collect();
        let mut activation_events = serde_json::Map::new();
        for extension in to_add {
            self.extensions.insert(extension.id.to_lowercase(), extension.description.clone());
            let events = crate::activation_events::read_activation_events(&extension.id, &extension.description);
            if !events.is_empty() {
                activation_events.insert(extension.id.to_lowercase(), json!(events));
            }
        }
        self.extensions_version += 1;
        let added: Vec<Value> = to_add.iter().map(|extension| extension_identifier(&extension.id)).collect();
        let descriptions: Vec<&Value> = to_add.iter().map(|extension| &extension.description).collect();
        let delta = json!({
            "versionId": self.extensions_version, "toRemove": removed, "toAdd": descriptions, "addActivationEvents": activation_events,
            "myToRemove": removed, "myToAdd": added,
        });
        self.call("ExtHostExtensionService", "$deltaExtensions", vec![Arg::Json(delta)], Duration::from_secs(60))?;
        for extension in to_add {
            if let Some(event) = self.activation_event_of_added(extension) {
                let reason = json!({ "startup": false, "extensionId": extension_identifier(&extension.id), "activationEvent": event });
                self.notify("ExtHostExtensionService", "$activate", vec![extension_identifier(&extension.id), reason]);
            }
        }
        Ok(())
    }

    /// `_activateAddedExtensionIfNeeded` with workspaceContains.ts `checkActivateWorkspaceContainsExtension`.
    fn activation_event_of_added(&self, extension: &ScannedExtension) -> Option<String> {
        let events = crate::activation_events::read_activation_events(&extension.id, &extension.description);
        if let Some(event) = events.iter().find(|event| self.requested_activation_events.contains(*event) || *event == "*" || *event == "onStartupFinished") {
            return Some(event.clone());
        }
        let workspace = self.workspace.as_ref()?;
        let mut globs = Vec::new();
        for event in &events {
            let Some(file_name_or_glob) = event.strip_prefix("workspaceContains:") else { continue };
            if file_name_or_glob.contains(['*', '?']) {
                globs.push(file_name_or_glob.to_owned());
            } else if workspace.folders().iter().any(|folder| folder.join(file_name_or_glob).exists()) {
                return Some(event.clone());
            }
        }
        (!globs.is_empty() && workspace.glob_file_exists(&globs)).then(|| format!("workspaceContains:{}", globs.join(",")))
    }

    /// Starts the test runner named in init data (`extensionTestsLocationURI`); `pump` returns false once it is done.
    pub fn execute_extension_tests(&mut self) {
        self.extension_tests_req = Some(self.send_request("ExtHostExtensionService", "$extensionTestsExecute", vec![]));
    }

    /// `IEnvironmentService.extensionTestsLocationURI` is set: the run is an extension test run.
    pub fn runs_extension_tests(&self) -> bool {
        self.extension_tests_req.is_some()
    }

    /// The exit code the app ends with after `execute_extension_tests`, as upstream's `_onExtensionHostExit`:
    /// the runner's result, 1 if it threw, or the host's own exit code if it died first.
    pub fn extension_tests_exit_code(&mut self) -> Option<i32> {
        let req = self.extension_tests_req?;
        Some(match self.results.remove(&req) {
            Some(CallResult::Ok(Reply::Json(code))) => code.as_i64().map_or(1, |code| code as i32),
            Some(CallResult::Ok(reply)) => {
                eprintln!("extension tests: unexpected result {reply:?}");
                1
            }
            Some(CallResult::Err(error)) => {
                eprintln!("extension tests failed: {}", error.map(|error| error_text(&error)).unwrap_or_default());
                1
            }
            None => {
                let status = self.host.child.wait().ok();
                eprintln!("extension host exited before the tests finished: {status:?}");
                status.and_then(|status| status.code()).filter(|code| *code != 0).unwrap_or(1)
            }
        })
    }

    /// Hands requests to `proxies` to `send` instead of answering them here; `pump` sends the answers that arrive on `replies`.
    pub fn forward(&mut self, proxies: &'static [&'static str], send: impl FnMut(ForwardedRequest) + Send + 'static, replies: std::sync::mpsc::Receiver<(u32, Result<Value, Value>)>) {
        self.forward = Some(Forward { proxies, send: Box::new(send), replies, asks: Vec::new() });
    }

    /// Asks the workbench from this side (`send` with a request no host sent) and waits for the answer while
    /// serving the host; `None` without a workbench or once the host is gone.
    pub fn ask_workbench(&mut self, proxy: &'static str, method: &str, args: Vec<Value>) -> Option<Result<Value, Value>> {
        let forward = self.forward.as_mut()?;
        let req = u32::MAX - forward.asks.len() as u32;
        forward.asks.push(req);
        (forward.send)(ForwardedRequest { req, proxy, method: method.into(), args });
        let result = loop {
            if let Some(result) = self.take_result(req) {
                break Some(result);
            }
            if !matches!(self.pump(Duration::from_millis(20)), Ok(true)) {
                break None;
            }
        };
        self.forward.as_mut().expect("set above").asks.retain(|ask| *ask != req);
        result
    }

    /// Answers a forwarded request; an error is a `SerializedError`.
    pub fn reply(&mut self, req: u32, result: Result<Value, Value>) {
        let message = match result {
            Ok(Value::Null) => Message::ReplyOk { req, value: Reply::Empty },
            Ok(value) => Message::ReplyOk { req, value: Reply::Json(value) },
            Err(error) => Message::ReplyErr { req, error: Some(error) },
        };
        self.host.send(rpc::encode(&message));
    }

    /// The host's answer to a `notify`, once it arrived.
    pub fn take_result(&mut self, req: u32) -> Option<Result<Value, Value>> {
        Some(match self.results.remove(&req)? {
            CallResult::Ok(Reply::Json(value) | Reply::JsonWithBuffers { json: value, .. }) => Ok(value),
            CallResult::Ok(_) => Ok(Value::Null),
            CallResult::Err(error) => Err(error.unwrap_or(Value::Null)),
        })
    }

    /// Sends a request without waiting for its reply.
    pub fn notify(&mut self, proxy: &str, method: &str, args: Vec<Value>) -> u32 {
        self.send_request(proxy, method, args.into_iter().map(Arg::Json).collect())
    }

    pub fn send_request(&mut self, proxy: &str, method: &str, args: Vec<Arg>) -> u32 {
        self.send(proxy, method, args, false)
    }

    fn send(&mut self, proxy: &str, method: &str, args: Vec<Arg>, cancellable: bool) -> u32 {
        self.next_req += 1;
        let req = self.next_req;
        if let ("$activateByEvent", Some(Arg::Json(Value::String(event)))) = (method, args.first()) {
            self.requested_activation_events.insert(event.clone());
        }
        let message = Message::Request { req, rpc_id: proxy_id(proxy), method: method.into(), args, cancellable };
        self.host.send(rpc::encode(&message));
        req
    }

    /// Sends a request and serves host requests until its reply arrives.
    pub fn call(&mut self, proxy: &str, method: &str, args: Vec<Arg>, timeout: Duration) -> anyhow::Result<Reply> {
        match self.request(proxy, method, args, timeout)? {
            CallResult::Ok(reply) => Ok(reply),
            CallResult::Err(error) => anyhow::bail!("{proxy}.{method} failed: {}", error.unwrap_or(Value::Null)),
        }
    }

    /// `call` that hands back the host's error object instead of failing with it.
    pub fn request(&mut self, proxy: &str, method: &str, args: Vec<Arg>, timeout: Duration) -> anyhow::Result<CallResult> {
        let req = self.send_request(proxy, method, args);
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(result) = self.results.remove(&req) {
                return Ok(result);
            }
            let left = deadline.checked_duration_since(Instant::now()).ok_or_else(|| anyhow::anyhow!("{proxy}.{method} timed out"))?;
            if !self.pump(left)? {
                anyhow::bail!("extension host exited during {proxy}.{method}");
            }
        }
    }

    /// A request whose host method takes a `CancellationToken`, cancelled after `timeout` (`raceCancellation`):
    /// `None` once cancelled or when the host is gone.
    pub fn request_cancellable(&mut self, proxy: &str, method: &str, args: Vec<Value>, timeout: Duration) -> Option<CallResult> {
        let req = self.send(proxy, method, args.into_iter().map(Arg::Json).collect(), true);
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(result) = self.results.remove(&req) {
                return Some(result);
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                self.host.send(rpc::encode(&Message::Cancel { req }));
                return None;
            };
            match self.pump(left) {
                Ok(true) => {}
                Ok(false) => return None,
                Err(error) => {
                    eprintln!("[exthost] {proxy}.{method}: {error:#}");
                    return None;
                }
            }
        }
    }

    /// Handles one incoming message, waiting up to `timeout`. Returns false once the host is gone or the extension tests are done.
    pub fn pump(&mut self, mut timeout: Duration) -> anyhow::Result<bool> {
        if let Some(args) = self.configuration.as_mut().and_then(MainThreadConfiguration::check_files) {
            self.notify("ExtHostConfiguration", "$acceptConfigurationChanged", args);
        }
        for event in self.files.as_mut().map(MainThreadFiles::take_events).unwrap_or_default() {
            self.notify("ExtHostFileSystemEventService", "$onFileEvent", vec![event]);
        }
        for outgoing in self.workspace.as_mut().map(MainThreadWorkspace::take_outgoing).unwrap_or_default() {
            match outgoing {
                Outgoing::TextSearchResult(args) => _ = self.notify("ExtHostWorkspace", "$handleTextSearchResult", args),
                Outgoing::Reply(reply) => self.host.send(rpc::encode(&reply)),
            }
        }
        if let Some(forward) = &self.forward {
            let answered: Vec<_> = forward.replies.try_iter().collect();
            let asks = forward.asks.clone();
            for (req, result) in answered {
                if !asks.contains(&req) {
                    self.reply(req, result);
                    continue;
                }
                let result = match result {
                    Ok(value) => CallResult::Ok(Reply::Json(value)),
                    Err(error) => CallResult::Err(Some(error)),
                };
                self.results.insert(req, result);
            }
            // An answer can come while a `call` waits (an extension's activate awaiting a forwarded request).
            timeout = timeout.min(Duration::from_millis(20));
        }
        let Some(incoming) = self.host.recv_timeout(timeout) else { return Ok(true) };
        let bytes = match incoming {
            Incoming::Message(bytes) => bytes,
            Incoming::Closed => return Ok(false),
        };
        match rpc::decode(&bytes)? {
            Message::Request { req, rpc_id, method, args, .. } if proxy_name(rpc_id) == "MainThreadConfiguration" && self.configuration.is_some() => {
                self.host.send(rpc::encode(&Message::Acknowledged { req }));
                *self.log.host_calls.entry(format!("MainThreadConfiguration.{method}")).or_default() += 1;
                let (change, reply) = self.configuration.as_mut().expect("checked by the match guard").serve(req, &method, &args);
                // The change reaches the host before the reply, as upstream's change event fires before the update resolves.
                if let Some(args) = change {
                    self.notify("ExtHostConfiguration", "$acceptConfigurationChanged", args);
                }
                self.host.send(rpc::encode(&reply));
            }
            Message::Request { req, rpc_id, method, args, .. } if proxy_name(rpc_id) == "MainThreadSecretState" && self.secrets.is_some() => {
                self.host.send(rpc::encode(&Message::Acknowledged { req }));
                *self.log.host_calls.entry(format!("MainThreadSecretState.{method}")).or_default() += 1;
                let (change, reply) = self.secrets.as_mut().expect("checked by the match guard").serve(req, &method, &args);
                // secretStorageService fires onDidChangeSecret while the write is still pending.
                if let Some(key) = change {
                    self.notify("ExtHostSecretState", "$onDidChangePassword", vec![key]);
                }
                self.host.send(rpc::encode(&reply));
            }
            Message::Request { req, rpc_id, method, args, .. }
                if matches!(proxy_name(rpc_id), "MainThreadFileSystem" | "MainThreadFileSystemEventService") && self.files.is_some() =>
            {
                self.host.send(rpc::encode(&Message::Acknowledged { req }));
                *self.log.host_calls.entry(format!("{}.{method}", proxy_name(rpc_id))).or_default() += 1;
                let reply = crate::main_thread_files::serve(self, req, &method, &args);
                self.host.send(rpc::encode(&reply));
            }
            Message::Request { req, rpc_id, method, args, .. } if proxy_name(rpc_id) == "MainThreadBulkEdits" && self.files.is_some() => {
                self.host.send(rpc::encode(&Message::Acknowledged { req }));
                *self.log.host_calls.entry(format!("MainThreadBulkEdits.{method}")).or_default() += 1;
                let reply = crate::main_thread_bulk_edits::serve(self, req, &args);
                self.host.send(rpc::encode(&reply));
            }
            Message::Request { req, rpc_id, method, args, .. }
                if proxy_name(rpc_id) == "MainThreadWorkspace" && MainThreadWorkspace::serves(&method) && self.workspace.is_some() =>
            {
                self.host.send(rpc::encode(&Message::Acknowledged { req }));
                *self.log.host_calls.entry(format!("MainThreadWorkspace.{method}")).or_default() += 1;
                self.workspace.as_mut().expect("checked by the match guard").start(req, &method, &args);
            }
            Message::Request { req, rpc_id, method, args, .. } => {
                self.host.send(rpc::encode(&Message::Acknowledged { req }));
                if let Some(forward) = self.forward.as_mut().filter(|forward| forward.proxies.contains(&proxy_name(rpc_id))) {
                    *self.log.host_calls.entry(format!("{}.{method}", proxy_name(rpc_id))).or_default() += 1;
                    if std::env::var_os("EXTHOST_TRACE").is_some() {
                        eprintln!("forward> {}.{method} {args:?}", proxy_name(rpc_id));
                    }
                    let args = args.into_iter().map(forwarded_arg).collect();
                    (forward.send)(ForwardedRequest { req, proxy: proxy_name(rpc_id), method, args });
                    return Ok(true);
                }
                self.serve(req, rpc_id, method, args);
            }
            Message::ReplyOk { req, value } => {
                self.results.insert(req, CallResult::Ok(value));
            }
            Message::ReplyErr { req, error } => {
                self.results.insert(req, CallResult::Err(error));
            }
            // A cancelled search or forwarded request; each side ignores a req it does not own.
            Message::Cancel { req } => {
                if let Some(workspace) = &self.workspace {
                    workspace.cancel(req);
                }
                if let Some(forward) = self.forward.as_mut() {
                    (forward.send)(ForwardedRequest { req, proxy: "", method: "$cancel".into(), args: Vec::new() });
                }
            }
            Message::Acknowledged { .. } => {}
        }
        Ok(self.extension_tests_req.is_none_or(|req| !self.results.contains_key(&req)))
    }

    /// Lets the workbench handler answer first; requests arriving while it calls the host wait for it.
    fn serve(&mut self, req: u32, rpc_id: u8, method: String, args: Vec<Arg>) {
        if self.waiting.busy {
            self.waiting.requests.push_back((req, rpc_id, method, args));
            return;
        }
        let proxy = proxy_name(rpc_id);
        let handled = match self.main_thread.take() {
            Some(mut main_thread) => {
                self.waiting.busy = true;
                let handled = main_thread.handle(self, proxy, &method, &args);
                main_thread.flush(self);
                self.waiting.busy = false;
                self.main_thread = Some(main_thread);
                handled
            }
            None => None,
        };
        let message = match handled {
            Some(Ok(value)) => Message::ReplyOk { req, value },
            Some(Err(error)) => Message::ReplyErr { req, error: Some(error) },
            None => Message::ReplyOk { req, value: self.handle_host_request(proxy, &method, &args) },
        };
        self.host.send(rpc::encode(&message));
        if let Some((req, rpc_id, method, args)) = self.waiting.requests.pop_front() {
            self.serve(req, rpc_id, method, args);
        }
    }

    /// Sends what the workbench queued for the host; host requests arriving meanwhile wait, as in `serve`.
    pub fn flush_main_thread(&mut self) {
        if self.waiting.busy {
            return;
        }
        if let Some(mut main_thread) = self.main_thread.take() {
            self.waiting.busy = true;
            main_thread.flush(self);
            self.waiting.busy = false;
            self.main_thread = Some(main_thread);
        }
        if let Some((req, rpc_id, method, args)) = self.waiting.requests.pop_front() {
            self.serve(req, rpc_id, method, args);
        }
    }

    /// Host console, errors and logger lines go to stderr, the port's log.
    fn handle_host_request(&mut self, proxy: &str, method: &str, args: &[Arg]) -> Reply {
        *self.log.host_calls.entry(format!("{proxy}.{method}")).or_default() += 1;
        if std::env::var_os("EXTHOST_TRACE").is_some() {
            eprintln!("host> {proxy}.{method} {args:?}");
        }
        let arg = |index: usize| match args.get(index) {
            Some(Arg::Json(value)) => value.clone(),
            _ => Value::Null,
        };
        match (proxy, method) {
            ("MainThreadConsole", "$logExtensionHostMessage") => {
                let message = arg(0);
                let text = format_console_entry(&message);
                eprintln!("{text}");
                self.log.console.push(format!("[{}] {text}", message["severity"].as_str().unwrap_or("log")));
            }
            ("MainThreadErrors", "$onUnexpectedError") => {
                let text = error_text(&arg(0));
                eprintln!("[exthost] unexpected error: {text}");
                self.log.console.push(format!("[unexpected] {text}"));
            }
            ("MainThreadLogger", "$log") => {
                let file = arg(0)["path"].as_str().unwrap_or_default().to_owned();
                for entry in arg(1).as_array().into_iter().flatten() {
                    eprintln!("[{file}] [{}] {}", log_level_name(&entry[0]), entry[1].as_str().unwrap_or_default());
                }
            }
            ("MainThreadExtensionService", "$getExtension") => {
                let id = arg(0).as_str().unwrap_or_default().to_lowercase();
                return self.extensions.get(&id).cloned().map_or(Reply::Empty, Reply::Json);
            }
            // The port has no browser: a URI is already what the host can load.
            ("MainThreadExtensionService", "$asBrowserUri") => return Reply::Json(arg(0)),
            ("MainThreadExtensionService", "$onWillActivateExtension") => {
                self.log.activation_started.insert(arg(0)["value"].as_str().unwrap_or_default().to_lowercase());
            }
            ("MainThreadExtensionService", "$onDidActivateExtension") => {
                self.log.activated.push(arg(0)["value"].as_str().unwrap_or_default().to_owned());
            }
            ("MainThreadExtensionService", "$onExtensionRuntimeError") => {
                eprintln!("[exthost] [{}] {}", arg(0)["value"].as_str().unwrap_or_default(), error_text(&arg(1)));
            }
            ("MainThreadExtensionService", "$onExtensionActivationError") => {
                let id = arg(0)["value"].as_str().unwrap_or_default().to_owned();
                let message = arg(1)["message"].as_str().map(str::to_owned).unwrap_or_else(|| arg(1).to_string());
                eprintln!("[exthost] activating {id} failed: {message}");
                self.log.activation_errors.insert(id, message);
            }
            ("MainThreadStorage", _) if self.storage.is_some() => return self.storage.as_mut().expect("checked by the match guard").serve(method, args),
            ("MainThreadWindow", "$getInitialState") => return Reply::Json(json!({ "isFocused": true, "isActive": true })),
            ("MainThreadUrls", "$createAppUri") => return Reply::Json(app_uri(&arg(0))),
            // No sign-in and no AI in the port: these APIs stay callable and report nothing.
            ("MainThreadLanguageModelTools", "$getTools")
            | ("MainThreadAuthentication", "$getAccounts")
            | ("MainThreadLanguageModels", "$selectChatModels")
            | (
                "MainThreadChatAgents2",
                "$provideCustomAgents" | "$provideInstructions" | "$provideSkills" | "$provideSlashCommands" | "$provideHooks" | "$providePlugins",
            ) => return Reply::Json(json!([])),
            _ => {}
        }
        // shortcut: every other MainThread* method answers `undefined`; each shape gets a real implementation as its ledger row lands.
        Reply::Empty
    }
}

/// product.json `urlProtocol`.
pub const URL_PROTOCOL: &str = "code-oss";

/// urlService.ts `NativeURLService.create`: the uri with the product's scheme.
pub fn app_uri(uri: &Value) -> Value {
    let text = |key: &str| uri[key].as_str().unwrap_or_default().to_owned();
    let (authority, mut path) = (text("authority"), text("path"));
    // URI validation requires a path if there is an authority
    if !authority.is_empty() && !path.is_empty() && !path.starts_with('/') {
        path.insert(0, '/');
    }
    json!({ "$mid": 1, "scheme": URL_PROTOCOL, "authority": authority, "path": path, "query": text("query"), "fragment": text("fragment") })
}

fn arg_value(arg: Arg) -> Value {
    match arg {
        Arg::Json(value) | Arg::JsonWithBuffers { json: value, .. } => value,
        Arg::Buffer(_) | Arg::Undefined => Value::Null,
    }
}

/// A forwarded request's argument; a `VSBuffer` (mainThreadWebviews.ts `$postMessage` buffers) travels as
/// `{"$vsbuffer": <base64>}` in the JSON the app reads.
fn forwarded_arg(arg: Arg) -> Value {
    match arg {
        Arg::Buffer(bytes) => json!({ VSBUFFER_KEY: STANDARD.encode(bytes) }),
        arg => arg_value(arg),
    }
}

pub const VSBUFFER_KEY: &str = "$vsbuffer";

/// A `SerializedError` (or any thrown value) as one string: its stack, else its message.
fn error_text(error: &Value) -> String {
    error["stack"].as_str().or(error["message"].as_str()).map_or_else(|| error.to_string(), str::to_owned)
}

/// `LogLevel` of platform/log/common/log.ts.
pub fn log_level_name(level: &Value) -> &'static str {
    match level.as_u64() {
        Some(0) => "off",
        Some(1) => "trace",
        Some(2) => "debug",
        Some(3) => "info",
        Some(4) => "warning",
        _ => "error",
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// workspaces.test.ts 'workspace identifiers are stable' (the local workspace file).
    #[test]
    fn workspace_file_identifiers_are_stable() {
        let workspace = Workspace { file: Some(PathBuf::from("/hello/test")), folders: Vec::new(), transient: false };
        assert_eq!(workspace_id(&workspace), "e36736311be12ff6d695feefe415b3e8");
        let upper = Workspace { file: Some(PathBuf::from("/Hello/Test")), ..workspace };
        assert_eq!(workspace_id(&upper), "e36736311be12ff6d695feefe415b3e8", "macOS paths are compared ignoring case");
    }

    #[test]
    fn app_uris_take_the_product_scheme() {
        let uri = json!({ "scheme": "vscode", "authority": "pub.ext", "path": "callback", "query": "a=1" });
        assert_eq!(app_uri(&uri), json!({ "$mid": 1, "scheme": "code-oss", "authority": "pub.ext", "path": "/callback", "query": "a=1", "fragment": "" }));
    }

    #[test]
    fn workspace_data_lists_every_folder() {
        let file = std::env::temp_dir().join(format!("session-test-{}.code-workspace", std::process::id()));
        std::fs::write(&file, r#"{ "folders": [{ "path": "a" }, { "path": "b", "name": "Second" }] }"#).unwrap();
        let workspace = Workspace::from_file(file.clone());
        let paths = SessionPaths { app_root: Path::new("/app"), user_data: Path::new("/user"), workspace: Some(&workspace) };
        let init = init_data(&paths, &[], "1.147.0");
        let name = format!("session-test-{} (Workspace)", std::process::id());
        assert_eq!(init["workspace"]["name"], name);
        assert_eq!(init["workspace"]["configuration"], file_uri(&file));
        let data = workspace_data(&init, &workspace);
        assert_eq!(data["folders"][1]["name"], "Second");
        assert_eq!(data["folders"][1]["index"], 1);
        assert_eq!(data["folders"][0]["uri"], file_uri(&file.parent().unwrap().join("a")));
        std::fs::remove_file(file).unwrap();
    }
}
