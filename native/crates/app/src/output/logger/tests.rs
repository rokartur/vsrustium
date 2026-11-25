//! log.test.ts `createUnexpectedErrorHandler`, and the `toErrorMessage`, `format` and spdlog rotation it relies on.

use std::time::{Duration, Instant};

use serde_json::json;

use super::*;

fn error(message: &str) -> Value {
    json!({ "$isError": true, "name": "Error", "message": message, "stack": format!("Error: {message}\n    at test.js:1:1") })
}

#[test]
fn logs_the_stack_of_native_errors() {
    let mut handler = UnexpectedErrorHandler::default();
    let error = error("Unexpected error");
    assert_eq!(handler.handle(&error, Instant::now()).as_deref(), error["stack"].as_str());
}

#[test]
fn preserves_formatting_for_stackless_errors_and_non_native_values() {
    let mut missing_stack = error("Missing stack");
    missing_stack["stack"] = Value::Null;
    let mut empty_stack = error("Empty stack");
    empty_stack["stack"] = json!("");
    let errors = [
        missing_stack,
        empty_stack,
        json!("Error text"),
        json!({ "message": "Serialized error", "stack": "Error: Serialized error\n    at generated.js:1:2" }),
        json!({ "detail": { "exception": { "message": "Nested error", "stack": "Error: Nested error\n    at generated.js:3:4" } } }),
        json!([error("First error"), error("Second error")]),
        Value::Null,
    ];
    let expected = [
        "Missing stack",
        "Empty stack",
        "Error text",
        "Serialized error: Error: Serialized error\n    at generated.js:1:2",
        "Nested error: Error: Nested error\n    at generated.js:3:4",
        "First error: Error: First error\n    at test.js:1:1 (2 errors in total)",
        DEFAULT_ERROR_MESSAGE,
    ];
    for (error, expected) in errors.iter().zip(expected) {
        assert_eq!(to_error_message(error, true), expected);
        // A fresh handler each time: the default message of the last one would repeat the one before.
        assert_eq!(UnexpectedErrorHandler::default().handle(error, Instant::now()).as_deref(), Some(expected));
    }
}

#[test]
fn suppresses_duplicates_through_1000ms_without_extending_the_interval() {
    let mut handler = UnexpectedErrorHandler::default();
    let error = error("Repeated error");
    let start = Instant::now();
    assert!(handler.handle(&error, start).is_some());
    assert!(handler.handle(&error, start + Duration::from_millis(1000)).is_none());
    assert!(handler.handle(&error, start + Duration::from_millis(1001)).is_some());
}

#[test]
fn compares_the_complete_formatted_stack_and_keeps_suppression_per_handler() {
    let now = Instant::now();
    let first = error("Same message");
    let mut second = error("Same message");
    second["stack"] = json!("Error: Same message\n    at other.js:2:2");
    let mut handler = UnexpectedErrorHandler::default();
    assert!(handler.handle(&first, now).is_some());
    assert!(handler.handle(&second, now).is_some());
    assert!(UnexpectedErrorHandler::default().handle(&first, now).is_some());
}

#[test]
fn system_errors_and_format_arguments() {
    // Only an error with a stack goes through `detectSystemErrorMessage`.
    let system = json!({ "message": "ENOENT: no such file", "code": "ENOENT", "errno": -2, "syscall": "open", "stack": "Error: ENOENT" });
    assert_eq!(to_error_message(&system, false), "A system error occurred (ENOENT: no such file)");
    assert_eq!(to_error_message(&json!({ "message": "ENOENT", "code": "ENOENT", "errno": -2, "syscall": "open" }), false), "ENOENT");
    assert_eq!(format(&[json!("a"), json!(1), json!({ "b": [true] }), error("x")], false), "a 1 {\"b\":[true]} x");
}

#[test]
fn console_entries_become_log_lines_like_remote_console_util() {
    let entry = |severity: &str, args: Value| json!({ "type": "__$console", "severity": severity, "arguments": args.to_string() });
    assert_eq!(console_log_line(&entry("error", json!(["boom", 1, { "__$stack": "at x" }])), false), Some((5, "[Extension Host] boom 1".into())));
    assert_eq!(console_log_line(&entry("log", json!(["note"])), false), None);
    assert_eq!(console_log_line(&entry("log", json!(["note"])), true), Some((3, "note".into())));
    assert_eq!(console_log_line(&entry("warn", json!(["careful"])), true), Some((4, "careful".into())));
    assert_eq!(console_log_line(&entry("error", json!([{ "not": "a string" }])), true), None);
    assert_eq!(console_log_line(&json!({ "severity": "error", "arguments": "{" }), false), Some((5, "[Extension Host] Unable to log remote console arguments {".into())));
}

#[test]
fn rotating_files_move_up_one_number_and_drop_the_seventh() {
    let dir = std::env::temp_dir().join(format!("code-gpui-log-rotate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("renderer.log");
    for index in 0..=6 {
        std::fs::write(rotated_name(&file, index), format!("file {index}")).unwrap();
    }
    std::fs::write(&file, vec![b'x'; (LOG_BYTES / ROTATING_FILES) as usize]).unwrap();
    LogFile { path: file.clone(), rotating: true, formatted: true }.append(3, "after the rotation");
    assert!(std::fs::read_to_string(&file).unwrap().ends_with(" [info] after the rotation\n"));
    assert_eq!(std::fs::metadata(rotated_name(&file, 1)).unwrap().len(), LOG_BYTES / ROTATING_FILES);
    assert_eq!(std::fs::read_to_string(rotated_name(&file, 2)).unwrap(), "file 1");
    assert_eq!(std::fs::read_to_string(rotated_name(&file, 6)).unwrap(), "file 5");
    assert!(!rotated_name(&file, 7).exists());
    assert_eq!(rotated_name(&file, 3), dir.join("renderer.3.log"));
}
