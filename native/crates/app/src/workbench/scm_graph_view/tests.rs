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
    let options = history_options(&[current, remote], 50, 100);
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

fn reference(id: &str, category: Option<&str>) -> HistoryItemRef {
    HistoryItemRef { id: id.into(), name: id.rsplit('/').next().unwrap().into(), category: category.map(str::to_string), ..HistoryItemRef::default() }
}

// scmHistoryViewPane.ts `_resolveHistoryItemFilter`.
#[test]
fn a_refs_filter_keeps_the_refs_still_there_and_falls_back_to_auto() {
    let current = [Some(reference("refs/heads/main", None)), None, None];
    let filter = RefsFilter::Refs(vec!["refs/heads/a".into(), "refs/heads/gone".into()]);
    let answer = vec![reference("refs/heads/a", None), reference("refs/heads/b", None)];
    let (filter, refs) = resolve_refs_filter(&filter, answer, &current);
    assert_eq!(filter, RefsFilter::Refs(vec!["refs/heads/a".into()]));
    assert_eq!(refs.len(), 1);
    let (filter, refs) = resolve_refs_filter(&RefsFilter::Refs(vec!["refs/heads/gone".into()]), vec![reference("refs/heads/a", None)], &current);
    assert_eq!((filter, refs[0].id.as_str()), (RefsFilter::Auto, "refs/heads/main"));
    let (filter, refs) = resolve_refs_filter(&RefsFilter::All, vec![reference("refs/heads/a", None), reference("refs/tags/v1", None)], &current);
    assert_eq!((filter, refs.len()), (RefsFilter::All, 2));
}

// `_getGraphColorMap`: the current refs keep their colors, the filter's other refs take their item's.
#[test]
fn graph_color_map_colors_only_the_current_refs() {
    let current = HistoryItemRef { color: Some(HISTORY_ITEM_REF_COLOR), ..reference("refs/heads/main", None) };
    let refs = [Some(current.clone()), None, None];
    let colors = graph_color_map(&refs, &[current, reference("refs/heads/a", None)]);
    assert_eq!(colors.get("refs/heads/main"), Some(&Some(HISTORY_ITEM_REF_COLOR)));
    assert_eq!(colors.get("refs/heads/a"), Some(&None));
    let detached = graph_color_map(&[None, None, None], &[reference("refs/heads/a", None)]);
    assert_eq!(detached.get("refs/heads/a"), Some(&None));
}

// `SCMHistoryItemRefsActionViewItem.updateLabel`.
#[test]
fn the_picker_label_names_the_filter() {
    let current = [Some(reference("refs/heads/main", None)), Some(reference("refs/remotes/origin/main", None)), None];
    assert_eq!(refs_filter_label(&RefsFilter::Auto, &[], &current), ("Auto".into(), "main, main".into()));
    assert_eq!(refs_filter_label(&RefsFilter::All, &[], &current).0, "All");
    let one = [reference("refs/heads/a", None)];
    assert_eq!(refs_filter_label(&RefsFilter::Refs(vec!["refs/heads/a".into()]), &one, &current), ("a".into(), "a".into()));
    let two = [reference("refs/heads/a", None), reference("refs/heads/b", None)];
    assert_eq!(refs_filter_label(&RefsFilter::Refs(vec!["refs/heads/a".into(), "refs/heads/b".into()]), &two, &current), ("2 Items".into(), "a, b".into()));
}

// `HistoryItemRefPicker`: All and Auto exclude each other and the refs.
#[test]
fn all_and_auto_are_exclusive_in_the_ref_picker() {
    use super::ref_picker::{accepted_filter, exclusive_selection};
    let id = |handle: i64| ["all", "auto", "refs/heads/a", "refs/heads/b"][handle as usize].to_string();
    assert_eq!(exclusive_selection(&[1], &[1, 2], id), vec![2]);
    assert_eq!(exclusive_selection(&[2], &[0, 2], id), vec![0]);
    assert_eq!(exclusive_selection(&[2, 3], &[1, 2, 3], id), vec![1]);
    assert_eq!(exclusive_selection(&[2, 3], &[2], id), vec![2]);
    let refs = [reference("refs/heads/a", None), reference("refs/heads/b", None)];
    assert_eq!(accepted_filter(&[2, 3], &refs), Some(RefsFilter::Refs(vec!["refs/heads/a".into(), "refs/heads/b".into()])));
    assert_eq!(accepted_filter(&[1], &refs), Some(RefsFilter::Auto));
    assert_eq!(accepted_filter(&[], &refs), None);
}

// `_createQuickPickItems`: All, Auto, then the refs by category; a list filter's refs move up and are selected.
#[test]
fn the_ref_picker_groups_refs_by_category() {
    use super::ref_picker::ref_picker_items;
    use crate::quick_input::Entry;
    let refs = || vec![reference("refs/tags/v1", Some("tags")), reference("refs/heads/a", Some("branches")), reference("refs/heads/b", Some("branches"))];
    let labels = |entries: &[Entry]| -> Vec<String> {
        entries
            .iter()
            .map(|entry| match entry {
                Entry::Item(item) => item.label.clone(),
                Entry::Separator(label) => format!("--{}", label.clone().unwrap_or_default()),
                Entry::Header(header) => header.label.clone(),
            })
            .collect()
    };
    let (entries, _, selected) = ref_picker_items(refs(), &RefsFilter::Auto);
    assert_eq!(labels(&entries), ["All", "Auto", "--branches", "a", "b", "--tags", "v1"]);
    assert_eq!(selected, [1]);
    let (entries, ordered, selected) = ref_picker_items(refs(), &RefsFilter::Refs(vec!["refs/tags/v1".into()]));
    assert_eq!(labels(&entries), ["All", "Auto", "--", "v1", "--branches", "a", "b"]);
    assert_eq!(ordered[selected[0] as usize - 2].id, "refs/tags/v1");
}
