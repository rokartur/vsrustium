use std::path::Path;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::{ScannedExtension, scan_extension};
use exthost::session::{ForwardedRequest, OBSERVED_REQ};
use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

fn open<'a>(cx: &'a mut TestAppContext, name: &str) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext, std::path::PathBuf) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-errors-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (sender, _receiver) = std::sync::mpsc::channel();
    let log = dir.join("window1/renderer.log");
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(sender), cx);
        workbench.register_window_log(log.clone());
        workbench
    });
    (workbench, cx, log)
}

fn observe(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, proxy: &'static str, method: &str, args: Value) {
    let request = ForwardedRequest { req: OBSERVED_REQ, proxy, method: method.into(), args: args.as_array().unwrap().clone() };
    workbench.update_in(cx, |workbench, window, cx| workbench.handle_host_request(request, window, cx));
}

/// The Window log without its timestamps.
fn logged(log: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    text.lines().filter(|line| line.len() > 24 && line.as_bytes()[4] == b'-').map(|line| line[24..].to_string()).collect()
}

fn console(severity: &str, args: Value) -> Value {
    json!([{ "type": "__$console", "severity": severity, "arguments": args.to_string() }])
}

#[gpui::test]
fn console_errors_reach_the_window_log_and_test_runs_log_every_entry(cx: &mut TestAppContext) {
    let (workbench, cx, log) = open(cx, "console");
    observe(&workbench, cx, "MainThreadConsole", "$logExtensionHostMessage", console("log", json!(["hello"])));
    observe(&workbench, cx, "MainThreadConsole", "$logExtensionHostMessage", console("error", json!(["failed %s", { "a": 1 }, { "__$stack": "at x" }])));
    observe(&workbench, cx, "MainThreadConsole", "$logExtensionHostMessage", console("error", json!([42])));
    assert_eq!(logged(&log), ["[error] [Extension Host] failed %s {\"a\":1}"]);
    workbench.update(cx, |workbench, _| workbench.skip_dialogs = true);
    observe(&workbench, cx, "MainThreadConsole", "$logExtensionHostMessage", console("log", json!(["  1 passing"])));
    observe(&workbench, cx, "MainThreadConsole", "$logExtensionHostMessage", console("warn", json!(["careful"])));
    assert_eq!(&logged(&log)[1..], ["[info]   1 passing", "[warning] careful"]);
}

#[gpui::test]
fn unexpected_errors_log_their_stack_once_a_second(cx: &mut TestAppContext) {
    let (workbench, cx, log) = open(cx, "unexpected");
    let error = json!([{ "$isError": true, "name": "Error", "message": "boom", "stack": "Error: boom\n    at a (b.js:1:2)" }]);
    observe(&workbench, cx, "MainThreadErrors", "$onUnexpectedError", error.clone());
    observe(&workbench, cx, "MainThreadErrors", "$onUnexpectedError", error);
    observe(&workbench, cx, "MainThreadErrors", "$onUnexpectedError", json!(["plain text"]));
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("[error] Error: boom\n    at a (b.js:1:2)\n"), "{text}");
    assert_eq!(text.matches("Error: boom").count(), 1);
    assert!(text.ends_with("[error] plain text\n"), "{text}");
}

#[gpui::test]
fn activation_errors_notify_while_an_extension_is_developed(cx: &mut TestAppContext) {
    let (workbench, cx, _) = open(cx, "activation");
    let failed = |dependency: Value| json!([{ "value": "pub.ext" }, { "$isError": true, "message": "activate threw", "stack": "Error: activate threw" }, dependency]);
    let messages = |workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext| {
        workbench.read_with(cx, |workbench, _| workbench.notifications.model.items.iter().map(|item| item.message.raw.clone()).collect::<Vec<_>>())
    };
    observe(&workbench, cx, "MainThreadExtensionService", "$onExtensionActivationError", failed(Value::Null));
    assert!(messages(&workbench, cx).is_empty(), "a built product only logs");
    workbench.update(cx, |workbench, _| {
        workbench.extension_development = true;
        workbench.extensions.push(ScannedExtension { id: "pub.ext".into(), location: "/x".into(), description: json!({ "name": "ext", "displayName": "My Ext" }) });
    });
    observe(&workbench, cx, "MainThreadExtensionService", "$onExtensionActivationError", failed(Value::Null));
    observe(&workbench, cx, "MainThreadExtensionService", "$onExtensionActivationError", failed(json!({ "dependency": "pub.missing" })));
    assert_eq!(messages(&workbench, cx), ["Cannot activate the 'My Ext' extension because it depends on an unknown 'pub.missing' extension.", "activate threw"]);
}

#[gpui::test]
fn created_loggers_write_through_their_level(cx: &mut TestAppContext) {
    let (workbench, cx, log) = open(cx, "logger");
    let file = log.with_file_name("exthost/custom.log");
    let uri = json!({ "scheme": "vscode-userdata", "path": file });
    let outcome = |workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, method: &str, args: Value| {
        let request = ForwardedRequest { req: 1, proxy: "MainThreadLogger", method: method.into(), args: args.as_array().unwrap().clone() };
        match workbench.update_in(cx, |workbench, window, cx| workbench.handle_host_request(request, window, cx)) {
            crate::commands::Outcome::Ready(result) => result,
            crate::commands::Outcome::Pending(_) => panic!("logger requests answer at once"),
        }
    };
    let error = outcome(&workbench, cx, "$log", json!([uri, [[3, "early"]]])).unwrap_err();
    assert_eq!(error["message"], "Create the logger before logging");
    assert_eq!(outcome(&workbench, cx, "$flush", json!([uri])).unwrap_err()["message"], "Create the logger before flushing");
    outcome(&workbench, cx, "$createLogger", json!([uri, { "name": "Custom", "logLevel": 4 }])).unwrap();
    outcome(&workbench, cx, "$log", json!([uri, [[3, "info dropped"], [5, "kept"]]])).unwrap();
    outcome(&workbench, cx, "$flush", json!([uri])).unwrap();
    assert_eq!(logged(&file), ["[error] kept"]);
    let channel = workbench.read_with(cx, |workbench, _| workbench.output.channels.iter().find(|channel| channel.file == file).map(|channel| channel.label.clone()));
    assert_eq!(channel.as_deref(), Some("Custom"));
    let raw = log.with_file_name("exthost/raw.log");
    let raw_uri = json!({ "scheme": "vscode-userdata", "path": raw });
    outcome(&workbench, cx, "$createLogger", json!([raw_uri, { "logLevel": "always", "donotUseFormatters": true, "hidden": true }])).unwrap();
    outcome(&workbench, cx, "$log", json!([raw_uri, [[1, "trace line"]]])).unwrap();
    assert_eq!(std::fs::read_to_string(&raw).unwrap(), "trace line\n");
}
