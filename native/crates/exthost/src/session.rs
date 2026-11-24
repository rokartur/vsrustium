//! Main-thread side of one extension host session: init data, the startup calls the
//! workbench's MainThread* actors make, and a dispatcher for requests from the host.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::console::format_console_entry;
use crate::rpc::{self, Arg, Message, Reply};
use crate::scan::{ScannedExtension, extension_identifier, file_uri};
use crate::{ExtensionHost, Incoming, proxy_id, proxy_name};

pub struct SessionPaths<'a> {
    pub app_root: &'a Path,
    pub user_data: &'a Path,
    pub workspace_folder: Option<&'a Path>,
}

/// `IExtensionHostInitData` for a desktop window. Telemetry identifiers are fixed placeholders:
/// the port never sends them anywhere, and extensions reading `env.machineId` get the same value as upstream's web build.
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
        "logsLocation": file_uri(&paths.user_data.join("logs/exthost")),
        // The local host starts EagerManualStart: the main side sends $startExtensionHost.
        "autoStart": false,
        "remote": { "isRemote": false, "authority": null, "connectionData": null },
        "consoleForward": { "includeStack": false, "logNative": false },
        "uiKind": 1,
    })
}

/// The `IWorkspaceData` of a single-folder workspace for `$initializeWorkspace`, with the id and name `init_data` gave it.
pub fn workspace_data(init: &Value, folder: &Path) -> Value {
    let name = &init["workspace"]["name"];
    json!({ "id": init["workspace"]["id"], "name": name, "folders": [{ "uri": file_uri(folder), "name": name, "index": 0 }] })
}

fn workspace_id(folder: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    folder.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// `IConfigurationInitData` with the defaults that extensions contribute; core settings are not registered yet.
pub fn configuration_init_data(extensions: &[ScannedExtension]) -> Value {
    let mut contents = serde_json::Map::new();
    let mut keys = Vec::new();
    let mut scopes = Vec::new();
    for extension in extensions {
        let configuration = &extension.description["contributes"]["configuration"];
        let sections: Vec<&Value> = match configuration {
            Value::Array(sections) => sections.iter().collect(),
            Value::Object(_) => vec![configuration],
            _ => continue,
        };
        for section in sections {
            let Some(properties) = section["properties"].as_object() else { continue };
            for (key, schema) in properties {
                if let Some(default) = schema.get("default") {
                    insert_dotted(&mut contents, key, default.clone());
                }
                keys.push(json!(key));
                scopes.push(json!([key, configuration_scope(schema.get("scope").and_then(Value::as_str))]));
            }
        }
    }
    let empty = json!({ "contents": {}, "keys": [], "overrides": [] });
    json!({
        "defaults": { "contents": contents, "keys": keys, "overrides": [] },
        "policy": empty,
        "application": empty,
        "userLocal": empty,
        "userRemote": empty,
        "workspace": empty,
        "folders": [],
        "configurationScopes": scopes,
    })
}

/// `ConfigurationScope` from configurationRegistry.ts; unset means WINDOW (4).
fn configuration_scope(scope: Option<&str>) -> u8 {
    match scope {
        Some("application") => 1,
        Some("machine") => 2,
        Some("application-machine") => 3,
        Some("resource") => 5,
        Some("language-overridable") => 6,
        Some("machine-overridable") => 7,
        _ => 4,
    }
}

fn insert_dotted(root: &mut serde_json::Map<String, Value>, key: &str, value: Value) {
    let mut parts = key.split('.').peekable();
    let mut node = root;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            node.insert(part.to_owned(), value);
            return;
        }
        let child = node.entry(part.to_owned()).or_insert_with(|| json!({}));
        if !child.is_object() {
            *child = json!({});
        }
        node = child.as_object_mut().unwrap();
    }
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
    forward: Option<Forward>,
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
}

impl Session {
    pub fn new(host: ExtensionHost) -> Self {
        Session { host, next_req: 0, results: HashMap::new(), log: SessionLog::default(), extensions: HashMap::new(), extension_tests_req: None, forward: None }
    }

    /// The calls the workbench makes once the handshake is done (MainThreadConfiguration,
    /// MainThreadWorkspace, MainThreadTelemetry constructors, then the extension service).
    pub fn start(&mut self, extensions: &[ScannedExtension], workspace: Option<Value>) -> anyhow::Result<()> {
        self.extensions = extensions.iter().map(|e| (e.id.to_lowercase(), e.description.clone())).collect();
        self.notify("ExtHostConfiguration", "$initializeConfiguration", vec![configuration_init_data(extensions)]);
        self.notify("ExtHostWorkspace", "$initializeWorkspace", vec![workspace.unwrap_or(Value::Null), json!(true)]);
        // TelemetryLevel.NONE and supportsTelemetry = false: the port has no telemetry.
        self.notify("ExtHostTelemetry", "$initializeTelemetryLevel", vec![json!(0), json!(false), json!({ "usage": false, "error": false })]);
        // MainThreadEditorTabs sends the model on construction; an empty window has one active group with no tabs.
        self.notify("ExtHostEditorTabs", "$acceptEditorTabModel", vec![json!([{ "isActive": true, "viewColumn": 0, "groupId": 0, "tabs": [] }])]);
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

    /// Hands requests to `proxies` to `send` instead of answering them here; `pump` sends the answers that arrive on `replies`.
    pub fn forward(&mut self, proxies: &'static [&'static str], send: impl FnMut(ForwardedRequest) + Send + 'static, replies: std::sync::mpsc::Receiver<(u32, Result<Value, Value>)>) {
        self.forward = Some(Forward { proxies, send: Box::new(send), replies });
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

    fn send_request(&mut self, proxy: &str, method: &str, args: Vec<Arg>) -> u32 {
        self.next_req += 1;
        let req = self.next_req;
        let message = Message::Request { req, rpc_id: proxy_id(proxy), method: method.into(), args, cancellable: false };
        self.host.send(rpc::encode(&message));
        req
    }

    /// Sends a request and serves host requests until its reply arrives.
    pub fn call(&mut self, proxy: &str, method: &str, args: Vec<Arg>, timeout: Duration) -> anyhow::Result<Reply> {
        let req = self.send_request(proxy, method, args);
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(result) = self.results.remove(&req) {
                return match result {
                    CallResult::Ok(reply) => Ok(reply),
                    CallResult::Err(error) => anyhow::bail!("{proxy}.{method} failed: {}", error.unwrap_or(Value::Null)),
                };
            }
            let left = deadline.checked_duration_since(Instant::now()).ok_or_else(|| anyhow::anyhow!("{proxy}.{method} timed out"))?;
            if !self.pump(left)? {
                anyhow::bail!("extension host exited during {proxy}.{method}");
            }
        }
    }

    /// Handles one incoming message, waiting up to `timeout`. Returns false once the host is gone or the extension tests are done.
    pub fn pump(&mut self, mut timeout: Duration) -> anyhow::Result<bool> {
        if let Some(forward) = &self.forward {
            let answered: Vec<_> = forward.replies.try_iter().collect();
            for (req, result) in answered {
                self.reply(req, result);
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
            Message::Request { req, rpc_id, method, args, .. } => {
                self.host.send(rpc::encode(&Message::Acknowledged { req }));
                if let Some(forward) = self.forward.as_mut().filter(|forward| forward.proxies.contains(&proxy_name(rpc_id))) {
                    *self.log.host_calls.entry(format!("{}.{method}", proxy_name(rpc_id))).or_default() += 1;
                    let args = args.into_iter().map(arg_value).collect();
                    (forward.send)(ForwardedRequest { req, proxy: proxy_name(rpc_id), method, args });
                    return Ok(true);
                }
                let reply = self.handle_host_request(proxy_name(rpc_id), &method, &args);
                self.host.send(rpc::encode(&Message::ReplyOk { req, value: reply }));
            }
            Message::ReplyOk { req, value } => {
                self.results.insert(req, CallResult::Ok(value));
            }
            Message::ReplyErr { req, error } => {
                self.results.insert(req, CallResult::Err(error));
            }
            // The forwarded request's cancellation token fired: it comes again as `$cancel`, with no proxy.
            Message::Cancel { req } => {
                if let Some(forward) = self.forward.as_mut() {
                    (forward.send)(ForwardedRequest { req, proxy: "", method: "$cancel".into(), args: Vec::new() });
                }
            }
            Message::Acknowledged { .. } => {}
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

fn arg_value(arg: Arg) -> Value {
    match arg {
        Arg::Json(value) | Arg::JsonWithBuffers { json: value, .. } => value,
        Arg::Buffer(_) | Arg::Undefined => Value::Null,
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
