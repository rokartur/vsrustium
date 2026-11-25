use serde_json::json;

use super::*;

fn item(name: &str, path: &str, line: usize) -> serde_json::Value {
    let range = json!({ "startLineNumber": line, "startColumn": 1, "endLineNumber": line, "endColumn": 9 });
    json!({ "_sessionId": "s", "_itemId": name, "name": name, "kind": 11, "uri": { "scheme": "file", "path": path }, "range": range, "selectionRange": range })
}

fn range(line: usize) -> serde_json::Value {
    json!({ "startLineNumber": line, "startColumn": 3, "endLineNumber": line, "endColumn": 6 })
}

/// Incoming calls: the callers sorted by file and range, each with the call ranges in its own file, previewed there.
#[test]
fn incoming_calls_preview_the_callers_file() {
    let mut tree = HierarchyTree::new(Direction::CallsTo, vec![HierarchyItem::from_dto(&item("main", "/a.ts", 1)).unwrap()]);
    tree.set_children(0, &json!([{ "from": item("late", "/b.ts", 9), "fromRanges": [range(10)] }, { "from": item("early", "/b.ts", 2), "fromRanges": [range(3), range(4)] }]));
    tree.nodes[0].expanded = true;
    let names: Vec<&str> = tree.rows().into_iter().map(|node| tree.nodes[node].item.name.as_str()).collect();
    assert_eq!(names, ["main", "early", "late"]);
    let (uri, _, ranges) = tree.preview(1);
    assert_eq!(uri, "file:///b.ts");
    assert_eq!(ranges, [Range::new(3, 3, 3, 6), Range::new(4, 3, 4, 6)]);
    assert_eq!(tree.preview(0).2, [Range::new(1, 1, 1, 9)], "the root marks its own name");
}

/// Outgoing calls: the calls are in the caller's file, so the preview shows the parent's file.
#[test]
fn outgoing_calls_preview_the_parents_file() {
    let mut tree = HierarchyTree::new(Direction::CallsFrom, vec![HierarchyItem::from_dto(&item("main", "/a.ts", 1)).unwrap()]);
    tree.set_children(0, &json!([{ "to": item("helper", "/b.ts", 5), "fromRanges": [range(2)] }]));
    assert_eq!(tree.preview(1), ("file:///a.ts".to_string(), json!({ "scheme": "file", "path": "/a.ts" }), vec![Range::new(2, 3, 2, 6)]));
    assert_eq!(tree.rows(), [0], "a collapsed root hides its children");
}

/// Types: the children are the items themselves; supertypes preview the parent's file and mark nothing elsewhere.
#[test]
fn supertypes_are_items() {
    let mut tree = HierarchyTree::new(Direction::Supertypes, vec![HierarchyItem::from_dto(&item("Dog", "/a.ts", 1)).unwrap()]);
    tree.set_children(0, &json!([item("Animal", "/b.ts", 3)]));
    assert_eq!(tree.nodes[1].item.name, "Animal");
    assert_eq!(tree.preview(1).0, "file:///a.ts");
    assert!(tree.preview(1).2.is_empty());
    assert_eq!(Direction::from_storage(HierarchyKind::Types, Some("bogus")), Direction::Subtypes);
}
