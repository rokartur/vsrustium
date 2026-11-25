use std::path::Path;

use serde_json::{Value, json};

use super::*;

fn one_line(line: usize, start: usize, end: usize) -> SearchRange {
    SearchRange { start_line: line, start_column: start, end_line: line, end_column: end }
}

/// `new TextSearchMatch(preview, range)`: the preview line holds the match at the range's columns.
fn text_search_match(preview: &str, range: SearchRange) -> Value {
    let preview_range = one_line(0, range.start_column, range.end_column);
    let json_range = |range: SearchRange| json!({ "startLineNumber": range.start_line, "startColumn": range.start_column, "endLineNumber": range.end_line, "endColumn": range.end_column });
    json!({ "previewText": preview, "rangeLocations": [{ "preview": json_range(preview_range), "source": json_range(range) }] })
}

fn pattern(replace: &str, search: &str, is_reg_exp: bool) -> ReplacePattern {
    ReplacePattern::new(replace, &PatternInfo { pattern: search, is_reg_exp, is_case_sensitive: false, is_word_match: false })
}

fn case_sensitive(replace: &str, search: &str) -> ReplacePattern {
    ReplacePattern::new(replace, &PatternInfo { pattern: search, is_reg_exp: true, is_case_sensitive: true, is_word_match: false })
}

#[test]
fn line_match() {
    let line_match = Match::new(vec!["0 foo bar".to_string()], one_line(0, 2, 5), one_line(1, 0, 5));
    assert_eq!(line_match.text(), "0 foo bar");
    assert_eq!(line_match.range, one_line(1, 0, 5));
    assert_eq!(line_match.full_match_text(false), "foo");
    assert_eq!(line_match.full_match_text(true), "0 foo bar");
}

#[test]
fn line_match_remove() {
    let mut result = SearchResult::default();
    result.add(0, Path::new("/folder/file.txt"), &[text_search_match("foo bar", one_line(1, 0, 3))]);
    result.remove_match(0, 0);
    assert!(result.is_empty());
}

#[test]
fn adding_a_raw_match_adds_a_file_match_with_line_matches() {
    let mut result = SearchResult::default();
    result.add(0, Path::new("/root/1"), &[text_search_match("preview 1", one_line(1, 1, 4)), text_search_match("preview 1", one_line(1, 4, 11)), text_search_match("preview 2", one_line(1, 0, 1))]);
    assert_eq!(result.count(), 3);
    assert_eq!(result.files.len(), 1);
    let matches = &result.files[0].matches;
    let summary: Vec<(&str, SearchRange)> = matches.iter().map(|found| (found.text(), found.range)).collect();
    assert_eq!(summary, vec![("preview 2", one_line(1, 0, 1)), ("preview 1", one_line(1, 1, 4)), ("preview 1", one_line(1, 4, 11))]);
}

#[test]
fn adding_multiple_raw_matches() {
    let mut result = SearchResult::default();
    result.add(0, Path::new("/root/1"), &[text_search_match("preview 1", one_line(1, 1, 4)), text_search_match("preview 1", one_line(1, 4, 11))]);
    result.add(0, Path::new("/root/2"), &[text_search_match("preview 2", one_line(1, 0, 1))]);
    assert_eq!(result.count(), 3);
    assert_eq!(result.file_count(), 2);
    assert_eq!(result.files[0].path, Path::new("/root/1"));
    assert_eq!(result.files[0].matches.len(), 2);
    assert_eq!(result.files[1].matches[0].text(), "preview 2");
}

#[test]
fn the_same_match_is_added_once() {
    let mut result = SearchResult::default();
    result.add(0, Path::new("/root/1"), &[text_search_match("preview 1", one_line(1, 1, 4))]);
    result.add(0, Path::new("/root/1"), &[text_search_match("preview 1", one_line(1, 1, 4))]);
    assert_eq!(result.count(), 1);
}

#[test]
fn context_lines_add_no_match() {
    let mut result = SearchResult::default();
    result.add(0, Path::new("/root/1"), &[json!({ "text": "context", "lineNumber": 3 })]);
    assert!(result.is_empty());
}

#[test]
fn file_match_name() {
    let file = FileMatch { path: "/folder/file.txt".into(), folder: 0, matches: Vec::new(), removed: Vec::new() };
    assert_eq!(file.name(), "file.txt");
}

#[test]
fn comparer() {
    let file = |path: &str| FileMatch { path: path.into(), folder: 0, matches: Vec::new(), removed: Vec::new() };
    assert_eq!(compare_file_matches(&file("/foo"), &file("/with/path")), Ordering::Less);
    assert_eq!(compare_file_matches(&file("/with/path"), &file("/foo")), Ordering::Greater);
    assert_eq!(compare_file_matches(&file("/foo"), &file("/foo")), Ordering::Equal);
    assert_eq!(compare_file_matches(&file("/with/path"), &file("/with/path/foo")), Ordering::Less);
    // By default, path < path2.
    assert_eq!(compare_file_matches(&file("/with/path/foo10"), &file("/with/path2/foo1")), Ordering::Less);
}

#[test]
fn cross_folder_comparer() {
    let file = |path: &str, folder| FileMatch { path: path.into(), folder, matches: Vec::new(), removed: Vec::new() };
    assert_eq!(compare_file_matches(&file("/voo/foo.a", 0), &file("/with/path.c", 1)), Ordering::Less);
    assert_eq!(compare_file_matches(&file("/with/path.c", 1), &file("/with/path/bar.b", 1)), Ordering::Less);
}

#[test]
fn preview_cuts_the_text_before_the_match() {
    let text = "export function compareFileNames(one: string, other: string): number {";
    let start = text.find("one").unwrap();
    let found = Match::new(vec![text.to_string()], one_line(0, start, start + 3), one_line(4, start, start + 3));
    let preview = found.preview();
    assert_eq!(preview.before, "\u{2026}function compareFileNames(");
    assert_eq!(preview.full_before, &text[..start]);
    assert_eq!(preview.inside, "one");
    assert_eq!(preview.after, ": string, other: string): number {");
}

#[test]
fn preview_of_a_match_over_lines_ends_with_its_first_line() {
    let found = Match::new(vec!["foo bar".to_string(), "baz".to_string()], SearchRange { start_line: 0, start_column: 4, end_line: 1, end_column: 1 }, one_line(0, 4, 7));
    assert_eq!(found.match_string(), "bar");
    assert_eq!(found.full_match_text(false), "bar\nb");
    assert_eq!(found.preview().after, "");
}

#[test]
fn lcut() {
    use super::lcut;
    assert_eq!(lcut("foo bar", 0, ""), "");
    assert_eq!(lcut("foo bar", 1, ""), "bar");
    assert_eq!(lcut("foo bar", 3, ""), "bar");
    assert_eq!(lcut("foo bar", 4, ""), "bar");
    assert_eq!(lcut("foo bar", 5, ""), "foo bar");
    assert_eq!(lcut("test string 0.1.2.3", 3, ""), "2.3");
    assert_eq!(lcut("foo bar", 0, "\u{2026}"), "\u{2026}");
    assert_eq!(lcut("foo bar", 1, "\u{2026}"), "\u{2026}bar");
    assert_eq!(lcut("foo bar", 3, "\u{2026}"), "\u{2026}bar");
    assert_eq!(lcut("foo bar", 4, "\u{2026}"), "\u{2026}bar");
    assert_eq!(lcut("foo bar", 5, "\u{2026}"), "foo bar");
    assert_eq!(lcut("test string 0.1.2.3", 3, "\u{2026}"), "\u{2026}2.3");
    assert_eq!(lcut("", 10, ""), "");
    assert_eq!(lcut("a", 10, ""), "a");
    assert_eq!(lcut(" a", 10, ""), "a");
    assert_eq!(lcut("            a", 10, ""), "a");
    assert_eq!(lcut(" bbbb       a", 10, ""), "bbbb       a");
    assert_eq!(lcut("............a", 10, ""), "............a");
    assert_eq!(lcut("", 10, "\u{2026}"), "");
    assert_eq!(lcut("a", 10, "\u{2026}"), "a");
    assert_eq!(lcut(" a", 10, "\u{2026}"), "a");
    assert_eq!(lcut("            a", 10, "\u{2026}"), "a");
    assert_eq!(lcut(" bbbb       a", 10, "\u{2026}"), "bbbb       a");
    assert_eq!(lcut("............a", 10, "\u{2026}"), "............a");
}

#[test]
fn parse_replace_string() {
    let test_parse = |input: &str, expected: &str, expected_has_parameters: bool| {
        let actual = pattern(input, "somepattern", true);
        assert_eq!(actual.pattern, expected, "{input}");
        assert_eq!(actual.has_parameters, expected_has_parameters, "{input}");
        let actual = pattern(&format!("hello{input}hi"), "sonepattern", true);
        assert_eq!(actual.pattern, format!("hello{expected}hi"), "{input}");
        assert_eq!(actual.has_parameters, expected_has_parameters, "{input}");
    };
    test_parse("hello", "hello", false);
    test_parse("\\thello", "\thello", false);
    test_parse("\\nhello", "\nhello", false);
    test_parse("\\\\thello", "\\thello", false);
    test_parse("\\\\\\thello", "\\\thello", false);
    test_parse("\\\\\\\\thello", "\\\\thello", false);
    test_parse("hello\\", "hello\\", false);
    test_parse("hello\\x", "hello\\x", false);
    test_parse("hello\\0", "hello\\0", false);
    test_parse("hello$1", "hello$1", true);
    test_parse("hello$2", "hello$2", true);
    test_parse("hello$12", "hello$12", true);
    test_parse("hello$99", "hello$99", true);
    test_parse("hello$99a", "hello$99a", true);
    test_parse("hello$100", "hello$100", false);
    test_parse("hello$100a", "hello$100a", false);
    test_parse("hello$10a0", "hello$10a0", true);
    test_parse("hello$$", "hello$$", false);
    test_parse("hello$$0", "hello$$0", false);
    test_parse("hello$0", "hello$&", true);
    test_parse("hello$02", "hello$&2", true);
    test_parse("hello$`", "hello$`", true);
    test_parse("hello$'", "hello$'", true);
}

#[test]
fn parameters_are_parsed_only_for_a_regex() {
    assert!(!pattern("hello$0", "abc", false).has_parameters);
    assert!(pattern("hello$0", "abc", true).has_parameters);
}

#[test]
fn get_replace_string_if_given_text_is_a_complete_match() {
    let require = "let\\s+(\\w+)\\s*=\\s*require\\s*\\(\\s*['\"]([\\w.\\-/]+)\\s*['\"]\\s*\\)\\s*";
    assert_eq!(pattern("hello", "bla", true).replace_string("bla", false).as_deref(), Some("hello"));
    assert_eq!(pattern("hello", "bla", false).replace_string("bla", false).as_deref(), Some("hello"));
    assert_eq!(pattern("hello", "(bla)", true).replace_string("bla", false).as_deref(), Some("hello"));
    assert_eq!(pattern("hello$0", "(bla)", true).replace_string("bla", false).as_deref(), Some("hellobla"));
    let test_object = pattern("import * as $1 from '$2';", require, true);
    assert_eq!(test_object.replace_string("let fs = require('fs')", false).as_deref(), Some("import * as fs from 'fs';"));
    assert_eq!(test_object.replace_string("let something = require('fs')", false).as_deref(), Some("import * as something from 'fs';"));
    assert_eq!(test_object.replace_string("let require('fs')", false), None);
    let test_object = pattern("import * as $1 from '$1';", require, true);
    assert_eq!(test_object.replace_string("let something = require('fs')", false).as_deref(), Some("import * as something from 'something';"));
    let test_object = pattern("import * as $2 from '$1';", require, true);
    assert_eq!(test_object.replace_string("let something = require('fs')", false).as_deref(), Some("import * as fs from 'something';"));
    let test_object = pattern("import * as $0 from '$0';", require, true);
    assert_eq!(test_object.replace_string("let something = require('fs');", false).as_deref(), Some("import * as let something = require('fs') from 'let something = require('fs')';"));
    let test_object = pattern("import * as $1 from '$2';", require, false);
    assert_eq!(test_object.replace_string("let fs = require('fs');", false), None);
    assert_eq!(pattern("cat$1", "for(.*)", true).replace_string("for ()", false).as_deref(), Some("cat ()"));
}

#[test]
fn case_operations() {
    let test_object = pattern("a\\u$1l\\u\\l\\U$2M$3n", "a(l)l(good)m(e)n", true);
    assert_eq!(test_object.replace_string("allgoodmen", false).as_deref(), Some("aLlGoODMen"));
}

#[test]
fn case_operations_no_false_positive() {
    assert_eq!(pattern("\\left $1", "(pattern)", true).replace_string("pattern", false).as_deref(), Some("\\left pattern"));
    assert_eq!(pattern("\\hi \\left $1", "(pattern)", true).replace_string("pattern", false).as_deref(), Some("\\hi \\left pattern"));
    assert_eq!(pattern("\\left \\L$1", "PATT(ERN)", true).replace_string("PATTERN", false).as_deref(), Some("\\left ern"));
}

#[test]
fn case_operations_and_newline() {
    // #140734
    assert_eq!(pattern("$1\n\\U$2", "(multi)(line)", true).replace_string("multiline", false).as_deref(), Some("multi\nLINE"));
}

#[test]
fn get_replace_string_for_no_matches() {
    assert_eq!(pattern("hello", "bla", true).replace_string("foo", false), None);
    assert_eq!(pattern("hello", "bla", false).replace_string("foo", false), None);
}

#[test]
fn get_replace_string_if_match_is_sub_string_of_the_text() {
    let text = "this is a bla text";
    let cases = [
        ("hello", "bla", true, "hello"),
        ("hello", "bla", false, "hello"),
        ("that", "this(?=.*bla)", true, "that"),
        ("$1at", "(th)is(?=.*bla)", true, "that"),
        ("$1e", "(th)is(?=.*bla)", true, "the"),
        ("$1ere", "(th)is(?=.*bla)", true, "there"),
        ("$1", "(th)is(?=.*bla)", true, "th"),
        ("ma$1", "(th)is(?=.*bla)", true, "math"),
        ("ma$1s", "(th)is(?=.*bla)", true, "maths"),
        ("$0", "(th)is(?=.*bla)", true, "this"),
        ("$0$1", "(th)is(?=.*bla)", true, "thisth"),
        ("foo", "bla(?=\\stext$)", true, "foo"),
        ("f$1", "b(la)(?=\\stext$)", true, "fla"),
        ("f$0", "b(la)(?=\\stext$)", true, "fbla"),
        ("$0ah", "b(la)(?=\\stext$)", true, "blaah"),
    ];
    for (replace, search, is_reg_exp, expected) in cases {
        assert_eq!(pattern(replace, search, is_reg_exp).replace_string(text, false).as_deref(), Some(expected), "{replace} {search}");
    }
    assert_eq!(case_sensitive("newrege$1", "Testrege(\\w*)").replace_string("Testregex", true).as_deref(), Some("Newregex"));
    assert_eq!(case_sensitive("newrege$1", "TESTREGE(\\w*)").replace_string("TESTREGEX", true).as_deref(), Some("NEWREGEX"));
    assert_eq!(case_sensitive("new_rege$1", "Test_Rege(\\w*)").replace_string("Test_Regex", true).as_deref(), Some("New_Regex"));
    assert_eq!(case_sensitive("new-rege$1", "Test-Rege(\\w*)").replace_string("Test-Regex", true).as_deref(), Some("New-Regex"));
}

#[test]
fn replace_string_of_a_match_for_regular_expressions() {
    let mut result = SearchResult::default();
    result.add(0, Path::new("/root/1"), &[text_search_match("preview 1", one_line(1, 1, 4)), text_search_match("preview 1", one_line(1, 4, 11))]);
    let found = &result.files[0].matches[0];
    assert_eq!(found.replace_string(&pattern("hello", "re", false), false), "hello");
    assert_eq!(found.replace_string(&pattern("hello", "re", true), false), "hello");
    assert_eq!(found.replace_string(&pattern("hello", "re(?:vi)", true), false), "hello");
    assert_eq!(found.replace_string(&pattern("hello", "r(e)(?:vi)", true), false), "hello");
    assert_eq!(found.replace_string(&pattern("hello$1", "r(e)(?:vi)", true), false), "helloe");
}

#[test]
fn messages() {
    assert_eq!(result_count_message(1, 1), "1 result in 1 file");
    assert_eq!(result_count_message(1, 2), "1 result in 2 files");
    assert_eq!(result_count_message(3, 1), "3 results in 1 file");
    assert_eq!(result_count_message(3, 2), "3 results in 2 files");
    assert_eq!(replace_all_confirmation_message(1, 1, "x"), "Replace 1 occurrence across 1 file with 'x'?");
    assert_eq!(replace_all_confirmation_message(3, 2, ""), "Replace 3 occurrences across 2 files?");
    let long: Vec<String> = (0..12).map(|line| line.to_string()).collect();
    assert_eq!(replace_all_confirmation_message(2, 1, &long.join("\n")), "Replace 2 occurrences across 1 file with '0\n1\n2\n3\n4\n5\n6\n7\n8\n9\n...'?");
    assert_eq!(after_replace_all_message(2, 1, "x"), "Replaced 2 occurrences across 1 file with 'x'.");
    assert_eq!(after_replace_all_message(1, 3, ""), "Replaced 1 occurrence across 3 files.");
}

#[test]
fn matches_follow_model_and_dismissed_stay_out() {
    let mut file = FileMatch { path: "/folder/file.txt".into(), folder: 0, matches: Vec::new(), removed: Vec::new() };
    let lines = vec!["the ąthe".to_string(), "other the".to_string()];
    let params = SearchParams { search_string: "the", is_regex: false, match_case: false, word_separators: None };
    file.update_from_lines(&lines, &params);
    let ranges: Vec<_> = file.matches.iter().map(|m| (m.range.start_line, m.range.start_column)).collect();
    assert_eq!(ranges, vec![(0, 0), (0, 5), (1, 1), (1, 6)]);
    assert_eq!(file.matches[1].preview().inside, "the");
    let mut result = SearchResult { files: vec![file] };
    result.remove_match(0, 1);
    result.files[0].update_from_lines(&lines, &params);
    assert_eq!(result.files[0].matches.len(), 3);
    assert_eq!(result.count(), 3);
}
