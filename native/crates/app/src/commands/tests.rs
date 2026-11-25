use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use futures::channel::oneshot;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use super::{Commands, Handler, Outcome, error_value, runnable_commands};
use crate::host_sync::{HostMessage, HostSync};
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

#[test]
fn malformed_arguments_are_rejected() {
    assert_eq!(runnable_commands(None), None);
    assert_eq!(runnable_commands(Some(&json!("a"))), None);
    assert_eq!(runnable_commands(Some(&json!({ "commands": "a" }))), None);
    assert_eq!(runnable_commands(Some(&json!({ "commands": ["a", 1] }))), None);
    assert_eq!(runnable_commands(Some(&json!({ "commands": [{ "args": [] }] }))), None);
    assert_eq!(runnable_commands(Some(&json!({ "commands": [] }))), Some(Vec::new()));
}

#[test]
fn array_args_spread_and_other_args_pass_as_one() {
    let commands = runnable_commands(Some(&json!({ "commands": ["a", { "command": "b", "args": [1, 2] }, { "command": "c", "args": { "x": 1 } }, { "command": "d", "args": null }] })));
    let expected = [("a", vec![]), ("b", vec![json!(1), json!(2)]), ("c", vec![json!({ "x": 1 })]), ("d", vec![Value::Null])];
    assert_eq!(commands, Some(expected.map(|(id, args)| (id.to_string(), args)).to_vec()));
}

#[gpui::test]
fn run_commands_runs_in_order_and_stops_at_a_failure(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx));
    let log = Rc::new(RefCell::new(Vec::new()));
    let run = |args: Value, cx: &mut VisualTestContext| {
        log.borrow_mut().clear();
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("runCommands", vec![args], window, cx)));
        cx.run_until_parked();
        Value::Array(log.borrow().clone())
    };
    let append_log = log.clone();
    workbench.update(cx, |workbench, _| {
        workbench.commands.register("test.append", move |_, args, _, _| {
            append_log.borrow_mut().push(Value::Array(args));
            Outcome::done()
        });
        workbench.commands.register("test.fail", |_, _, _, _| Outcome::error("failed"));
    });

    let calls = run(json!({ "commands": ["test.append", { "command": "test.append", "args": [1, 2] }, { "command": "test.append", "args": "x" }] }), cx);
    assert_eq!(calls, json!([[], [1, 2], ["x"]]));
    assert_eq!(run(json!({ "commands": ["test.append", "test.fail", "test.append"] }), cx), json!([[]]));
    assert_eq!(run(json!({ "commands": ["test.append", "test.missing", "test.append"] }), cx), json!([[]]));
    assert_eq!(run(json!({ "commands": ["test.append", 3] }), cx), json!([]));
}

fn workbench_with_host(cx: &mut TestAppContext) -> (gpui::Entity<Workbench>, &mut VisualTestContext, std::sync::mpsc::Receiver<HostMessage>) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let (sender, host) = std::sync::mpsc::channel();
    let mut host_sync = HostSync::new(sender);
    host_sync.installed_extensions_registered = true;
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), host_sync, cx));
    (workbench, cx, host)
}

/// The `$activateByEvent` requests the host received since the last call, with their reply channels.
fn activations(host: &std::sync::mpsc::Receiver<HostMessage>) -> Vec<(String, oneshot::Sender<Result<Value, Value>>)> {
    host.try_iter().filter(|message| message.method == "$activateByEvent").map(|message| (message.args[0].as_str().unwrap().to_owned(), message.reply.unwrap())).collect()
}

/// Runs `id` and fills the returned cell once its outcome is ready.
fn run(workbench: &gpui::Entity<Workbench>, id: &str, cx: &mut VisualTestContext) -> Rc<RefCell<Option<Result<Value, Value>>>> {
    let result = Rc::new(RefCell::new(None));
    let outcome = workbench.update_in(cx, |workbench, window, cx| workbench.execute_command(id, Vec::new(), window, cx));
    match outcome {
        Outcome::Ready(ready) => *result.borrow_mut() = Some(ready),
        Outcome::Pending(pending) => {
            let result = result.clone();
            cx.spawn(async move |_| *result.borrow_mut() = Some(pending.await.unwrap_or(Ok(Value::Null)))).detach();
        }
    }
    result
}

// commands.test.ts 'register/dispose' and 'register/register/dispose'; 'register command - no handler' cannot be
// written in Rust, and no port command declares the argument constraints 'command with description' checks.
#[test]
fn a_disposed_registration_brings_back_the_one_under_it() {
    let mut commands = Commands::default();
    let same = |a: Option<Handler>, b: &Option<Handler>| match (a, b) {
        (Some(a), Some(b)) => Rc::ptr_eq(&a, b),
        (a, b) => a.is_none() && b.is_none(),
    };

    let reg = commands.register("foo", |_, _, _, _| Outcome::done());
    assert!(commands.get("foo").is_some());
    commands.dispose("foo", reg);
    assert!(commands.get("foo").is_none());

    let reg1 = commands.register("foo", |_, _, _, _| Outcome::done());
    let command1 = commands.get("foo");
    let reg2 = commands.register("foo", |_, _, _, _| Outcome::done());
    assert!(!same(commands.get("foo"), &command1));
    commands.dispose("foo", reg2);
    assert!(same(commands.get("foo"), &command1));
    commands.dispose("foo", reg1);
    assert!(commands.get("foo").is_none());

    // dispose override command first
    let reg1 = commands.register("foo", |_, _, _, _| Outcome::done());
    let reg2 = commands.register("foo", |_, _, _, _| Outcome::done());
    let command2 = commands.get("foo");
    commands.dispose("foo", reg1);
    assert!(same(commands.get("foo"), &command2));
    commands.dispose("foo", reg2);
    assert!(commands.get("foo").is_none());
    assert_eq!(commands.ids().count(), 0);
}

// commandService.test.ts 'activateOnCommand' and 'fwd activation error'.
#[gpui::test]
fn execute_command_activates_on_command_and_forwards_its_error(cx: &mut TestAppContext) {
    let (workbench, cx, host) = workbench_with_host(cx);
    let result = run(&workbench, "foo", cx);
    let sent = activations(&host);
    assert_eq!(sent.iter().map(|(event, _)| event.as_str()).collect::<Vec<_>>(), ["onCommand:foo"]);
    for (_, reply) in sent {
        reply.send(Err(error_value("bad_activate"))).unwrap();
    }
    cx.run_until_parked();
    assert_eq!(result.borrow().as_ref().unwrap().as_ref().unwrap_err()["message"], "bad_activate");

    let result = run(&workbench, "unknownCommandId", cx);
    for (_, reply) in activations(&host) {
        reply.send(Ok(Value::Null)).unwrap();
    }
    cx.run_until_parked();
    assert_eq!(result.borrow().as_ref().unwrap().as_ref().unwrap_err()["message"], "command 'unknownCommandId' not found");
}

// commandService.test.ts 'issue #71471: wait for onCommand activation even if a command is registered' and
// 'issue #142155: execute commands synchronously if possible'. The port activates '*' before it serves any request,
// so the race against '*' activation of '#62457' does not apply.
#[gpui::test]
fn a_registered_command_waits_for_its_activation_once_then_runs_synchronously(cx: &mut TestAppContext) {
    let (workbench, cx, host) = workbench_with_host(cx);
    let order = Rc::new(RefCell::new(Vec::new()));
    let executed = order.clone();
    workbench.update(cx, |workbench, _| {
        workbench.commands.register("bizBaz", move |_, _, _, _| {
            executed.borrow_mut().push("executing command");
            Outcome::done()
        });
    });

    let first = run(&workbench, "bizBaz", cx);
    let second = run(&workbench, "bizBaz", cx);
    cx.run_until_parked();
    assert!(order.borrow().is_empty(), "waits for onCommand:bizBaz");
    let sent = activations(&host);
    assert_eq!(sent.len(), 1, "the second call shares the first activation");
    order.borrow_mut().push("resolving activation event");
    for (_, reply) in sent {
        reply.send(Ok(Value::Null)).unwrap();
    }
    cx.run_until_parked();
    assert_eq!(*order.borrow(), ["resolving activation event", "executing command", "executing command"]);
    assert!(first.borrow().is_some() && second.borrow().is_some());

    order.borrow_mut().clear();
    order.borrow_mut().push("before call");
    let third = run(&workbench, "bizBaz", cx);
    order.borrow_mut().push("after call");
    assert_eq!(*order.borrow(), ["before call", "executing command", "after call"]);
    assert!(third.borrow().is_some(), "resolved without a tick");
}

// commandService.test.ts '!onReady, but executeCommand' and 'issue #34913: !onReady, unknown command'.
#[gpui::test]
fn before_the_host_is_up_a_registered_command_runs_without_waiting(cx: &mut TestAppContext) {
    let (workbench, cx, host) = workbench_with_host(cx);
    workbench.update(cx, |workbench, _| {
        workbench.host.installed_extensions_registered = false;
        workbench.commands.register("bar", |_, _, _, _| Outcome::Ready(Ok(json!(true))));
    });
    assert_eq!(*run(&workbench, "bar", cx).borrow(), Some(Ok(json!(true))));
    assert_eq!(activations(&host).len(), 1, "the activation is still sent");

    let unknown = run(&workbench, "unknownCommandId", cx);
    cx.run_until_parked();
    assert!(unknown.borrow().is_none(), "an unknown command waits for its activation");
    for (_, reply) in activations(&host) {
        reply.send(Ok(Value::Null)).unwrap();
    }
    cx.run_until_parked();
    assert_eq!(unknown.borrow().as_ref().unwrap().as_ref().unwrap_err()["message"], "command 'unknownCommandId' not found");
}
