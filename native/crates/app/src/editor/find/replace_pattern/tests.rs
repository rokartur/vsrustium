//! replacePattern.test.ts.

use super::*;
use ReplacePiece::{Group, Static};

fn s(value: &str) -> ReplacePiece {
    Static(value.to_string())
}

fn group(index: usize) -> ReplacePiece {
    Group(index, Vec::new())
}

fn test_parse(input: &str, expected: Vec<ReplacePiece>) {
    assert_eq!(parse_replace_string(input), ReplacePattern::new(expected), "Parsing {input}");
}

/// `regex.exec(target)` with JavaScript's `undefined` groups as "".
fn exec(target: &str, regex: &str) -> Vec<String> {
    let regex = onig::Regex::new(regex).unwrap();
    let captures = regex.captures(target).unwrap();
    (0..captures.len()).map(|index| captures.at(index).unwrap_or("").to_string()).collect()
}

fn assert_replace(target: &str, search: &str, replace: &str, expected: &str) {
    let matches = exec(target, search);
    assert_eq!(parse_replace_string(replace).build(Some(&matches), false), expected, "{target}.replace({search}, {replace})");
}

#[test]
fn parse_replace_string_() {
    test_parse("hello", vec![s("hello")]);
    test_parse("\\thello", vec![s("\thello")]);
    test_parse("h\\tello", vec![s("h\tello")]);
    test_parse("hello\\t", vec![s("hello\t")]);
    test_parse("\\nhello", vec![s("\nhello")]);
    test_parse("\\\\thello", vec![s("\\thello")]);
    test_parse("h\\\\tello", vec![s("h\\tello")]);
    test_parse("hello\\\\t", vec![s("hello\\t")]);
    test_parse("\\\\\\thello", vec![s("\\\thello")]);
    test_parse("\\\\\\\\thello", vec![s("\\\\thello")]);
    test_parse("hello\\", vec![s("hello\\")]);
    test_parse("hello\\x", vec![s("hello\\x")]);
    test_parse("hello\\0", vec![s("hello\\0")]);
    test_parse("hello$&", vec![s("hello"), group(0)]);
    test_parse("hello$0", vec![s("hello"), group(0)]);
    test_parse("hello$02", vec![s("hello"), group(0), s("2")]);
    test_parse("hello$1", vec![s("hello"), group(1)]);
    test_parse("hello$2", vec![s("hello"), group(2)]);
    test_parse("hello$9", vec![s("hello"), group(9)]);
    test_parse("$9hello", vec![group(9), s("hello")]);
    test_parse("hello$12", vec![s("hello"), group(12)]);
    test_parse("hello$99", vec![s("hello"), group(99)]);
    test_parse("hello$99a", vec![s("hello"), group(99), s("a")]);
    test_parse("hello$1a", vec![s("hello"), group(1), s("a")]);
    test_parse("hello$100", vec![s("hello"), group(10), s("0")]);
    test_parse("hello$100a", vec![s("hello"), group(10), s("0a")]);
    test_parse("hello$10a0", vec![s("hello"), group(10), s("a0")]);
    test_parse("hello$$", vec![s("hello$")]);
    test_parse("hello$$0", vec![s("hello$0")]);
    test_parse("hello$`", vec![s("hello$`")]);
    test_parse("hello$'", vec![s("hello$'")]);
}

#[test]
fn parse_replace_string_with_case_modifiers() {
    test_parse("hello\\U$1", vec![s("hello"), Group(1, vec!['U'])]);
    assert_replace("func privateFunc(", r"func (\w+)\(", "func \\U$1(", "func PRIVATEFUNC(");
    test_parse("hello\\u$1", vec![s("hello"), Group(1, vec!['u'])]);
    assert_replace("func privateFunc(", r"func (\w+)\(", "func \\u$1(", "func PrivateFunc(");
    test_parse("hello\\L$1", vec![s("hello"), Group(1, vec!['L'])]);
    assert_replace("func privateFunc(", r"func (\w+)\(", "func \\L$1(", "func privatefunc(");
    test_parse("hello\\l$1", vec![s("hello"), Group(1, vec!['l'])]);
    assert_replace("func PrivateFunc(", r"func (\w+)\(", "func \\l$1(", "func privateFunc(");
    test_parse("hello$1\\u\\u\\U$4goodbye", vec![s("hello"), group(1), Group(4, vec!['u', 'u', 'U']), s("goodbye")]);
    assert_replace("hellogooDbye", r"hello(\w+)", "hello\\u\\u\\l\\l\\U$1", "helloGOodBYE");
}

#[test]
fn replace_has_javascript_semantics() {
    // The expected values are what 'hi'.replace(regex, replace) gives in JavaScript.
    assert_replace("hi", "hi", "hello", "hello");
    assert_replace("hi", "hi", "\\t", "\t");
    assert_replace("hi", "hi", "\\n", "\n");
    assert_replace("hi", "hi", "\\\\t", "\\t");
    assert_replace("hi", "hi", "\\\\n", "\\n");
    assert_replace("hi", "hi", "hello$&", "hellohi");
    assert_replace("hi", "hi", "hello$0", "hellohi");
    assert_replace("hi", "hi", "hello$&1", "hellohi1");
    assert_replace("hi", "hi", "hello$01", "hellohi1");
    assert_replace("hi", "(hi)", "hello$10", "hellohi0");
    assert_replace("hi", "(hi)()()()()()()()()()", "hello$10", "hello");
    assert_replace("hi", "(hi)", "hello$100", "hellohi00");
    assert_replace("hi", "(hi)", "hello$20", "hello$20");
}

#[test]
fn get_replace_string_if_given_text_is_a_complete_match() {
    assert_replace("bla", "bla", "hello", "hello");
    assert_replace("bla", "(bla)", "hello", "hello");
    assert_replace("bla", "(bla)", "hello$0", "hellobla");
    let search = r#"let\s+(\w+)\s*=\s*require\s*\(\s*['"]([\w\.\-/]+)\s*['"]\s*\)\s*"#;
    assert_replace("let fs = require('fs')", search, "import * as $1 from '$2';", "import * as fs from 'fs';");
    assert_replace("let something = require('fs')", search, "import * as $1 from '$2';", "import * as something from 'fs';");
    assert_replace("let something = require('fs')", search, "import * as $1 from '$1';", "import * as something from 'something';");
    assert_replace("let something = require('fs')", search, "import * as $2 from '$1';", "import * as fs from 'something';");
    assert_replace("let something = require('fs')", search, "import * as $0 from '$0';", "import * as let something = require('fs') from 'let something = require('fs')';");
    assert_replace("for ()", "for(.*)", "cat$1", "cat ()");
    // issue #18111
    assert_replace("HRESULT OnAmbientPropertyChange(DISPID   dispid);", r"\b\s{3}\b", " ", " ");
}

#[test]
fn get_replace_string_if_match_is_sub_string_of_the_text() {
    assert_replace("this is a bla text", "bla", "hello", "hello");
    assert_replace("this is a bla text", "this(?=.*bla)", "that", "that");
    assert_replace("this is a bla text", "(th)is(?=.*bla)", "$1at", "that");
    assert_replace("this is a bla text", "(th)is(?=.*bla)", "$1e", "the");
    assert_replace("this is a bla text", "(th)is(?=.*bla)", "$1ere", "there");
    assert_replace("this is a bla text", "(th)is(?=.*bla)", "$1", "th");
    assert_replace("this is a bla text", "(th)is(?=.*bla)", "ma$1", "math");
    assert_replace("this is a bla text", "(th)is(?=.*bla)", "ma$1s", "maths");
    assert_replace("this is a bla text", "(th)is(?=.*bla)", "$0", "this");
    assert_replace("this is a bla text", "(th)is(?=.*bla)", "$0$1", "thisth");
    assert_replace("this is a bla text", r"bla(?=\stext$)", "foo", "foo");
    assert_replace("this is a bla text", r"b(la)(?=\stext$)", "f$1", "fla");
    assert_replace("this is a bla text", r"b(la)(?=\stext$)", "f$0", "fbla");
    assert_replace("this is a bla text", r"b(la)(?=\stext$)", "$0ah", "blaah");
}

#[test]
fn issue_19740_an_unmatched_group_inserts_an_empty_string() {
    assert_replace("abcd", "a(z)?", "a{$1}", "a{}");
}

fn case_preserved_cases() -> Vec<(Vec<&'static str>, &'static str, &'static str)> {
    vec![
        (vec!["abc"], "Def", "def"),
        (vec!["Abc"], "Def", "Def"),
        (vec!["ABC"], "Def", "DEF"),
        (vec!["abc", "Abc"], "Def", "def"),
        (vec!["Abc", "abc"], "Def", "Def"),
        (vec!["ABC", "abc"], "Def", "DEF"),
        (vec!["aBc", "abc"], "Def", "def"),
        (vec!["AbC"], "Def", "Def"),
        (vec!["aBC"], "Def", "def"),
        (vec!["aBc"], "DeF", "deF"),
        (vec!["Foo-Bar"], "newfoo-newbar", "Newfoo-Newbar"),
        (vec!["Foo-Bar-Abc"], "newfoo-newbar-newabc", "Newfoo-Newbar-Newabc"),
        (vec!["Foo-Bar-abc"], "newfoo-newbar", "Newfoo-newbar"),
        (vec!["foo-Bar"], "newfoo-newbar", "newfoo-Newbar"),
        (vec!["foo-BAR"], "newfoo-newbar", "newfoo-NEWBAR"),
        (vec!["foO-BAR"], "NewFoo-NewBar", "newFoo-NEWBAR"),
        (vec!["Foo_Bar"], "newfoo_newbar", "Newfoo_Newbar"),
        (vec!["Foo_Bar_Abc"], "newfoo_newbar_newabc", "Newfoo_Newbar_Newabc"),
        (vec!["Foo_Bar_abc"], "newfoo_newbar", "Newfoo_newbar"),
        (vec!["Foo_Bar-abc"], "newfoo_newbar-abc", "Newfoo_newbar-abc"),
        (vec!["foo_Bar"], "newfoo_newbar", "newfoo_Newbar"),
    ]
}

#[test]
fn build_replace_string_with_case_preserved() {
    let mut cases = case_preserved_cases();
    cases.push((vec!["Foo_BAR"], "newfoo_newbar", "Newfoo_NEWBAR"));
    for (target, replace, expected) in cases {
        let target: Vec<String> = target.into_iter().map(String::from).collect();
        assert_eq!(replace_with_case_preserved(Some(&target), replace), expected, "{target:?} {replace}");
    }
}

#[test]
fn preserve_case() {
    let mut cases = case_preserved_cases();
    cases.push((vec!["foo_BAR"], "newfoo_newbar", "newfoo_NEWBAR"));
    for (target, replace, expected) in cases {
        let target: Vec<String> = target.into_iter().map(String::from).collect();
        assert_eq!(parse_replace_string(replace).build(Some(&target), true), expected, "{target:?} {replace}");
    }
}
