//! editor/contrib/dnd/browser: the edits of a drag and drop of selected text (dragAndDropCommand.ts) and the
//! selections a press on a selection leaves when it is released without a drag (dnd.ts `_onEditorMouseDrop`).

use super::edit::Command;
use super::{ContentChanged, EditReason, OpenEditor, Position, Replace, Selection, text_between};

impl OpenEditor {
    /// `_onEditorMouseDrag`: the one selection the pointer is in, which the drag moves.
    pub fn dragged_selection_at(&self, position: Position) -> Option<Selection> {
        let mut containing = self.selections.iter().filter(|selection| contains(**selection, position));
        let first = containing.next().copied();
        if containing.next().is_some() {
            return None;
        }
        first
    }

    /// `_onEditorMouseDrop` without a drag: Shift extends the primary selection to `position`, else the
    /// selections around it collapse there.
    pub fn release_press_on_selection(&mut self, position: Position, shift: bool) {
        let selections = match shift {
            true => vec![Selection { anchor: self.selection().anchor, head: position }],
            false => self.selections.iter().map(|&selection| if contains(selection, position) { Selection::caret(position) } else { selection }).collect(),
        };
        self.set_selections(selections);
    }

    /// `executeCommand(DragAndDropCommand)` between undo stops: the dragged text moves, or with `copy` is copied,
    /// to `target`; only the primary cursor stays (`killSecondaryCursors`).
    pub fn drop_dragged_text(&mut self, dragged: Selection, target: Position, copy: bool) -> Option<ContentChanged> {
        self.selections.truncate(1);
        let text = text_between(&self.document().lines, dragged.start(), dragged.end());
        let mut edits = Vec::new();
        if !copy {
            edits.push(Replace::new(dragged.start(), dragged.end(), ""));
        }
        edits.push(Replace::new(target, target, text));
        let selection = target_selection(dragged, target, copy);
        let command = Command { edits, tracked: None, cursor_state: Box::new(move |_, _| selection) };
        self.push_undo_stop();
        let change = self.execute_commands(vec![command], EditReason::Cursor { kind: "executeCommand", detailed_source: "editor.contrib.dragAndDrop" });
        self.push_undo_stop();
        change
    }
}

/// `Range.containsPosition`: the edges count.
pub fn contains(selection: Selection, position: Position) -> bool {
    selection.start() <= position && position <= selection.end()
}

/// `DragAndDropCommand.getEditOperations`: where the dropped text ends up selected, from the positions before the edits.
fn target_selection(dragged: Selection, target: Position, copy: bool) -> Selection {
    let (start, end) = (dragged.start(), dragged.end());
    let at = |line, column| Position::new(line, column);
    let selection = |anchor, head| Selection { anchor, head };
    let one_line = start.line == end.line;
    let end_column = |from: usize| if one_line { from + end.column - start.column } else { end.column };
    if contains(dragged, target) && !(copy && (target == end || target == start)) {
        return dragged;
    }
    if copy {
        return selection(target, at(end.line - start.line + target.line, end_column(target.column)));
    }
    if target.line > end.line {
        return selection(at(target.line - end.line + start.line, target.column), at(target.line, end_column(target.column)));
    }
    if target.line < end.line {
        return selection(target, at(target.line + end.line - start.line, end_column(target.column)));
    }
    if end.column <= target.column {
        let head_column = if one_line { target.column } else { end.column };
        return selection(at(target.line - end.line + start.line, target.column - end.column + start.column), at(target.line, head_column));
    }
    selection(at(target.line - end.line + start.line, target.column), at(target.line, target.column + end.column - start.column))
}

#[cfg(test)]
mod tests {
    use crate::editor::Position;
    use crate::editor::Selection;
    use crate::editor::edit::tests::editor;

    fn drop(text: &str, dragged: ((usize, usize), (usize, usize)), target: (usize, usize), copy: bool) -> (String, Selection) {
        let mut model = editor(text, "\n");
        let dragged = Selection { anchor: Position::new(dragged.0.0, dragged.0.1), head: Position::new(dragged.1.0, dragged.1.1) };
        model.set_selections(vec![dragged]);
        model.drop_dragged_text(dragged, Position::new(target.0, target.1), copy);
        (model.document().lines.join("\n"), model.selections[0])
    }

    fn selection(anchor: (usize, usize), head: (usize, usize)) -> Selection {
        Selection { anchor: Position::new(anchor.0, anchor.1), head: Position::new(head.0, head.1) }
    }

    #[test]
    fn a_drop_moves_the_text_and_selects_it_where_it_lands() {
        // After the selection on its line, before it, and on lines below and above.
        assert_eq!(drop("Port notes for", ((0, 5), (0, 10)), (0, 14), false), ("Port  fornotes".into(), selection((0, 9), (0, 14))));
        assert_eq!(drop("Port notes for", ((0, 5), (0, 10)), (0, 0), false), ("notesPort  for".into(), selection((0, 0), (0, 5))));
        assert_eq!(drop("ab cd\nxyz", ((0, 3), (0, 5)), (1, 1), false), ("ab \nxcdyz".into(), selection((1, 1), (1, 3))));
        assert_eq!(drop("xyz\nab cd", ((1, 0), (1, 2)), (0, 3), false), ("xyzab\n cd".into(), selection((0, 3), (0, 5))));
        // A multi-line selection keeps its end column.
        assert_eq!(drop("ab\ncd\nef", ((0, 1), (1, 1)), (2, 2), false), ("ad\nefb\nc".into(), selection((1, 2), (2, 1))));
    }

    #[test]
    fn a_copy_drop_keeps_the_text_and_may_land_beside_it() {
        assert_eq!(drop("ab cd", ((0, 3), (0, 5)), (0, 0), true), ("cdab cd".into(), selection((0, 0), (0, 2))));
        assert_eq!(drop("ab cd", ((0, 3), (0, 5)), (0, 5), true), ("ab cdcd".into(), selection((0, 5), (0, 7))));
    }

    #[test]
    fn a_press_released_on_a_selection_collapses_it_there() {
        let mut model = editor("ab cd", "\n");
        model.set_selections(vec![selection((0, 0), (0, 2)), selection((0, 3), (0, 5))]);
        assert_eq!(model.dragged_selection_at(Position::new(0, 4)), Some(selection((0, 3), (0, 5))));
        model.release_press_on_selection(Position::new(0, 4), false);
        assert_eq!(model.selections, [selection((0, 0), (0, 2)), selection((0, 4), (0, 4))]);
        model.release_press_on_selection(Position::new(0, 5), true);
        assert_eq!(model.selections, [selection((0, 0), (0, 5))]);
    }
}
