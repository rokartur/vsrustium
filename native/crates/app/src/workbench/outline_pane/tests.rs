use gpui::TestAppContext;
use serde_json::json;

use super::*;
use crate::workbench::goto_symbol::tests::{answer, open};

/// The view asks the providers for the active editor, lists the symbols by position with children below their
/// expanded parent, and opens a symbol at the start of its name; Left collapses, Up and Down move the selection.
#[gpui::test]
fn the_outline_lists_the_symbols_and_opens_the_selected_one(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, _) = open(cx, "outline-pane", "class A {\n  run() {}\n}\nconst b = 1;\n");
    let rows = |cx: &mut gpui::VisualTestContext| {
        workbench.update(cx, |workbench, cx| {
            workbench.refresh_outline_pane(cx);
            let Status::Ready { model_id, outline, .. } = &workbench.outline_pane.status else { return Vec::new() };
            workbench.outline_rows(*model_id, outline).iter().map(|row| format!("{}{}", "  ".repeat(row.depth), row.element.unwrap().symbol.name)).collect::<Vec<_>>()
        })
    };
    assert!(rows(cx).is_empty());
    assert!(workbench.read_with(cx, |workbench, _| matches!(workbench.outline_pane.status, Status::NoProvider)), "no provider, no outline");

    workbench.update(cx, |workbench, _| {
        workbench.language_features.register(crate::workbench::document_symbols::REGISTER, vec![json!(11), json!("*"), json!("Test")]);
        workbench.outline_pane.key = None;
    });
    rows(cx);
    let range = |start: usize, end: usize| json!({ "startLineNumber": start, "startColumn": 1, "endLineNumber": end, "endColumn": 20 });
    let selection = |line: usize, column: usize| json!({ "startLineNumber": line, "startColumn": column, "endLineNumber": line, "endColumn": column + 1 });
    answer(&receiver, "$provideDocumentSymbols", json!([
        { "name": "b", "detail": "", "kind": 13, "range": range(4, 4), "selectionRange": selection(4, 7), "children": [] },
        { "name": "A", "detail": "", "kind": 4, "range": range(1, 3), "selectionRange": selection(1, 7), "children": [
            { "name": "run", "detail": "", "kind": 5, "range": range(2, 2), "selectionRange": selection(2, 3), "children": [] },
        ] },
    ]), cx);
    assert_eq!(rows(cx), ["A", "  run", "b"]);

    let key = |key: &'static str, cx: &mut gpui::VisualTestContext| workbench.update_in(cx, |workbench, window, cx| workbench.outline_key_down(key, window, cx));
    key("down", cx);
    key("down", cx);
    key("enter", cx);
    workbench.read_with(cx, |workbench, _| {
        assert_eq!(workbench.outline_pane.selected.as_deref(), Some("root/provider_0/A/run"));
        let editor = workbench.editors.active().unwrap();
        assert_eq!(editor.selection().head, crate::editor::Position::new(1, 2), "the start of the name");
        assert_eq!(workbench.breadcrumb_symbols(editor), [(Some(4), "A".to_string()), (Some(5), "run".to_string())], "the breadcrumbs follow the cursor");
    });
    key("left", cx);
    assert_eq!(rows(cx), ["A", "  run", "b"], "a childless row has nothing to collapse");
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.outline_pane.selected.clone()).as_deref(), Some("root/provider_0/A"), "Left on a leaf goes to its parent");
    key("left", cx);
    assert_eq!(rows(cx), ["A", "b"]);
    key("right", cx);
    assert_eq!(rows(cx), ["A", "  run", "b"]);
}

/// outlineActions.ts: Sort By reorders siblings, Collapse All folds every parent, and Follow Cursor expands to and
/// selects the symbol around the cursor.
#[gpui::test]
fn the_title_actions_sort_collapse_and_follow_the_cursor(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, _) = open(cx, "outline-actions", "class B {\n  run() {}\n}\nconst a = 1;\n");
    workbench.update(cx, |workbench, cx| {
        workbench.language_features.register(crate::workbench::document_symbols::REGISTER, vec![json!(11), json!("*"), json!("Test")]);
        workbench.refresh_outline_pane(cx);
        assert!(matches!(workbench.outline_pane.status, Status::NoProvider), "the editor opened before the provider");
        workbench.language_providers_changed(crate::workbench::document_symbols::REGISTER, cx);
    });
    let range = |start: usize, end: usize| json!({ "startLineNumber": start, "startColumn": 1, "endLineNumber": end, "endColumn": 20 });
    answer(&receiver, "$provideDocumentSymbols", json!([
        { "name": "a", "detail": "", "kind": 13, "range": range(4, 4), "selectionRange": range(4, 4), "children": [] },
        { "name": "B", "detail": "", "kind": 4, "range": range(1, 3), "selectionRange": range(1, 1), "children": [
            { "name": "run", "detail": "", "kind": 5, "range": range(2, 2), "selectionRange": range(2, 2), "children": [] },
        ] },
    ]), cx);
    let rows = |cx: &mut gpui::VisualTestContext| {
        workbench.update(cx, |workbench, _| {
            let Status::Ready { model_id, outline, .. } = &workbench.outline_pane.status else { return Vec::new() };
            workbench.outline_rows(*model_id, outline).iter().map(|row| format!("{}{}", "  ".repeat(row.depth), row.element.unwrap().symbol.name)).collect::<Vec<_>>()
        })
    };
    let run = |command: &str, cx: &mut gpui::VisualTestContext| workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(command, Vec::new(), window, cx)));
    assert_eq!(rows(cx), ["B", "  run", "a"]);
    run("outline.sortByName", cx);
    assert_eq!(rows(cx), ["a", "B", "  run"]);
    run("outline.sortByKind", cx);
    assert_eq!(rows(cx), ["B", "  run", "a"], "class (4) before constant (13)");
    run("outline.collapse", cx);
    assert_eq!(rows(cx), ["B", "a"]);
    assert!(workbench.read_with(cx, |workbench, _| workbench.outline_all_collapsed()));
    run("outline.followCursor", cx);
    workbench.update(cx, |workbench, cx| {
        workbench.editors.active_mut().unwrap().set_selections(vec![crate::editor::Selection::caret(crate::editor::Position::new(1, 3))]);
        workbench.refresh_outline_pane(cx);
    });
    assert_eq!(rows(cx), ["B", "  run", "a"], "the parent of the followed symbol expands");
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.outline_pane.selected.clone()).as_deref(), Some("root/provider_0/B/run"));
    let stored = workbench.read_with(cx, |workbench, _| workbench.workspace_storage.lock().unwrap().get("outline/state").map(str::to_owned));
    assert_eq!(stored.as_deref(), Some(r#"{"followCursor":true,"sortBy":2,"filterOnType":true}"#));
}
