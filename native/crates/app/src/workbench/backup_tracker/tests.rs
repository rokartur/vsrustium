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
        workbench.start_backups(&user_data, None, cx);
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
    assert_eq!(backups::resolve(&backups[0]).unwrap().content, "xa");
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
