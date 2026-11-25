use gpui::TestAppContext;
use serde_json::json;

use super::*;
use crate::workbench::goto_symbol::tests::{answer, open, run};

fn symbol(name: &str, kind: u64, container: Option<&str>, line: usize) -> WorkspaceSymbol {
    let dto = json!({
        "name": name, "kind": kind, "containerName": container,
        "location": { "uri": { "scheme": "file", "path": "/work/src/main.ts" }, "range": { "startLineNumber": line, "startColumn": 1, "endLineNumber": line, "endColumn": 5 } },
    });
    WorkspaceSymbol::from_dto(1, false, &dto).unwrap()
}

fn rows(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| match entry {
            Entry::Separator(label) => format!("--{}--", label.clone().unwrap_or_default()),
            Entry::Item(item) => format!("{}|{}|{:?}|{:?}", item.label, item.description.clone().unwrap_or_default(), item.label_highlights, item.description_highlights),
        })
        .collect()
}

/// `doGetSymbolPicks`: an empty query lists by name; a word scores the name; a second word must match the
/// container label, and a query matching a whole name skips the container.
#[test]
fn workspace_symbol_picks_score_the_name_and_the_container() {
    let symbols = [symbol("run", 5, Some("Runner"), 2), symbol("Main", 4, None, 1), symbol("rename", 11, None, 4)];
    let path = |_: &str| "src/main.ts".to_string();
    assert_eq!(rows(&workspace_symbol_entries(&symbols, &prepare_query(""), path).0), ["Main|src/main.ts|[]|[]", "rename|src/main.ts|[]|[]", "run|Runner \u{2022} src/main.ts|[]|[]"]);
    assert_eq!(rows(&workspace_symbol_entries(&symbols, &prepare_query("ru"), path).0), ["run|Runner \u{2022} src/main.ts|[0..2]|[]"]);
    assert_eq!(rows(&workspace_symbol_entries(&symbols, &prepare_query("r runner"), path).0), ["run|Runner \u{2022} src/main.ts|[0..1]|[0..6]"]);
    assert!(workspace_symbol_entries(&symbols, &prepare_query("r zzz"), path).0.is_empty());
}

/// `getWorkspaceSymbols`: the same symbol from two providers is listed once.
#[test]
fn duplicate_workspace_symbols_are_dropped() {
    let symbols = deduplicate(vec![symbol("a", 5, None, 1), symbol("b", 5, None, 1), symbol("a", 5, None, 1), symbol("a", 5, None, 9)]);
    assert_eq!(symbols.iter().map(|symbol| (symbol.name.as_str(), symbol.range.start.0)).collect::<Vec<_>>(), [("a", 1), ("a", 9), ("b", 1)]);
}

/// Cmd+T asks the providers for the filter after the typing delay, lists their symbols and opens the accepted one at
/// the start of its range.
#[gpui::test]
fn the_picker_asks_the_providers_and_opens_the_accepted_symbol(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, dir) = open(cx, "workspace-symbols", "let a = 1;\nfunction main() {}\n");
    workbench.update(cx, |workbench, _| workbench.language_features.register(REGISTER, vec![json!(7), json!(false)]));
    run(&workbench, cx, "workbench.action.showAllSymbols");
    cx.executor().advance_clock(TYPING_SEARCH_DELAY);
    let uri = json!({ "scheme": "file", "path": dir.join("a.txt").to_string_lossy() });
    let range = json!({ "startLineNumber": 2, "startColumn": 10, "endLineNumber": 2, "endColumn": 14 });
    answer(&receiver, "$provideWorkspaceSymbols", json!({ "cacheId": 3, "symbols": [{ "name": "main", "kind": 11, "location": { "uri": uri, "range": range } }] }), cx);
    let labels = workbench.read_with(cx, |workbench, _| {
        let input = workbench.quick_inputs.current().unwrap();
        input.order.iter().map(|row| input.rows[*row].item.label.clone()).collect::<Vec<_>>()
    });
    assert_eq!(labels, ["main"]);
    run(&workbench, cx, "workbench.action.acceptSelectedQuickOpenItem");
    cx.run_until_parked();
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.editors.active().unwrap().selection().head, crate::editor::Position::new(1, 9)));
}
