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

#[gpui::test]
fn a_hover_shows_after_the_delay_and_hides_when_the_pointer_leaves(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-hover-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
        workbench.restore_profile_state(Arc::new(Mutex::new(Storage::in_memory())));
        workbench
    });
    cx.run_until_parked();
    let width = cx.update(|window, _| window.viewport_size().width);
    // Title bar right: 12px inset, 22px items 4px apart; Toggle Secondary Side Bar is last, Toggle Panel before it.
    let secondary = point(width - px(23.), px(18.5));
    let panel = point(width - px(49.), px(18.5));
    let outside = point(width / 2., px(300.));
    let shown = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.hovers.shown_text().map(str::to_owned));
    let wait = |cx: &mut gpui::VisualTestContext, ms| {
        cx.executor().advance_clock(Duration::from_millis(ms));
        cx.run_until_parked();
    };

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
