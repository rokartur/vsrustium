//! Cases of debug/test/browser/breakpoints.test.ts and the storage round trip of debugStorage.ts.

use serde_json::{Map, Value, json};

use super::*;

fn file(path: &str) -> Value {
    json!({ "scheme": "file", "path": path })
}

fn lines(model: &DebugModel) -> Vec<u32> {
    model.breakpoints.iter().map(Breakpoint::line_number).collect()
}

#[test]
fn simple() {
    let mut model = DebugModel::default();
    let change = model.add_breakpoints(&file("/myfolder/myfile.js"), vec![BreakpointData::at(5), BreakpointData { enabled: false, ..BreakpointData::at(10) }]);
    assert_eq!(change.added.len(), 2);
    assert!(model.breakpoints_activated);
    assert_eq!(model.breakpoints.len(), 2);
    let ids: Vec<String> = model.breakpoints.iter().map(|breakpoint| breakpoint.id().to_owned()).collect();
    let change = model.remove_breakpoints(&ids);
    assert_eq!(change.removed.len(), 2);
    assert!(model.breakpoints.is_empty());
}

#[test]
fn toggling() {
    let mut model = DebugModel::default();
    let uri = file("/myfolder/myfile.js");
    model.add_breakpoints(&uri, vec![BreakpointData::at(5), BreakpointData { enabled: false, ..BreakpointData::at(10) }]);
    model.add_breakpoints(&uri, vec![BreakpointData { condition: Some("fake condition".into()), ..BreakpointData::at(12) }]);
    assert_eq!(model.breakpoints.len(), 3);
    let last = model.breakpoints.last().unwrap().id().to_owned();
    model.remove_breakpoints(&[last]);
    assert_eq!(model.breakpoints.len(), 2);
    model.breakpoints_activated = false;
    let id = model.breakpoints[0].id().to_owned();
    model.set_enablement(&id, true);
    assert!(model.breakpoints_activated, "enabling a breakpoint activates them");
}

#[test]
fn two_files() {
    let mut model = DebugModel::default();
    let first = file("/myfolder/my file first.js");
    let second = file("/secondfolder/second/second file.js");
    model.add_breakpoints(&first, vec![BreakpointData::at(5), BreakpointData { enabled: false, ..BreakpointData::at(10) }]);
    model.add_breakpoints(&second, vec![BreakpointData::at(1), BreakpointData::at(2), BreakpointData { enabled: false, ..BreakpointData::at(3) }]);
    assert_eq!(model.breakpoints.len(), 5);
    assert_eq!(model.breakpoints_in(&uri_string(&first)).count(), 2);
    assert_eq!(model.breakpoints_in(&uri_string(&second)).count(), 3);
    assert_eq!(model.breakpoints.iter().filter(|breakpoint| breakpoint.line_number() == 5).count(), 1);
    assert_eq!(model.breakpoints.iter().filter(|breakpoint| breakpoint.column() == Some(5)).count(), 0);

    let id = model.breakpoints[0].id().to_owned();
    let mut update = Map::new();
    update.insert("lineNumber".into(), json!(100));
    let change = model.update_breakpoints(&[(id.clone(), update)]);
    assert_eq!(change.changed, vec![id.clone()]);
    assert_eq!(model.breakpoint(&id).unwrap().line_number(), 100);
    assert_eq!(model.breakpoints.iter().filter(|breakpoint| breakpoint.base.enabled).count(), 3);
    model.enable_or_disable_all(false);
    assert!(model.breakpoints.iter().all(|breakpoint| !breakpoint.base.enabled));
    model.set_enablement(&id, true);
    assert!(model.breakpoint(&id).unwrap().base.enabled);
    let ids: Vec<String> = model.breakpoints_in(&uri_string(&first)).map(|breakpoint| breakpoint.id().to_owned()).collect();
    model.remove_breakpoints(&ids);
    assert_eq!(model.breakpoints.len(), 3);
}

#[test]
fn breakpoints_sort_by_file_name_then_line_and_drop_duplicates() {
    let mut model = DebugModel::default();
    model.add_breakpoints(&file("/b/zeta.js"), vec![BreakpointData::at(1)]);
    model.add_breakpoints(&file("/a/alpha.js"), vec![BreakpointData::at(7), BreakpointData::at(3), BreakpointData::at(3)]);
    assert_eq!(lines(&model), vec![3, 7, 1]);
}

#[test]
fn conditions() {
    let mut model = DebugModel::default();
    let uri = file("/myfolder/my file first.js");
    model.add_breakpoints(&uri, vec![BreakpointData { condition: Some("i < 5".into()), hit_condition: Some("17".into()), ..BreakpointData::at(5) }, BreakpointData { condition: Some("j < 3".into()), ..BreakpointData::at(10) }]);
    assert_eq!(model.breakpoints[0].base.condition.as_deref(), Some("i < 5"));
    assert_eq!(model.breakpoints[0].base.hit_condition.as_deref(), Some("17"));
    assert_eq!(model.breakpoints[1].base.condition.as_deref(), Some("j < 3"));
    assert_eq!(model.breakpoints[1].base.hit_condition, None);
    let id = model.breakpoints[0].id().to_owned();
    model.remove_breakpoints(&[id]);
    assert_eq!(model.breakpoints.len(), 1);
}

#[test]
fn function_breakpoints() {
    let mut model = DebugModel::default();
    model.add_function_breakpoint("foo", &json!({}), Some("1".into()));
    model.add_function_breakpoint("bar", &json!({}), Some("2".into()));
    let mut update = Map::new();
    update.insert("name".into(), json!("fooUpdated"));
    model.update_function_breakpoint("1", &update);
    model.update_function_breakpoint("2", &Map::from_iter([("name".to_owned(), json!("barUpdated"))]));
    let names: Vec<&str> = model.function_breakpoints.iter().map(|breakpoint| breakpoint.name.as_str()).collect();
    assert_eq!(names, ["fooUpdated", "barUpdated"]);
    model.remove_function_breakpoints(None);
    assert!(model.function_breakpoints.is_empty());
}

#[test]
fn multiple_sessions_verify_one_location() {
    let mut model = DebugModel::default();
    model.add_breakpoints(&file("/myfolder/myfile.js"), vec![BreakpointData { id: Some("bp".into()), ..BreakpointData::at(5) }]);
    let capabilities = json!({ "supportsConditionalBreakpoints": true });
    model.set_breakpoint_session_data("one", &capabilities, Some(&[("bp".to_owned(), json!({ "verified": true, "line": 6 }))]));
    assert_eq!(model.breakpoints[0].line_number(), 6);
    assert!(model.breakpoints[0].verified(false));
    assert!(!model.breakpoints[0].verified(true), "a modified file's breakpoint is unverified");
    // A second session verifying another line leaves no single location: the user's line shows.
    model.set_breakpoint_session_data("two", &capabilities, Some(&[("bp".to_owned(), json!({ "verified": true, "line": 8 }))]));
    assert_eq!(model.breakpoints[0].line_number(), 5);
    model.set_breakpoint_session_data("two", &capabilities, None);
    assert_eq!(model.breakpoints[0].line_number(), 6);
    assert_eq!(model.breakpoints[0].base.id_from_adapter("one"), None);
}

#[test]
fn exception_breakpoints_follow_the_session_filters() {
    let mut model = DebugModel::default();
    let filters = [json!({ "filter": "uncaught", "label": "Uncaught Exceptions", "default": true }), json!({ "filter": "all", "label": "Caught Exceptions" })];
    assert!(model.set_exception_breakpoints_for_session("one", &filters));
    assert!(!model.set_exception_breakpoints_for_session("two", &filters), "known filters add nothing");
    assert_eq!(model.exception_breakpoints.len(), 2);
    assert!(model.exception_breakpoints[0].base.enabled);
    assert!(!model.exception_breakpoints[1].base.enabled);
    assert_eq!(model.exception_breakpoints_for(Some("two")).count(), 2);
    model.remove_exception_breakpoints_for_session("two");
    assert_eq!(model.exception_breakpoints_for(Some("two")).count(), 0);
    model.set_exception_breakpoint_fallback_session("one");
    assert_eq!(model.exception_breakpoints_for(None).count(), 2);
}

#[test]
fn storage_round_trip_keeps_every_kind() {
    let mut model = DebugModel::default();
    model.add_breakpoints(&file("/w/app.js"), vec![BreakpointData { log_message: Some("x={x}".into()), column: Some(4), ..BreakpointData::at(3) }]);
    model.add_function_breakpoint("main", &json!({ "enabled": false }), None);
    model.set_exception_breakpoints_for_session("one", &[json!({ "filter": "uncaught", "label": "Uncaught Exceptions", "default": true })]);
    model.add_watch_expression("a + b");
    model.breakpoints_activated = false;
    let stored: std::collections::HashMap<&str, Option<String>> = model.storage_entries().into_iter().collect();
    let stored_breakpoints: Value = serde_json::from_str(stored[BREAKPOINTS_KEY].as_deref().unwrap()).unwrap();
    assert_eq!(stored_breakpoints[0]["uri"]["external"], "file:///w/app.js");
    assert_eq!(stored_breakpoints[0]["lineNumber"], 3);
    let loaded = DebugModel::from_storage(|key| stored.get(key).cloned().flatten());
    let mut expected = model.clone();
    for breakpoint in &mut expected.exception_breakpoints {
        breakpoint.supported_sessions.clear();
    }
    assert_eq!(loaded, expected);
    assert_eq!(DebugModel::default().storage_entries().iter().filter(|(_, value)| value.is_some()).count(), 0);
}

#[test]
fn dtos_carry_zero_based_lines() {
    let mut model = DebugModel::default();
    model.add_breakpoints(&file("/w/app.js"), vec![BreakpointData { id: Some("a".into()), condition: Some("x".into()), ..BreakpointData::at(3) }]);
    model.add_function_breakpoint("func", &json!({ "enabled": false, "condition": "c", "hitCondition": "h", "logMessage": "l" }), Some("f".into()));
    let dtos = model.dtos(&model.all_ids());
    assert_eq!(dtos[0], json!({ "type": "source", "id": "a", "enabled": true, "condition": "x", "uri": normalize_uri(&file("/w/app.js")), "line": 2, "character": 0 }));
    assert_eq!(dtos[1], json!({ "type": "function", "id": "f", "enabled": false, "condition": "c", "hitCondition": "h", "logMessage": "l", "functionName": "func" }));
}
