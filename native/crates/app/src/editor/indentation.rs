//! The indentation actions of editor/contrib/indentation/browser/indentation.ts and the trim command of
//! editor/common/commands/trimTrailingWhitespaceCommand.ts, on the model's options and lines.

use super::edit::command;
use super::{ContentChanged, Indentation, OpenEditor, Position, guess_indentation, indent_guides};

impl OpenEditor {
    /// `model.updateOptions`: the indent guides follow the new sizes.
    pub fn set_indentation(&mut self, indentation: Indentation) {
        self.document.indentation = indentation;
        self.guides = indent_guides(&self.document.lines, indentation.tab_size, indentation.indent_size(), self.document.off_side);
    }

    /// `TextModel.detectIndentation`: the guess sets the indent size to the guessed tab size.
    pub fn detect_indentation(&mut self, insert_spaces: bool, tab_size: usize) {
        let guessed = guess_indentation(&self.document.lines, Indentation { tab_size, indent_size: None, insert_spaces });
        self.set_indentation(Indentation { indent_size: Some(guessed.tab_size), ..guessed });
    }

    /// `IndentationToSpacesAction` and `IndentationToTabsAction`: every line's indentation converted between
    /// tabs and `tabSize` spaces between undo stops, the primary selection tracked, then `insertSpaces` set.
    pub fn convert_indentation(&mut self, to_spaces: bool) -> Option<ContentChanged> {
        let id = if to_spaces { "editor.action.indentationToSpaces" } else { "editor.action.indentationToTabs" };
        let edits = indentation_edits(&self.document.lines, self.document.indentation.tab_size, to_spaces);
        self.push_undo_stop();
        let change = self.execute_edits_tracking_primary(edits, command(id));
        self.push_undo_stop();
        self.set_indentation(Indentation { insert_spaces: to_spaces, ..self.document.indentation });
        change
    }

    /// `TrimTrailingWhitespaceAction` without `reason: 'auto-save'`: no cursor keeps its whitespace.
    pub fn trim_trailing_whitespace(&mut self, trim_in_regex_and_strings: bool) -> Option<ContentChanged> {
        let lines = &self.document.lines;
        let tokens = &self.document.tokens;
        let in_string_or_regex = |line: usize, offset: usize| {
            use textmate::StandardTokenType::{RegEx, String};
            // shortcut: the line is tokenized when needed, where upstream skips lines without accurate tokens.
            matches!(tokens.token_type_at(lines, line, offset), String | RegEx)
        };
        let edits = trim_trailing_whitespace_edits(lines, &[], |line, offset| !trim_in_regex_and_strings && in_string_or_regex(line, offset));
        self.push_undo_stop();
        let change = self.execute_edits_tracking_primary(edits, command("editor.action.trimTrailingWhitespace"));
        self.push_undo_stop();
        change
    }
}

/// `getIndentationEditOperations`: each indented line's leading whitespace, tabs replaced by `tab_size` spaces,
/// or each run of `tab_size` spaces by a tab; none for an empty model.
fn indentation_edits(lines: &[String], tab_size: usize, to_spaces: bool) -> Vec<(Position, Position, String)> {
    if lines.len() == 1 && lines[0].is_empty() {
        return Vec::new();
    }
    let spaces = " ".repeat(tab_size);
    let mut edits = Vec::new();
    for (line, text) in lines.iter().enumerate() {
        let indentation_end = text.find(|c| c != ' ' && c != '\t').unwrap_or(text.len());
        if indentation_end == 0 {
            continue;
        }
        let indentation = &text[..indentation_end];
        let converted = if to_spaces { indentation.replace('\t', &spaces) } else { indentation.replace(&spaces, "\t") };
        edits.push((Position::new(line, 0), Position::new(line, indentation_end), converted));
    }
    edits
}

/// `trimTrailingWhitespace`: deletes each line's trailing whitespace, not left of a cursor on the line (the last
/// cursor of a line counts), and not where `keep(line, byte offset)` holds.
fn trim_trailing_whitespace_edits(lines: &[String], cursors: &[Position], keep: impl Fn(usize, usize) -> bool) -> Vec<(Position, Position, String)> {
    let mut edits = Vec::new();
    for (line, text) in lines.iter().enumerate() {
        let length = text.chars().count();
        let cursor = cursors.iter().filter(|cursor| cursor.line == line).map(|cursor| cursor.column).max();
        if cursor == Some(length) || text.is_empty() {
            continue;
        }
        let content_end = text.trim_end_matches([' ', '\t']).len();
        if content_end == text.len() {
            continue;
        }
        // Upstream looks up the token at the 1-based start column as a 0-based offset: one char into the whitespace.
        if keep(line, content_end + 1) {
            continue;
        }
        let from = text[..content_end].chars().count().max(cursor.unwrap_or(0));
        edits.push((Position::new(line, from), Position::new(line, length), String::new()));
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Selection;
    use crate::editor::edit::tests::editor;

    /// `testIndentationToSpacesCommand` and `testIndentationToTabsCommand` of indentation.test.ts, with 0-based positions.
    fn assert_converted(lines: &[&str], caret: (usize, usize), tab_size: usize, to_spaces: bool, expected: &[&str], expected_caret: (usize, usize)) {
        let mut model = editor(&lines.join("\n"), "\n");
        model.set_indentation(Indentation { tab_size, indent_size: None, insert_spaces: !to_spaces });
        model.move_to(Position::new(caret.0, caret.1), false);
        model.convert_indentation(to_spaces);
        assert_eq!(model.document.lines, expected);
        assert_eq!(model.selection(), Selection::caret(Position::new(expected_caret.0, expected_caret.1)));
        assert_eq!(model.document.indentation.insert_spaces, to_spaces);
    }

    #[test]
    fn change_indentation_to_spaces() {
        assert_converted(&["first", "second line", "third line", "\tfourth line", "\tfifth"], (1, 2), 4, true, &["first", "second line", "third line", "    fourth line", "    fifth"], (1, 2));
        assert_converted(&["\t\tfirst", "\tsecond line", "\t\t\t third line", "fourth line", "fifth"], (0, 4), 3, true, &["      first", "   second line", "          third line", "fourth line", "fifth"], (0, 8));
        assert_converted(&["\t\tfirst\t", "\tsecond  \t line \t", "\t\t\t third line", " \tfourth line", "fifth"], (0, 4), 2, true, &["    first\t", "  second  \t line \t", "       third line", "   fourth line", "fifth"], (0, 6));
        assert_converted(&["\t\t\t", "\t", "\t\t"], (0, 3), 2, true, &["      ", "  ", "    "], (0, 3));
        // issue #45996
        assert_converted(&["\tabc"], (0, 2), 4, true, &["    abc"], (0, 5));
    }

    #[test]
    fn change_indentation_to_tabs() {
        assert_converted(&["    first", "second line", "    third line", "fourth line", "fifth"], (1, 2), 4, false, &["\tfirst", "second line", "\tthird line", "fourth line", "fifth"], (1, 2));
        assert_converted(&["first", "   second line", "          third line", "fourth line", "     fifth"], (0, 4), 3, false, &["first", "\tsecond line", "\t\t\t third line", "fourth line", "\t  fifth"], (0, 4));
        assert_converted(&["      first   ", "  second     line \t", "       third line", "   fourth line", "fifth"], (0, 7), 2, false, &["\t\t\tfirst   ", "\tsecond     line \t", "\t\t\t third line", "\t fourth line", "fifth"], (0, 4));
    }

    fn trims(lines: &[&str], cursors: &[(usize, usize)]) -> Vec<(usize, usize, usize)> {
        let lines: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
        let cursors: Vec<Position> = cursors.iter().map(|&(line, column)| Position::new(line, column)).collect();
        let edits = trim_trailing_whitespace_edits(&lines, &cursors, |_, _| false);
        edits.into_iter().map(|(start, end, _)| (start.line, start.column, end.column)).collect()
    }

    /// trimTrailingWhitespaceCommand.test.ts 'remove trailing whitespace', 0-based: (line, start column, end column).
    #[test]
    fn remove_trailing_whitespace() {
        assert_eq!(trims(&[""], &[]), []);
        assert_eq!(trims(&["text"], &[]), []);
        assert_eq!(trims(&["text   "], &[]), [(0, 4, 7)]);
        assert_eq!(trims(&["text\t   "], &[]), [(0, 4, 8)]);
        assert_eq!(trims(&["\t   "], &[]), [(0, 0, 4)]);
        assert_eq!(trims(&["text\t"], &[]), [(0, 4, 5)]);
        let mixed = ["some text\t", "some more text", "\t  ", "even more text  ", "and some mixed\t   \t"];
        assert_eq!(trims(&mixed, &[]), [(0, 9, 10), (2, 0, 3), (3, 14, 16), (4, 14, 19)]);
        assert_eq!(trims(&["text   "], &[(0, 0), (0, 1), (0, 2)]), [(0, 4, 7)]);
        assert_eq!(trims(&["text   "], &[(0, 0), (0, 4)]), [(0, 4, 7)]);
        assert_eq!(trims(&["text   "], &[(0, 0), (0, 4), (0, 5)]), [(0, 5, 7)]);
        assert_eq!(trims(&mixed, &[(0, 10), (2, 1), (4, 0), (3, 0), (4, 9)]), [(2, 1, 3), (3, 14, 16), (4, 14, 19)]);
    }

    /// 'skips strings and regex if configured': the whitespace starting in a string stays.
    #[test]
    fn skips_trailing_whitespace_that_starts_in_a_string() {
        let lines: Vec<String> = ["const a = `  ", "  a string  ", "`;  "].iter().map(|line| line.to_string()).collect();
        let in_string = |line: usize, offset: usize| line == 0 && offset >= 10 || line == 1 || line == 2 && offset < 1;
        let edits = trim_trailing_whitespace_edits(&lines, &[], in_string);
        assert_eq!(edits, [(Position::new(2, 2), Position::new(2, 4), String::new())]);
    }

    #[test]
    fn trim_keeps_the_primary_selection() {
        let mut model = editor("a  \nb\t", "\n");
        model.move_to(Position::new(1, 1), false);
        model.trim_trailing_whitespace(true);
        assert_eq!((model.document.lines.as_slice(), model.selection()), (["a", "b"].map(String::from).as_slice(), Selection::caret(Position::new(1, 1))));
    }
}
