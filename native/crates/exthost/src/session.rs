//! Main-thread side of one extension host session: init data, the startup calls the
//! workbench's MainThread* actors make, and a dispatcher for requests from the host.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use config::configuration::Configuration;
use config::registry::Registry;
use serde_json::{Value, json};

use crate::console::format_console_entry;
use crate::main_thread_configuration::MainThreadConfiguration;
use crate::main_thread_files::MainThreadFiles;
use crate::main_thread_secret_state::MainThreadSecretState;
use crate::main_thread_storage::MainThreadStorage;
use crate::rpc::{self, Arg, Message, Reply};
use crate::scan::{ScannedExtension, extension_identifier, file_uri};
use crate::utc::logs_folder_name;
use crate::{ExtensionHost, Incoming, proxy_id, proxy_name};

pub struct SessionPaths<'a> {
    pub app_root: &'a Path,
    pub user_data: &'a Path,
    pub workspace_folder: Option<&'a Path>,
}

/// `IExtensionHostInitData` for a desktop window. Telemetry identifiers are placeholders the app replaces with
/// `MainThreadStorage::telemetry_info`; exthost-probe keeps them.
pub fn init_data(paths: &SessionPaths, extensions: &[ScannedExtension], version: &str) -> Value {
    let descriptions: Vec<&Value> = extensions.iter().map(|e| &e.description).collect();
    let ids: Vec<Value> = extensions.iter().map(|e| extension_identifier(&e.id)).collect();
    let activation_events: BTreeMap<&str, &Value> =
        extensions.iter().map(|e| (e.id.as_str(), e.description.get("activationEvents").unwrap_or(&Value::Null))).collect();
    let activation_events: BTreeMap<&str, Value> = activation_events
        .into_iter()
        .map(|(id, events)| (id, if events.is_array() { events.clone() } else { json!([]) }))
        .collect();
    let workspace = paths.workspace_folder.map(|folder| {
        json!({ "id": workspace_id(folder), "name": folder.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(), "transient": false, "configuration": null, "isUntitled": false })
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
            "appUriScheme": "code-oss",
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
        // environmentService.extHostLogsPath: <userData>/logs/<session start>/window1/exthost.
        "logsLocation": file_uri(&paths.user_data.join("logs").join(logs_folder_name(SystemTime::now())).join("window1/exthost")),
        // The local host starts EagerManualStart: the main side sends $startExtensionHost.
        "autoStart": false,
        "remote": { "isRemote": false, "authority": null, "connectionData": null },
        // A product without quality is not 'stable', so upstream forwards console stacks; extension tests turn both off.
        "consoleForward": { "includeStack": true, "logNative": false },
        "uiKind": 1,
    })
}

/// The `IWorkspaceData` of a single-folder workspace for `$initializeWorkspace`, with the id and name `init_data` gave it.
pub fn workspace_data(init: &Value, folder: &Path) -> Value {
    let name = &init["workspace"]["name"];
    json!({ "id": init["workspace"]["id"], "name": name, "folders": [{ "uri": file_uri(folder), "name": name, "index": 0 }] })
}

/// `getSingleFolderWorkspaceIdentifier` on macOS: MD5 of the path and the folder's birth time in milliseconds,
/// so a folder created again gets new workspace storage.
pub fn workspace_id(folder: &Path) -> String {
    use md5::Digest;
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
    pub console: Vec<String>,
}

pub struct Session {
    pub host: ExtensionHost,
    next_req: u32,
    results: HashMap<u32, CallResult>,
    pub log: SessionLog,
    /// `IExtensionDescription`s by lowercase id, for `$getExtension`.
    extensions: HashMap<String, Value>,
    extension_tests_req: Option<u32>,
    /// The window's settings; without it the host gets the registered defaults and writes are ignored.
    pub configuration: Option<MainThreadConfiguration>,
    /// Extension state on disk; without it `globalState` and `workspaceState` start empty and are not kept.
    pub storage: Option<MainThreadStorage>,
    /// Extension secrets; without it every MainThreadSecretState call answers `undefined`.
    pub secrets: Option<MainThreadSecretState>,
    /// The disk and its watchers; without it file calls answer `undefined` and no file events reach the host.
    pub files: Option<MainThreadFiles>,
}

impl Session {
    pub fn new(host: ExtensionHost) -> Self {
        Session { host, next_req: 0, results: HashMap::new(), log: SessionLog::default(), extensions: HashMap::new(), extension_tests_req: None, configuration: None, storage: None, secrets: None, files: None }
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

    /// Starts the test runner named in init data (`extensionTestsLocationURI`); `pump` returns false once it is done.
    pub fn execute_extension_tests(&mut self) {
        self.extension_tests_req = Some(self.send_request("ExtHostExtensionService", "$extensionTestsExecute", vec![]));
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

    /// Sends a request without waiting for its reply.
    pub fn notify(&mut self, proxy: &str, method: &str, args: Vec<Value>) -> u32 {
        self.send_request(proxy, method, args.into_iter().map(Arg::Json).collect())
    }

    fn send_request(&mut self, proxy: &str, method: &str, args: Vec<Arg>) -> u32 {
        self.next_req += 1;
        let req = self.next_req;
        let message = Message::Request { req, rpc_id: proxy_id(proxy), method: method.into(), args, cancellable: false };
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

    /// Handles one incoming message, waiting up to `timeout`. Returns false once the host is gone or the extension tests are done.
    pub fn pump(&mut self, timeout: Duration) -> anyhow::Result<bool> {
        if let Some(args) = self.configuration.as_mut().and_then(MainThreadConfiguration::check_files) {
            self.notify("ExtHostConfiguration", "$acceptConfigurationChanged", args);
        }
        for event in self.files.as_mut().map(MainThreadFiles::take_events).unwrap_or_default() {
            self.notify("ExtHostFileSystemEventService", "$onFileEvent", vec![event]);
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
            Message::Request { req, rpc_id, method, args, .. } => {
                self.host.send(rpc::encode(&Message::Acknowledged { req }));
                let reply = self.handle_host_request(proxy_name(rpc_id), &method, &args);
                self.host.send(rpc::encode(&Message::ReplyOk { req, value: reply }));
            }
            Message::ReplyOk { req, value } => {
                self.results.insert(req, CallResult::Ok(value));
            }
            Message::ReplyErr { req, error } => {
                self.results.insert(req, CallResult::Err(error));
            }
            Message::Acknowledged { .. } | Message::Cancel { .. } => {}
        }
        Ok(self.extension_tests_req.is_none_or(|req| !self.results.contains_key(&req)))
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

/// A `SerializedError` (or any thrown value) as one string: its stack, else its message.
fn error_text(error: &Value) -> String {
    error["stack"].as_str().or(error["message"].as_str()).map_or_else(|| error.to_string(), str::to_owned)
}

/// `LogLevel` of platform/log/common/log.ts.
fn log_level_name(level: &Value) -> &'static str {
    match level.as_u64() {
        Some(0) => "off",
        Some(1) => "trace",
        Some(2) => "debug",
        Some(3) => "info",
        Some(4) => "warning",
        _ => "error",
    }
}
