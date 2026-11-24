use std::path::{Path, PathBuf};

use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext, point, px};
use serde_json::json;

use crate::editor::{Position, Selection};
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::menus::MenuAction;
use crate::theme::load_theme;
use crate::workbench::Workbench;

fn workbench_with<'a>(name: &str, files: &[&str], cx: &'a mut TestAppContext) -> (PathBuf, gpui::Entity<Workbench>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<PathBuf> = files.iter().map(|file| dir.join(file)).collect();
    for path in &paths {
        std::fs::write(path, "hello world\nsecond line").unwrap();
    }
    let keybindings = dir.join("keybindings.json");
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.skip_dialogs = true;
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), keybindings, cx);
        for path in &paths {
            workbench.open_file(path.clone(), true, window, cx);
        }
        workbench
    });
    cx.run_until_parked();
    (dir, workbench, cx)
}

fn menu_titles(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    workbench.read_with(cx, |workbench, _| {
        let menu = workbench.context_menu.as_ref().expect("a context menu shows");
        menu.actions
            .iter()
            .map(|action| match action {
                MenuAction::Command { title, enabled: true, .. } => title.clone(),
                MenuAction::Command { title, .. } => format!("({title})"),
                _ => "---".into(),
            })
            .collect()
    })
}

fn open_names(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    workbench.read_with(cx, |workbench, _| workbench.editors.editors.iter().map(|editor| editor.document.name()).collect())
}

#[gpui::test]
fn the_editor_menu_moves_the_cursor_to_the_pointer_unless_a_selection_holds_it(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("editor-menu", &["a.txt"], cx);
    let pointer = point(px(400.), px(200.));
    workbench.update_in(cx, |workbench, window, cx| workbench.show_editor_context_menu(1, 3.2, pointer, window, cx));
    assert_eq!(menu_titles(&workbench, cx), ["Cut", "Copy", "Paste", "---", "Command Palette..."]);
    let selections = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().selections.clone());
    assert_eq!(selections(cx), [Selection::caret(Position::new(1, 3))]);
    cx.simulate_keystrokes("escape");

    let selected = Selection { anchor: Position::new(0, 0), head: Position::new(0, 5) };
    workbench.update(cx, |workbench, _| workbench.editors.active_mut().unwrap().selections = vec![selected]);
    workbench.update_in(cx, |workbench, window, cx| workbench.show_editor_context_menu(0, 5.0, pointer, window, cx));
    assert_eq!(selections(cx), [selected]);
    assert!(workbench.update_in(cx, |workbench, window, _| workbench.context_menu.is_some() && !workbench.editor_focus.is_focused(window)));
    cx.simulate_keystrokes("escape");
    assert!(workbench.update_in(cx, |workbench, window, _| workbench.context_menu.is_none() && workbench.editor_focus.is_focused(window)));
}

#[gpui::test]
fn the_tab_menu_closes_around_the_right_clicked_editor(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_with("tab-menu", &["a.txt", "b.txt", "c.txt", "d.txt"], cx);
    let pointer = point(px(200.), px(50.));
    workbench.update_in(cx, |workbench, window, cx| workbench.show_tab_context_menu(1, pointer, window, cx));
    assert_eq!(menu_titles(&workbench, cx), ["Close", "Close Others", "Close to the Right", "Close Saved", "Close All", "---", "(Keep Open)"]);
    cx.simulate_keystrokes("escape");
    workbench.update_in(cx, |workbench, window, cx| workbench.show_tab_context_menu(3, pointer, window, cx));
    assert_eq!(menu_titles(&workbench, cx)[2], "(Close to the Right)");
    cx.simulate_keystrokes("escape");

    let run = |command: &str, editor_index: usize, cx: &mut VisualTestContext| {
        let args = vec![json!({ "scheme": "file", "path": "" }), json!({ "groupId": 0, "editorIndex": editor_index })];
        workbench.update_in(cx, |workbench, window, cx| workbench.execute_command(command, args, window, cx));
        cx.run_until_parked();
    };
    run("workbench.action.closeEditorsToTheRight", 2, cx);
    assert_eq!(open_names(&workbench, cx), ["a.txt", "b.txt", "c.txt"]);
    run("workbench.action.closeActiveEditor", 0, cx);
    assert_eq!(open_names(&workbench, cx), ["b.txt", "c.txt"]);
    run("workbench.action.closeOtherEditors", 1, cx);
    assert_eq!(open_names(&workbench, cx), ["c.txt"]);

    // Close Saved leaves a dirty editor; Close All asks for it (Don't Save while dialogs are skipped).
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.open_file(dir.join("a.txt"), true, window, cx);
        window.focus(&workbench.editor_focus, cx);
    });
    cx.simulate_input("x");
    run("workbench.action.closeUnmodifiedEditors", 0, cx);
    assert_eq!(open_names(&workbench, cx), ["a.txt"]);
    run("workbench.action.closeEditorsInGroup", 0, cx);
    assert!(open_names(&workbench, cx).is_empty());
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "hello world\nsecond line");
}
