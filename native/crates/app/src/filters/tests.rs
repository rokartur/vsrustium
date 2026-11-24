//! Ported from src/vs/base/test/common/filters.test.ts and iconLabels.test.ts.

use super::*;

fn ok(result: Option<Matches>, expected: &[Range<usize>]) {
    assert_eq!(result.expect("matches"), expected);
}

#[test]
fn prefix_filter_ignore_case() {
    ok(matches_prefix("alpha", "alpha"), &[0..5]);
    ok(matches_prefix("alpha", "alphasomething"), &[0..5]);
    assert!(matches_prefix("alpha", "alp").is_none());
    ok(matches_prefix("a", "alpha"), &[0..1]);
    ok(matches_prefix("ä", "Älpha"), &[0..1]);
    assert!(matches_prefix("x", "alpha").is_none());
    ok(matches_prefix("A", "alpha"), &[0..1]);
    ok(matches_prefix("AlPh", "alPHA"), &[0..4]);
    assert!(matches_prefix("T", "4").is_none());
}

#[test]
fn camel_case_filter() {
    assert!(matches_camel_case("", "").is_none());
    ok(matches_camel_case("", "anything"), &[]);
    ok(matches_camel_case("alpha", "alpha"), &[0..5]);
    ok(matches_camel_case("AlPhA", "alpha"), &[0..5]);
    ok(matches_camel_case("alpha", "alphasomething"), &[0..5]);
    assert!(matches_camel_case("alpha", "alp").is_none());
    ok(matches_camel_case("c", "CamelCaseRocks"), &[0..1]);
    ok(matches_camel_case("cc", "CamelCaseRocks"), &[0..1, 5..6]);
    ok(matches_camel_case("ccr", "CamelCaseRocks"), &[0..1, 5..6, 9..10]);
    ok(matches_camel_case("cacr", "CamelCaseRocks"), &[0..2, 5..6, 9..10]);
    ok(matches_camel_case("cacar", "CamelCaseRocks"), &[0..2, 5..7, 9..10]);
    ok(matches_camel_case("ccarocks", "CamelCaseRocks"), &[0..1, 5..7, 9..14]);
    ok(matches_camel_case("cr", "CamelCaseRocks"), &[0..1, 9..10]);
    ok(matches_camel_case("fba", "FooBarAbe"), &[0..1, 3..5]);
    ok(matches_camel_case("fbar", "FooBarAbe"), &[0..1, 3..6]);
    ok(matches_camel_case("fbara", "FooBarAbe"), &[0..1, 3..7]);
    ok(matches_camel_case("fbaa", "FooBarAbe"), &[0..1, 3..5, 6..7]);
    ok(matches_camel_case("fbaab", "FooBarAbe"), &[0..1, 3..5, 6..8]);
    ok(matches_camel_case("c2d", "canvasCreation2D"), &[0..1, 14..16]);
    ok(matches_camel_case("cce", "_canvasCreationEvent"), &[1..2, 7..8, 15..16]);
}

#[test]
fn camel_case_filter_19256() {
    assert!(matches_camel_case("Debug Console", "Open: Debug Console").is_some());
    assert!(matches_camel_case("Debug console", "Open: Debug Console").is_some());
    assert!(matches_camel_case("debug console", "Open: Debug Console").is_some());
}

#[test]
fn contiguous_sub_string() {
    ok(matches_contiguous_sub_string("cela", "cancelAnimationFrame()"), &[3..7]);
}

#[test]
fn empty_queries_match_empty() {
    for label in ["", "Hello", " Hello", "$(zap)", "$(zap) ", "$(zap) Hello", "$(zap) $(zap) Hello"] {
        ok(matches_fuzzy_icon_aware("", &parse_label_with_icons(label)), &[0..0]);
    }
    ok(matches_fuzzy("", "first"), &[0..0]);
    ok(matches_contiguous_sub_string("", ""), &[0..0]);
}

#[test]
fn fuzzy_wildcards_match_greedily() {
    ok(matches_fuzzy("a*b", "xaxbxb"), &[1..6]);
    ok(matches_fuzzy("A.B", "xa.b"), &[1..4]);
    assert!(matches_fuzzy("a*c", "ab\nc").is_none());
}

#[test]
fn fuzzy_icon_aware() {
    ok(matches_fuzzy_icon_aware("ccr", &parse_label_with_icons("$(codicon)CamelCaseRocks$(codicon)")), &[10..11, 15..16, 19..20]);
    ok(matches_fuzzy_icon_aware("ccr", &parse_label_with_icons("$(codicon) CamelCaseRocks $(codicon)")), &[11..12, 16..17, 20..21]);
    ok(matches_fuzzy_icon_aware("iut", &parse_label_with_icons("$(codicon) Indent $(octico) Using $(octic) Tpaces")), &[11..12, 28..29, 43..44]);
    ok(matches_fuzzy_icon_aware("using", &parse_label_with_icons("$(codicon) Indent Using Spaces")), &[18..23]);
    ok(matches_fuzzy_icon_aware("codicon", &parse_label_with_icons("This $(codicon Indent Using Spaces")), &[7..14]);
    ok(matches_fuzzy_icon_aware("indent", &parse_label_with_icons("This $codicon Indent Using Spaces")), &[14..20]);
    ok(matches_fuzzy_icon_aware("unt", &parse_label_with_icons("$(primitive-dot) $(file-text) Untitled-1")), &[30..33]);
    ok(matches_fuzzy_icon_aware("s", &parse_label_with_icons("$(loading~spin) start")), &[16..17]);
}

#[test]
fn compare_anything_puts_prefix_then_suffix_matches_first() {
    let mut labels = vec!["xfoo", "foobar", "foo", "barfoo2", "bar"];
    labels.sort_by(|a, b| compare_anything(a, b, "foo"));
    assert_eq!(labels, ["foo", "foobar", "xfoo", "bar", "barfoo2"]);
    assert_eq!(compare_file_names("file2", "file10"), Ordering::Less);
}
