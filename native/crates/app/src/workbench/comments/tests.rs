use gpui::TestAppContext;
use serde_json::json;

use crate::workbench::goto_symbol::tests::open;

/// mainThreadComments.ts: a controller's threads come and change through the host requests; the view registers
/// with the first thread, and revealing one opens its file at its range.
#[gpui::test]
fn threads_follow_the_host_and_reveal_their_range(cx: &mut TestAppContext) {
    let (workbench, cx, _, dir) = open(cx, "comments", "one\ntwo\nthree\n");
    let resource = json!({ "scheme": "file", "path": dir.join("a.txt").to_string_lossy() });
    let request = |method: &str, args: Vec<serde_json::Value>, cx: &mut gpui::VisualTestContext| {
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.handle_comments_request(method, &args, window, cx)));
    };
    request("$registerCommentController", vec![json!(1), json!("review"), json!("Review"), json!("test.ext")], cx);
    assert!(!workbench.read_with(cx, |workbench, _| workbench.comments_view_registered()));
    let range = json!({ "startLineNumber": 3, "startColumn": 2, "endLineNumber": 3, "endColumn": 4 });
    request("$createCommentThread", vec![json!(1), json!(7), json!("t"), resource.clone(), range, json!([]), json!({ "value": "test.ext" }), json!(false)], cx);
    assert!(workbench.read_with(cx, |workbench, _| workbench.comments_view_registered()));
    let comment = |id: u64, name: &str| json!({ "uniqueIdInThread": id, "body": "Looks good", "userName": name });
    request("$updateCommentThread", vec![json!(1), json!(7), json!("t"), resource.clone(), json!({ "comments": [comment(1, "Ann"), comment(2, "Bob")], "state": 0, "label": "Review" })], cx);
    workbench.read_with(cx, |workbench, _| {
        let thread = &workbench.comments.controllers[0].threads[0];
        assert_eq!((thread.comments.len(), thread.state, thread.label.as_deref(), thread.can_reply.clone()), (2, Some(0), Some("Review"), json!(true)));
    });
    request("$revealCommentThread", vec![json!(1), json!(7), json!(1), json!({})], cx);
    let head = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().selection().head);
    assert_eq!(head, crate::editor::Position::new(2, 1));
    request("$deleteCommentThread", vec![json!(1), json!(7)], cx);
    request("$unregisterCommentController", vec![json!(1)], cx);
    assert!(workbench.read_with(cx, |workbench, _| workbench.comments.controllers.is_empty()));
}
