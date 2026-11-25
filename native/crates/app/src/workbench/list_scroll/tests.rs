use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use config::workspace::Workspace;
use exthost::scan::scan_extension;
use gpui::{Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext, point, px};
use serde_json::json;

use super::ListId;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::list_tree::StickyRow;
use crate::theme::load_theme;
use crate::workbench::Workbench;

/// A folder with `deep/` holding 200 files, expanded and focused in the Explorer.
fn explorer<'a>(name: &str, cx: &'a mut TestAppContext) -> (PathBuf, gpui::Entity<Workbench>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-list-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("deep")).unwrap();
    std::fs::create_dir_all(dir.join("User")).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    for index in 0..200 {
        std::fs::write(dir.join(format!("deep/f{index:03}.txt")), "").unwrap();
    }
    let host = HostSync::new(std::sync::mpsc::channel().0);
    let folder = dir.clone();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(Workspace::folder(folder.clone())), Languages::new(&[]), settings.clone(), host, cx);
        workbench.skip_dialogs = true;
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), folder.join("keybindings.json"), cx);
        let tree = workbench.folder.as_mut().unwrap();
        tree.set_expanded(&folder.join("deep"), true);
        tree.set_selection(&folder.join("deep/f001.txt"));
        window.focus(&workbench.explorer.focus.clone(), cx);
        workbench
    });
    cx.run_until_parked();
    (dir, workbench, cx)
}

fn run(workbench: &gpui::Entity<Workbench>, command: &str, cx: &mut VisualTestContext) {
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(command, Vec::new(), window, cx)));
    cx.run_until_parked();
}

fn scroll_top(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> f32 {
    workbench.read_with(cx, |workbench, _| workbench.list_scroller(ListId::Explorer).position().y)
}

fn wheel(workbench: &gpui::Entity<Workbench>, lines: f32, modifiers: Modifiers, cx: &mut VisualTestContext) {
    let bounds = workbench.read_with(cx, |workbench, _| workbench.list_scroller(ListId::Explorer).bounds());
    cx.simulate_event(ScrollWheelEvent { position: bounds.center(), delta: ScrollDelta::Lines(point(0., lines)), modifiers, ..Default::default() });
    cx.run_until_parked();
}

/// listCommands.ts `list.scrollDown` (10px) and scrollableElement.ts `_onMouseWheel`: 50px a notch times
/// `workbench.list.mouseWheelScrollSensitivity`, `fastScrollSensitivity` more with Alt; `listScrollAtBoundary`.
#[gpui::test]
fn the_wheel_and_the_scroll_commands_move_the_explorer(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("scroll", cx);
    let boundary = |cx: &mut VisualTestContext| workbench.update_in(cx, |workbench, window, _| workbench.key_context(window).get("listScrollAtBoundary").cloned());
    assert_eq!(boundary(cx), Some(json!("top")));
    run(&workbench, "list.scrollDown", cx);
    assert_eq!(scroll_top(&workbench, cx), 10.);
    assert_eq!(boundary(cx), Some(json!("none")));
    wheel(&workbench, -1., Modifiers::none(), cx);
    assert_eq!(scroll_top(&workbench, cx), 60.);
    wheel(&workbench, -1., Modifiers::alt(), cx);
    assert_eq!(scroll_top(&workbench, cx), 310.);
    workbench.update(cx, |workbench, _| workbench.update_setting("workbench.list.mouseWheelScrollSensitivity", json!(2)));
    wheel(&workbench, 1., Modifiers::none(), cx);
    assert_eq!(scroll_top(&workbench, cx), 210.);
    run(&workbench, "list.scrollUp", cx);
    assert_eq!(scroll_top(&workbench, cx), 200.);
    std::fs::remove_dir_all(dir).unwrap();
}

/// abstractTree.ts sticky scroll: the scrolled folder sticks at the top; a press on it scrolls it back under the
/// widget and selects it (`handleStickyScrollMouseEvent`); `tree.toggleStickyScroll` turns it off.
#[gpui::test]
fn the_scrolled_folder_sticks_and_a_press_goes_back_to_it(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("sticky", cx);
    let sticky = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.tree_sticky_rows(ListId::Explorer, &workbench.tree_depths(ListId::Explorer)));
    assert_eq!(sticky(cx), []);
    workbench.update(cx, |workbench, cx| workbench.set_list_scroll(ListId::Explorer, point(0., 300.), cx));
    cx.run_until_parked();
    // Rows: a.txt is after deep/ and its files, so deep/ is row 0.
    assert_eq!(sticky(cx), [StickyRow { index: 0, position: 0. }]);

    let bounds = workbench.read_with(cx, |workbench, _| workbench.list_scroller(ListId::Explorer).bounds());
    cx.simulate_click(bounds.origin + point(px(60.), px(11.)), Modifiers::none());
    assert_eq!(scroll_top(&workbench, cx), 0.);
    let (selected, expanded) = workbench.read_with(cx, |workbench, _| {
        let tree = workbench.folder.as_ref().unwrap();
        (tree.selected.clone(), tree.rows().len() > 2)
    });
    assert_eq!(selected, [dir.join("deep")]);
    assert!(expanded, "a press on a sticky folder does not collapse it");

    run(&workbench, "tree.toggleStickyScroll", cx);
    workbench.update(cx, |workbench, cx| workbench.set_list_scroll(ListId::Explorer, point(0., 300.), cx));
    assert_eq!(sticky(cx), []);
    std::fs::remove_dir_all(dir).unwrap();
}

/// listCommands.ts `list.focusParent`, `list.showHover` and `list.collapseAllToFocus` on the Explorer.
#[gpui::test]
fn the_tree_commands_move_to_the_parent_show_its_hover_and_collapse_it(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("tree-commands", cx);
    let focused = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.folder.as_ref().unwrap().focused.clone());
    run(&workbench, "list.focusParent", cx);
    assert_eq!(focused(cx), Some(dir.join("deep")));
    run(&workbench, "list.showHover", cx);
    let hover = workbench.read_with(cx, |workbench, _| workbench.hovers.shown_text().map(str::to_string));
    assert_eq!(hover, Some(crate::quick_input::path_label(&dir.join("deep"), None)));
    run(&workbench, "list.collapseAllToFocus", cx);
    let (rows, selected) = workbench.read_with(cx, |workbench, _| {
        let tree = workbench.folder.as_ref().unwrap();
        (tree.rows().len(), tree.selected.clone())
    });
    // deep/, User/ and a.txt.
    assert_eq!((rows, selected), (3, vec![dir.join("deep")]));
    std::fs::remove_dir_all(dir).unwrap();
}

/// The real ui-pair steps: a key press (GPUI's keyboard modality) and then the wheel, the pointer still.
#[gpui::test]
fn the_wheel_scrolls_after_a_key_press(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("scroll-after-key", cx);
    cx.simulate_keystrokes("down");
    wheel(&workbench, -1., Modifiers::none(), cx);
    assert_eq!(scroll_top(&workbench, cx), 50.);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A scroll moves the row out from under its hover: Chromium's mouse move after the scroll leaves the row.
#[gpui::test]
fn scrolling_hides_the_rows_hover(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("scroll-hover", cx);
    let hover = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.hovers.shown_text().map(str::to_string));
    run(&workbench, "list.focusParent", cx);
    run(&workbench, "list.showHover", cx);
    assert!(hover(cx).is_some());
    run(&workbench, "list.scrollDown", cx);
    cx.executor().advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(hover(cx), None);
    std::fs::remove_dir_all(dir).unwrap();
}
