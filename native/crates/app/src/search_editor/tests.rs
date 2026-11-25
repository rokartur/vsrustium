use std::path::{Path, PathBuf};

use search::text_search::SearchRange;

use super::*;

fn one_line(line: usize, start: usize, end: usize) -> SearchRange {
    SearchRange { start_line: line, start_column: start, end_line: line, end_column: end }
}

fn file(path: &str, matches: Vec<Match>, context: &[(usize, &str)]) -> FileMatch {
    let context = context.iter().map(|(line, text)| (*line, text.to_string())).collect();
    FileMatch { path: PathBuf::from(path), matches, context, ..FileMatch::default() }
}

fn line_match(preview: &str, line: usize, start: usize, end: usize) -> Match {
    Match::new(vec![preview.to_string()], one_line(0, start, end), one_line(line, start, end))
}

fn label(path: &Path) -> String {
    path.strip_prefix("/root").unwrap().to_string_lossy().trim_start_matches('/').to_string()
}

#[test]
fn header_round_trips_every_field() {
    let config = SearchConfiguration {
        query: "a\\b\nc".to_string(),
        files_to_include: "src/**".to_string(),
        files_to_exclude: "*.md".to_string(),
        is_regexp: true,
        is_case_sensitive: true,
        use_exclude_settings_and_ignore_files: false,
        match_whole_word: true,
        context_lines: 2,
        show_includes_excludes: true,
        only_open_editors: true,
    };
    let header = serialize_search_configuration(&config);
    assert_eq!(header, "# Query: a\\\\b\\nc\n# Flags: CaseSensitive WordMatch RegExp OpenEditors IgnoreExcludeSettings\n# Including: src/**\n# Excluding: *.md\n# ContextLines: 2\n");
    assert_eq!(extract_search_query_from_lines(header.split('\n')).unwrap(), config);
}

#[test]
fn default_header_is_the_query_alone() {
    let config = SearchConfiguration { query: "foo".to_string(), ..SearchConfiguration::default() };
    assert_eq!(serialize_search_configuration(&config), "# Query: foo\n");
    assert_eq!(extract_search_query_from_lines(["# Query: foo", ""]).unwrap(), config);
}

#[test]
fn open_editors_alone_writes_no_flags_line() {
    let config = SearchConfiguration { query: "foo".to_string(), only_open_editors: true, ..SearchConfiguration::default() };
    assert_eq!(serialize_search_configuration(&config), "# Query: foo\n");
}

#[test]
fn a_lone_backslash_in_the_query_is_an_error() {
    assert!(extract_search_query_from_lines(["# Query: a\\b"]).is_err());
    assert!(extract_search_query_from_lines(["# Query: a\\"]).is_err());
}

#[test]
fn lines_that_are_not_directives_are_skipped() {
    let config = extract_search_query_from_lines(["#Query: x", "# Query:x", "Query: x", "# Excluding: a: b"]).unwrap();
    assert_eq!(config.query, "");
    assert_eq!(config.files_to_exclude, "a: b");
    assert!(config.show_includes_excludes);
}

#[test]
fn parse_splits_the_header_at_its_first_empty_line() {
    let (config, body) = parse_serialized_search_editor("# Query: foo\r\n# Flags: RegExp\r\n\r\n1 result - 1 file\r\n\r\nfile.txt:").unwrap();
    assert_eq!(config.query, "foo");
    assert!(config.is_regexp);
    assert_eq!(body, "1 result - 1 file\n\nfile.txt:");
}

#[test]
fn results_serialize_with_counts_labels_and_match_ranges() {
    let mut result = SearchResult::default();
    result.files.push(file("/root/a.txt", vec![line_match("foo bar foo\n", 0, 0, 3), line_match("foo bar foo\n", 0, 8, 11), line_match("x foo", 9, 2, 5)], &[]));
    result.files.push(file("/root/b/c.txt", vec![line_match("foo", 2, 0, 3)], &[]));
    let serialized = serialize_search_result_for_editor(&result, false, label);
    assert_eq!(serialized.text, "4 results - 2 files\n\na.txt:\n   1: foo bar foo\n  10: x foo\n\nb/c.txt:\n  3: foo\n");
    let range = |line, start, end| TextRange { start_line: line, start_column: start, end_line: line, end_column: end };
    assert_eq!(serialized.match_ranges, vec![range(4, 7, 10), range(4, 15, 18), range(5, 9, 12), range(8, 6, 9)]);
}

#[test]
fn no_results_and_the_limit_warning() {
    let serialized = serialize_search_result_for_editor(&SearchResult::default(), true, label);
    assert_eq!(serialized.text, "No Results\nThe result set only contains a subset of all matches. Be more specific in your search to narrow down the results.\n");
    assert!(serialized.match_ranges.is_empty());
}

#[test]
fn context_lines_go_before_their_match_with_a_gap_where_they_skip() {
    let mut result = SearchResult::default();
    result.files.push(file("/root/a.txt", vec![line_match("foo", 4, 0, 3)], &[(2, "two"), (4, "four"), (6, "six"), (12, "twelve")]));
    let serialized = serialize_search_result_for_editor(&result, false, label);
    assert_eq!(serialized.text, "1 result - 1 file\n\na.txt:\n  2  two\n\n  4  four\n  5: foo\n  6  six\n  12  twelve\n");
    assert_eq!(serialized.match_ranges, vec![TextRange { start_line: 7, start_column: 6, end_line: 7, end_column: 9 }]);
}

#[test]
fn a_multiline_match_marks_each_of_its_lines() {
    let preview = vec!["ab\n".to_string(), "cd\n".to_string(), "ef".to_string()];
    let found = Match::new(preview, SearchRange { start_line: 0, start_column: 1, end_line: 2, end_column: 1 }, SearchRange { start_line: 0, start_column: 1, end_line: 2, end_column: 1 });
    let mut result = SearchResult::default();
    result.files.push(file("/root/a.txt", vec![found], &[]));
    let serialized = serialize_search_result_for_editor(&result, false, label);
    assert_eq!(serialized.text, "1 result - 1 file\n\na.txt:\n  1: ab\n  2: cd\n  3: ef\n");
    let range = |line, start, end| TextRange { start_line: line, start_column: start, end_line: line, end_column: end };
    assert_eq!(serialized.match_ranges, vec![range(4, 7, 9), range(5, 6, 9), range(6, 6, 7)]);
}

#[test]
fn names_cut_the_query_or_use_the_backing_file() {
    assert_eq!(search_editor_name(None, "  "), "Search");
    assert_eq!(search_editor_name(None, "short query"), "Search: short query");
    assert_eq!(search_editor_name(None, "a longer query"), "Search: a longer ...");
    assert_eq!(search_editor_name(Some(Path::new("/tmp/found.code-search")), "x"), "Search: found");
}

#[test]
fn suggested_file_names_replace_runs_of_other_characters() {
    assert_eq!(suggest_file_name("foo.*bar(baz)"), "foo_bar_baz_.code-search");
    assert_eq!(suggest_file_name("a-b c_d"), "a-b c_d.code-search");
    assert_eq!(suggest_file_name(""), "Search.code-search");
}

#[test]
fn open_args_set_the_configuration_with_legacy_names() {
    let args = serde_json::json!({ "query": "a", "includes": "src", "filesToExclude": "out", "regexp": true, "isCaseSensitive": true, "contextLines": 2, "triggerSearch": true });
    let config = SearchConfiguration::default().with_args(&args);
    let expected = SearchConfiguration { query: "a".into(), files_to_include: "src".into(), files_to_exclude: "out".into(), is_regexp: true, is_case_sensitive: true, context_lines: 2, ..SearchConfiguration::default() };
    assert_eq!(config, expected);
    assert_eq!(expected.clone().with_args(&serde_json::Value::Null), expected);
}

fn edit(start: (usize, usize), end: (usize, usize), text: &str) -> crate::editor::ContentChange {
    crate::editor::ContentChange { start_line: start.0, start_column: start.1, end_line: end.0, end_column: end.1, range_offset: 0, range_length: 0, text: text.into() }
}

#[test]
fn match_ranges_follow_edits_without_growing_at_their_edges() {
    let range = |line, start, end| TextRange { start_line: line, start_column: start, end_line: line, end_column: end };
    let matched = range(3, 5, 8);
    assert_eq!(matched.after_change(&edit((3, 5), (3, 5), "xy")), range(3, 7, 10));
    assert_eq!(matched.after_change(&edit((3, 8), (3, 8), "xy")), matched);
    assert_eq!(matched.after_change(&edit((3, 6), (3, 6), "xy")), range(3, 5, 10));
    assert_eq!(matched.after_change(&edit((1, 1), (2, 1), "")), range(2, 5, 8));
    assert_eq!(matched.after_change(&edit((3, 2), (3, 2), "a\nb")), range(4, 5, 8));
    assert_eq!(matched.after_change(&edit((3, 1), (3, 6), "")), range(3, 1, 3));
    assert_eq!(matched.after_change(&edit((3, 4), (3, 9), "z")), range(3, 5, 5));
    assert_eq!(matched.after_change(&edit((4, 1), (4, 3), "")), matched);
}

fn text_lines(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_string).collect()
}

#[test]
fn adjacent_match_wraps_around_both_ways() {
    let range = |line, column| TextRange { start_line: line, start_column: column, end_line: line, end_column: column + 3 };
    let ranges = [range(3, 5), range(3, 20), range(6, 5)];
    assert_eq!(adjacent_match(&ranges, (3, 5), false), Some(ranges[1]));
    assert_eq!(adjacent_match(&ranges, (6, 5), false), Some(ranges[0]));
    assert_eq!(adjacent_match(&ranges, (3, 20), true), Some(ranges[0]));
    assert_eq!(adjacent_match(&ranges, (1, 1), true), Some(ranges[2]));
    assert_eq!(adjacent_match(&[], (1, 1), false), None);
}

#[test]
fn result_block_spans_the_file_and_its_lines() {
    let lines = text_lines("# Query: a\n\n2 results - 2 files\n\na.txt:\n  1: a\n\n  4  b\n\nb.txt:\n  2: a\n");
    let (mut deleted, ending) = result_block_lines(&lines, &[(5, 5)]);
    deleted.sort_unstable();
    assert_eq!(deleted, vec![4, 5, 6, 7, 8]);
    assert_eq!(ending, vec![9]);
    let (mut deleted, ending) = result_block_lines(&lines, &[(9, 9)]);
    deleted.sort_unstable();
    assert_eq!(deleted, vec![9, 10, 11]);
    assert!(ending.is_empty());
}

#[test]
fn result_definition_maps_the_column_into_the_file() {
    let lines = text_lines("a.txt:\n  9: one \u{27ea} 20 characters skipped \u{27eb} two\n  10  context\nb.txt:");
    assert_eq!(result_definition(&lines, 1, 7), Some(("a.txt".to_string(), 8, 2)));
    let after = "  9: one \u{27ea} 20 characters skipped \u{27eb} ".encode_utf16().count();
    assert_eq!(result_definition(&lines, 1, after), Some(("a.txt".to_string(), 8, 4 + 20 + 1)));
    assert_eq!(result_definition(&lines, 1, 2), Some(("a.txt".to_string(), 8, 2)));
    assert_eq!(result_definition(&lines, 2, 6), Some(("a.txt".to_string(), 9, 0)));
    assert_eq!(result_definition(&lines, 0, 1), None);
    assert_eq!(result_definition(&lines, 3, 1), None);
}

#[test]
fn result_paths_resolve_as_the_search_result_extension_does() {
    let one = [("app".to_string(), PathBuf::from("/w/app"))];
    let two = [("app".to_string(), PathBuf::from("/w/app")), ("lib".to_string(), PathBuf::from("/w/lib"))];
    let home = Some(Path::new("/Users/me"));
    assert_eq!(resolve_result_path("src/a.rs", &one, home), Some(PathBuf::from("/w/app/src/a.rs")));
    assert_eq!(resolve_result_path("lib \u{2022} b.rs", &two, home), Some(PathBuf::from("/w/lib/b.rs")));
    assert_eq!(resolve_result_path("other \u{2022} b.rs", &two, home), None);
    assert_eq!(resolve_result_path("b.rs", &two, home), None);
    assert_eq!(resolve_result_path("~/notes/c.md", &[], home), Some(PathBuf::from("/Users/me/notes/c.md")));
    assert_eq!(resolve_result_path("/tmp/d.txt", &[], home), Some(PathBuf::from("/tmp/d.txt")));
}
