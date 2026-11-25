use super::filter::{Level, LogEntry, parse_log_entries};
use super::*;

fn entry_at(text: &str, line: usize) -> Option<(LogEntry, String)> {
    let lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    let entry = parse_log_entries(&lines).into_iter().find(|entry| entry.lines.start == line)?;
    let range_text = lines[entry.lines.clone()].join("\n");
    Some((entry, range_text))
}

#[test]
fn should_parse_log_entry_with_all_components() {
    let text = "2023-10-15 14:30:45.123 [info] [Git] Initializing repository";
    let (entry, range_text) = entry_at(text, 0).unwrap();
    assert_eq!(entry.level, Level::Info);
    assert_eq!(entry.category.as_deref(), Some("Git"));
    assert_eq!(range_text, text);
}

#[test]
fn should_parse_timestamp_correctly() {
    // `new Date(text).getTime()` reads the text as local time.
    let local = |text: &str| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.3f").unwrap().and_local_timezone(chrono::Local).unwrap().timestamp_millis();
    for timestamp in ["2023-01-01 00:00:00.000", "2023-12-31 23:59:59.999", "2023-06-15 12:30:45.500"] {
        let (entry, _) = entry_at(&format!("{timestamp} [info] Test message"), 0).unwrap();
        assert_eq!(entry.timestamp, local(timestamp), "Failed for timestamp: {timestamp}");
    }
    let (entry, _) = entry_at("2023-01-01 00:00:00.001 [info] a", 0).unwrap();
    assert_eq!(entry.timestamp - local("2023-01-01 00:00:00.000"), 1);
}

#[test]
fn should_parse_multi_line_log_entry() {
    let text = ["2023-10-15 14:30:45.123 [error] [Extension] Failed with error:", "Error: Could not load extension", "    at Object.load (/path/to/file:10:5)"].join("\n");
    let (entry, range_text) = entry_at(&text, 0).unwrap();
    assert_eq!(entry.level, Level::Error);
    assert_eq!(entry.category.as_deref(), Some("Extension"));
    assert_eq!(range_text, text);
}

#[test]
fn should_parse_log_entry_without_category() {
    let text = "2023-10-15 14:30:45.123 [warning] System is running low on memory";
    let (entry, range_text) = entry_at(text, 0).unwrap();
    assert_eq!(entry.level, Level::Warning);
    assert_eq!(entry.category, None);
    assert_eq!(range_text, text);
}

#[test]
fn should_return_null_for_invalid_log_entry() {
    assert_eq!(entry_at("Not a valid log entry", 0), None);
}

#[test]
fn should_parse_all_supported_log_levels() {
    let levels = [("info", Level::Info), ("trace", Level::Trace), ("debug", Level::Debug), ("warning", Level::Warning), ("error", Level::Error)];
    for (level_text, expected) in levels {
        let (entry, _) = entry_at(&format!("2023-10-15 14:30:45.123 [{level_text}] Test message"), 0).unwrap();
        assert_eq!(entry.level, expected, "Failed for log level: {level_text}");
    }
}

#[test]
fn should_handle_last_line_of_file() {
    let text = ["2023-10-15 14:30:45.123 [info] First message", "2023-10-15 14:30:45.124 [info] Last message", ""].join("\n");
    let (first, range_text) = entry_at(&text, 0).unwrap();
    assert_eq!((first.level, first.category), (Level::Info, None));
    assert_eq!(range_text, "2023-10-15 14:30:45.123 [info] First message");
    let (last, range_text) = entry_at(&text, 1).unwrap();
    assert_eq!((last.level, last.category), (Level::Info, None));
    assert_eq!(range_text, "2023-10-15 14:30:45.124 [info] Last message");
    assert_eq!(entry_at(&text, 2), None);
}

#[test]
fn should_parse_multi_line_log_entry_with_empty_lines() {
    let text = [
        "2025-01-27 09:53:00.450 [info] Found with version <20.18.1>",
        "Now using node v20.18.1 (npm v10.8.2)",
        "",
        "> husky - npm run -s precommit",
        "> husky - node v20.18.1",
        "",
        "Reading git index versions...",
    ]
    .join("\n");
    let (entry, range_text) = entry_at(&text, 0).unwrap();
    assert_eq!((entry.level, entry.category), (Level::Info, None));
    assert_eq!(range_text, text);
}

#[test]
fn disposing_the_active_channel_preserves_another_open_channel() {
    let mut output = Output::default();
    for id in ["output-disposal-test-0", "output-disposal-test-1"] {
        output.register_channel(OutputChannel::new(id.into(), id.into(), PathBuf::from(format!("/logs/{id}.log")), false));
    }
    output.active = Some("output-disposal-test-1".into());
    output.remove_channel("output-disposal-test-1");
    assert_eq!(output.active.as_deref(), Some("output-disposal-test-0"));
    assert_eq!(output.channel("output-disposal-test-0").map(|channel| channel.id.as_str()), Some("output-disposal-test-0"));
    assert!(output.channel("output-disposal-test-1").is_none());
}
