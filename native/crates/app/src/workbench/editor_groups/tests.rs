use std::path::{Path, PathBuf};

use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::json;

use crate::editor::Position;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

fn workbench_with<'a>(name: &str, files: &[&str], cx: &'a mut TestAppContext) -> (PathBuf, gpui::Entity<Workbench>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-groups-{name}-{}", std::process::id()));
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

fn run(workbench: &gpui::Entity<Workbench>, command: &str, cx: &mut VisualTestContext) {
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(command, vec![], window, cx)));
    cx.run_until_parked();
}

/// The groups in grid order with their tab names, and the active group's index.
fn groups(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> (Vec<Vec<String>>, usize) {
    workbench.read_with(cx, |workbench, _| {
        let order = workbench.groups_in_grid_order();
        let names = order.iter().map(|group| workbench.editors.group_tabs(*group).0.iter().map(|editor| editor.document().name()).collect()).collect();
        (names, order.iter().position(|group| *group == workbench.editor_group_id()).unwrap())
    })
}

#[gpui::test]
fn split_editor_shows_the_same_model_in_a_new_group_to_the_right(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("split", &["a.txt"], cx);
    workbench.update(cx, |workbench, _| workbench.editors.active_mut().unwrap().move_to(Position::new(1, 3), false));
    cx.simulate_keystrokes("cmd-\\");
    cx.run_until_parked();
    assert_eq!(groups(&workbench, cx), (vec![vec!["a.txt".to_string()], vec!["a.txt".to_string()]], 1));
    workbench.update(cx, |workbench, _| {
        // splitEditor copies the view state: the cursor is where it was.
        assert_eq!(workbench.editors.active().unwrap().selection().head, Position::new(1, 3));
        let left = workbench.groups_in_grid_order()[0];
        let shared = &workbench.editors.group_tabs(left).0[0].model;
        assert!(std::rc::Rc::ptr_eq(shared, &workbench.editors.active().unwrap().model));
    });
    let context = workbench.update_in(cx, |workbench, window, _| workbench.key_context(window));
    assert_eq!((context["multipleEditorGroups"].clone(), context["activeEditorGroupIndex"].clone(), context["activeEditorGroupLast"].clone()), (json!(true), json!(2), json!(true)));
}

#[gpui::test]
fn an_edit_in_one_group_moves_the_cursor_of_the_other(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("sync", &["a.txt"], cx);
    workbench.update(cx, |workbench, _| workbench.editors.active_mut().unwrap().move_to(Position::new(0, 6), false));
    run(&workbench, "workbench.action.splitEditor", cx);
    workbench.update(cx, |workbench, _| {
        let editor = workbench.editors.active_mut().unwrap();
        editor.move_to(Position::new(0, 0), false);
        editor.type_text(">> ");
    });
    run(&workbench, "workbench.action.focusFirstEditorGroup", cx);
    workbench.update(cx, |workbench, _| {
        let editor = workbench.editors.active().unwrap();
        assert_eq!(editor.document().lines[0], ">> hello world");
        assert_eq!(editor.selection().head, Position::new(0, 9));
    });
}

#[gpui::test]
fn closing_the_last_editor_of_a_group_closes_the_group(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("close", &["a.txt", "b.txt"], cx);
    run(&workbench, "workbench.action.moveEditorToNextGroup", cx);
    assert_eq!(groups(&workbench, cx), (vec![vec!["a.txt".to_string()], vec!["b.txt".to_string()]], 1));
    run(&workbench, "workbench.action.closeActiveEditor", cx);
    assert_eq!(groups(&workbench, cx), (vec![vec!["a.txt".to_string()]], 0));
}

#[gpui::test]
fn close_group_merges_its_editors_into_the_most_recent_group(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("merge", &["a.txt", "b.txt"], cx);
    run(&workbench, "workbench.action.splitEditorDown", cx);
    run(&workbench, "workbench.action.focusPreviousGroup", cx);
    run(&workbench, "workbench.action.moveEditorToNextGroup", cx);
    assert_eq!(groups(&workbench, cx), (vec![vec!["a.txt".to_string()], vec!["b.txt".to_string()]], 1));
    run(&workbench, "workbench.action.closeGroup", cx);
    assert_eq!(groups(&workbench, cx), (vec![vec!["a.txt".to_string(), "b.txt".to_string()]], 0));
}

#[gpui::test]
fn editor_layouts_add_and_merge_groups(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("layouts", &["a.txt"], cx);
    run(&workbench, "workbench.action.editorLayoutTwoByTwoGrid", cx);
    assert_eq!(groups(&workbench, cx).0.len(), 4);
    run(&workbench, "workbench.action.focusFourthEditorGroup", cx);
    assert_eq!(groups(&workbench, cx).1, 3);
    run(&workbench, "workbench.action.editorLayoutSingle", cx);
    assert_eq!(groups(&workbench, cx), (vec![vec!["a.txt".to_string()]], 0));
}

#[gpui::test]
fn go_back_returns_to_the_previous_editor_and_location(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("navigate", &["a.txt", "b.txt"], cx);
    let active = |workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().name());
    assert_eq!(active(&workbench, cx), "b.txt");
    run(&workbench, "workbench.action.navigateBack", cx);
    assert_eq!(active(&workbench, cx), "a.txt");
    let context = workbench.update_in(cx, |workbench, window, _| workbench.key_context(window));
    assert_eq!((context["canNavigateBack"].clone(), context["canNavigateForward"].clone()), (json!(false), json!(true)));
    run(&workbench, "workbench.action.navigateForward", cx);
    assert_eq!(active(&workbench, cx), "b.txt");
}

#[gpui::test]
fn reopen_closed_editor_brings_back_the_last_closed_file(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("reopen", &["a.txt", "b.txt"], cx);
    run(&workbench, "workbench.action.closeActiveEditor", cx);
    assert_eq!(groups(&workbench, cx).0, vec![vec!["a.txt".to_string()]]);
    run(&workbench, "workbench.action.reopenClosedEditor", cx);
    assert_eq!(groups(&workbench, cx).0, vec![vec!["a.txt".to_string(), "b.txt".to_string()]]);
}

#[test]
fn a_drag_splits_at_the_edges_and_moves_in_the_center() {
    use super::split_direction;
    use crate::editor_grid::Direction;
    // editorDropTarget.ts with openSideBySideDirection right: the left and right thirds split sideways.
    assert_eq!(split_direction(50., 50., 100., 100., true), None);
    assert_eq!(split_direction(5., 50., 100., 100., true), Some(Direction::Left));
    assert_eq!(split_direction(95., 50., 100., 100., true), Some(Direction::Right));
    assert_eq!(split_direction(50., 5., 100., 100., true), Some(Direction::Up));
    assert_eq!(split_direction(50., 5., 100., 100., false), Some(Direction::Up));
    assert_eq!(split_direction(20., 95., 100., 100., false), Some(Direction::Down));
}

#[gpui::test]
fn the_output_view_shows_in_the_panel_and_takes_the_focus_from_the_groups(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_with("output-panel", &["a.txt"], cx);
    std::fs::write(dir.join("renderer.log"), "started\n").unwrap();
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.output.register_window_log(dir.join("renderer.log"));
        workbench.show_output_channel(crate::output::WINDOW_LOG_ID, false, window, cx);
    });
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, _| {
        assert!(workbench.output_view_visible());
        assert_eq!(workbench.editors.id, crate::editor::PANEL_GROUP);
        let context = workbench.key_context(window);
        assert_eq!((context["inOutput"].clone(), context["activeEditorGroupIndex"].clone()), (json!(true), json!(1)));
    });
    assert_eq!(groups(&workbench, cx), (vec![vec!["a.txt".to_string()]], 0));
    run(&workbench, "workbench.action.splitEditor", cx);
    assert_eq!(groups(&workbench, cx), (vec![vec!["a.txt".to_string()], vec!["a.txt".to_string()]], 1));
}

#[gpui::test]
fn close_editors_in_other_groups_leaves_the_active_group(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("close-others", &["a.txt"], cx);
    cx.simulate_keystrokes("cmd-\\");
    cx.simulate_keystrokes("cmd-\\");
    cx.run_until_parked();
    assert_eq!(groups(&workbench, cx).0.len(), 3);
    run(&workbench, "workbench.action.closeEditorsInOtherGroups", cx);
    assert_eq!(groups(&workbench, cx), (vec![vec!["a.txt".to_string()]], 0));
    // CloseAllEditorsAction empties every group; the emptied ones close.
    cx.simulate_keystrokes("cmd-\\");
    run(&workbench, "workbench.action.closeAllEditors", cx);
    assert_eq!(groups(&workbench, cx), (vec![Vec::<String>::new()], 0));
}

#[gpui::test]
fn the_grid_and_its_editors_come_back_from_workspace_storage(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("part-state", &["a.txt", "b.txt"], cx);
    cx.simulate_keystrokes("cmd-\\");
    cx.run_until_parked();
    run(&workbench, "workbench.action.focusFirstEditorGroup", cx);
    let saved = workbench.read_with(cx, |workbench, _| workbench.workspace_storage.lock().unwrap().get("memento/workbench.parts.editor").map(str::to_owned).unwrap());
    let before = groups(&workbench, cx);
    assert_eq!(before, (vec![vec!["a.txt".to_string(), "b.txt".to_string()], vec!["b.txt".to_string()]], 0));
    run(&workbench, "workbench.action.closeAllEditors", cx);
    assert_eq!(groups(&workbench, cx), (vec![Vec::<String>::new()], 0));
    let storage = std::sync::Arc::new(std::sync::Mutex::new(storage::Storage::in_memory()));
    storage.lock().unwrap().set("memento/workbench.parts.editor", saved);
    workbench.update(cx, |workbench, _| workbench.restore_workspace_state(storage));
    assert_eq!(groups(&workbench, cx), before);
    let active = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().name());
    assert_eq!(active, "b.txt");
}

/// editorCommands.ts `moveEditorToRightGroup` (the tab menu's Split & Move > Move Right): a new group when there is
/// none that way, the existing one after.
#[gpui::test]
fn move_right_moves_the_editor_into_the_group_on_the_right(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("move-right", &["a.txt", "b.txt"], cx);
    run(&workbench, "workbench.action.moveEditorToRightGroup", cx);
    assert_eq!(groups(&workbench, cx), (vec![vec!["a.txt".to_string()], vec!["b.txt".to_string()]], 1));
    run(&workbench, "workbench.action.focusFirstEditorGroup", cx);
    run(&workbench, "workbench.action.moveEditorToRightGroup", cx);
    assert_eq!(groups(&workbench, cx).0, vec![vec!["b.txt".to_string(), "a.txt".to_string()]], "into the existing group, the emptied one closes");
}

/// editorGroupModel.ts `serialize`/`deserialize`: the last sticky editor's index is stored and the sticky ones come
/// back sticky.
#[gpui::test]
fn sticky_editors_come_back_sticky_from_workspace_storage(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("part-state-sticky", &["a.txt", "b.txt", "c.txt"], cx);
    run(&workbench, "workbench.action.pinEditor", cx);
    let saved = workbench.read_with(cx, |workbench, _| workbench.workspace_storage.lock().unwrap().get("memento/workbench.parts.editor").map(str::to_owned).unwrap());
    let state: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(state["editorpart.state"]["serializedGrid"]["root"]["data"][0]["data"]["sticky"], json!(0));
    workbench.update(cx, |workbench, _| {
        for index in (0..3).rev() {
            workbench.editors.close(index);
        }
    });
    let storage = std::sync::Arc::new(std::sync::Mutex::new(storage::Storage::in_memory()));
    storage.lock().unwrap().set("memento/workbench.parts.editor", saved);
    workbench.update(cx, |workbench, _| workbench.restore_workspace_state(storage));
    let tabs = workbench.read_with(cx, |workbench, _| workbench.editors.editors.iter().map(|editor| (editor.document().name(), editor.sticky)).collect::<Vec<_>>());
    assert_eq!(tabs, [("c.txt".to_string(), true), ("a.txt".to_string(), false), ("b.txt".to_string(), false)]);
}

/// dnd.ts `ResourcesDropHandler`: a file dropped from Finder on a group's right half opens pinned in a new group there.
#[gpui::test]
fn files_dropped_from_outside_open_in_the_group_the_overlay_shows(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_with("external-drop", &["a.txt"], cx);
    let dropped = dir.join("b.txt");
    std::fs::write(&dropped, "dropped").unwrap();
    workbench.update_in(cx, |workbench, window, cx| {
        let group = workbench.editor_group_id();
        workbench.editor_groups.drop_target = Some((group, Some(crate::editor_grid::Direction::Right)));
        let target = workbench.drop_target_group(group).unwrap();
        workbench.drop_external_paths(vec![dropped.clone()], target, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(groups(&workbench, cx), (vec![vec!["a.txt".to_owned()], vec!["b.txt".to_owned()]], 1));
    assert!(workbench.read_with(cx, |workbench, _| !workbench.editors.active().unwrap().preview));
    std::fs::remove_dir_all(&dir).unwrap();
}
