//! anchorSelect.ts `SelectionAnchorController`: the selection anchor of the group's code editor, a collapsed
//! `NeverGrowsWhenTypingAtEdges` decoration at the cursor.

use super::OpenEditor;
use super::cursor::{Position, Selection};
use super::edit::{end_of, text_between};
use super::folding::tracked;

impl OpenEditor {
    /// `setSelectionAnchor`: the anchor moves to the primary cursor.
    pub fn set_selection_anchor(&mut self) {
        self.selection_anchor = Some(self.selection().head);
    }

    /// `goToSelectionAnchor`: one cursor at the anchor (`setPosition`, which does not reveal).
    pub fn go_to_selection_anchor(&mut self) {
        if let Some(anchor) = self.selection_anchor {
            self.set_selections(vec![Selection::caret(anchor)]);
        }
    }

    /// `selectFromAnchorToCursor`: one selection from the anchor to the primary cursor, then the anchor goes.
    pub fn select_from_anchor_to_cursor(&mut self) {
        if let Some(anchor) = self.selection_anchor.take() {
            let head = self.selection().head;
            self.set_selections(vec![Selection { anchor, head }]);
        }
    }
}

/// intervalTree.ts `nodeAcceptEdit` for a collapsed `NeverGrowsWhenTypingAtEdges` marker, when `start..end` of
/// `lines` becomes `text`: inside the replaced range it keeps its offset while the inserted text reaches it
/// (`commonLength`), else it goes as `tracked` says.
pub(super) fn anchor_after_edit(anchor: Position, lines: &[String], start: Position, end: Position, text: &str) -> Position {
    if start < anchor && anchor < end {
        let offset = text_between(lines, start, anchor).chars().count();
        if offset < text.chars().count() {
            return end_of(start, &text.chars().take(offset).collect::<String>());
        }
    }
    tracked(anchor, start, end, end_of(start, text), false)
}

#[cfg(test)]
mod tests {
    use super::anchor_after_edit;
    use crate::editor::cursor::Position;

    fn moved(anchor: (usize, usize), start: (usize, usize), end: (usize, usize), text: &str) -> (usize, usize) {
        let lines = vec!["hello world".to_string(), "second".to_string()];
        let position = anchor_after_edit(Position::new(anchor.0, anchor.1), &lines, Position::new(start.0, start.1), Position::new(end.0, end.1), text);
        (position.line, position.column)
    }

    #[test]
    fn the_anchor_never_grows_when_typing_at_its_edges() {
        // Typing at the anchor pushes it past the text.
        assert_eq!(moved((0, 5), (0, 5), (0, 5), "ab"), (0, 7));
        // Typing before it shifts it, after it leaves it.
        assert_eq!(moved((0, 5), (0, 0), (0, 0), "a\nb"), (1, 6));
        assert_eq!(moved((0, 5), (0, 6), (0, 6), "x"), (0, 5));
        // A deletion starting at the anchor leaves it, one around it collapses it to the start.
        assert_eq!(moved((0, 5), (0, 5), (0, 8), ""), (0, 5));
        assert_eq!(moved((0, 5), (0, 2), (0, 8), ""), (0, 2));
        // A replace over it keeps its offset into the inserted text, or ends after a shorter one.
        assert_eq!(moved((0, 5), (0, 2), (0, 8), "abcdef"), (0, 5));
        assert_eq!(moved((0, 5), (0, 2), (0, 8), "ab"), (0, 4));
        assert_eq!(moved((1, 0), (0, 9), (1, 3), "abcdef"), (0, 12));
    }
}
