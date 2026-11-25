//! The debug service of the workbench: debugService.ts (start, stop, focus, breakpoints sent to sessions),
//! debugSession.ts and rawDebugSession.ts (the protocol with one adapter), debugConfigurationManager.ts (launch
//! configurations, configuration providers) and mainThreadDebugService.ts (the extension host's side). Every debug
//! adapter runs in the extension host (`ExtHostDebugService`), as on desktop: inline, executable and server adapters
//! alike reach this file as `$acceptDAMessage`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use exthost::session::ForwardedRequest;
use futures::channel::oneshot;
use gpui::{AsyncWindowContext, Context, WeakEntity, Window};
use serde_json::{Map, Value, json};

use super::Workbench;
use crate::commands::{Outcome, error_value};
use crate::debug::configuration::{self, Debugger, Interactive, Launch};
use crate::debug::session::{self, ReplElement, Session, Severity, StackFrame, State, Variable};
use crate::debug::{BreakpointData, BreakpointsChange, DataBreakpoint, DebugModel};
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Entry, Event, Item, Kind, QuickInput};

type Reply = oneshot::Receiver<Result<Value, Value>>;

/// debugStorage.ts `DEBUG_CHOSEN_ENVIRONMENTS_KEY`, and debugConfigurationManager.ts's selected launch.
const CHOSEN_ENVIRONMENTS_KEY: &str = "debug.chosenenvironment";
const SELECTED_CONFIG_KEY: &str = "debug.selectedconfigname";
const SELECTED_ROOT_KEY: &str = "debug.selectedroot";

/// A `$registerDebugConfigurationProvider` registration.
#[derive(Clone, Debug)]
pub struct ConfigurationProvider {
    pub handle: i64,
    pub debug_type: String,
    pub has_resolve: bool,
    pub has_resolve2: bool,
}

/// The focus of debugViewModel.ts: a session, a thread of it and a frame of that thread.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DebugFocus {
    pub session: Option<String>,
    pub thread: Option<i64>,
    pub frame: Option<i64>,
}

#[derive(Default)]
pub struct DebugService {
    pub model: DebugModel,
    pub debuggers: Vec<Debugger>,
    /// The types whose adapters the extension host creates (`$registerDebugTypes`).
    pub adapter_types: Vec<String>,
    pub providers: Vec<ConfigurationProvider>,
    pub sessions: Vec<Session>,
    next_handle: u32,
    pub focus: DebugFocus,
    /// The scopes of the focused frame, then each expanded row's children by `(session, variablesReference)`.
    pub scopes: Vec<Variable>,
    pub children: HashMap<(String, i64), Vec<Variable>>,
    /// Watch expression results by expression id.
    pub watch_values: HashMap<String, Variable>,
    pub repl: Vec<ReplElement>,
    /// Sessions the extension host caches (`$sessionCached`): their DTO is the id alone.
    known_sessions: HashSet<String>,
    /// `debugService.initializing`: a start is resolving its configuration.
    pub initializing: bool,
    /// `viewModel.firstSessionStart`.
    pub first_session_started: bool,
    /// `chosenEnvironments`: the debugger picked for a file without a launch configuration.
    chosen_environments: Map<String, Value>,
    /// The launch configuration the Run and Debug view's dropdown selects: its folder and name.
    pub selected: Option<(Option<PathBuf>, String)>,
}

impl DebugService {
    pub fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|session| session.id == id)
    }

    pub fn session_mut(&mut self, id: &str) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|session| session.id == id)
    }

    fn session_by_handle(&self, handle: u32) -> Option<&Session> {
        self.sessions.iter().find(|session| session.handle == handle)
    }

    pub fn focused_session(&self) -> Option<&Session> {
        self.focus.session.as_deref().and_then(|id| self.session(id))
    }

    pub fn focused_frame(&self) -> Option<&StackFrame> {
        let session = self.focused_session()?;
        let thread = session.thread(self.focus.thread?)?;
        thread.frames.iter().find(|frame| Some(frame.id) == self.focus.frame)
    }

    /// debugService.ts `state`: the focused session's, else initializing while a start resolves.
    pub fn state(&self) -> State {
        if let Some(session) = self.focused_session() {
            return session.state(self.focus.thread);
        }
        if self.initializing || self.sessions.iter().any(|session| session.state(None) == State::Initializing) {
            return State::Initializing;
        }
        State::Inactive
    }

    /// `getSessionDto`: the id alone once the host cached the session.
    fn session_dto(&self, session: &Session) -> Value {
        match self.known_sessions.contains(&session.id) {
            true => json!(session.id),
            false => session.dto(),
        }
    }

    pub fn debugger(&self, debug_type: &str) -> Option<&Debugger> {
        self.debuggers.iter().find(|debugger| debugger.debug_type == debug_type)
    }

    /// debugViewModel.ts `getStackFrameThreadAndSessionToFocus`: the first stopped thread's top frame with a source.
    fn focus_target(&self, session_id: Option<&str>, thread_id: Option<i64>) -> DebugFocus {
        let session = session_id.and_then(|id| self.session(id)).or_else(|| self.sessions.iter().find(|session| session.state(None) == State::Stopped)).or(self.sessions.first());
        let Some(session) = session else { return DebugFocus::default() };
        let thread = thread_id.and_then(|id| session.thread(id)).or_else(|| session.threads.iter().find(|thread| thread.stopped)).or(session.threads.first());
        let Some(thread) = thread else { return DebugFocus { session: Some(session.id.clone()), ..Default::default() } };
        let frame = thread.frames.iter().find(|frame| frame.path().is_some() && !frame.deemphasized()).or(thread.frames.first());
        DebugFocus { session: Some(session.id.clone()), thread: Some(thread.id), frame: frame.map(|frame| frame.id) }
    }
}

/// The message of an error the host or an adapter answered.
fn error_message(error: &Value) -> String {
    error["message"].as_str().map(str::to_owned).unwrap_or_else(|| error.to_string())
}

async fn answer(reply: Reply) -> Result<Value, String> {
    match reply.await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error_message(&error)),
        Err(_) => Err("Canceled".into()),
    }
}

/// An async step on the workbench; a closed window ends the flow.
fn on_workbench<R>(this: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, step: impl FnOnce(&mut Workbench, &mut Window, &mut Context<Workbench>) -> R) -> Result<R, String> {
    this.update_in(cx, step).map_err(|_| "Canceled".to_owned())
}

impl Workbench {
    /// MainThreadDebugService's methods.
    pub(crate) fn handle_debug_request(&mut self, request: ForwardedRequest, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let args = request.args;
        let arg = |index: usize| args.get(index).cloned().unwrap_or(Value::Null);
        match request.method.as_str() {
            "$registerDebugTypes" => {
                self.debug.adapter_types = arg(0).as_array().into_iter().flatten().filter_map(|value| value.as_str().map(str::to_owned)).collect();
                self.debug.debuggers = configuration::debuggers(&self.extensions);
            }
            "$sessionCached" => _ = self.debug.known_sessions.insert(arg(0).as_str().unwrap_or_default().to_owned()),
            "$acceptDAMessage" => self.accept_adapter_message(arg(0).as_u64().unwrap_or_default() as u32, &session::from_adapter_paths(&arg(1)), window, cx),
            "$acceptDAError" => {
                let handle = arg(0).as_u64().unwrap_or_default() as u32;
                let message = arg(2).as_str().unwrap_or_default().to_owned();
                self.adapter_exited(handle, Some(message), cx);
            }
            "$acceptDAExit" => self.adapter_exited(arg(0).as_u64().unwrap_or_default() as u32, None, cx),
            "$registerDebugConfigurationProvider" => self.debug.providers.push(ConfigurationProvider {
                debug_type: arg(0).as_str().unwrap_or_default().to_owned(),
                has_resolve: arg(3) == true,
                has_resolve2: arg(4) == true,
                handle: arg(5).as_i64().unwrap_or_default(),
            }),
            "$unregisterDebugConfigurationProvider" => self.debug.providers.retain(|provider| Some(provider.handle) != arg(0).as_i64()),
            "$registerDebugAdapterDescriptorFactory" | "$unregisterDebugAdapterDescriptorFactory" => {}
            "$startDebugging" => {
                let folder = crate::debug::configuration_folder(&arg(0));
                let mut options = arg(2);
                // mainThreadDebugService.ts: the host's options carry the parent's id, the session wants the parent.
                if options.is_null() {
                    options = json!({});
                }
                return Outcome::Pending(self.start_debugging(folder, arg(1), options, window, cx));
            }
            "$stopDebugging" => return Outcome::Pending(self.stop_session(arg(0).as_str().map(str::to_owned), false, window, cx)),
            "$setDebugSessionName" => {
                let id = arg(0).as_str().unwrap_or_default().to_owned();
                let name = arg(1).as_str().unwrap_or_default().to_owned();
                if let Some(session) = self.debug.session_mut(&id) {
                    session.name = name.clone();
                    let dto = session.dto();
                    self.host.send("ExtHostDebugService", "$acceptDebugSessionNameChanged", vec![dto, json!(name)]);
                    cx.notify();
                }
            }
            "$customDebugAdapterRequest" => {
                let id = arg(0).as_str().unwrap_or_default().to_owned();
                if self.debug.session(&id).is_none() {
                    return Outcome::error("debug session not found");
                }
                return Outcome::Pending(self.dap_request(&id, arg(1).as_str().unwrap_or_default(), arg(2)));
            }
            "$getDebugProtocolBreakpoint" => {
                let (session, id) = (arg(0).as_str().unwrap_or_default().to_owned(), arg(1).as_str().unwrap_or_default().to_owned());
                let model = &self.debug.model;
                let bases = model.breakpoints.iter().map(|breakpoint| &breakpoint.base).chain(model.function_breakpoints.iter().map(|breakpoint| &breakpoint.base));
                let found = bases.chain(model.data_breakpoints.iter().map(|breakpoint| &breakpoint.base)).find(|base| base.id == id).and_then(|base| base.debug_protocol_breakpoint(&session));
                return Outcome::Ready(Ok(found.unwrap_or(Value::Null)));
            }
            "$appendDebugConsole" => {
                let session_id = self.debug.focus.session.clone().unwrap_or_default();
                self.append_to_repl(&session_id, arg(0).as_str().unwrap_or_default(), Severity::Info, None, cx);
            }
            "$registerBreakpoints" => self.register_host_breakpoints(arg(0).as_array().map_or(&[], Vec::as_slice), cx),
            "$unregisterBreakpoints" => {
                let ids = |index: usize| -> Vec<String> { arg(index).as_array().into_iter().flatten().filter_map(|id| id.as_str().map(str::to_owned)).collect() };
                let mut change = self.debug.model.remove_breakpoints(&ids(0));
                for id in ids(1) {
                    change.removed.extend(self.debug.model.remove_function_breakpoints(Some(&id)).removed);
                }
                for id in ids(2) {
                    change.removed.extend(self.debug.model.remove_data_breakpoints(Some(&id)).removed);
                }
                self.breakpoints_changed(change, true, cx);
            }
            // Visualizers serve the chat-era "visualize" UI of the variables view, which has no desktop caller here.
            _ => {}
        }
        Outcome::done()
    }

    /// `$registerBreakpoints`: source breakpoints by resource, function and data breakpoints.
    fn register_host_breakpoints(&mut self, dtos: &[Value], cx: &mut Context<Self>) {
        let mut change = BreakpointsChange::default();
        let text = |value: &Value, key: &str| value[key].as_str().map(str::to_owned);
        for dto in dtos {
            match dto["type"].as_str() {
                Some("sourceMulti") => {
                    let data = dto["lines"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|line| BreakpointData {
                            id: text(line, "id"),
                            line: line["line"].as_u64().unwrap_or_default() as u32 + 1,
                            column: line["character"].as_u64().filter(|character| *character > 0).map(|character| character as u32 + 1),
                            enabled: line["enabled"] != false,
                            condition: text(line, "condition"),
                            hit_condition: text(line, "hitCondition"),
                            log_message: text(line, "logMessage"),
                            mode: text(line, "mode"),
                            mode_label: None,
                            triggered_by: None,
                        })
                        .collect();
                    change.added.extend(self.debug.model.add_breakpoints(&dto["uri"], data).added);
                }
                Some("function") => {
                    let options = json!({ "enabled": dto["enabled"], "condition": dto["condition"], "hitCondition": dto["hitCondition"], "logMessage": dto["logMessage"], "mode": dto["mode"] });
                    change.added.extend(self.debug.model.add_function_breakpoint(dto["functionName"].as_str().unwrap_or_default(), &options, text(dto, "id")).added);
                }
                Some("data") => {
                    let options = json!({ "enabled": dto["enabled"], "condition": dto["condition"], "hitCondition": dto["hitCondition"], "logMessage": dto["logMessage"] });
                    let breakpoint = DataBreakpoint {
                        base: crate::debug::base_breakpoint(text(dto, "id").unwrap_or_else(crate::debug::generate_uuid), &options),
                        description: text(dto, "label").unwrap_or_default(),
                        data_id: text(dto, "dataId").unwrap_or_default(),
                        can_persist: dto["canPersist"] == true,
                        access_type: text(dto, "accessType").unwrap_or_else(|| "write".into()),
                        access_types: None,
                    };
                    change.added.extend(self.debug.model.add_data_breakpoint(breakpoint).added);
                }
                _ => {}
            }
        }
        // The host already knows these breakpoints; it ignores the delta of ids it has.
        self.breakpoints_changed(change, true, cx);
    }

    /// The model changed: storage, the extension host (unless only sessions changed) and the sessions.
    pub(crate) fn breakpoints_changed(&mut self, change: BreakpointsChange, send_to_sessions: bool, cx: &mut Context<Self>) {
        self.store_debug_model();
        if !change.session_only && !change.is_empty() {
            let mut delta = Map::new();
            if !change.added.is_empty() {
                delta.insert("added".into(), json!(self.debug.model.dtos(&change.added)));
            }
            if !change.removed.is_empty() {
                delta.insert("removed".into(), json!(change.removed));
            }
            if !change.changed.is_empty() {
                delta.insert("changed".into(), json!(self.debug.model.dtos(&change.changed)));
            }
            self.host.send("ExtHostDebugService", "$acceptBreakpointsDelta", vec![Value::Object(delta)]);
        }
        if send_to_sessions && !change.session_only {
            let ids: Vec<String> = self.debug.sessions.iter().map(|session| session.id.clone()).collect();
            for id in ids {
                self.send_all_breakpoints(&id, cx);
            }
        }
        cx.notify();
    }

    /// debugStorage.ts `storeBreakpoints` and `storeWatchExpressions`.
    pub(crate) fn store_debug_model(&mut self) {
        let mut storage = self.workspace_storage.lock().unwrap();
        for (key, value) in self.debug.model.storage_entries() {
            match value {
                Some(value) => storage.set(key, value),
                None => storage.delete(key),
            }
        }
    }

    /// DebugStorage's load with the workspace state; the host hears of the stored breakpoints
    /// (mainThreadDebugService.ts `sendBreakpointsAndListen`).
    pub(crate) fn restore_debug_state(&mut self) {
        let storage = self.workspace_storage.lock().unwrap();
        self.debug.model = DebugModel::from_storage(|key| storage.get(key).map(str::to_owned));
        self.debug.chosen_environments = storage.get(CHOSEN_ENVIRONMENTS_KEY).and_then(|raw| serde_json::from_str(raw).ok()).unwrap_or_default();
        let name = storage.get(SELECTED_CONFIG_KEY).map(str::to_owned);
        let root = storage.get(SELECTED_ROOT_KEY).and_then(crate::editor_history::file_path);
        drop(storage);
        self.debug.selected = name.map(|name| (root, name));
        let ids = self.debug.model.all_ids();
        if !ids.is_empty() {
            self.host.send("ExtHostDebugService", "$acceptBreakpointsDelta", vec![json!({ "added": self.debug.model.dtos(&ids) })]);
        }
        self.update_debug_status();
    }

    pub(crate) fn store_selected_configuration(&mut self) {
        let mut storage = self.workspace_storage.lock().unwrap();
        match &self.debug.selected {
            Some((root, name)) => {
                storage.set(SELECTED_CONFIG_KEY, name.clone());
                match root {
                    Some(root) => storage.set(SELECTED_ROOT_KEY, crate::editor_history::file_uri_string(root)),
                    None => storage.delete(SELECTED_ROOT_KEY),
                }
            }
            None => {
                storage.delete(SELECTED_CONFIG_KEY);
                storage.delete(SELECTED_ROOT_KEY);
            }
        }
        drop(storage);
        self.update_debug_status();
    }

    /// The launch.json of a workspace folder (`Launch.getConfig`: the folder's value of the `launch` setting).
    pub(crate) fn folder_launch(&self, folder: &std::path::Path) -> Launch {
        let settings = self.settings.lock().unwrap();
        let overrides = config::configuration::Overrides { resource: Some(folder), override_identifier: None };
        let inspected = settings.configuration.inspect("launch", &overrides);
        Launch::from_section(inspected.workspace_folder.value.or(inspected.workspace.value).as_ref())
    }

    pub(crate) fn workspace_folders(&self) -> Vec<PathBuf> {
        self.workspace().map(|workspace| workspace.folders.iter().map(|folder| folder.path.clone()).collect()).unwrap_or_default()
    }

    /// rawDebugSession.ts `send`: a request to the session's adapter; the answer is its body or the adapter's error.
    pub(crate) fn dap_request(&mut self, session_id: &str, command: &str, arguments: Value) -> Reply {
        let (sender, receiver) = oneshot::channel();
        let Some(session) = self.debug.session_mut(session_id).filter(|session| !session.exited) else {
            let _ = sender.send(Err(json!({ "message": format!("No debugger available, can not send '{command}'") })));
            return receiver;
        };
        let message = session.request(command, arguments, sender);
        let handle = session.handle;
        self.host.send("ExtHostDebugService", "$sendDAMessage", vec![json!(handle), session::to_adapter_paths(&message)]);
        receiver
    }

    fn send_response(&mut self, session_id: &str, request: &Value, result: Result<Value, String>) {
        let Some(session) = self.debug.session_mut(session_id) else { return };
        let mut response = json!({ "type": "response", "seq": 0, "command": request["command"], "request_seq": request["seq"], "success": result.is_ok() });
        match result {
            Ok(body) if !body.is_null() => response["body"] = body,
            Ok(_) => {}
            Err(message) => response["message"] = json!(message),
        }
        let handle = session.handle;
        let response = session.number(response);
        self.host.send("ExtHostDebugService", "$sendDAMessage", vec![json!(handle), response]);
    }

    /// One message of an adapter: a response, an event, or a reverse request.
    fn accept_adapter_message(&mut self, handle: u32, message: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session_id) = self.debug.session_by_handle(handle).map(|session| session.id.clone()) else { return };
        match message["type"].as_str() {
            Some("response") => {
                if let Some(session) = self.debug.session_mut(&session_id) {
                    session.accept_response(message);
                }
            }
            Some("event") => self.accept_adapter_event(&session_id, message, window, cx),
            Some("request") => self.accept_reverse_request(&session_id, message, window, cx),
            _ => {}
        }
    }

    /// rawDebugSession.ts `dispatchRequest`: `runInTerminal` goes to the host, `startDebugging` starts a child.
    fn accept_reverse_request(&mut self, session_id: &str, request: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let arguments = request["arguments"].clone();
        let answer_with = |reply: Reply, session_id: String, request: Value, body: fn(Value) -> Result<Value, String>, window: &mut Window, cx: &mut Context<Self>| {
            cx.spawn_in(window, async move |this, cx| {
                let result = answer(reply).await.and_then(body);
                let _ = on_workbench(&this, cx, |workbench, _, _| workbench.send_response(&session_id, &request, result));
            })
            .detach();
        };
        match request["command"].as_str() {
            Some("runInTerminal") => {
                let reply = self.host.call("ExtHostDebugService", "$runInTerminal", vec![arguments, json!(session_id)]);
                answer_with(reply, session_id.to_owned(), request.clone(), |pid| Ok(if pid.is_number() { json!({ "shellProcessId": pid }) } else { json!({}) }), window, cx);
            }
            Some("startDebugging") => {
                let Some(session) = self.debug.session(session_id) else { return };
                let mut configuration = arguments["configuration"].clone();
                configuration["request"] = arguments["request"].clone();
                configuration["type"] = json!(session.session_type());
                if configuration["name"].as_str().is_none() {
                    configuration["name"] = json!(session.name);
                }
                let folder = session.folder_uri.as_ref().and_then(crate::debug::configuration_folder);
                let reply = self.start_debugging(folder, configuration, json!({ "parentSessionID": session_id }), window, cx);
                answer_with(reply, session_id.to_owned(), request.clone(), |started| if started == true { Ok(Value::Null) } else { Err("Failed to start debugging".into()) }, window, cx);
            }
            _ => self.send_response(session_id, request, Err(format!("Unknown request '{}'", request["command"].as_str().unwrap_or_default()))),
        }
    }

    /// rawDebugSession.ts's event switch with debugSession.ts's listeners.
    fn accept_adapter_event(&mut self, session_id: &str, event: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let body = &event["body"];
        match event["event"].as_str().unwrap_or_default() {
            // An `initialized` event in the same batch as the initialize response waits for the launch request, which
            // upstream sends in the response's continuation, before any other message.
            "initialized" => match self.debug.session_mut(session_id) {
                Some(session) if !session.initialized => session.initialized_event_pending = true,
                _ => self.session_initialized(session_id.to_owned(), window, cx),
            },
            "stopped" => self.session_stopped(session_id.to_owned(), body.clone(), window, cx),
            "continued" => {
                let thread = if body["allThreadsContinued"] == false { body["threadId"].as_i64() } else { None };
                self.threads_continued(session_id, thread, cx);
            }
            "thread" => match body["reason"].as_str() {
                Some("exited") => {
                    if let Some(session) = self.debug.session_mut(session_id) {
                        session.threads.retain(|thread| Some(thread.id) != body["threadId"].as_i64());
                    }
                    cx.notify();
                }
                _ => self.fetch_threads(session_id.to_owned(), window, cx),
            },
            "output" => self.accept_output_event(session_id, body, cx),
            "breakpoint" => self.accept_breakpoint_event(session_id, body, cx),
            "terminated" => {
                if !body["restart"].is_null() && body["restart"] != false {
                    drop(self.restart_session(session_id.to_owned(), window, cx));
                } else {
                    // debugSession.ts `onDidTerminateDebugee`: the session disconnects itself, whoever manages its lifecycle.
                    drop(self.raw_disconnect(session_id.to_owned(), false, false, window, cx));
                }
            }
            "loadedSource" => {
                if let Some(session) = self.debug.session_mut(session_id) {
                    session.accept_loaded_source(body);
                }
                cx.notify();
            }
            "capabilities" => {
                if let (Some(session), Some(capabilities)) = (self.debug.session_mut(session_id), body["capabilities"].as_object()) {
                    for (key, value) in capabilities {
                        session.capabilities[key] = value.clone();
                    }
                }
                cx.notify();
            }
            "invalidated" => self.refresh_focus(window, cx),
            "exited" | "progressStart" | "progressUpdate" | "progressEnd" | "memory" | "process" | "module" => {}
            _ => {
                let Some(session) = self.debug.session(session_id) else { return };
                let dto = self.debug.session_dto(session);
                self.host.send("ExtHostDebugService", "$acceptDebugSessionCustomEvent", vec![dto, event.clone()]);
            }
        }
    }

    /// debugSession.ts `onDidInitialize`: breakpoints, then `configurationDone`, then the threads.
    fn session_initialized(&mut self, session_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let sent = self.send_all_breakpoints(&session_id, cx);
        cx.spawn_in(window, async move |this, cx| {
            for reply in sent {
                let _ = reply.await;
            }
            let supports = on_workbench(&this, cx, |workbench, _, _| workbench.debug.session(&session_id).is_some_and(|session| session.capabilities["supportsConfigurationDoneRequest"] == true));
            if supports == Ok(true) {
                let reply = on_workbench(&this, cx, |workbench, _, _| workbench.dap_request(&session_id, "configurationDone", Value::Null));
                if let Ok(reply) = reply
                    && let Err(error) = answer(reply).await
                {
                    let _ = on_workbench(&this, cx, |workbench, _, cx| workbench.append_to_repl(&session_id, &error, Severity::Error, None, cx));
                }
            }
            let _ = on_workbench(&this, cx, |workbench, _, cx| workbench.debug.session_mut(&session_id).map(|session| session.configuration_done = true).map(|_| cx.notify()));
            let _ = on_workbench(&this, cx, |workbench, window, cx| workbench.fetch_threads(session_id.clone(), window, cx));
        })
        .detach();
    }

    /// debugService.ts `sendAllBreakpoints` for one session: sources per file, then functions, data and exceptions.
    pub(crate) fn send_all_breakpoints(&mut self, session_id: &str, cx: &mut Context<Self>) -> Vec<Reply> {
        let Some(session) = self.debug.session(session_id) else { return Vec::new() };
        if session.no_debug() || !session.initialized {
            return Vec::new();
        }
        let capabilities = session.capabilities.clone();
        let activated = self.debug.model.breakpoints_activated;
        let mut uris: Vec<String> = Vec::new();
        for breakpoint in &self.debug.model.breakpoints {
            if !uris.contains(&breakpoint.uri_string()) {
                uris.push(breakpoint.uri_string());
            }
        }
        let mut replies = Vec::new();
        for uri in uris {
            replies.push(self.send_breakpoints(session_id, &uri, false, cx));
        }
        if capabilities["supportsFunctionBreakpoints"] == true {
            let breakpoints: Vec<Value> = self.debug.model.function_breakpoints.iter().filter(|breakpoint| breakpoint.base.enabled && activated).map(|breakpoint| breakpoint.to_dap()).collect();
            let ids: Vec<String> = self.debug.model.function_breakpoints.iter().filter(|breakpoint| breakpoint.base.enabled && activated).map(|breakpoint| breakpoint.base.id.clone()).collect();
            replies.push(self.send_breakpoint_request(session_id, "setFunctionBreakpoints", json!({ "breakpoints": breakpoints }), ids, cx));
        }
        if capabilities["supportsDataBreakpoints"] == true {
            let enabled: Vec<&DataBreakpoint> = self.debug.model.data_breakpoints.iter().filter(|breakpoint| breakpoint.base.enabled && activated).collect();
            let breakpoints: Vec<Value> = enabled.iter().map(|breakpoint| json!({ "dataId": breakpoint.data_id, "accessType": breakpoint.access_type, "condition": breakpoint.base.condition, "hitCondition": breakpoint.base.hit_condition })).collect();
            let ids = enabled.iter().map(|breakpoint| breakpoint.base.id.clone()).collect();
            replies.push(self.send_breakpoint_request(session_id, "setDataBreakpoints", json!({ "breakpoints": breakpoints }), ids, cx));
        }
        // debugSession.ts `sendExceptionBreakpoints`: only once the adapter has filters.
        let session_filters: Vec<&crate::debug::ExceptionBreakpoint> = self.debug.model.exception_breakpoints_for(Some(session_id)).collect();
        if !session_filters.is_empty() {
            let enabled: Vec<&&crate::debug::ExceptionBreakpoint> = session_filters.iter().filter(|breakpoint| breakpoint.base.enabled && activated).collect();
            let filters: Vec<&str> = enabled.iter().filter(|breakpoint| breakpoint.base.condition.is_none()).map(|breakpoint| breakpoint.filter.as_str()).collect();
            let mut arguments = json!({ "filters": filters });
            if capabilities["supportsExceptionFilterOptions"] == true {
                let options: Vec<Value> = enabled.iter().map(|breakpoint| json!({ "filterId": breakpoint.filter, "condition": breakpoint.base.condition })).map(|mut option| {
                    crate::debug::strip_nulls(&mut option);
                    option
                }).collect();
                arguments = json!({ "filters": [], "filterOptions": options });
            }
            let ids = enabled.iter().map(|breakpoint| breakpoint.base.id.clone()).collect();
            replies.push(self.send_breakpoint_request(session_id, "setExceptionBreakpoints", arguments, ids, cx));
        }
        replies
    }

    /// debugSession.ts `sendBreakpoints`: the file's enabled breakpoints; the answer verifies them by position.
    pub(crate) fn send_breakpoints(&mut self, session_id: &str, uri: &str, source_modified: bool, cx: &mut Context<Self>) -> Reply {
        let activated = self.debug.model.breakpoints_activated;
        let breakpoints: Vec<&crate::debug::Breakpoint> = self
            .debug
            .model
            .breakpoints_in(uri)
            .filter(|breakpoint| breakpoint.base.enabled && activated)
            .filter(|breakpoint| breakpoint.triggered_by.is_none() || breakpoint.sessions_did_trigger.iter().any(|id| id == session_id))
            .collect();
        let Some(first) = self.debug.model.breakpoints_in(uri).next() else {
            let source = source_of_uri(&crate::debug::uri_from_string(uri));
            return self.send_breakpoint_request(session_id, "setBreakpoints", json!({ "source": source, "lines": [], "breakpoints": [], "sourceModified": source_modified }), Vec::new(), cx);
        };
        let mut source = source_of_uri(&first.uri);
        if let Some(adapter_data) = breakpoints.iter().map(|breakpoint| breakpoint.adapter_data()).find(|data| !data.is_null()) {
            source["adapterData"] = adapter_data.clone();
        }
        let lines: Vec<u32> = breakpoints.iter().map(|breakpoint| breakpoint.session_agnostic_line().0).collect();
        let dap: Vec<Value> = breakpoints.iter().map(|breakpoint| breakpoint.to_dap()).collect();
        let ids = breakpoints.iter().map(|breakpoint| breakpoint.id().to_owned()).collect();
        self.send_breakpoint_request(session_id, "setBreakpoints", json!({ "source": source, "lines": lines, "breakpoints": dap, "sourceModified": source_modified }), ids, cx)
    }

    /// Sends one of the `set*Breakpoints` requests; the answer's breakpoints match `ids` by position.
    fn send_breakpoint_request(&mut self, session_id: &str, command: &str, arguments: Value, ids: Vec<String>, cx: &mut Context<Self>) -> Reply {
        let reply = self.dap_request(session_id, command, arguments);
        let (sender, receiver) = oneshot::channel();
        let session_id = session_id.to_owned();
        cx.spawn(async move |this, cx| {
            let result = reply.await.unwrap_or(Err(Value::Null));
            if let Ok(body) = &result {
                let answered: Vec<(String, Value)> = ids.into_iter().zip(body["breakpoints"].as_array().cloned().unwrap_or_default()).collect();
                let _ = this.update(cx, |workbench, cx| {
                    let capabilities = workbench.debug.session(&session_id).map(|session| session.capabilities.clone()).unwrap_or_default();
                    for (id, breakpoint) in answered {
                        set_one_session_data(&mut workbench.debug.model, &session_id, &capabilities, &id, &breakpoint);
                    }
                    cx.notify();
                });
            }
            let _ = sender.send(result);
        })
        .detach();
        receiver
    }

    /// debugSession.ts `onDidBreakpoint`: an adapter's breakpoint changed, by its adapter id.
    fn accept_breakpoint_event(&mut self, session_id: &str, body: &Value, cx: &mut Context<Self>) {
        let breakpoint = &body["breakpoint"];
        let Some(adapter_id) = breakpoint["id"].as_i64() else { return };
        let capabilities = self.debug.session(session_id).map(|session| session.capabilities.clone()).unwrap_or_default();
        let model = &mut self.debug.model;
        let bases = model.breakpoints.iter().map(|breakpoint| &breakpoint.base).chain(model.function_breakpoints.iter().map(|breakpoint| &breakpoint.base));
        let Some(id) = bases.chain(model.data_breakpoints.iter().map(|breakpoint| &breakpoint.base)).find(|base| base.id_from_adapter(session_id) == Some(adapter_id)).map(|base| base.id.clone()) else { return };
        match body["reason"].as_str() {
            Some("removed") => clear_one_session_data(model, session_id, &id),
            _ => set_one_session_data(model, session_id, &capabilities, &id, breakpoint),
        }
        cx.notify();
    }

    /// debugSession.ts `onDidOutput`: telemetry is dropped, `important` also shows a notification.
    fn accept_output_event(&mut self, session_id: &str, body: &Value, cx: &mut Context<Self>) {
        let category = body["category"].as_str();
        if category == Some("telemetry") {
            return;
        }
        if category == Some("important") {
            self.notify_warning(body["output"].as_str().unwrap_or_default().to_owned());
            return;
        }
        let severity = session::output_severity(category);
        let source = body["source"]["path"].as_str().map(|path| (path.to_owned(), body["line"].as_u64().unwrap_or(1) as u32));
        if body["variablesReference"].as_i64().is_some_and(|reference| reference > 0) {
            let mut variable = Variable::from_dap(body);
            variable.value = body["output"].as_str().unwrap_or_default().trim_end().to_owned();
            self.debug.repl.push(ReplElement::Variable { session_id: session_id.to_owned(), variable, severity });
            cx.notify();
            return;
        }
        self.append_to_repl(session_id, body["output"].as_str().unwrap_or_default(), severity, source, cx);
    }

    /// replModel.ts `appendToRepl`: text joins the last unfinished line; a repeated line counts up.
    pub(crate) fn append_to_repl(&mut self, session_id: &str, text: &str, severity: Severity, source: Option<(String, u32)>, cx: &mut Context<Self>) {
        crate::debug::session::append_output(&mut self.debug.repl, session_id, text, severity, source);
        cx.notify();
    }

    /// debugSession.ts `fetchThreads`.
    pub(crate) fn fetch_threads(&mut self, session_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let reply = self.dap_request(&session_id, "threads", Value::Null);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(body) = answer(reply).await else { return };
            let _ = on_workbench(&this, cx, |workbench, _, cx| {
                if let Some(session) = workbench.debug.session_mut(&session_id) {
                    session.set_threads(body["threads"].as_array().map_or(&[], Vec::as_slice));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// debugSession.ts `handleStop` and debugService.ts's focus: threads, the top frame, focus, then the rest.
    fn session_stopped(&mut self, session_id: String, body: Value, window: &mut Window, cx: &mut Context<Self>) {
        let threads = self.dap_request(&session_id, "threads", Value::Null);
        cx.spawn_in(window, async move |this, cx| {
            let threads = answer(threads).await.unwrap_or_default();
            let thread_id = body["threadId"].as_i64();
            let ids = on_workbench(&this, cx, |workbench, _, _| {
                let session = workbench.debug.session_mut(&session_id)?;
                session.set_threads(threads["threads"].as_array().map_or(&[], Vec::as_slice));
                session.stop_threads(&body);
                Some(session.threads.iter().filter(|thread| thread.stopped).map(|thread| thread.id).collect::<Vec<i64>>())
            });
            let Ok(Some(ids)) = ids else { return };
            // The stopped thread first, as handleStop fetches its call stack before the others'.
            let mut ordered: Vec<i64> = thread_id.into_iter().filter(|id| ids.contains(id)).collect();
            ordered.extend(ids.iter().filter(|id| Some(**id) != thread_id));
            for (index, id) in ordered.into_iter().enumerate() {
                let reply = on_workbench(&this, cx, |workbench, _, _| workbench.dap_request(&session_id, "stackTrace", json!({ "threadId": id, "startFrame": 0, "levels": 20 })));
                let Ok(reply) = reply else { return };
                let frames = answer(reply).await.unwrap_or_default();
                let focus_now = index == 0;
                let _ = on_workbench(&this, cx, |workbench, window, cx| {
                    workbench.accept_stack_trace(&session_id, id, &frames);
                    if focus_now {
                        workbench.focus_after_stop(&session_id, id, window, cx);
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// callStackView.ts "Load More Stack Frames": the next 20 frames after those fetched.
    pub(crate) fn fetch_more_frames(&mut self, session_id: &str, thread_id: i64, cx: &mut Context<Self>) {
        let Some(start) = self.debug.session(session_id).and_then(|session| session.thread(thread_id)).map(|thread| thread.frames.len()) else { return };
        let reply = self.dap_request(session_id, "stackTrace", json!({ "threadId": thread_id, "startFrame": start, "levels": 20 }));
        let session_id = session_id.to_owned();
        cx.spawn(async move |this, cx| {
            let Ok(body) = answer(reply).await else { return };
            let _ = this.update(cx, |workbench, cx| {
                let Some(thread) = workbench.debug.session_mut(&session_id).and_then(|session| session.thread_mut(thread_id)) else { return };
                let fetched = std::mem::take(&mut thread.frames);
                workbench.accept_stack_trace(&session_id, thread_id, &body);
                let Some(thread) = workbench.debug.session_mut(&session_id).and_then(|session| session.thread_mut(thread_id)) else { return };
                let more = std::mem::replace(&mut thread.frames, fetched);
                thread.frames.extend(more);
                thread.total_frames = body["totalFrames"].as_u64().filter(|total| *total as usize > thread.frames.len());
                cx.notify();
            });
        })
        .detach();
    }

    fn accept_stack_trace(&mut self, session_id: &str, thread_id: i64, body: &Value) {
        let Some(thread) = self.debug.session_mut(session_id).and_then(|session| session.thread_mut(thread_id)) else { return };
        thread.frames = body["stackFrames"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|frame| StackFrame {
                id: frame["id"].as_i64().unwrap_or_default(),
                name: frame["name"].as_str().unwrap_or_default().to_owned(),
                source: frame["source"].clone(),
                line: frame["line"].as_u64().unwrap_or(1) as u32,
                column: frame["column"].as_u64().unwrap_or(1) as u32,
                presentation_hint: frame["presentationHint"].as_str().map(str::to_owned),
                can_restart: frame["canRestart"] == true,
            })
            .collect();
        thread.total_frames = body["totalFrames"].as_u64().filter(|total| *total as usize > thread.frames.len());
    }

    /// The focus after a stop: the focused session or none focused takes the stopped thread
    /// (debugService.ts `onDidStop` with `focusStackFrame`), and `debug.openDebug: openOnDebugBreak` shows the view.
    fn focus_after_stop(&mut self, session_id: &str, thread_id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.debug.focus.session.as_deref();
        let focused_is_running = self.debug.focused_session().is_none_or(|session| session.state(self.debug.focus.thread) != State::Stopped);
        if focused.is_none() || focused == Some(session_id) || focused_is_running {
            let target = self.debug.focus_target(Some(session_id), Some(thread_id));
            self.set_debug_focus(target, true, window, cx);
        }
        if self.setting("debug.openDebug") == "openOnDebugBreak" {
            self.show_debug_view(cx);
        }
        if self.setting("debug.focusWindowOnBreak") != false {
            window.activate_window();
        }
    }

    fn threads_continued(&mut self, session_id: &str, thread_id: Option<i64>, cx: &mut Context<Self>) {
        let Some(session) = self.debug.session_mut(session_id) else { return };
        session.continue_threads(thread_id);
        if self.debug.focus.session.as_deref() == Some(session_id) && thread_id.is_none_or(|id| Some(id) == self.debug.focus.thread) {
            self.debug.focus.frame = None;
            self.close_exception_widget(cx);
            self.debug.scopes.clear();
            self.debug.children.clear();
            self.send_stack_frame_focus();
        }
        cx.notify();
    }

    /// debugService.ts `focusStackFrame`: the view model's focus, the frame's file opened at its line, the
    /// variables and watches of the frame, and `$acceptStackFrameFocus` for the host.
    pub(crate) fn set_debug_focus(&mut self, focus: DebugFocus, reveal: bool, window: &mut Window, cx: &mut Context<Self>) {
        let session_changed = focus.session != self.debug.focus.session;
        self.debug.focus = focus;
        if session_changed {
            let dto = self.debug.focused_session().map(|session| self.debug.session_dto(session));
            self.host.send("ExtHostDebugService", "$acceptDebugSessionActiveChanged", vec![dto.unwrap_or(Value::Null)]);
        }
        self.send_stack_frame_focus();
        self.debug.scopes.clear();
        self.debug.children.clear();
        if reveal && let Some(frame) = self.debug.focused_frame().cloned() {
            self.reveal_stack_frame(&frame, window, cx);
        }
        self.toggle_exception_widget(cx);
        self.refresh_focus(window, cx);
    }

    fn send_stack_frame_focus(&self) {
        let focus = &self.debug.focus;
        let value = match (&focus.session, focus.thread, focus.frame) {
            (Some(session), Some(thread), Some(frame)) => json!({ "kind": "stackFrame", "threadId": thread, "frameId": frame, "sessionId": session }),
            (Some(session), Some(thread), None) => json!({ "kind": "thread", "threadId": thread, "sessionId": session }),
            _ => Value::Null,
        };
        self.host.send("ExtHostDebugService", "$acceptStackFrameFocus", vec![value]);
    }

    /// `StackFrame.openInEditor`: the file at the frame's line, revealed in the center if outside the view.
    fn reveal_stack_frame(&mut self, frame: &StackFrame, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = frame.path() else { return };
        let path = PathBuf::from(path);
        if !path.is_file() {
            return;
        }
        let editor_focused = self.editor_focus.is_focused(window);
        let previous_focus = window.focused(cx);
        self.open_file(path.clone(), true, window, cx);
        let line = frame.line.max(1) as usize;
        let column = frame.column.max(1) as usize;
        self.select_range(&path, crate::anything_quick_access::TextRange { start_line_number: line, start_column: column, end_line_number: line, end_column: column }, cx);
        // preserveFocus unless the editor had it: the stop must not take the focus from a view.
        match previous_focus {
            Some(previous) if !editor_focused => window.focus(&previous, cx),
            None => window.blur(cx),
            Some(_) => {}
        }
    }

    /// The focused frame's scopes and the watch expressions, evaluated again (variablesView.ts and
    /// watchExpressionsView.ts on focus change).
    pub(crate) fn refresh_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(session_id), Some(frame_id)) = (self.debug.focus.session.clone(), self.debug.focus.frame) else {
            self.debug.watch_values.clear();
            cx.notify();
            return;
        };
        let scopes = self.dap_request(&session_id, "scopes", json!({ "frameId": frame_id }));
        let watches: Vec<(String, Reply)> = self
            .debug
            .model
            .watch_expressions
            .clone()
            .into_iter()
            .map(|expression| (expression.id, self.dap_request(&session_id, "evaluate", json!({ "expression": expression.name, "frameId": frame_id, "context": "watch" }))))
            .collect();
        cx.spawn_in(window, async move |this, cx| {
            let scopes = answer(scopes).await.unwrap_or_default();
            let scopes: Vec<Variable> = scopes["scopes"].as_array().into_iter().flatten().map(Variable::from_dap).collect();
            // variablesView.ts expands the first scope that is not expensive.
            let first = scopes.iter().find(|scope| !scope.expensive).map(|scope| scope.variables_reference);
            let _ = on_workbench(&this, cx, |workbench, _, cx| {
                workbench.debug.scopes = scopes;
                cx.notify();
            });
            if let Some(reference) = first {
                let _ = on_workbench(&this, cx, |workbench, window, cx| workbench.fetch_children(&session_id, reference, window, cx));
            }
            for (id, reply) in watches {
                let result = answer(reply).await;
                let _ = on_workbench(&this, cx, |workbench, _, cx| {
                    let name = workbench.debug.model.watch_expressions.iter().find(|expression| expression.id == id).map(|expression| expression.name.clone()).unwrap_or_default();
                    workbench.debug.watch_values.insert(id, evaluation_result(name, result));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// `Variable.getChildren`: the `variables` request of a row, cached until the focus moves.
    pub(crate) fn fetch_children(&mut self, session_id: &str, reference: i64, window: &mut Window, cx: &mut Context<Self>) {
        let key = (session_id.to_owned(), reference);
        if self.debug.children.contains_key(&key) {
            return;
        }
        self.debug.children.insert(key.clone(), Vec::new());
        let reply = self.dap_request(session_id, "variables", json!({ "variablesReference": reference }));
        cx.spawn_in(window, async move |this, cx| {
            let body = answer(reply).await.unwrap_or_default();
            let variables = body["variables"].as_array().into_iter().flatten().map(Variable::from_dap).collect();
            let _ = on_workbench(&this, cx, |workbench, _, cx| {
                workbench.debug.children.insert(key, variables);
                cx.notify();
            });
        })
        .detach();
    }

    /// debugService.ts `startDebugging` as the host and the start commands call it: a launch configuration by
    /// name (or a compound), or a configuration object.
    pub(crate) fn start_debugging(&mut self, folder: Option<PathBuf>, name_or_config: Value, options: Value, window: &mut Window, cx: &mut Context<Self>) -> Reply {
        let (sender, receiver) = oneshot::channel();
        self.debug.initializing = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = start_debugging_flow(&this, cx, folder, name_or_config, options).await;
            let result = on_workbench(&this, cx, |workbench, _, cx| {
                workbench.debug.initializing = false;
                cx.notify();
                match result {
                    Ok(started) => Ok(json!(started)),
                    Err(message) => {
                        workbench.notify_error(message.clone());
                        Err(error_value(&message))
                    }
                }
            });
            let _ = sender.send(result.unwrap_or_else(|message| Err(error_value(&message))));
        })
        .detach();
        receiver
    }

    /// debugService.ts `stopSession`: terminate one session, or all.
    pub(crate) fn stop_session(&mut self, session_id: Option<String>, disconnect: bool, window: &mut Window, cx: &mut Context<Self>) -> Reply {
        let ids: Vec<String> = match session_id {
            Some(id) => vec![id],
            None => self.debug.sessions.iter().map(|session| session.id.clone()).collect(),
        };
        let (sender, receiver) = oneshot::channel();
        let stops: Vec<Reply> = ids.into_iter().map(|id| if disconnect { self.disconnect_session(id, false, false, window, cx) } else { self.terminate_session(id, window, cx) }).collect();
        cx.background_executor()
            .spawn(async move {
                for stop in stops {
                    let _ = stop.await;
                }
                let _ = sender.send(Ok(Value::Null));
            })
            .detach();
        receiver
    }

    /// debugSession.ts `terminate`: the parent of a child managed by it, else `terminate` when supported for a
    /// launch, else `disconnect` that terminates the debuggee.
    fn terminate_session(&mut self, session_id: String, window: &mut Window, cx: &mut Context<Self>) -> Reply {
        let Some(session) = self.debug.session(&session_id) else { return ready(Value::Null) };
        if session.options["lifecycleManagedByParent"] == true && let Some(parent) = session.parent.clone() {
            return self.terminate_session(parent, window, cx);
        }
        if session.exited {
            self.session_ended(&session_id, cx);
            return ready(Value::Null);
        }
        let terminate = session.capabilities["supportsTerminateRequest"] == true && session.configuration["request"] == "launch" && !session.terminated;
        if !terminate {
            return self.disconnect_session(session_id, false, true, window, cx);
        }
        if let Some(session) = self.debug.session_mut(&session_id) {
            session.terminated = true;
        }
        let reply = self.dap_request(&session_id, "terminate", json!({ "restart": false }));
        let (sender, receiver) = oneshot::channel();
        cx.spawn_in(window, async move |this, cx| {
            // rawDebugSession.ts `terminate`: a failed terminate falls back to disconnect.
            if answer(reply).await.is_err() {
                let disconnect = on_workbench(&this, cx, |workbench, window, cx| workbench.disconnect_session(session_id, false, true, window, cx));
                if let Ok(disconnect) = disconnect {
                    let _ = disconnect.await;
                }
            }
            let _ = sender.send(Ok(Value::Null));
        })
        .detach();
        receiver
    }

    /// rawDebugSession.ts `disconnect` then `stopAdapter`: the adapter has 2 s to answer, then it is stopped.
    fn disconnect_session(&mut self, session_id: String, restart: bool, terminate_debuggee: bool, window: &mut Window, cx: &mut Context<Self>) -> Reply {
        let Some(session) = self.debug.session(&session_id) else { return ready(Value::Null) };
        if session.options["lifecycleManagedByParent"] == true && let Some(parent) = session.parent.clone() {
            return self.disconnect_session(parent, restart, terminate_debuggee, window, cx);
        }
        self.raw_disconnect(session_id, restart, terminate_debuggee, window, cx)
    }

    /// rawDebugSession.ts `disconnect`, then the adapter stops and the session ends.
    fn raw_disconnect(&mut self, session_id: String, restart: bool, terminate_debuggee: bool, window: &mut Window, cx: &mut Context<Self>) -> Reply {
        if let Some(session) = self.debug.session_mut(&session_id) {
            session.terminated = true;
        }
        let reply = self.dap_request(&session_id, "disconnect", json!({ "restart": restart, "terminateDebuggee": terminate_debuggee }));
        let (sender, receiver) = oneshot::channel();
        cx.spawn_in(window, async move |this, cx| {
            let timer = cx.background_executor().timer(std::time::Duration::from_secs(2));
            futures::future::select(Box::pin(reply), Box::pin(timer)).await;
            let _ = on_workbench(&this, cx, |workbench, _, cx| {
                if let Some(session) = workbench.debug.session(&session_id) {
                    workbench.host.send("ExtHostDebugService", "$stopDASession", vec![json!(session.handle)]);
                }
                workbench.session_ended(&session_id, cx);
            });
            let _ = sender.send(Ok(Value::Null));
        })
        .detach();
        receiver
    }

    /// debugService.ts `restartSession` for a `terminated` event with `restart`: the same configuration again.
    fn restart_session(&mut self, session_id: String, window: &mut Window, cx: &mut Context<Self>) -> Reply {
        let Some(session) = self.debug.session(&session_id) else { return ready(Value::Null) };
        let configuration = session.unresolved_configuration.clone();
        let folder = session.folder_uri.as_ref().and_then(crate::debug::configuration_folder);
        let options = session.options.clone();
        let stop = self.terminate_session(session_id, window, cx);
        let (sender, receiver) = oneshot::channel();
        cx.spawn_in(window, async move |this, cx| {
            let _ = stop.await;
            let start = on_workbench(&this, cx, |workbench, window, cx| workbench.start_debugging(folder, configuration, options, window, cx));
            if let Ok(start) = start {
                let _ = sender.send(start.await.unwrap_or(Ok(Value::Null)));
            }
        })
        .detach();
        receiver
    }

    /// The adapter of a handle exited or failed (`onDidExitAdapter`).
    fn adapter_exited(&mut self, handle: u32, error: Option<String>, cx: &mut Context<Self>) {
        let Some(session) = self.debug.session_by_handle(handle) else { return };
        let (session_id, ending) = (session.id.clone(), session.terminated);
        // A connection that closes after a disconnect is the end the session asked for, not a crash.
        if let Some(error) = error.filter(|_| !ending) {
            self.notify_error(format!("Debug adapter process has terminated unexpectedly ({error})"));
        }
        self.session_ended(&session_id, cx);
    }

    /// debugService.ts `onDidEndAdapter`: the session leaves the model, its requests fail, the focus moves on.
    fn session_ended(&mut self, session_id: &str, cx: &mut Context<Self>) {
        let Some(index) = self.debug.sessions.iter().position(|session| session.id == session_id) else { return };
        let mut session = self.debug.sessions.remove(index);
        session.exited = true;
        session.cancel_pending();
        let dto = self.debug.session_dto(&session);
        self.debug.known_sessions.remove(session_id);
        self.host.send("ExtHostDebugService", "$acceptDebugSessionTerminated", vec![dto]);
        let capabilities = session.capabilities.clone();
        self.debug.model.set_breakpoint_session_data(session_id, &capabilities, None);
        self.debug.model.set_exception_breakpoint_fallback_session(session_id);
        self.debug.model.remove_exception_breakpoints_for_session(session_id);
        if self.debug.focus.session.as_deref() == Some(session_id) {
            self.debug.focus = self.debug.focus_target(None, None);
            self.close_exception_widget(cx);
            self.debug.scopes.clear();
            self.debug.children.clear();
            self.debug.watch_values.clear();
            let dto = self.debug.focused_session().map(|session| self.debug.session_dto(session));
            self.host.send("ExtHostDebugService", "$acceptDebugSessionActiveChanged", vec![dto.unwrap_or(Value::Null)]);
            self.send_stack_frame_focus();
        }
        if self.debug.sessions.is_empty() {
            // Data breakpoints that cannot persist end with the last session.
            let transient: Vec<String> = self.debug.model.data_breakpoints.iter().filter(|breakpoint| !breakpoint.can_persist).map(|breakpoint| breakpoint.base.id.clone()).collect();
            for id in transient {
                self.debug.model.remove_data_breakpoints(Some(&id));
            }
            if self.setting("debug.openExplorerOnEnd") == true {
                self.side_bar_view = super::search_view::SideBarView::Explorer;
            }
        }
        self.store_debug_model();
        cx.notify();
    }

    /// `quickInputService.pick` for the start flow: answers the picked index, `None` when dismissed.
    pub(super) fn debug_pick(&mut self, placeholder: &str, items: Vec<(String, Option<String>)>, window: &mut Window, cx: &mut Context<Self>) -> oneshot::Receiver<Option<String>> {
        let (sender, receiver) = oneshot::channel();
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some(placeholder.into());
        input.set_items(items.into_iter().enumerate().map(|(handle, (label, description))| Entry::Item(Item { handle: handle as i64, label, description, ..Item::default() })).collect());
        let id = self.quick_inputs.add(input, Owner::Debug(Some(sender)));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
        receiver
    }

    /// `quickInputService.input` for `promptString` inputs.
    fn debug_input(&mut self, prompt: Option<String>, value: Option<String>, password: bool, window: &mut Window, cx: &mut Context<Self>) -> oneshot::Receiver<Option<String>> {
        let (sender, receiver) = oneshot::channel();
        let mut input = QuickInput::new(0, Kind::InputBox);
        input.prompt = prompt;
        input.password = password;
        if let Some(value) = value {
            input.set_value(value);
        }
        let id = self.quick_inputs.add(input, Owner::Debug(Some(sender)));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
        receiver
    }

    /// The picker or input box of the start flow: Enter answers the item's handle or the typed value.
    pub fn debug_pick_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Accept(_) => {
                let Some(input) = self.quick_inputs.get(id) else { return };
                let answer = match input.kind {
                    Kind::QuickPick => input.selected_handles().first().map(|handle| handle.to_string()),
                    Kind::InputBox => Some(input.input_box.value.clone()),
                };
                let Some(answer) = answer else { return };
                if let Some(Owner::Debug(reply)) = self.quick_inputs.owner_mut(id)
                    && let Some(reply) = reply.take()
                {
                    let _ = reply.send(Some(answer));
                }
                self.hide_quick_input_by_id(id, window, cx);
            }
            Event::Hide => {
                if let Some(Owner::Debug(reply)) = self.quick_inputs.owner_mut(id)
                    && let Some(reply) = reply.take()
                {
                    let _ = reply.send(None);
                }
                self.quick_inputs.remove(id);
            }
            _ => {}
        }
    }

    /// Opens the Run and Debug view in the side bar.
    pub(crate) fn show_debug_view(&mut self, cx: &mut Context<Self>) {
        self.side_bar_view = super::search_view::SideBarView::Debug;
        self.set_part_hidden(super::layout::Part::SideBar, false, cx);
    }
}

fn ready(value: Value) -> Reply {
    let (sender, receiver) = oneshot::channel();
    let _ = sender.send(Ok(value));
    receiver
}

/// The DAP `Source` of a breakpoint's resource: a file's path, else the URI string.
fn source_of_uri(uri: &Value) -> Value {
    let path = match uri["scheme"].as_str() {
        Some("file") => uri["path"].clone(),
        _ => json!(crate::debug::uri_string(uri)),
    };
    let name = uri["path"].as_str().and_then(|path| path.rsplit('/').next()).unwrap_or_default();
    json!({ "name": name, "path": path })
}

fn set_one_session_data(model: &mut DebugModel, session_id: &str, capabilities: &Value, id: &str, breakpoint: &Value) {
    let data = crate::debug::SessionData::from_dap(breakpoint, capabilities);
    if let Some(found) = model.breakpoint_mut(id) {
        found.set_session_data(session_id, Some(data));
        return;
    }
    let model_bases = model.function_breakpoints.iter_mut().map(|breakpoint| &mut breakpoint.base).chain(model.data_breakpoints.iter_mut().map(|breakpoint| &mut breakpoint.base));
    let mut bases = model_bases.chain(model.exception_breakpoints.iter_mut().map(|breakpoint| &mut breakpoint.base));
    if let Some(base) = bases.find(|base| base.id == id) {
        base.set_session_data(session_id, Some(data));
    }
}

fn clear_one_session_data(model: &mut DebugModel, session_id: &str, id: &str) {
    if let Some(found) = model.breakpoint_mut(id) {
        found.set_session_data(session_id, None);
    }
}

/// A watch or REPL evaluation: the result row, or the error message drawn as one.
fn evaluation_result(name: String, result: Result<Value, String>) -> Variable {
    match result {
        Ok(body) => Variable { name, ..Variable::from_dap(&body) },
        Err(message) => Variable { name, value: message, error: true, ..Variable::default() },
    }
}

/// The start flow of debugService.ts `startDebugging` and `createSession`; `Err` rejects the start.
async fn start_debugging_flow(this: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, folder: Option<PathBuf>, name_or_config: Value, options: Value) -> Result<bool, String> {
    let activation = on_workbench(this, cx, |workbench, _, _| workbench.host.activate_by_event("onDebug"))?;
    let _ = activation.await;
    let launch = match &folder {
        Some(folder) => Some(on_workbench(this, cx, |workbench, _, _| workbench.folder_launch(folder))?),
        None => None,
    };
    let config = match &name_or_config {
        Value::String(name) => {
            if let Some(compound) = launch.as_ref().and_then(|launch| launch.compound(name)).cloned() {
                return start_compound(this, cx, folder, launch.expect("a compound comes from a launch"), compound, options).await;
            }
            match launch.as_ref().and_then(|launch| launch.configuration(name)) {
                Some(config) => Some(config),
                None if launch.is_some() => return Err(format!("Configuration '{name}' is missing in 'launch.json'.")),
                None => return Err("'launch.json' does not exist for passed workspace folder.".into()),
            }
        }
        Value::Object(_) => Some(name_or_config.clone()),
        _ => None,
    };
    let launch_inputs = launch.map(|launch| launch.inputs).unwrap_or_default();
    create_session(this, cx, folder, config, options, launch_inputs).await
}

/// A compound: each named configuration of the same launch.json, a success only when all start.
async fn start_compound(this: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, folder: Option<PathBuf>, launch: Launch, compound: Value, options: Value) -> Result<bool, String> {
    let Some(names) = compound["configurations"].as_array() else {
        return Err("Compound must have \"configurations\" attribute set in order to start multiple configurations.".into());
    };
    let mut all = true;
    for name in names {
        let name = name.as_str().or(name["name"].as_str()).unwrap_or_default();
        if Some(name) == compound["name"].as_str() {
            all = false;
            continue;
        }
        let Some(config) = launch.configuration(name) else { return Err(format!("Could not find launch configuration '{name}' in the workspace.")) };
        all &= Box::pin(create_session(this, cx, folder.clone(), Some(config), options.clone(), launch.inputs.clone())).await?;
    }
    Ok(all)
}

/// debugService.ts `createSession`: the type (guessed from the active editor without one), the providers'
/// resolution, variables, then a new session; its errors are shown and answer false.
async fn create_session(this: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, folder: Option<PathBuf>, config: Option<Value>, options: Value, inputs: Vec<Value>) -> Result<bool, String> {
    let mut config = config.unwrap_or_else(|| json!({}));
    if options["noDebug"] == true {
        config["noDebug"] = json!(true);
    }
    let unresolved = config.clone();
    let mut debug_type = config["type"].as_str().map(str::to_owned);
    if debug_type.is_none() {
        let Some(guessed) = guess_debugger(this, cx).await? else { return Ok(false) };
        debug_type = Some(guessed);
    }
    let folder_uri = folder.as_deref().map(exthost::scan::file_uri);
    let resolved = resolve_by_providers(this, cx, folder_uri.clone(), debug_type.clone().unwrap_or_default(), config).await?;
    let Some(resolved) = resolved.filter(|resolved| resolved["type"].is_string()) else { return Ok(false) };
    match finish_session(this, cx, folder, folder_uri, resolved, unresolved, options, inputs).await {
        Ok(started) => Ok(started),
        Err(message) => {
            on_workbench(this, cx, |workbench, _, cx| {
                workbench.notify_error(message);
                cx.notify();
            })?;
            Ok(false)
        }
    }
}

/// debugAdapterManager.ts `guessDebugger`: the debuggers interested in the active editor's language, picked when
/// more than one; remembered per file in `chosenEnvironments`.
async fn guess_debugger(this: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext) -> Result<Option<String>, String> {
    let (resource, language) = on_workbench(this, cx, |workbench, _, _| workbench.editors.active().map(|editor| (editor.document().uri_string(), editor.document().language_id.clone())).unzip())?;
    if let Some(resource) = &resource
        && let Some(chosen) = on_workbench(this, cx, |workbench, _, _| workbench.debug.chosen_environments.get(resource).and_then(|chosen| chosen["type"].as_str()).map(str::to_owned))?
    {
        return Ok(Some(chosen));
    }
    let activation = on_workbench(this, cx, |workbench, _, _| (workbench.host.activate_by_event("onDebugInitialConfigurations"), workbench.host.activate_by_event("onDebug")))?;
    let _ = activation.0.await;
    let _ = activation.1.await;
    let candidates = on_workbench(this, cx, |workbench, window, _| {
        let context = workbench.key_context(window);
        let holds = |when: Option<&str>| when.and_then(crate::contextkey::Expr::deserialize).is_none_or(|when| when.evaluate(&context));
        let enabled: Vec<&Debugger> = workbench.debug.debuggers.iter().filter(|debugger| holds(debugger.when()) && workbench.debug.adapter_types.contains(&debugger.debug_type)).collect();
        let interested: Vec<&Debugger> = enabled.iter().copied().filter(|debugger| language.as_deref().is_some_and(|language| debugger.interested_in_language(language))).collect();
        let mut candidates: Vec<(String, String)> = match interested.len() {
            1 => return interested.iter().map(|debugger| (debugger.debug_type.clone(), debugger.label().to_owned())).collect(),
            0 => enabled.iter().filter(|debugger| debugger.has_initial_configuration() || workbench.debug.providers.iter().any(|provider| provider.debug_type == debugger.debug_type)).map(|debugger| (debugger.debug_type.clone(), debugger.label().to_owned())).collect(),
            _ => interested.iter().map(|debugger| (debugger.debug_type.clone(), debugger.label().to_owned())).collect(),
        };
        candidates.retain(|(debug_type, _)| workbench.debug.debugger(debug_type).is_none_or(|debugger| !debugger.hidden_when().and_then(crate::contextkey::Expr::deserialize).is_some_and(|when| when.evaluate(&context))));
        candidates.sort_by(|first, second| first.1.cmp(&second.1));
        candidates
    })?;
    let debug_type = match candidates.len() {
        0 => {
            on_workbench(this, cx, |workbench, _, _| workbench.notify_error("The active file can not be debugged. Make sure it is saved and that you have a debug extension installed for that file type."))?;
            return Ok(None);
        }
        1 => candidates[0].0.clone(),
        _ => {
            let items = candidates.iter().map(|(_, label)| (label.clone(), None)).collect();
            let picked = on_workbench(this, cx, |workbench, window, cx| workbench.debug_pick("Select debugger", items, window, cx))?;
            let Some(index) = picked.await.ok().flatten().and_then(|index| index.parse::<usize>().ok()) else { return Ok(None) };
            candidates[index].0.clone()
        }
    };
    if let Some(resource) = resource {
        on_workbench(this, cx, |workbench, _, _| {
            workbench.debug.chosen_environments.insert(resource, json!({ "type": debug_type }));
            let stored = Value::Object(workbench.debug.chosen_environments.clone()).to_string();
            workbench.workspace_storage.lock().unwrap().set(CHOSEN_ENVIRONMENTS_KEY, stored);
        })?;
    }
    Ok(Some(debug_type))
}

/// debugConfigurationManager.ts `resolveConfigurationByProviders`: the type's providers then `*`'s, again while
/// a provider changes the type.
async fn resolve_by_providers(this: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, folder_uri: Option<Value>, debug_type: String, config: Value) -> Result<Option<Value>, String> {
    let mut resolved_type = config["type"].as_str().map(str::to_owned).unwrap_or(debug_type.clone());
    let mut result = Some(config);
    let mut seen = HashSet::new();
    while let Some(config) = result.clone() {
        if !seen.insert(resolved_type.clone()) {
            break;
        }
        let mut current = Some(config);
        for kind in [resolved_type.clone(), "*".to_owned()] {
            if kind != "*" {
                let events = on_workbench(this, cx, |workbench, _, _| {
                    ["onDebugResolve".to_owned(), "onDebug".to_owned(), format!("onDebugResolve:{kind}")].map(|event| workbench.host.activate_by_event(&event))
                })?;
                for event in events {
                    let _ = event.await;
                }
            }
            let handles = on_workbench(this, cx, |workbench, _, _| workbench.debug.providers.iter().filter(|provider| provider.debug_type == kind && provider.has_resolve).map(|provider| provider.handle).collect::<Vec<i64>>())?;
            for handle in handles {
                let Some(config) = current.take() else { break };
                let reply = on_workbench(this, cx, |workbench, _, _| workbench.host.call("ExtHostDebugService", "$resolveDebugConfiguration", vec![json!(handle), json!(folder_uri), config]))?;
                current = Some(answer(reply).await?).filter(|config| config.is_object());
            }
        }
        result = current;
        resolved_type = result.as_ref().and_then(|config| config["type"].as_str()).map(str::to_owned).unwrap_or(debug_type.clone());
    }
    Ok(result)
}

/// The rest of `createSession` after the providers: variables, `resolveDebugConfigurationWithSubstitutedVariables`,
/// the debugger check, then `doCreateSession` and `launchOrAttachToSession`.
#[allow(clippy::too_many_arguments)]
async fn finish_session(this: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, folder: Option<PathBuf>, folder_uri: Option<Value>, config: Value, unresolved: Value, options: Value, inputs: Vec<Value>) -> Result<bool, String> {
    let debug_type = config["type"].as_str().unwrap_or_default().to_owned();
    let (registered, debugger) = on_workbench(this, cx, |workbench, _, _| (workbench.debug.adapter_types.contains(&debug_type), workbench.debug.debugger(&debug_type).cloned()))?;
    // debugService.ts `substituteVariables`: through the extension host for a type it has an adapter for.
    let mut config = config;
    if registered {
        let reply = on_workbench(this, cx, |workbench, _, _| workbench.host.call("ExtHostDebugService", "$substituteVariables", vec![json!(folder_uri), config.clone()]))?;
        config = answer(reply).await?;
        let Some(resolved) = resolve_interactive(this, cx, config, debugger.as_ref(), &inputs).await? else { return Ok(false) };
        config = resolved;
    }
    // shortcut: preLaunchTask and postDebugTask wait for the task system of milestone 15; the session starts without it.
    if let Some(task) = config["preLaunchTask"].as_str() {
        eprintln!("[debug] preLaunchTask '{task}' is not run: no task system yet");
    }
    let handles = on_workbench(this, cx, |workbench, _, _| {
        let of_type = workbench.debug.providers.iter().filter(|provider| provider.debug_type == debug_type && provider.has_resolve2);
        let any = workbench.debug.providers.iter().filter(|provider| provider.debug_type == "*" && provider.has_resolve2);
        of_type.chain(any).map(|provider| provider.handle).collect::<Vec<i64>>()
    })?;
    for handle in handles {
        let reply = on_workbench(this, cx, |workbench, _, _| workbench.host.call("ExtHostDebugService", "$resolveDebugConfigurationWithSubstitutedVariables", vec![json!(handle), json!(folder_uri), config.clone()]))?;
        let Some(resolved) = Some(answer(reply).await?).filter(Value::is_object) else { return Ok(false) };
        config = resolved;
    }
    let request = config["request"].as_str();
    if request != Some("launch") && request != Some("attach") {
        return Err(match request {
            Some(request) => format!("Attribute 'request' has an unsupported value '{request}' in the chosen debug configuration."),
            None => "Attribute 'request' is missing from the chosen debug configuration.".into(),
        });
    }
    if !registered {
        return Err(match config["type"].as_str() {
            Some(debug_type) if !debug_type.is_empty() => format!("Configured debug type '{debug_type}' is not supported."),
            _ => "Missing property 'type' for the chosen launch configuration.".into(),
        });
    }
    let session_id = on_workbench(this, cx, |workbench, _, cx| {
        let id = crate::debug::generate_uuid();
        workbench.debug.next_handle += 1;
        let session = Session::new(id.clone(), workbench.debug.next_handle, config.clone(), unresolved, folder_uri.clone(), options.clone());
        workbench.debug.sessions.push(session);
        // debug.openDebug: the view opens on the first session start, not for Run Without Debugging.
        let open_debug = workbench.setting("debug.openDebug");
        if config["noDebug"] != true && options["suppressDebugView"] != true && (open_debug == "openOnSessionStart" || open_debug == "openOnFirstSessionStart" && !workbench.debug.first_session_started) {
            workbench.show_debug_view(cx);
        }
        cx.notify();
        id
    })?;
    let _ = folder;
    match launch_or_attach(this, cx, &session_id).await {
        Ok(()) => {
            on_workbench(this, cx, |workbench, window, cx| {
                let console = config["internalConsoleOptions"].as_str().map(str::to_owned).unwrap_or_else(|| workbench.setting("debug.internalConsoleOptions").as_str().unwrap_or_default().to_owned());
                if console == "openOnSessionStart" || console == "openOnFirstSessionStart" && !workbench.debug.first_session_started {
                    workbench.open_debug_console(false, window, cx);
                }
                workbench.debug.first_session_started = true;
                workbench.update_debug_status();
                let Some(session) = workbench.debug.session(&session_id) else { return };
                let dto = workbench.debug.session_dto(session);
                workbench.host.send("ExtHostDebugService", "$acceptDebugSessionStarted", vec![dto]);
                if workbench.debug.focus.session.is_none() {
                    let focus = workbench.debug.focus_target(Some(&session_id), None);
                    workbench.set_debug_focus(focus, false, window, cx);
                }
                cx.notify();
            })?;
            Ok(true)
        }
        Err(error) => {
            on_workbench(this, cx, |workbench, window, cx| {
                drop(workbench.disconnect_session(session_id.clone(), false, true, window, cx));
                if error.1 {
                    workbench.notify_error(error.0);
                }
            })?;
            Ok(false)
        }
    }
}

/// debugSession.ts `initialize` and `launchOrAttach`; the error and whether to show it (`showUser`).
async fn launch_or_attach(this: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, session_id: &str) -> Result<(), (String, bool)> {
    let start = on_workbench(this, cx, |workbench, _, _| {
        let session = workbench.debug.session(session_id)?;
        let dto = session.dto();
        Some(workbench.host.call("ExtHostDebugService", "$startDASession", vec![json!(session.handle), dto]))
    });
    let Ok(Some(start)) = start else { return Err(("Canceled".into(), false)) };
    answer(start).await.map_err(|message| (message, true))?;
    let initialize = on_workbench(this, cx, |workbench, _, _| {
        let adapter_id = workbench.debug.session(session_id).map(|session| session.session_type().to_owned()).unwrap_or_default();
        workbench.dap_request(
            session_id,
            "initialize",
            json!({
                "clientID": "vscode", "clientName": super::menubar::NAME_LONG, "adapterID": adapter_id, "pathFormat": "path",
                "linesStartAt1": true, "columnsStartAt1": true, "supportsVariableType": true, "supportsVariablePaging": true,
                "supportsRunInTerminalRequest": true, "locale": "en", "supportsProgressReporting": true, "supportsInvalidatedEvent": true,
                "supportsMemoryReferences": true, "supportsArgsCanBeInterpretedByShell": true, "supportsMemoryEvent": true,
                "supportsStartDebuggingRequest": true, "supportsANSIStyling": true,
            }),
        )
    })
    .map_err(|message| (message, false))?;
    let capabilities = initialize.await.unwrap_or(Err(Value::Null));
    let capabilities = capabilities.map_err(|error| (error_message(&error), error["showUser"] != false))?;
    let launch = on_workbench(this, cx, |workbench, window, cx| {
        let session = workbench.debug.session_mut(session_id)?;
        session.capabilities = if capabilities.is_object() { capabilities.clone() } else { json!({}) };
        session.initialized = true;
        let filters = session.capabilities["exceptionBreakpointFilters"].as_array().cloned().unwrap_or_default();
        let (request, configuration) = (session.configuration["request"].as_str().unwrap_or("launch").to_owned(), session.configuration.clone());
        workbench.debug.model.set_exception_breakpoints_for_session(session_id, &filters);
        workbench.store_debug_model();
        cx.notify();
        let launch = workbench.dap_request(session_id, &request, configuration);
        if workbench.debug.session_mut(session_id).is_some_and(|session| std::mem::take(&mut session.initialized_event_pending)) {
            workbench.session_initialized(session_id.to_owned(), window, cx);
        }
        Some(launch)
    });
    let Ok(Some(launch)) = launch else { return Err(("Canceled".into(), false)) };
    match launch.await.unwrap_or(Err(Value::Null)) {
        Ok(_) => Ok(()),
        Err(error) => Err((error_message(&error), error["showUser"] != false)),
    }
}

/// configurationResolver.ts `resolveWithInteractionReplace`: `${command:...}` runs the command (through the
/// debugger's `variables`), `${input:...}` asks with the launch's `inputs`; `None` when the user cancels.
async fn resolve_interactive(this: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, mut config: Value, debugger: Option<&Debugger>, inputs: &[Value]) -> Result<Option<Value>, String> {
    let mut resolved = Vec::new();
    for variable in configuration::interactive_variables(&config) {
        let value = match &variable {
            Interactive::Command(name) => {
                let command = debugger.map_or(name.as_str(), |debugger| debugger.variable_command(name)).to_owned();
                let reply = on_workbench(this, cx, |workbench, window, cx| outcome_reply(workbench.execute_command(&command, vec![config.clone()], window, cx)))?;
                match answer(reply).await? {
                    Value::String(text) => Some(text),
                    Value::Null => None,
                    _ => return Err(format!("Cannot substitute command variable '{name}' because command did not return a result of type string.")),
                }
            }
            Interactive::Input(name) => {
                let Some(input) = configuration::input(inputs, name) else { return Err(format!("Undefined input variable '{name}' encountered. Remove or define '{name}' to continue.")) };
                let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_owned);
                match input.get("type").and_then(Value::as_str) {
                    Some("promptString") => {
                        let answer = on_workbench(this, cx, |workbench, window, cx| workbench.debug_input(text("description"), text("default"), input.get("password") == Some(&json!(true)), window, cx))?;
                        answer.await.ok().flatten()
                    }
                    Some("pickString") => {
                        let options: Vec<(String, Option<String>)> = input
                            .get("options")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .map(|option| match option {
                                Value::String(value) => (value.clone(), None),
                                _ => (option["value"].as_str().unwrap_or_default().to_owned(), option["label"].as_str().map(str::to_owned)),
                            })
                            .collect();
                        let placeholder = text("description").unwrap_or_default();
                        let picked = on_workbench(this, cx, |workbench, window, cx| workbench.debug_pick(&placeholder, options.clone(), window, cx))?;
                        picked.await.ok().flatten().and_then(|index| index.parse::<usize>().ok()).map(|index| options[index].0.clone())
                    }
                    Some("command") => {
                        let command = text("command").unwrap_or_default();
                        let args = input.get("args").cloned().map_or_else(Vec::new, |args| vec![args]);
                        let reply = on_workbench(this, cx, |workbench, window, cx| outcome_reply(workbench.execute_command(&command, args, window, cx)))?;
                        match answer(reply).await? {
                            Value::String(text) => Some(text),
                            Value::Null => None,
                            other => Some(other.to_string()),
                        }
                    }
                    _ => return Err(format!("Input variable '{name}' can only be of type 'promptString', 'pickString', or 'command'.")),
                }
            }
        };
        let Some(value) = value else { return Ok(None) };
        resolved.push((variable.variable(), value));
    }
    configuration::replace_variables(&mut config, &resolved);
    Ok(Some(config))
}

fn outcome_reply(outcome: Outcome) -> Reply {
    match outcome {
        Outcome::Ready(result) => {
            let (sender, receiver) = oneshot::channel();
            let _ = sender.send(result);
            receiver
        }
        Outcome::Pending(receiver) => receiver,
    }
}
