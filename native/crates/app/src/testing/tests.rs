use serde_json::json;

use super::*;

fn add(id: &str, expand: u8) -> Value {
    json!({ "op": 0, "item": { "expand": expand, "item": { "extId": id, "label": id.rsplit('\0').next().unwrap(), "tags": [], "uri": { "scheme": "file", "path": "/w/a.test.js" }, "range": { "startLineNumber": 3, "startColumn": 1, "endLineNumber": 5, "endColumn": 1 } } } })
}

#[test]
fn diffs_build_the_tree_and_remove_subtrees() {
    let mut collection = TestCollection::default();
    let roots = collection.apply(&[add("ctrl", 1), add("ctrl\0b", 0), add("ctrl\0a", 1), add("ctrl\0a\0x", 0)]);
    assert_eq!(roots, vec!["ctrl".to_owned(), "ctrl\0a".to_owned()]);
    assert_eq!(collection.children_of("ctrl"), ["ctrl\0a".to_owned(), "ctrl\0b".to_owned()]);
    let resolvable = collection.apply(&[json!({ "op": 1, "item": { "extId": "ctrl\0b", "expand": 1, "item": { "label": "renamed" } } })]);
    assert_eq!(resolvable, vec!["ctrl\0b".to_owned()]);
    assert_eq!(collection.items["ctrl\0b"].label(), "renamed");
    collection.apply(&[json!({ "op": 3, "itemId": "ctrl\0a" })]);
    assert!(!collection.items.contains_key("ctrl\0a\0x"));
    assert_eq!(collection.children_of("ctrl"), ["ctrl\0b".to_owned()]);
    assert_eq!(collection.tests_in_file("/w/a.test.js").count(), 2);
}

#[test]
fn terminal_states_do_not_go_back_and_parents_compute_the_worst() {
    let mut result = TestResult::new("r".into(), json!({ "targets": [{ "testIds": ["c\0a"] }] }));
    result.add_task(&json!({ "id": "t", "name": "task", "ctrlId": "c", "running": true }));
    result.add_chain(&[json!({ "extId": "c", "label": "c" }), json!({ "extId": "c\0a", "label": "a" })]);
    result.add_chain(&[json!({ "extId": "c", "label": "c" }), json!({ "extId": "c\0b", "label": "b" })]);
    result.update_state("c\0a", "t", ResultState::Failed, Some(3.));
    result.update_state("c\0a", "t", ResultState::Passed, None);
    result.update_state("c\0b", "t", ResultState::Running, None);
    assert_eq!(result.items["c\0a"].own_state, ResultState::Failed);
    assert_eq!(result.computed_state("c"), ResultState::Running);
    result.mark_complete(1);
    assert_eq!(result.items["c\0b"].own_state, ResultState::Skipped);
    assert_eq!(result.computed_state("c"), ResultState::Failed);
    assert_eq!(result.name(), "a");
}

#[test]
fn output_goes_to_the_task_and_marks_the_test() {
    let mut result = TestResult::new("r".into(), json!({}));
    result.add_task(&json!({ "id": "t", "name": "task", "ctrlId": "c" }));
    result.add_chain(&[json!({ "extId": "c", "label": "c" })]);
    result.append_output("t", "one\r\n", None, None);
    result.append_output("t", "two", None, Some("c"));
    assert_eq!(result.tasks[0].output, "one\r\ntwo");
    assert_eq!(result.items["c"].tasks[0].messages[0]["offset"], 5);
}

#[test]
fn default_profile_respects_tags() {
    let test = TestItem { id: "c\0a".into(), expand: ExpandState::NotExpandable, item: json!({ "tags": ["c\0slow"] }) };
    let profile = |id, tag: Option<&str>, is_default| Profile { controller_id: "c".into(), profile_id: id, label: String::new(), group: GROUP_RUN, is_default, tag: tag.map(str::to_owned) };
    let profiles = [profile(1, Some("fast"), true), profile(2, Some("slow"), false), profile(3, None, false)];
    assert_eq!(default_profile(&profiles, GROUP_RUN, &test).map(|profile| profile.profile_id), Some(2));
    assert_eq!(default_profile(&profiles, GROUP_DEBUG, &test), None);
}
