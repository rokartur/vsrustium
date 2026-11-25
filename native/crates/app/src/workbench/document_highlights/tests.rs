use gpui::TestAppContext;
use serde_json::json;

use super::*;
use crate::workbench::goto_symbol::tests::{answer, open, run};

fn range(line: usize, start: usize, end: usize) -> Value {
    json!({ "startLineNumber": line, "startColumn": start, "endLineNumber": line, "endColumn": end })
}

/// Answers the pending `$provideDocumentHighlights` calls in the order sent, one answer each.
fn answer_each(receiver: &std::sync::mpsc::Receiver<crate::host_sync::HostMessage>, answers: Vec<Value>, cx: &mut gpui::VisualTestContext) {
    let calls: Vec<_> = receiver.try_iter().filter(|message| message.method == "$provideDocumentHighlights").collect();
    assert_eq!(calls.len(), answers.len());
    for (call, answer) in calls.into_iter().zip(answers) {
        let _ = call.reply.expect("a call").send(Ok(answer));
    }
    cx.run_until_parked();
}

fn highlights(workbench: &gpui::Entity<Workbench>, cx: &mut gpui::VisualTestContext) -> Vec<(usize, Range<usize>)> {
    workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().highlights.clone())
}

/// wordHighlighter.ts with a provider: its occurrences replace the textual ones, Write drawn strong.
#[gpui::test]
fn a_document_highlight_provider_replaces_the_textual_highlights(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, _) = open(cx, "highlights", "foo\nbar foo\nfoo\n");
    workbench.update(cx, |workbench, _| workbench.language_features.register(REGISTER, vec![json!(9), json!("*")]));
    run(&workbench, cx, "cursorDown");
    answer(&receiver, "$provideDocumentHighlights", json!([{ "range": range(1, 1, 4), "kind": 2 }, { "range": range(3, 1, 4), "kind": 1 }]), cx);
    assert_eq!(highlights(&workbench, cx), [(0, 0..3), (2, 0..3)]);
    workbench.read_with(cx, |workbench, _| {
        assert_eq!(workbench.word_highlight_colors(0, &(0..3)).0, workbench.theme.color("editor.wordHighlightStrongBackground"));
        assert_eq!(workbench.word_highlight_colors(2, &(0..3)).0, workbench.theme.color("editor.wordHighlightBackground"));
    });
}

/// wordHighlighter.test.ts "keeps the first answer when merging is disabled" and "an empty answer still stops the
/// default provider search": the first array in score order wins, even empty.
#[gpui::test]
fn the_first_answer_wins_and_an_empty_one_stops_the_textual_fallback(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, _) = open(cx, "highlights-first", "foo\nbar foo\nfoo\n");
    workbench.update(cx, |workbench, _| {
        workbench.language_features.register(REGISTER, vec![json!(9), json!("*")]);
        workbench.language_features.register(REGISTER, vec![json!(10), json!("plaintext")]);
    });
    run(&workbench, cx, "cursorDown");
    answer_each(&receiver, vec![json!([{ "range": range(3, 1, 4) }]), json!([{ "range": range(1, 1, 4) }])], cx);
    assert_eq!(highlights(&workbench, cx), [(2, 0..3)]);
    run(&workbench, cx, "cursorTop");
    answer_each(&receiver, vec![json!([]), json!([{ "range": range(1, 1, 4) }])], cx);
    assert!(highlights(&workbench, cx).is_empty());
}

/// "preserves textual fallback after a declined answer when merging is disabled": every provider answers null, so
/// the textual occurrences stay.
#[gpui::test]
fn declined_answers_keep_the_textual_highlights(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, _) = open(cx, "highlights-declined", "foo\nbar foo\nfoo\n");
    workbench.update(cx, |workbench, _| workbench.language_features.register(REGISTER, vec![json!(9), json!("*")]));
    run(&workbench, cx, "cursorDown");
    answer(&receiver, "$provideDocumentHighlights", Value::Null, cx);
    assert_eq!(highlights(&workbench, cx), [(1, 0..3)]);
}

/// "queries different priorities concurrently and deduplicates ranges in provider order" and "does not use textual
/// fallback when a matching provider returns null": with `editor.occurrencesHighlightFromAllProviders` the answers
/// merge, and no answer leaves no highlights.
#[gpui::test]
fn merging_asks_every_provider_and_drops_duplicate_ranges(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, _) = open(cx, "highlights-merge", "foo\nbar foo\nfoo\nbaz");
    workbench.update(cx, |workbench, _| {
        let value = json!(true);
        workbench.settings.lock().unwrap().update_value("editor.occurrencesHighlightFromAllProviders", Some(&value), config::service::Target::User, None, &[]).unwrap();
        workbench.language_features.register(REGISTER, vec![json!(9), json!("*")]);
        workbench.language_features.register(REGISTER, vec![json!(10), json!("plaintext")]);
    });
    run(&workbench, cx, "cursorDown");
    answer_each(&receiver, vec![json!([{ "range": range(1, 1, 4) }, { "range": range(2, 5, 8) }]), json!([{ "range": range(2, 5, 8) }, { "range": range(3, 1, 4) }])], cx);
    assert_eq!(highlights(&workbench, cx), [(0, 0..3), (1, 4..7), (2, 0..3)]);
    run(&workbench, cx, "cursorBottom");
    answer_each(&receiver, vec![Value::Null, Value::Null], cx);
    assert!(highlights(&workbench, cx).is_empty());
}

/// `_executeDocumentHighlights` without providers answers the textual occurrences, kind Text.
#[gpui::test]
fn execute_document_highlights_falls_back_to_the_textual_provider(cx: &mut TestAppContext) {
    let (workbench, cx, _receiver, dir) = open(cx, "highlights-execute", "foo\nbar foo\n");
    let uri = json!({ "scheme": "file", "path": dir.join("a.txt").to_string_lossy() });
    let outcome = workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("_executeDocumentHighlights", vec![uri, json!({ "lineNumber": 2, "column": 6 })], window, cx));
    cx.run_until_parked();
    let Outcome::Pending(mut result) = outcome else { panic!("the answer comes later") };
    let answer = result.try_recv().unwrap().unwrap().unwrap();
    assert_eq!(answer, json!([{ "range": range(1, 1, 4), "kind": 0 }, { "range": range(2, 5, 8), "kind": 0 }]));
}
