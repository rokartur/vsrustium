//! The text participants of workbench/contrib/codeEditor/browser/saveParticipants.ts for an explicit save, in
//! the order `SaveParticipantsContribution` registers them; the selections stay as they were before the save.

use super::edit::EditReason;
use super::indentation::trim_trailing_whitespace_edits;
use super::{ContentChanged, OpenEditor, Position};

/// The `files.*` settings of the model's language and resource that turn each participant on.
pub struct SaveParticipants {
    pub trim_trailing_whitespace: bool,
    pub trim_in_regex_and_strings: bool,
    pub insert_final_newline: bool,
    pub trim_final_newlines: bool,
}

impl OpenEditor {
    /// `doSave` closes the undo group first; each participant's change follows in turn.
    pub fn run_save_participants(&mut self, participants: &SaveParticipants) -> Vec<ContentChanged> {
        self.push_undo_stop();
        let mut changes = Vec::new();
        if participants.trim_trailing_whitespace {
            changes.extend(self.trim_trailing_whitespace_on_save(participants.trim_in_regex_and_strings));
        }
        if participants.insert_final_newline {
            changes.extend(self.insert_final_newline_on_save());
        }
        if participants.trim_final_newlines {
            changes.extend(self.trim_final_newlines_on_save());
        }
        changes
    }

    /// `TrimWhitespaceParticipant`: no cursor keeps its whitespace on an explicit save.
    fn trim_trailing_whitespace_on_save(&mut self, trim_in_regex_and_strings: bool) -> Option<ContentChanged> {
        let lines = &self.document.lines;
        let tokens = &self.document.tokens;
        let keep = |line: usize, offset: usize| {
            use textmate::StandardTokenType::{RegEx, String};
            !trim_in_regex_and_strings && matches!(tokens.token_type_at(lines, line, offset), String | RegEx)
        };
        let edits = trim_trailing_whitespace_edits(lines, &[], keep).into_iter().map(|(start, end, text)| (start, end, text, false)).collect();
        self.execute_edits(edits, Some(self.selections.clone()), EditReason::Unknown("trimTrailingWhitespace"))
    }

    /// `FinalNewLineParticipant`: the EOL after a last line with other than whitespace.
    fn insert_final_newline_on_save(&mut self) -> Option<ContentChanged> {
        let last = self.document.lines.len() - 1;
        let text = &self.document.lines[last];
        if text.trim_matches([' ', '\t']).is_empty() {
            return None;
        }
        let end = Position::new(last, text.chars().count());
        let edits = vec![(end, end, self.document.eol.to_string(), false)];
        self.execute_edits(edits, Some(self.selections.clone()), EditReason::Unknown("insertFinalNewLine"))
    }

    /// `TrimFinalNewLinesParticipant`: the empty lines after the last line with text go, but for the line break
    /// that ends it.
    fn trim_final_newlines_on_save(&mut self) -> Option<ContentChanged> {
        let lines = &self.document.lines;
        if lines.len() == 1 {
            return None;
        }
        let delete_from = lines.iter().rposition(|line| !line.is_empty()).map_or(0, |line| line + 1);
        let last = lines.len() - 1;
        let end = Position::new(last, lines[last].chars().count());
        if delete_from > last || Position::new(delete_from, 0) == end {
            return None;
        }
        let edits = vec![(Position::new(delete_from, 0), end, String::new(), false)];
        self.execute_edits(edits, Some(self.selections.clone()), EditReason::Unknown("trimFinalNewlines"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Selection;
    use crate::editor::edit::tests::editor;

    const NONE: SaveParticipants = SaveParticipants { trim_trailing_whitespace: false, trim_in_regex_and_strings: false, insert_final_newline: false, trim_final_newlines: false };
    const INSERT: SaveParticipants = SaveParticipants { insert_final_newline: true, ..NONE };
    const TRIM_FINAL: SaveParticipants = SaveParticipants { trim_final_newlines: true, ..NONE };

    fn saved(text: &str, participants: &SaveParticipants) -> String {
        let mut model = editor(text, "\n");
        model.run_save_participants(participants);
        model.document.lines.join("\n")
    }

    /// saveParticipant.test.ts 'insert final new line'.
    #[test]
    fn insert_final_new_line() {
        assert_eq!(saved("", &INSERT), "");
        assert_eq!(saved("Hello New Line\n", &INSERT), "Hello New Line\n");
        assert_eq!(saved("Hello New Line", &INSERT), "Hello New Line\n");
        assert_eq!(saved("Hello New Line\nHello New Line\nHello New Line", &INSERT), "Hello New Line\nHello New Line\nHello New Line\n");
    }

    /// saveParticipant.test.ts 'trim final new lines'.
    #[test]
    fn trim_final_new_lines() {
        assert_eq!(saved("Trim New Line", &TRIM_FINAL), "Trim New Line");
        assert_eq!(saved("Trim New Line\n", &TRIM_FINAL), "Trim New Line\n");
        assert_eq!(saved("Trim New Line\n\n", &TRIM_FINAL), "Trim New Line\n");
        assert_eq!(saved("Trim New Line\nTrim New Line\n\n\n", &TRIM_FINAL), "Trim New Line\nTrim New Line\n");
        assert_eq!(saved("\n\n", &TRIM_FINAL), "");
        assert_eq!(saved("Trim New Line\n  \n", &TRIM_FINAL), "Trim New Line\n  \n");
    }

    /// saveParticipant.test.ts 'trim final new lines bug#39750': a save that changes nothing keeps the redo stack.
    #[test]
    fn trim_final_new_lines_keeps_the_redo_stack() {
        let mut model = editor("Trim New Line", "\n");
        model.execute_edits(vec![(Position::new(0, 13), Position::new(0, 13), ".".into(), false)], None, EditReason::Unknown("test"));
        model.undo();
        model.run_save_participants(&TRIM_FINAL);
        model.redo();
        assert_eq!(model.document.lines, ["Trim New Line."]);
    }

    /// saveParticipant.test.ts 'trim final new lines bug#46075': saving again changes nothing, and undo goes back at once.
    #[test]
    fn trim_final_new_lines_saved_many_times_undo_in_one_step() {
        let mut model = editor("Test\n\n", "\n");
        for _ in 0..10 {
            model.run_save_participants(&TRIM_FINAL);
        }
        assert_eq!(model.document.lines, ["Test", ""]);
        model.undo();
        assert_eq!(model.document.lines, ["Test", "", ""]);
        model.redo();
        assert_eq!(model.document.lines, ["Test", ""]);
    }

    /// saveParticipant.test.ts 'trim whitespace'.
    #[test]
    fn trim_whitespace() {
        let mut model = editor("Test \t", "\n");
        for _ in 0..10 {
            model.run_save_participants(&SaveParticipants { trim_trailing_whitespace: true, ..NONE });
        }
        assert_eq!(model.document.lines, ["Test"]);
    }

    /// The participants run in their registration order and undo together; the selections stay.
    #[test]
    fn the_participants_run_in_order_as_one_undo_step() {
        let mut model = editor("a  \nb  \n\n", "\n");
        model.move_to(Position::new(1, 1), false);
        model.run_save_participants(&SaveParticipants { trim_trailing_whitespace: true, insert_final_newline: true, trim_final_newlines: true, ..NONE });
        assert_eq!(model.document.lines, ["a", "b", ""]);
        assert_eq!(model.selection(), Selection::caret(Position::new(1, 1)));
        model.undo();
        assert_eq!(model.document.lines, ["a  ", "b  ", "", ""]);
    }
}
