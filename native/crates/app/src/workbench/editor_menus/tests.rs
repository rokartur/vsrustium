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
        menu.levels[0].actions
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
    assert_eq!(menu_titles(&workbench, cx), ["Change All Occurrences", "---", "Cut", "Copy", "Paste", "---", "Command Palette..."]);
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
    assert_eq!(menu_titles(&workbench, cx), ["Close", "Close Others", "Close to the Right", "Close Saved", "Close All", "---", "Copy Path", "Copy Relative Path", "---", "Reveal in Finder", "---", "(Keep Open)"]);
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

#[gpui::test]
fn the_host_moves_and_closes_tabs_by_their_ids(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("editor-tabs", &["a.txt", "b.txt", "c.txt"], cx);
    let request = |method: &str, args: Vec<serde_json::Value>, cx: &mut VisualTestContext| {
        let request = exthost::session::ForwardedRequest { req: 1, proxy: "MainThreadEditorTabs", method: method.into(), args };
        let outcome = workbench.update_in(cx, |workbench, window, cx| workbench.handle_host_request(request, window, cx));
        let crate::commands::Outcome::Ready(result) = outcome else { panic!("{method} waits") };
        result
    };
    let id = |name: &str, cx: &mut VisualTestContext| {
        workbench.read_with(cx, |workbench, _| crate::host_sync::tab_id(workbench.editors.editors.iter().find(|editor| editor.document.name() == name).unwrap()))
    };
    let a = id("a.txt", cx);
    // An index past the tabs moves the tab last, and the move pins it.
    assert_eq!(request("$moveTab", vec![json!(a), json!(7), json!(0)], cx), Ok(json!(null)));
    assert_eq!(open_names(&workbench, cx), ["b.txt", "c.txt", "a.txt"]);
    assert!(workbench.read_with(cx, |workbench, _| !workbench.editors.editors[2].preview));
    assert!(request("$moveTab", vec![json!("missing"), json!(0), json!(0)], cx).is_err());

    let b = id("b.txt", cx);
    assert_eq!(request("$closeTab", vec![json!([b, a])], cx), Ok(json!(true)));
    assert_eq!(open_names(&workbench, cx), ["c.txt"]);
    assert_eq!(request("$closeGroup", vec![json!([0])], cx), Ok(json!(true)));
    assert!(open_names(&workbench, cx).is_empty());
}

#[gpui::test]
fn shift_f10_shows_the_editor_menu_below_the_cursor_without_moving_it(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_with("show-context-menu", &["a.txt"], cx);
    let cursor = Selection::caret(Position::new(1, 3));
    workbench.update(cx, |workbench, _| workbench.editors.active_mut().unwrap().set_selections(vec![cursor]));
    workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("editor.action.showContextMenu", vec![], window, cx));
    assert_eq!(menu_titles(&workbench, cx), ["Change All Occurrences", "---", "Cut", "Copy", "Paste", "---", "Command Palette..."]);
    let (origin, anchor, selections) = workbench.update_in(cx, |workbench, window, _| (workbench.context_menu.as_ref().unwrap().levels[0].origin, workbench.cursor_anchor(window).unwrap(), workbench.editors.active().unwrap().selections.clone()));
    assert_eq!(selections, [cursor]);
    assert!(origin.x >= anchor.x - px(1.) && origin.y >= anchor.y - px(1.), "{origin:?} at {anchor:?}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn the_line_number_menu_selects_the_line_and_shows_contributed_items(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_with("line-number-menu", &["a.txt"], cx);
    let extension = exthost::scan::ScannedExtension {
        id: "test.lines".into(),
        location: dir.clone(),
        description: json!({ "contributes": {
            "commands": [{ "command": "lines.copy", "title": "Copy Line Link" }, { "command": "lines.other", "title": "Other Line" }],
            "menus": { "editor/lineNumber/context": [{ "command": "lines.copy", "when": "editorLineNumber == 2" }, { "command": "lines.other", "when": "editorLineNumber == 1" }] }
        } }),
    };
    workbench.update(cx, |workbench, _| workbench.menus.extend(crate::menus::extension_menus(&[extension])));
    let pointer = point(px(30.), px(100.));
    workbench.update_in(cx, |workbench, window, cx| workbench.show_line_number_context_menu(1, pointer, window, cx));
    assert_eq!(menu_titles(&workbench, cx), ["Copy Line Link"]);
    let selections = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().selections.clone());
    assert_eq!(selections, [Selection { anchor: Position::new(1, 0), head: Position::new(1, 11) }]);
    cx.simulate_keystrokes("escape");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn gutter_presses_select_whole_lines_and_drags_extend_them(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_with("gutter-select", &["a.txt"], cx);
    workbench.update(cx, |workbench, _| {
        let edit = (Position::new(0, 0), Position::new(1, 11), "one\ntwo\nthree\nfour".to_string(), false);
        workbench.editors.active_mut().unwrap().execute_edits(vec![edit], None, crate::editor::EditReason::Unknown("test"));
    });
    let selections = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().selections.clone());
    let none = gpui::Modifiers::none();
    workbench.update(cx, |workbench, cx| workbench.gutter_line_select(1, none, cx));
    assert_eq!(selections(cx), [Selection { anchor: Position::new(1, 0), head: Position::new(2, 0) }]);
    workbench.update(cx, |workbench, cx| workbench.gutter_line_select_drag(3, cx));
    assert_eq!(selections(cx), [Selection { anchor: Position::new(1, 0), head: Position::new(3, 4) }], "the last line ends at its end");
    workbench.update(cx, |workbench, cx| workbench.gutter_line_select_drag(0, cx));
    assert_eq!(selections(cx), [Selection { anchor: Position::new(2, 0), head: Position::new(0, 0) }], "above, from the entering line's end");
    workbench.update(cx, |workbench, _| workbench.line_select_start = None);
    let alt = gpui::Modifiers { alt: true, ..none };
    workbench.update(cx, |workbench, cx| workbench.gutter_line_select(3, alt, cx));
    assert_eq!(selections(cx).len(), 2, "Alt adds a cursor selecting the line");
    let shift = gpui::Modifiers { shift: true, ..none };
    workbench.update(cx, |workbench, cx| {
        workbench.editors.active_mut().unwrap().set_selections(vec![Selection::caret(Position::new(0, 2))]);
        workbench.gutter_line_select(2, shift, cx);
    });
    assert_eq!(selections(cx), [Selection { anchor: Position::new(0, 2), head: Position::new(3, 0) }], "Shift extends from the cursor");
    std::fs::remove_dir_all(dir).unwrap();
}
