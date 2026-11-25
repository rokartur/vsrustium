use super::*;

#[test]
fn next_word_takes_the_whitespace_and_one_word() {
    assert_eq!(next_word("  foo bar"), "  foo");
    assert_eq!(next_word("(x)"), "(");
    assert_eq!(next_word("bar"), "bar");
}

#[test]
fn a_completion_without_range_replaces_the_word_before_the_cursor() {
    let lines = vec!["let co".to_owned()];
    let indentation = crate::editor::Indentation { tab_size: 4, indent_size: None, insert_spaces: true };
    let item = completion_item(&lines, Position::new(0, 6), indentation, 1, 2, &json!({ "insertText": "console", "idx": 3 })).unwrap();
    assert_eq!((item.edit.start, item.edit.end, item.idx), (Position::new(0, 4), Position::new(0, 6), 3));
    let ranged = completion_item(&lines, Position::new(0, 6), indentation, 1, 2, &json!({ "insertText": { "snippet": "co${1:nsole}" }, "range": { "startLineNumber": 1, "startColumn": 5, "endLineNumber": 1, "endColumn": 7 } })).unwrap();
    assert_eq!((ranged.edit.text.as_str(), ranged.snippet.as_deref()), ("console", Some("co${1:nsole}")));
}
