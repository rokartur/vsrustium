use std::io::Write;

use serde_json::json;

use super::*;

fn temp_file(name: &str, text: &str) -> PathBuf {
    let file = std::env::temp_dir().join(format!("output-test-{}-{name}", std::process::id()));
    std::fs::write(&file, text).unwrap();
    file
}

fn append(file: &Path, text: &str) {
    std::fs::OpenOptions::new().append(true).open(file).unwrap().write_all(text.as_bytes()).unwrap();
}

/// extHostOutput.ts `ExtHostOutputChannel`: clear and replace send the byte offset written so far as `till`.
#[test]
fn a_channel_reads_its_file_from_the_reset_offset() {
    let file = temp_file("reset", "one\n");
    let mut channel = OutputChannel::new("c".into(), "C".into(), file.clone(), false);
    assert_eq!(channel.read_all(), "one\n");
    append(&file, "two\n");
    assert_eq!(channel.read_appended(), Content::Append("two\n".into()));
    assert_eq!(channel.read_appended(), Content::Append(String::new()));
    // clear(): till is the 8 bytes written.
    channel.reset(Some(8));
    assert_eq!(channel.read_all(), "");
    // replace("three"): the value is written, then the text starts after the old 8 bytes.
    append(&file, "three");
    channel.reset(Some(8));
    assert_eq!(channel.read_all(), "three");
    // resetToEnd without till.
    channel.reset(None);
    append(&file, "four");
    assert_eq!(channel.read_appended(), Content::Append("four".into()));
    std::fs::write(&file, "x").unwrap();
    assert_eq!(channel.read_appended(), Content::Reset("x".into()), "a shorter file restarts at 0");
    std::fs::remove_file(file).unwrap();
}

#[test]
fn a_missing_file_reads_as_empty() {
    let mut channel = OutputChannel::new("c".into(), "C".into(), PathBuf::from("/no/such/output.log"), false);
    assert_eq!(channel.read_all(), "");
}

/// mainThreadOutputService.ts `$register` ids and outputServices.ts active channel on register and dispose.
#[test]
fn extension_channels_are_numbered_per_extension() {
    let mut output = Output::default();
    let first = output.register_extension_channel("hello", PathBuf::from("/1-hello.log"), None, "pub.ext");
    let second = output.register_extension_channel("hello", PathBuf::from("/2-hello.log"), Some("json".into()), "pub.ext");
    let other = output.register_extension_channel("x", PathBuf::from("/3-x.log"), None, "pub.other");
    assert_eq!(first, "extension-output-pub.ext-#1-hello");
    assert_eq!(second, "extension-output-pub.ext-#2-hello");
    assert_eq!(other, "extension-output-pub.other-#1-x");
    assert_eq!(output.active.as_deref(), Some(first.as_str()));
    assert_eq!(output.channel(&second).unwrap().language(), "json");
    assert_eq!(output.channel(&first).unwrap().language(), OUTPUT_LANGUAGE);
    output.remove_channel(&first);
    assert_eq!(output.active.as_deref(), Some(second.as_str()));
    output.remove_channel(&other);
    output.remove_channel(&second);
    assert_eq!(output.active, None);
}

/// logs.contribution.ts `LogOutputChannels`: hidden loggers have no channel until they become visible.
#[test]
fn visible_loggers_are_log_channels() {
    let mut output = Output::default();
    let git = logger_from_json(&json!({ "id": "vscode.git.Git.log", "name": "Git", "resource": { "scheme": "file", "path": "/logs/vscode.git/Git.log" }, "extensionId": "vscode.git" }));
    let hidden = logger_from_json(&json!({ "id": "x", "resource": { "scheme": "file", "path": "/logs/x.log" }, "hidden": true }));
    output.register_logger(git);
    output.register_logger(hidden);
    let labels: Vec<_> = output.channels.iter().map(|channel| (channel.id.as_str(), channel.label.as_str(), channel.language())).collect();
    assert_eq!(labels, [("vscode.git.Git.log", "Git", LOG_LANGUAGE)]);
    output.set_logger_visibility(Path::new("/logs/x.log"), true);
    assert_eq!(output.channel("x").unwrap().label, "x", "a logger without a name is labeled by its id");
    output.deregister_logger(Path::new("/logs/vscode.git/Git.log"));
    assert!(output.channel("vscode.git.Git.log").is_none());
    assert_eq!(output.active.as_deref(), Some("x"));
}

fn lines(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_owned).collect()
}

/// outputChannelModel.ts `parseLogEntryAt`: an entry takes the lines up to the next header, not the last empty line.
#[test]
fn log_entries_span_their_continuation_lines() {
    use filter::{Level, LogEntry, parse_log_entries};
    let text = lines("2025-01-02 03:04:05.678 [info] started\n  at a\n2025-01-02 03:04:05.679 [error] [git] failed\n");
    let entries = parse_log_entries(&text);
    assert_eq!(
        entries,
        [LogEntry { lines: 0..2, timestamp: entries[0].timestamp, level: Level::Info, category: None }, LogEntry { lines: 2..3, timestamp: entries[0].timestamp + 1, level: Level::Error, category: Some("git".into()) }]
    );
    assert_eq!(parse_log_entries(&lines("plain\n2025-01-02 03:04:05.678 [info] x")), []);
}

/// outputServices.ts `OutputViewFilters`: commas split, quotes keep commas, `!` excludes; matching ignores case.
#[test]
fn filters_hide_entries_and_lines() {
    use filter::{Filters, Level, parse_log_entries};
    let mut filters = Filters::default();
    filters.set_text(" Foo, !bar ,\"a,b\", !");
    assert_eq!(filters.include, ["Foo", "\"a,b\""]);
    assert_eq!(filters.exclude, ["bar"]);

    filters.set_text("foo,!BAR");
    let plain = lines("FOO one\nfoo bar\nnothing");
    assert_eq!(filters.hidden_lines(&plain, &[], "c"), [1..2, 2..3]);

    let log = lines("2025-01-02 03:04:05.678 [info] foo\n  bar\n2025-01-02 03:04:05.679 [trace] [git] foo\n");
    let entries = parse_log_entries(&log);
    assert_eq!(filters.hidden_lines(&log, &entries, "c"), [0..2]);
    filters.set_text("");
    *filters.level_mut(Level::Trace) = false;
    assert_eq!(filters.hidden_lines(&log, &entries, "c"), [2..3]);
    assert!(filters.more_filters(Some("c")));
    *filters.level_mut(Level::Trace) = true;
    filters.toggle_category("c:git");
    assert!(filters.has_category("c:git"));
    assert!(filters.more_filters(Some("c")) && !filters.more_filters(Some("d")));
    assert_eq!(filters.hidden_lines(&log, &entries, "c"), [2..3]);
    filters.toggle_category("c:git");
    assert_eq!(filters.categories, ",");
}
