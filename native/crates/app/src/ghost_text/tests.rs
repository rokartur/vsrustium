//! computeGhostText.test.ts: `[`/`]` mark the replaced range; the result is rendered with each part in brackets.

use super::*;

fn output(text: &str, suggestion: &str, mode: GhostTextMode) -> Option<String> {
    let start = text.find('[').unwrap();
    let end = text.find(']').unwrap() - 1;
    let cleaned = text.replacen('[', "", 1).replacen(']', "", 1);
    let lines: Vec<String> = cleaned.split('\n').map(String::from).collect();
    let position = |offset: usize| position_after(Position::default(), &cleaned[..offset]);
    let edit = Replacement { start: position(start), end: position(end), text: suggestion.to_owned() };
    compute_ghost_text(&edit, &lines, mode, None, 0).map(|ghost| ghost.render(&lines))
}

fn both(text: &str, suggestion: &str) -> (Option<String>, Option<String>) {
    (output(text, suggestion, GhostTextMode::Prefix), output(text, suggestion, GhostTextMode::Subword))
}

fn same(text: &str, suggestion: &str, expected: Option<&str>) {
    let expected = expected.map(String::from);
    assert_eq!(both(text, suggestion), (expected.clone(), expected), "{text} {suggestion}");
}

#[test]
fn basic() {
    same("[foo]baz", "foobar", Some("foo[bar]baz"));
    same("[aaa]aaa", "aaaaaa", Some("aaa[aaa]aaa"));
    same("[foo]baz", "boobar", None);
    same("[foo]foo", "foofoo", Some("foo[foo]foo"));
    same("foo[]", "bar\nhello", Some("foo[bar\nhello]"));
}

#[test]
fn empty_ghost_text() {
    same("[foo]", "foo", Some("foo"));
}

#[test]
fn whitespace_in_the_indentation() {
    same("[ foo]", "foobar", Some(" foo[bar]"));
    same("[\tfoo]", "foobar", Some("\tfoo[bar]"));
    same("[\t foo]", "\tfoobar", Some("\t foo[bar]"));
    assert_eq!(both("[\tfoo]", "\t\tfoobar"), (None, Some("\t[\t]foo[bar]".into())));
    same("[\t]", "\t\tfoobar", Some("\t[\tfoobar]"));
    same("\t[]", "\t", Some("\t[\t]"));
    same("\t[\t]", "", Some("\t\t"));
    same("[ ]", "return 1", Some(" [return 1]"));
}

#[test]
fn whitespace_outside_the_indentation() {
    same("bar[ foo]", "foobar", None);
    same("bar[\tfoo]", "foobar", None);
}

#[test]
fn new_lines() {
    same("fo[o\n]", "x\nbar", None);
    same("fo[o\n]", "o\nbar", Some("foo\n[bar]"));
}

#[test]
fn multi_part_diffing() {
    assert_eq!(both("foo[()]", "(x);"), (None, Some("foo([x])[;]".into())));
    assert_eq!(both("[(y ===)]", "(y === 1) { f(); }"), (None, Some("(y ===[ 1])[ { f(); }]".into())));
    assert_eq!(both("[(y ==)]", "(y === 1) { f(); }"), (None, Some("(y ==[= 1])[ { f(); }]".into())));
    assert_eq!(both("[if () ()]", "if (1 == f()) ()"), (None, Some("if ([1 == f()]) ()".into())));
    assert_eq!(both("[)]", "())"), (None, Some("[(])[)]".into())));
    assert_eq!(both("[))]", "(())"), (None, Some("[((]))".into())));
}

#[test]
fn parentheses_match() {
    assert_eq!(both("[console.log()]", "console.log({ label: \"(\" })"), (None, Some("console.log([{ label: \"(\" }])".into())));
}

#[test]
fn a_completion_shows_while_the_typed_text_is_its_prefix() {
    let lines = vec!["con".to_owned()];
    let edit = |text: &str| Replacement { start: Position::new(0, 0), end: Position::new(0, 3), text: text.to_owned() };
    assert!(inline_completion_is_visible(&edit("console"), None, &lines, Position::new(0, 3)));
    assert!(!inline_completion_is_visible(&edit("cat"), None, &lines, Position::new(0, 3)));
    // On another line than the cursor, or nothing left to insert.
    assert!(!inline_completion_is_visible(&edit("console"), None, &lines, Position::new(1, 0)));
    assert!(!inline_completion_is_visible(&edit("con"), None, &lines, Position::new(0, 3)));
}
