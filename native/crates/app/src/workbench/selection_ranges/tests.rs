use gpui::TestAppContext;
use serde_json::json;

use super::*;
use crate::workbench::goto_symbol::tests::{answer, open, run};

fn range(start: usize, end: usize) -> Value {
    json!({ "range": { "startLineNumber": 1, "startColumn": start, "endLineNumber": 1, "endColumn": end } })
}

/// smartSelect.ts with a provider: Expand asks it once, its ranges replace the bracket ones; a second Expand walks
/// the kept ranges; `_executeSelectionRangeProvider` answers the same ranges as `IRange`s.
#[gpui::test]
fn expand_selection_takes_the_provider_ranges(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, dir) = open(cx, "selection-ranges", "foo(barBaz)\n");
    workbench.update(cx, |workbench, _| workbench.language_features.register(REGISTER, vec![json!(4), json!("*")]));
    run(&workbench, cx, "cursorRight");
    run(&workbench, cx, "cursorRight");
    run(&workbench, cx, "cursorRight");
    run(&workbench, cx, "cursorRight");
    run(&workbench, cx, "cursorRight");
    run(&workbench, cx, "editor.action.smartSelect.expand");
    answer(&receiver, "$provideSelectionRanges", json!([[range(1, 12)]]), cx);
    let selected = |workbench: &gpui::Entity<Workbench>, cx: &mut gpui::VisualTestContext| {
        workbench.read_with(cx, |workbench, _| {
            let selection = workbench.editors.active().unwrap().selection();
            (selection.anchor.column, selection.head.column)
        })
    };
    assert_eq!(selected(&workbench, cx), (4, 7), "the subword first");
    run(&workbench, cx, "editor.action.smartSelect.expand");
    assert_eq!(selected(&workbench, cx), (4, 10));
    run(&workbench, cx, "editor.action.smartSelect.expand");
    assert_eq!(selected(&workbench, cx), (0, 11), "the provider's range, no bracket pair");
    assert!(receiver.try_iter().all(|message| message.method != "$provideSelectionRanges"), "the kept ranges");

    let uri = json!({ "scheme": "file", "path": dir.join("a.txt").to_string_lossy() });
    let outcome = workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("_executeSelectionRangeProvider", vec![uri, json!([{ "lineNumber": 1, "column": 6 }])], window, cx));
    cx.run_until_parked();
    answer(&receiver, "$provideSelectionRanges", json!([[range(1, 12)]]), cx);
    let Outcome::Pending(mut result) = outcome else { panic!("the provider answers later") };
    let ranges = result.try_recv().unwrap().unwrap().unwrap();
    let columns: Vec<_> = ranges[0].as_array().unwrap().iter().map(|range| (range["startColumn"].clone(), range["endColumn"].clone())).collect();
    // The last is the whole document, to line 2's column 1.
    assert_eq!(columns, [(json!(5), json!(8)), (json!(5), json!(11)), (json!(1), json!(12)), (json!(1), json!(1))]);
}

#[test]
fn an_answer_counts_only_with_one_list_per_position() {
    let lines = vec!["héllo".to_owned()];
    let answers = [json!([[range(1, 3)], []]), json!([[range(2, 4)]]), Value::Null];
    let provided = provided_ranges(&lines, &answers, 1);
    assert_eq!(provided, vec![vec![(Position::new(0, 2), Position::new(0, 4))]]);
}
