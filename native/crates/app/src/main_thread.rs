//! The workbench's side of `exthost::main_thread::MainThread`: a request the workbench answers goes to the GPUI
//! thread while the session thread waits, and what the workbench queued for the host goes out before the reply.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use exthost::main_thread::{MainThread, serialized_error};
use exthost::rpc::{Arg, Reply};
use exthost::session::Session;
use futures::channel::mpsc::UnboundedSender;
use serde_json::{Value, json};

use crate::host_sync::HostMessage;
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
    /// Extension file system providers by scheme, with their handle (`$registerFileSystemProvider`).
    pub providers: HashMap<String, Value>,
}

/// mainThreadFileSystem.ts reads and writes through the extension host; the session thread can call it, the GPUI thread cannot.
const PROVIDER_CALL_TIMEOUT: Duration = Duration::from_secs(30);

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
            // Noted here, still answered by the session.
            ("MainThreadFileSystem", "$registerFileSystemProvider") => {
                self.providers.insert(args[1].as_str().unwrap_or_default().to_string(), args[0].clone());
                return None;
            }
            ("MainThreadFileSystem", "$unregisterProvider") => {
                self.providers.retain(|_, handle| *handle != args[0]);
                return None;
            }
            _ if !HOST_REQUESTS.contains(&(proxy, method)) => return None,
            _ => {}
        }
        let provider = args.first().and_then(|uri| uri["scheme"].as_str()).and_then(|scheme| self.providers.get(scheme)).cloned();
        let result = match (method, provider) {
            ("$tryOpenDocument", Some(handle)) => match read_file(session, &handle, &args[0]) {
                Ok(bytes) => {
                    // host_requests.rs decodes the provider's bytes as it decodes a file.
                    args.resize(2, Value::Null);
                    args.push(json!(bytes));
                    self.ask(proxy, method, args)
                }
                Err(error) => Err(serialized_error(&format!("cannot open {}. Detail: {error:#}", uri_text(&args[0])))),
            },
            ("$trySaveDocument", Some(handle)) => self.save_through_provider(session, &handle, &args[0]),
            _ => self.ask(proxy, method, args),
        };
        Some(result.map(|value| if value.is_null() { Reply::Empty } else { Reply::Json(value) }))
    }

    fn flush(&mut self, session: &mut Session) {
        for message in self.messages.try_iter() {
            session.notify(message.proxy, message.method, message.args);
        }
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

    /// `$trySaveDocument` of a provider's document: the workbench gives the bytes, the provider writes it, the model is clean.
    fn save_through_provider(&self, session: &mut Session, handle: &Value, uri: &Value) -> Result<Value, Value> {
        let bytes = match self.ask("workbench", "providerSaveText", vec![uri.clone()])? {
            bytes @ Value::Array(_) => serde_json::from_value::<Vec<u8>>(bytes).expect("bytes from the workbench"),
            saved_or_missing => return Ok(saved_or_missing),
        };
        let options = json!({ "create": true, "overwrite": true, "unlock": false, "atomic": false });
        let written = session.call("ExtHostFileSystem", "$writeFile", vec![Arg::Json(handle.clone()), Arg::Json(uri.clone()), Arg::Buffer(bytes), Arg::Json(options)], PROVIDER_CALL_TIMEOUT);
        if let Err(error) = written {
            return Err(serialized_error(&format!("Unable to write file '{}' ({error:#})", uri_text(uri))));
        }
        self.ask("workbench", "providerSaved", vec![uri.clone()])
    }
}

fn read_file(session: &mut Session, handle: &Value, uri: &Value) -> anyhow::Result<Vec<u8>> {
    match session.call("ExtHostFileSystem", "$readFile", vec![Arg::Json(handle.clone()), Arg::Json(uri.clone())], PROVIDER_CALL_TIMEOUT)? {
        Reply::Buffer(bytes) => Ok(bytes),
        reply => anyhow::bail!("$readFile replied {reply:?}"),
    }
}

fn uri_text(uri: &Value) -> String {
    format!("{}:{}", uri["scheme"].as_str().unwrap_or_default(), uri["path"].as_str().unwrap_or_default())
}
