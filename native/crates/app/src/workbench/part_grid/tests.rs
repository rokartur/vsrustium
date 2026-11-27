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
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let theme = load_theme(&[scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap()], "Dark Modern").unwrap();
    let (profile, workspace) = (stores.profile.clone(), stores.workspace.clone());
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root, folder.map(Workspace::folder), Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), std::env::temp_dir().join("code-gpui-part-grid-keybindings.json"), cx);
        workbench.restore_profile_state(profile.clone(), profile);
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
fn the_activity_bar_on_top_is_a_tab_strip_in_the_side_bar_header_whose_tabs_switch_views(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("code-gpui-compact-bar-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (workbench, window_cx) = open(Some(dir.clone()), &stores(), cx);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None);
    workbench.update(window_cx, |workbench, _| *workbench.settings.lock().unwrap() = settings);
    workbench.update_in(window_cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.activityBarLocation.top", Vec::new(), window, cx)));
    window_cx.run_until_parked();
    assert!(workbench.read_with(window_cx, |workbench, _| !workbench.is_part_visible(Part::ActivityBar)));
    // The side bar card starts 4pt in under the 35pt title bar; its second 36pt tab is Search.
    window_cx.simulate_click(point(px(4. + 36. + 18.), px(35. + 1. + 16.)), Modifiers::none());
    assert!(workbench.read_with(window_cx, |workbench, _| workbench.side_bar_view == crate::workbench::search_view::SideBarView::Search));
    std::fs::remove_dir_all(&dir).unwrap();
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
fn cmd_j_shows_the_panel_under_the_editor_and_maximize_hides_the_editor_until_the_panel_closes(cx: &mut TestAppContext) {
    let stores = stores();
    // The Output view, as the default Terminal view would start a shell the test scheduler cannot track.
    stores.workspace.lock().unwrap().set("workbench.panelpart.activepanelid", super::super::output_view::OUTPUT_VIEW_ID.to_string());
    let (workbench, window_cx) = open(None, &stores, cx);
    let column = |cx: &mut VisualTestContext| {
        workbench.read_with(cx, |workbench, _| {
            let grid = workbench.part_grid.grid.as_ref().unwrap();
            let height = |part| grid.part_bounds(part).map_or(0., |bounds| bounds[3]);
            vec![height(Part::Editor), height(Part::Panel)]
        })
    };
    window_cx.simulate_keystrokes("cmd-j");
    assert!(workbench.read_with(window_cx, |workbench, _| workbench.is_part_visible(Part::Panel)));
    assert_eq!(stores.workspace.lock().unwrap().get("workbench.panel.hidden"), Some("false"));
    // layout.ts: the panel takes a third of the window's height, 768 / 3 in the test window.
    let [editor, panel] = column(window_cx)[..] else { panic!() };
    assert!(editor > 0. && panel > 0.);

    window_cx.update(|window, cx| workbench.update(cx, |workbench, cx| workbench.execute_command("workbench.action.toggleMaximizedPanel", Vec::new(), window, cx)));
    assert!(workbench.read_with(window_cx, |workbench, _| workbench.is_panel_maximized()));
    assert_eq!(column(window_cx)[0], 0.);
    window_cx.simulate_keystrokes("cmd-j");
    assert!(!workbench.read_with(window_cx, |workbench, _| workbench.is_part_visible(Part::Panel) || workbench.is_panel_maximized()));
    assert_eq!(column(window_cx)[0], editor + panel);

    window_cx.simulate_keystrokes("alt-cmd-b");
    assert!(workbench.read_with(window_cx, |workbench, _| workbench.is_part_visible(Part::AuxiliaryBar)));
    let context = workbench.update_in(window_cx, |workbench, window, _| workbench.key_context(window));
    assert_eq!((context["auxiliaryBarVisible"].clone(), context["panelVisible"].clone()), (true.into(), false.into()));
}

#[gpui::test]
fn dragging_the_side_bar_sash_resizes_it_stores_the_size_and_past_half_its_minimum_hides_it(cx: &mut TestAppContext) {
    let stores = stores();
    let dir = std::env::temp_dir().join(format!("code-gpui-part-grid-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (workbench, window_cx) = open(Some(dir.clone()), &stores, cx);
    let side_bar_width = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.part_grid.grid.as_ref().unwrap().part_bounds(Part::SideBar).map_or(0., |bounds| bounds[2]));
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

#[gpui::test]
fn explorer_pane_headers_toggle_with_the_mouse_and_keys_and_their_state_is_restored(cx: &mut TestAppContext) {
    let stores = stores();
    let dir = std::env::temp_dir().join(format!("code-gpui-view-panes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let collapsed = |stores: &Stores, id: &str| {
        let state: serde_json::Value = serde_json::from_str(stores.workspace.lock().unwrap().get("workbench.explorer.views.state").unwrap()).unwrap();
        state[id]["collapsed"].as_bool().unwrap()
    };
    let (_, window_cx) = open(Some(dir.clone()), &stores, cx);
    // The folder's header is the first pane, under the 35pt title bar, the card's border and the 32pt view title.
    window_cx.simulate_click(point(px(150.), px(35. + 1. + 32. + 14.)), Modifiers::none());
    assert!(collapsed(&stores, "workbench.explorer.fileView"));
    window_cx.simulate_keystrokes("down right");
    assert!(!collapsed(&stores, "outline"));
    assert!(collapsed(&stores, "timeline"));
    // gpui clicks a focused element when the activation key is released.
    for key in ["space", "enter"] {
        window_cx.simulate_keystrokes(key);
        window_cx.simulate_event(gpui::KeyUpEvent { keystroke: gpui::Keystroke::parse(key).unwrap() });
    }
    assert!(!collapsed(&stores, "outline"));
    window_cx.simulate_keystrokes("space");
    window_cx.simulate_event(gpui::KeyUpEvent { keystroke: gpui::Keystroke::parse("space").unwrap() });
    assert!(collapsed(&stores, "outline"));
    window_cx.update(|window, _| window.remove_window());

    let (_, window_cx) = open(Some(dir.clone()), &stores, cx);
    window_cx.simulate_click(point(px(150.), px(35. + 1. + 32. + 14.)), Modifiers::none());
    assert!(!collapsed(&stores, "workbench.explorer.fileView"));
    assert!(collapsed(&stores, "outline"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[gpui::test]
fn the_panel_moves_and_aligns_and_comes_back_where_it_was(cx: &mut TestAppContext) {
    use super::super::layout::{Alignment, Position};
    let stores = stores();
    stores.workspace.lock().unwrap().set("workbench.panelpart.activepanelid", super::super::output_view::OUTPUT_VIEW_ID.to_string());
    let (workbench, window_cx) = open(None, &stores, cx);
    let run = |command: &str, cx: &mut VisualTestContext| cx.update(|window, cx| workbench.update(cx, |workbench, cx| workbench.execute_command(command, Vec::new(), window, cx)));
    let bounds = |part: Part, cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.part_grid.grid.as_ref().unwrap().part_bounds(part));

    run("workbench.action.positionPanelRight", window_cx);
    window_cx.run_until_parked();
    let editor = bounds(Part::Editor, window_cx).unwrap();
    let panel = bounds(Part::Panel, window_cx).unwrap();
    assert_eq!(panel[0], editor[0] + editor[2]);
    assert_eq!(panel[3], editor[3]);
    assert_eq!(stores.workspace.lock().unwrap().get("workbench.panel.position"), Some("1"));
    let context = workbench.update_in(window_cx, |workbench, window, _| workbench.key_context(window));
    assert_eq!(context["panelPosition"], "right");

    // Aligning moves a side panel back to the bottom; only a centered panel maximizes.
    run("workbench.action.alignPanelJustify", window_cx);
    window_cx.run_until_parked();
    assert_eq!(stores.workspace.lock().unwrap().get("workbench.panel.position"), Some("2"));
    assert_eq!(stores.profile.lock().unwrap().get("workbench.panel.alignment"), Some("justify"));
    // Under the empty window's hidden side bar, from the activity bar to the window's edge.
    assert_eq!(bounds(Part::Panel, window_cx).unwrap()[0], 48.);
    run("workbench.action.toggleMaximizedPanel", window_cx);
    assert!(workbench.read_with(window_cx, |workbench, _| !workbench.is_panel_maximized() && workbench.is_part_visible(Part::Editor)));

    run("workbench.action.positionPanelLeft", window_cx);
    window_cx.run_until_parked();
    run("workbench.action.positionPanelTop", window_cx);
    window_cx.run_until_parked();
    run("workbench.action.alignPanelJustify", window_cx);
    window_cx.run_until_parked();
    assert_eq!(bounds(Part::Panel, window_cx).unwrap()[..2], [48., 0.]);
    run("workbench.action.positionPanelLeft", window_cx);
    window_cx.run_until_parked();
    run("workbench.action.toggleMaximizedPanel", window_cx);
    assert!(workbench.read_with(window_cx, |workbench, _| workbench.is_panel_maximized()));
    assert_eq!(stores.workspace.lock().unwrap().get("workbench.panel.wasLastMaximized"), Some("true"));
    window_cx.update(|window, _| window.remove_window());

    let (reopened, window_cx) = open(None, &stores, cx);
    let arrangement = reopened.read_with(window_cx, |workbench, _| workbench.arrangement());
    assert_eq!((arrangement.panel, arrangement.alignment), (Position::Left, Alignment::Justify));
    assert!(reopened.read_with(window_cx, |workbench, _| workbench.is_panel_maximized()));
}

#[gpui::test]
fn zen_mode_hides_the_parts_and_puts_back_the_ones_it_hid(cx: &mut TestAppContext) {
    let stores = stores();
    let dir = std::env::temp_dir().join(format!("code-gpui-zen-mode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (workbench, window_cx) = open(Some(dir.clone()), &stores, cx);
    let shown = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| (workbench.is_part_visible(Part::SideBar), workbench.is_part_visible(Part::ActivityBar), workbench.is_status_bar_visible()));
    assert_eq!(shown(window_cx), (true, true, true));
    window_cx.simulate_keystrokes("cmd-k z");
    assert_eq!(shown(window_cx), (false, false, false));
    let context = workbench.update_in(window_cx, |workbench, window, _| workbench.key_context(window));
    assert_eq!((context["inZenMode"].clone(), context["isCenteredLayout"].clone()), (true.into(), true.into()));
    assert_eq!(stores.workspace.lock().unwrap().get("workbench.zenMode.active"), Some("true"));
    window_cx.update(|window, _| window.remove_window());

    // zenMode.restore: the next window opens in zen mode, and leaving it shows the side bar again.
    let (reopened, window_cx) = open(Some(dir.clone()), &stores, cx);
    assert!(reopened.read_with(window_cx, |workbench, _| workbench.zen_mode.active && !workbench.is_part_visible(Part::SideBar)));
    window_cx.simulate_keystrokes("escape escape");
    assert!(reopened.read_with(window_cx, |workbench, _| !workbench.zen_mode.active && workbench.is_part_visible(Part::SideBar) && workbench.is_status_bar_visible()));
    assert!(!reopened.read_with(window_cx, |workbench, _| workbench.is_main_editor_centered()));

    // The first escape reaches a focused text editor first, the chord still completes.
    std::fs::write(dir.join("notes.txt"), "text\n").unwrap();
    reopened.update_in(window_cx, |workbench, window, cx| workbench.open_file(dir.join("notes.txt"), true, window, cx));
    window_cx.run_until_parked();
    window_cx.simulate_keystrokes("cmd-k z");
    assert!(reopened.read_with(window_cx, |workbench, _| workbench.zen_mode.active));
    window_cx.simulate_keystrokes("escape escape");
    assert!(reopened.read_with(window_cx, |workbench, _| !workbench.zen_mode.active));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[gpui::test]
fn maximizing_the_secondary_side_bar_hides_the_other_parts_and_restore_brings_them_back(cx: &mut TestAppContext) {
    let stores = stores();
    let dir = std::env::temp_dir().join(format!("code-gpui-auxiliary-maximized-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (workbench, window_cx) = open(Some(dir.clone()), &stores, cx);
    window_cx.simulate_keystrokes("alt-cmd-b");
    let shown = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| [Part::SideBar, Part::Editor, Part::Panel, Part::AuxiliaryBar].map(|part| workbench.is_part_visible(part)));
    let width = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.part_grid.grid.as_ref().unwrap().part_bounds(Part::AuxiliaryBar).unwrap()[2]);
    assert_eq!(shown(window_cx), [true, true, false, true]);
    let before = width(window_cx);
    let run = |command: &str, cx: &mut VisualTestContext| workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(command, Vec::new(), window, cx)));

    run("workbench.action.toggleMaximizedAuxiliaryBar", window_cx);
    assert_eq!(shown(window_cx), [false, false, false, true]);
    let rest = workbench.read_with(window_cx, |workbench, _| {
        let grid = workbench.part_grid.grid.as_ref().unwrap();
        grid.dimensions()[0] - grid.part_bounds(Part::ActivityBar).unwrap()[2]
    });
    assert_eq!(width(window_cx), rest);
    let context = workbench.update_in(window_cx, |workbench, window, _| workbench.key_context(window));
    assert_eq!(context["auxiliaryBarMaximized"], true);

    // Showing another part leaves the maximized state first (layout.ts `setSideBarHidden`).
    window_cx.simulate_keystrokes("cmd-b");
    assert_eq!(shown(window_cx), [true, true, false, true]);
    assert!((width(window_cx) - before).abs() < 1., "{} {before}", width(window_cx));

    run("workbench.action.maximizeAuxiliaryBar", window_cx);
    run("workbench.action.closeAuxiliaryBar", window_cx);
    assert_eq!(shown(window_cx), [true, true, false, false]);
    assert!(!workbench.read_with(window_cx, |workbench, _| workbench.is_auxiliary_bar_maximized()));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// layout.ts `showEditorIfHidden`: an editor opened under a maximized secondary side bar or panel brings the editor
/// area back.
#[gpui::test]
fn opening_an_editor_restores_a_maximized_part(cx: &mut TestAppContext) {
    let stores = stores();
    stores.workspace.lock().unwrap().set("workbench.panelpart.activepanelid", super::super::output_view::OUTPUT_VIEW_ID.to_string());
    let dir = std::env::temp_dir().join(format!("code-gpui-maximized-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    std::fs::write(dir.join("b.txt"), "b").unwrap();
    let (workbench, window_cx) = open(Some(dir.clone()), &stores, cx);
    let run = |command: &str, cx: &mut VisualTestContext| workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(command, Vec::new(), window, cx)));
    let open_file = |name: &str, cx: &mut VisualTestContext| workbench.update_in(cx, |workbench, window, cx| workbench.open_file_in_active_group(dir.join(name), true, window, cx));

    open_file("a.txt", window_cx);
    run("workbench.action.toggleMaximizedAuxiliaryBar", window_cx);
    assert!(!workbench.read_with(window_cx, |workbench, _| workbench.is_part_visible(Part::Editor)));
    open_file("b.txt", window_cx);
    assert!(workbench.read_with(window_cx, |workbench, _| workbench.is_part_visible(Part::Editor) && !workbench.is_auxiliary_bar_maximized()));

    run("workbench.action.toggleMaximizedPanel", window_cx);
    assert!(workbench.read_with(window_cx, |workbench, _| workbench.is_panel_maximized()));
    open_file("a.txt", window_cx);
    assert!(workbench.read_with(window_cx, |workbench, _| workbench.is_part_visible(Part::Editor) && !workbench.is_panel_maximized()));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[gpui::test]
fn a_panel_view_dropped_on_the_secondary_side_bar_opens_there_and_stays_there(cx: &mut TestAppContext) {
    use super::super::output_view::OUTPUT_VIEW_ID;
    let stores = stores();
    stores.workspace.lock().unwrap().set("workbench.panelpart.activepanelid", OUTPUT_VIEW_ID.to_string());
    let (workbench, window_cx) = open(None, &stores, cx);
    workbench.update_in(window_cx, |workbench, window, cx| workbench.move_view_container(OUTPUT_VIEW_ID, Part::AuxiliaryBar, window, cx));
    window_cx.run_until_parked();
    assert!(workbench.read_with(window_cx, |workbench, _| workbench.is_part_visible(Part::AuxiliaryBar) && workbench.panel_view_visible(OUTPUT_VIEW_ID)));
    let tabs = |part: Part, cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.part_tabs(part).iter().map(|view| view.id).collect::<Vec<_>>());
    assert_eq!(tabs(Part::AuxiliaryBar, window_cx), [OUTPUT_VIEW_ID]);
    assert!(!tabs(Part::Panel, window_cx).contains(&OUTPUT_VIEW_ID));
    let stored = |stores: &Stores| serde_json::from_str::<serde_json::Value>(stores.profile.lock().unwrap().get("views.customizations").unwrap()).unwrap();
    assert_eq!(stored(&stores), serde_json::json!({ "viewContainerLocations": { OUTPUT_VIEW_ID: 2 }, "viewLocations": {}, "viewContainerBadgeEnablementStates": {} }));
    // viewPane.ts `shouldShowFilterInHeader`: the secondary side bar is narrower than 600px.
    assert!(!workbench.read_with(window_cx, |workbench, _| workbench.view_filter_in_header(OUTPUT_VIEW_ID)));

    workbench.update_in(window_cx, |workbench, window, cx| workbench.move_view_container(OUTPUT_VIEW_ID, Part::Panel, window, cx));
    window_cx.run_until_parked();
    assert!(workbench.read_with(window_cx, |workbench, _| workbench.is_part_visible(Part::Panel) && workbench.panel_view_visible(OUTPUT_VIEW_ID)));
    assert_eq!(stored(&stores), serde_json::json!({ "viewContainerLocations": {}, "viewLocations": {}, "viewContainerBadgeEnablementStates": {} }));
}

/// progressService.ts `showOnActivityBar`: the badge after the 300 ms delay, kept 300 ms once shown.
#[gpui::test]
fn progress_at_a_view_container_badges_its_activity_bar_item(cx: &mut TestAppContext) {
    use super::super::activity::Badge;
    use std::time::Duration;
    let stores = stores();
    let (workbench, window_cx) = open(None, &stores, cx);
    let badges = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.container_badges("workbench.view.explorer"));
    workbench.update_in(window_cx, |workbench, window, cx| drop(workbench.start_progress(&[serde_json::json!(7), serde_json::json!({ "location": 1 })], window, cx)));
    window_cx.executor().advance_clock(Duration::from_millis(299));
    assert!(!badges(window_cx).contains(&Badge::Progress));
    window_cx.executor().advance_clock(Duration::from_millis(1));
    assert!(badges(window_cx).contains(&Badge::Progress));
    workbench.update_in(window_cx, |workbench, window, cx| drop(workbench.end_progress(&[serde_json::json!(7)], window, cx)));
    window_cx.executor().advance_clock(Duration::from_millis(100));
    assert!(badges(window_cx).contains(&Badge::Progress));
    window_cx.executor().advance_clock(Duration::from_millis(200));
    assert!(!badges(window_cx).contains(&Badge::Progress));
}

/// layoutDensityActions.ts: Compact writes `window.density.layout`, and the rail narrows to 40px and the cards close up.
#[gpui::test]
fn the_compact_layout_density_narrows_the_rail_and_joins_the_cards(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("code-gpui-layout-density-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("User")).unwrap();
    let (workbench, window_cx) = open(Some(dir.clone()), &stores(), cx);
    let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None);
    workbench.update(window_cx, |workbench, _| *workbench.settings.lock().unwrap() = settings);
    let rail = |window_cx: &mut VisualTestContext| workbench.read_with(window_cx, |workbench, _| workbench.part_grid.grid.as_ref().unwrap().part_bounds(Part::ActivityBar).unwrap()[2]);
    assert_eq!(rail(window_cx), 48.);
    workbench.update_in(window_cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.setLayoutDensity.compact", Vec::new(), window, cx)));
    window_cx.run_until_parked();
    assert_eq!(workbench.read_with(window_cx, |workbench, _| workbench.setting("window.density.layout")), "compact");
    assert_eq!(rail(window_cx), 40.);
    assert_eq!(workbench.read_with(window_cx, |workbench, _| (workbench.status_bar_height(), workbench.pane_row_inset())), (26., 2.));
    workbench.update_in(window_cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.setLayoutDensity.default", Vec::new(), window, cx)));
    window_cx.run_until_parked();
    assert_eq!(rail(window_cx), 48.);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// layoutActions.ts `CustomizeLayoutAction`: the sections with their icons, eye buttons and checks; a pick runs its
/// command and the items follow; Restore Defaults puts the side bar left and the quick input on top; a second run
/// hides the picker.
#[gpui::test]
fn customize_layout_shows_the_layout_live_and_restores_the_defaults(cx: &mut TestAppContext) {
    use crate::main_thread_quick_open::Owner;
    use crate::quick_input::{Entry, Event};
    let dir = std::env::temp_dir().join(format!("code-gpui-customize-layout-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("User")).unwrap();
    let stores = stores();
    stores.workspace.lock().unwrap().set("workbench.panelpart.activepanelid", super::super::output_view::OUTPUT_VIEW_ID.to_string());
    let (workbench, window_cx) = open(Some(dir.clone()), &stores, cx);
    let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None);
    workbench.update(window_cx, |workbench, _| *workbench.settings.lock().unwrap() = settings);
    let run = |command: &'static str, cx: &mut VisualTestContext| {
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(command, Vec::new(), window, cx)));
        cx.run_until_parked();
    };
    let picker = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.quick_inputs.find_id(|owner| matches!(owner, Owner::CustomizeLayout(_))));
    let rows = |cx: &mut VisualTestContext| {
        workbench.read_with(cx, |workbench, _| {
            let id = workbench.quick_inputs.find_id(|owner| matches!(owner, Owner::CustomizeLayout(_))).unwrap();
            let input = workbench.quick_inputs.get(id).unwrap();
            let rows: Vec<String> = input
                .items()
                .iter()
                .map(|entry| match entry {
                    Entry::Separator(label) => format!("-{}", label.clone().unwrap_or_default()),
                    Entry::Item(item) => format!("{}{}", item.label, item.buttons.iter().map(|button| format!(" [{}]", button.codicon.clone().unwrap_or_default())).collect::<String>()),
                    Entry::Header(_) => unreachable!(),
                })
                .collect();
            (rows, input.active_handles())
        })
    };
    let event = |event: Event, cx: &mut VisualTestContext| {
        workbench.update_in(cx, |workbench, window, cx| {
            let id = workbench.quick_inputs.find_id(|owner| matches!(owner, Owner::CustomizeLayout(_))).unwrap();
            if let Event::Accept(_) = event {
                workbench.quick_inputs.get_mut(id).unwrap().set_selected_items(&[12]);
            }
            workbench.customize_layout_event(id, event, window, cx);
        });
        cx.run_until_parked();
    };

    run("workbench.action.customizeLayout", window_cx);
    let (shown, _) = rows(window_cx);
    assert_eq!(shown[..15], [
        "-Visibility",
        "$(layout-activitybar-left) Activity Bar [eye]",
        "$(layout-sidebar-left) Primary Side Bar [eye]",
        "$(layout-sidebar-right) Secondary Side Bar [eye-closed]",
        "$(layout-panel) Panel [eye-closed]",
        "$(layout-statusbar) Status Bar [eye]",
        "-Primary Side Bar Position",
        "$(layout-sidebar-left) Left $(check)",
        "$(layout-sidebar-right) Right",
        "-Panel Alignment",
        "$(layout-panel-left) Left",
        "$(layout-panel-right) Right",
        "$(layout-panel-center) Center $(check)",
        "$(layout-panel-justify) Justify",
        "-Quick Input Position",
    ]);
    assert_eq!(shown[15..17], ["$(arrow-up) Top $(check)", "$(circle) Center"]);
    assert_eq!(shown[shown.len() - 4..], ["-Modes", "$(screen-full) Full Screen", "$(target) Zen Mode", "$(layout-centered) Centered Layout"]);

    // The Panel row's eye button shows the panel; the row follows.
    event(Event::TriggerItemButton(3, 0, None), window_cx);
    assert!(workbench.read_with(window_cx, |workbench, _| workbench.is_part_visible(Part::Panel)));
    assert_eq!(rows(window_cx).0[4], "$(layout-panel) Panel [eye]");
    // Accepting Center moves the quick input to the center snap line and keeps that row active.
    event(Event::Accept(false), window_cx);
    let (shown, active) = rows(window_cx);
    assert_eq!((shown[15..17].to_vec(), active), (vec!["$(arrow-up) Top".to_owned(), "$(circle) Center $(check)".to_owned()], vec![12]));
    run("workbench.action.moveSideBarRight", window_cx);
    assert_eq!(rows(window_cx).0[1], "$(layout-activitybar-right) Activity Bar [eye]");

    event(Event::TriggerButton(0, None), window_cx);
    assert_eq!(workbench.read_with(window_cx, |workbench, _| workbench.setting("workbench.sideBar.location")), "left");
    assert_eq!(rows(window_cx).0[15..17], ["$(arrow-up) Top $(check)", "$(circle) Center"]);
    run("workbench.action.customizeLayout", window_cx);
    assert_eq!(picker(window_cx), None);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// sidebarPart.ts `toggleActivityBarVisibility`: hiding remembers the location, showing restores it.
#[gpui::test]
fn toggling_the_activity_bar_restores_its_location(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("code-gpui-activity-bar-toggle-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("User")).unwrap();
    let (workbench, window_cx) = open(Some(dir.clone()), &stores(), cx);
    let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None);
    workbench.update(window_cx, |workbench, _| *workbench.settings.lock().unwrap() = settings);
    let mut locations = Vec::new();
    for command in ["workbench.action.activityBarLocation.top", "workbench.action.toggleActivityBarVisibility", "workbench.action.toggleActivityBarVisibility"] {
        workbench.update_in(window_cx, |workbench, window, cx| drop(workbench.execute_command(command, Vec::new(), window, cx)));
        locations.push(workbench.read_with(window_cx, |workbench, _| workbench.setting("workbench.activityBar.location")));
    }
    assert_eq!(locations, ["top", "hidden", "top"]);
    std::fs::remove_dir_all(&dir).unwrap();
}
