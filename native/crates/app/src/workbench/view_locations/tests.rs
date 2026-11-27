use std::path::Path;
use std::sync::{Arc, Mutex};

use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};
use storage::Storage;

use super::*;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::problems_view::{MARKERS_CONTAINER_ID, MARKERS_VIEW_ID};

fn open(profile: Arc<Mutex<Storage>>, cx: &mut TestAppContext) -> (gpui::Entity<Workbench>, &mut VisualTestContext) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let theme = load_theme(&[scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap()], "Dark Modern").unwrap();
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root, None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.restore_profile_state(profile.clone(), profile);
        workbench.restore_workspace_state(Arc::new(Mutex::new(Storage::in_memory())));
        // An empty window starts without the side bar, where the Explorer's panes are laid out.
        workbench.set_part_hidden(Part::SideBar, false, cx);
        workbench
    });
    cx.run_until_parked();
    (workbench, cx)
}

fn stored(profile: &Arc<Mutex<Storage>>) -> Value {
    serde_json::from_str(profile.lock().unwrap().get(VIEWS_CUSTOMIZATIONS_KEY).unwrap()).unwrap()
}

fn explorer_panes(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<&'static str> {
    workbench.read_with(cx, |workbench, _| workbench.panes(PaneContainer::Explorer).view.as_ref().unwrap().ids().to_vec())
}

#[gpui::test]
fn outline_moved_to_a_new_panel_entry_gets_its_own_tab_and_comes_back_without_a_trace(cx: &mut TestAppContext) {
    let profile = Arc::new(Mutex::new(Storage::in_memory()));
    let (workbench, cx) = open(profile.clone(), cx);
    let container = workbench.update(cx, |workbench, cx| {
        let container = workbench.move_view_to_location(OUTLINE_VIEW, Part::Panel, cx);
        workbench.open_view_container_of(container, OUTLINE_VIEW, cx);
        container
    });
    cx.run_until_parked();
    assert!(container.starts_with("workbench.views.service.panel."));
    assert_eq!(stored(&profile), json!({ "viewContainerLocations": { container: 1 }, "viewLocations": { OUTLINE_VIEW: container }, "viewContainerBadgeEnablementStates": {} }));
    let tabs = workbench.read_with(cx, |workbench, _| workbench.part_tabs(Part::Panel).iter().map(|tab| (tab.id, tab.title)).collect::<Vec<_>>());
    assert_eq!(tabs.last(), Some(&(container, "Outline")));
    assert!(workbench.read_with(cx, |workbench, _| workbench.is_view_visible(OUTLINE_VIEW)));
    assert!(!explorer_panes(&workbench, cx).contains(&OUTLINE_VIEW));

    // The panel hides first: left without the generated container it would show the default Terminal, whose shell's
    // pty-read thread the test scheduler aborts on.
    workbench.update(cx, |workbench, cx| {
        workbench.hide_panel(cx);
        workbench.move_view_to_container(OUTLINE_VIEW, EXPLORER_CONTAINER, cx);
    });
    cx.run_until_parked();
    assert_eq!(stored(&profile), json!({ "viewContainerLocations": {}, "viewLocations": {}, "viewContainerBadgeEnablementStates": {} }));
    assert!(explorer_panes(&workbench, cx).contains(&OUTLINE_VIEW));
    assert!(!workbench.read_with(cx, |workbench, _| workbench.part_tabs(Part::Panel).iter().any(|tab| tab.id == container)));
}

#[gpui::test]
fn problems_moved_into_the_explorer_leaves_the_panel_and_opens_there(cx: &mut TestAppContext) {
    let profile = Arc::new(Mutex::new(Storage::in_memory()));
    let (workbench, cx) = open(profile.clone(), cx);
    workbench.update(cx, |workbench, cx| workbench.move_view_to_container(MARKERS_VIEW_ID, EXPLORER_CONTAINER, cx));
    workbench.update(cx, |workbench, cx| workbench.open_panel_view(MARKERS_CONTAINER_ID, cx));
    cx.run_until_parked();
    assert_eq!(stored(&profile)["viewLocations"], json!({ MARKERS_VIEW_ID: EXPLORER_CONTAINER }));
    assert!(!workbench.read_with(cx, |workbench, _| workbench.part_tabs(Part::Panel).iter().any(|tab| tab.id == MARKERS_CONTAINER_ID)));
    assert_eq!(explorer_panes(&workbench, cx).last(), Some(&MARKERS_VIEW_ID));
    assert!(workbench.read_with(cx, |workbench, _| workbench.side_bar_view == SideBarView::Explorer && workbench.panel_view_visible(MARKERS_CONTAINER_ID)));
}

#[gpui::test]
fn a_view_alone_in_a_new_side_bar_entry_is_not_offered_another_one(cx: &mut TestAppContext) {
    let profile = Arc::new(Mutex::new(Storage::in_memory()));
    let (workbench, cx) = open(profile, cx);
    let labels = workbench.update(cx, |workbench, cx| {
        let container = workbench.move_view_to_location(TIMELINE_VIEW, Part::SideBar, cx);
        let view = workbench.movable_view(TIMELINE_VIEW).unwrap();
        workbench.move_destinations(&view, container).into_iter().map(|(separator, label, _)| (separator, label)).collect::<Vec<_>>()
    });
    assert_eq!(labels[..3], [(None, "New Panel Entry".to_string()), (None, "New Secondary Side Bar Entry".to_string()), (Some("Side Bar"), "Explorer".to_string())]);
    assert_eq!(labels[3], (None, "Source Control".to_string()));
    assert!(labels.iter().all(|(_, label)| label != "Problems"));
}

#[gpui::test]
fn a_header_dropped_on_another_container_takes_the_drop_position(cx: &mut TestAppContext) {
    let profile = Arc::new(Mutex::new(Storage::in_memory()));
    let (workbench, cx) = open(profile, cx);
    workbench.update(cx, |workbench, cx| workbench.move_view_to_container(MARKERS_VIEW_ID, EXPLORER_CONTAINER, cx));
    cx.run_until_parked();
    assert_eq!(explorer_panes(&workbench, cx), [crate::workbench::view_panes::EMPTY_VIEW, OUTLINE_VIEW, TIMELINE_VIEW, MARKERS_VIEW_ID]);
    let container = workbench.update(cx, |workbench, cx| workbench.move_view_to_location(OUTLINE_VIEW, Part::SideBar, cx));
    cx.run_until_parked();
    workbench.update(cx, |workbench, cx| workbench.drop_pane_from_container(OUTLINE_VIEW, PaneContainer::Explorer, MARKERS_VIEW_ID, false, cx));
    cx.run_until_parked();
    assert_eq!(explorer_panes(&workbench, cx), [crate::workbench::view_panes::EMPTY_VIEW, TIMELINE_VIEW, OUTLINE_VIEW, MARKERS_VIEW_ID]);
    assert!(workbench.read_with(cx, |workbench, _| workbench.generated_containers().is_empty() && !workbench.generated_panes.contains_key(container)));
}
