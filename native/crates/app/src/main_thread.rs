//! The workbench's side of `exthost::main_thread::MainThread`: a request the workbench answers goes to the GPUI
//! thread while the session thread waits, and what the workbench queued for the host goes out before the reply.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use exthost::extension_management::WindowExtensions;
use exthost::main_thread::{MainThread, serialized_error};
use exthost::main_thread_files::{Providers, resource};
use exthost::rpc::{Arg, Reply};
use exthost::session::{CallResult, Session, URL_PROTOCOL};
use files::FileSystem;
use futures::channel::mpsc::UnboundedSender;
use futures::channel::oneshot;
use serde_json::{Value, json};

use crate::host_sync::HostMessage;
use crate::uri::Uri;
use crate::workbench::host_requests::HOST_REQUESTS;

pub struct HostRequest {
    pub proxy: String,
    pub method: String,
    pub args: Vec<Value>,
    pub reply: Sender<Result<Value, Value>>,
}

pub struct WorkbenchMainThread {
    pub requests: UnboundedSender<HostRequest>,
    pub messages: Receiver<HostMessage>,
    /// Host calls the workbench made with `HostSync::call`, waiting for their answer, and whether a dropped receiver
    /// still has to cancel the request.
    pub waiting: Vec<(u32, bool, oneshot::Sender<Result<Value, Value>>)>,
    /// `$registerTextContentProvider` handles and schemes, newest first as textModelResolverService.ts keeps them.
    pub content_providers: Vec<(Value, String)>,
    pub extensions: WindowExtensions,
}

impl MainThread for WorkbenchMainThread {
    fn handle(&mut self, session: &mut Session, proxy: &str, method: &str, args: &[Arg]) -> Option<Result<Reply, Value>> {
        let mut args: Vec<Value> = args
            .iter()
            .map(|arg| match arg {
                Arg::Json(value) => value.clone(),
                _ => Value::Null,
            })
            .collect();
        match (proxy, method) {
            ("MainThreadDocumentContentProviders", "$registerTextContentProvider") => {
                self.content_providers.insert(0, (args[0].clone(), args[1].as_str().unwrap_or_default().to_string()));
                return Some(Ok(Reply::Empty));
            }
            ("MainThreadDocumentContentProviders", "$unregisterTextContentProvider") => {
                self.content_providers.retain(|(handle, _)| *handle != args[0]);
                return Some(Ok(Reply::Empty));
            }
            ("MainThreadWindow", "$asExternalUri") => return Some(as_external_uri(session, &args[0])),
            // Recorded for Developer: Startup Performance; the session still handles them.
            ("MainThreadExtensionService", "$setPerformanceMarks") => {
                crate::perf::set_ext_host_marks(&args[0]);
                return None;
            }
            ("MainThreadExtensionService", "$onDidActivateExtension") => {
                crate::perf::extension_activated(&args);
                return None;
            }
            _ if !HOST_REQUESTS.contains(&(proxy, method)) => return None,
            _ => {}
        }
        // mainThreadFileSystem.ts reads and writes through the extension host; the session thread can call it, the GPUI thread cannot.
        // Other host requests can come without arguments; only these two carry the document uri first.
        let is_document_request = matches!(method, "$tryOpenDocument" | "$trySaveDocument");
        let scheme = if is_document_request { args[0]["scheme"].as_str().unwrap_or_default() } else { "" };
        let capabilities = if is_document_request { (Providers { session: &mut *session }).capabilities(scheme) } else { None };
        // host_requests.rs opens `vscode` documents: the default settings content lives in the workbench.
        let through_provider = scheme != "file" && scheme != "vscode" && capabilities.is_some();
        if method == "$tryOpenDocument" {
            // host_requests.rs reads the provider's bytes at 2 and whether paths ignore case at 3.
            args.resize(3, Value::Null);
            args.push(json!(capabilities.is_some_and(|capabilities| capabilities & files::PATH_CASE_SENSITIVE == 0)));
        }
        if method == "$trySaveDocument" && self.ask("workbench", "isDirty", vec![args[0].clone()]) == Ok(json!(true)) {
            participate_in_save(session, &args[0]);
        }
        let result = match (method, through_provider) {
            ("$tryOpenDocument", true) => match files::read_file(&mut Providers { session: &mut *session }, &resource(&args[0])) {
                Ok(bytes) => {
                    args[2] = json!(bytes);
                    self.ask(proxy, method, args)
                }
                Err(error) => Err(serialized_error(&format!("cannot open {}. Detail: {error}", uri_text(&args[0])))),
            },
            ("$trySaveDocument", true) => self.save_through_provider(session, &args[0]),
            ("$tryOpenDocument", false) if self.has_content_provider(&args[0]) && self.ask("workbench", "hasModel", vec![args[0].clone()]) == Ok(json!(false)) => {
                match self.provide_text_content(session, &args[0]) {
                    Ok(text) => {
                        args[2] = json!(text);
                        self.ask(proxy, method, args)
                    }
                    Err(detail) => Err(serialized_error(&format!("cannot open {}. Detail: {detail}", uri_text(&args[0])))),
                }
            }
            _ => self.ask(proxy, method, args),
        };
        Some(result.map(|value| if value.is_null() { Reply::Empty } else { Reply::Json(value) }))
    }

    fn flush(&mut self, session: &mut Session) {
        for message in self.messages.try_iter() {
            if let Some(work) = message.files {
                work.run(&mut Providers { session: &mut *session });
                continue;
            }
            if let Some((job, reply)) = message.extensions {
                let _ = reply.send(self.extensions.run(session, job).map_err(|error| format!("{error:#}")));
                continue;
            }
            if (message.proxy, message.method) == ("workbench", "stopExtensionHost") {
                session.host.terminate();
                continue;
            }
            if (message.proxy, message.method) == ("workbench", "resolveTextContent") {
                let content = self.resolve_text_content(session, &message.args[0]);
                drop(message.reply.expect("resolveTextContent waits for its answer").send(content));
                continue;
            }
            let req = if message.cancellable {
                session.notify_cancellable(message.proxy, message.method, message.args)
            } else {
                let mut args: Vec<Arg> = message.args.into_iter().map(Arg::Json).collect();
                // extHostWebview.ts `$onMessage` reads its last argument as a SerializableObjectWithBuffers of VSBuffers,
                // which the workbench passes as base64 strings.
                if (message.proxy, message.method) == ("ExtHostWebviews", "$onMessage") && let Some(Arg::Json(encoded)) = args.pop() {
                    let buffers: Vec<Vec<u8>> = encoded.as_array().into_iter().flatten().map(|text| STANDARD.decode(text.as_str().unwrap_or_default()).unwrap_or_default()).collect();
                    let json = (0..buffers.len()).map(|index| json!({ "$$ref$$": index })).collect();
                    args.push(Arg::JsonWithBuffers { json, buffers });
                }
                // webviewViewPane.ts passes the title it was never given as `undefined`, which the view's getter returns.
                if (message.proxy, message.method) == ("ExtHostWebviewViews", "$resolveWebviewView") && matches!(args.get(2), Some(Arg::Json(Value::Null))) {
                    args[2] = Arg::Undefined;
                }
                session.send_request(message.proxy, message.method, args)
            };
            if let Some(reply) = message.reply {
                self.waiting.push((req, message.cancellable, reply));
            }
        }
        let mut still_waiting = Vec::new();
        for (req, cancellable, reply) in self.waiting.drain(..) {
            match session.take_result(req) {
                Some(result) => drop(reply.send(result)),
                None if cancellable && reply.is_canceled() => {
                    session.cancel(req);
                    still_waiting.push((req, false, reply));
                }
                None => still_waiting.push((req, cancellable, reply)),
            }
        }
        self.waiting = still_waiting;
    }
}

impl WorkbenchMainThread {
    /// Runs one request on the GPUI thread and waits for its answer.
    fn ask(&self, proxy: &str, method: &str, args: Vec<Value>) -> Result<Value, Value> {
        let (reply, receiver) = channel();
        let request = HostRequest { proxy: proxy.to_string(), method: method.to_string(), args, reply };
        // The workbench is gone once its window closes; the host hears the request was canceled.
        if self.requests.unbounded_send(request).is_err() {
            return Err(serialized_error("Canceled"));
        }
        receiver.recv().unwrap_or_else(|_| Err(serialized_error("Canceled")))
    }

    fn has_content_provider(&self, uri: &Value) -> bool {
        self.content_providers.iter().any(|(_, scheme)| uri["scheme"] == json!(scheme))
    }

    /// `ResourceModelCollection.ensureResolvedTextModelContent`: the first provider of the scheme that gives a string;
    /// a provider that fails fails the open.
    fn provide_text_content(&self, session: &mut Session, uri: &Value) -> Result<String, String> {
        let handles: Vec<Value> = self.content_providers.iter().filter(|(_, scheme)| uri["scheme"] == json!(scheme)).map(|(handle, _)| handle.clone()).collect();
        for handle in handles {
            let args = vec![Arg::Json(handle), Arg::Json(uri.clone())];
            match session.request("ExtHostDocumentContentProviders", "$provideTextDocumentContent", args, PROVIDER_TIMEOUT) {
                Ok(CallResult::Ok(Reply::Json(Value::String(text)))) => return Ok(text),
                Ok(CallResult::Ok(_)) => {}
                Ok(CallResult::Err(error)) => return Err(error.and_then(|error| error["message"].as_str().map(String::from)).unwrap_or_default()),
                Err(error) => return Err(error.to_string()),
            }
        }
        Err(format!("Unable to resolve text model content for resource {}", uri_text(uri)))
    }

    /// `ITextModelService.createModelReference` for a resource the workbench has no model of: the text of the scheme's
    /// content provider, else the bytes of its file system provider, in the form `$tryOpenDocument` reads at 2.
    fn resolve_text_content(&self, session: &mut Session, uri: &Value) -> Result<Value, Value> {
        if self.has_content_provider(uri) {
            return self.provide_text_content(session, uri).map(Value::String).map_err(|detail| serialized_error(&format!("cannot open {}. Detail: {detail}", uri_text(uri))));
        }
        match files::read_file(&mut Providers { session }, &resource(uri)) {
            Ok(bytes) => Ok(json!(bytes)),
            Err(error) => Err(serialized_error(&format!("cannot open {}. Detail: {error}", uri_text(uri)))),
        }
    }

    /// `$trySaveDocument` of a provider's document: the workbench gives the bytes, the provider writes it, the model is clean.
    fn save_through_provider(&self, session: &mut Session, uri: &Value) -> Result<Value, Value> {
        let bytes = match self.ask("workbench", "providerSaveText", vec![uri.clone()])? {
            bytes @ Value::Array(_) => serde_json::from_value::<Vec<u8>>(bytes).expect("bytes from the workbench"),
            saved_or_missing => return Ok(saved_or_missing),
        };
        if let Err(error) = files::write_file(&mut Providers { session }, &resource(uri), &bytes) {
            return Err(serialized_error(&format!("Unable to write file '{}' ({error})", uri_text(uri))));
        }
        self.ask("workbench", "providerSaved", vec![uri.clone()])
    }
}


/// The time `$provideTextDocumentContent` gets, as main_thread_files.rs gives a file system provider.
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(300);

/// mainThreadSaveParticipant.ts: the host's `onWillSaveTextDocument` listeners apply their edits through
/// `$tryApplyWorkspaceEdit` while this waits; a failed or late listener is logged and the save goes on.
fn participate_in_save(session: &mut Session, uri: &Value) {
    // SaveReason.EXPLICIT
    let args = vec![uri.clone(), json!(1)];
    if session.request_cancellable("ExtHostDocumentSaveParticipant", "$participateInSave", args, Duration::from_millis(1750)).is_none() {
        eprintln!("[exthost] Aborted onWillSaveTextDocument-event after 1750ms");
    }
}

/// window.ts `resolveExternalUri` without tunnels: a resource a file system provider serves becomes an app uri that
/// opens it; nothing else resolves, and extHost.api.impl.ts then keeps an http or https uri as it is.
fn as_external_uri(session: &mut Session, uri: &Value) -> Result<Reply, Value> {
    let uri = Uri::from_json(uri).map_err(|error| serialized_error(&error))?;
    if (Providers { session }).capabilities(&uri.scheme).is_none() {
        return Err(serialized_error(&format!("Could not resolve external URI: {}", uri.to_string(false))));
    }
    Ok(Reply::Json(json!({ "$mid": 1, "scheme": URL_PROTOCOL, "path": "workspace", "query": uri.to_string(false) })))
}

fn uri_text(uri: &Value) -> String {
    let part = |key: &str| uri[key].as_str().unwrap_or_default();
    match part("authority") {
        "" => format!("{}:{}", part("scheme"), part("path")),
        authority => format!("{}://{authority}{}", part("scheme"), part("path")),
    }
}
