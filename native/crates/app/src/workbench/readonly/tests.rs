use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext, size};

use super::Readonly;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;
use crate::workbench::editor_message::{MessageLayout, layout_in_page};

/// A folder holding `name` with `text` and `.vscode/settings.json` with `settings`, opened in a workbench.
fn open<'a>(cx: &'a mut TestAppContext, name: &str, text: &str, settings_json: &str) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext, PathBuf) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    static OPENED: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("code-gpui-readonly-{}-{}", std::process::id(), OPENED.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();
    std::fs::write(dir.join(".vscode/settings.json"), settings_json).unwrap();
    let file = dir.join(name);
    std::fs::write(&file, text).unwrap();
    let workspace = config::workspace::Workspace::folder(dir.clone());
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(workspace.clone()))));
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.watch_editor_focus(window, cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("User/keybindings.json"), cx);
        workbench.open_file(file.clone(), true, window, cx);
        workbench
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (workbench, cx, dir)
}

fn readonly_of(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, path: &Path) -> Option<Readonly> {
    workbench.read_with(cx, |workbench, _| workbench.readonly("file", path))
}

fn run(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, command: &str) {
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command(command, vec![], window, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn the_session_commands_set_toggle_and_reset_the_override(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = open(cx, "a.txt", "a", "{}");
    let file = dir.join("a.txt");
    assert_eq!(readonly_of(&workbench, cx, &file), None);
    run(&workbench, cx, "workbench.action.files.setActiveEditorReadonlyInSession");
    assert_eq!(readonly_of(&workbench, cx, &file), Some(Readonly::Session));
    run(&workbench, cx, "workbench.action.files.toggleActiveEditorReadonlyInSession");
    assert_eq!(readonly_of(&workbench, cx, &file), None, "toggle sets the opposite of isReadonly as the override");
    run(&workbench, cx, "workbench.action.files.toggleActiveEditorReadonlyInSession");
    assert_eq!(readonly_of(&workbench, cx, &file), Some(Readonly::Session));
    run(&workbench, cx, "workbench.action.files.setActiveEditorWriteableInSession");
    assert_eq!(readonly_of(&workbench, cx, &file), None);
    run(&workbench, cx, "workbench.action.files.resetActiveEditorReadonlyInSession");
    assert_eq!(readonly_of(&workbench, cx, &file), None);
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn include_minus_exclude_then_permissions_decide_and_the_session_wins(cx: &mut TestAppContext) {
    let settings = r#"{"files.readonlyInclude": {"**/*.txt": true}, "files.readonlyExclude": {"keep.txt": true}, "files.readonlyFromPermissions": true}"#;
    let (workbench, cx, dir) = open(cx, "ro.txt", "a", settings);
    std::fs::write(dir.join("keep.txt"), "").unwrap();
    let locked = dir.join("locked.md");
    std::fs::write(&locked, "").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o444)).unwrap();
    assert_eq!(readonly_of(&workbench, cx, &dir.join("ro.txt")), Some(Readonly::Configured));
    assert_eq!(readonly_of(&workbench, cx, &dir.join("keep.txt")), None, "readonlyExclude wins over readonlyInclude");
    assert_eq!(readonly_of(&workbench, cx, &dir.join(".vscode/settings.json")), None);
    assert_eq!(readonly_of(&workbench, cx, &locked), Some(Readonly::Locked));
    assert_eq!(readonly_of(&workbench, cx, &dir.join("User/settings.json")), None, "the user data is never read-only");
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.readonly("untitled", Path::new("Untitled-1.txt"))), None);
    run(&workbench, cx, "workbench.action.files.setActiveEditorWriteableInSession");
    assert_eq!(readonly_of(&workbench, cx, &dir.join("ro.txt")), None, "the session override wins");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn an_edit_in_a_read_only_editor_shows_why_and_escape_closes_it(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = open(cx, "ro.txt", "abc\ndef", r#"{"files.readonlyInclude": {"ro.txt": true}}"#);
    let text = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().lines.join("\n"));
    let visible = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editor_message_visible());
    workbench.update_in(cx, |workbench, window, cx| window.focus(&workbench.editor_focus, cx));
    cx.simulate_keystrokes("x");
    cx.run_until_parked();
    assert_eq!(text(cx), "abc\ndef");
    assert!(visible(cx), "onDidAttemptReadOnlyEdit");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!visible(cx), "leaveEditorMessage");

    cx.simulate_keystrokes("cmd-backspace");
    cx.run_until_parked();
    assert!(!visible(cx), "deleteAllLeft has precondition writable: nothing runs, nothing shows");
    cx.simulate_keystrokes("delete");
    cx.run_until_parked();
    assert_eq!(text(cx), "abc\ndef");
    assert!(visible(cx), "deleteRight is a cursor edit");
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    assert!(!visible(cx), "onDidChangeCursorPosition");

    run(&workbench, cx, "workbench.action.files.toggleActiveEditorReadonlyInSession");
    cx.simulate_keystrokes("x");
    cx.run_until_parked();
    assert_eq!(text(cx), "abc\nxdef");
    assert!(!visible(cx));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_message_goes_above_the_cursor_unless_the_window_top_is_too_close() {
    let editor = gpui::Bounds { origin: gpui::point(300., 80.), size: size(800., 600.) };
    let window = size(1200., 800.);
    // 22px clearance: the 32px tall widget above line 1 at window y 80 starts at 48.
    assert_eq!(layout_in_page(70., 0., 18., 200., editor, window), MessageLayout { left: 70., top: -32., below: false });
    let high = gpui::Bounds { origin: gpui::point(300., 40.), ..editor };
    assert_eq!(layout_in_page(70., 0., 18., 200., high, window), MessageLayout { left: 70., top: 18., below: true });
    // 15px from the window's right edge: 300 + 1000 + 200 passes 1185.
    assert_eq!(layout_in_page(1000., 100., 18., 200., editor, window), MessageLayout { left: 685., top: 68., below: false });
}
