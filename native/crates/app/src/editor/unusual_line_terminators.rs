//! textModel.ts `removeUnusualLineTerminators`: LINE SEPARATOR and PARAGRAPH SEPARATOR (strings.ts
//! `UNUSUAL_LINE_TERMINATORS`) are deleted, as one edit each.

use super::{ContentChanged, EditReason, OpenEditor, Position};

fn is_unusual_line_terminator(c: char) -> bool {
    matches!(c, '\u{2028}' | '\u{2029}')
}

pub fn has_unusual_line_terminators(lines: &[String]) -> bool {
    lines.iter().any(|line| line.contains(is_unusual_line_terminator))
}

impl OpenEditor {
    pub fn remove_unusual_line_terminators(&mut self) -> Option<ContentChanged> {
        let mut edits = Vec::new();
        for (line, text) in self.document().lines.iter().enumerate() {
            for (column, _) in text.chars().enumerate().filter(|(_, c)| is_unusual_line_terminator(*c)) {
                edits.push((Position::new(line, column), Position::new(line, column + 1), String::new(), false));
            }
        }
        self.execute_edits(edits, None, EditReason::Unknown("removeUnusualLineTerminators"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Selection;
    use crate::editor::edit::tests::editor;

    #[test]
    fn removes_every_ls_and_ps_and_keeps_the_cursor_on_its_text() {
        let mut editor = editor("a\u{2028}b\u{2029}c\nplain\n\u{2029}", "\n");
        assert!(has_unusual_line_terminators(&editor.document().lines));
        editor.selections = vec![Selection::caret(Position::new(0, 4))];
        let change = editor.remove_unusual_line_terminators().expect("edited");
        assert_eq!(editor.document().lines, ["abc", "plain", ""]);
        assert_eq!(editor.selections, [Selection::caret(Position::new(0, 2))]);
        assert!(!has_unusual_line_terminators(&editor.document().lines));
        assert!(!change.is_undoing);
        editor.undo();
        assert!(editor.last_change_was_undo());
        assert!(has_unusual_line_terminators(&editor.document().lines));
    }
}
