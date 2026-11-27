use gpui::{Bounds, point, px, size};

use std::path::Path;
use std::time::Duration;

use exthost::scan::scan_extension;
use gpui::TestAppContext;

use super::{LayoutAnchor, LayoutAnchorMode, LayoutAnchorPosition, focus_next, focus_previous, layout, layout2d, layout2d_beside, submenu_layout};
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::menus::MenuAction;
use crate::theme::load_theme;
use crate::workbench::Workbench;

fn at(viewport: f32, offset: f32, size: f32, position: LayoutAnchorPosition) -> f32 {
    layout(viewport, 20., &LayoutAnchor { offset, size, mode: LayoutAnchorMode::Avoid, position })
}

/// layout.test.ts `layout`.
#[test]
fn layout_places_the_view_next_to_the_anchor() {
    use LayoutAnchorPosition::{After, Before};
    assert_eq!(at(200., 0., 0., Before), 0.);
    assert_eq!(at(200., 50., 0., Before), 50.);
    assert_eq!(at(200., 200., 0., Before), 180.);
    assert_eq!(at(200., 0., 0., After), 0.);
    assert_eq!(at(200., 50., 0., After), 30.);
    assert_eq!(at(200., 200., 0., After), 180.);
    assert_eq!(at(200., 0., 50., Before), 50.);
    assert_eq!(at(200., 50., 50., Before), 100.);
    assert_eq!(at(200., 150., 50., Before), 130.);
    assert_eq!(at(200., 0., 50., After), 50.);
    assert_eq!(at(200., 50., 50., After), 30.);
    assert_eq!(at(200., 150., 50., After), 130.);
}

#[test]
fn layout2d_opens_below_right_of_the_pointer_and_flips_at_the_edges() {
    let viewport = size(1000., 800.);
    let menu = size(282., 353.);
    let pointer = |x: f32, y: f32| Bounds { origin: point(x, y), size: size(2., 2.) };
    // The reference's menu for a click at (93, 151) sits at (93, 153).
    assert_eq!(layout2d(viewport, menu, pointer(93., 151.), LayoutAnchorPosition::Before), point(px(93.), px(153.)));
    // No room below: above the pointer, still aligned with it.
    assert_eq!(layout2d(viewport, menu, pointer(93., 700.), LayoutAnchorPosition::Before), point(px(93.), px(347.)));
    // No room right: left of the pointer.
    assert_eq!(layout2d(viewport, menu, pointer(900., 100.), LayoutAnchorPosition::Before), point(px(620.), px(102.)));
}

#[test]
fn keyboard_focus_skips_separators_and_disabled_items_and_loops() {
    let item = |enabled| MenuAction::Command { id: String::new(), title: String::new(), enabled, checked: false, alt: None };
    let actions = [item(true), MenuAction::Separator, item(false), item(true)];
    assert_eq!(focus_next(&actions, 0), Some(3));
    assert_eq!(focus_next(&actions, 3), Some(0));
    assert_eq!(focus_previous(&actions, 3), Some(0));
    assert_eq!(focus_previous(&actions, 0), Some(3));
    assert_eq!(focus_next(&[item(false)], 0), None);
}

/// globalCompositeBar.ts: the Manage menu opens right of the button, top aligned, or bottom aligned near the edge.
#[test]
fn the_manage_menu_opens_beside_its_button() {
    let viewport = size(1000., 800.);
    let menu = size(200., 100.);
    let button = |y: f32| Bounds { origin: point(4., y), size: size(36., 36.) };
    assert_eq!(layout2d_beside(viewport, menu, button(700.)), point(px(40.), px(700.)));
    assert_eq!(layout2d_beside(viewport, menu, button(720.)), point(px(40.), px(656.)));
}

/// menu.ts `calculateSubmenuMenuLayout`: right of the entry from its top, left of the menu without room on the right,
/// over the menu 10px in without room on either side, and up from the entry's bottom without room below.
#[test]
fn submenus_open_beside_their_item() {
    let viewport = size(1000., 800.);
    let submenu = size(200., 100.);
    let entry = |x: f32, y: f32| Bounds { origin: point(x, y), size: size(300., 32.) };
    assert_eq!(submenu_layout(viewport, submenu, entry(100., 50.)), point(px(400.), px(50.)));
    assert_eq!(submenu_layout(viewport, submenu, entry(600., 50.)), point(px(400.), px(50.)));
    assert_eq!(submenu_layout(size(500., 800.), submenu, entry(100., 50.)), point(px(110.), px(60.)));
    assert_eq!(submenu_layout(viewport, submenu, entry(100., 750.)), point(px(400.), px(682.)));
}

/// menu.ts `SubmenuMenuActionViewItem`: hover opens after 250ms, another item closes it after 750ms, Right and
/// Enter open it with its first item focused, Left and Escape close only it, its items run their command.
#[gpui::test]
fn submenus_open_on_hover_and_keys_and_run_their_items(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx));
    let runs = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let command = |id: &str| MenuAction::Command { id: id.into(), title: id.into(), enabled: true, checked: false, alt: None };
    let actions = vec![command("port.top"), MenuAction::Submenu { title: "More".into(), actions: vec![command("port.first"), command("port.second")], checked: false }];
    workbench.update_in(cx, |workbench, window, cx| {
        for id in ["port.top", "port.first", "port.second"] {
            let runs = runs.clone();
            workbench.commands.register(id, move |_, args, _, _| {
                runs.borrow_mut().push((id, args));
                crate::commands::Outcome::done()
            });
        }
        let anchor = Bounds { origin: point(px(100.), px(100.)), size: size(px(0.), px(0.)) };
        workbench.show_context_menu_actions(actions.clone(), &Default::default(), anchor, vec![serde_json::json!("arg")], window, cx);
    });
    cx.run_until_parked();
    let levels = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.context_menu.as_ref().map(|menu| menu.levels.iter().map(|level| level.focused).collect::<Vec<_>>()));
    let (origin, width) = workbench.read_with(cx, |workbench, _| {
        let root = &workbench.context_menu.as_ref().unwrap().levels[0];
        (root.origin, root.width)
    });
    let row = |index: f32| origin + point(px(20.), px(1. + 4. + 24. * index + 12.));

    cx.simulate_mouse_move(row(1.), None, gpui::Modifiers::none());
    cx.executor().advance_clock(Duration::from_millis(249));
    cx.run_until_parked();
    assert_eq!(levels(cx), Some(vec![Some(1)]), "the submenu waits 250ms");
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(levels(cx), Some(vec![Some(1), None]), "the hovered submenu opens without a focused item");
    let submenu = workbench.read_with(cx, |workbench, _| workbench.context_menu.as_ref().unwrap().levels[1].origin);
    assert_eq!(submenu, origin + point(px(width - 1.), px(25.)), "right of the menu, its first item level with the submenu item");

    cx.simulate_mouse_move(row(0.), None, gpui::Modifiers::none());
    cx.executor().advance_clock(Duration::from_millis(749));
    cx.run_until_parked();
    assert_eq!(levels(cx), Some(vec![Some(0), None]));
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(levels(cx), Some(vec![Some(0)]), "another item closes the submenu after 750ms");

    cx.simulate_keystrokes("down right");
    assert_eq!(levels(cx), Some(vec![Some(1), Some(0)]), "Right opens it with its first item");
    cx.simulate_keystrokes("left");
    assert_eq!(levels(cx), Some(vec![Some(1)]));
    cx.simulate_keystrokes("enter down");
    assert_eq!(levels(cx), Some(vec![Some(1), Some(1)]));
    cx.simulate_keystrokes("escape");
    assert_eq!(levels(cx), Some(vec![Some(1)]), "Escape closes only the submenu");
    cx.simulate_keystrokes("right down enter");
    assert_eq!(levels(cx), None);
    assert_eq!(*runs.borrow(), [("port.second", vec![serde_json::json!("arg")])]);
}
