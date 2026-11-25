use std::sync::{Arc, Mutex};

use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use super::*;
use crate::menus::{Entry, MenuItem};
use crate::workbench::goto_symbol::tests::open;

/// A comment the extension switches to Editing opens its edit form with its text; Cmd+Enter runs the first
/// `comments/comment/context` action with the `CommentThreadNode`; switching back to Preview closes the form.
#[gpui::test]
fn a_comment_switched_to_editing_runs_the_edit_actions(cx: &mut TestAppContext) {
    let (workbench, cx, _, dir) = open(cx, "comment-node", "one\ntwo\nthree\n");
    let resource = json!({ "scheme": "file", "path": dir.join("a.txt").to_string_lossy() });
    let request = |method: &str, args: Vec<Value>, cx: &mut VisualTestContext| {
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.handle_comments_request(method, &args, window, cx)));
    };
    let comment = |mode: u64| json!({ "uniqueIdInThread": 3, "body": "Looks good", "userName": "Ann", "mode": mode });
    let update = |mode: u64, cx: &mut VisualTestContext| {
        request("$updateCommentThread", vec![json!(1), json!(7), json!("t"), resource.clone(), json!({ "comments": [comment(mode)], "collapseState": 1 })], cx);
    };
    request("$registerCommentController", vec![json!(1), json!("review"), json!("Review"), json!("test.ext")], cx);
    let range = json!({ "startLineNumber": 2, "startColumn": 1, "endLineNumber": 2, "endColumn": 4 });
    request("$createCommentThread", vec![json!(1), json!(7), json!("t"), resource.clone(), range, json!([comment(1)]), json!({ "value": "test.ext" }), json!(false)], cx);
    update(1, cx);
    let editing = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.comments.reply.as_ref().map(|reply| (reply.comment, reply.input.value.clone())));
    assert_eq!(editing(cx), None);

    let ran = Arc::new(Mutex::new(Vec::new()));
    workbench.update_in(cx, |workbench, _, _| {
        let record = ran.clone();
        workbench.commands.register("test.save", move |_, args, _, _| {
            record.lock().unwrap().extend(args);
            crate::commands::Outcome::done()
        });
        let entry = Entry::Command { id: "test.save".into(), precondition: None, toggled: None, alt: None };
        workbench.menus.push(MenuItem { menu: MenuId::CommentActions, entry, title: "Save".into(), group: "inline".into(), order: 0., when: None });
    });
    update(EDITING, cx);
    assert_eq!(editing(cx), Some((Some(3), "Looks good".to_owned())));
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.edit_comment_reply_input(|input| input.insert(None, "!"), cx);
        workbench.submit_comment_edit(window, cx);
    });
    cx.run_until_parked();
    let thread = json!({ "$mid": 7, "commentControlHandle": 1, "commentThreadHandle": 7 });
    assert_eq!(*ran.lock().unwrap(), vec![json!({ "thread": thread, "commentUniqueId": 3, "text": "Looks good!", "$mid": 11 })]);
    assert_eq!(editing(cx), None);

    update(1, cx);
    update(EDITING, cx);
    assert!(editing(cx).is_some());
    update(1, cx);
    assert_eq!(editing(cx), None);
}

#[test]
fn a_reaction_tooltip_names_its_count_or_reactors() {
    let reaction = |count: u64, reactors: Vec<&str>| json!({ "label": "+1", "count": count, "reactors": reactors });
    assert_eq!(reaction_tooltip(&reaction(1, vec![]), true), "Toggle reaction, 1 reaction with +1");
    assert_eq!(reaction_tooltip(&reaction(2, vec![]), false), "2 reactions with +1");
    assert_eq!(reaction_tooltip(&reaction(2, vec!["Ann", "Bob"]), false), "Ann, Bob reacted with +1");
    assert_eq!(reaction_tooltip(&reaction(5, vec!["Ann", "Bob"]), true), "Toggle reaction, Ann, Bob and 3 more reacted with +1");
}

/// The picker registers one `reaction.command.<label>` per reaction; the next picker replaces them.
#[gpui::test]
fn the_reaction_picker_registers_a_command_per_reaction(cx: &mut TestAppContext) {
    let (workbench, cx, _, _) = open(cx, "comment-reactions", "one\n");
    let comment = |labels: &[&str]| json!({ "uniqueIdInThread": 1, "commentReactions": labels.iter().map(|label| json!({ "label": label, "count": 0 })).collect::<Vec<_>>() });
    let registered = |cx: &mut VisualTestContext| {
        workbench.read_with(cx, |workbench, _| workbench.commands.ids().filter(|id| id.starts_with("reaction.command.")).map(str::to_owned).collect::<Vec<_>>())
    };
    let anchor = gpui::Bounds::default();
    workbench.update_in(cx, |workbench, window, cx| workbench.show_reaction_picker(1, 7, &comment(&["+1", "heart"]), anchor, window, cx));
    let mut ids = registered(cx);
    ids.sort();
    assert_eq!(ids, ["reaction.command.+1", "reaction.command.heart"]);
    workbench.update_in(cx, |workbench, window, cx| workbench.show_reaction_picker(1, 7, &comment(&["laugh"]), anchor, window, cx));
    assert_eq!(registered(cx), ["reaction.command.laugh"]);
}
