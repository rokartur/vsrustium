//! findModel.test.ts. Upstream runs each test on two text buffers; the port has one.

use super::*;
use crate::editor::edit::tests::editor;
use crate::editor::find::state::NewFindState;

const VIEWPORT: Viewport = Viewport { width: 800., height: 600., char_width: 7.2, sticky_scroll_lines: 0 };

const TEXT: &str = "// my cool header
#include \"cool.h\"
#include <iostream>

int main() {
    cout << \"hello world, Hello!\" << endl;
    cout << \"hello world again\" << endl;
    cout << \"Hello world again\" << endl;
    cout << \"helloworld again\" << endl;
}
// blablablaciao
";

/// The editor, the state and the model bound to them, with upstream's 1-based ranges.
struct Test {
    editor: OpenEditor,
    state: FindState,
    model: Option<FindModel>,
}

fn position(line: usize, column: usize) -> Position {
    Position::new(line - 1, column - 1)
}

fn range(start_line: usize, start_column: usize, end_line: usize, end_column: usize) -> Range<Position> {
    position(start_line, start_column)..position(end_line, end_column)
}

fn selection(start_line: usize, start_column: usize, end_line: usize, end_column: usize) -> Selection {
    Selection { anchor: position(start_line, start_column), head: position(end_line, end_column) }
}

fn from_range(range: &Range<Position>) -> [usize; 4] {
    [range.start.line + 1, range.start.column + 1, range.end.line + 1, range.end.column + 1]
}

impl Test {
    fn new() -> Self {
        Test { editor: editor(TEXT, "\n"), state: FindState::default(), model: None }
    }

    fn bind(&mut self) {
        self.model = Some(FindModel::new(&mut self.editor, &mut self.state, VIEWPORT));
    }

    fn dispose(&mut self) {
        self.model = None;
    }

    fn change(&mut self, new: NewFindState, move_cursor: bool) {
        let Some(changed) = self.state.change(new, move_cursor, false) else { return };
        if let Some(model) = &mut self.model {
            model.state_changed(&mut self.editor, &mut self.state, VIEWPORT, changed);
        }
    }

    fn assert_state(&self, cursor: [usize; 4], highlighted: Option<[usize; 4]>, find_decorations: &[[usize; 4]]) {
        let selection = self.editor.selection();
        assert_eq!(from_range(&(selection.start()..selection.end())), cursor, "cursor");
        let (matches, current) = match &self.model {
            Some(model) => (model.matches.iter().map(from_range).collect(), model.highlighted.map(|index| from_range(&model.matches[index]))),
            None => (Vec::new(), None),
        };
        assert_eq!((current, matches), (highlighted, find_decorations.to_vec()), "state");
    }

    fn assert_selections(&self, expected: &[[usize; 4]]) {
        let selections: Vec<_> = self.editor.selections.iter().map(|selection| from_range(&(selection.anchor..selection.head))).collect();
        assert_eq!(selections, expected);
    }

    /// `CoreNavigationCommands.MoveTo` from the mouse: an explicit cursor change.
    fn move_to(&mut self, line: usize, column: usize) {
        self.editor.set_selections(vec![Selection::caret(position(line, column))]);
        let editor = &self.editor;
        if let Some(model) = &mut self.model {
            model.cursor_moved(editor);
        }
    }

    fn set_selection(&mut self, start_line: usize, start_column: usize, end_line: usize, end_column: usize) {
        self.editor.set_selections(vec![selection(start_line, start_column, end_line, end_column)]);
    }

    /// `setValue`: one edit over the whole text, then the model's content listener.
    fn set_value(&mut self, text: &str) {
        let end = last_position(&self.editor.document.lines);
        let change = self.editor.execute_edits(vec![(Position::default(), end, text.into(), false)], Some(vec![Selection::caret(Position::default())]), EditReason::Unknown("setValue")).expect("text changed");
        if let Some(model) = &mut self.model {
            model.content_changed(&mut self.editor, &mut self.state, VIEWPORT, &change);
        }
    }

    fn line(&self, line: usize) -> &str {
        &self.editor.document.lines[line - 1]
    }

    fn next(&mut self) {
        let Test { editor, state, model } = self;
        model.as_mut().expect("bound").move_to_next_match(editor, state, VIEWPORT);
    }

    fn prev(&mut self) {
        let Test { editor, state, model } = self;
        model.as_mut().expect("bound").move_to_prev_match(editor, state, VIEWPORT);
    }

    fn replace(&mut self) {
        let Test { editor, state, model } = self;
        model.as_mut().expect("bound").replace(editor, state, VIEWPORT);
    }

    fn replace_all(&mut self) {
        let Test { editor, state, model } = self;
        model.as_mut().expect("bound").replace_all(editor, state, VIEWPORT);
    }

    fn select_all_matches(&mut self) {
        let Test { editor, state, model } = self;
        model.as_mut().expect("bound").select_all_matches(editor, state);
    }
}

/// incremental find from beginning of file
#[test]
fn incremental_find_from_beginning_of_file() {
    let mut t = Test::new();
    t.set_selection(1, 1, 1, 1);
    t.bind();

    // simulate typing the search string
    t.change(NewFindState { search_string: Some("H".into()), ..Default::default() }, true);
    t.assert_state([1, 12, 1, 13], Some([1, 12, 1, 13]), &[[1, 12, 1, 13], [2, 16, 2, 17], [6, 14, 6, 15], [6, 27, 6, 28], [7, 14, 7, 15], [8, 14, 8, 15], [9, 14, 9, 15]]);

    // simulate typing the search string
    t.change(NewFindState { search_string: Some("He".into()), ..Default::default() }, true);
    t.assert_state([1, 12, 1, 14], Some([1, 12, 1, 14]), &[[1, 12, 1, 14], [6, 14, 6, 16], [6, 27, 6, 29], [7, 14, 7, 16], [8, 14, 8, 16], [9, 14, 9, 16]]);

    // simulate typing the search string
    t.change(NewFindState { search_string: Some("Hello".into()), ..Default::default() }, true);
    t.assert_state([6, 14, 6, 19], Some([6, 14, 6, 19]), &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19], [9, 14, 9, 19]]);

    // simulate toggling on `matchCase`
    t.change(NewFindState { match_case: Some(true), ..Default::default() }, true);
    t.assert_state([6, 27, 6, 32], Some([6, 27, 6, 32]), &[[6, 27, 6, 32], [8, 14, 8, 19]]);

    // simulate typing the search string
    t.change(NewFindState { search_string: Some("hello".into()), ..Default::default() }, true);
    t.assert_state([6, 14, 6, 19], Some([6, 14, 6, 19]), &[[6, 14, 6, 19], [7, 14, 7, 19], [9, 14, 9, 19]]);

    // simulate toggling on `wholeWord`
    t.change(NewFindState { whole_word: Some(true), ..Default::default() }, true);
    t.assert_state([6, 14, 6, 19], Some([6, 14, 6, 19]), &[[6, 14, 6, 19], [7, 14, 7, 19]]);

    // simulate toggling off `matchCase`
    t.change(NewFindState { match_case: Some(false), ..Default::default() }, true);
    t.assert_state([6, 14, 6, 19], Some([6, 14, 6, 19]), &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    // simulate toggling off `wholeWord`
    t.change(NewFindState { whole_word: Some(false), ..Default::default() }, true);
    t.assert_state([6, 14, 6, 19], Some([6, 14, 6, 19]), &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19], [9, 14, 9, 19]]);

    // simulate adding a search scope
    t.change(NewFindState { search_scope: Some(Some(vec![range(8, 1, 10, 1)])), ..Default::default() }, true);
    t.assert_state([8, 14, 8, 19], Some([8, 14, 8, 19]), &[[8, 14, 8, 19], [9, 14, 9, 19]]);

    // simulate removing the search scope
    t.change(NewFindState { search_scope: Some(None), ..Default::default() }, true);
    t.assert_state([6, 14, 6, 19], Some([6, 14, 6, 19]), &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19], [9, 14, 9, 19]]);

    t.dispose();
}

/// find model removes its decorations
#[test]
fn find_model_removes_its_decorations() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), ..Default::default() }, false);
    t.bind();

    assert_eq!(t.state.matches_count, 5);
    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19], [9, 14, 9, 19]]);

    t.dispose();

    t.assert_state([1, 1, 1, 1], None, &[]);
}

/// find model updates state matchesCount
#[test]
fn find_model_updates_state_matchescount() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), ..Default::default() }, false);
    t.bind();

    assert_eq!(t.state.matches_count, 5);
    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19], [9, 14, 9, 19]]);

    t.change(NewFindState { search_string: Some("helloo".into()), ..Default::default() }, false);
    assert_eq!(t.state.matches_count, 0);
    t.assert_state([1, 1, 1, 1], None, &[]);

    t.dispose();
}

/// find model reacts to position change
#[test]
fn find_model_reacts_to_position_change() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19], [9, 14, 9, 19]]);

    t.move_to(6, 20);

    t.assert_state([6, 20, 6, 20], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19], [9, 14, 9, 19]]);

    t.change(NewFindState { search_string: Some("Hello".into()), ..Default::default() }, true);
    t.assert_state([6, 27, 6, 32], Some([6, 27, 6, 32]), &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19], [9, 14, 9, 19]]);

    t.dispose();
}

/// find model next
#[test]
fn find_model_next() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), whole_word: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.next();
    t.assert_state([6, 14, 6, 19], Some([6, 14, 6, 19]), &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.next();
    t.assert_state([6, 27, 6, 32], Some([6, 27, 6, 32]), &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.next();
    t.assert_state([7, 14, 7, 19], Some([7, 14, 7, 19]), &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.next();
    t.assert_state([8, 14, 8, 19], Some([8, 14, 8, 19]), &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.next();
    t.assert_state([6, 14, 6, 19], Some([6, 14, 6, 19]), &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.dispose();
}

/// find model next stays in scope
#[test]
fn find_model_next_stays_in_scope() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), whole_word: Some(true), search_scope: Some(Some(vec![range(7, 1, 9, 1)])), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[7, 14, 7, 19], [8, 14, 8, 19]]);

    t.next();
    t.assert_state([7, 14, 7, 19], Some([7, 14, 7, 19]), &[[7, 14, 7, 19], [8, 14, 8, 19]]);

    t.next();
    t.assert_state([8, 14, 8, 19], Some([8, 14, 8, 19]), &[[7, 14, 7, 19], [8, 14, 8, 19]]);

    t.next();
    t.assert_state([7, 14, 7, 19], Some([7, 14, 7, 19]), &[[7, 14, 7, 19], [8, 14, 8, 19]]);

    t.dispose();
}

/// multi-selection find model next stays in scope (overlap)
#[test]
fn multi_selection_find_model_next_stays_in_scope_overlap() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), whole_word: Some(true), search_scope: Some(Some(vec![range(7, 1, 8, 2), range(8, 1, 9, 1)])), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[7, 14, 7, 19], [8, 14, 8, 19]]);

    t.next();
    t.assert_state([7, 14, 7, 19], Some([7, 14, 7, 19]), &[[7, 14, 7, 19], [8, 14, 8, 19]]);

    t.next();
    t.assert_state([8, 14, 8, 19], Some([8, 14, 8, 19]), &[[7, 14, 7, 19], [8, 14, 8, 19]]);

    t.next();
    t.assert_state([7, 14, 7, 19], Some([7, 14, 7, 19]), &[[7, 14, 7, 19], [8, 14, 8, 19]]);

    t.dispose();
}

/// multi-selection find model next stays in scope
#[test]
fn multi_selection_find_model_next_stays_in_scope() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), match_case: Some(true), whole_word: Some(false), search_scope: Some(Some(vec![range(6, 1, 7, 38), range(9, 3, 9, 38)])), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [7, 14, 7, 19], [9, 14, 9, 19]]);

    t.next();
    t.assert_state([6, 14, 6, 19], Some([6, 14, 6, 19]), &[[6, 14, 6, 19], [7, 14, 7, 19], [9, 14, 9, 19]]);

    t.next();
    t.assert_state([7, 14, 7, 19], Some([7, 14, 7, 19]), &[[6, 14, 6, 19], [7, 14, 7, 19], [9, 14, 9, 19]]);

    t.next();
    t.assert_state([9, 14, 9, 19], Some([9, 14, 9, 19]), &[[6, 14, 6, 19], [7, 14, 7, 19], [9, 14, 9, 19]]);

    t.next();
    t.assert_state([6, 14, 6, 19], Some([6, 14, 6, 19]), &[[6, 14, 6, 19], [7, 14, 7, 19], [9, 14, 9, 19]]);

    t.dispose();
}

/// find model prev
#[test]
fn find_model_prev() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), whole_word: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.prev();
    t.assert_state([8, 14, 8, 19], Some([8, 14, 8, 19]), &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.prev();
    t.assert_state([7, 14, 7, 19], Some([7, 14, 7, 19]), &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.prev();
    t.assert_state([6, 27, 6, 32], Some([6, 27, 6, 32]), &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.prev();
    t.assert_state([6, 14, 6, 19], Some([6, 14, 6, 19]), &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.prev();
    t.assert_state([8, 14, 8, 19], Some([8, 14, 8, 19]), &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.dispose();
}

/// find model prev stays in scope
#[test]
fn find_model_prev_stays_in_scope() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), whole_word: Some(true), search_scope: Some(Some(vec![range(7, 1, 9, 1)])), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[7, 14, 7, 19], [8, 14, 8, 19]]);

    t.prev();
    t.assert_state([8, 14, 8, 19], Some([8, 14, 8, 19]), &[[7, 14, 7, 19], [8, 14, 8, 19]]);

    t.prev();
    t.assert_state([7, 14, 7, 19], Some([7, 14, 7, 19]), &[[7, 14, 7, 19], [8, 14, 8, 19]]);

    t.prev();
    t.assert_state([8, 14, 8, 19], Some([8, 14, 8, 19]), &[[7, 14, 7, 19], [8, 14, 8, 19]]);

    t.dispose();
}

/// find model next/prev with no matches
#[test]
fn find_model_next_prev_with_no_matches() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("helloo".into()), whole_word: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[]);

    t.next();
    t.assert_state([1, 1, 1, 1], None, &[]);

    t.prev();
    t.assert_state([1, 1, 1, 1], None, &[]);

    t.dispose();
}

/// find model next/prev respects cursor position
#[test]
fn find_model_next_prev_respects_cursor_position() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), whole_word: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.move_to(6, 20);
    t.assert_state([6, 20, 6, 20], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.next();
    t.assert_state([6, 27, 6, 32], Some([6, 27, 6, 32]), &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.dispose();
}

/// find ^
#[test]
fn find_caret() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("^".into()), is_regex: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[1, 1, 1, 1], [2, 1, 2, 1], [3, 1, 3, 1], [4, 1, 4, 1], [5, 1, 5, 1], [6, 1, 6, 1], [7, 1, 7, 1], [8, 1, 8, 1], [9, 1, 9, 1], [10, 1, 10, 1], [11, 1, 11, 1], [12, 1, 12, 1]]);

    t.next();
    t.assert_state([2, 1, 2, 1], Some([2, 1, 2, 1]), &[[1, 1, 1, 1], [2, 1, 2, 1], [3, 1, 3, 1], [4, 1, 4, 1], [5, 1, 5, 1], [6, 1, 6, 1], [7, 1, 7, 1], [8, 1, 8, 1], [9, 1, 9, 1], [10, 1, 10, 1], [11, 1, 11, 1], [12, 1, 12, 1]]);

    t.next();
    t.assert_state([3, 1, 3, 1], Some([3, 1, 3, 1]), &[[1, 1, 1, 1], [2, 1, 2, 1], [3, 1, 3, 1], [4, 1, 4, 1], [5, 1, 5, 1], [6, 1, 6, 1], [7, 1, 7, 1], [8, 1, 8, 1], [9, 1, 9, 1], [10, 1, 10, 1], [11, 1, 11, 1], [12, 1, 12, 1]]);

    t.dispose();
}

/// find $
#[test]
fn find_dollar() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("$".into()), is_regex: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[1, 18, 1, 18], [2, 18, 2, 18], [3, 20, 3, 20], [4, 1, 4, 1], [5, 13, 5, 13], [6, 43, 6, 43], [7, 41, 7, 41], [8, 41, 8, 41], [9, 40, 9, 40], [10, 2, 10, 2], [11, 17, 11, 17], [12, 1, 12, 1]]);

    t.next();
    t.assert_state([1, 18, 1, 18], Some([1, 18, 1, 18]), &[[1, 18, 1, 18], [2, 18, 2, 18], [3, 20, 3, 20], [4, 1, 4, 1], [5, 13, 5, 13], [6, 43, 6, 43], [7, 41, 7, 41], [8, 41, 8, 41], [9, 40, 9, 40], [10, 2, 10, 2], [11, 17, 11, 17], [12, 1, 12, 1]]);

    t.next();
    t.assert_state([2, 18, 2, 18], Some([2, 18, 2, 18]), &[[1, 18, 1, 18], [2, 18, 2, 18], [3, 20, 3, 20], [4, 1, 4, 1], [5, 13, 5, 13], [6, 43, 6, 43], [7, 41, 7, 41], [8, 41, 8, 41], [9, 40, 9, 40], [10, 2, 10, 2], [11, 17, 11, 17], [12, 1, 12, 1]]);

    t.next();
    t.assert_state([3, 20, 3, 20], Some([3, 20, 3, 20]), &[[1, 18, 1, 18], [2, 18, 2, 18], [3, 20, 3, 20], [4, 1, 4, 1], [5, 13, 5, 13], [6, 43, 6, 43], [7, 41, 7, 41], [8, 41, 8, 41], [9, 40, 9, 40], [10, 2, 10, 2], [11, 17, 11, 17], [12, 1, 12, 1]]);

    t.dispose();
}

/// find next ^$
#[test]
fn find_next_caret_dollar() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("^$".into()), is_regex: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[4, 1, 4, 1], [12, 1, 12, 1]]);

    t.next();
    t.assert_state([4, 1, 4, 1], Some([4, 1, 4, 1]), &[[4, 1, 4, 1], [12, 1, 12, 1]]);

    t.next();
    t.assert_state([12, 1, 12, 1], Some([12, 1, 12, 1]), &[[4, 1, 4, 1], [12, 1, 12, 1]]);

    t.next();
    t.assert_state([4, 1, 4, 1], Some([4, 1, 4, 1]), &[[4, 1, 4, 1], [12, 1, 12, 1]]);

    t.dispose();
}

/// find .*
#[test]
fn find_dot_star() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some(".*".into()), is_regex: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[1, 1, 1, 18], [2, 1, 2, 18], [3, 1, 3, 20], [4, 1, 4, 1], [5, 1, 5, 13], [6, 1, 6, 43], [7, 1, 7, 41], [8, 1, 8, 41], [9, 1, 9, 40], [10, 1, 10, 2], [11, 1, 11, 17], [12, 1, 12, 1]]);

    t.dispose();
}

/// find next ^.*$
#[test]
fn find_next_caret_dot_star_dollar() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("^.*$".into()), is_regex: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[1, 1, 1, 18], [2, 1, 2, 18], [3, 1, 3, 20], [4, 1, 4, 1], [5, 1, 5, 13], [6, 1, 6, 43], [7, 1, 7, 41], [8, 1, 8, 41], [9, 1, 9, 40], [10, 1, 10, 2], [11, 1, 11, 17], [12, 1, 12, 1]]);

    t.next();
    t.assert_state([1, 1, 1, 18], Some([1, 1, 1, 18]), &[[1, 1, 1, 18], [2, 1, 2, 18], [3, 1, 3, 20], [4, 1, 4, 1], [5, 1, 5, 13], [6, 1, 6, 43], [7, 1, 7, 41], [8, 1, 8, 41], [9, 1, 9, 40], [10, 1, 10, 2], [11, 1, 11, 17], [12, 1, 12, 1]]);

    t.next();
    t.assert_state([2, 1, 2, 18], Some([2, 1, 2, 18]), &[[1, 1, 1, 18], [2, 1, 2, 18], [3, 1, 3, 20], [4, 1, 4, 1], [5, 1, 5, 13], [6, 1, 6, 43], [7, 1, 7, 41], [8, 1, 8, 41], [9, 1, 9, 40], [10, 1, 10, 2], [11, 1, 11, 17], [12, 1, 12, 1]]);

    t.dispose();
}

/// find prev ^.*$
#[test]
fn find_prev_caret_dot_star_dollar() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("^.*$".into()), is_regex: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[1, 1, 1, 18], [2, 1, 2, 18], [3, 1, 3, 20], [4, 1, 4, 1], [5, 1, 5, 13], [6, 1, 6, 43], [7, 1, 7, 41], [8, 1, 8, 41], [9, 1, 9, 40], [10, 1, 10, 2], [11, 1, 11, 17], [12, 1, 12, 1]]);

    t.prev();
    t.assert_state([12, 1, 12, 1], Some([12, 1, 12, 1]), &[[1, 1, 1, 18], [2, 1, 2, 18], [3, 1, 3, 20], [4, 1, 4, 1], [5, 1, 5, 13], [6, 1, 6, 43], [7, 1, 7, 41], [8, 1, 8, 41], [9, 1, 9, 40], [10, 1, 10, 2], [11, 1, 11, 17], [12, 1, 12, 1]]);

    t.prev();
    t.assert_state([11, 1, 11, 17], Some([11, 1, 11, 17]), &[[1, 1, 1, 18], [2, 1, 2, 18], [3, 1, 3, 20], [4, 1, 4, 1], [5, 1, 5, 13], [6, 1, 6, 43], [7, 1, 7, 41], [8, 1, 8, 41], [9, 1, 9, 40], [10, 1, 10, 2], [11, 1, 11, 17], [12, 1, 12, 1]]);

    t.dispose();
}

/// find prev ^$
#[test]
fn find_prev_caret_dollar() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("^$".into()), is_regex: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[4, 1, 4, 1], [12, 1, 12, 1]]);

    t.prev();
    t.assert_state([12, 1, 12, 1], Some([12, 1, 12, 1]), &[[4, 1, 4, 1], [12, 1, 12, 1]]);

    t.prev();
    t.assert_state([4, 1, 4, 1], Some([4, 1, 4, 1]), &[[4, 1, 4, 1], [12, 1, 12, 1]]);

    t.prev();
    t.assert_state([12, 1, 12, 1], Some([12, 1, 12, 1]), &[[4, 1, 4, 1], [12, 1, 12, 1]]);

    t.dispose();
}

/// replace hello
#[test]
fn replace_hello() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), replace_string: Some("hi".into()), whole_word: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.move_to(6, 20);
    t.assert_state([6, 20, 6, 20], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);
    assert_eq!(t.line(6), "    cout << \"hello world, Hello!\" << endl;");

    t.replace();
    t.assert_state([6, 27, 6, 32], Some([6, 27, 6, 32]), &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);
    assert_eq!(t.line(6), "    cout << \"hello world, Hello!\" << endl;");

    t.replace();
    t.assert_state([7, 14, 7, 19], Some([7, 14, 7, 19]), &[[6, 14, 6, 19], [7, 14, 7, 19], [8, 14, 8, 19]]);
    assert_eq!(t.line(6), "    cout << \"hello world, hi!\" << endl;");

    t.replace();
    t.assert_state([8, 14, 8, 19], Some([8, 14, 8, 19]), &[[6, 14, 6, 19], [8, 14, 8, 19]]);
    assert_eq!(t.line(7), "    cout << \"hi world again\" << endl;");

    t.replace();
    t.assert_state([6, 14, 6, 19], Some([6, 14, 6, 19]), &[[6, 14, 6, 19]]);
    assert_eq!(t.line(8), "    cout << \"hi world again\" << endl;");

    t.replace();
    t.assert_state([6, 16, 6, 16], None, &[]);
    assert_eq!(t.line(6), "    cout << \"hi world, hi!\" << endl;");

    t.dispose();
}

/// replace bla
#[test]
fn replace_bla() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("bla".into()), replace_string: Some("ciao".into()), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[11, 4, 11, 7], [11, 7, 11, 10], [11, 10, 11, 13]]);

    t.replace();
    t.assert_state([11, 4, 11, 7], Some([11, 4, 11, 7]), &[[11, 4, 11, 7], [11, 7, 11, 10], [11, 10, 11, 13]]);
    assert_eq!(t.line(11), "// blablablaciao");

    t.replace();
    t.assert_state([11, 8, 11, 11], Some([11, 8, 11, 11]), &[[11, 8, 11, 11], [11, 11, 11, 14]]);
    assert_eq!(t.line(11), "// ciaoblablaciao");

    t.replace();
    t.assert_state([11, 12, 11, 15], Some([11, 12, 11, 15]), &[[11, 12, 11, 15]]);
    assert_eq!(t.line(11), "// ciaociaoblaciao");

    t.replace();
    t.assert_state([11, 16, 11, 16], None, &[]);
    assert_eq!(t.line(11), "// ciaociaociaociao");

    t.dispose();
}

/// replaceAll hello
#[test]
fn replaceall_hello() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), replace_string: Some("hi".into()), whole_word: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.move_to(6, 20);
    t.assert_state([6, 20, 6, 20], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);
    assert_eq!(t.line(6), "    cout << \"hello world, Hello!\" << endl;");

    t.replace_all();
    t.assert_state([6, 17, 6, 17], None, &[]);
    assert_eq!(t.line(6), "    cout << \"hi world, hi!\" << endl;");
    assert_eq!(t.line(7), "    cout << \"hi world again\" << endl;");
    assert_eq!(t.line(8), "    cout << \"hi world again\" << endl;");

    t.dispose();
}

/// replaceAll two spaces with one space
#[test]
fn replaceall_two_spaces_with_one_space() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("  ".into()), replace_string: Some(" ".into()), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 1, 6, 3], [6, 3, 6, 5], [7, 1, 7, 3], [7, 3, 7, 5], [8, 1, 8, 3], [8, 3, 8, 5], [9, 1, 9, 3], [9, 3, 9, 5]]);

    t.replace_all();
    t.assert_state([1, 1, 1, 1], None, &[[6, 1, 6, 3], [7, 1, 7, 3], [8, 1, 8, 3], [9, 1, 9, 3]]);
    assert_eq!(t.line(6), "  cout << \"hello world, Hello!\" << endl;");
    assert_eq!(t.line(7), "  cout << \"hello world again\" << endl;");
    assert_eq!(t.line(8), "  cout << \"Hello world again\" << endl;");
    assert_eq!(t.line(9), "  cout << \"helloworld again\" << endl;");

    t.dispose();
}

/// replaceAll bla
#[test]
fn replaceall_bla() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("bla".into()), replace_string: Some("ciao".into()), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[11, 4, 11, 7], [11, 7, 11, 10], [11, 10, 11, 13]]);

    t.replace_all();
    t.assert_state([1, 1, 1, 1], None, &[]);
    assert_eq!(t.line(11), "// ciaociaociaociao");

    t.dispose();
}

/// replaceAll bla with \\t\\n
#[test]
fn replaceall_bla_with_tab_newline() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("bla".into()), replace_string: Some("<\\n\\t>".into()), is_regex: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[11, 4, 11, 7], [11, 7, 11, 10], [11, 10, 11, 13]]);

    t.replace_all();
    t.assert_state([1, 1, 1, 1], None, &[]);
    assert_eq!(t.line(11), "// <");
    assert_eq!(t.line(12), "\t><");
    assert_eq!(t.line(13), "\t><");
    assert_eq!(t.line(14), "\t>ciao");

    t.dispose();
}

/// issue #3516: "replace all" moves page/cursor/focus/scroll to the place of the last replacement
#[test]
fn issue_3516_replace_all_moves_page_cursor_focus_scroll_to_the_place_of_the_last_replacement() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("include".into()), replace_string: Some("bar".into()), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[2, 2, 2, 9], [3, 2, 3, 9]]);

    t.replace_all();
    t.assert_state([1, 1, 1, 1], None, &[]);

    assert_eq!(t.line(2), "#bar \"cool.h\"");
    assert_eq!(t.line(3), "#bar <iostream>");

    t.dispose();
}

/// listens to model content changes
#[test]
fn listens_to_model_content_changes() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), replace_string: Some("hi".into()), whole_word: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.set_value("hello\nhi");
    // Upstream's `setValue` flushes and clears the decorations until the search 100 ms later; the port
    // has no flush edit and searches at once, so this is the state after that search.
    t.assert_state([1, 1, 1, 1], None, &[[1, 1, 1, 6]]);

    t.dispose();
}

/// selectAllMatches
#[test]
fn selectallmatches() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), replace_string: Some("hi".into()), whole_word: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.select_all_matches();

    t.assert_selections(&[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.assert_state([6, 14, 6, 19], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.dispose();
}

/// issue #14143 selectAllMatches should maintain primary cursor if feasible
#[test]
fn issue_14143_selectallmatches_should_maintain_primary_cursor_if_feasible() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), replace_string: Some("hi".into()), whole_word: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.set_selection(7, 14, 7, 19);

    t.select_all_matches();

    t.assert_selections(&[[7, 14, 7, 19], [6, 14, 6, 19], [6, 27, 6, 32], [8, 14, 8, 19]]);

    assert_eq!(t.editor.selection(), selection(7, 14, 7, 19));

    t.assert_state([7, 14, 7, 19], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.dispose();
}

/// issue #1914: NPE when there is only one find match
#[test]
fn issue_1914_npe_when_there_is_only_one_find_match() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("cool.h".into()), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[2, 11, 2, 17]]);

    t.next();
    t.assert_state([2, 11, 2, 17], Some([2, 11, 2, 17]), &[[2, 11, 2, 17]]);

    t.next();
    t.assert_state([2, 11, 2, 17], Some([2, 11, 2, 17]), &[[2, 11, 2, 17]]);

    t.dispose();
}

/// replace when search string has look ahed regex
#[test]
fn replace_when_search_string_has_look_ahed_regex() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello(?=\\sworld)".into()), replace_string: Some("hi".into()), is_regex: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.replace();

    t.assert_state([6, 14, 6, 19], Some([6, 14, 6, 19]), &[[6, 14, 6, 19], [7, 14, 7, 19], [8, 14, 8, 19]]);
    assert_eq!(t.line(6), "    cout << \"hello world, Hello!\" << endl;");

    t.replace();
    t.assert_state([7, 14, 7, 19], Some([7, 14, 7, 19]), &[[7, 14, 7, 19], [8, 14, 8, 19]]);
    assert_eq!(t.line(6), "    cout << \"hi world, Hello!\" << endl;");

    t.replace();
    t.assert_state([8, 14, 8, 19], Some([8, 14, 8, 19]), &[[8, 14, 8, 19]]);
    assert_eq!(t.line(7), "    cout << \"hi world again\" << endl;");

    t.replace();
    t.assert_state([8, 16, 8, 16], None, &[]);
    assert_eq!(t.line(8), "    cout << \"hi world again\" << endl;");

    t.dispose();
}

/// replace when search string has look ahed regex and cursor is at the last find match
#[test]
fn replace_when_search_string_has_look_ahed_regex_and_cursor_is_at_the_last_find_match() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello(?=\\sworld)".into()), replace_string: Some("hi".into()), is_regex: Some(true), ..Default::default() }, false);
    t.bind();

    t.move_to(8, 14);

    t.assert_state([8, 14, 8, 14], None, &[[6, 14, 6, 19], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.replace();

    t.assert_state([8, 14, 8, 19], Some([8, 14, 8, 19]), &[[6, 14, 6, 19], [7, 14, 7, 19], [8, 14, 8, 19]]);

    assert_eq!(t.line(8), "    cout << \"Hello world again\" << endl;");

    t.replace();
    t.assert_state([6, 14, 6, 19], Some([6, 14, 6, 19]), &[[6, 14, 6, 19], [7, 14, 7, 19]]);
    assert_eq!(t.line(8), "    cout << \"hi world again\" << endl;");

    t.replace();
    t.assert_state([7, 14, 7, 19], Some([7, 14, 7, 19]), &[[7, 14, 7, 19]]);
    assert_eq!(t.line(6), "    cout << \"hi world, Hello!\" << endl;");

    t.replace();
    t.assert_state([7, 16, 7, 16], None, &[]);
    assert_eq!(t.line(7), "    cout << \"hi world again\" << endl;");

    t.dispose();
}

/// replaceAll when search string has look ahed regex
#[test]
fn replaceall_when_search_string_has_look_ahed_regex() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello(?=\\sworld)".into()), replace_string: Some("hi".into()), is_regex: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.replace_all();

    assert_eq!(t.line(6), "    cout << \"hi world, Hello!\" << endl;");
    assert_eq!(t.line(7), "    cout << \"hi world again\" << endl;");
    assert_eq!(t.line(8), "    cout << \"hi world again\" << endl;");

    t.assert_state([1, 1, 1, 1], None, &[]);

    t.dispose();
}

/// replace when search string has look ahed regex and replace string has capturing groups
#[test]
fn replace_when_search_string_has_look_ahed_regex_and_replace_string_has_capturing_groups() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hel(lo)(?=\\sworld)".into()), replace_string: Some("hi$1".into()), is_regex: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.replace();

    t.assert_state([6, 14, 6, 19], Some([6, 14, 6, 19]), &[[6, 14, 6, 19], [7, 14, 7, 19], [8, 14, 8, 19]]);
    assert_eq!(t.line(6), "    cout << \"hello world, Hello!\" << endl;");

    t.replace();
    t.assert_state([7, 14, 7, 19], Some([7, 14, 7, 19]), &[[7, 14, 7, 19], [8, 14, 8, 19]]);
    assert_eq!(t.line(6), "    cout << \"hilo world, Hello!\" << endl;");

    t.replace();
    t.assert_state([8, 14, 8, 19], Some([8, 14, 8, 19]), &[[8, 14, 8, 19]]);
    assert_eq!(t.line(7), "    cout << \"hilo world again\" << endl;");

    t.replace();
    t.assert_state([8, 18, 8, 18], None, &[]);
    assert_eq!(t.line(8), "    cout << \"hilo world again\" << endl;");

    t.dispose();
}

/// replaceAll when search string has look ahed regex and replace string has capturing groups
#[test]
fn replaceall_when_search_string_has_look_ahed_regex_and_replace_string_has_capturing_groups() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("wo(rl)d(?=.*;$)".into()), replace_string: Some("gi$1".into()), is_regex: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 20, 6, 25], [7, 20, 7, 25], [8, 20, 8, 25], [9, 19, 9, 24]]);

    t.replace_all();

    assert_eq!(t.line(6), "    cout << \"hello girl, Hello!\" << endl;");
    assert_eq!(t.line(7), "    cout << \"hello girl again\" << endl;");
    assert_eq!(t.line(8), "    cout << \"Hello girl again\" << endl;");
    assert_eq!(t.line(9), "    cout << \"hellogirl again\" << endl;");

    t.assert_state([1, 1, 1, 1], None, &[]);

    t.dispose();
}

/// replaceAll when search string is multiline and has look ahed regex and replace string has capturing groups
#[test]
fn replaceall_when_search_string_is_multiline_and_has_look_ahed_regex_and_replace_string_has_capturing_groups() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("wo(rl)d(.*;\\n)(?=.*hello)".into()), replace_string: Some("gi$1$2".into()), is_regex: Some(true), match_case: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 20, 7, 1], [8, 20, 9, 1]]);

    t.replace_all();

    assert_eq!(t.line(6), "    cout << \"hello girl, Hello!\" << endl;");
    assert_eq!(t.line(8), "    cout << \"Hello girl again\" << endl;");

    t.assert_state([1, 1, 1, 1], None, &[]);

    t.dispose();
}

/// replaceAll preserving case
#[test]
fn replaceall_preserving_case() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), replace_string: Some("goodbye".into()), is_regex: Some(false), match_case: Some(false), preserve_case: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19], [9, 14, 9, 19]]);

    t.replace_all();

    assert_eq!(t.line(6), "    cout << \"goodbye world, Goodbye!\" << endl;");
    assert_eq!(t.line(7), "    cout << \"goodbye world again\" << endl;");
    assert_eq!(t.line(8), "    cout << \"Goodbye world again\" << endl;");
    assert_eq!(t.line(9), "    cout << \"goodbyeworld again\" << endl;");

    t.assert_state([1, 1, 1, 1], None, &[]);

    t.dispose();
}

/// issue #18711 replaceAll with empty string
#[test]
fn issue_18711_replaceall_with_empty_string() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), replace_string: Some("".into()), whole_word: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [6, 27, 6, 32], [7, 14, 7, 19], [8, 14, 8, 19]]);

    t.replace_all();
    t.assert_state([1, 1, 1, 1], None, &[]);
    assert_eq!(t.line(6), "    cout << \" world, !\" << endl;");
    assert_eq!(t.line(7), "    cout << \" world again\" << endl;");
    assert_eq!(t.line(8), "    cout << \" world again\" << endl;");

    t.dispose();
}

/// issue #32522 replaceAll with ^ on more than 1000 matches
#[test]
fn issue_32522_replaceall_with_on_more_than_1000_matches() {
    let mut t = Test::new();
    let initial: String = (0..1100).map(|i| format!("line{i}\n")).collect();
    t.set_value(&initial);
    t.change(NewFindState { search_string: Some("^".into()), replace_string: Some("a ".into()), is_regex: Some(true), ..Default::default() }, false);
    t.bind();

    t.replace_all();

    let expected: String = (0..1100).map(|i| format!("a line{i}\n")).collect::<String>() + "a ";
    assert_eq!(t.editor.document.lines.join("\n"), expected);

    t.dispose();
}

/// issue #19740 Find and replace capture group/backreference inserts `undefined` instead of empty string
#[test]
fn issue_19740_find_and_replace_capture_group_backreference_inserts_undefined_instead_of_empty_string() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello(z)?".into()), replace_string: Some("hi$1".into()), is_regex: Some(true), match_case: Some(true), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[6, 14, 6, 19], [7, 14, 7, 19], [9, 14, 9, 19]]);

    t.replace_all();
    t.assert_state([1, 1, 1, 1], None, &[]);
    assert_eq!(t.line(6), "    cout << \"hi world, Hello!\" << endl;");
    assert_eq!(t.line(7), "    cout << \"hi world again\" << endl;");
    assert_eq!(t.line(9), "    cout << \"hiworld again\" << endl;");

    t.dispose();
}

/// issue #27083. search scope works even if it is a single line
#[test]
fn issue_27083_search_scope_works_even_if_it_is_a_single_line() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), whole_word: Some(true), search_scope: Some(Some(vec![range(7, 1, 8, 1)])), ..Default::default() }, false);
    t.bind();

    t.assert_state([1, 1, 1, 1], None, &[[7, 14, 7, 19]]);

    t.dispose();
}

/// issue #3516: Control behavior of "Next" operations (not looping back to beginning)
#[test]
fn issue_3516_control_behavior_of_next_operations_not_looping_back_to_beginning() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), is_loop: Some(false), ..Default::default() }, false);
    t.bind();

    assert_eq!(t.state.matches_count, 5);

    // Test next operations
    assert_eq!(t.state.matches_position, 0);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.next();
    assert_eq!(t.state.matches_position, 1);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), false);

    t.next();
    assert_eq!(t.state.matches_position, 2);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.next();
    assert_eq!(t.state.matches_position, 3);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.next();
    assert_eq!(t.state.matches_position, 4);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.next();
    assert_eq!(t.state.matches_position, 5);
    assert_eq!(t.state.can_navigate_forward(), false);
    assert_eq!(t.state.can_navigate_back(), true);

    t.next();
    assert_eq!(t.state.matches_position, 5);
    assert_eq!(t.state.can_navigate_forward(), false);
    assert_eq!(t.state.can_navigate_back(), true);

    t.next();
    assert_eq!(t.state.matches_position, 5);
    assert_eq!(t.state.can_navigate_forward(), false);
    assert_eq!(t.state.can_navigate_back(), true);

    // Test previous operations
    t.prev();
    assert_eq!(t.state.matches_position, 4);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.prev();
    assert_eq!(t.state.matches_position, 3);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.prev();
    assert_eq!(t.state.matches_position, 2);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.prev();
    assert_eq!(t.state.matches_position, 1);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), false);

    t.prev();
    assert_eq!(t.state.matches_position, 1);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), false);

    t.prev();
    assert_eq!(t.state.matches_position, 1);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), false);

}

/// issue #3516: Control behavior of "Next" operations (looping back to beginning)
#[test]
fn issue_3516_control_behavior_of_next_operations_looping_back_to_beginning() {
    let mut t = Test::new();
    t.change(NewFindState { search_string: Some("hello".into()), ..Default::default() }, false);
    t.bind();

    assert_eq!(t.state.matches_count, 5);

    // Test next operations
    assert_eq!(t.state.matches_position, 0);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.next();
    assert_eq!(t.state.matches_position, 1);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.next();
    assert_eq!(t.state.matches_position, 2);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.next();
    assert_eq!(t.state.matches_position, 3);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.next();
    assert_eq!(t.state.matches_position, 4);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.next();
    assert_eq!(t.state.matches_position, 5);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.next();
    assert_eq!(t.state.matches_position, 1);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.next();
    assert_eq!(t.state.matches_position, 2);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    // Test previous operations
    t.prev();
    assert_eq!(t.state.matches_position, 1);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.prev();
    assert_eq!(t.state.matches_position, 5);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.prev();
    assert_eq!(t.state.matches_position, 4);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.prev();
    assert_eq!(t.state.matches_position, 3);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.prev();
    assert_eq!(t.state.matches_position, 2);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

    t.prev();
    assert_eq!(t.state.matches_position, 1);
    assert_eq!(t.state.can_navigate_forward(), true);
    assert_eq!(t.state.can_navigate_back(), true);

}

/// issue #288515: Wrong current index in find widget if matches > 1000
#[test]
fn issue_288515_wrong_current_index_in_find_widget_if_matches_1000() {
    let mut t = Test { editor: editor(&["hello"; 1001].join("\n"), "\n"), state: FindState::default(), model: None };
    t.set_selection(900, 1, 900, 6);
    t.change(NewFindState { search_string: Some("hello".into()), ..Default::default() }, false);
    t.bind();
    assert_eq!(t.state.matches_count, 1001);
    assert_eq!(t.state.matches_position, 900);
}
