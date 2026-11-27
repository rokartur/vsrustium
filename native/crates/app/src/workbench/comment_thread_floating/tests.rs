use gpui::{Modifiers, MouseButton, MouseDownEvent, TestAppContext, VisualTestContext, point, px};
use serde_json::{Value, json};

use super::*;
use crate::workbench::goto_symbol::tests::open;

fn floating(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Option<(Option<Layout>, f32, f32, f32)> {
    workbench.read_with(cx, |workbench, _| {
        workbench.comments.widgets.iter().find_map(|widget| match &widget.place {
            Place::Floating(floating) => Some((floating.layout, floating.top, floating.width, floating.height)),
            Place::Zone(_) => None,
        })
    })
}

fn set_style(workbench: &gpui::Entity<Workbench>, style: &str, cx: &mut VisualTestContext) {
    workbench.update(cx, |workbench, _| {
        workbench.settings.lock().unwrap().update_value_derived("comments.thread.experimental.style", Some(&json!(style))).unwrap();
        workbench.sync_comment_widgets();
    });
}

/// With the floating style an expanded thread gets a widget instead of a zone, centered on its code at the right;
/// a title drag makes the place the user's, which the thread gets back after a collapse, and the inline style
/// brings the zone back.
#[gpui::test]
fn a_floating_thread_is_placed_by_its_code_and_keeps_where_the_user_moved_it(cx: &mut TestAppContext) {
    let text: String = (1..=60).map(|line| format!("line {line}\n")).collect();
    let (workbench, cx, _, dir) = open(cx, "comment-thread-floating", &text);
    let resource = json!({ "scheme": "file", "path": dir.join("a.txt").to_string_lossy() });
    let request = |method: &str, args: Vec<Value>, cx: &mut VisualTestContext| {
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.handle_comments_request(method, &args, window, cx)));
    };
    set_style(&workbench, "floating", cx);
    request("$registerCommentController", vec![json!(1), json!("review"), json!("Review"), json!("test.ext")], cx);
    let range = json!({ "startLineNumber": 20, "startColumn": 1, "endLineNumber": 20, "endColumn": 8 });
    request("$createCommentThread", vec![json!(1), json!(7), json!("t"), resource.clone(), range, json!([]), json!({ "value": "test.ext" }), json!(false)], cx);
    let comment = json!({ "uniqueIdInThread": 1, "body": "Looks good", "userName": "Ann" });
    request("$updateCommentThread", vec![json!(1), json!(7), json!("t"), resource.clone(), json!({ "comments": [comment], "collapseState": 1 })], cx);
    cx.run_until_parked();

    let zones = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().view_zones.len());
    assert_eq!(zones, 0);
    let (layout, top, width, height) = floating(&workbench, cx).expect("a floating widget");
    let layout = layout.expect("placed at the first render");
    let (line_height, viewport_width) = workbench.read_with(cx, |workbench, _| (workbench.editors.viewport.line_height, workbench.editors.viewport.width));
    // Line 20's code is short, so the widget is centered on it after the lines next to it, and sized to its content.
    assert!(!layout.is_chosen_by_user && layout.offset_top.is_none(), "{layout:?}");
    assert_eq!(top, centered_top(19. * line_height, 20. * line_height, height));
    assert!(width >= 240. && layout.left + width <= viewport_width - VERTICAL_SCROLLBAR_WIDTH, "{layout:?} {width}");

    let key = (1, 7, workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().model_id));
    workbench.update_in(cx, |workbench, window, cx| {
        let down = MouseDownEvent { button: MouseButton::Left, position: point(px(500.), px(300.)), modifiers: Modifiers::default(), click_count: 1, first_mouse: false };
        workbench.start_floating_drag(key, &down, cx);
        workbench.move_floating_gesture(point(px(480.), px(340.)), true, window, cx);
    });
    cx.run_until_parked();
    let (moved, moved_top, ..) = floating(&workbench, cx).unwrap();
    let moved = moved.unwrap();
    assert!(moved.is_chosen_by_user);
    assert_eq!((moved.left, moved_top), (layout.left - 20., top + 40.));

    workbench.update(cx, |workbench, cx| workbench.toggle_comment_thread(1, 7, cx));
    assert!(floating(&workbench, cx).is_none());
    workbench.update(cx, |workbench, cx| workbench.toggle_comment_thread(1, 7, cx));
    cx.run_until_parked();
    assert_eq!(floating(&workbench, cx).unwrap().0, Some(moved));

    set_style(&workbench, "inline", cx);
    let zones = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().view_zones.len());
    assert_eq!((zones, floating(&workbench, cx).is_none()), (1, true));
}
