use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::json;

use super::*;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;

#[test]
fn etag_is_mtime_base_29_and_size_base_31() {
    assert_eq!(etag(1_700_000_000_000, 1234), format!("{}{}", radix(1_700_000_000_000, 29), "18p"));
    assert_eq!(radix(0, 31), "0");
    assert_eq!(radix(30, 31), "u");
}

fn scratch() -> PathBuf {
    static MADE: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("vsrustium-backup-tracker-{}-{}", std::process::id(), MADE.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(dir.join("folder/.vscode")).unwrap();
    std::fs::write(dir.join("folder/a.txt"), "a").unwrap();
    dir
}

/// A window on `<dir>/folder` with `a.txt` open, `settings_json` as user settings and its backups in `<dir>/user`.
fn open<'a>(cx: &'a mut TestAppContext, dir: &Path, settings_json: &str) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext) {
    open_with_editor_part(cx, dir, settings_json, None)
}

/// `open`, with the editor part restored from `editor_part` first, as main.rs restores workspace state before backups.
fn open_with_editor_part<'a>(cx: &'a mut TestAppContext, dir: &Path, settings_json: &str, editor_part: Option<String>) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let folder = dir.join("folder");
    std::fs::create_dir_all(dir.join("user/User")).unwrap();
    std::fs::write(dir.join("user/User/settings.json"), settings_json).unwrap();
    let workspace = config::workspace::Workspace::folder(folder.clone());
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("user/User"), Some(workspace.clone()))));
    let user_data = dir.join("user");
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        if let Some(editor_part) = editor_part {
            let storage = Arc::new(Mutex::new(storage::Storage::in_memory()));
            storage.lock().unwrap().set("memento/workbench.parts.editor", editor_part);
            workbench.restore_workspace_state(storage);
        }
        workbench.start_backups(&user_data, None, window, cx);
        workbench.open_file(folder.join("a.txt"), true, window, cx);
        workbench
    });
    cx.run_until_parked();
    (workbench, cx)
}

fn type_x(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) {
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("type", vec![json!({ "text": "x" })], window, cx));
    cx.run_until_parked();
}

fn backups_of(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<PathBuf> {
    workbench.read_with(cx, |workbench, _| backups::list(workbench.lifecycle.backups.home.as_ref().unwrap()))
}

/// workingCopyBackupTracker.test.ts 'Track backups (file)': a change backs up after the delay, a save discards it.
#[gpui::test]
fn a_dirty_file_is_backed_up_after_a_second_and_saving_discards_it(cx: &mut TestAppContext) {
    let dir = scratch();
    let (workbench, cx) = open(cx, &dir, "{}");
    type_x(&workbench, cx);
    assert!(backups_of(&workbench, cx).is_empty());
    cx.executor().advance_clock(BACKUP_DELAY);
    cx.run_until_parked();
    let backups = backups_of(&workbench, cx);
    assert_eq!(backups.len(), 1);
    assert_eq!(backups::resolve(&backups[0]).unwrap().content, b"xa");
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("workbench.action.files.save", vec![], window, cx));
    cx.run_until_parked();
    assert!(backups_of(&workbench, cx).is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// workingCopyBackupTracker.test.ts 'onWillShutdown - no veto and backups saved' then 'Restore backups': a quit
/// with hot exit backs up without asking, and the next window on the folder opens the editor dirty again.
#[gpui::test]
fn quit_backs_up_and_the_next_window_restores(cx: &mut TestAppContext) {
    let dir = scratch();
    let (workbench, vcx) = open(cx, &dir, "{}");
    type_x(&workbench, vcx);
    let shut_down = Rc::new(Cell::new(false));
    let done = shut_down.clone();
    workbench.update_in(vcx, |workbench, window, cx| workbench.shutdown(ShutdownReason::Quit, window, cx, move |_, _, _| done.set(true)));
    vcx.run_until_parked();
    assert!(shut_down.get(), "no veto");
    assert_eq!(backups_of(&workbench, vcx).len(), 1);
    assert_eq!(std::fs::read_to_string(dir.join("folder/a.txt")).unwrap(), "a", "nothing saved");

    let (restored, vcx) = open(cx, &dir, "{}");
    let path = dir.join("folder/a.txt");
    let (dirty, text) = restored.read_with(vcx, |workbench, _| {
        let model = workbench.editors.model("file", &path).unwrap();
        (model.is_dirty(), model.document().lines.join("\n"))
    });
    assert!(dirty);
    assert_eq!(text, "xa");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// untitledTextEditorHandler.ts: with hot exit the editor part keeps an untitled editor in its place, and the
/// restored window shows it there with its backed up text instead of opening it again at the end.
#[gpui::test]
fn an_untitled_editor_comes_back_in_its_tab(cx: &mut TestAppContext) {
    let dir = scratch();
    let (workbench, vcx) = open(cx, &dir, "{}");
    workbench.update_in(vcx, |workbench, window, cx| _ = workbench.execute_command("workbench.action.closeAllEditors", vec![], window, cx));
    workbench.update_in(vcx, |workbench, window, cx| _ = workbench.execute_command("workbench.action.files.newUntitledFile", vec![], window, cx));
    vcx.run_until_parked();
    type_x(&workbench, vcx);
    workbench.update_in(vcx, |workbench, window, cx| workbench.open_file(dir.join("folder/a.txt"), false, window, cx));
    vcx.run_until_parked();
    workbench.update_in(vcx, |workbench, window, cx| workbench.shutdown(ShutdownReason::Quit, window, cx, |_, _, _| {}));
    vcx.run_until_parked();
    let editor_part = workbench.read_with(vcx, |workbench, _| workbench.workspace_storage.lock().unwrap().get("memento/workbench.parts.editor").map(str::to_owned).unwrap());

    let (restored, vcx) = open_with_editor_part(cx, &dir, "{}", Some(editor_part));
    let tabs = restored.read_with(vcx, |workbench, _| {
        let group = workbench.editor_group_id();
        workbench.editors.group_tabs(group).0.iter().map(|editor| (editor.document().name(), editor.is_dirty(), editor.document().lines.join("\n"))).collect::<Vec<_>>()
    });
    assert_eq!(tabs, vec![("Untitled-1".to_string(), true, "x".to_string()), ("a.txt".to_string(), false, "a".to_string())]);
    assert_eq!(restored.read_with(vcx, |workbench, _| workbench.dirty_working_copy_count()), 1, "the explorer's badge");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `files.hotExit: off` asks instead; without dialogs that is Don't Save, which reverts and leaves no backup.
#[gpui::test]
fn hot_exit_off_asks(cx: &mut TestAppContext) {
    let dir = scratch();
    let (workbench, cx) = open(cx, &dir, r#"{ "files.hotExit": "off" }"#);
    workbench.update(cx, |workbench, _| workbench.skip_dialogs = true);
    type_x(&workbench, cx);
    workbench.update_in(cx, |workbench, window, cx| workbench.shutdown(ShutdownReason::Quit, window, cx, |_, _, _| {}));
    cx.run_until_parked();
    assert!(backups_of(&workbench, cx).is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

const NOTEBOOK: &str = "test-notebook";

/// A window on `<dir>/folder` with a `test-notebook` type for `*.tnb` and its serializer, `a.txt` open; its host
/// messages arrive on the receiver.
fn open_with_notebooks<'a>(cx: &'a mut TestAppContext, dir: &Path) -> (gpui::Entity<Workbench>, std::sync::mpsc::Receiver<crate::host_sync::HostMessage>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let folder = dir.join("folder");
    std::fs::create_dir_all(dir.join("user/User")).unwrap();
    let workspace = config::workspace::Workspace::folder(folder.clone());
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("user/User"), Some(workspace.clone()))));
    let user_data = dir.join("user");
    let (sender, receiver) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(sender), cx);
        let description = json!({ "contributes": { "notebooks": [{ "type": NOTEBOOK, "displayName": "Test", "selector": [{ "filenamePattern": "*.tnb" }] }] } });
        workbench.extensions = vec![exthost::scan::ScannedExtension { id: "port.test".into(), location: "/ext".into(), description }];
        workbench.host.notebooks.serializers.push(crate::notebook::Serializer { handle: 1, view_type: NOTEBOOK.into(), options: json!({}), registration: Value::Null });
        workbench.start_backups(&user_data, None, window, cx);
        workbench.open_file(folder.join("a.txt"), true, window, cx);
        workbench
    });
    cx.run_until_parked();
    (workbench, receiver, cx)
}

/// The next host message to `method`, the ones before it dropped.
fn next_message(receiver: &std::sync::mpsc::Receiver<crate::host_sync::HostMessage>, method: &str) -> crate::host_sync::HostMessage {
    loop {
        let message = receiver.try_recv().unwrap_or_else(|_| panic!("no {method}"));
        if message.method == method {
            return message;
        }
    }
}

fn answer(message: crate::host_sync::HostMessage, value: Value, cx: &mut VisualTestContext) {
    let _ = message.reply.expect("a call").send(Ok(value));
    cx.run_until_parked();
}

fn notebook_data(source: &str) -> Value {
    json!({ "metadata": {}, "cells": [{ "cellKind": 2, "language": "python", "source": source, "outputs": [], "metadata": {} }] })
}

/// notebookEditorModel.ts with workingCopyBackupTracker.ts: a dirty notebook backs up the serializer's bytes as a
/// `notebook/<type>/<type>` working copy, a quit keeps it, the next window opens the notebook in the background
/// from it (`$dataToNotebook` of the backed up bytes) and dirty, and a revert drops the backup.
#[gpui::test]
fn a_dirty_notebook_backs_up_through_its_serializer_and_opens_from_its_backup(cx: &mut TestAppContext) {
    let dir = scratch();
    std::fs::write(dir.join("folder/a.tnb"), "saved").unwrap();
    let uri = crate::uri::Uri::new("file", "", &dir.join("folder/a.tnb").to_string_lossy(), "", "").unwrap();
    let key = crate::notebook::uri_string(&uri.to_json());
    let home;
    {
        let (workbench, receiver, cx) = open_with_notebooks(cx, &dir);
        home = workbench.read_with(cx, |workbench, _| workbench.lifecycle.backups.home.clone().unwrap());
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.open_notebook(uri.to_json(), NOTEBOOK.into(), json!({}), window, cx)));
        cx.run_until_parked();
        answer(next_message(&receiver, "resolveTextContent"), json!("saved"), cx);
        answer(next_message(&receiver, "$dataToNotebook"), notebook_data("x"), cx);
        workbench.update_in(cx, |workbench, window, cx| {
            window.focus(&workbench.editor_focus, cx);
            _ = workbench.execute_command("type", vec![json!({ "text": "1" })], window, cx);
        });
        cx.run_until_parked();
        assert!(workbench.read_with(cx, |workbench, _| workbench.host.notebooks.model(&key).unwrap().dirty));
        cx.executor().advance_clock(BACKUP_DELAY);
        cx.run_until_parked();
        let to_data = next_message(&receiver, "$notebookToData");
        assert_eq!(to_data.args[1]["$withBuffers"]["cells"][0]["source"], "1x");
        answer(to_data, json!({ "$vsbuffer": "QkFDS1VQ" }), cx);
        let backups = backups::list(&home);
        assert_eq!(backups.len(), 1);
        let backup = backups::resolve(&backups[0]).unwrap();
        assert_eq!((backup.resource, backup.type_id.as_str(), backup.content.as_slice()), (uri.clone(), "notebook/test-notebook/test-notebook", b"BACKUP".as_slice()));
        assert_eq!(backup.meta.unwrap()["size"], 5, "the file's stat");

        let shut_down = Rc::new(Cell::new(false));
        let done = shut_down.clone();
        workbench.update_in(cx, |workbench, window, cx| workbench.shutdown(ShutdownReason::Quit, window, cx, move |_, _, _| done.set(true)));
        cx.run_until_parked();
        let pending: Vec<&str> = receiver.try_iter().map(|message| message.method).collect();
        assert!(shut_down.get(), "no veto {pending:?}");
        assert_eq!(backups::list(&home).len(), 1, "a dirty notebook alone keeps the backups");
    }

    let (workbench, receiver, cx) = open_with_notebooks(cx, &dir);
    let from_backup = next_message(&receiver, "$dataToNotebook");
    assert_eq!(from_backup.args[1], json!({ "$vsbuffer": "QkFDS1VQ" }));
    answer(from_backup, notebook_data("1x"), cx);
    let (dirty, active) = workbench.read_with(cx, |workbench, _| (workbench.host.notebooks.model(&key).unwrap().dirty, workbench.editors.active_tab().unwrap().document().name()));
    assert!(dirty);
    assert_eq!(active, "a.txt", "the notebook opens in the background");

    workbench.update(cx, |workbench, cx| workbench.revert_notebook(&key, cx));
    cx.run_until_parked();
    answer(next_message(&receiver, "resolveTextContent"), json!("saved"), cx);
    answer(next_message(&receiver, "$dataToNotebook"), notebook_data("x"), cx);
    assert!(backups::list(&home).is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}
