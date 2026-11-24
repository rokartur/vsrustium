use gpui::{Bounds, point, px, size};

use super::{LayoutAnchor, LayoutAnchorMode, LayoutAnchorPosition, focus_next, focus_previous, layout, layout2d};
use crate::menus::MenuAction;

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
    assert_eq!(layout2d(viewport, menu, pointer(93., 151.)), point(px(93.), px(153.)));
    // No room below: above the pointer, still aligned with it.
    assert_eq!(layout2d(viewport, menu, pointer(93., 700.)), point(px(93.), px(347.)));
    // No room right: left of the pointer.
    assert_eq!(layout2d(viewport, menu, pointer(900., 100.)), point(px(620.), px(102.)));
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
