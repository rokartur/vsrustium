use std::path::Path;

use exthost::scan::scan_extension;
use gpui::{Modifiers, MouseButton, MouseDownEvent, TestAppContext, point, px};

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

#[gpui::test]
fn dragging_the_header_moves_the_quick_input_and_a_double_click_puts_it_back(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-drag-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
        workbench
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("shift-cmd-p");
    let bounds = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.quick_inputs.bounds.get());
    let start = bounds(cx);

    // The input box, here its top border, is not a drag area.
    let in_input = start.origin + point(start.size.width / 2., px(6.5));
    cx.simulate_mouse_down(in_input, MouseButton::Left, Modifiers::none());
    assert!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.drag.is_none()));
    cx.simulate_mouse_up(in_input, MouseButton::Left, Modifiers::none());

    // The header's 6px padding is.
    let grab = start.origin + point(px(3.), px(3.));
    cx.simulate_mouse_down(grab, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(grab + point(px(-100.), px(100.)), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(grab + point(px(-100.), px(100.)), MouseButton::Left, Modifiers::none());
    let moved = bounds(cx);
    assert_eq!(moved.origin - start.origin, point(px(-100.), px(100.)));
    assert!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.view_state.is_some()));

    // Released: moving the pointer leaves it in place.
    cx.simulate_mouse_move(grab, None, Modifiers::none());
    assert_eq!(bounds(cx).origin, moved.origin);

    let header = moved.origin + point(px(3.), px(3.));
    cx.simulate_event(MouseDownEvent { position: header, modifiers: Modifiers::none(), button: MouseButton::Left, click_count: 2, first_mouse: false });
    cx.simulate_mouse_up(header, MouseButton::Left, Modifiers::none());
    assert_eq!(bounds(cx).origin, start.origin);
    std::fs::remove_dir_all(dir).unwrap();
}
