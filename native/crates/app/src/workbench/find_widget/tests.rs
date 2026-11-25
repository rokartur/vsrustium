use std::path::Path;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::TestAppContext;

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

#[gpui::test]
fn find_on_type_and_cursor_move_on_type_off_wait_for_enter(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-find-on-type-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    std::fs::write(dir.join("User/settings.json"), r#"{ "editor.find.findOnType": false, "editor.find.cursorMoveOnType": false }"#).unwrap();
    let file = dir.join("a.txt");
    std::fs::write(&file, "the cat\nand the dog").unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings.clone(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
        workbench.open_file(file.clone(), true, window, cx);
        workbench
    });
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.editors.active_mut().unwrap().set_selections(vec![crate::editor::Selection::caret(crate::editor::Position::new(1, 0))]);
        workbench.execute_command("actions.find", vec![], window, cx)
    });
    cx.run_until_parked();
    let found = |cx: &mut gpui::VisualTestContext| {
        workbench.read_with(cx, |workbench, _| {
            let state = &workbench.find.controller.state;
            let head = workbench.editors.active().unwrap().selection().head;
            (state.search_string.clone(), state.matches_count, (head.line, head.column))
        })
    };

    assert_eq!(found(cx), ("and".into(), 1, (1, 0)), "seeded from the word at the cursor");
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("dog");
    assert_eq!(found(cx), ("and".into(), 1, (1, 0)), "typing does not search");
    cx.simulate_keystrokes("enter");
    assert_eq!(found(cx), ("dog".into(), 1, (1, 11)), "Enter searches, then Find Next moves to the match");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[gpui::test]
fn cursor_move_on_type_off_keeps_the_cursor_while_typing(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-cursor-move-on-type-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    std::fs::write(dir.join("User/settings.json"), r#"{ "editor.find.cursorMoveOnType": false }"#).unwrap();
    let file = dir.join("a.txt");
    std::fs::write(&file, "the cat\nand the dog").unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings.clone(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
        workbench.open_file(file.clone(), true, window, cx);
        workbench
    });
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("actions.find", vec![], window, cx));
    cx.run_until_parked();
    cx.simulate_input("dog");
    let (count, head) = workbench.read_with(cx, |workbench, _| (workbench.find.controller.state.matches_count, workbench.editors.active().unwrap().selection().head));
    assert_eq!((count, head.line, head.column), (1, 0, 0));
    std::fs::remove_dir_all(&dir).unwrap();
}
