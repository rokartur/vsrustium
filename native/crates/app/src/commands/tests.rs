use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use super::{Outcome, runnable_commands};
use crate::host_sync::HostSync;
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
