use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use exthost::scan::scan_extension;
use gpui::{Bounds, Modifiers, MouseButton, TestAppContext, point, px, size};

use super::{HoverLayout, HoverOptions, HoverPosition, layout_hover};
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;
use storage::Storage;

/// The reference's window in port/results/raw/hover/ref.
const VIEWPORT: gpui::Size<f32> = gpui::Size { width: 1024., height: 768. };

fn layout(target: (f32, f32, f32, f32), hover: (f32, f32), options: HoverOptions) -> HoverLayout {
    let target = Bounds { origin: point(target.0, target.1), size: size(target.2, target.3) };
    layout_hover(VIEWPORT, target, size(hover.0, hover.1), &options, None)
}

fn assert_near(actual: gpui::Point<f32>, expected: (f32, f32), tolerance: f32) {
    assert!((actual.x - expected.0).abs() <= tolerance && (actual.y - expected.1).abs() <= tolerance, "{actual:?} is not near {expected:?}");
}

#[test]
fn hovers_sit_where_the_reference_puts_them() {
    let composite_bar = HoverOptions::composite_bar(HoverPosition::Right);
    // Activity bar "Search (⇧⌘F)" and "Manage": right of the item, the caret on the left.
    let search = layout((8., 83., 36., 36.), (99.5, 25.), composite_bar);
    assert_eq!((search.side, search.pointer), (HoverPosition::Right, Some(11.5)));
    assert_near(search.origin, (47., 87.5), 0.01);
    assert_near(layout((8., 696., 36., 36.), (62.9, 25.), composite_bar).origin, (47., 700.5), 0.01);
    // Title bar actions: no room above, so below with the caret on top, centered on the item.
    let back = layout((284.4375, 7.5, 22., 22.), (93.88, 25.), HoverOptions::instant());
    assert_eq!(back.side, HoverPosition::Below);
    assert_near(back.origin, (249.5, 32.5), 0.5);
    assert_near(layout((912., 7.5, 22., 22.), (129.73, 25.), HoverOptions::instant()).origin, (859., 32.5), 0.01);
    // The command center's hover target is inside its 1px border.
    assert_near(layout((343.4375, 7.5, 389.12, 22.), (266.58, 25.), HoverOptions::instant()).origin, (405.49, 32.5), 0.5);
    // Too wide to center on "Toggle Primary Side Bar": right-aligned 2px from the window edge, then 1px right.
    let toggle = layout((938., 7.5, 22., 22.), (184.9, 25.), HoverOptions::instant());
    assert_near(toggle.origin, (838., 32.5), 0.01);
    assert_near(layout((964., 7.5, 22., 22.), (120.34, 25.), HoverOptions::instant()).origin, (903., 32.5), 0.01);
    assert_near(layout((990., 7.5, 22., 22.), (213.68, 25.), HoverOptions::instant()).origin, (809., 32.5), 0.01);
    assert_eq!(toggle.pointer, Some(949. - toggle.origin.x));
    // Status bar "No Problems": above, the caret at the bottom.
    let problems = layout((40., 742., 73.25, 24.), (90.47, 25.), HoverOptions::status_bar());
    assert_eq!(problems.side, HoverPosition::Above);
    assert_near(problems.origin, (32.5, 714.), 0.5);
}

#[test]
fn mouse_hovers_open_below_the_target_10px_right_of_the_pointer() {
    let target = |x, y, w, h| Bounds { origin: point(x, y), size: size(w, h) };
    // Tab "notes.txt" and Explorer row "src".
    let tab = layout_hover(VIEWPORT, target(309., 36., 111.8, 31.), size(217.5, 25.), &HoverOptions::mouse(), Some(374.));
    assert_eq!((tab.origin, tab.pointer), (point(374., 67.), None));
    let row = layout_hover(VIEWPORT, target(53., 118., 246., 22.), size(185., 25.), &HoverOptions::mouse(), Some(126.));
    assert_eq!(row.origin, point(126., 140.));
}

/// hoverService.test.ts "should centre the hover pointer on its target for odd and even hover widths".
#[test]
fn the_pointer_is_centered_on_its_target_for_odd_and_even_widths() {
    let below = HoverOptions { position: HoverPosition::Below, ..HoverOptions::instant() };
    for width in [202., 203.] {
        let hover = layout((300., 100., 100., 20.), (width, 25.), below);
        assert_eq!(hover.origin.x + hover.pointer.unwrap(), 350.);
    }
}

/// hoverService.test.ts "should re-evaluate the requested side on every layout as the content size changes", with
/// its RIGHT and LEFT variants.
#[test]
fn the_requested_side_is_re_evaluated_for_each_hover_width() {
    let width = VIEWPORT.width;
    let side = |target: (f32, f32, f32, f32), client_width: f32, position| {
        let options = HoverOptions { position, ..HoverOptions::instant() };
        let hover = layout(target, (client_width + 2., 25.), options);
        (hover.side, hover.origin.x)
    };
    let right = (width - 300., 100., 50., 20.);
    assert_eq!(side(right, 100., HoverPosition::Right), (HoverPosition::Right, width - 250. + 3.));
    assert_eq!(side(right, 300., HoverPosition::Right), (HoverPosition::Left, width - 300. - 3. - 302.));
    assert_eq!(side(right, width, HoverPosition::Right).0, HoverPosition::Below);
    let left = (250., 100., 50., 20.);
    assert_eq!(side(left, 100., HoverPosition::Left), (HoverPosition::Left, 250. - 3. - 102.));
    assert_eq!(side(left, 300., HoverPosition::Left), (HoverPosition::Right, 300. + 3.));
    assert_eq!(side(left, width, HoverPosition::Left).0, HoverPosition::Below);
}

/// hoverService.test.ts "should centre the hover pointer vertically for odd hover heights": the caret's centre is
/// half the client height down the side.
#[test]
fn the_pointer_is_centered_vertically_for_odd_heights() {
    let right = HoverOptions { position: HoverPosition::Right, ..HoverOptions::instant() };
    let pointer = |client_height: f32| layout((300., 100., 100., 200.), (102., client_height + 2.), right).pointer.unwrap();
    assert_eq!((pointer(40.), pointer(41.)), (20., 20.5));
}

/// hoverService.test.ts "should re-evaluate a requested ABOVE side on every layout as the content size changes".
#[test]
fn a_requested_above_side_is_re_evaluated_for_each_height() {
    let above = HoverOptions { position: HoverPosition::Above, ..HoverOptions::instant() };
    let side = |height: f32| layout((100., 20., 100., 22.), (102., height + 2.), above).side;
    assert_eq!([side(10.), side(VIEWPORT.height), side(10.)], [HoverPosition::Above, HoverPosition::Below, HoverPosition::Above]);
}

/// hoverService.test.ts "should not choose a side using the maximum height applied by a previous layout" and "should
/// use the maximum height of the hover when choosing a vertical fallback": the side is chosen with the content's
/// height capped to `maxHeightRatio` of the window.
#[test]
fn the_vertical_fallback_uses_the_capped_height() {
    let no_pointer = |position| HoverOptions { position, follows_mouse: true, max_height_ratio: 0.5, ..HoverOptions::instant() };
    let side = |viewport: gpui::Size<f32>, top: f32, height: f32, position| {
        let target = Bounds { origin: point(100., top), size: size(100., 22.) };
        layout_hover(viewport, target, size(102., height), &no_pointer(position), None).side
    };
    assert_eq!(side(size(1024., 600.), 200., 800., HoverPosition::Above), HoverPosition::Below);
    assert_eq!(side(size(1024., 1200.), 400., 800., HoverPosition::Above), HoverPosition::Below);
    assert_eq!(side(VIEWPORT, 44., VIEWPORT.height, HoverPosition::Below), HoverPosition::Below);
}

/// hoverService.test.ts "should keep a side hover taller than the room above its target within the window".
#[test]
fn a_tall_side_hover_stays_inside_the_window() {
    let right = HoverOptions { position: HoverPosition::Right, ..HoverOptions::instant() };
    let hover = layout((0., 20., 100., 20.), (102., 302.), right);
    assert_eq!((hover.origin.y, hover.pointer), (2., Some(26.)));
}

fn open_workbench<'a>(name: &str, cx: &'a mut TestAppContext) -> (gpui::Entity<Workbench>, &'a mut gpui::VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
        let storage = Arc::new(Mutex::new(Storage::in_memory()));
        workbench.restore_profile_state(storage.clone(), storage);
        workbench
    });
    cx.run_until_parked();
    (workbench, cx)
}

fn wait(cx: &mut gpui::VisualTestContext, ms: u64) {
    cx.executor().advance_clock(Duration::from_millis(ms));
    cx.run_until_parked();
}

fn shown(workbench: &gpui::Entity<Workbench>, cx: &mut gpui::VisualTestContext) -> Option<String> {
    workbench.read_with(cx, |workbench, _| workbench.hovers.shown_text().map(str::to_owned))
}

/// hoverService.test.ts "should not show a pending hover after the target was clicked" and "should hide a hover when
/// the mouse left the target shortly before the delay ended", then hover locking: Alt keeps a hover the pointer
/// left (`_keyDown`), releasing Alt away from it hides it (`_keyUp`).
#[gpui::test]
fn clicks_cancel_pending_hovers_and_alt_locks_a_shown_one(cx: &mut TestAppContext) {
    let (workbench, cx) = open_workbench("hover-lock", cx);
    let width = cx.update(|window, _| window.viewport_size().width);
    let panel = point(width - px(49.), px(18.5));
    let outside = point(width / 2., px(300.));

    cx.simulate_mouse_move(panel, None, Modifiers::none());
    wait(cx, 1000);
    cx.simulate_mouse_down(panel, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(panel, MouseButton::Left, Modifiers::none());
    wait(cx, 1000);
    assert_eq!(shown(&workbench, cx), None, "the click cancelled the pending hover");

    cx.simulate_mouse_move(outside, None, Modifiers::none());
    wait(cx, 1000);
    cx.simulate_mouse_move(panel, None, Modifiers::none());
    wait(cx, 1400);
    cx.simulate_mouse_move(outside, None, Modifiers::none());
    wait(cx, 300);
    assert_eq!(shown(&workbench, cx), None, "leaving before the delay ended");

    cx.simulate_mouse_move(panel, None, Modifiers::none());
    wait(cx, 1500);
    assert!(shown(&workbench, cx).is_some());
    // Modifier changes reach the workbench through the focused element.
    workbench.update_in(cx, |workbench, window, cx| window.focus(&workbench.status_bar_focus, cx));
    cx.simulate_modifiers_change(Modifiers::alt());
    cx.simulate_mouse_move(outside, None, Modifiers::alt());
    wait(cx, 1000);
    assert!(shown(&workbench, cx).is_some(), "Alt locks the hover");
    cx.simulate_modifiers_change(Modifiers::none());
    cx.run_until_parked();
    assert_eq!(shown(&workbench, cx), None, "releasing Alt away from the hover hides it");
}

#[gpui::test]
fn a_hover_shows_after_the_delay_and_hides_when_the_pointer_leaves(cx: &mut TestAppContext) {
    let (workbench, cx) = open_workbench("hover", cx);
    let width = cx.update(|window, _| window.viewport_size().width);
    // Title bar right: 12px inset, 22px items 4px apart; Toggle Secondary Side Bar is last, Toggle Panel before it.
    let secondary = point(width - px(23.), px(18.5));
    let panel = point(width - px(49.), px(18.5));
    let outside = point(width / 2., px(300.));
    let shown = |cx: &mut gpui::VisualTestContext| shown(&workbench, cx);

    cx.simulate_mouse_move(secondary, None, Modifiers::none());
    wait(cx, 1499);
    assert_eq!(shown(cx), None);
    wait(cx, 1);
    assert_eq!(shown(cx).as_deref(), Some("Toggle Secondary Side Bar (⌥⌘B)"));

    // Instant hover: the neighbour within 200ms of the hide shows at once.
    cx.simulate_mouse_move(panel, None, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(shown(cx).as_deref(), Some("Toggle Panel (⌘J)"));

    // Leaving hides it once the mouse tracker's 200ms pass.
    cx.simulate_mouse_move(outside, None, Modifiers::none());
    wait(cx, 199);
    assert!(shown(cx).is_some());
    wait(cx, 1);
    assert_eq!(shown(cx), None);

    // Past the 200ms the delay applies again; a click hides the hover until the pointer leaves the target.
    wait(cx, 200);
    cx.simulate_mouse_move(panel, None, Modifiers::none());
    wait(cx, 1500);
    assert!(shown(cx).is_some());
    cx.simulate_mouse_down(panel, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(panel, MouseButton::Left, Modifiers::none());
    assert_eq!(shown(cx), None);
    cx.simulate_mouse_move(panel + point(px(1.), px(0.)), None, Modifiers::none());
    wait(cx, 1500);
    assert_eq!(shown(cx), None);

    // A key no keybinding takes hides it too.
    cx.simulate_mouse_move(secondary, None, Modifiers::none());
    wait(cx, 1500);
    assert!(shown(cx).is_some());
    cx.simulate_keystrokes("x");
    assert_eq!(shown(cx), None);
}

/// Chromium's fake mouse move: a target uncovered by a closing context menu gets its hover with the pointer still.
#[gpui::test]
fn a_target_under_a_closing_context_menu_gets_its_hover(cx: &mut TestAppContext) {
    let (workbench, cx) = open_workbench("hover-menu", cx);
    let width = cx.update(|window, _| window.viewport_size().width);
    let panel = point(width - px(49.), px(18.5));
    workbench.update_in(cx, |workbench, window, cx| {
        let actions = vec![crate::menus::MenuAction::Command { id: "workbench.action.closeAllEditors".into(), title: "Close All".into(), enabled: true, checked: false, alt: None }];
        workbench.show_context_menu_actions(actions, &Default::default(), Bounds::new(point(px(400.), px(300.)), size(px(0.), px(0.))), Vec::new(), window, cx);
    });
    cx.simulate_mouse_move(panel, None, Modifiers::none());
    wait(cx, 1500);
    assert_eq!(shown(&workbench, cx), None, "the open menu's backdrop takes the pointer");
    workbench.update_in(cx, |workbench, window, cx| workbench.hide_context_menu(window, cx));
    // Two frames as the platform loop delivers them: next-frame callbacks, then the draw.
    for _ in 0..2 {
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            drop(window.draw(cx));
        });
    }
    wait(cx, 1500);
    assert_eq!(shown(&workbench, cx).as_deref(), Some("Toggle Panel (⌘J)"));
}

/// A `---` paragraph is a 1px rule, 8px below the paragraph before it and 4px above the one after it.
#[test]
fn a_rule_paragraph_collapses_its_margins() {
    use crate::notifications::Node;
    let text = || vec![Node::Text("a".into())];
    assert_eq!(super::paragraphs_height(&[text(), text()]), 19. + 8. + 19.);
    assert_eq!(super::paragraphs_height(&[text(), Vec::new(), text()]), 19. + 8. + 1. + 4. + 19.);
}
