use std::collections::HashMap;

use serde_json::json;

use super::*;
use crate::scm_history::COLOR_REGISTRY;

fn item(id: &str, parents: &[&str]) -> HistoryItem {
    HistoryItem { id: id.into(), parent_ids: parents.iter().map(|parent| parent.to_string()).collect(), ..HistoryItem::default() }
}

fn merge_graph(items: &[HistoryItem]) -> Vec<HistoryItemViewModel> {
    to_history_item_view_models(items, &HashMap::new(), None, None, None, false, false, None)
}

/// M merges A and B, which both come from C.
fn merge_items() -> Vec<HistoryItem> {
    vec![item("M", &["A", "B"]), item("A", &["C"]), item("B", &["C"]), item("C", &[])]
}

#[test]
fn merge_commit_branches_out_and_joins_back() {
    use Segment::*;
    let [fg1, fg2, ..] = COLOR_REGISTRY;
    let view_models = merge_graph(&merge_items());

    let merge = &view_models[0];
    assert_eq!(
        graph_paths(merge),
        [
            GraphPath { color: fg2, width: 1., segments: vec![Move(11., 11.), Arc { radius: 11., sweep: true, x: 22., y: 22. }, Move(11., 11.), Line(11., 11.)] },
            GraphPath { color: fg1, width: 1., segments: vec![Move(11., 11.), Line(11., 22.)] },
        ]
    );
    assert_eq!(graph_discs(merge), [(7., None), (5., Some(fg1)), (4., None), (2., Some(fg1))]);

    let join = &view_models[3];
    assert_eq!(
        graph_paths(join),
        [
            GraphPath { color: fg2, width: 1., segments: vec![Move(22., 0.), Arc { radius: 11., sweep: true, x: 11., y: 11. }, Line(11., 11.)] },
            GraphPath { color: fg1, width: 1., segments: vec![Move(11., 0.), Line(11., 11.)] },
        ]
    );
    assert_eq!(graph_discs(join), [(6., None), (4., Some(fg1))]);
}

#[test]
fn history_options_ask_for_the_ref_revisions_after_the_loaded_items() {
    let current = HistoryItemRef { id: "refs/heads/main".into(), revision: Some("abc".into()), ..HistoryItemRef::default() };
    let remote = HistoryItemRef { id: "refs/remotes/origin/main".into(), ..HistoryItemRef::default() };
    let options = history_options(&[Some(current), Some(remote), None], 50, 100);
    assert_eq!(options, json!({ "historyItemRefs": ["abc", "refs/remotes/origin/main"], "limit": 50, "skip": 100 }));
}

#[test]
fn load_more_follows_a_page_that_ends_before_the_root() {
    let items = merge_items();
    assert!(has_more(&merge_graph(&items[..3])));
    assert!(!has_more(&merge_graph(&items)));
    assert!(!has_more(&[]));
}

#[test]
fn changes_of_an_item_compare_it_with_its_first_parent() {
    let view_models = merge_graph(&merge_items());
    let refs = CurrentRefs::default();
    assert_eq!(changes_request(&view_models[0], &refs, None), Some(("M".into(), Some("A".into()))));
    assert_eq!(changes_request(&view_models[3], &refs, None), Some(("C".into(), None)));
}
