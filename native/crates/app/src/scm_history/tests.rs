use super::*;
use serde_json::json;

const C0: ColorId = COLOR_REGISTRY[0];
const C1: ColorId = COLOR_REGISTRY[1];
const C2: ColorId = COLOR_REGISTRY[2];
const REF: ColorId = HISTORY_ITEM_REF_COLOR;
const REMOTE: ColorId = HISTORY_ITEM_REMOTE_REF_COLOR;
const BASE: ColorId = HISTORY_ITEM_BASE_REF_COLOR;
const INCOMING: &str = SCM_INCOMING_HISTORY_ITEM_ID;

fn item(id: &str, parent_ids: &[&str], references: &[&str]) -> HistoryItem {
    HistoryItem {
        id: id.to_string(),
        parent_ids: parent_ids.iter().map(|id| id.to_string()).collect(),
        references: references.iter().map(|id| reference(id, None)).collect(),
        ..HistoryItem::default()
    }
}

fn reference(id: &str, revision: Option<&str>) -> HistoryItemRef {
    HistoryItemRef { id: id.to_string(), name: id.to_string(), revision: revision.map(str::to_string), ..HistoryItemRef::default() }
}

fn color_map(entries: &[(&str, ColorId)]) -> HashMap<String, Option<ColorId>> {
    entries.iter().map(|(id, color)| (id.to_string(), Some(*color))).collect()
}

fn view_models(items: &[HistoryItem]) -> Vec<HistoryItemViewModel> {
    to_history_item_view_models(items, &HashMap::new(), None, None, None, false, false, None)
}

fn lanes(nodes: &[GraphNode]) -> Vec<(&str, ColorId)> {
    nodes.iter().map(|node| (node.id.as_str(), node.color)).collect()
}

#[track_caller]
fn assert_node(vm: &HistoryItemViewModel, input: &[(&str, ColorId)], output: &[(&str, ColorId)]) {
    assert_eq!(lanes(&vm.input_swimlanes), input, "input of {}", vm.history_item.id);
    assert_eq!(lanes(&vm.output_swimlanes), output, "output of {}", vm.history_item.id);
}

#[test]
fn empty_graph() {
    assert!(view_models(&[]).is_empty());
}

#[test]
fn single_commit() {
    let vms = view_models(&[item("a", &[], &[])]);
    assert_eq!(vms.len(), 1);
    assert_node(&vms[0], &[], &[]);
}

#[test]
fn linear_graph() {
    let vms = view_models(&[
        item("a", &["b"], &[]),
        item("b", &["c"], &[]),
        item("c", &["d"], &[]),
        item("d", &["e"], &[]),
        item("e", &[], &[]),
    ]);
    assert_eq!(vms.len(), 5);
    assert_node(&vms[0], &[], &[("b", C0)]);
    assert_node(&vms[1], &[("b", C0)], &[("c", C0)]);
    assert_node(&vms[2], &[("c", C0)], &[("d", C0)]);
    assert_node(&vms[3], &[("d", C0)], &[("e", C0)]);
    assert_node(&vms[4], &[("e", C0)], &[]);
}

#[test]
fn merge_commit_single_commit_in_topic_branch() {
    let vms = view_models(&[
        item("a", &["b"], &[]),
        item("b", &["c", "d"], &[]),
        item("d", &["c"], &[]),
        item("c", &["e"], &[]),
        item("e", &["f"], &[]),
    ]);
    assert_eq!(vms.len(), 5);
    assert_node(&vms[0], &[], &[("b", C0)]);
    assert_node(&vms[1], &[("b", C0)], &[("c", C0), ("d", C1)]);
    assert_node(&vms[2], &[("c", C0), ("d", C1)], &[("c", C0), ("c", C1)]);
    assert_node(&vms[3], &[("c", C0), ("c", C1)], &[("e", C0)]);
    assert_node(&vms[4], &[("e", C0)], &[("f", C0)]);
}

#[test]
fn merge_commit_multiple_commits_in_topic_branch() {
    let vms = view_models(&[
        item("a", &["b", "c"], &[]),
        item("c", &["d"], &[]),
        item("b", &["e"], &[]),
        item("e", &["f"], &[]),
        item("f", &["d"], &[]),
        item("d", &["g"], &[]),
    ]);
    assert_eq!(vms.len(), 6);
    assert_node(&vms[0], &[], &[("b", C0), ("c", C1)]);
    assert_node(&vms[1], &[("b", C0), ("c", C1)], &[("b", C0), ("d", C1)]);
    assert_node(&vms[2], &[("b", C0), ("d", C1)], &[("e", C0), ("d", C1)]);
    assert_node(&vms[3], &[("e", C0), ("d", C1)], &[("f", C0), ("d", C1)]);
    assert_node(&vms[4], &[("f", C0), ("d", C1)], &[("d", C0), ("d", C1)]);
    assert_node(&vms[5], &[("d", C0), ("d", C1)], &[("g", C0)]);
}

#[test]
fn create_branch_from_merge_commit() {
    let vms = view_models(&[
        item("a", &["b", "c"], &[]),
        item("c", &["b"], &[]),
        item("b", &["d", "e"], &[]),
        item("e", &["f"], &[]),
        item("f", &["g"], &[]),
        item("d", &["h"], &[]),
    ]);
    assert_eq!(vms.len(), 6);
    assert_node(&vms[0], &[], &[("b", C0), ("c", C1)]);
    assert_node(&vms[1], &[("b", C0), ("c", C1)], &[("b", C0), ("b", C1)]);
    assert_node(&vms[2], &[("b", C0), ("b", C1)], &[("d", C0), ("e", C2)]);
    assert_node(&vms[3], &[("d", C0), ("e", C2)], &[("d", C0), ("f", C2)]);
    assert_node(&vms[4], &[("d", C0), ("f", C2)], &[("d", C0), ("g", C2)]);
    assert_node(&vms[5], &[("d", C0), ("g", C2)], &[("h", C0), ("g", C2)]);
}

#[test]
fn create_multiple_branches_from_a_commit() {
    let vms = view_models(&[
        item("a", &["b", "c"], &[]),
        item("c", &["d"], &[]),
        item("b", &["e", "f"], &[]),
        item("f", &["g"], &[]),
        item("e", &["g"], &[]),
        item("d", &["g"], &[]),
        item("g", &["h"], &[]),
    ]);
    assert_eq!(vms.len(), 7);
    assert_node(&vms[0], &[], &[("b", C0), ("c", C1)]);
    assert_node(&vms[1], &[("b", C0), ("c", C1)], &[("b", C0), ("d", C1)]);
    assert_node(&vms[2], &[("b", C0), ("d", C1)], &[("e", C0), ("d", C1), ("f", C2)]);
    assert_node(&vms[3], &[("e", C0), ("d", C1), ("f", C2)], &[("e", C0), ("d", C1), ("g", C2)]);
    assert_node(&vms[4], &[("e", C0), ("d", C1), ("g", C2)], &[("g", C0), ("d", C1), ("g", C2)]);
    assert_node(&vms[5], &[("g", C0), ("d", C1), ("g", C2)], &[("g", C0), ("g", C1), ("g", C2)]);
    assert_node(&vms[6], &[("g", C0), ("g", C1), ("g", C2)], &[("h", C0)]);
}

#[test]
fn graph_with_color_map() {
    let items = [
        item("a", &["b"], &["topic"]),
        item("b", &["c"], &[]),
        item("c", &["d"], &["origin/topic"]),
        item("d", &["e"], &[]),
        item("e", &["f", "g"], &[]),
        item("g", &["h"], &["origin/main"]),
    ];
    let colors = color_map(&[("topic", REF), ("origin/topic", REMOTE), ("origin/main", BASE)]);
    let vms = to_history_item_view_models(&items, &colors, None, None, None, false, false, None);
    assert_eq!(vms.len(), 6);
    assert_node(&vms[0], &[], &[("b", REF)]);
    assert_node(&vms[1], &[("b", REF)], &[("c", REF)]);
    assert_node(&vms[2], &[("c", REF)], &[("d", REMOTE)]);
    assert_node(&vms[3], &[("d", REMOTE)], &[("e", REMOTE)]);
    assert_node(&vms[4], &[("e", REMOTE)], &[("f", REMOTE), ("g", BASE)]);
    assert_node(&vms[5], &[("f", REMOTE), ("g", BASE)], &[("f", REMOTE), ("h", BASE)]);
}

fn kinds(vms: &[HistoryItemViewModel]) -> Vec<HistoryItemKind> {
    vms.iter().map(|vm| vm.kind).collect()
}

#[test]
fn graph_with_incoming_outgoing_changes_remote_ref_first() {
    let items = [
        item("a", &["b"], &["origin/main"]),
        item("b", &["e"], &[]),
        item("c", &["d"], &["main"]),
        item("d", &["e"], &[]),
        item("e", &["f"], &[]),
        item("f", &["g"], &[]),
    ];
    let colors = color_map(&[("origin/main", REMOTE), ("main", REF)]);
    let main = reference("main", Some("c"));
    let origin_main = reference("origin/main", Some("a"));
    let vms = to_history_item_view_models(&items, &colors, Some(&main), Some(&origin_main), None, true, true, Some("e"));
    assert_eq!(vms.len(), 8);
    use HistoryItemKind::*;
    assert_eq!(kinds(&vms), [Node, Node, OutgoingChanges, Head, Node, IncomingChanges, Node, Node]);
    assert_node(&vms[0], &[], &[("b", REMOTE)]);
    assert_node(&vms[1], &[("b", REMOTE)], &[("e", REMOTE)]);
    assert_node(&vms[2], &[("e", REMOTE)], &[("e", REMOTE), ("c", REF)]);
    assert_node(&vms[3], &[("e", REMOTE), ("c", REF)], &[("e", REMOTE), ("d", REF)]);
    assert_node(&vms[4], &[(INCOMING, REMOTE), ("d", REF)], &[(INCOMING, REMOTE), ("e", REF)]);
    assert_node(&vms[5], &[(INCOMING, REMOTE), ("e", REF)], &[("e", REMOTE), ("e", REF)]);
    assert_node(&vms[6], &[("e", REMOTE), ("e", REF)], &[("f", REMOTE)]);
    assert_node(&vms[7], &[("f", REMOTE)], &[("g", REMOTE)]);
}

#[test]
fn graph_with_incoming_outgoing_changes_local_ref_first() {
    let items = [
        item("a", &["b"], &["main"]),
        item("b", &["e"], &[]),
        item("c", &["d"], &["origin/main"]),
        item("d", &["e"], &[]),
        item("e", &["f"], &[]),
        item("f", &["g"], &[]),
    ];
    let colors = color_map(&[("origin/main", REMOTE), ("main", REF)]);
    let main = reference("main", Some("a"));
    let origin_main = reference("origin/main", Some("c"));
    let vms = to_history_item_view_models(&items, &colors, Some(&main), Some(&origin_main), None, true, true, Some("e"));
    assert_eq!(vms.len(), 8);
    use HistoryItemKind::*;
    assert_eq!(kinds(&vms), [OutgoingChanges, Head, Node, Node, Node, IncomingChanges, Node, Node]);
    assert_node(&vms[0], &[], &[("a", REF)]);
    assert_node(&vms[1], &[("a", REF)], &[("b", REF)]);
    assert_node(&vms[2], &[("b", REF)], &[("e", REF)]);
    assert_node(&vms[3], &[("e", REF)], &[("e", REF), ("d", REMOTE)]);
    assert_node(&vms[4], &[("e", REF), ("d", REMOTE)], &[("e", REF), (INCOMING, REMOTE)]);
    assert_node(&vms[5], &[("e", REF), (INCOMING, REMOTE)], &[("e", REF), ("e", REMOTE)]);
    assert_node(&vms[6], &[("e", REF), ("e", REMOTE)], &[("f", REF)]);
    assert_node(&vms[7], &[("f", REF)], &[("g", REF)]);
}

#[test]
fn graph_with_merged_incoming_changes() {
    let items = [
        item("a", &["b"], &["origin/main"]),
        item("b", &["c", "d"], &[]),
        item("c", &["e"], &["main"]),
        item("d", &["e"], &[]),
        item("e", &["f"], &[]),
        item("f", &["g"], &[]),
    ];
    let colors = color_map(&[("origin/main", REMOTE), ("main", REF)]);
    let main = reference("main", Some("c"));
    let origin_main = reference("origin/main", Some("a"));
    let vms = to_history_item_view_models(&items, &colors, Some(&main), Some(&origin_main), None, true, true, Some("c"));
    assert_eq!(vms.len(), 6);
    use HistoryItemKind::*;
    assert_eq!(kinds(&vms), [Node, Node, Head, Node, Node, Node]);
    assert_node(&vms[0], &[], &[("b", REMOTE)]);
    assert_node(&vms[1], &[("b", REMOTE)], &[("c", REMOTE), ("d", C0)]);
    assert_node(&vms[2], &[("c", REMOTE), ("d", C0)], &[("e", REF), ("d", C0)]);
    assert_node(&vms[3], &[("e", REF), ("d", C0)], &[("e", REF), ("e", C0)]);
    assert_node(&vms[4], &[("e", REF), ("e", C0)], &[("f", REF)]);
    assert_node(&vms[5], &[("f", REF)], &[("g", REF)]);
}

#[test]
fn incoming_and_outgoing_items_and_sorted_refs() {
    let mut items = [item("a", &["b"], &["tag", "origin/main"]), item("b", &["e"], &[]), item("e", &[], &["main"])];
    items[0].display_id = Some("aaaaaaa".to_string());
    let mut colors = color_map(&[("origin/main", REMOTE), ("main", REF)]);
    colors.insert("tag".to_string(), None);
    let main = reference("main", Some("e"));
    let origin_main = reference("origin/main", Some("a"));
    let vms = to_history_item_view_models(&items, &colors, Some(&main), Some(&origin_main), None, true, true, Some("b"));

    let refs: Vec<_> = vms[0].history_item.references.iter().map(|r| (r.id.as_str(), r.color)).collect();
    assert_eq!(refs, [("origin/main", Some(REMOTE)), ("tag", Some(REMOTE))]);
    let incoming = &vms[1].history_item;
    assert_eq!((incoming.id.as_str(), incoming.display_id.as_deref()), (INCOMING, Some("0000000")));
    assert_eq!((incoming.subject.as_str(), incoming.author.as_deref()), ("Incoming Changes", Some("origin/main")));
    let outgoing = &vms[3].history_item;
    assert_eq!((outgoing.id.as_str(), outgoing.parent_ids.as_slice()), (SCM_OUTGOING_HISTORY_ITEM_ID, ["e".to_string()].as_slice()));
    assert_eq!(vms[4].kind, HistoryItemKind::Head);
    assert_eq!(lanes(&vms[4].input_swimlanes), [("e", REMOTE), ("e", REF)]);
    assert_eq!(history_item_index(&vms[4]), 0);
    assert_eq!(history_item_index(&vms[3]), 1);
}

#[test]
fn compare_history_item_refs_orders_current_remote_base_colored_rest() {
    let current = reference("main", None);
    let remote = reference("origin/main", None);
    let base = reference("upstream/main", None);
    let colored = HistoryItemRef { color: Some(C0), ..reference("topic", None) };
    let plain = reference("tag", None);
    let mut refs = vec![plain.clone(), colored.clone(), base.clone(), remote.clone(), current.clone()];
    refs.sort_by(|a, b| compare_history_item_refs(a, b, Some(&current), Some(&remote), Some(&base)));
    assert_eq!(refs, [current, remote, base, colored, plain]);
}

#[test]
fn from_dto() {
    let dto = json!({
        "id": "a", "parentIds": ["b", "c"], "subject": "fix", "message": "fix\n\nbody", "displayId": "a1b2c3d",
        "author": "Ann", "timestamp": 1700000000000.0, "statistics": {"files": 2, "insertions": 3, "deletions": 1},
        "references": [{"id": "refs/heads/main", "name": "main", "revision": "a", "icon": {"id": "git-branch"}}, {"name": "no id"}]
    });
    let history_item = HistoryItem::from_dto(&dto).expect("valid dto");
    assert_eq!(history_item.parent_ids, ["b", "c"]);
    assert_eq!(history_item.display_id.as_deref(), Some("a1b2c3d"));
    assert_eq!(history_item.statistics, Some(HistoryItemStatistics { files: 2, insertions: 3, deletions: 1 }));
    assert_eq!(history_item.references.len(), 1);
    assert_eq!(history_item.references[0].icon, Some(json!({"id": "git-branch"})));
    assert_eq!(history_item.author_email, None);
    assert!(HistoryItem::from_dto(&json!({"subject": "no id"})).is_none());
}
