//! editor/contrib/caretOperations/browser: Move Selected Text Left/Right (caretOperations.ts, moveCaretCommand.ts)
//! and Transpose Letters (transpose.ts).

use super::edit::Command;
use super::{ContentChanged, OpenEditor, Position, Replace, Selection, cursor, text_between};

impl OpenEditor {
    /// `MoveCaretAction.run`: a `MoveCaretCommand` per selection, which swaps a one-line selection with the char
    /// before or after it.
    pub fn move_selected_text(&mut self, left: bool) -> Option<ContentChanged> {
        let lines = &self.document.lines;
        let commands = self.selections.iter().map(|&selection| move_caret_command(lines, selection, left)).collect();
        let id = if left { "editor.action.moveCarretLeftAction" } else { "editor.action.moveCarretRightAction" };
        self.run_commands_between_stops(commands, id)
    }

    /// `TransposeLettersAction.run`: each empty cursor swaps the chars around it, or the two before it at a line end.
    pub fn transpose_letters(&mut self) -> Option<ContentChanged> {
        let lines = &self.document.lines;
        let mut commands = Vec::new();
        for selection in &self.selections {
            if !selection.is_empty() {
                continue;
            }
            let position = selection.head;
            let line_len = cursor::line_len(lines, position.line);
            if position.line == 0 && (position.column == 0 || (position.column == 1 && line_len == 1)) {
                continue;
            }
            let end = if position.column == line_len { position } else { cursor::right(lines, position) };
            let middle = cursor::left(lines, end);
            let begin = cursor::left(lines, middle);
            let swapped = text_between(lines, middle, end) + &text_between(lines, begin, middle);
            // ReplaceCommand: the cursor lands after the inserted text.
            commands.push(Command { edits: vec![Replace::new(begin, end, swapped)], tracked: None, cursor_state: Box::new(|_, ranges| Selection::caret(ranges[0].1)) });
        }
        if commands.is_empty() {
            return None;
        }
        // `executeCommands` gives command `i` to cursor `i`, also when an earlier cursor had none; the rest keep theirs.
        while commands.len() < self.selections.len() {
            commands.push(Command { edits: Vec::new(), tracked: None, cursor_state: Box::new(|_, _| unreachable!("a command without edits keeps its selection")) });
        }
        self.run_commands_between_stops(commands, "editor.action.transposeLetters")
    }
}

/// `MoveCaretCommand`: nothing for an empty or multi-line selection or one at the line edge it moves toward.
fn move_caret_command(lines: &[String], selection: Selection, left: bool) -> Command {
    let (start, end) = (selection.start(), selection.end());
    let none = || Command { edits: Vec::new(), tracked: None, cursor_state: Box::new(|_, _| unreachable!("a command without edits keeps its selection")) };
    if start.line != end.line || selection.is_empty() {
        return none();
    }
    let line = start.line;
    if (left && start.column == 0) || (!left && end.column == cursor::line_len(lines, line)) {
        return none();
    }
    let at = |column| Position::new(line, column);
    let edits = if left {
        let before = text_between(lines, at(start.column - 1), start);
        vec![Replace::new(at(start.column - 1), start, ""), Replace::new(end, end, before)]
    } else {
        let after = text_between(lines, end, at(end.column + 1));
        vec![Replace::new(end, at(end.column + 1), ""), Replace::new(start, start, after)]
    };
    let shift = |position: Position| if left { at(position.column - 1) } else { at(position.column + 1) };
    // computeCursorState builds the selection from start to end, so the direction is lost as upstream loses it.
    let moved = Selection { anchor: shift(start), head: shift(end) };
    Command { edits, tracked: None, cursor_state: Box::new(move |_, _| moved) }
}

#[cfg(test)]
mod tests {
    use crate::editor::Position;
    use crate::editor::Selection;
    use crate::editor::edit::tests::editor;

    /// 1-based (start line, start column, end line, end column) as upstream's `new Selection(...)`.
    fn selection((anchor_line, anchor_column, head_line, head_column): (usize, usize, usize, usize)) -> Selection {
        Selection { anchor: Position::new(anchor_line - 1, anchor_column - 1), head: Position::new(head_line - 1, head_column - 1) }
    }

    fn move_caret(left: bool, given: (usize, usize, usize, usize), expected_line: &str, expected: (usize, usize, usize, usize)) {
        let mut model = editor("012345", "\n");
        model.set_selections(vec![selection(given)]);
        model.move_selected_text(left);
        assert_eq!(model.document.lines, [expected_line]);
        assert_eq!(model.selections, [selection(expected)]);
    }

    /// moveCarretCommand.test.ts
    #[test]
    fn move_selected_text_swaps_it_with_the_char_beside_it() {
        move_caret(true, (1, 3, 1, 5), "023145", (1, 2, 1, 4));
        move_caret(false, (1, 3, 1, 5), "014235", (1, 4, 1, 6));
        move_caret(true, (1, 1, 1, 1), "012345", (1, 1, 1, 1));
        move_caret(false, (1, 5, 1, 7), "012345", (1, 5, 1, 7));
    }

    fn transpose(text: &str, at: (usize, usize)) -> (String, (usize, usize)) {
        let mut model = editor(text, "\n");
        model.set_selections(vec![selection((at.0, at.1, at.0, at.1))]);
        model.transpose_letters();
        let head = model.selections[0].head;
        (model.document.lines.join("\n"), (head.line + 1, head.column + 1))
    }

    #[test]
    fn transpose_letters_swaps_around_the_cursor_or_before_a_line_end() {
        assert_eq!(transpose("abc", (1, 2)), ("bac".into(), (1, 3)));
        assert_eq!(transpose("abc", (1, 4)), ("acb".into(), (1, 4)));
        assert_eq!(transpose("abc", (1, 1)), ("abc".into(), (1, 1)));
        assert_eq!(transpose("a", (1, 2)), ("a".into(), (1, 2)));
        assert_eq!(transpose("ab\ncd", (2, 1)), ("abc\nd".into(), (2, 1)));
    }
}
