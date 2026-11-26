use serde_json::json;

use super::*;

fn thread(handle: u64, line: u64, column: u64) -> (u64, u64, Option<Value>) {
    (1, handle, Some(json!({ "startLineNumber": line, "startColumn": column, "endLineNumber": line, "endColumn": column + 1 })))
}

#[test]
fn the_nearest_thread_starts_after_the_cursor_or_before_it_in_reverse() {
    let threads = vec![thread(3, 9, 1), thread(1, 2, 1), thread(2, 2, 5), (1, 4, None)];
    let handle = |found: Option<(u64, u64, Option<Value>)>| found.map(|(_, handle, _)| handle);
    assert_eq!(handle(nearest_thread(threads.clone(), Position::new(1, 0), false)), Some(2));
    assert_eq!(handle(nearest_thread(threads.clone(), Position::new(1, 4), false)), Some(3));
    assert_eq!(handle(nearest_thread(threads.clone(), Position::new(8, 0), false)), None);
    assert_eq!(handle(nearest_thread(threads.clone(), Position::new(1, 4), true)), Some(1));
    assert_eq!(handle(nearest_thread(threads, Position::new(0, 0), true)), Some(4));
}

#[test]
fn the_nearest_commenting_range_skips_the_one_holding_the_cursor_and_its_neighbors() {
    let ranges = [(0, 1), (2, 3), (6, 7), (9, 9)];
    assert_eq!(nearest_commenting_range(&ranges, 0, false), Some((6, 7)));
    assert_eq!(nearest_commenting_range(&ranges, 4, false), Some((6, 7)));
    assert_eq!(nearest_commenting_range(&ranges, 9, false), Some((0, 1)));
    assert_eq!(nearest_commenting_range(&ranges, 7, true), Some((2, 3)));
    assert_eq!(nearest_commenting_range(&ranges, 0, true), Some((9, 9)));
    assert_eq!(nearest_commenting_range(&[], 0, false), None);
}

/// Dismiss Comment in a reply collapses its thread and selects the thread's range; Expand All opens it again.
#[gpui::test]
fn dismissing_a_reply_collapses_its_thread(cx: &mut gpui::TestAppContext) {
    let (workbench, cx, _, dir) = crate::workbench::goto_symbol::tests::open(cx, "comment-dismiss", "one\ntwo\nthree\n");
    let resource = json!({ "scheme": "file", "path": dir.join("a.txt").to_string_lossy() });
    let range = json!({ "startLineNumber": 2, "startColumn": 1, "endLineNumber": 2, "endColumn": 4 });
    let comment = json!({ "uniqueIdInThread": 1, "body": "Looks good", "userName": "Ann" });
    let state = |cx: &mut gpui::VisualTestContext| {
        workbench.read_with(cx, |workbench, _| {
            let thread = &workbench.comments.controllers[0].threads[0];
            (thread.collapse_state, workbench.comments.reply.is_some(), workbench.editors.active().unwrap().selection().start())
        })
    };
    workbench.update_in(cx, |workbench, window, cx| {
        let request = |workbench: &mut Workbench, method: &str, args: Vec<Value>, window: &mut Window, cx: &mut Context<Workbench>| drop(workbench.handle_comments_request(method, &args, window, cx));
        request(workbench, "$registerCommentController", vec![json!(1), json!("review"), json!("Review"), json!("test.ext")], window, cx);
        request(workbench, "$createCommentThread", vec![json!(1), json!(7), json!("t"), resource.clone(), range, json!([comment]), json!({ "value": "test.ext" }), json!(false)], window, cx);
        drop(workbench.execute_command("workbench.action.expandAllComments", Vec::new(), window, cx));
        workbench.expand_comment_reply(1, 7, window, cx);
    });
    assert_eq!(state(cx), (EXPANDED, true, Position::new(0, 0)));
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.hideComment", Vec::new(), window, cx)));
    assert_eq!(state(cx), (COLLAPSED, false, Position::new(1, 0)));

    let zones = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.comments.widgets.len());
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.focusCommentOnCurrentLine", Vec::new(), window, cx)));
    assert_eq!((state(cx).0, zones(cx)), (EXPANDED, 1));
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.toggleCommenting", Vec::new(), window, cx)));
    assert_eq!(zones(cx), 0);
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.toggleCommenting", Vec::new(), window, cx)));
    assert_eq!(zones(cx), 1);
}

/// A new empty template shows through `shouldReveal`; after the host's Expanded echo, Dismiss Comment changes nothing:
/// the zone and the focused reply stay, as in the reference. Collapse All closes it.
#[gpui::test]
fn dismissing_a_new_template_keeps_it_open(cx: &mut gpui::TestAppContext) {
    let (workbench, cx, _, dir) = crate::workbench::goto_symbol::tests::open(cx, "comment-template", "one\ntwo\nthree\n");
    let resource = json!({ "scheme": "file", "path": dir.join("a.txt").to_string_lossy() });
    let range = json!({ "startLineNumber": 3, "startColumn": 1, "endLineNumber": 3, "endColumn": 1 });
    let state = |cx: &mut gpui::VisualTestContext| {
        workbench.read_with(cx, |workbench, _| {
            let thread = &workbench.comments.controllers[0].threads[0];
            (thread.collapse_state, workbench.comments.widgets.len(), workbench.comments.reply.is_some())
        })
    };
    workbench.update_in(cx, |workbench, window, cx| {
        let request = |workbench: &mut Workbench, method: &str, args: Vec<Value>, window: &mut Window, cx: &mut Context<Workbench>| drop(workbench.handle_comments_request(method, &args, window, cx));
        request(workbench, "$registerCommentController", vec![json!(1), json!("review"), json!("Review"), json!("test.ext")], window, cx);
        request(workbench, "$createCommentThread", vec![json!(1), json!(7), json!("t"), resource.clone(), range, json!([]), json!({ "value": "test.ext" }), json!(true)], window, cx);
    });
    assert_eq!(state(cx), (COLLAPSED, 1, true));
    workbench.update_in(cx, |workbench, window, cx| {
        drop(workbench.handle_comments_request("$updateCommentThread", &[json!(1), json!(7), json!("t"), resource.clone(), json!({ "collapseState": 1, "isTemplate": false })], window, cx));
        drop(workbench.execute_command("workbench.action.hideComment", Vec::new(), window, cx));
    });
    assert_eq!(state(cx), (EXPANDED, 1, true));
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.collapseAllComments", Vec::new(), window, cx)));
    let (collapse_state, zones, _) = state(cx);
    assert_eq!((collapse_state, zones), (COLLAPSED, 0));
}
