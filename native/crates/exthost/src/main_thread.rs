//! The seam where the workbench answers MainThread* requests that need its state (documents, editors,
//! clipboard), while `Session` keeps the ones it can answer alone.

use std::collections::VecDeque;

use serde_json::{Value, json};

use crate::rpc::{Arg, Reply};
use crate::session::Session;

pub trait MainThread: Send {
    /// Answers a host request, or `None` to leave it to the session. `Err` is a `SerializedError`.
    /// The handler may call the host through `session` while it works (a file system provider read).
    fn handle(&mut self, session: &mut Session, proxy: &str, method: &str, args: &[Arg]) -> Option<Result<Reply, Value>>;

    /// Sends what the workbench queued for the host since the last call.
    fn flush(&mut self, session: &mut Session);
}

/// Host requests that arrived while the main thread handler was calling the host; served once it returns.
#[derive(Default)]
pub(crate) struct Waiting {
    pub busy: bool,
    pub requests: VecDeque<(u32, u8, String, Vec<Arg>)>,
}

/// `errors.ts` `transformErrorForSerialization` of an `Error` with this message.
pub fn serialized_error(message: &str) -> Value {
    json!({ "$isError": true, "name": "Error", "message": message, "stack": format!("Error: {message}") })
}
