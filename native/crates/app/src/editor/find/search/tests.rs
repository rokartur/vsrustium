//! textModelSearch.test.ts. Expected ranges keep upstream's 1-based columns; every text here is in the
//! BMP, so UTF-16 and char columns agree except in the surrogate pair test. The CRLF runs of
//! `assertFindMatches` are left out: lines hold no line break, every search joins them by "\n".

use super::*;
use crate::editor::WORD_SEPARATORS;

fn lines(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_string).collect()
}

fn range(start_line: usize, start_column: usize, end_line: usize, end_column: usize) -> Range<Position> {
    Position::new(start_line - 1, start_column - 1)..Position::new(end_line - 1, end_column - 1)
}

fn params<'a>(search_string: &'a str, is_regex: bool, match_case: bool, word_separators: Option<&'static str>) -> SearchParams<'a> {
    SearchParams { search_string, is_regex, match_case, word_separators }
}

fn found(range: Range<Position>, matches: Option<&[&str]>) -> Option<FindMatch> {
    Some(FindMatch { range, matches: matches.map(|matches| matches.iter().map(|m| m.to_string()).collect()) })
}

fn full_range(lines: &[String]) -> Range<Position> {
    Position::default()..last_position(lines)
}

fn assert_find_matches_in(lines: &[String], params: &SearchParams, expected: &[Range<Position>]) {
    let expected: Vec<FindMatch> = expected.iter().map(|range| FindMatch { range: range.clone(), matches: None }).collect();
    assert_eq!(find_matches(lines, params, full_range(lines), false, 1000), expected, "findMatches OK");

    assert_eq!(find_next_match(lines, params, Position::default(), false).as_ref(), expected.first(), "findNextMatch (1,1)");
    for expected_match in &expected {
        let start = expected_match.range.start;
        assert_eq!(find_next_match(lines, params, start, false).as_ref(), Some(expected_match), "findNextMatch {start:?}");
    }

    let end = last_position(lines);
    assert_eq!(find_previous_match(lines, params, end, false).as_ref(), expected.last(), "findPrevMatch {end:?}");
    for expected_match in &expected {
        let start = expected_match.range.end;
        assert_eq!(find_previous_match(lines, params, start, false).as_ref(), Some(expected_match), "findPrevMatch {start:?}");
    }
}

fn assert_find_matches(text: &str, search_string: &str, is_regex: bool, match_case: bool, word_separators: Option<&'static str>, expected: &[[usize; 4]]) {
    let expected: Vec<_> = expected.iter().map(|&[a, b, c, d]| range(a, b, c, d)).collect();
    assert_find_matches_in(&lines(text), &params(search_string, is_regex, match_case, word_separators), &expected);
}

const REGULAR_TEXT: &str = "This is some foo - bar text which contains foo and bar - as in Barcelona.\n\
Now it begins a word fooBar and now it is caps Foo-isn't this great?\n\
And here's a dull line with nothing interesting in it\n\
It is also interesting if it's part of a word like amazingFooBar\n\
Again nothing interesting here";

const SEPARATORS: Option<&str> = Some(WORD_SEPARATORS);

#[test]
fn simple_find() {
    assert_find_matches(REGULAR_TEXT, "foo", false, false, None, &[[1, 14, 1, 17], [1, 44, 1, 47], [2, 22, 2, 25], [2, 48, 2, 51], [4, 59, 4, 62]]);
}

#[test]
fn case_sensitive_find() {
    assert_find_matches(REGULAR_TEXT, "foo", false, true, None, &[[1, 14, 1, 17], [1, 44, 1, 47], [2, 22, 2, 25]]);
}

#[test]
fn whole_words_find() {
    assert_find_matches(REGULAR_TEXT, "foo", false, false, SEPARATORS, &[[1, 14, 1, 17], [1, 44, 1, 47], [2, 48, 2, 51]]);
}

#[test]
fn line_start_find() {
    assert_find_matches(REGULAR_TEXT, "^", true, false, None, &[[1, 1, 1, 1], [2, 1, 2, 1], [3, 1, 3, 1], [4, 1, 4, 1], [5, 1, 5, 1]]);
}

#[test]
fn line_end_find() {
    assert_find_matches(REGULAR_TEXT, "$", true, false, None, &[[1, 74, 1, 74], [2, 69, 2, 69], [3, 54, 3, 54], [4, 65, 4, 65], [5, 31, 5, 31]]);
}

#[test]
fn dot_star_find() {
    assert_find_matches(REGULAR_TEXT, ".*", true, false, None, &[[1, 1, 1, 74], [2, 1, 2, 69], [3, 1, 3, 54], [4, 1, 4, 65], [5, 1, 5, 31]]);
}

#[test]
fn empty_line_find() {
    let text = "This is some foo - bar text which contains foo and bar - as in Barcelona.\n\nAnd here's a dull line with nothing interesting in it\n\nAgain nothing interesting here";
    assert_find_matches(text, "^$", true, false, None, &[[2, 1, 2, 1], [4, 1, 4, 1]]);
}

const JUST_SOME_TEXT: &str = "Just some text text\nJust some text text\nsome text again\nagain some text";

#[test]
fn multiline_find_1() {
    assert_find_matches(JUST_SOME_TEXT, "text\\n", true, false, None, &[[1, 16, 2, 1], [2, 16, 3, 1]]);
}

#[test]
fn multiline_find_2() {
    assert_find_matches(JUST_SOME_TEXT, "text\\nJust", true, false, None, &[[1, 16, 2, 5]]);
}

#[test]
fn multiline_find_3() {
    assert_find_matches(JUST_SOME_TEXT, "\\nagain", true, false, None, &[[3, 16, 4, 6]]);
}

#[test]
fn multiline_find_4() {
    assert_find_matches(JUST_SOME_TEXT, ".*\\nJust.*\\n", true, false, None, &[[1, 1, 3, 1]]);
}

#[test]
fn multiline_find_with_line_beginning_regex() {
    assert_find_matches("if\nelse\n\nif\nelse", "^if\\nelse", true, false, None, &[[1, 1, 2, 5], [4, 1, 5, 5]]);
}

#[test]
fn matching_empty_lines_using_boundary_expression() {
    assert_find_matches("if\n\nelse\n  \nif\n \nelse", "^\\s*$\\n", true, false, None, &[[2, 1, 3, 1], [4, 1, 5, 1], [6, 1, 7, 1]]);
}

#[test]
fn matching_lines_starting_with_a_and_ending_with_b() {
    assert_find_matches("a if b\na\nab\neb", "^a.*b$", true, false, None, &[[1, 1, 1, 7], [3, 1, 3, 3]]);
}

#[test]
fn multiline_find_with_line_ending_regex() {
    assert_find_matches("if\nelse\n\nif\nelseif\nelse", "if\\nelse$", true, false, None, &[[1, 1, 2, 5], [5, 5, 6, 5]]);
}

#[test]
fn issue_4836_whole_lines() {
    let text = "Just some text text\n\nsome text again\n\nagain some text";
    assert_find_matches(text, "^.*$", true, false, None, &[[1, 1, 1, 20], [2, 1, 2, 1], [3, 1, 3, 16], [4, 1, 4, 1], [5, 1, 5, 16]]);
}

#[test]
fn multiline_find_for_non_regex_string() {
    let text = "Just some text text\nsome text text\nsome text again\nagain some text\nbut not some";
    assert_find_matches(text, "text\nsome", false, false, None, &[[1, 16, 2, 5], [2, 11, 3, 5]]);
}

#[test]
fn issue_3623_match_whole_word_for_non_latin_characters() {
    assert_find_matches("я\nкомпилятор\nобфускация\n:я-я", "я", false, false, SEPARATORS, &[[1, 1, 1, 2], [4, 2, 4, 3], [4, 4, 4, 5]]);
}

#[test]
fn issue_27459_match_whole_words_regression() {
    let text = "this._register(this._textAreaInput.onKeyDown((e: IKeyboardEvent) => {\n\tthis._viewController.emitKeyDown(e);\n}));";
    assert_find_matches(text, "((e: ", false, false, SEPARATORS, &[[1, 45, 1, 50]]);
}

#[test]
fn issue_27594_search_results_disappear() {
    assert_find_matches("this.server.listen(0);", "listen(", false, false, SEPARATORS, &[[1, 13, 1, 20]]);
}

#[test]
fn find_next_match_without_regex() {
    let lines = lines("line line one\nline two\nthree");
    let params = params("line", false, false, None);
    let actual = find_next_match(&lines, &params, Position::new(0, 0), false);
    assert_eq!(actual, found(range(1, 1, 1, 5), None));
    let actual = find_next_match(&lines, &params, actual.unwrap().range.end, false);
    assert_eq!(actual, found(range(1, 6, 1, 10), None));
    let actual = find_next_match(&lines, &params, Position::new(0, 2), false);
    assert_eq!(actual, found(range(1, 6, 1, 10), None));
    let actual = find_next_match(&lines, &params, actual.unwrap().range.end, false);
    assert_eq!(actual, found(range(2, 1, 2, 5), None));
    let actual = find_next_match(&lines, &params, actual.unwrap().range.end, false);
    assert_eq!(actual, found(range(1, 1, 1, 5), None));
}

#[test]
fn find_next_match_with_beginning_boundary_regex() {
    for text in ["line one\nline two\nthree", "line line one\nline two\nthree"] {
        let lines = lines(text);
        let params = params("^line", true, false, None);
        let actual = find_next_match(&lines, &params, Position::new(0, 0), false);
        assert_eq!(actual, found(range(1, 1, 1, 5), None));
        let actual = find_next_match(&lines, &params, actual.unwrap().range.end, false);
        assert_eq!(actual, found(range(2, 1, 2, 5), None));
        let actual = find_next_match(&lines, &params, Position::new(0, 2), false);
        assert_eq!(actual, found(range(2, 1, 2, 5), None));
        let actual = find_next_match(&lines, &params, actual.unwrap().range.end, false);
        assert_eq!(actual, found(range(1, 1, 1, 5), None));
    }
}

#[test]
fn find_next_match_with_beginning_boundary_multiline_regex_and_repetitive_beginnings() {
    let lines = lines("line line one\nline two\nline three\nline four");
    let params = params("^line.*\\nline", true, false, None);
    let actual = find_next_match(&lines, &params, Position::new(0, 0), false);
    assert_eq!(actual, found(range(1, 1, 2, 5), None));
    let actual = find_next_match(&lines, &params, actual.unwrap().range.end, false);
    assert_eq!(actual, found(range(3, 1, 4, 5), None));
    let actual = find_next_match(&lines, &params, Position::new(1, 0), false);
    assert_eq!(actual, found(range(2, 1, 3, 5), None));
}

#[test]
fn find_next_match_with_ending_boundary_regex() {
    let lines = lines("one line line\ntwo line\nthree");
    let params = params("line$", true, false, None);
    let actual = find_next_match(&lines, &params, Position::new(0, 0), false);
    assert_eq!(actual, found(range(1, 10, 1, 14), None));
    let actual = find_next_match(&lines, &params, Position::new(0, 3), false);
    assert_eq!(actual, found(range(1, 10, 1, 14), None));
    let actual = find_next_match(&lines, &params, actual.unwrap().range.end, false);
    assert_eq!(actual, found(range(2, 5, 2, 9), None));
    let actual = find_next_match(&lines, &params, actual.unwrap().range.end, false);
    assert_eq!(actual, found(range(1, 10, 1, 14), None));
}

const ONE_LINE_LINE: &str = "one line line\ntwo line\nthree";
const LINE_GROUPS: &[&str] = &["line", "line", "in"];
const LINE_BREAK_GROUPS: &[&str] = &["line\n", "line", "in"];

#[test]
fn find_matches_with_capturing_matches() {
    let lines = lines(ONE_LINE_LINE);
    let actual = find_matches(&lines, &params("(l(in)e)", true, false, None), full_range(&lines), true, 100);
    let expected: Vec<_> = [range(1, 5, 1, 9), range(1, 10, 1, 14), range(2, 5, 2, 9)].into_iter().map(|range| found(range, Some(LINE_GROUPS)).unwrap()).collect();
    assert_eq!(actual, expected);
}

#[test]
fn find_matches_multiline_with_capturing_matches() {
    let lines = lines(ONE_LINE_LINE);
    let actual = find_matches(&lines, &params("(l(in)e)\\n", true, false, None), full_range(&lines), true, 100);
    let expected: Vec<_> = [range(1, 10, 2, 1), range(2, 5, 3, 1)].into_iter().map(|range| found(range, Some(LINE_BREAK_GROUPS)).unwrap()).collect();
    assert_eq!(actual, expected);
}

#[test]
fn find_next_and_previous_match_with_capturing_matches() {
    let lines = lines(ONE_LINE_LINE);
    let start = Position::new(0, 0);
    assert_eq!(find_next_match(&lines, &params("(l(in)e)", true, false, None), start, true), found(range(1, 5, 1, 9), Some(LINE_GROUPS)));
    assert_eq!(find_next_match(&lines, &params("(l(in)e)\\n", true, false, None), start, true), found(range(1, 10, 2, 1), Some(LINE_BREAK_GROUPS)));
    assert_eq!(find_previous_match(&lines, &params("(l(in)e)", true, false, None), start, true), found(range(2, 5, 2, 9), Some(LINE_GROUPS)));
    assert_eq!(find_previous_match(&lines, &params("(l(in)e)\\n", true, false, None), start, true), found(range(2, 5, 3, 1), Some(LINE_BREAK_GROUPS)));
}

#[test]
fn line_feed_matches_a_crlf_line_break() {
    let lines = lines("a\nb\nc\nd\ne\nf\ng\nh\ni");
    let first = |search: &str| find_matches(&lines, &params(search, true, false, None), full_range(&lines), true, 1000).into_iter().next();
    assert_eq!(first("h\\n"), found(range(8, 1, 9, 1), Some(&["h\n"])));
    assert_eq!(first("g\\nh\\n"), found(range(7, 1, 9, 1), Some(&["g\nh\n"])));
    assert_eq!(first("\\ni"), found(range(8, 2, 9, 2), Some(&["\ni"])));
}

#[test]
fn carriage_return_can_never_be_found() {
    let lines = lines("a\nb\nc\nd\ne\nf\ng\nh\ni");
    let params = params("\\r\\n", true, false, None);
    assert_eq!(find_next_match(&lines, &params, Position::new(0, 0), true), None);
    assert_eq!(find_matches(&lines, &params, full_range(&lines), true, 1000), Vec::new());
}

fn assert_parse_search_result(search_string: &str, is_regex: bool, match_case: bool, word_separators: Option<&'static str>, expected: Option<(&str, bool, Option<&str>)>) {
    let actual = params(search_string, is_regex, match_case, word_separators).parse_search_request();
    let Some((source, multiline, simple_search)) = expected else {
        assert!(actual.is_none(), "{search_string}");
        return;
    };
    let actual = actual.unwrap();
    assert_eq!((actual.source.as_str(), actual.multiline, actual.ignore_case, actual.simple_search.as_deref()), (source, multiline, !match_case, simple_search), "{search_string}");
    assert_eq!(actual.word_separators.is_some(), word_separators.is_some());
}

#[test]
fn parse_search_request_invalid() {
    assert_parse_search_result("", true, true, SEPARATORS, None);
    assert_parse_search_result("(", true, false, None, None);
}

#[test]
fn parse_search_request_non_regex() {
    assert_parse_search_result("foo", false, false, None, Some(("foo", false, None)));
    assert_parse_search_result("foo", false, false, SEPARATORS, Some(("foo", false, None)));
    assert_parse_search_result("foo", false, true, None, Some(("foo", false, Some("foo"))));
    assert_parse_search_result("foo", false, true, SEPARATORS, Some(("foo", false, Some("foo"))));
    assert_parse_search_result("foo\\n", false, false, None, Some(("foo\\\\n", false, None)));
    assert_parse_search_result("foo\\\\n", false, false, None, Some(("foo\\\\\\\\n", false, None)));
    assert_parse_search_result("foo\\r", false, false, None, Some(("foo\\\\r", false, None)));
    assert_parse_search_result("foo\\\\r", false, false, None, Some(("foo\\\\\\\\r", false, None)));
}

#[test]
fn parse_search_request_regex() {
    assert_parse_search_result("foo", true, false, None, Some(("foo", false, None)));
    assert_parse_search_result("foo", true, false, SEPARATORS, Some(("foo", false, None)));
    assert_parse_search_result("foo", true, true, None, Some(("foo", false, None)));
    assert_parse_search_result("foo", true, true, SEPARATORS, Some(("foo", false, None)));
    assert_parse_search_result("foo\\n", true, false, None, Some(("foo\\n", true, None)));
    assert_parse_search_result("foo\\\\n", true, false, None, Some(("foo\\\\n", false, None)));
    assert_parse_search_result("foo\\r", true, false, None, Some(("foo\\r", true, None)));
    assert_parse_search_result("foo\\\\r", true, false, None, Some(("foo\\\\r", false, None)));
}

#[test]
fn issue_53415_non_word_matches_line_break() {
    assert_find_matches("text\n180702-\n180703-180704", "\\d{6}-\\W", true, false, None, &[[2, 1, 3, 1]]);
    assert_find_matches("Just some text\n\nJust", "\\W", true, false, None, &[[1, 5, 1, 6], [1, 10, 1, 11], [1, 15, 2, 1], [2, 1, 3, 1]]);
    assert_find_matches("Just some text\n\tJust\nJust", "\\W", true, false, None, &[[1, 5, 1, 6], [1, 10, 1, 11], [1, 15, 2, 1], [2, 1, 2, 2], [2, 6, 3, 1]]);
    assert_find_matches("Just  some text\n\nJust", "\\W{2}", true, false, None, &[[1, 5, 1, 7], [1, 16, 3, 1]]);
}

#[test]
fn simple_find_using_unicode_escape_sequences() {
    assert_find_matches(REGULAR_TEXT, "\\u{0066}\\u006f\\u006F", true, false, None, &[[1, 14, 1, 17], [1, 44, 1, 47], [2, 22, 2, 25], [2, 48, 2, 51], [4, 59, 4, 62]]);
    // Not upstream: the escape rewrite must not slice inside a multi-byte char.
    params("\\é\\u{}\\u12", true, true, None).parse_search_request();
}

#[test]
fn is_multiline_regex_source_() {
    for single_line in ["foo", "", "foo\\sbar", "\\\\notnewline", "MARK:\\s*(?<label>.*)$", "^// Header$", "\\s*[-=]+\\s*"] {
        assert!(!is_multiline_regex_source(single_line), "{single_line}");
    }
    let multi_line = ["foo\\nbar", "foo\\nbar\\s", "foo\\r\\n", "\\n", "foo\\W", "foo\n", "foo\r\n", "^// =+\\n^// (?<label>[^\\n]+?)\\n^// =+$", "header\\r\\nfooter", "start\\r|\\nend", "top\nmiddle\r\nbottom"];
    for pattern in multi_line {
        assert!(is_multiline_regex_source(pattern), "{pattern}");
    }
}

#[test]
fn issue_74715_digits_star_finds_empty_string_and_keeps_searching() {
    let lines = lines("10.243.30.10");
    let actual = find_matches(&lines, &params("\\d*", true, false, None), full_range(&lines), true, 100);
    let expected = [([1, 3], "10"), ([3, 3], ""), ([4, 7], "243"), ([7, 7], ""), ([8, 10], "30"), ([10, 10], ""), ([11, 13], "10")];
    let expected: Vec<_> = expected.iter().map(|&([start, end], text)| found(range(1, start, 1, end), Some(&[text])).unwrap()).collect();
    assert_eq!(actual, expected);
}

/// Upstream expects UTF-16 columns 1, 2, 4, 5 (and 1, 2, 4, 5, 7, 8): no match inside a surrogate pair.
#[test]
fn issue_100134_zero_length_matches_step_over_surrogate_pairs() {
    assert_find_matches("1\u{1F4BB}1", "()", true, false, None, &[[1, 1, 1, 1], [1, 2, 1, 2], [1, 3, 1, 3], [1, 4, 1, 4]]);
    let hacker_cat = "1\u{1F431}\u{200D}\u{1F4BB}1";
    assert_find_matches(hacker_cat, "()", true, false, None, &[[1, 1, 1, 1], [1, 2, 1, 2], [1, 3, 1, 3], [1, 4, 1, 4], [1, 5, 1, 5], [1, 6, 1, 6]]);
}
