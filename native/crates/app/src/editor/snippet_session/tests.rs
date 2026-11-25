use super::*;

fn session(text: &str, stops: &[(u32, Range<usize>)]) -> SnippetSession {
    let snippet = Inserted { start: Position::new(0, 0), text, stops, choices: &[], transforms: &[], line_indent: "" };
    SnippetSession::new(&[snippet], 1)
}

fn ranges(selections: &[Selection]) -> Vec<(usize, usize)> {
    selections.iter().map(|selection| (selection.start().column, selection.end().column)).collect()
}

/// snippetSession.test.ts 'snippets, selections and new text with newlines' order: lowest index first, $0 last.
#[test]
fn moves_go_through_the_indexes_then_the_final_tab_stop() {
    let mut session = session("a two one", &[(2, 2..5), (1, 6..9), (0, 9..9)]);
    assert_eq!(ranges(&session.move_to(true)), [(6, 9)]);
    assert!(session.is_at_first() && !session.is_at_last());
    assert_eq!(ranges(&session.move_to(true)), [(2, 5)]);
    assert_eq!(ranges(&session.move_to(false)), [(6, 9)]);
    session.move_to(true);
    assert_eq!(ranges(&session.move_to(true)), [(9, 9)]);
    assert!(session.is_at_last());
}

/// Typing in the current placeholder grows it; the placeholders after it shift.
#[test]
fn the_current_placeholder_grows_when_typing_at_its_edges() {
    let mut session = session("f(x, y)", &[(1, 2..3), (2, 5..6), (0, 7..7)]);
    session.move_to(true);
    session.track(Position::new(0, 2), Position::new(0, 3), "");
    session.track(Position::new(0, 2), Position::new(0, 2), "abc");
    assert_eq!(ranges(&session.selections()), [(2, 5)]);
    assert_eq!(ranges(&session.move_to(true)), [(7, 8)]);
    assert!(session.selections_within(&[Selection::caret(Position::new(0, 8))]));
    assert!(!session.selections_within(&[Selection::caret(Position::new(0, 1))]));
}

/// `_hasPlaceholderBeenCollapsed`: a placeholder whose text was deleted is skipped.
#[test]
fn a_collapsed_placeholder_is_skipped() {
    let mut session = session("x y", &[(1, 0..1), (2, 2..3), (0, 3..3)]);
    session.move_to(true);
    session.track(Position::new(0, 2), Position::new(0, 3), "");
    assert_eq!(ranges(&session.move_to(true)), [(2, 2)]);
    assert!(session.is_at_last());
}

/// One group holds the placeholders of every cursor's copy.
#[test]
fn a_group_spans_every_copy() {
    let stops = [(1, 0..1), (0, 1..1)];
    let copies = [Inserted { start: Position::new(0, 0), text: "a", stops: &stops, choices: &[], transforms: &[], line_indent: "" }, Inserted { start: Position::new(1, 2), text: "a", stops: &stops, choices: &[], transforms: &[], line_indent: "" }];
    let mut session = SnippetSession::new(&copies, 1);
    let selections = session.move_to(true);
    assert_eq!(selections.iter().map(|selection| (selection.start().line, selection.start().column)).collect::<Vec<_>>(), [(0, 0), (1, 2)]);
}

/// snippetSession.test.ts 'snippets, transform': leaving a placeholder transforms its text.
#[test]
fn leaving_a_transformed_placeholder_gives_its_edit() {
    let snippet = crate::editor::snippet::expand("${1/foo/bar/}$0", "", true, Indentation { tab_size: 4, indent_size: None, insert_spaces: true }, |_| None);
    let inserted = Inserted { start: Position::new(0, 0), text: &snippet.text, stops: &snippet.stops, choices: &[], transforms: &snippet.transforms, line_indent: "" };
    let mut session = SnippetSession::new(&[inserted], 1);
    session.move_to(true);
    session.track(Position::new(0, 0), Position::new(0, 0), "foo");
    let indentation = Indentation { tab_size: 4, indent_size: None, insert_spaces: true };
    assert_eq!(session.transform_edits(&["foo".to_owned()], indentation), [(Position::new(0, 0), Position::new(0, 3), "bar".to_owned())]);
}

/// snippetSession.test.ts 'snippets, merge': a nested snippet's placeholders come right after the outer ones
/// before it, its `$0` before the outer next one.
#[test]
fn a_nested_snippet_takes_the_current_placeholders_place() {
    let mut session = session("( )", &[(1, 1..2), (2, 2..2), (0, 3..3)]);
    session.move_to(true);
    let stops = [(1, 0..1), (0, 2..2)];
    let nested = Inserted { start: Position::new(0, 1), text: "x;", stops: &stops, choices: &[], transforms: &[], line_indent: "" };
    assert!(session.merge(&[nested]));
    assert_eq!(ranges(&session.selections()), [(1, 2)]);
    assert_eq!(ranges(&session.move_to(true)), [(3, 3)]);
    assert!(!session.placeholders().any(|(placeholder, current)| current && placeholder.is_final), "the nested $0 is an ordinary stop");
    assert_eq!(ranges(&session.move_to(true)), [(2, 2)]);
}
