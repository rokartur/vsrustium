use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use exthost::session::ForwardedRequest;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::commands::Outcome;
use crate::host_sync::{HostMessage, HostSync};
use crate::languages::Languages;
use crate::output::OUTPUT_SCHEME;
use crate::theme::load_theme;
use crate::workbench::Workbench;

fn open(cx: &mut TestAppContext) -> (gpui::Entity<Workbench>, &mut VisualTestContext, Receiver<HostMessage>, PathBuf) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    static OPENED: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("code-gpui-output-{}-{}", std::process::id(), OPENED.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (sender, receiver) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(sender), cx);
        workbench.watch_editor_focus(window, cx);
        workbench
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (workbench, cx, receiver, dir)
}

fn request(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, proxy: &'static str, method: &str, args: Value) -> Value {
    let request = ForwardedRequest { req: 1, proxy, method: method.into(), args: args.as_array().unwrap().clone() };
    let outcome = workbench.update_in(cx, |workbench, window, cx| workbench.handle_host_request(request, window, cx));
    cx.run_until_parked();
    match outcome {
        Outcome::Ready(result) => result.unwrap(),
        Outcome::Pending(_) => panic!("output requests answer at once"),
    }
}

fn shown_text(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Option<(String, String)> {
    workbench.read_with(cx, |workbench, _| {
        let editor = workbench.editors.active().filter(|editor| editor.document.scheme == OUTPUT_SCHEME)?;
        Some((editor.document.path.to_string_lossy().into_owned(), editor.document.lines.join("\n")))
    })
}

fn visible_channels(receiver: &Receiver<HostMessage>) -> Vec<Value> {
    receiver.try_iter().filter(|message| message.method == "$setVisibleChannel").map(|message| message.args[0].clone()).collect()
}

/// extHostOutput.ts writes the file, then `$update`s; mainThreadOutputService.ts registers, reveals, updates, closes
/// and disposes channels.
#[gpui::test]
fn extension_channels_show_their_file_in_the_output_view(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, dir) = open(cx);
    let file = dir.join("1-hello.log");
    std::fs::write(&file, "").unwrap();
    let id = request(&workbench, cx, "MainThreadOutputService", "$register", json!(["hello", { "scheme": "file", "path": file }, null, "pub.ext"]));
    assert_eq!(id, "extension-output-pub.ext-#1-hello");
    assert_eq!(shown_text(&workbench, cx), None, "registering shows nothing");

    std::fs::write(&file, "one\n").unwrap();
    request(&workbench, cx, "MainThreadOutputService", "$reveal", json!([id, true]));
    assert_eq!(shown_text(&workbench, cx), Some((id.as_str().unwrap().into(), "one\n".into())));
    assert_eq!(visible_channels(&receiver), [id.clone()]);

    std::fs::write(&file, "one\ntwo").unwrap();
    request(&workbench, cx, "MainThreadOutputService", "$update", json!([id, 1]));
    assert_eq!(shown_text(&workbench, cx).unwrap().1, "one\ntwo");
    // clear(): till is the 7 bytes written.
    request(&workbench, cx, "MainThreadOutputService", "$update", json!([id, 3, 7]));
    assert_eq!(shown_text(&workbench, cx).unwrap().1, "");
    std::fs::write(&file, "one\ntwonew").unwrap();
    request(&workbench, cx, "MainThreadOutputService", "$update", json!([id, 2, 7]));
    assert_eq!(shown_text(&workbench, cx).unwrap().1, "new");
    let readonly = workbench.read_with(cx, |workbench, _| workbench.active_editor_readonly());
    assert_eq!(readonly, Some(crate::workbench::readonly::Readonly::Editor));

    request(&workbench, cx, "MainThreadOutputService", "$close", json!([id]));
    assert_eq!(shown_text(&workbench, cx), None);
    assert_eq!(visible_channels(&receiver), [Value::Null]);

    request(&workbench, cx, "MainThreadOutputService", "$dispose", json!([id]));
    assert!(workbench.read_with(cx, |workbench, _| workbench.output.channels.is_empty()));
    std::fs::remove_dir_all(dir).unwrap();
}

/// mainThreadLogService.ts `$registerLogger`: the host's loggers are log channels; a disposed shown channel gives way
/// to the first one left.
#[gpui::test]
fn loggers_are_log_channels(cx: &mut TestAppContext) {
    let (workbench, cx, _receiver, dir) = open(cx);
    for (id, name) in [("exthost", "Extension Host"), ("vscode.git.Git.log", "Git")] {
        let file = dir.join(format!("{id}.log"));
        std::fs::write(&file, format!("{name} log")).unwrap();
        request(&workbench, cx, "MainThreadLogger", "$registerLogger", json!([{ "id": id, "name": name, "resource": { "scheme": "file", "path": file } }]));
    }
    workbench.update_in(cx, |workbench, window, cx| workbench.show_output_channel("vscode.git.Git.log", true, window, cx));
    assert_eq!(shown_text(&workbench, cx), Some(("vscode.git.Git.log".into(), "Git log".into())));
    let language = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document.language_id.clone());
    assert_eq!(language, "log");
    request(&workbench, cx, "MainThreadLogger", "$deregisterLogger", json!([{ "scheme": "file", "path": dir.join("vscode.git.Git.log.log") }]));
    assert_eq!(shown_text(&workbench, cx), Some(("exthost".into(), "Extension Host log".into())));
    let tabs = workbench.read_with(cx, |workbench, _| workbench.editors.editors.len());
    assert_eq!(tabs, 1, "the view's one tab shows the other channel");
    std::fs::remove_dir_all(dir).unwrap();
}

/// viewsService.ts `OpenViewAction`: the toggle opens the view focused, and closes it when it has the focus.
#[gpui::test]
fn toggle_output_opens_and_closes_the_view(cx: &mut TestAppContext) {
    let (workbench, cx, _receiver, dir) = open(cx);
    let file = dir.join("exthost.log");
    std::fs::write(&file, "started").unwrap();
    request(&workbench, cx, "MainThreadLogger", "$registerLogger", json!([{ "id": "exthost", "name": "Extension Host", "resource": { "scheme": "file", "path": file } }]));
    let toggle = |workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext| {
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.output.toggleOutput", vec![], window, cx)));
        cx.run_until_parked();
    };
    toggle(&workbench, cx);
    assert_eq!(shown_text(&workbench, cx), Some(("exthost".into(), "started".into())));
    toggle(&workbench, cx);
    assert_eq!(shown_text(&workbench, cx), None);
    std::fs::remove_dir_all(dir).unwrap();
}
