//! Cases of debugUtils.test.ts `convertPaths` and the session's request bookkeeping.

use futures::channel::oneshot;
use serde_json::json;

use super::*;

#[test]
fn request_paths_become_uris_and_response_uris_become_paths() {
    let request = json!({ "type": "request", "seq": 1, "command": "setBreakpoints", "arguments": { "source": { "path": "/a/b.js" }, "breakpoints": [{ "line": 2 }] } });
    let converted = to_adapter_paths(&request);
    assert_eq!(converted["arguments"]["source"]["path"], json!({ "$mid": 1, "scheme": "file", "authority": "", "path": "/a/b.js", "query": "", "fragment": "" }));
    assert_eq!(request["arguments"]["source"]["path"], "/a/b.js", "the original message is left as it was");

    let with_reference = json!({ "type": "request", "command": "source", "arguments": { "source": { "path": "/a/b.js", "sourceReference": 3 } } });
    assert_eq!(to_adapter_paths(&with_reference), with_reference);
    let relative = json!({ "type": "request", "command": "source", "arguments": { "source": { "path": "b.js" } } });
    assert_eq!(to_adapter_paths(&relative), relative);
    let uri = json!({ "type": "request", "command": "source", "arguments": { "source": { "path": "untitled:Untitled-1" } } });
    assert_eq!(to_adapter_paths(&uri)["arguments"]["source"]["path"]["scheme"], "untitled");

    let response = json!({ "type": "response", "success": true, "command": "stackTrace", "body": { "stackFrames": [{ "id": 1, "source": { "path": { "scheme": "file", "path": "/a/b.js" } } }, { "id": 2 }] } });
    let converted = from_adapter_paths(&response);
    assert_eq!(converted["body"]["stackFrames"][0]["source"]["path"], "/a/b.js");
    let event = json!({ "type": "event", "event": "output", "body": { "output": "x", "source": { "path": { "scheme": "untitled", "path": "Untitled-1" } } } });
    assert_eq!(from_adapter_paths(&event)["body"]["source"]["path"], "untitled:Untitled-1");
}

#[test]
fn requests_count_up_and_responses_answer_them() {
    let mut session = Session::new("s".into(), 1, json!({ "type": "node", "name": "Launch" }), json!({}), None, json!({}));
    let (first, mut first_answer) = oneshot::channel();
    let (second, mut second_answer) = oneshot::channel();
    assert_eq!(session.request("threads", serde_json::Value::Null, first), json!({ "command": "threads", "type": "request", "seq": 1 }));
    assert_eq!(session.request("evaluate", json!({ "expression": "x" }), second)["seq"], 2);
    session.accept_response(&json!({ "type": "response", "request_seq": 1, "success": true, "command": "threads", "body": { "threads": [] } }));
    assert_eq!(first_answer.try_recv().unwrap(), Some(Ok(json!({ "threads": [] }))));
    session.accept_response(&json!({ "type": "response", "request_seq": 2, "success": false, "command": "evaluate", "message": "x", "body": { "error": { "format": "'{name}' is not defined", "variables": { "name": "x" } } } }));
    assert_eq!(second_answer.try_recv().unwrap().unwrap().unwrap_err()["message"], "'x' is not defined");
}

#[test]
fn stopped_and_continued_events_move_the_threads() {
    let mut session = Session::new("s".into(), 1, json!({ "type": "node", "name": "Launch" }), json!({}), None, json!({}));
    session.initialized = true;
    session.set_threads(&[json!({ "id": 1, "name": "main" }), json!({ "id": 2, "name": "worker" })]);
    assert_eq!(session.state(None), State::Running);
    session.stop_threads(&json!({ "reason": "breakpoint", "threadId": 1 }));
    assert_eq!(session.state(None), State::Stopped);
    assert_eq!(session.state(Some(2)), State::Running);
    assert_eq!(session.thread(1).unwrap().state_label(), "Paused on breakpoint");
    session.set_threads(&[json!({ "id": 1, "name": "main thread" })]);
    assert!(session.thread(1).unwrap().stopped, "a listed thread keeps its state");
    session.continue_threads(None);
    assert_eq!(session.state(None), State::Running);
}
