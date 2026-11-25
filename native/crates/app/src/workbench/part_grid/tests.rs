use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use config::workspace::Workspace;
use exthost::scan::scan_extension;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use storage::Storage;

use super::super::layout::Part;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

struct Stores {
    profile: Arc<Mutex<Storage>>,
    workspace: Arc<Mutex<Storage>>,
}

fn open<'a>(folder: Option<PathBuf>, stores: &Stores, cx: &'a mut TestAppContext) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let theme = load_theme(&[scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap()], "Dark Modern").unwrap();
    let (profile, workspace) = (stores.profile.clone(), stores.workspace.clone());
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root, folder.map(Workspace::folder), Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), std::env::temp_dir().join("code-gpui-part-grid-keybindings.json"), cx);
        workbench.restore_profile_state(profile);
        workbench.restore_workspace_state(workspace);
        workbench
    });
    cx.run_until_parked();
    (workbench, cx)
}

fn stores() -> Stores {
    Stores { profile: Arc::new(Mutex::new(Storage::in_memory())), workspace: Arc::new(Mutex::new(Storage::in_memory())) }
}

fn side_bar_visible(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> bool {
    workbench.read_with(cx, |workbench, _| workbench.is_part_visible(Part::SideBar))
}

#[gpui::test]
fn the_side_bar_is_hidden_in_an_empty_window_and_cmd_b_toggles_it_for_the_next_open(cx: &mut TestAppContext) {
    let stores = stores();
    let (workbench, window_cx) = open(None, &stores, cx);
    assert!(!side_bar_visible(&workbench, window_cx));
    window_cx.simulate_keystrokes("cmd-b");
    assert!(side_bar_visible(&workbench, window_cx));
    assert_eq!(stores.workspace.lock().unwrap().get("workbench.sideBar.hidden"), Some("false"));
    window_cx.simulate_keystrokes("cmd-b");
    assert!(!side_bar_visible(&workbench, window_cx));
    // layoutActions.ts: Close Primary Side Bar is enabled only while the side bar shows.
    let context = workbench.update_in(window_cx, |workbench, window, _| workbench.key_context(window));
    assert_eq!(context["sideBarVisible"], false);
    window_cx.simulate_keystrokes("cmd-b");
    drop(workbench);
    window_cx.update(|window, _| window.remove_window());

    let (reopened, window_cx) = open(None, &stores, cx);
    assert!(side_bar_visible(&reopened, window_cx));
}

#[gpui::test]
fn dragging_the_side_bar_sash_resizes_it_stores_the_size_and_past_half_its_minimum_hides_it(cx: &mut TestAppContext) {
    let stores = stores();
    let dir = std::env::temp_dir().join(format!("code-gpui-part-grid-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (workbench, window_cx) = open(Some(dir.clone()), &stores, cx);
    let side_bar_width = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.part_grid.grid.as_ref().unwrap().row_sizes()[1]);
    let width = side_bar_width(window_cx);
    // The sash fills the 4px gap after the side bar, below the 35px title bar.
    let sash = point(px(48. + width + 2.), px(300.));
    window_cx.simulate_mouse_move(sash, None, Modifiers::none());
    window_cx.simulate_mouse_down(sash, MouseButton::Left, Modifiers::none());
    window_cx.simulate_mouse_move(sash + point(px(40.), px(0.)), MouseButton::Left, Modifiers::none());
    window_cx.simulate_mouse_up(sash + point(px(40.), px(0.)), MouseButton::Left, Modifiers::none());
    assert_eq!(side_bar_width(window_cx), width + 40.);
    assert_eq!(stores.profile.lock().unwrap().get("workbench.sideBar.size"), Some((width + 40.).to_string().as_str()));

    let sash = sash + point(px(40.), px(0.));
    window_cx.simulate_mouse_down(sash, MouseButton::Left, Modifiers::none());
    window_cx.simulate_mouse_move(sash - point(px(width + 40. - 80.), px(0.)), MouseButton::Left, Modifiers::none());
    window_cx.simulate_mouse_up(sash - point(px(width + 40. - 80.), px(0.)), MouseButton::Left, Modifiers::none());
    assert!(!side_bar_visible(&workbench, window_cx));
    std::fs::remove_dir_all(&dir).unwrap();
}
