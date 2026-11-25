//! cursorUndo.ts `CursorUndoRedoController`: Cursor Undo and Cursor Redo step through the selections and
//! scroll positions before each cursor change that left the text as it was.

use super::cursor::Selection;
use super::{OpenEditor, Scroll};

/// The undo stack's bound.
const MAX_STATES: usize = 50;

#[derive(Debug, Default)]
pub struct CursorStates {
    undo: Vec<(Vec<Selection>, Scroll)>,
    redo: Vec<(Vec<Selection>, Scroll)>,
}

impl OpenEditor {
    /// `onDidChangeCursorSelection` without a text change: `previous` are the selections it replaced.
    pub(super) fn record_cursor_state(&mut self, previous: Vec<Selection>) {
        let states = &mut self.cursor_states;
        if states.undo.last().is_some_and(|(selections, _)| *selections == previous) {
            return;
        }
        states.undo.push((previous, self.scroll));
        states.redo.clear();
        if states.undo.len() > MAX_STATES {
            states.undo.remove(0);
        }
    }

    /// `onDidChangeModel` and `onDidChangeModelContent`.
    pub fn clear_cursor_states(&mut self) {
        self.cursor_states = CursorStates::default();
    }

    /// `cursorUndo`: the scroll position to restore, `None` with nothing to undo.
    pub fn cursor_undo(&mut self) -> Option<Scroll> {
        let state = self.cursor_states.undo.pop()?;
        self.cursor_states.redo.push((self.selections.clone(), self.scroll));
        Some(self.apply_cursor_state(state))
    }

    /// `cursorRedo`: the scroll position to restore, `None` with nothing to redo.
    pub fn cursor_redo(&mut self) -> Option<Scroll> {
        let state = self.cursor_states.redo.pop()?;
        self.cursor_states.undo.push((self.selections.clone(), self.scroll));
        Some(self.apply_cursor_state(state))
    }

    /// `_applyState`: setting the selections records nothing.
    fn apply_cursor_state(&mut self, (selections, scroll): (Vec<Selection>, Scroll)) -> Scroll {
        let states = std::mem::take(&mut self.cursor_states);
        self.set_selections(selections);
        self.cursor_states = states;
        scroll
    }
}

#[cfg(test)]
mod tests {
    use crate::editor::cursor::{Position, Selection};
    use crate::editor::edit::tests::editor;

    fn caret(column: usize) -> Vec<Selection> {
        vec![Selection::caret(Position::new(0, column))]
    }

    #[test]
    fn issue_82535_edge_case_with_cursor_undo() {
        let mut editor = editor("", "\n");
        editor.type_text("hello");
        editor.move_horizontally(false, false);
        editor.delete_right();
        assert_eq!(editor.document().lines, ["hell"]);
        assert_eq!(editor.selections, caret(4));
        editor.move_horizontally(false, false);
        assert_eq!(editor.selections, caret(3));
        editor.cursor_undo();
        assert_eq!(editor.selections, caret(4));
    }

    #[test]
    fn issue_82535_edge_case_with_cursor_undo_reverse() {
        let mut editor = editor("", "\n");
        editor.type_text("hell");
        editor.type_text("o");
        assert_eq!(editor.document().lines, ["hello"]);
        assert_eq!(editor.selections, caret(5));
        editor.cursor_undo();
        assert_eq!(editor.selections, caret(5));
    }

    #[test]
    fn redo_steps_forward_and_a_new_move_drops_it() {
        let mut editor = editor("hello", "\n");
        editor.move_horizontally(true, false);
        editor.move_horizontally(true, false);
        assert!(editor.cursor_undo().is_some());
        assert_eq!(editor.selections, caret(1));
        editor.cursor_redo();
        assert_eq!(editor.selections, caret(2));
        editor.cursor_undo();
        editor.move_horizontally(true, true);
        assert!(editor.cursor_redo().is_none());
        editor.cursor_undo();
        editor.cursor_undo();
        assert_eq!(editor.selections, caret(0));
        assert!(editor.cursor_undo().is_none());
    }

    #[test]
    fn an_edit_drops_the_states() {
        let mut editor = editor("hello", "\n");
        editor.move_horizontally(true, false);
        editor.type_text("x");
        assert!(editor.cursor_undo().is_none());
    }
}
