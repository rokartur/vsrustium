use gpui::{ClipboardItem, TestAppContext};
use serde_json::json;

use super::*;
use crate::workbench::goto_symbol::tests::{answer, open, run};

fn widget_position(workbench: &gpui::Entity<Workbench>, cx: &mut gpui::VisualTestContext) -> Position {
    workbench.read_with(cx, |workbench, _| workbench.paste_edits.widget.as_ref().expect("a widget").position)
}

fn text(workbench: &gpui::Entity<Workbench>, cx: &mut gpui::VisualTestContext) -> String {
    workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().lines.join("\n"))
}

/// copyPasteController.ts with a provider: the copy prepares its data, the paste sends it with the clipboard text,
/// the provider's edit applies and the post-paste widget switches to the default text edit.
#[gpui::test]
fn a_paste_provider_edit_applies_and_the_widget_switches_edits(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, _) = open(cx, "paste-edits", "foo\n");
    let metadata = json!({ "supportsCopy": true, "supportsPaste": true, "supportsResolve": false, "copyMimeTypes": ["x/ref"], "pasteMimeTypes": ["text/plain", "x/ref"], "providedPasteEditKinds": ["markdown.link"] });
    workbench.update(cx, |workbench, _| workbench.language_features.register(REGISTER, vec![json!(5), json!("*"), metadata]));
    run(&workbench, cx, "editor.action.selectAll");
    run(&workbench, cx, "editor.action.clipboardCopyAction");
    let prepared: Vec<_> = receiver.try_iter().filter(|message| message.method == "$prepareDocumentPaste").collect();
    assert_eq!(prepared.len(), 1);
    assert_eq!(prepared[0].args[2], json!([{ "startLineNumber": 1, "startColumn": 1, "endLineNumber": 2, "endColumn": 1 }]));
    for message in prepared {
        let _ = message.reply.expect("a call").send(Ok(json!({ "items": [["x/ref", { "id": "1", "asString": "ref" }]] })));
    }
    run(&workbench, cx, "cursorBottom");
    run(&workbench, cx, "editor.action.clipboardPasteAction");
    let calls: Vec<_> = receiver.try_iter().filter(|message| message.method == "$providePasteEdits").collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].args[4]["items"], json!([["text/plain", { "id": "0", "asString": "foo\n" }], ["x/ref", { "id": "1", "asString": "ref" }]]));
    let edit = json!([{ "title": "Insert Link", "kind": { "value": "markdown.link" }, "insertText": "[foo](x)" }]);
    for call in calls {
        let _ = call.reply.expect("a call").send(Ok(edit.clone()));
    }
    cx.run_until_parked();
    assert_eq!(text(&workbench, cx), "foo\n[foo](x)");
    assert!(workbench.read_with(cx, |workbench, _| workbench.paste_widget_valid()));
    assert_eq!(widget_position(&workbench, cx), Position::new(1, 8), "the widget sits at the end of the pasted text");
    workbench.update_in(cx, |workbench, window, cx| workbench.select_paste_edit(1, window, cx));
    cx.run_until_parked();
    assert_eq!(text(&workbench, cx), "foo\nfoo\n");
    assert!(workbench.read_with(cx, |workbench, _| workbench.paste_widget_valid()), "the widget stays for the next pick");
    assert_eq!(widget_position(&workbench, cx), Position::new(2, 0));
    run(&workbench, cx, "cursorLeft");
    assert!(!workbench.read_with(cx, |workbench, _| workbench.paste_widget_valid()), "a cursor move hides it");
}

/// Without a provider for the clipboard's types the editor pastes as before, at once.
#[gpui::test]
fn without_a_matching_provider_the_paste_is_the_default_one(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, _) = open(cx, "paste-default", "foo");
    let metadata = json!({ "supportsCopy": false, "supportsPaste": true, "pasteMimeTypes": ["image/png"], "providedPasteEditKinds": ["image"] });
    workbench.update(cx, |workbench, _| workbench.language_features.register(REGISTER, vec![json!(5), json!("*"), metadata]));
    cx.write_to_clipboard(ClipboardItem::new_string("bar".into()));
    run(&workbench, cx, "cursorEnd");
    run(&workbench, cx, "editor.action.clipboardPasteAction");
    assert_eq!(text(&workbench, cx), "foobar");
    answer(&receiver, "$providePasteEdits", json!([]), cx);
}

#[test]
fn yield_to_puts_an_edit_after_the_ones_it_yields_to() {
    let edit = |title: &str, kind: &str, yield_to: Value| paste_edit(Some(1), &json!({ "title": title, "kind": { "value": kind }, "insertText": "", "yieldTo": yield_to }));
    let edits = vec![edit("a", "markdown.link", json!([{ "kind": "text" }])), edit("b", "image", json!([])), default_text_edit("x")];
    let titles: Vec<String> = sort_by_yield_to(edits).into_iter().map(|edit| edit.title).collect();
    assert_eq!(titles, ["Insert Plain Text", "a", "b"]);
    assert!(matches_mime("text/*", "text/plain") && matches_mime("*/*", "image/png") && !matches_mime("text/*", "image/png"));
    assert!(kind_contains("markdown", "markdown.link") && !kind_contains("markdown.link", "markdown"));
}

/// `pasteAs` without arguments picks among every edit; Paste as Text applies the default text edit alone.
#[gpui::test]
fn paste_as_picks_an_edit_and_paste_as_text_skips_the_providers(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, _) = open(cx, "paste-as", "");
    let metadata = json!({ "supportsPaste": true, "pasteMimeTypes": ["text/plain"], "providedPasteEditKinds": ["markdown.link"] });
    workbench.update(cx, |workbench, _| workbench.language_features.register(REGISTER, vec![json!(5), json!("*"), metadata]));
    cx.write_to_clipboard(ClipboardItem::new_string("bar".into()));
    run(&workbench, cx, "editor.action.pasteAs");
    answer(&receiver, "$providePasteEdits", json!([{ "title": "Insert Link", "kind": { "value": "markdown.link" }, "insertText": "[bar]()" }]), cx);
    let picked = workbench.read_with(cx, |workbench, _| workbench.paste_edits.pick.as_ref().map(|(edits, _)| edits.iter().map(|edit| edit.title.clone()).collect::<Vec<_>>()));
    assert_eq!(picked, Some(vec!["Insert Link".to_owned(), "Insert Plain Text".to_owned()]));
    workbench.update(cx, |workbench, cx| workbench.accept_paste_as_pick(0, cx));
    cx.run_until_parked();
    assert_eq!(text(&workbench, cx), "[bar]()");
    run(&workbench, cx, "editor.action.pasteAsText");
    assert!(receiver.try_iter().all(|message| message.method != "$providePasteEdits"));
    assert_eq!(text(&workbench, cx), "[bar]()bar");
}

/// Files dropped with Shift: the extension's drop edit applies at the drop position, the path edits stay in the
/// post-drop widget.
#[gpui::test]
fn dropped_files_go_through_the_drop_providers(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, _) = open(cx, "drop-edits", "ab");
    let metadata = json!({ "supportsResolve": false, "dropMimeTypes": ["text/uri-list"] });
    workbench.update(cx, |workbench, _| workbench.language_features.register("$registerDocumentOnDropEditProvider", vec![json!(6), json!("*"), metadata]));
    workbench.update_in(cx, |workbench, window, cx| workbench.drop_into_editor(Position::new(0, 1), vec!["/elsewhere/c.md".into()], window, cx));
    let calls: Vec<_> = receiver.try_iter().filter(|message| message.method == "$provideDocumentOnDropEdits").collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].args[3], json!({ "lineNumber": 1, "column": 2 }));
    assert_eq!(calls[0].args[4]["items"][0][1]["asString"], json!("file:///elsewhere/c.md"));
    for call in calls {
        let _ = call.reply.expect("a call").send(Ok(json!([{ "title": "Insert Link", "kind": "markdown.link", "insertText": "[c](c.md)", "yieldTo": [] }])));
    }
    cx.run_until_parked();
    assert_eq!(text(&workbench, cx), "a[c](c.md)b");
    let titles = workbench.read_with(cx, |workbench, _| workbench.paste_edits.widget.as_ref().map(|widget| (widget.drop, widget.edits.iter().map(|edit| edit.title.clone()).collect::<Vec<_>>())));
    assert_eq!(titles, Some((true, vec!["Insert Link".to_owned(), "Insert Path".to_owned()])));
    assert_eq!(widget_position(&workbench, cx), Position::new(0, 10));
    workbench.update_in(cx, |workbench, window, cx| workbench.select_paste_edit(1, window, cx));
    cx.run_until_parked();
    assert_eq!(text(&workbench, cx), "a/elsewhere/c.mdb");
}
