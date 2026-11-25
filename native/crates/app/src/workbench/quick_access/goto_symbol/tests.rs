use gpui::TestAppContext;
use serde_json::json;

use super::*;
use crate::outline_model::Range;
use crate::workbench::goto_symbol::tests::{answer, open, run};

fn symbol(name: &str, kind: u64, line: usize, container: Option<&str>) -> DocumentSymbol {
    let range = Range::new(line, 1, line + 1, 2);
    DocumentSymbol { name: name.into(), detail: String::new(), kind, tags: Vec::new(), container_name: container.map(String::from), range, selection_range: Range::new(line, 10, line, 13), children: Vec::new() }
}

fn rows(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| match entry {
            Entry::Separator(label) => format!("--{}--", label.clone().unwrap_or_default()),
            Entry::Header(header) => format!("##{}##", header.label),
            Entry::Item(item) => format!("{}|{}|{:?}", item.label, item.description.clone().unwrap_or_default(), item.label_highlights),
        })
        .collect()
}

/// `doGetSymbolPicks`: an empty filter lists every symbol in order under `symbols (N)`; a filter scores on the
/// name after the icon; a second word filters on the container.
#[test]
fn symbol_picks_filter_on_the_name_and_the_container() {
    let symbols = [symbol("main", 11, 1, None), symbol("helper", 5, 4, Some("Util")), symbol("other", 12, 7, Some("Main"))];
    assert_eq!(rows(&symbol_entries(&symbols, "").0), ["--symbols (3)--", "$(symbol-function) main||[]", "$(symbol-method) helper|Util|[]", "$(symbol-variable) other|Main|[]"]);
    assert_eq!(rows(&symbol_entries(&symbols, "hel").0), ["--symbols (1)--", "$(symbol-method) helper|Util|[17..20]"]);
    let (entries, targets) = symbol_entries(&symbols, "o ma");
    assert_eq!(rows(&entries), ["--symbols (1)--", "$(symbol-variable) other|Main|[19..20]"]);
    assert_eq!(targets, [SymbolTarget { selection: (7, 10), decoration: Range::new(7, 1, 8, 2) }]);
    assert!(symbol_entries(&symbols, "zzz").1.is_empty());
}

/// `@:` groups the picks by kind label, each separator with its count.
#[test]
fn symbol_picks_by_category_group_by_kind() {
    let symbols = [symbol("a", 11, 1, None), symbol("b", 12, 2, None), symbol("c", 11, 3, None)];
    assert_eq!(
        rows(&symbol_entries(&symbols, ":").0),
        ["--functions (2)--", "$(symbol-function) a||[]", "$(symbol-function) c||[]", "--variables (1)--", "$(symbol-variable) b||[]"]
    );
}

/// The picker asks the providers once, previews the active pick and goes to the symbol on accept; without a
/// provider it says so.
#[gpui::test]
fn the_picker_lists_the_symbols_and_goes_to_the_accepted_one(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, _) = open(cx, "goto-symbol", "function main() {\n  return 1;\n}\nconst other = 2;\n");
    run(&workbench, cx, "workbench.action.gotoSymbol");
    let labels = |cx: &mut gpui::VisualTestContext| {
        workbench.read_with(cx, |workbench, _| {
            let input = workbench.quick_inputs.current().unwrap();
            input.order.iter().map(|row| input.rows[*row].item.label.clone()).collect::<Vec<_>>()
        })
    };
    assert_eq!(labels(cx), ["The active text editor does not provide symbol information."]);
    run(&workbench, cx, "workbench.action.closeQuickOpen");
    workbench.update(cx, |workbench, _| workbench.language_features.register(crate::workbench::document_symbols::REGISTER, vec![json!(11), json!("*"), json!("Test")]));
    run(&workbench, cx, "workbench.action.gotoSymbol");
    let range = |start: usize, end: usize| json!({ "startLineNumber": start, "startColumn": 1, "endLineNumber": end, "endColumn": 2 });
    let selection = |line: usize| json!({ "startLineNumber": line, "startColumn": 10, "endLineNumber": line, "endColumn": 14 });
    answer(&receiver, "$provideDocumentSymbols", json!([
        { "name": "main", "detail": "", "kind": 11, "range": range(1, 3), "selectionRange": selection(1), "children": [] },
        { "name": "other", "detail": "", "kind": 13, "range": range(4, 4), "selectionRange": { "startLineNumber": 4, "startColumn": 7, "endLineNumber": 4, "endColumn": 12 }, "children": [] },
    ]), cx);
    assert_eq!(labels(cx), ["$(symbol-function) main", "$(symbol-constant) other"]);
    workbench.read_with(cx, |workbench, _| {
        let state = workbench.quick_access.goto_line.as_ref().unwrap();
        assert_eq!(state.highlight(workbench.editors.active().unwrap().model_id), Some((0, 2)), "the symbol around the cursor is active and previewed");
    });
    run(&workbench, cx, "workbench.action.quickOpenSelectNext");
    run(&workbench, cx, "workbench.action.acceptSelectedQuickOpenItem");
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.editors.active().unwrap().selection().head, crate::editor::Position::new(3, 6)));
    // The second picker reuses the cached outline: the providers are not asked again.
    run(&workbench, cx, "workbench.action.gotoSymbol");
    assert!(receiver.try_iter().all(|message| message.method != "$provideDocumentSymbols"));
    // `ItemActivation.NONE`: outside every symbol no pick is active.
    run(&workbench, cx, "workbench.action.closeQuickOpen");
    workbench.update(cx, |workbench, _| workbench.editors.active_mut().unwrap().move_to(crate::editor::Position::new(4, 0), false));
    run(&workbench, cx, "workbench.action.gotoSymbol");
    workbench.read_with(cx, |workbench, _| assert!(workbench.quick_inputs.current().unwrap().focused.is_none(), "no active pick"));
}
