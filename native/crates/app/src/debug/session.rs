//! A debug session's state (debugSession.ts `DebugSession`, the `Thread`, `StackFrame`, `Scope` and `Variable` of
//! debugModel.ts, the REPL elements of replModel.ts) and the path conversion of debugUtils.ts. The workbench drives
//! the protocol in `workbench/debug_service.rs`.

use std::collections::HashMap;

use futures::channel::oneshot;
use serde_json::{Value, json};

#[cfg(test)]
mod tests;

/// debug.ts `State`, as the `debugState` context key spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Inactive,
    Initializing,
    Stopped,
    Running,
}

impl State {
    pub fn context_value(self) -> &'static str {
        match self {
            State::Inactive => "inactive",
            State::Initializing => "initializing",
            State::Stopped => "stopped",
            State::Running => "running",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StackFrame {
    pub id: i64,
    pub name: String,
    /// The DAP `Source`, paths already converted to the workbench's.
    pub source: Value,
    pub line: u32,
    pub column: u32,
    pub presentation_hint: Option<String>,
    pub can_restart: bool,
}

impl StackFrame {
    /// The file the frame is in, when it is on disk.
    pub fn path(&self) -> Option<&str> {
        let reference = self.source["sourceReference"].as_i64().unwrap_or(0);
        self.source["path"].as_str().filter(|path| reference <= 0 && path.starts_with('/'))
    }

    /// `Source.name`, else the path's last segment.
    pub fn source_name(&self) -> String {
        match self.source["name"].as_str() {
            Some(name) => name.to_owned(),
            None => self.source["path"].as_str().and_then(|path| path.rsplit('/').next()).unwrap_or("Unknown Source").to_owned(),
        }
    }

    /// `presentationHint: 'subtle'` and `'label'` frames, and sources marked `deemphasize`, draw dimmed.
    pub fn deemphasized(&self) -> bool {
        matches!(self.presentation_hint.as_deref(), Some("subtle" | "deemphasize")) || self.source["presentationHint"] == "deemphasize"
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Thread {
    pub id: i64,
    pub name: String,
    pub stopped: bool,
    /// The `stopped` event's body while stopped.
    pub stopped_details: Value,
    pub frames: Vec<StackFrame>,
    /// `totalFrames` when the adapter reported more than were fetched.
    pub total_frames: Option<u64>,
}

impl Thread {
    /// callStackView.ts `getThreadLabel` state: "Paused on breakpoint", "Running".
    pub fn state_label(&self) -> String {
        if !self.stopped {
            return "Running".into();
        }
        let details = &self.stopped_details;
        if let Some(description) = details["description"].as_str() {
            return description.to_owned();
        }
        match details["reason"].as_str() {
            Some(reason) => format!("Paused on {reason}"),
            None => "Paused".into(),
        }
    }
}

/// A row of the Variables, Watch or REPL trees (`Variable`, `Scope`, `Expression`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub type_name: Option<String>,
    pub variables_reference: i64,
    pub evaluate_name: Option<String>,
    pub presentation_hint: Value,
    pub named_variables: Option<u64>,
    pub indexed_variables: Option<u64>,
    /// A scope the adapter marks `expensive` starts collapsed.
    pub expensive: bool,
    /// An evaluation that failed: its message shows in `debugTokenExpression.error`.
    pub error: bool,
}

impl Variable {
    pub fn from_dap(variable: &Value) -> Variable {
        let text = |key: &str| variable[key].as_str().map(str::to_owned);
        Variable {
            name: text("name").unwrap_or_default(),
            value: text("value").or(text("result")).unwrap_or_default(),
            type_name: text("type"),
            variables_reference: variable["variablesReference"].as_i64().unwrap_or(0),
            evaluate_name: text("evaluateName"),
            presentation_hint: variable["presentationHint"].clone(),
            named_variables: variable["namedVariables"].as_u64(),
            indexed_variables: variable["indexedVariables"].as_u64(),
            expensive: variable["expensive"] == true,
            error: false,
        }
    }

    pub fn has_children(&self) -> bool {
        self.variables_reference > 0
    }
}

/// replModel.ts severity of an output event's category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
    Ignore,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReplElement {
    /// `ReplOutputElement`: text of an output event, with the source it came from; a repeat counts up.
    Output { session_id: String, value: String, severity: Severity, source: Option<(String, u32)>, count: u32 },
    /// `ReplVariableElement`: an output event with `variablesReference`.
    Variable { session_id: String, variable: Variable, severity: Severity },
    /// `ReplEvaluationInput` and `ReplEvaluationResult`.
    Evaluation { session_id: String, input: String, result: Option<Variable> },
}

/// A pending DAP request: what answers it.
pub type Pending = oneshot::Sender<Result<Value, Value>>;

/// debugSession.ts `DebugSession` with its raw session's request state.
pub struct Session {
    pub id: String,
    /// The handle the extension host's debug adapter has.
    pub handle: u32,
    pub name: String,
    pub configuration: Value,
    pub unresolved_configuration: Value,
    pub folder_uri: Option<Value>,
    pub parent: Option<String>,
    /// `IDebugSessionOptions` the session started with (`repl`, `compact`, `lifecycleManagedByParent`, `suppress*`).
    pub options: Value,
    pub capabilities: Value,
    /// `initialized`: the adapter answered `initialize`.
    pub initialized: bool,
    /// The adapter sent its `initialized` event and got the breakpoints.
    pub configuration_done: bool,
    /// The adapter sent `initialized` before the launch request went out.
    pub initialized_event_pending: bool,
    pub terminated: bool,
    /// The adapter process or connection is gone.
    pub exited: bool,
    pub threads: Vec<Thread>,
    pub loaded_sources: Vec<Value>,
    seq: i64,
    pending: HashMap<i64, Pending>,
}

impl Session {
    pub fn new(id: String, handle: u32, configuration: Value, unresolved_configuration: Value, folder_uri: Option<Value>, options: Value) -> Session {
        Session {
            id,
            handle,
            name: configuration["name"].as_str().unwrap_or_default().to_owned(),
            parent: options["parentSessionID"].as_str().map(str::to_owned),
            configuration,
            unresolved_configuration,
            folder_uri,
            options,
            capabilities: json!({}),
            initialized: false,
            configuration_done: false,
            initialized_event_pending: false,
            terminated: false,
            exited: false,
            threads: Vec::new(),
            loaded_sources: Vec::new(),
            seq: 1,
            pending: HashMap::new(),
        }
    }

    /// `IDebugSessionDto` (`IDebugSessionFullDto`).
    pub fn dto(&self) -> Value {
        let mut dto = json!({ "id": self.id, "type": self.configuration["type"], "name": self.name, "parent": self.parent, "folderUri": self.folder_uri, "configuration": self.configuration });
        crate::debug::strip_nulls(&mut dto);
        dto
    }

    pub fn session_type(&self) -> &str {
        self.configuration["type"].as_str().unwrap_or_default()
    }

    /// `DebugSession.state` for `focused_thread` (of this session) or any thread.
    pub fn state(&self, focused_thread: Option<i64>) -> State {
        if !self.initialized {
            return State::Initializing;
        }
        if self.exited {
            return State::Inactive;
        }
        let stopped = match focused_thread.and_then(|id| self.threads.iter().find(|thread| thread.id == id)) {
            Some(thread) => thread.stopped,
            None => self.threads.iter().any(|thread| thread.stopped),
        };
        if stopped { State::Stopped } else { State::Running }
    }

    pub fn thread(&self, id: i64) -> Option<&Thread> {
        self.threads.iter().find(|thread| thread.id == id)
    }

    pub fn thread_mut(&mut self, id: i64) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|thread| thread.id == id)
    }

    /// `noDebug` sessions run without breakpoints.
    pub fn no_debug(&self) -> bool {
        self.configuration["noDebug"] == true
    }

    /// rawDebugSession.ts `send`: the request message with the next sequence number; its answer goes to `reply`.
    pub fn request(&mut self, command: &str, arguments: Value, reply: Pending) -> Value {
        let seq = self.seq;
        self.seq += 1;
        self.pending.insert(seq, reply);
        let mut request = json!({ "command": command, "type": "request", "seq": seq });
        if !arguments.is_null() {
            request["arguments"] = arguments;
        }
        request
    }

    /// `AbstractDebugAdapter.internalSend`: a response to the adapter takes the next sequence number too.
    pub fn number(&mut self, mut message: Value) -> Value {
        message["seq"] = json!(self.seq);
        self.seq += 1;
        message
    }

    /// A response for a request of ours: its `body`, or the error rawDebugSession.ts builds from it.
    pub fn accept_response(&mut self, response: &Value) {
        let Some(reply) = response["request_seq"].as_i64().and_then(|seq| self.pending.remove(&seq)) else { return };
        let result = match response["success"] == true {
            true => Ok(response["body"].clone()),
            false => Err(response_error(response)),
        };
        let _ = reply.send(result);
    }

    /// The adapter is gone: every request still waiting fails (rawDebugSession.ts `cancelPendingRequests`).
    pub fn cancel_pending(&mut self) {
        for (_, reply) in self.pending.drain() {
            let _ = reply.send(Err(json!({ "message": "Debug adapter process has terminated unexpectedly" })));
        }
    }

    /// `fetchThreads`: the threads the adapter lists, keeping what is known about the ones that stay.
    pub fn set_threads(&mut self, threads: &[Value]) {
        let mut next = Vec::new();
        for value in threads {
            let id = value["id"].as_i64().unwrap_or_default();
            let name = value["name"].as_str().unwrap_or_default().to_owned();
            match self.threads.iter().position(|thread| thread.id == id) {
                Some(index) => {
                    let mut thread = self.threads.remove(index);
                    thread.name = name;
                    next.push(thread);
                }
                None => next.push(Thread { id, name, ..Default::default() }),
            }
        }
        self.threads = next;
    }

    /// `stopped` event: the thread (or all with `allThreadsStopped`) stops with the event's details.
    pub fn stop_threads(&mut self, body: &Value) {
        let thread_id = body["threadId"].as_i64();
        let all = body["allThreadsStopped"] == true;
        for thread in &mut self.threads {
            if all || Some(thread.id) == thread_id {
                thread.stopped = true;
                thread.frames.clear();
                thread.total_frames = None;
                thread.stopped_details = if Some(thread.id) == thread_id || thread_id.is_none() { body.clone() } else { json!({ "reason": body["reason"] }) };
            }
        }
    }

    /// `continued` event or a step: the thread (or all) runs again and forgets its frames.
    pub fn continue_threads(&mut self, thread_id: Option<i64>) {
        for thread in self.threads.iter_mut().filter(|thread| thread_id.is_none_or(|id| id == thread.id)) {
            thread.stopped = false;
            thread.frames.clear();
            thread.total_frames = None;
            thread.stopped_details = Value::Null;
        }
    }

    /// `loadedSource` event: `new`, `changed` and `removed` by path or reference.
    pub fn accept_loaded_source(&mut self, body: &Value) {
        let source = &body["source"];
        let same = |known: &Value| known["path"] == source["path"] && known["sourceReference"] == source["sourceReference"];
        match body["reason"].as_str() {
            Some("removed") => self.loaded_sources.retain(|known| !same(known)),
            Some("changed") => {
                if let Some(known) = self.loaded_sources.iter_mut().find(|known| same(known)) {
                    *known = source.clone();
                }
            }
            _ => {
                if !self.loaded_sources.iter().any(same) {
                    self.loaded_sources.push(source.clone());
                }
            }
        }
    }
}

/// rawDebugSession.ts `send` error: the formatted `body.error` or the response `message`.
pub fn response_error(response: &Value) -> Value {
    let error = &response["body"]["error"];
    let message = match error["format"].as_str() {
        Some(format) => format_message(format, &error["variables"]),
        None => response["message"].as_str().unwrap_or("").to_owned(),
    };
    json!({ "message": message, "showUser": error["showUser"] })
}

/// rawDebugSession.ts `formatPII`: `{name}` placeholders from `variables`, unknown names left as they are.
fn format_message(format: &str, variables: &Value) -> String {
    let mut result = String::new();
    let mut rest = format;
    while let Some(start) = rest.find('{') {
        result.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else { break };
        let name = &rest[start + 1..start + end];
        match variables[name].as_str() {
            Some(value) => result.push_str(value),
            None => result.push_str(&rest[start..start + end + 1]),
        }
        rest = &rest[start + end + 1..];
    }
    result.push_str(rest);
    result
}

/// The repl's severity of an `output` event category (replModel.ts `appendToRepl` callers in debugSession.ts).
pub fn output_severity(category: Option<&str>) -> Severity {
    match category {
        Some("stderr") => Severity::Error,
        Some("console") => Severity::Warning,
        Some("telemetry") => Severity::Ignore,
        _ => Severity::Info,
    }
}

/// replModel.ts `appendToRepl` of text: it continues the last output element while that one has no line end and
/// the same severity and source; a repeat of the last whole line counts up instead (`ReplOutputElement.count`).
pub fn append_output(repl: &mut Vec<ReplElement>, session_id: &str, text: &str, severity: Severity, source: Option<(String, u32)>) {
    if text.is_empty() {
        return;
    }
    if let Some(ReplElement::Output { value, severity: last_severity, source: last_source, count, session_id: last_session }) = repl.last_mut()
        && *last_severity == severity
        && *last_source == source
        && last_session == session_id
    {
        if !value.ends_with('\n') {
            value.push_str(text);
            return;
        }
        if value == text {
            *count += 1;
            return;
        }
    }
    repl.push(ReplElement::Output { session_id: session_id.to_owned(), value: text.to_owned(), severity, source, count: 1 });
}

/// debugUtils.ts `convertToDAPaths(message, true)`: the source paths of requests become `UriComponents`.
pub fn to_adapter_paths(message: &Value) -> Value {
    let mut message = message.clone();
    if message["type"] == "request" {
        let command = message["command"].as_str().unwrap_or_default().to_owned();
        match command.as_str() {
            "setBreakpoints" | "breakpointLocations" | "source" | "gotoTargets" => path_to_uri(&mut message["arguments"]["source"]),
            _ => {}
        }
    }
    message
}

/// debugUtils.ts `convertToVSCPaths(message, false)`: the `UriComponents` of events and responses become paths.
pub fn from_adapter_paths(message: &Value) -> Value {
    let mut message = message.clone();
    match message["type"].as_str() {
        Some("event") => match message["event"].as_str().unwrap_or_default() {
            "output" | "loadedSource" => uri_to_path(&mut message["body"]["source"]),
            "breakpoint" => uri_to_path(&mut message["body"]["breakpoint"]["source"]),
            _ => {}
        },
        Some("response") if message["success"] == true => {
            let command = message["command"].as_str().unwrap_or_default().to_owned();
            let body = &mut message["body"];
            let each = |list: &mut Value, key: Option<&str>| {
                for item in list.as_array_mut().into_iter().flatten() {
                    match key {
                        Some(key) => uri_to_path(&mut item[key]),
                        None => uri_to_path(item),
                    }
                }
            };
            match command.as_str() {
                "stackTrace" => each(&mut body["stackFrames"], Some("source")),
                "loadedSources" => each(&mut body["sources"], None),
                "scopes" => each(&mut body["scopes"], Some("source")),
                "setFunctionBreakpoints" | "setBreakpoints" => each(&mut body["breakpoints"], Some("source")),
                "disassemble" => each(&mut body["instructions"], Some("location")),
                "locations" => uri_to_path(&mut body["source"]),
                _ => {}
            }
        }
        _ => {}
    }
    message
}

/// `stringToUri`: an absolute path or a URI string, unless the source has a reference.
fn path_to_uri(source: &mut Value) {
    if source["sourceReference"].as_i64().is_some_and(|reference| reference > 0) {
        return;
    }
    let Some(path) = source["path"].as_str() else { return };
    let uri = if let Some((scheme, rest)) = path.split_once(':').filter(|(scheme, _)| is_uri_scheme(scheme)) {
        let rest = rest.strip_prefix("//").unwrap_or(rest);
        let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        json!({ "$mid": 1, "scheme": scheme, "authority": authority, "path": percent_decode(path), "query": "", "fragment": "" })
    } else if path.starts_with('/') {
        json!({ "$mid": 1, "scheme": "file", "authority": "", "path": path, "query": "", "fragment": "" })
    } else {
        return;
    };
    source["path"] = uri;
}

/// `uriToString`: a `file` URI becomes its fsPath, others their string.
fn uri_to_path(source: &mut Value) {
    if !source["path"].is_object() {
        return;
    }
    let uri = source["path"].clone();
    source["path"] = match uri["scheme"].as_str() {
        Some("file") => uri["path"].clone(),
        _ => json!(crate::debug::uri_string(&uri)),
    };
}

/// debugUtils.ts `isUriString`: a scheme of two or more characters.
fn is_uri_scheme(scheme: &str) -> bool {
    scheme.len() >= 2 && scheme.starts_with(|c: char| c.is_ascii_alphabetic()) && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
}

pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match (bytes[index], text.get(index + 1..index + 3).and_then(|hex| u8::from_str_radix(hex, 16).ok())) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                index += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}
