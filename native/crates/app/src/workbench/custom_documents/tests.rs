use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::json;

use super::*;
use crate::host_sync::{HostMessage, HostSync};
use crate::languages::Languages;
use crate::theme::load_theme;

const VIEW_TYPE: &str = "test.edit";

fn scratch() -> PathBuf {
    static MADE: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("vsrustium-custom-documents-{}-{}", std::process::id(), MADE.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(dir.join("folder")).unwrap();
    std::fs::create_dir_all(dir.join("user/User")).unwrap();
    std::fs::write(dir.join("folder/a.bin"), "a").unwrap();
    dir
}

/// A window on `<dir>/folder` with its backups in `<dir>/user` and the `test.edit` provider registered.
fn open<'a>(cx: &'a mut TestAppContext, dir: &Path) -> (gpui::Entity<Workbench>, Receiver<HostMessage>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let workspace = config::workspace::Workspace::folder(dir.join("folder"));
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("user/User"), Some(workspace.clone()))));
    let (sender, receiver) = std::sync::mpsc::channel();
    let user_data = dir.join("user");
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(sender), cx);
        workbench.start_backups(&user_data, None, window, cx);
        workbench
    });
    cx.run_until_parked();
    (workbench, receiver, cx)
}

fn register_provider(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) {
    let extension = json!({ "id": "port.test", "location": { "scheme": "file", "path": "/ext" } });
    let args = [extension, json!(VIEW_TYPE), json!({}), json!({}), json!(false), json!(false)];
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.handle_custom_editors_request("$registerCustomEditorProvider", &args, window, cx)));
    cx.run_until_parked();
}

fn resource(dir: &Path) -> Value {
    Uri::new("file", "", &dir.join("folder/a.bin").to_string_lossy(), "", "").unwrap().to_json()
}

/// The next message to `method`, the ones before it dropped (their calls cancelled).
fn next(receiver: &Receiver<HostMessage>, method: &str) -> HostMessage {
    loop {
        let message = receiver.try_recv().unwrap_or_else(|_| panic!("no {method}"));
        if message.method == method {
            return message;
        }
    }
}

fn answer(message: HostMessage, value: Value, cx: &mut VisualTestContext) {
    let _ = message.reply.expect("a call").send(Ok(value));
    cx.run_until_parked();
}

fn run(workbench: &gpui::Entity<Workbench>, command: &str, cx: &mut VisualTestContext) {
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(command, Vec::new(), window, cx)));
    cx.run_until_parked();
}

fn host_event(workbench: &gpui::Entity<Workbench>, method: &str, args: Vec<Value>, cx: &mut VisualTestContext) {
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.handle_custom_editors_request(method, &args, window, cx)));
    cx.run_until_parked();
}

fn active_dirty(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> bool {
    workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().is_dirty())
}

/// Opens a.bin in the editable `test.edit` editor: the host creates the document and resolves the webview.
fn open_document(workbench: &gpui::Entity<Workbench>, receiver: &Receiver<HostMessage>, dir: &Path, cx: &mut VisualTestContext) {
    let resource = resource(dir);
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.open_with_resolver(&resource, Some(VIEW_TYPE), json!({ "pinned": true }), window, cx)));
    cx.run_until_parked();
    answer(next(receiver, "$createCustomDocument"), json!({ "editable": true }), cx);
    answer(next(receiver, "$resolveCustomEditor"), Value::Null, cx);
}

#[gpui::test]
fn edits_undo_redo_save_and_revert_through_the_host(cx: &mut TestAppContext) {
    let dir = scratch();
    let (workbench, receiver, cx) = open(cx, &dir);
    register_provider(&workbench, cx);
    open_document(&workbench, &receiver, &dir, cx);
    let resource = resource(&dir);
    assert!(!active_dirty(&workbench, cx));

    // As the host revives and sends it: `$mid` and `external` besides the parts the workbench sent.
    let mut host_resource = resource.clone();
    host_resource["$mid"] = json!(1);
    host_resource["external"] = json!(format!("file://{}", dir.join("folder/a.bin").display()));
    host_event(&workbench, "$onDidEdit", vec![host_resource, json!(VIEW_TYPE), json!(1), json!("Edit")], cx);
    assert!(active_dirty(&workbench, cx));

    run(&workbench, "undo", cx);
    let undo = next(&receiver, "$undo");
    assert_eq!(undo.args[2..], [json!(1), json!(false)]);
    assert!(!active_dirty(&workbench, cx));

    run(&workbench, "redo", cx);
    assert_eq!(next(&receiver, "$redo").args[2..], [json!(1), json!(true)]);
    assert!(active_dirty(&workbench, cx));

    run(&workbench, "workbench.action.files.save", cx);
    answer(next(&receiver, "$onSave"), Value::Null, cx);
    assert!(!active_dirty(&workbench, cx));

    host_event(&workbench, "$onDidEdit", vec![resource.clone(), json!(VIEW_TYPE), json!(2), json!("Edit")], cx);
    run(&workbench, "undo", cx);
    // A new edit drops the undone one.
    host_event(&workbench, "$onDidEdit", vec![resource.clone(), json!(VIEW_TYPE), json!(3), json!("Edit")], cx);
    assert_eq!(next(&receiver, "$disposeEdits").args[2], json!([2]));
    assert!(active_dirty(&workbench, cx));

    run(&workbench, "workbench.action.files.revert", cx);
    next(&receiver, "$revert");
    assert_eq!(next(&receiver, "$disposeEdits").args[2], json!([3]));
    assert!(!active_dirty(&workbench, cx));

    host_event(&workbench, "$onContentChange", vec![resource, json!(VIEW_TYPE)], cx);
    assert!(active_dirty(&workbench, cx));
    let _ = std::fs::remove_dir_all(dir);
}

#[gpui::test]
fn a_dirty_document_backs_up_and_opens_from_its_backup(cx: &mut TestAppContext) {
    let dir = scratch();
    let resource = resource(&dir);
    {
        let (workbench, receiver, cx) = open(cx, &dir);
        register_provider(&workbench, cx);
        open_document(&workbench, &receiver, &dir, cx);
        host_event(&workbench, "$onDidEdit", vec![resource.clone(), json!(VIEW_TYPE), json!(1), json!("Edit")], cx);
        cx.executor().advance_clock(std::time::Duration::from_millis(1000));
        cx.run_until_parked();
        let backup = next(&receiver, "$backup");
        assert_eq!(backup.args[..2], [resource.clone(), json!(VIEW_TYPE)]);
        answer(backup, json!("backup-7"), cx);
        cx.run_until_parked();
    }
    let home = std::fs::read_dir(dir.join("user/Backups")).unwrap().filter_map(Result::ok).find(|entry| entry.path().is_dir()).unwrap().path();
    let backups = backups::list(&home);
    assert_eq!(backups.len(), 1);
    let backup = backups::resolve(&backups[0]).unwrap();
    assert_eq!(backup.resource, working_copy_uri(&resource, VIEW_TYPE).unwrap());
    assert_eq!((backup.meta.as_ref().unwrap()["backupId"].clone(), backup.content), (json!("backup-7"), Vec::new()));

    let (workbench, receiver, cx) = open(cx, &dir);
    register_provider(&workbench, cx);
    let create = next(&receiver, "$createCustomDocument");
    assert_eq!(create.args[..3], [resource.clone(), json!(VIEW_TYPE), json!("backup-7")]);
    answer(create, json!({ "editable": true }), cx);
    assert!(workbench.read_with(cx, |workbench, _| workbench.editors.editors.iter().any(|editor| editor.is_dirty())));

    // Reverting the restored document drops its backup.
    workbench.update_in(cx, |workbench, _, cx| {
        let index = workbench.editors.editors.iter().position(|editor| editor.is_dirty()).unwrap();
        workbench.editors.activate(index);
        workbench.editors_changed(cx);
    });
    run(&workbench, "workbench.action.files.revert", cx);
    assert!(backups::list(&home).is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn working_copy_uri_is_the_view_type_and_the_utf16_base64_of_the_resource() {
    let resource = json!({ "scheme": "file", "path": "/a b.bin" });
    let uri = working_copy_uri(&resource, "my.view type").unwrap();
    assert_eq!((uri.scheme.as_str(), uri.authority.as_str()), (CUSTOM_EDITOR_SCHEME, "my-view-type"));
    // btoa(toBinary('file:///a b.bin')): UTF-16LE bytes of each code unit.
    assert_eq!(uri.path, "/ZgBpAGwAZQA6AC8ALwAvAGEAIABiAC4AYgBpAG4A");
    assert_eq!(uri.query, r#"{"$mid":1,"path":"/a b.bin","scheme":"file"}"#);
}
