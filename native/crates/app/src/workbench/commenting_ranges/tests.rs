use gpui::TestAppContext;
use serde_json::json;

use crate::workbench::goto_symbol::tests::{answer, open, run};

/// commentsController.ts: a controller with a commenting range on lines 2-3 is asked for the active model; a press
/// and release on the line 3 glyph asks for a template thread there, which opens with its reply form; Escape on
/// the empty template deletes it.
#[gpui::test]
fn a_commenting_range_click_creates_a_template_thread(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, dir) = open(cx, "commenting-ranges", "one\ntwo\nthree\n");
    let resource = json!({ "scheme": "file", "path": dir.join("a.txt").to_string_lossy() });
    let request = |method: &str, args: Vec<serde_json::Value>, cx: &mut gpui::VisualTestContext| {
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.handle_comments_request(method, &args, window, cx)));
    };
    request("$registerCommentController", vec![json!(1), json!("review"), json!("Review"), json!("test.ext")], cx);
    request("$updateCommentingRanges", vec![json!(1)], cx);
    cx.run_until_parked();
    let range = json!({ "startLineNumber": 2, "startColumn": 1, "endLineNumber": 3, "endColumn": 6 });
    answer(&receiver, "$provideCommentingRanges", json!({ "ranges": [range], "fileComments": false }), cx);
    let in_range = |line: usize, cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.line_has_commenting_range(workbench.editors.active().unwrap(), line));
    assert_eq!([in_range(0, cx), in_range(1, cx), in_range(2, cx)], [false, true, true]);

    workbench.update_in(cx, |workbench, window, cx| {
        workbench.commenting_glyph_mouse_down(2);
        workbench.commenting_glyph_mouse_up(2, gpui::Point::default(), window, cx);
    });
    let calls: Vec<_> = receiver.try_iter().filter(|message| message.method == "$createCommentThreadTemplate").collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].args[2], json!({ "startLineNumber": 3, "startColumn": 1, "endLineNumber": 3, "endColumn": 1 }));

    let line3 = json!({ "startLineNumber": 3, "startColumn": 1, "endLineNumber": 3, "endColumn": 1 });
    request("$createCommentThread", vec![json!(1), json!(9), json!("t"), resource.clone(), line3, json!([]), json!({ "value": "test.ext" }), json!(true)], cx);
    request("$updateCommentThread", vec![json!(1), json!(9), json!("t"), resource, json!({ "collapseState": 1 })], cx);
    let replying = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.comments.reply.as_ref().map(|reply| reply.thread));
    assert_eq!(replying(cx), Some(9), "the template opens its reply form");

    // The reference keeps a new template open on Escape (port/notes.md, milestone 20 comment Escape).
    cx.simulate_keystrokes("escape");
    assert_eq!(replying(cx), Some(9));
    assert!(receiver.try_iter().all(|message| message.method != "$deleteCommentThread"));

    run(&workbench, cx, "cursorTop");
    run(&workbench, cx, "workbench.action.addComment");
    assert!(receiver.try_iter().all(|message| message.method != "$createCommentThreadTemplate"), "line 1 has no commenting range");
}
