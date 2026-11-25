use gpui::TestAppContext;
use serde_json::json;

use super::*;
use crate::workbench::goto_symbol::tests::{answer, open, run};

fn item(dir: &std::path::Path, name: &str, line: usize) -> Value {
    let range = json!({ "startLineNumber": line, "startColumn": 10, "endLineNumber": line, "endColumn": 14 });
    json!({ "_sessionId": "s1", "_itemId": name, "name": name, "kind": 11, "detail": "", "uri": { "scheme": "file", "path": dir.join("a.txt").to_string_lossy() }, "range": range, "selectionRange": range })
}

fn rows(workbench: &gpui::Entity<Workbench>, cx: &mut gpui::VisualTestContext) -> Vec<String> {
    workbench.read_with(cx, |workbench, _| {
        let tree = workbench.hierarchy_peek.widget.as_ref().unwrap().tree.as_ref().unwrap();
        tree.rows().into_iter().map(|node| format!("{}{}", "  ".repeat(tree.nodes[node].depth), tree.nodes[node].item.name)).collect()
    })
}

/// Shift+Alt+H prepares the item at the cursor, lists its callers below it with the first focused, flips to the
/// calls it makes, and Enter opens the focused item; the direction is stored on close.
#[gpui::test]
fn the_call_hierarchy_peek_lists_the_callers_and_flips_direction(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, dir) = open(cx, "call-hierarchy", "function main() {\n  helper();\n}\nfunction helper() {}\nfunction other() { main(); }\n");
    workbench.update(cx, |workbench, _| workbench.language_features.register(CALLS.register, vec![json!(9), json!("*")]));
    run(&workbench, cx, "editor.showCallHierarchy");
    assert!(workbench.read_with(cx, |workbench, _| matches!(workbench.hierarchy_peek.widget.as_ref().unwrap().state, State::Loading)));
    answer(&receiver, "$prepareCallHierarchy", json!([item(&dir, "main", 1)]), cx);
    let call = json!({ "startLineNumber": 5, "startColumn": 20, "endLineNumber": 5, "endColumn": 24 });
    answer(&receiver, "$provideCallHierarchyIncomingCalls", json!([{ "from": item(&dir, "other", 5), "fromRanges": [call] }]), cx);
    assert_eq!(rows(&workbench, cx), ["main", "  other"]);
    workbench.read_with(cx, |workbench, _| {
        let widget = workbench.hierarchy_peek.widget.as_ref().unwrap();
        assert!(matches!(widget.state, State::Data));
        assert_eq!(widget.focused, Some(1));
    });

    run(&workbench, cx, "editor.showOutgoingCalls");
    answer(&receiver, "$provideCallHierarchyOutgoingCalls", json!([]), cx);
    workbench.read_with(cx, |workbench, _| {
        let widget = workbench.hierarchy_peek.widget.as_ref().unwrap();
        assert!(matches!(&widget.state, State::Message(message) if message == "No calls from 'main'"));
    });
    run(&workbench, cx, "editor.closeCallHierarchy");
    workbench.read_with(cx, |workbench, _| {
        assert!(workbench.hierarchy_peek.widget.is_none());
        assert_eq!(workbench.profile_storage.lock().unwrap().get(CALLS.direction_key).as_deref(), Some("outgoingCalls"));
    });
    assert!(receiver.try_iter().any(|message| message.method == "$releaseCallHierarchy"), "the session is released");
}

/// Types: supertypes asked for the root; the API commands resolve through the prepared session.
#[gpui::test]
fn the_type_hierarchy_api_commands_use_the_prepared_session(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, dir) = open(cx, "type-hierarchy", "class A {}\nclass B extends A {}\n");
    workbench.update(cx, |workbench, _| workbench.language_features.register(TYPES.register, vec![json!(4), json!("*")]));
    let uri = json!({ "scheme": "file", "path": dir.join("a.txt").to_string_lossy() });
    let prepared = workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("_executePrepareTypeHierarchy", vec![uri, json!({ "lineNumber": 2, "column": 7 })], window, cx));
    answer(&receiver, "$prepareTypeHierarchy", json!([item(&dir, "B", 2)]), cx);
    let Outcome::Pending(mut prepared) = prepared else { panic!("pending") };
    let roots = prepared.try_recv().unwrap().unwrap().unwrap();
    assert_eq!(roots[0]["name"], "B");
    let supertypes = workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("_executeProvideSupertypes", vec![roots[0].clone()], window, cx));
    answer(&receiver, "$provideTypeHierarchySupertypes", json!([item(&dir, "A", 1)]), cx);
    let Outcome::Pending(mut supertypes) = supertypes else { panic!("pending") };
    assert_eq!(supertypes.try_recv().unwrap().unwrap().unwrap()[0]["name"], "A");
}
