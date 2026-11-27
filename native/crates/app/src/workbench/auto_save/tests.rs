use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::json;

use super::*;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;

fn configuration(files: Value) -> AutoSaveConfiguration {
    auto_save_configuration(&files, || true)
}

/// filesConfigurationService.ts `computeAutoSaveConfiguration`.
#[test]
fn the_configuration_follows_the_files_settings() {
    assert_eq!(configuration(json!({})), AutoSaveConfiguration::default());
    assert_eq!(configuration(json!({ "autoSave": "off" })), AutoSaveConfiguration::default());
    assert_eq!(configuration(json!({ "autoSave": "afterDelay", "autoSaveDelay": 1000 })), AutoSaveConfiguration { auto_save: Some("afterDelay"), delay: Some(1000), short_delay: true, ..Default::default() });
    let long = configuration(json!({ "autoSave": "afterDelay", "autoSaveDelay": 1001 }));
    assert_eq!((long.delay, long.short_delay), (Some(1001), false));
    assert_eq!(configuration(json!({ "autoSave": "afterDelay", "autoSaveDelay": -1 })).delay, Some(1000), "a negative delay is the default");
    assert_eq!(configuration(json!({ "autoSave": "onFocusChange" })).auto_save, Some("onFocusChange"));
    assert_eq!(configuration(json!({ "autoSave": "onWindowChange" })).auto_save, Some("onWindowChange"));
    let no_errors = configuration(json!({ "autoSave": "afterDelay", "autoSaveWhenNoErrors": true }));
    assert!(no_errors.when_no_errors && !no_errors.short_delay);
    let outside = auto_save_configuration(&json!({ "autoSave": "afterDelay", "autoSaveWorkspaceFilesOnly": true }), || false);
    assert!(outside.out_of_workspace && !outside.short_delay);
}

/// filesConfigurationService.ts `getAutoSaveMode`.
#[test]
fn the_mode_checks_the_reason_the_workspace_and_errors() {
    let after_delay = configuration(json!({ "autoSave": "afterDelay" }));
    assert_eq!(auto_save_mode(&after_delay, None, || false), AutoSaveMode::AfterShortDelay);
    assert_eq!(auto_save_mode(&after_delay, Some(SaveReason::Auto), || false), AutoSaveMode::AfterShortDelay);
    assert_eq!(auto_save_mode(&after_delay, Some(SaveReason::FocusChange), || false), AutoSaveMode::Off(Disabled::Settings));
    let long = configuration(json!({ "autoSave": "afterDelay", "autoSaveDelay": 2000 }));
    assert_eq!(auto_save_mode(&long, None, || false), AutoSaveMode::AfterLongDelay);
    let focus = configuration(json!({ "autoSave": "onFocusChange" }));
    assert_eq!(auto_save_mode(&focus, Some(SaveReason::WindowChange), || false), AutoSaveMode::OnFocusChange);
    assert_eq!(auto_save_mode(&focus, Some(SaveReason::Auto), || false), AutoSaveMode::Off(Disabled::Settings));
    let window = configuration(json!({ "autoSave": "onWindowChange" }));
    assert_eq!(auto_save_mode(&window, Some(SaveReason::FocusChange), || false), AutoSaveMode::Off(Disabled::Settings));
    assert_eq!(auto_save_mode(&window, Some(SaveReason::WindowChange), || false), AutoSaveMode::OnWindowChange);
    assert_eq!(auto_save_mode(&AutoSaveConfiguration::default(), None, || false), AutoSaveMode::Off(Disabled::Settings));
    let outside = auto_save_configuration(&json!({ "autoSave": "afterDelay", "autoSaveWorkspaceFilesOnly": true }), || false);
    assert_eq!(auto_save_mode(&outside, None, || false), AutoSaveMode::Off(Disabled::OutOfWorkspace));
    let no_errors = configuration(json!({ "autoSave": "afterDelay", "autoSaveWhenNoErrors": true }));
    assert_eq!(auto_save_mode(&no_errors, None, || true), AutoSaveMode::Off(Disabled::Errors));
    assert_eq!(auto_save_mode(&no_errors, None, || false), AutoSaveMode::AfterLongDelay, "errors can come in later");
}

/// A folder with `a.txt` and `b.txt` and `.vscode/settings.json` holding `settings`, `a.txt` open.
fn open<'a>(cx: &'a mut TestAppContext, settings_json: &str) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext, PathBuf) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    static OPENED: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("code-gpui-auto-save-{}-{}", std::process::id(), OPENED.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();
    std::fs::write(dir.join(".vscode/settings.json"), settings_json).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    std::fs::write(dir.join("b.txt"), "b").unwrap();
    let workspace = config::workspace::Workspace::folder(dir.clone());
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(workspace.clone()))));
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.watch_editor_focus(window, cx);
        workbench.open_file(dir.join("a.txt"), true, window, cx);
        workbench
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (workbench, cx, dir)
}

fn type_text(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, text: &str) {
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("type", vec![json!({ "text": text })], window, cx));
    cx.run_until_parked();
}

fn a_is_dirty(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, dir: &Path) -> bool {
    workbench.read_with(cx, |workbench, _| workbench.editors.model("file", &dir.join("a.txt")).unwrap().is_dirty())
}

/// editorAutoSave.test.ts 'editor auto saves after short delay if configured'.
#[gpui::test]
fn an_edit_saves_after_the_delay(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = open(cx, r#"{ "files.autoSave": "afterDelay", "files.autoSaveDelay": 50 }"#);
    type_text(&workbench, cx, "x");
    assert!(a_is_dirty(&workbench, cx, &dir));
    cx.executor().advance_clock(Duration::from_millis(30));
    type_text(&workbench, cx, "y");
    cx.executor().advance_clock(Duration::from_millis(30));
    cx.run_until_parked();
    assert!(a_is_dirty(&workbench, cx, &dir), "the second edit restarts the delay");
    cx.executor().advance_clock(Duration::from_millis(30));
    cx.run_until_parked();
    assert!(!a_is_dirty(&workbench, cx, &dir));
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "xya");
    std::fs::remove_dir_all(dir).unwrap();
}

/// editorAutoSave.test.ts 'editor auto saves on focus change if configured'.
#[gpui::test]
fn opening_another_editor_saves_on_focus_change(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = open(cx, r#"{ "files.autoSave": "onFocusChange" }"#);
    type_text(&workbench, cx, "x");
    assert!(a_is_dirty(&workbench, cx, &dir));
    workbench.update_in(cx, |workbench, window, cx| workbench.open_file(dir.join("b.txt"), true, window, cx));
    cx.run_until_parked();
    assert!(!a_is_dirty(&workbench, cx, &dir));
    std::fs::remove_dir_all(dir).unwrap();
}

/// editorAutoSave.ts `onWindowFocusChange(false)` with `onWindowChange`.
#[gpui::test]
fn the_window_losing_the_focus_saves_on_window_change(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = open(cx, r#"{ "files.autoSave": "onWindowChange" }"#);
    type_text(&workbench, cx, "x");
    workbench.update_in(cx, |workbench, window, cx| workbench.open_file(dir.join("b.txt"), true, window, cx));
    cx.run_until_parked();
    assert!(a_is_dirty(&workbench, cx, &dir), "a focus change does not save on window change");
    cx.deactivate_window();
    cx.run_until_parked();
    assert!(!a_is_dirty(&workbench, cx, &dir));
    std::fs::remove_dir_all(dir).unwrap();
}

/// editorAutoSave.ts `onDidChangeAutoSaveConfiguration`: turning auto save on saves the dirty models.
#[gpui::test]
fn toggling_auto_save_on_saves_the_dirty_models(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = open(cx, "{}");
    type_text(&workbench, cx, "x");
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("workbench.action.toggleAutoSave", vec![], window, cx));
    cx.run_until_parked();
    assert!(!a_is_dirty(&workbench, cx, &dir));
    std::fs::remove_dir_all(dir).unwrap();
}
