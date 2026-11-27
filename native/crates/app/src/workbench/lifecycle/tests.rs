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
fn confirm_before_close_asks_always_or_for_keyboard_only() {
    assert!(asks_before_close(&json!("always"), false));
    assert!(asks_before_close(&json!("keyboardOnly"), true));
    assert!(!asks_before_close(&json!("keyboardOnly"), false));
    assert!(!asks_before_close(&json!("never"), true));
}

/// A folder with `a.txt` (holding "a") open and `settings_json` as its folder and user settings (`files.hotExit`
/// is an application setting).
fn open<'a>(cx: &'a mut TestAppContext, settings_json: &str) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext, PathBuf) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    static OPENED: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("code-gpui-lifecycle-{}-{}", std::process::id(), OPENED.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();
    std::fs::write(dir.join(".vscode/settings.json"), settings_json).unwrap();
    std::fs::create_dir_all(dir.join("User")).unwrap();
    std::fs::write(dir.join("User/settings.json"), settings_json).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    let workspace = config::workspace::Workspace::folder(dir.clone());
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(workspace.clone()))));
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.open_file(dir.join("a.txt"), true, window, cx);
        workbench
    });
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("type", vec![json!({ "text": "x" })], window, cx));
    cx.run_until_parked();
    (workbench, cx, dir)
}

/// Runs a quit shutdown; true when nothing vetoed.
fn quit(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> bool {
    let shut_down = Rc::new(Cell::new(false));
    let done = shut_down.clone();
    workbench.update_in(cx, |workbench, window, cx| workbench.shutdown(ShutdownReason::Quit, window, cx, move |_, _, _| done.set(true)));
    cx.run_until_parked();
    shut_down.get()
}

fn a_is_dirty(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, dir: &Path) -> bool {
    workbench.read_with(cx, |workbench, _| workbench.editors.model("file", &dir.join("a.txt")).unwrap().is_dirty())
}

/// workingCopyBackupTracker.test.ts 'onWillShutdown - auto save is ON'.
#[gpui::test]
fn auto_save_saves_before_the_shutdown(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = open(cx, r#"{ "files.autoSave": "onFocusChange", "files.hotExit": "off" }"#);
    assert!(a_is_dirty(&workbench, cx, &dir));
    assert!(quit(&workbench, cx));
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "xa");
    assert!(!a_is_dirty(&workbench, cx, &dir));
}

/// workingCopyBackupTracker.test.ts 'onWillShutdown - no veto if user does not want to save': without dialogs
/// (extension tests) `showSaveConfirm` answers Don't Save, which reverts.
#[gpui::test]
fn dont_save_reverts_and_shuts_down(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = open(cx, r#"{ "files.hotExit": "off" }"#);
    workbench.update(cx, |workbench, _| workbench.skip_dialogs = true);
    assert!(quit(&workbench, cx));
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "a");
    assert!(!a_is_dirty(&workbench, cx, &dir));
}
