//! The workbench's side of `exthost::main_thread::MainThread`: a request the workbench answers goes to the GPUI
//! thread while the session thread waits, and what the workbench queued for the host goes out before the reply.

use std::sync::mpsc::{Receiver, Sender, channel};

use exthost::main_thread::{MainThread, serialized_error};
use exthost::main_thread_files::{Providers, resource};
use exthost::rpc::{Arg, Reply};
use exthost::session::Session;
use files::FileSystem;
use futures::channel::mpsc::UnboundedSender;
use futures::channel::oneshot;
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
    /// Host calls the workbench made with `HostSync::call`, waiting for their answer.
    pub waiting: Vec<(u32, oneshot::Sender<Result<Value, Value>>)>,
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
        if !HOST_REQUESTS.contains(&(proxy, method)) {
            return None;
        }
        // mainThreadFileSystem.ts reads and writes through the extension host; the session thread can call it, the GPUI thread cannot.
        let through_provider = matches!(method, "$tryOpenDocument" | "$trySaveDocument") && {
            let scheme = args[0]["scheme"].as_str().unwrap_or_default();
            scheme != "file" && (Providers { session: &mut *session }).capabilities(scheme).is_some()
        };
        let result = match (method, through_provider) {
            ("$tryOpenDocument", true) => match files::read_file(&mut Providers { session: &mut *session }, &resource(&args[0])) {
                Ok(bytes) => {
                    // host_requests.rs decodes the provider's bytes as it decodes a file.
                    args.resize(2, Value::Null);
                    args.push(json!(bytes));
                    self.ask(proxy, method, args)
                }
                Err(error) => Err(serialized_error(&format!("cannot open {}. Detail: {error}", uri_text(&args[0])))),
            },
            ("$trySaveDocument", true) => self.save_through_provider(session, &args[0]),
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
            let req = session.notify(message.proxy, message.method, message.args);
            if let Some(reply) = message.reply {
                self.waiting.push((req, reply));
            }
        }
        let mut still_waiting = Vec::new();
        for (req, reply) in self.waiting.drain(..) {
            match session.take_result(req) {
                Some(result) => drop(reply.send(result)),
                None => still_waiting.push((req, reply)),
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


fn uri_text(uri: &Value) -> String {
    format!("{}:{}", uri["scheme"].as_str().unwrap_or_default(), uri["path"].as_str().unwrap_or_default())
}
