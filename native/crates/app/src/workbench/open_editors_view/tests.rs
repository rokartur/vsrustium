use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::{Settings, Target};

use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::json;

use super::Row;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;
use crate::workbench::view_panes::{HEADER_SIZE, OPEN_EDITORS_VIEW, PaneContainer};

fn workbench_with<'a>(name: &str, files: &[&str], cx: &'a mut TestAppContext) -> (PathBuf, gpui::Entity<Workbench>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-open-editors-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<PathBuf> = files.iter().map(|file| dir.join(file)).collect();
    for path in &paths {
        std::fs::write(path, "hello").unwrap();
    }
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.skip_dialogs = true;
        workbench.profile_storage.lock().unwrap().set("workbench.explorer.views.state.hidden", json!([{ "id": OPEN_EDITORS_VIEW, "isHidden": false }]).to_string());
        for path in &paths {
            workbench.open_file(path.clone(), true, window, cx);
        }
        workbench
    });
    cx.run_until_parked();
    (dir, workbench, cx)
}

fn names(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    workbench.read_with(cx, |workbench, _| {
        let row_name = |row| match row {
            Row::Group(_, ordinal) => format!("GROUP {ordinal}"),
            Row::Editor(group, index) => workbench.editors.group_tabs(group).0[index].document().name(),
        };
        workbench.open_editors_rows().into_iter().map(row_name).collect()
    })
}

fn open_editors_size(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> f32 {
    workbench.update(cx, |workbench, cx| {
        workbench.layout_view_panes(900., cx);
        let view = workbench.panes(PaneContainer::Explorer).view.as_ref().unwrap();
        view.sizes()[view.ids().iter().position(|&id| id == OPEN_EDITORS_VIEW).unwrap()]
    })
}

#[gpui::test]
fn rows_are_the_editors_and_the_groups_show_from_two_on(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("rows", &["b.txt", "a.txt"], cx);
    assert_eq!(names(&workbench, cx), ["b.txt", "a.txt"]);
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.splitEditor", vec![], window, cx)));
    cx.run_until_parked();
    assert_eq!(names(&workbench, cx), ["GROUP 1", "b.txt", "a.txt", "GROUP 2", "a.txt"]);
    workbench.update(cx, |workbench, _| drop(workbench.settings.lock().unwrap().update_value("explorer.openEditors.sortOrder", Some(&json!("alphabetical")), Target::User, None, &[]).unwrap()));
    assert_eq!(names(&workbench, cx), ["GROUP 1", "a.txt", "b.txt", "GROUP 2", "a.txt"]);
}

#[gpui::test]
fn the_pane_is_as_tall_as_its_rows_up_to_the_visible_setting(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("size", &["a.txt", "b.txt", "c.txt"], cx);
    assert_eq!(open_editors_size(&workbench, cx), HEADER_SIZE + 3. * 22.);
    workbench.update(cx, |workbench, _| drop(workbench.settings.lock().unwrap().update_value("explorer.openEditors.visible", Some(&json!(2)), Target::User, None, &[]).unwrap()));
    // At least 2 rows, at most 3: the size it had stays.
    assert_eq!(open_editors_size(&workbench, cx), HEADER_SIZE + 3. * 22.);
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.closeAllEditors", vec![], window, cx)));
    cx.run_until_parked();
    assert_eq!(open_editors_size(&workbench, cx), HEADER_SIZE);
}

#[gpui::test]
fn the_badge_counts_each_unsaved_working_copy_once(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("badge", &["a.txt"], cx);
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.dirty_working_copy_count(), 0));
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("type", vec![json!({ "text": "x" })], window, cx)));
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.dirty_working_copy_count(), 1));
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.splitEditor", vec![], window, cx)));
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.dirty_working_copy_count(), 1, "the split copy is the same working copy"));
}

#[gpui::test]
fn the_focus_command_shows_the_hidden_view_and_focuses_it(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("focus", &["a.txt"], cx);
    workbench.update(cx, |workbench, cx| {
        workbench.profile_storage.lock().unwrap().set("workbench.explorer.views.state.hidden", "[]".into());
        workbench.layout_view_panes(900., cx);
        assert!(!workbench.panes(PaneContainer::Explorer).view.as_ref().unwrap().ids().contains(&OPEN_EDITORS_VIEW), "hidden by default");
    });
    workbench.update_in(cx, |workbench, window, cx| {
        drop(workbench.execute_command("workbench.files.action.focusOpenEditorsView", vec![], window, cx));
        assert!(workbench.panes(PaneContainer::Explorer).view.as_ref().unwrap().ids().contains(&OPEN_EDITORS_VIEW));
        assert!(workbench.open_editors_focus.is_focused(window));
    });
}

fn menu_titles(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    workbench.read_with(cx, |workbench, _| {
        let menu = workbench.context_menu.as_ref().expect("a context menu shows");
        let title = |action: &crate::menus::MenuAction| match action {
            crate::menus::MenuAction::Command { title, enabled: true, .. } => title.clone(),
            crate::menus::MenuAction::Command { title, .. } => format!("({title})"),
            _ => "---".into(),
        };
        menu.levels[0].actions.iter().map(title).collect()
    })
}

/// listCommands.ts on the Open Editors list: arrows move the focus from the active editor's row, Space opens the
/// focused editor keeping the focus, Enter opens it pinned with the focus.
#[gpui::test]
fn the_keyboard_moves_the_focus_and_opens_rows(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("keyboard", &["a.txt", "b.txt", "c.txt"], cx);
    let run = |command: &str, cx: &mut VisualTestContext| {
        workbench.update_in(cx, |workbench, window, cx| {
            window.focus(&workbench.open_editors_focus.clone(), cx);
            drop(workbench.execute_command(command, vec![], window, cx));
        })
    };
    let state = |cx: &mut VisualTestContext| {
        workbench.update_in(cx, |workbench, window, _| (workbench.open_editors_focused_row, workbench.editors.active().unwrap().document().name(), workbench.open_editors_focus.is_focused(window)))
    };
    run("list.focusUp", cx);
    assert_eq!(state(cx), (Some(1), "c.txt".to_string(), true), "from the active editor's row");
    run("list.focusUp", cx);
    run("list.focusUp", cx);
    assert_eq!(state(cx).0, Some(0));
    workbench.update(cx, |workbench, _| workbench.editors.editors[0].preview = true);
    run("list.toggleExpand", cx);
    assert_eq!(state(cx), (None, "a.txt".to_string(), true), "Space opens it, the focus stays in the list");
    assert!(workbench.read_with(cx, |workbench, _| workbench.editors.editors[0].preview), "as a preview");
    run("list.focusLast", cx);
    run("list.select", cx);
    assert_eq!(state(cx), (None, "c.txt".to_string(), false), "Enter opens it with the focus");
}

/// fileActions.contribution.ts `MenuId.OpenEditorsContext`: an editor row's and a group row's items; a sticky row's
/// action unpins it.
#[gpui::test]
fn rows_show_the_open_editors_menu(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = workbench_with("menu", &["a.txt", "b.txt"], cx);
    let pointer = gpui::point(gpui::px(100.), gpui::px(100.));
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.pinEditor", vec![json!({ "editorIndex": 1 })], window, cx)));
    assert_eq!(names(&workbench, cx), ["b.txt", "a.txt"], "the pinned editor's row comes first");
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.unpinEditor", vec![json!({ "editorIndex": 0 })], window, cx)));
    assert!(workbench.read_with(cx, |workbench, _| workbench.editors.editors.iter().all(|editor| !editor.sticky)));
    workbench.update_in(cx, |workbench, window, cx| workbench.show_open_editors_context_menu(0, Row::Editor(0, 0), pointer, window, cx));
    assert_eq!(
        menu_titles(&workbench, cx),
        ["Open to the Side", "Reveal in Finder", "---", "Copy Path", "Copy Relative Path", "---", "Reopen Editor With...", "---", "(Save)", "(Revert File)", "---", "(Compare with Saved)", "Select for Compare", "---", "Close", "Close Others", "Close Saved", "Close All"]
    );
    cx.simulate_keystrokes("escape");
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.splitEditor", vec![], window, cx)));
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, cx| workbench.show_open_editors_context_menu(0, Row::Group(0, 1), pointer, window, cx));
    assert_eq!(menu_titles(&workbench, cx), ["(Save All)", "---", "Close Saved", "Close All"]);
    cx.simulate_keystrokes("escape");

    let a = workbench.read_with(cx, |workbench, _| workbench.editors.group_tabs(0).0.iter().find(|editor| editor.document().name() == "a.txt").unwrap().document().uri());
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("explorer.openToSide", vec![a], window, cx)));
    cx.run_until_parked();
    assert_eq!(names(&workbench, cx), ["GROUP 1", "b.txt", "a.txt", "GROUP 2", "b.txt", "a.txt"], "Open to the Side opens in the group beside");
}
