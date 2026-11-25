use serde_json::{Value, json};

use super::*;

/// completionModel.test.ts `createSuggestItem`: `overwrite_before` chars before `position` (0-based) are replaced.
fn item(label: Value, overwrite_before: usize, kind: u64, incomplete: bool, position: Position, sort_text: Option<&str>, filter_text: Option<&str>) -> CompletionItem {
    let range = json!({ "start": position.column - overwrite_before, "end": position.column });
    let mut dto = json!({ "a": label, "b": kind, "j": range });
    if let Some(sort_text) = sort_text {
        dto["e"] = json!(sort_text);
    }
    if let Some(filter_text) = filter_text {
        dto["f"] = json!(filter_text);
    }
    let line = position.line;
    CompletionItem::from_dto(&dto, 1, false, incomplete, position, &Value::Null, |range| {
        (Position::new(line, range["start"].as_u64().unwrap() as usize), Position::new(line, range["end"].as_u64().unwrap() as usize))
    })
}

fn simple(label: &str, overwrite_before: usize) -> CompletionItem {
    item(json!(label), overwrite_before, KIND_PROPERTY, false, Position::new(0, 9), None, None)
}

fn kind(label: &str, overwrite_before: usize, kind: u64) -> CompletionItem {
    item(json!(label), overwrite_before, kind, false, Position::new(0, 9), None, None)
}

/// The model at upstream's 0-based `column`, shifted as the items' default position is (0, 9) for upstream's (1, 1).
fn model(items: Vec<CompletionItem>, column: usize, leading: &str, snippet_sort: SnippetSort) -> CompletionModel {
    let context = LineContext { leading_line_content: leading.into(), character_count_delta: 0 };
    CompletionModel::new(items, column + 9, context, true, snippet_sort, FuzzyScoreOptions::default())
}

fn labels(model: &mut CompletionModel) -> Vec<String> {
    model.items().iter().map(|item| item.label.clone()).collect()
}

#[test]
fn filtering_cached() {
    let mut model = model(vec![simple("foo", 3), simple("Foo", 3), simple("foo", 2)], 0, "foo", SnippetSort::Inline);
    let first = labels(&mut model);
    model.set_line_context(LineContext { leading_line_content: "foo".into(), character_count_delta: 0 });
    assert_eq!(labels(&mut model), first);
    assert!(model.refilter == Refilter::Nothing);
    model.set_line_context(LineContext { leading_line_content: "foo1".into(), character_count_delta: 1 });
    assert!(model.refilter == Refilter::Incremental);
}

#[test]
fn complete_and_incomplete() {
    assert!(model(vec![simple("foo", 3)], 0, "foo", SnippetSort::Inline).incomplete_providers().is_empty());
    let mut incomplete = item(json!("foo"), 3, KIND_PROPERTY, true, Position::new(0, 9), None, None);
    incomplete.provider = 2;
    let model = model(vec![incomplete, simple("foo", 2)], 0, "foo", SnippetSort::Inline);
    assert_eq!(model.incomplete_providers(), vec![2]);
}

#[test]
fn proper_current_word_when_length_is_0() {
    let items = vec![simple("    </div", 4), simple("a", 0), simple("p", 0), simple("    </tag", 4), simple("    XYZ", 4)];
    let mut model = model(items, 0, "   <", SnippetSort::Inline);
    assert_eq!(labels(&mut model), ["    </div", "    </tag", "a", "p"]);
}

#[test]
fn keep_snippet_sorting_with_prefix() {
    let mut top = model(vec![kind("Snippet1", 1, KIND_SNIPPET), kind("tnippet2", 1, KIND_SNIPPET), kind("semver", 1, KIND_PROPERTY)], 0, "s", SnippetSort::Top);
    assert_eq!(labels(&mut top), ["Snippet1", "semver"]);
    let items = top.items();
    assert!(items[0].score.score < items[1].score.score);

    let mut bottom = model(vec![kind("snippet1", 1, KIND_SNIPPET), kind("tnippet2", 1, KIND_SNIPPET), kind("Semver", 1, KIND_PROPERTY)], 0, "s", SnippetSort::Bottom);
    assert_eq!(labels(&mut bottom), ["Semver", "snippet1"]);
    let items = bottom.items();
    assert!(items[0].score.score < items[1].score.score);

    let mut inline = model(vec![kind("snippet1", 1, KIND_SNIPPET), kind("tnippet2", 1, KIND_SNIPPET), simple("Semver", 1)], 0, "s", SnippetSort::Inline);
    assert_eq!(labels(&mut inline), ["snippet1", "Semver"]);
}

#[test]
fn filter_text_is_matched_not_the_label() {
    let with_filter = item(json!("Map - java.util"), 1, KIND_PROPERTY, false, Position::new(0, 9), None, Some("Map"));
    let mut model = model(vec![with_filter, simple("Map - java.util", 1)], 0, "M", SnippetSort::Inline);
    assert_eq!(model.items().len(), 2);
    model.set_line_context(LineContext { leading_line_content: "Map ".into(), character_count_delta: 3 });
    assert_eq!(model.items().len(), 1);
}

#[test]
fn sort_text_orders_equal_scores() {
    let groups = item(json!("<- groups"), 2, KIND_PROPERTY, false, Position::new(0, 11), Some("00002"), Some("  groups"));
    let source = item(json!("source"), 0, KIND_PROPERTY, false, Position::new(0, 11), Some("00001"), Some("source"));
    let mut items = vec![groups, source];
    items.sort_by(|a, b| compare_provided(a, b, SnippetSort::Inline));
    let mut model = model(items, 2, "  ", SnippetSort::Inline);
    assert_eq!(labels(&mut model), ["source", "<- groups"]);
}

#[test]
fn label_details_sort_by_label() {
    let zzz = item(json!({ "label": "ZZZ" }), 0, 11, false, Position::new(0, 9), None, None);
    let aaa = item(json!({ "label": "AAA" }), 0, 11, false, Position::new(0, 9), None, None);
    let iii = item(json!("III"), 0, 11, false, Position::new(0, 9), None, None);
    let mut items = vec![zzz, aaa, iii];
    items.sort_by(|a, b| compare_provided(a, b, SnippetSort::Inline));
    assert_eq!(items.iter().map(|item| item.label.as_str()).collect::<Vec<_>>(), ["AAA", "III", "ZZZ"]);
}

#[test]
fn score_only_filtered_items_when_typing_more() {
    let mut model = model(vec![simple("console", 0), simple("co_new", 0), simple("bar", 0), simple("car", 0), simple("foo", 0)], 0, "", SnippetSort::Inline);
    assert_eq!(model.items().len(), 5);
    model.set_line_context(LineContext { leading_line_content: "c".into(), character_count_delta: 1 });
    assert_eq!(model.items().len(), 3);
    model.set_line_context(LineContext { leading_line_content: "cn".into(), character_count_delta: 2 });
    assert_eq!(model.items().len(), 2);
    model.set_line_context(LineContext { leading_line_content: "".into(), character_count_delta: 0 });
    assert_eq!(model.items().len(), 5);
}

#[test]
fn relaxed_matching_tries_swapped_chars() {
    let mut model = model(vec![simple("result", 0), simple("replyToUser", 0), simple("randomLolut", 0), simple("car", 0), simple("foo", 0)], 0, "", SnippetSort::Inline);
    model.set_line_context(LineContext { leading_line_content: "rlut".into(), character_count_delta: 4 });
    assert_eq!(labels(&mut model), ["result", "replyToUser", "randomLolut"]);
}

#[test]
fn exact_prefix_ranks_first() {
    let mut model = model(vec![simple("from", 0), simple("form", 0), simple("form:get", 0), simple("testForeignMeasure", 0), simple("fooRoom", 0)], 0, "", SnippetSort::Inline);
    model.set_line_context(LineContext { leading_line_content: "form".into(), character_count_delta: 4 });
    assert_eq!(model.items().len(), 5);
    assert_eq!(labels(&mut model)[..3], ["form", "form:get", "from"]);
}

#[test]
fn select_first_takes_a_preselected_top_item() {
    let mut preselected = simple("bar", 0);
    preselected.preselect = true;
    let mut model = model(vec![simple("foo", 0), preselected], 0, "", SnippetSort::Inline);
    assert_eq!(select_first(model.items()), 1);
}
