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
    /// An auto save (`SaveReason.AUTO`) spares the lines of the cursors of the code editor showing the model.
    pub spared_cursors: Vec<Position>,
}

impl OpenEditor {
    /// `doSave` closes the undo group first; each participant's change follows in turn.
    pub fn run_save_participants(&mut self, participants: &SaveParticipants) -> Vec<ContentChanged> {
        self.push_undo_stop();
        let mut changes = Vec::new();
        if participants.trim_trailing_whitespace {
            changes.extend(self.trim_trailing_whitespace_on_save(participants.trim_in_regex_and_strings, &participants.spared_cursors));
        }
        if participants.insert_final_newline {
            changes.extend(self.insert_final_newline_on_save());
        }
        if participants.trim_final_newlines {
            changes.extend(self.trim_final_newlines_on_save(&participants.spared_cursors));
        }
        changes
    }

    /// `TrimWhitespaceParticipant`: only an auto save keeps the whitespace before a cursor.
    fn trim_trailing_whitespace_on_save(&mut self, trim_in_regex_and_strings: bool, spared_cursors: &[Position]) -> Option<ContentChanged> {
        let lines = &self.document.lines;
        let tokens = &self.document.tokens;
        let keep = |line: usize, offset: usize| {
            use textmate::StandardTokenType::{RegEx, String};
            !trim_in_regex_and_strings && matches!(tokens.token_type_at(lines, line, offset), String | RegEx)
        };
        let edits = trim_trailing_whitespace_edits(lines, spared_cursors, keep).into_iter().map(|(start, end, text)| (start, end, text, false)).collect();
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
    /// that ends it; an auto save keeps the lines up to the last spared cursor.
    fn trim_final_newlines_on_save(&mut self, spared_cursors: &[Position]) -> Option<ContentChanged> {
        let lines = &self.document.lines;
        if lines.len() == 1 {
            return None;
        }
        let last_with_text = lines.iter().rposition(|line| !line.is_empty()).map_or(0, |line| line + 1);
        let cannot_touch = spared_cursors.iter().map(|cursor| cursor.line + 1).max().unwrap_or(0);
        let delete_from = last_with_text.max(cannot_touch);
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

    fn none() -> SaveParticipants {
        SaveParticipants { trim_trailing_whitespace: false, trim_in_regex_and_strings: false, insert_final_newline: false, trim_final_newlines: false, spared_cursors: Vec::new() }
    }

    fn insert() -> SaveParticipants {
        SaveParticipants { insert_final_newline: true, ..none() }
    }

    fn trim_final() -> SaveParticipants {
        SaveParticipants { trim_final_newlines: true, ..none() }
    }

    fn saved(text: &str, participants: &SaveParticipants) -> String {
        let mut model = editor(text, "\n");
        model.run_save_participants(participants);
        model.document.lines.join("\n")
    }

    /// saveParticipant.test.ts 'insert final new line'.
    #[test]
    fn insert_final_new_line() {
        assert_eq!(saved("", &insert()), "");
        assert_eq!(saved("Hello New Line\n", &insert()), "Hello New Line\n");
        assert_eq!(saved("Hello New Line", &insert()), "Hello New Line\n");
        assert_eq!(saved("Hello New Line\nHello New Line\nHello New Line", &insert()), "Hello New Line\nHello New Line\nHello New Line\n");
    }

    /// saveParticipant.test.ts 'trim final new lines'.
    #[test]
    fn trim_final_new_lines() {
        assert_eq!(saved("Trim New Line", &trim_final()), "Trim New Line");
        assert_eq!(saved("Trim New Line\n", &trim_final()), "Trim New Line\n");
        assert_eq!(saved("Trim New Line\n\n", &trim_final()), "Trim New Line\n");
        assert_eq!(saved("Trim New Line\nTrim New Line\n\n\n", &trim_final()), "Trim New Line\nTrim New Line\n");
        assert_eq!(saved("\n\n", &trim_final()), "");
        assert_eq!(saved("Trim New Line\n  \n", &trim_final()), "Trim New Line\n  \n");
    }

    /// saveParticipant.test.ts 'trim final new lines bug#39750': a save that changes nothing keeps the redo stack.
    #[test]
    fn trim_final_new_lines_keeps_the_redo_stack() {
        let mut model = editor("Trim New Line", "\n");
        model.execute_edits(vec![(Position::new(0, 13), Position::new(0, 13), ".".into(), false)], None, EditReason::Unknown("test"));
        model.undo();
        model.run_save_participants(&trim_final());
        model.redo();
        assert_eq!(model.document.lines, ["Trim New Line."]);
    }

    /// saveParticipant.test.ts 'trim final new lines bug#46075': saving again changes nothing, and undo goes back at once.
    #[test]
    fn trim_final_new_lines_saved_many_times_undo_in_one_step() {
        let mut model = editor("Test\n\n", "\n");
        for _ in 0..10 {
            model.run_save_participants(&trim_final());
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
            model.run_save_participants(&SaveParticipants { trim_trailing_whitespace: true, ..none() });
        }
        assert_eq!(model.document.lines, ["Test"]);
    }

    /// saveParticipants.ts with `SaveReason.AUTO`: the whitespace before a cursor and the empty lines up to the
    /// cursor's line stay, as the user is typing there.
    #[test]
    fn an_auto_save_spares_the_cursor_lines() {
        let mut model = editor("a  \nb  \n\n\n", "\n");
        let spared_cursors = vec![Position::new(1, 3), Position::new(2, 0)];
        model.run_save_participants(&SaveParticipants { trim_trailing_whitespace: true, trim_final_newlines: true, spared_cursors, ..none() });
        assert_eq!(model.document.lines, ["a", "b  ", "", ""]);
    }

    /// The participants run in their registration order and undo together; the selections stay.
    #[test]
    fn the_participants_run_in_order_as_one_undo_step() {
        let mut model = editor("a  \nb  \n\n", "\n");
        model.move_to(Position::new(1, 1), false);
        model.run_save_participants(&SaveParticipants { trim_trailing_whitespace: true, insert_final_newline: true, trim_final_newlines: true, ..none() });
        assert_eq!(model.document.lines, ["a", "b", ""]);
        assert_eq!(model.selection(), Selection::caret(Position::new(1, 1)));
        model.undo();
        assert_eq!(model.document.lines, ["a  ", "b  ", "", ""]);
    }
}
