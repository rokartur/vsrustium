use std::sync::{Arc, Mutex};

use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use super::*;
use crate::menus::{Entry, MenuItem};
use crate::workbench::goto_symbol::tests::open;

/// An expanded thread gets a zone below its range, the glyph toggle removes it, and the reply form grows it; a
/// reply button runs its command with the `CommentReply` and hides the form.
#[gpui::test]
fn an_expanded_thread_shows_a_zone_and_replies_run_the_form_actions(cx: &mut TestAppContext) {
    let (workbench, cx, _, dir) = open(cx, "comment-thread-widget", "one\ntwo\nthree\n");
    let resource = json!({ "scheme": "file", "path": dir.join("a.txt").to_string_lossy() });
    let request = |method: &str, args: Vec<Value>, cx: &mut VisualTestContext| {
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.handle_comments_request(method, &args, window, cx)));
    };
    request("$registerCommentController", vec![json!(1), json!("review"), json!("Review"), json!("test.ext")], cx);
    let range = json!({ "startLineNumber": 2, "startColumn": 1, "endLineNumber": 2, "endColumn": 4 });
    request("$createCommentThread", vec![json!(1), json!(7), json!("t"), resource.clone(), range, json!([]), json!({ "value": "test.ext" }), json!(false)], cx);
    let zones = |workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext| {
        workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().view_zones.iter().map(|zone| (zone.anchor, zone.height)).collect::<Vec<_>>())
    };
    assert!(zones(&workbench, cx).is_empty());
    let comment = json!({ "uniqueIdInThread": 1, "body": "Looks good", "userName": "Ann" });
    request("$updateCommentThread", vec![json!(1), json!(7), json!("t"), resource.clone(), json!({ "comments": [comment], "collapseState": 1 })], cx);
    let shown = zones(&workbench, cx);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].0, Some(Position::new(1, 2)));

    let ran = Arc::new(Mutex::new(Vec::new()));
    workbench.update_in(cx, |workbench, window, cx| {
        let record = ran.clone();
        workbench.commands.register("test.reply", move |_, args, _, _| {
            record.lock().unwrap().extend(args);
            crate::commands::Outcome::done()
        });
        let entry = Entry::Command { id: "test.reply".into(), precondition: None, toggled: None, alt: None };
        workbench.menus.push(MenuItem { menu: MenuId::CommentThreadActions, entry, title: "Reply".into(), group: "inline".into(), order: 0., when: None });
        workbench.expand_comment_reply(1, 7, window, cx);
        workbench.edit_comment_reply_input(|input| input.insert(None, "Agreed"), cx);
    });
    assert!(zones(&workbench, cx)[0].1 > shown[0].1);
    workbench.update_in(cx, |workbench, window, cx| workbench.submit_comment_reply(window, cx));
    cx.run_until_parked();
    let thread = json!({ "$mid": 7, "commentControlHandle": 1, "commentThreadHandle": 7 });
    assert_eq!(*ran.lock().unwrap(), vec![json!({ "thread": thread, "text": "Agreed", "$mid": 9 })]);
    assert_eq!(zones(&workbench, cx)[0].1, shown[0].1);

    workbench.update_in(cx, |workbench, _, cx| workbench.toggle_comment_thread(1, 7, cx));
    assert!(zones(&workbench, cx).is_empty());
}

#[test]
fn the_height_counts_the_head_comments_and_form_in_whole_lines() {
    let comment = |body: Value| json!({ "body": body, "userName": "Ann" });
    let thread = |comments: Vec<Value>| Thread {
        handle: 1,
        resource: Value::Null,
        range: None,
        label: None,
        context_value: None,
        comments,
        collapse_state: 1,
        can_reply: json!(true),
        state: None,
        is_template: false,
    };
    // 22 head + 1 + (42 + 18) + 48 form + 6 arrow + 4 frame + 8 = 149, 9 lines of 18.
    assert_eq!(widget_height(&thread(vec![comment(json!("one"))]), None, false, 18.), 162.);
    // A markdown body of two paragraphs counts two lines: 167, 10 lines.
    assert_eq!(widget_height(&thread(vec![comment(json!({ "value": "one\n\ntwo" }))]), None, false, 18.), 180.);
}
