use std::path::{Path, PathBuf};

use exthost::scan::scan_extension;
use gpui::{AnyWindowHandle, TestAppContext, VisualTestContext};

use super::super::editor_groups::MAIN_PART;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

fn workbench_with<'a>(name: &str, files: &[&str], cx: &'a mut TestAppContext) -> (PathBuf, gpui::Entity<Workbench>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-auxiliary-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<PathBuf> = files.iter().map(|file| dir.join(file)).collect();
    for path in &paths {
        std::fs::write(path, "hello world").unwrap();
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

/// Each part's groups with their tab names, the main part first.
fn parts(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<Vec<Vec<String>>> {
    workbench.read_with(cx, |workbench, _| {
        let ids: Vec<u32> = std::iter::once(MAIN_PART).chain(workbench.auxiliary_windows.part_ids()).collect();
        let names = |group: u32| workbench.editors.group_tabs(group).0.iter().map(|editor| editor.document().name()).collect();
        ids.into_iter().map(|part| workbench.part_groups(part).into_iter().map(names).collect()).collect()
    })
}

fn editor_window(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> AnyWindowHandle {
    let part = workbench.read_with(cx, |workbench, _| workbench.auxiliary_windows.part_ids().next().unwrap());
    workbench.read_with(cx, |workbench, _| workbench.auxiliary_windows.windows.iter().find(|window| window.part == part).unwrap().handle.unwrap().into())
}

#[gpui::test]
fn move_editor_into_new_window_opens_a_part_with_the_editor(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("move", &["a.txt", "b.txt"], cx);
    run(&workbench, "workbench.action.moveEditorToNewWindow", cx);
    assert_eq!(parts(&workbench, cx), vec![vec![vec!["a.txt".to_string()]], vec![vec!["b.txt".to_string()]]]);
    assert_eq!(cx.windows().len(), 2);
    let (active_part, columns) = workbench.read_with(cx, |workbench, _| (workbench.editor_groups.active_part(), workbench.groups_in_grid_order().len()));
    assert_ne!(active_part, MAIN_PART);
    assert_eq!(columns, 2);
    // The editor window's keys reach the workbench: Cmd+W closes its last editor, and the window with it.
    let handle = editor_window(&workbench, cx);
    let mut window_cx = VisualTestContext::from_window(handle, cx);
    window_cx.simulate_keystrokes("cmd-w");
    window_cx.run_until_parked();
    assert_eq!(parts(&workbench, cx), vec![vec![vec!["a.txt".to_string()]]]);
    assert_eq!(cx.windows().len(), 1);
}

#[gpui::test]
fn closing_an_editor_window_moves_its_dirty_editors_to_the_main_window(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("close", &["a.txt", "b.txt"], cx);
    run(&workbench, "workbench.action.copyEditorGroupToNewWindow", cx);
    assert_eq!(parts(&workbench, cx), vec![vec![vec!["a.txt".to_string(), "b.txt".to_string()]], vec![vec!["a.txt".to_string(), "b.txt".to_string()]]]);
    run(&workbench, "workbench.action.closeActiveEditor", cx);
    run(&workbench, "workbench.action.files.newUntitledFile", cx);
    workbench.update(cx, |workbench, _| workbench.editors.active_mut().unwrap().type_text("draft"));
    assert_eq!(parts(&workbench, cx)[1], vec![vec!["a.txt".to_string(), "Untitled-1".to_string()]]);
    let part = workbench.read_with(cx, |workbench, _| workbench.editor_groups.active_part());
    workbench.update(cx, |workbench, cx| workbench.close_auxiliary_window(part, cx));
    cx.run_until_parked();
    // auxiliaryEditorPart.ts `doClose(true)`: the saved editor closes, the untitled one moves.
    assert_eq!(parts(&workbench, cx), vec![vec![vec!["a.txt".to_string(), "b.txt".to_string(), "Untitled-1".to_string()]]]);
    assert_eq!(cx.windows().len(), 1);
}

#[gpui::test]
fn a_tab_dropped_on_the_main_part_leaves_the_editor_window(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("drop", &["a.txt", "b.txt"], cx);
    run(&workbench, "workbench.action.moveEditorToNewWindow", cx);
    workbench.update_in(cx, |workbench, window, cx| {
        let source = workbench.editors.id;
        let target = workbench.part_active_group(MAIN_PART).unwrap();
        let dragged = workbench.dragged_tab(source, 0, cx);
        workbench.move_dragged_tab(&dragged, target, None, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(parts(&workbench, cx), vec![vec![vec!["a.txt".to_string(), "b.txt".to_string()]]]);
    assert_eq!(cx.windows().len(), 1);
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.editor_groups.active_part()), MAIN_PART);
}

#[gpui::test]
fn editor_windows_come_back_with_their_editors(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("restore", &["a.txt", "b.txt"], cx);
    run(&workbench, "workbench.action.moveEditorToNewWindow", cx);
    run(&workbench, "workbench.action.splitEditorDown", cx);
    let storage = workbench.read_with(cx, |workbench, _| workbench.workspace_storage.clone());
    let state = workbench.read_with(cx, |workbench, _| workbench.stored_editor_parts_state()).unwrap();
    assert_eq!(state["auxiliary"][0]["state"]["serializedGrid"]["root"]["data"].as_array().unwrap().len(), 2);
    assert_eq!(state["mru"], serde_json::json!([1, 0]));
    // A second window on the same workspace storage restores the part.
    let (_dir, restored, cx) = workbench_with("restored", &["a.txt"], &mut cx.cx);
    restored.update(cx, |workbench, cx| {
        workbench.workspace_storage = storage;
        workbench.restore_auxiliary_windows(cx);
    });
    cx.run_until_parked();
    assert_eq!(parts(&restored, cx)[1], vec![vec!["b.txt".to_string()], vec!["b.txt".to_string()]]);
    assert_ne!(restored.read_with(cx, |workbench, _| workbench.editor_groups.active_part()), MAIN_PART);
}
