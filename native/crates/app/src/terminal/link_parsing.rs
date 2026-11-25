//! Parses possible links out of a terminal line with only the line text, without checking that the
//! paths exist (terminalLinkParsing.ts). Ports the Linux and macOS branches only. Every `index`
//! counts chars, where upstream counts UTF-16 code units: the two differ only outside the BMP.

#![allow(dead_code, reason = "suffix helpers serve the link hover and quick access, not ported yet")]

use std::sync::LazyLock;

use regex::{Captures, Regex, RegexBuilder};

/// `ILinkPartialRange`: `text` starts at char `index` of the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkPartialRange {
    pub index: usize,
    pub text: String,
}

/// `ILinkSuffix`: the line and column info after a path, 1-based as written in the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkSuffix {
    pub row: Option<u32>,
    pub col: Option<u32>,
    pub row_end: Option<u32>,
    pub col_end: Option<u32>,
    pub suffix: LinkPartialRange,
}

/// `IParsedLink`: the path, the quotes before it and its suffix, all underlined as one link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedLink {
    pub path: LinkPartialRange,
    pub prefix: Option<LinkPartialRange>,
    pub suffix: Option<LinkSuffix>,
}

/// `linkSuffixRegexEol`
static LINK_SUFFIX_REGEX_EOL: LazyLock<Regex> = LazyLock::new(|| generate_link_suffix_regex(true));
/// `linkSuffixRegex`
static LINK_SUFFIX_REGEX: LazyLock<Regex> = LazyLock::new(|| generate_link_suffix_regex(false));

/// `generateLinkSuffixRegex`. `\d` is spelled `[0-9]`: Rust's `\d` matches every Unicode digit, JS's
/// only ASCII ones. The clauses list the formats they match in the upstream comments and tests.
fn generate_link_suffix_regex(eol_only: bool) -> Regex {
    let eol = if eol_only { "$" } else { "" };
    let line_and_column_regex_clauses = [
        format!(
            r#"(?::|#| |['"],|, )(?<row0>[0-9]+)([:.](?<col0>[0-9]+)(?:-(?:(?<rowEnd0>[0-9]+)\.)?(?<colEnd0>[0-9]+))?)?{eol}"#
        ),
        format!(
            r#"['"]?(?:,? |: ?| on )lines? (?<row1>[0-9]+)(?:-(?<rowEnd1>[0-9]+))?(?:,? (?:col(?:umn)?|characters?) (?<col1>[0-9]+)(?:-(?<colEnd1>[0-9]+))?)?{eol}"#
        ),
        format!(r":? ?[\[\(](?<row2>[0-9]+)(?:(?:, ?|:)(?<col2>[0-9]+))?[\]\)]{eol}"),
    ];
    let suffix_clause = line_and_column_regex_clauses.join("|").replace(' ', "[\u{A0} ]");
    Regex::new(&format!("({suffix_clause})")).expect("link suffix regex compiles")
}

/// `removeLinkSuffix`
pub fn remove_link_suffix(link: &str) -> &str {
    match LINK_SUFFIX_REGEX_EOL.find(link) {
        Some(suffix) => &link[..suffix.start()],
        None => link,
    }
}

/// `removeLinkQueryString`
pub fn remove_link_query_string(link: &str) -> &str {
    // Skip the ? of a UNC path.
    let start = if link.starts_with(r"\\?\") { 4 } else { 0 };
    match link[start..].find('?') {
        Some(index) => &link[..start + index],
        None => link,
    }
}

/// `detectLinkSuffixes`
pub fn detect_link_suffixes(line: &str) -> Vec<LinkSuffix> {
    LINK_SUFFIX_REGEX.captures_iter(line).map(|captures| to_link_suffix(line, &captures)).collect()
}

/// `getLinkSuffix`
pub fn get_link_suffix(link: &str) -> Option<LinkSuffix> {
    LINK_SUFFIX_REGEX_EOL.captures(link).map(|captures| to_link_suffix(link, &captures))
}

/// `toLinkSuffix`
fn to_link_suffix(line: &str, captures: &Captures) -> LinkSuffix {
    let number = |names: [&str; 3]| {
        let digits = names.iter().find_map(|name| captures.name(name))?;
        // Only overflow fails to parse: the regex matched ASCII digits.
        Some(digits.as_str().parse().unwrap_or(u32::MAX))
    };
    let suffix = captures.get_match();
    LinkSuffix {
        row: number(["row0", "row1", "row2"]),
        col: number(["col0", "col1", "col2"]),
        row_end: number(["rowEnd0", "rowEnd1", "rowEnd2"]),
        col_end: number(["colEnd0", "colEnd1", "colEnd2"]),
        suffix: LinkPartialRange { index: char_index(line, suffix.start()), text: suffix.as_str().to_string() },
    }
}

/// `linkWithSuffixPathCharacters`: the first class lists chars a path cannot start with, the second
/// chars it cannot contain.
static LINK_WITH_SUFFIX_PATH_CHARACTERS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?<path>(?:file:\/\/\/)?[^\s\|<>\[\({][^\s\|<>]*)$").expect("path regex compiles")
});

/// `detectLinks` for Linux and macOS: links with suffixes first, then paths without suffixes that
/// do not overlap them, sorted by position.
pub fn detect_links(line: &str) -> Vec<ParsedLink> {
    let mut results = detect_links_via_suffix(line);
    let no_suffix_paths = detect_paths_no_suffix(line);
    binary_insert_list(&mut results, no_suffix_paths);
    results
}

/// `binaryInsertList`
fn binary_insert_list(list: &mut Vec<ParsedLink>, new_items: Vec<ParsedLink>) {
    if list.is_empty() {
        *list = new_items;
        return;
    }
    for item in new_items {
        let high = list.len() as isize;
        binary_insert(list, item, 0, high);
    }
}

/// `binaryInsert`. Like upstream, an item past the last link is inserted without an overlap check.
fn binary_insert(list: &mut Vec<ParsedLink>, new_item: ParsedLink, low: isize, high: isize) {
    if low > high {
        return;
    }
    let mid = ((low + high) / 2) as usize;
    let start = new_item.path.index;
    if mid >= list.len() || (start < list[mid].path.index && (mid == 0 || start > list[mid - 1].path.index)) {
        let end = start + new_item.path.text.chars().count();
        if mid >= list.len()
            || (end < list[mid].path.index && (mid == 0 || start > end_of(&list[mid - 1].path)))
        {
            list.insert(mid, new_item);
        }
        return;
    }
    if start > list[mid].path.index {
        binary_insert(list, new_item, mid as isize + 1, high);
    } else {
        binary_insert(list, new_item, low, mid as isize - 1);
    }
}

fn end_of(range: &LinkPartialRange) -> usize {
    range.index + range.text.chars().count()
}

/// `detectLinksViaSuffix`
fn detect_links_via_suffix(line: &str) -> Vec<ParsedLink> {
    let mut results = Vec::new();
    for captures in LINK_SUFFIX_REGEX.captures_iter(line) {
        let suffix_match = captures.get_match();
        // A suffix followed by `/` is a numeric Git diff prefix such as `1/`, part of a path.
        if line[suffix_match.end()..].starts_with('/') {
            continue;
        }
        let Some(path_match) = LINK_WITH_SUFFIX_PATH_CHARACTERS.find(&line[..suffix_match.start()]) else {
            continue;
        };
        let mut link_start = path_match.start();
        let mut path = path_match.as_str();
        let mut prefix = None;
        let quotes_len = path.len() - path.trim_start_matches(['\'', '"']).len();
        if quotes_len > 0 {
            let mut prefix_text = &path[..quotes_len];
            path = &path[quotes_len..];
            if path.trim().is_empty() {
                continue;
            }
            // In `echo "'foo' on line 1"` the prefix is `'`, not `"'`: keep only the quote the suffix closes.
            if quotes_len > 1 && suffix_match.as_str().as_bytes()[0] == prefix_text.as_bytes()[quotes_len - 1] {
                link_start += quotes_len - 1;
                prefix_text = &prefix_text[quotes_len - 1..];
            }
            prefix = Some(LinkPartialRange { index: char_index(line, link_start), text: prefix_text.to_string() });
        }
        let path_start = link_start + prefix.as_ref().map_or(0, |prefix| prefix.text.len());
        let suffix = to_link_suffix(line, &captures);
        results.push(ParsedLink {
            path: LinkPartialRange { index: char_index(line, path_start), text: path.to_string() },
            prefix: prefix.clone(),
            suffix: Some(suffix.clone()),
        });

        // `(?<bracket>[\[\(])(?![\]\)])`: each opening bracket not closed right away also starts a path.
        let bytes = path.as_bytes();
        for (i, &byte) in bytes.iter().enumerate() {
            let opens = byte == b'[' || byte == b'(';
            if opens && !matches!(bytes.get(i + 1), Some(b']' | b')')) {
                results.push(ParsedLink {
                    path: LinkPartialRange {
                        index: char_index(line, path_start + i + 1),
                        text: path[i + 1..].to_string(),
                    },
                    prefix: prefix.clone(),
                    suffix: Some(suffix.clone()),
                });
            }
        }
    }
    results
}

/// `unixLocalLinkClause`: paths such as `/foo`, `~/foo`, `./foo`, `../foo` and `foo/bar`. Built
/// with octal escapes on so upstream's `\0` (NUL) keeps its meaning.
static UNIX_LOCAL_LINK: LazyLock<Regex> = LazyLock::new(|| {
    let path_prefix = r"(?:\.\.?|\~|file:\/\/)";
    let path_separator_clause = r"\/";
    // '":; are allowed in paths but often separate them; \\ is out to avoid catastrophic backtracking (#24795).
    let excluded_path_characters_clause = r#"[^\0<>\?\s!`&*()'":;\\]"#;
    let excluded_start_path_characters_clause = r#"[^\0<>\?\s!`&*()\[\]'":;\\]"#;
    let clause = format!(
        "(?:(?:{path_prefix}|(?:{excluded_start_path_characters_clause}{excluded_path_characters_clause}*))?(?:{path_separator_clause}(?:{excluded_path_characters_clause})+)+)"
    );
    RegexBuilder::new(&clause).octal(true).build().expect("unix path regex compiles")
});

/// `gitDiffLineRegex`: `--- a/foo` and `+++ b/foo`, where `diffFilePrefix` covers Git's mnemonic
/// prefixes and the `1/` and `2/` of `git diff --no-index`.
static GIT_DIFF_LINE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[-+]{3} [abciow12]\/").expect("git diff line regex compiles"));
/// `gitDiffTextRegex`: `a/foo` in `diff --git a/foo b/foo`.
static GIT_DIFF_TEXT_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[abciow12]\/").expect("git diff text regex compiles"));

/// `detectPathsNoSuffix` for Linux and macOS.
fn detect_paths_no_suffix(line: &str) -> Vec<ParsedLink> {
    let is_git_diff_line = GIT_DIFF_LINE_REGEX.is_match(line);
    let mut results = Vec::new();
    for path_match in UNIX_LOCAL_LINK.find_iter(line) {
        let mut text = path_match.as_str();
        let mut start = path_match.start();
        if (is_git_diff_line && start == 4) || (line.starts_with("diff --git") && GIT_DIFF_TEXT_REGEX.is_match(text)) {
            text = &text[2..];
            start += 2;
        }
        results.push(ParsedLink {
            path: LinkPartialRange { index: char_index(line, start), text: text.to_string() },
            prefix: None,
            suffix: None,
        });
    }
    results
}

fn char_index(line: &str, byte_index: usize) -> usize {
    line[..byte_index].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_ROW: u32 = 339;
    const TEST_COL: u32 = 12;
    const TEST_ROW_END: u32 = 341;
    const TEST_COL_END: u32 = 789;

    /// `ITestLink`: link, prefix, suffix, hasRow, hasCol, hasRowEnd, hasColEnd.
    type TestLink = (&'static str, Option<&'static str>, Option<&'static str>, bool, bool, bool, bool);

    #[rustfmt::skip]
    const TEST_LINKS: &[TestLink] = &[
        // Simple
        ("foo", None, None, false, false, false, false),
        ("foo:339", None, Some(":339"), true, false, false, false),
        ("foo:339:12", None, Some(":339:12"), true, true, false, false),
        ("foo:339:12-789", None, Some(":339:12-789"), true, true, false, true),
        ("foo:339.12", None, Some(":339.12"), true, true, false, false),
        ("foo:339.12-789", None, Some(":339.12-789"), true, true, false, true),
        ("foo:339.12-341.789", None, Some(":339.12-341.789"), true, true, true, true),
        ("foo#339", None, Some("#339"), true, false, false, false),
        ("foo#339:12", None, Some("#339:12"), true, true, false, false),
        ("foo#339:12-789", None, Some("#339:12-789"), true, true, false, true),
        ("foo#339.12", None, Some("#339.12"), true, true, false, false),
        ("foo#339.12-789", None, Some("#339.12-789"), true, true, false, true),
        ("foo#339.12-341.789", None, Some("#339.12-341.789"), true, true, true, true),
        ("foo 339", None, Some(" 339"), true, false, false, false),
        ("foo 339:12", None, Some(" 339:12"), true, true, false, false),
        ("foo 339:12-789", None, Some(" 339:12-789"), true, true, false, true),
        ("foo 339.12", None, Some(" 339.12"), true, true, false, false),
        ("foo 339.12-789", None, Some(" 339.12-789"), true, true, false, true),
        ("foo 339.12-341.789", None, Some(" 339.12-341.789"), true, true, true, true),
        ("foo, 339", None, Some(", 339"), true, false, false, false),

        // Double quotes
        (r#""foo",339"#, Some("\""), Some(r#"",339"#), true, false, false, false),
        (r#""foo",339:12"#, Some("\""), Some(r#"",339:12"#), true, true, false, false),
        (r#""foo",339.12"#, Some("\""), Some(r#"",339.12"#), true, true, false, false),
        (r#""foo", line 339"#, Some("\""), Some(r#"", line 339"#), true, false, false, false),
        (r#""foo", line 339, col 12"#, Some("\""), Some(r#"", line 339, col 12"#), true, true, false, false),
        (r#""foo", line 339, column 12"#, Some("\""), Some(r#"", line 339, column 12"#), true, true, false, false),
        (r#""foo":line 339"#, Some("\""), Some(r#"":line 339"#), true, false, false, false),
        (r#""foo":line 339, col 12"#, Some("\""), Some(r#"":line 339, col 12"#), true, true, false, false),
        (r#""foo":line 339, column 12"#, Some("\""), Some(r#"":line 339, column 12"#), true, true, false, false),
        (r#""foo": line 339"#, Some("\""), Some(r#"": line 339"#), true, false, false, false),
        (r#""foo": line 339, col 12"#, Some("\""), Some(r#"": line 339, col 12"#), true, true, false, false),
        (r#""foo": line 339, column 12"#, Some("\""), Some(r#"": line 339, column 12"#), true, true, false, false),
        (r#""foo" on line 339"#, Some("\""), Some(r#"" on line 339"#), true, false, false, false),
        (r#""foo" on line 339, col 12"#, Some("\""), Some(r#"" on line 339, col 12"#), true, true, false, false),
        (r#""foo" on line 339, column 12"#, Some("\""), Some(r#"" on line 339, column 12"#), true, true, false, false),
        (r#""foo" line 339"#, Some("\""), Some(r#"" line 339"#), true, false, false, false),
        (r#""foo" line 339 column 12"#, Some("\""), Some(r#"" line 339 column 12"#), true, true, false, false),

        // Single quotes
        ("'foo',339", Some("'"), Some("',339"), true, false, false, false),
        ("'foo',339:12", Some("'"), Some("',339:12"), true, true, false, false),
        ("'foo',339.12", Some("'"), Some("',339.12"), true, true, false, false),
        ("'foo', line 339", Some("'"), Some("', line 339"), true, false, false, false),
        ("'foo', line 339, col 12", Some("'"), Some("', line 339, col 12"), true, true, false, false),
        ("'foo', line 339, column 12", Some("'"), Some("', line 339, column 12"), true, true, false, false),
        ("'foo':line 339", Some("'"), Some("':line 339"), true, false, false, false),
        ("'foo':line 339, col 12", Some("'"), Some("':line 339, col 12"), true, true, false, false),
        ("'foo':line 339, column 12", Some("'"), Some("':line 339, column 12"), true, true, false, false),
        ("'foo': line 339", Some("'"), Some("': line 339"), true, false, false, false),
        ("'foo': line 339, col 12", Some("'"), Some("': line 339, col 12"), true, true, false, false),
        ("'foo': line 339, column 12", Some("'"), Some("': line 339, column 12"), true, true, false, false),
        ("'foo' on line 339", Some("'"), Some("' on line 339"), true, false, false, false),
        ("'foo' on line 339, col 12", Some("'"), Some("' on line 339, col 12"), true, true, false, false),
        ("'foo' on line 339, column 12", Some("'"), Some("' on line 339, column 12"), true, true, false, false),
        ("'foo' line 339", Some("'"), Some("' line 339"), true, false, false, false),
        ("'foo' line 339 column 12", Some("'"), Some("' line 339 column 12"), true, true, false, false),

        // No quotes
        ("foo, line 339", None, Some(", line 339"), true, false, false, false),
        ("foo, line 339, col 12", None, Some(", line 339, col 12"), true, true, false, false),
        ("foo, line 339, column 12", None, Some(", line 339, column 12"), true, true, false, false),
        ("foo:line 339", None, Some(":line 339"), true, false, false, false),
        ("foo:line 339, col 12", None, Some(":line 339, col 12"), true, true, false, false),
        ("foo:line 339, column 12", None, Some(":line 339, column 12"), true, true, false, false),
        ("foo: line 339", None, Some(": line 339"), true, false, false, false),
        ("foo: line 339, col 12", None, Some(": line 339, col 12"), true, true, false, false),
        ("foo: line 339, column 12", None, Some(": line 339, column 12"), true, true, false, false),
        ("foo on line 339", None, Some(" on line 339"), true, false, false, false),
        ("foo on line 339, col 12", None, Some(" on line 339, col 12"), true, true, false, false),
        ("foo on line 339, column 12", None, Some(" on line 339, column 12"), true, true, false, false),
        ("foo line 339", None, Some(" line 339"), true, false, false, false),
        ("foo line 339 column 12", None, Some(" line 339 column 12"), true, true, false, false),

        // Parentheses
        ("foo(339)", None, Some("(339)"), true, false, false, false),
        ("foo(339,12)", None, Some("(339,12)"), true, true, false, false),
        ("foo(339, 12)", None, Some("(339, 12)"), true, true, false, false),
        ("foo (339)", None, Some(" (339)"), true, false, false, false),
        ("foo (339,12)", None, Some(" (339,12)"), true, true, false, false),
        ("foo (339, 12)", None, Some(" (339, 12)"), true, true, false, false),
        ("foo: (339)", None, Some(": (339)"), true, false, false, false),
        ("foo: (339,12)", None, Some(": (339,12)"), true, true, false, false),
        ("foo: (339, 12)", None, Some(": (339, 12)"), true, true, false, false),
        ("foo(339:12)", None, Some("(339:12)"), true, true, false, false),
        ("foo (339:12)", None, Some(" (339:12)"), true, true, false, false),

        // Square brackets
        ("foo[339]", None, Some("[339]"), true, false, false, false),
        ("foo[339,12]", None, Some("[339,12]"), true, true, false, false),
        ("foo[339, 12]", None, Some("[339, 12]"), true, true, false, false),
        ("foo [339]", None, Some(" [339]"), true, false, false, false),
        ("foo [339,12]", None, Some(" [339,12]"), true, true, false, false),
        ("foo [339, 12]", None, Some(" [339, 12]"), true, true, false, false),
        ("foo: [339]", None, Some(": [339]"), true, false, false, false),
        ("foo: [339,12]", None, Some(": [339,12]"), true, true, false, false),
        ("foo: [339, 12]", None, Some(": [339, 12]"), true, true, false, false),
        ("foo[339:12]", None, Some("[339:12]"), true, true, false, false),
        ("foo [339:12]", None, Some(" [339:12]"), true, true, false, false),

        // OCaml-style
        (r#""foo", line 339, character 12"#, Some("\""), Some(r#"", line 339, character 12"#), true, true, false, false),
        (r#""foo", line 339, characters 12-789"#, Some("\""), Some(r#"", line 339, characters 12-789"#), true, true, false, true),
        (r#""foo", lines 339-341"#, Some("\""), Some(r#"", lines 339-341"#), true, false, true, false),
        (r#""foo", lines 339-341, characters 12-789"#, Some("\""), Some(r#"", lines 339-341, characters 12-789"#), true, true, true, true),

        // Non-breaking space
        ("foo\u{A0}339:12", None, Some("\u{A0}339:12"), true, true, false, false),
        ("\"foo\" on line 339,\u{A0}column 12", Some("\""), Some("\" on line 339,\u{A0}column 12"), true, true, false, false),
        ("'foo' on line\u{A0}339, column 12", Some("'"), Some("' on line\u{A0}339, column 12"), true, true, false, false),
        ("foo (339,\u{A0}12)", None, Some(" (339,\u{A0}12)"), true, true, false, false),
        ("foo\u{A0}[339, 12]", None, Some("\u{A0}[339, 12]"), true, true, false, false),
    ];

    const MAC_OS_TEST_PATH: &str = "/test/path/macintosh";
    const LINUX_TEST_PATH: &str = "/test/path/linux";

    fn range(index: usize, text: &str) -> LinkPartialRange {
        LinkPartialRange { index, text: text.to_string() }
    }

    fn suffix(row: Option<u32>, col: Option<u32>, index: usize, text: &str) -> LinkSuffix {
        LinkSuffix { row, col, row_end: None, col_end: None, suffix: range(index, text) }
    }

    fn path_only(index: usize, text: &str) -> ParsedLink {
        ParsedLink { path: range(index, text), prefix: None, suffix: None }
    }

    fn expected_suffix(test_link: &TestLink, index: usize) -> LinkSuffix {
        let &(_, _, suffix, has_row, has_col, has_row_end, has_col_end) = test_link;
        LinkSuffix {
            row: has_row.then_some(TEST_ROW),
            col: has_col.then_some(TEST_COL),
            row_end: has_row_end.then_some(TEST_ROW_END),
            col_end: has_col_end.then_some(TEST_COL_END),
            suffix: range(index, suffix.unwrap()),
        }
    }

    fn len(text: &str) -> usize {
        text.chars().count()
    }

    #[test]
    fn remove_link_suffix_table() {
        for &(link, _, suffix, ..) in TEST_LINKS {
            let expected = match suffix {
                Some(suffix) => link.replacen(suffix, "", 1),
                None => link.to_string(),
            };
            assert_eq!(remove_link_suffix(link), expected, "`{link}`");
        }
    }

    #[test]
    fn get_link_suffix_table() {
        for test_link in TEST_LINKS {
            let (link, _, suffix, ..) = *test_link;
            let expected = suffix.map(|suffix| expected_suffix(test_link, len(link) - len(suffix)));
            assert_eq!(get_link_suffix(link), expected, "`{link}`");
        }
    }

    #[test]
    fn detect_link_suffixes_table() {
        for test_link in TEST_LINKS {
            let (link, _, suffix, ..) = *test_link;
            let expected: Vec<_> =
                suffix.map(|suffix| expected_suffix(test_link, len(link) - len(suffix))).into_iter().collect();
            assert_eq!(detect_link_suffixes(link), expected, "`{link}`");
        }
    }

    #[test]
    fn detect_link_suffixes_foo_1_2_bar_3_4_baz_on_line_5() {
        assert_eq!(
            detect_link_suffixes("foo(1, 2) bar[3, 4] baz on line 5"),
            vec![
                suffix(Some(1), Some(2), 3, "(1, 2)"),
                suffix(Some(3), Some(4), 13, "[3, 4]"),
                suffix(Some(5), None, 23, " on line 5"),
            ]
        );
    }

    #[test]
    fn remove_link_query_string_should_remove_any_query_string_from_the_link() {
        assert_eq!(remove_link_query_string("?a=b"), "");
        assert_eq!(remove_link_query_string("foo?a=b"), "foo");
        assert_eq!(remove_link_query_string("./foo?a=b"), "./foo");
        assert_eq!(remove_link_query_string("/foo/bar?a=b"), "/foo/bar");
        assert_eq!(remove_link_query_string("foo?a=b?"), "foo");
        assert_eq!(remove_link_query_string("foo?a=b&c=d"), "foo");
    }

    #[test]
    fn remove_link_query_string_should_respect_question_mark_in_unc_paths() {
        assert_eq!(remove_link_query_string(r"\\?\foo?a=b"), r"\\?\foo");
    }

    #[test]
    fn detect_links_foo_1_2_bar_3_4_quoted_baz_on_line_5() {
        assert_eq!(
            detect_links(r#"foo(1, 2) bar[3, 4] "baz" on line 5"#),
            vec![
                ParsedLink { path: range(0, "foo"), prefix: None, suffix: Some(suffix(Some(1), Some(2), 3, "(1, 2)")) },
                ParsedLink { path: range(10, "bar"), prefix: None, suffix: Some(suffix(Some(3), Some(4), 13, "[3, 4]")) },
                ParsedLink {
                    path: range(21, "baz"),
                    prefix: Some(range(20, "\"")),
                    suffix: Some(suffix(Some(5), None, 24, "\" on line 5")),
                },
            ]
        );
    }

    #[test]
    fn detect_links_should_detect_multiple_links_when_opening_brackets_are_in_the_text() {
        assert_eq!(
            detect_links("notlink[foo:45]"),
            vec![
                ParsedLink { path: range(0, "notlink[foo"), prefix: None, suffix: Some(suffix(Some(45), None, 11, ":45")) },
                ParsedLink { path: range(8, "foo"), prefix: None, suffix: Some(suffix(Some(45), None, 11, ":45")) },
            ]
        );
    }

    #[test]
    fn detect_links_should_extract_the_link_prefix() {
        assert_eq!(
            detect_links(r#""foo", line 5, col 6"#),
            vec![ParsedLink {
                path: range(1, "foo"),
                prefix: Some(range(0, "\"")),
                suffix: Some(suffix(Some(5), Some(6), 4, "\", line 5, col 6")),
            }]
        );
    }

    #[test]
    fn detect_links_should_be_smart_about_determining_the_link_prefix_when_multiple_prefix_characters_exist() {
        assert_eq!(
            detect_links(r#"echo '"foo", line 5, col 6'"#),
            vec![ParsedLink {
                path: range(7, "foo"),
                prefix: Some(range(6, "\"")),
                suffix: Some(suffix(Some(5), Some(6), 10, "\", line 5, col 6")),
            }],
            "The outer single quotes should be excluded from the link prefix and suffix"
        );
    }

    #[test]
    fn detect_links_angle_brackets_should_exclude_bracket_characters_from_link_paths() {
        for path in [LINUX_TEST_PATH, MAC_OS_TEST_PATH] {
            assert_eq!(detect_links(&format!("<{path}<")), vec![path_only(1, path)]);
            assert_eq!(detect_links(&format!(">{path}>")), vec![path_only(1, path)]);
        }
    }

    #[test]
    fn detect_links_angle_brackets_should_exclude_bracket_characters_from_link_paths_with_suffixes() {
        for path in [LINUX_TEST_PATH, MAC_OS_TEST_PATH] {
            let expected = vec![ParsedLink {
                path: range(1, path),
                prefix: None,
                suffix: Some(suffix(Some(400), None, 1 + len(path), ":400")),
            }];
            assert_eq!(detect_links(&format!("<{path}:400<")), expected);
            assert_eq!(detect_links(&format!(">{path}:400>")), expected);
        }
    }

    #[test]
    fn detect_links_query_strings_should_exclude_query_strings_from_link_paths() {
        for path in [LINUX_TEST_PATH, MAC_OS_TEST_PATH] {
            assert_eq!(detect_links(&format!("{path}?a=b")), vec![path_only(0, path)]);
            assert_eq!(detect_links(&format!("{path}?a=b&c=d")), vec![path_only(0, path)]);
        }
    }

    #[test]
    fn detect_links_query_strings_should_not_detect_links_starting_with_question_mark_204195() {
        for line in ["http://foo.com/?bar=/a/b&baz=c", r"http://foo.com/?bar=a:\b&baz=c"] {
            assert!(!detect_links(line).iter().any(|link| link.path.text.starts_with('?')), "{line}");
        }
    }

    #[test]
    fn detect_links_git_diffs_minus_minus_minus_a_foo_bar() {
        for prefix in ["a", "c", "w", "i", "o"] {
            assert_eq!(detect_links(&format!("--- {prefix}/foo/bar")), vec![path_only(6, "foo/bar")]);
        }
    }

    #[test]
    fn detect_links_git_diffs_plus_plus_plus_b_foo_bar() {
        for prefix in ["b", "c", "w", "i", "o"] {
            assert_eq!(detect_links(&format!("+++ {prefix}/foo/bar")), vec![path_only(6, "foo/bar")]);
        }
    }

    #[test]
    fn detect_links_git_diffs_diff_git_a_foo_bar_b_foo_baz() {
        for (source, destination) in [("a", "b"), ("c", "w"), ("i", "o")] {
            assert_eq!(
                detect_links(&format!("diff --git {source}/foo/bar {destination}/foo/baz")),
                vec![path_only(13, "foo/bar"), path_only(23, "foo/baz")]
            );
        }
    }

    #[test]
    fn detect_links_git_diffs_numeric_prefixes_used_by_git_diff_no_index() {
        assert_eq!(detect_links("--- 1/foo/bar"), vec![path_only(6, "foo/bar")]);
        assert_eq!(detect_links("+++ 2/foo/baz"), vec![path_only(6, "foo/baz")]);
        assert_eq!(
            detect_links("diff --git 1/foo/bar 2/foo/baz"),
            vec![path_only(13, "foo/bar"), path_only(23, "foo/baz")]
        );
    }

    #[test]
    fn detect_links_git_diffs_reversed_numeric_prefixes_used_by_git_diff_no_index_r() {
        assert_eq!(detect_links("--- 2/foo/baz"), vec![path_only(6, "foo/baz")]);
        assert_eq!(detect_links("+++ 1/foo/bar"), vec![path_only(6, "foo/bar")]);
        assert_eq!(
            detect_links("diff --git 2/foo/baz 1/foo/bar"),
            vec![path_only(13, "foo/baz"), path_only(23, "foo/bar")]
        );
    }

    #[test]
    fn detect_links_git_diffs_ordinary_numeric_line_suffix() {
        assert_eq!(
            detect_links("foo 1"),
            vec![ParsedLink { path: range(0, "foo"), prefix: None, suffix: Some(suffix(Some(1), None, 3, " 1")) }]
        );
    }

    #[test]
    fn detect_links_git_diffs_numeric_suffix_followed_by_a_path_separator() {
        assert_eq!(detect_links("foo 1/bar"), vec![path_only(4, "1/bar")]);
    }

    #[test]
    fn detect_links_git_diffs_ordinary_numeric_line_suffix_after_diff_git_text() {
        assert_eq!(
            detect_links("diff --git foo.ts:123"),
            vec![ParsedLink {
                path: range(11, "foo.ts"),
                prefix: None,
                suffix: Some(suffix(Some(123), None, 17, ":123")),
            }]
        );
    }

    #[test]
    fn detect_links_should_detect_3_suffix_links_on_a_single_line() {
        let with_suffix: Vec<&TestLink> = TEST_LINKS.iter().filter(|test_link| test_link.2.is_some()).collect();
        for window in with_suffix.windows(3) {
            let line = format!(" {} {} {} ", window[0].0, window[1].0, window[2].0);
            let mut expected = Vec::new();
            let mut start = 1;
            for test_link in window {
                let (link, prefix, suffix, ..) = **test_link;
                let suffix = suffix.unwrap();
                let prefix_len = prefix.map_or(0, len);
                expected.push(ParsedLink {
                    prefix: prefix.map(|prefix| range(start, prefix)),
                    path: range(
                        start + prefix_len,
                        &link.replacen(suffix, "", 1).replacen(prefix.unwrap_or(""), "", 1),
                    ),
                    suffix: Some(expected_suffix(test_link, start + len(link) - len(suffix))),
                });
                start += len(link) + 1;
            }
            assert_eq!(detect_links(&line), expected, "`{}`", line.replace('\u{A0}', "<nbsp>"));
        }
    }

    #[test]
    fn detect_links_should_ignore_links_with_suffixes_when_the_path_itself_is_the_empty_string() {
        assert_eq!(detect_links("\"\"\",1"), vec![]);
    }
}
