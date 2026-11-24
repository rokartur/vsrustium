//! Editing an open editor's text with one cursor: the cursor commands of coreCommands.ts and
//! cursorTypeOperations.ts, the undo stack of editStack.ts, and the content change events
//! mainThreadDocuments.ts forwards to the extension host. Line breaks inside edit text are "\n";
//! the host hears them in the document's EOL.

use super::cursor::{self, Position, Selection};
use super::{OpenEditor, indent_guides};

/// `EditOperationType`: what the last edit was, which decides where undo stops go.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum EditKind {
    #[default]
    Other,
    DeletingLeft,
    DeletingRight,
    TypingOther,
    TypingFirstSpace,
    TypingConsecutiveSpace,
}

impl EditKind {
    fn is_typing(self) -> bool {
        matches!(self, EditKind::TypingOther | EditKind::TypingFirstSpace | EditKind::TypingConsecutiveSpace)
    }

    fn is_space(self) -> bool {
        matches!(self, EditKind::TypingFirstSpace | EditKind::TypingConsecutiveSpace)
    }
}

/// `shouldPushStackElementBetween` (cursorTypeEditOperations.ts).
fn stop_between(previous: EditKind, next: EditKind) -> bool {
    if previous.is_typing() && !next.is_typing() {
        return true;
    }
    // `abc |d` keeps one undo group; `abc  |d` starts a new one at the second space.
    if previous == EditKind::TypingFirstSpace {
        return false;
    }
    previous != next && !(previous.is_space() && next.is_space())
}

/// The `TextModelEditSource` metadata the host receives as `detailedReason`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EditReason {
    /// `EditSources.cursor`: kind "type", "compositionType", "paste", "cut" or "executeCommands", and what ran it.
    Cursor { kind: &'static str, detailed_source: &'static str },
    /// `EditSources.unknown`, which `executeEdits` reports with the command id.
    Unknown(&'static str),
    /// `EditSources.applyEdits`, which undo and redo report.
    ApplyEdits,
}

const TYPED: EditReason = EditReason::Cursor { kind: "type", detailed_source: "keyboard" };

fn command(id: &'static str) -> EditReason {
    EditReason::Cursor { kind: "executeCommands", detailed_source: id }
}

/// `IModelContentChange`: 1-based lines and UTF-16 columns and offsets in the text before the change.
#[derive(Clone, Debug, PartialEq)]
pub struct ContentChange {
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub range_offset: usize,
    pub range_length: usize,
    pub text: String,
}

/// One model content change event, as `$acceptModelChanged` carries it.
#[derive(Debug)]
pub struct ContentChanged {
    pub changes: Vec<ContentChange>,
    pub version_id: u32,
    pub is_undoing: bool,
    pub is_redoing: bool,
    pub reason: EditReason,
    /// The dirty state before the change, so the caller can tell the host when it flips.
    pub was_dirty: bool,
}

/// The undo stack and the version ids dirty state is measured by (editStack.ts, textModel.ts).
#[derive(Debug)]
pub struct History {
    undo: Vec<UndoGroup>,
    redo: Vec<UndoGroup>,
    /// The last undo group takes the next edit until an undo stop is pushed.
    open: bool,
    last_kind: EditKind,
    /// `getAlternativeVersionId`: undo returns to the id the text had, so undoing to the saved text is clean.
    alternative_version_id: u32,
    saved_alternative_version_id: u32,
}

impl Default for History {
    fn default() -> Self {
        History { undo: Vec::new(), redo: Vec::new(), open: false, last_kind: EditKind::Other, alternative_version_id: 1, saved_alternative_version_id: 1 }
    }
}

#[derive(Debug)]
struct UndoGroup {
    edits: Vec<AppliedEdit>,
    selection_before: Selection,
    selection_after: Selection,
    alternative_before: u32,
    alternative_after: u32,
}

#[derive(Debug)]
struct AppliedEdit {
    start: Position,
    removed: String,
    inserted: String,
}

struct Replace {
    start: Position,
    end: Position,
    text: String,
}

impl OpenEditor {
    pub fn is_dirty(&self) -> bool {
        self.history.alternative_version_id != self.history.saved_alternative_version_id
    }

    /// `pushStackElement`: the next edit starts a new undo group.
    pub fn push_undo_stop(&mut self) {
        self.history.open = false;
    }

    /// A cursor move: an undo stop, the vertical leftover dropped, and the word highlights at the new head.
    pub fn move_to(&mut self, head: Position, select: bool) {
        self.push_undo_stop();
        self.selection = self.selection.moved(head, select);
        self.leftover = 0;
        self.refresh_highlights();
    }

    /// `cursorLeft`/`cursorRight`: a selection collapses to its start or end before the cursor moves.
    pub fn move_horizontally(&mut self, right: bool, select: bool) {
        let lines = &self.document.lines;
        let head = match (select || self.selection.is_empty(), right) {
            (true, false) => cursor::left(lines, self.selection.head),
            (true, true) => cursor::right(lines, self.selection.head),
            (false, false) => self.selection.start(),
            (false, true) => self.selection.end(),
        };
        self.move_to(head, select);
    }

    /// `cursorUp`/`cursorDown`: from a selection's start or end, keeping the visible column through short lines.
    pub fn move_vertically(&mut self, down: bool, select: bool) {
        let from = match (select || self.selection.is_empty(), down) {
            (true, _) => self.selection.head,
            (false, false) => self.selection.start(),
            (false, true) => self.selection.end(),
        };
        let (head, leftover) = cursor::vertical(&self.document.lines, from, self.leftover, down, self.document.indentation.tab_size);
        self.move_to(head, select);
        self.leftover = leftover;
    }

    /// `cursorWordStartLeft`/`cursorWordEndRight` from the head.
    pub fn move_word(&mut self, right: bool, select: bool) {
        let lines = &self.document.lines;
        let head = if right { cursor::word_right(lines, self.selection.head) } else { cursor::word_left(lines, self.selection.head) };
        self.move_to(head, select);
    }

    /// `cursorHome`/`cursorEnd`; Home toggles between the first non-blank character and column 0.
    pub fn move_to_line_edge(&mut self, end: bool, select: bool) {
        let lines = &self.document.lines;
        let head = if end { cursor::line_end(lines, self.selection.head) } else { cursor::line_start(lines, self.selection.head) };
        self.move_to(head, select);
    }

    /// `cursorTop`/`cursorBottom`.
    pub fn move_to_document_edge(&mut self, end: bool, select: bool) {
        let head = if end { cursor::last_position(&self.document.lines) } else { Position::default() };
        self.move_to(head, select);
    }

    pub fn select_all(&mut self) {
        self.push_undo_stop();
        self.selection = Selection { anchor: Position::default(), head: cursor::last_position(&self.document.lines) };
        self.refresh_highlights();
    }

    /// The word highlights at the head; a multi-line selection has none (wordHighlighter.ts `_onPositionChanged`).
    pub(super) fn refresh_highlights(&mut self) {
        let Selection { anchor, head } = self.selection;
        let text = &self.document.lines[head.line];
        self.highlights = match super::word_at(text, head.column).filter(|_| anchor.line == head.line) {
            Some(range) => super::word_occurrences(&self.document.lines, &text.chars().skip(range.start).take(range.len()).collect::<String>()),
            None => Vec::new(),
        };
    }

    /// The text `copy` puts on the clipboard: the selection, or the whole line with its line break
    /// for an empty one (`emptySelectionClipboard`); the flag says which.
    pub fn copy_text(&self) -> (String, bool) {
        let lines = &self.document.lines;
        if self.selection.is_empty() {
            return (format!("{}{}", lines[self.selection.head.line], self.document.eol), true);
        }
        (text_between(lines, self.selection.start(), self.selection.end()).replace('\n', self.document.eol), false)
    }

    /// `type`: text replaces the selection. Spaces and other text group into undo steps as upstream does.
    pub fn type_text(&mut self, text: &str) -> Option<ContentChanged> {
        let (start, end) = (self.selection.start(), self.selection.end());
        self.type_in_range(start, end, text, "type")
    }

    /// Typing that replaces `start..end`, as IME composition does with its marked text.
    pub fn type_in_range(&mut self, start: Position, end: Position, text: &str, kind: &'static str) -> Option<ContentChanged> {
        let text = normalize_line_breaks(text);
        let previous = self.history.last_kind;
        let typing = match text.as_str() {
            " " if previous.is_space() => EditKind::TypingConsecutiveSpace,
            " " => EditKind::TypingFirstSpace,
            _ => EditKind::TypingOther,
        };
        if stop_between(previous, typing) {
            self.push_undo_stop();
        }
        let caret = end_of(start, &text);
        let change = self.apply(vec![Replace { start, end, text }], Selection::caret(caret), EditReason::Cursor { kind, detailed_source: "keyboard" });
        self.history.last_kind = typing;
        change
    }

    /// Enter: a line break plus the indentation before the cursor.
    /// shortcut: the language's onEnter and indentation rules are not ported, so `{` does not indent the next line.
    pub fn enter(&mut self) -> Option<ContentChanged> {
        self.push_undo_stop();
        let (start, end) = (self.selection.start(), self.selection.end());
        let indent: String = self.document.lines[start.line].chars().take(start.column).take_while(|c| *c == ' ' || *c == '\t').collect();
        let text = format!("\n{}", cursor::normalize_indentation(&indent, self.document.indentation));
        let caret = end_of(start, &text);
        let change = self.apply(vec![Replace { start, end, text }], Selection::caret(caret), TYPED);
        self.history.last_kind = EditKind::TypingOther;
        change
    }

    pub fn delete_left(&mut self) -> Option<ContentChanged> {
        let (start, end) = cursor::delete_left_range(&self.document.lines, self.selection, self.document.indentation);
        self.delete(start, end, EditKind::DeletingLeft, "deleteLeft")
    }

    pub fn delete_right(&mut self) -> Option<ContentChanged> {
        let (start, end) = cursor::delete_right_range(&self.document.lines, self.selection);
        self.delete(start, end, EditKind::DeletingRight, "deleteRight")
    }

    /// Consecutive deletes in one direction undo together until one crosses a line break.
    fn delete(&mut self, start: Position, end: Position, kind: EditKind, id: &'static str) -> Option<ContentChanged> {
        if self.history.last_kind != kind || start.line != end.line {
            self.push_undo_stop();
        }
        let change = self.apply(vec![Replace { start, end, text: String::new() }], Selection::caret(start), command(id));
        self.history.last_kind = kind;
        change
    }

    pub fn delete_word_left(&mut self) -> Option<ContentChanged> {
        let (start, end) = cursor::delete_word_left_range(&self.document.lines, self.selection);
        self.apply_between_stops(vec![Replace { start, end, text: String::new() }], Selection::caret(start), command("deleteWordLeft"))
    }

    pub fn delete_word_right(&mut self) -> Option<ContentChanged> {
        let (start, end) = cursor::delete_word_right_range(&self.document.lines, self.selection);
        self.apply_between_stops(vec![Replace { start, end, text: String::new() }], Selection::caret(start), command("deleteWordRight"))
    }

    pub fn delete_all_left(&mut self) -> Option<ContentChanged> {
        let (start, end) = cursor::delete_all_left_range(&self.document.lines, self.selection);
        self.apply_between_stops(vec![Replace { start, end, text: String::new() }], Selection::caret(start), EditReason::Unknown("deleteAllLeft"))
    }

    pub fn delete_all_right(&mut self) -> Option<ContentChanged> {
        let (start, end) = cursor::delete_all_right_range(&self.document.lines, self.selection);
        self.apply_between_stops(vec![Replace { start, end, text: String::new() }], Selection::caret(start), EditReason::Unknown("deleteAllRight"))
    }

    /// `TabOperation.getCommands`: a blank line takes the indentation of the line above, an empty or
    /// in-line selection jumps to the next indent stop, a selection over whole lines indents them.
    /// shortcut: the indent of a blank line comes from the line above only, not from the language's indentation rules.
    pub fn tab(&mut self) -> Option<ContentChanged> {
        let lines = &self.document.lines;
        let indentation = self.document.indentation;
        let (start, end) = (self.selection.start(), self.selection.end());
        let whole_lines = start.line != end.line || (start.column == 0 && end.column == cursor::line_len(lines, end.line));
        if !self.selection.is_empty() && whole_lines {
            return self.shift(false, "tab");
        }
        let line = &lines[start.line];
        if self.selection.is_empty() && line.chars().all(|c| c == ' ' || c == '\t') {
            let above = lines[..start.line].iter().rev().find(|line| !line.trim_matches([' ', '\t']).is_empty());
            let indent: String = above.map(|line| line.chars().take_while(|c| *c == ' ' || *c == '\t').collect()).unwrap_or_default();
            let indent = cursor::normalize_indentation(if indent.is_empty() { "\t" } else { &indent }, indentation);
            if !line.starts_with(&indent) {
                let end = Position::new(start.line, cursor::line_len(lines, start.line));
                let caret = end_of(Position::new(start.line, 0), &indent);
                return self.apply_between_stops(vec![Replace { start: Position::new(start.line, 0), end, text: indent }], Selection::caret(caret), command("tab"));
            }
        }
        let text = cursor::tab_text(line, start.column, indentation);
        let caret = end_of(start, &text);
        self.apply_between_stops(vec![Replace { start, end, text }], Selection::caret(caret), command("tab"))
    }

    /// Shift+Tab: `ShiftCommand` unshift over the selected lines.
    pub fn outdent(&mut self) -> Option<ContentChanged> {
        self.shift(true, "outdent")
    }

    /// `ShiftCommand`: every selected line moves one indent stop; a multi-line selection skips empty lines.
    fn shift(&mut self, unshift: bool, id: &'static str) -> Option<ContentChanged> {
        let lines = &self.document.lines;
        let selection = self.selection;
        let indent_empty = selection.start().line == selection.end().line;
        let mut replaces = Vec::new();
        for line in cursor::shift_lines(selection) {
            let Some((indent_end, indent)) = cursor::shift_line_edit(&lines[line], self.document.indentation, unshift, indent_empty) else { continue };
            replaces.push(Replace { start: Position::new(line, 0), end: Position::new(line, indent_end), text: indent });
        }
        let moved = |position: Position| match replaces.iter().find(|replace| replace.start.line == position.line) {
            // A selection over whole lines keeps starting at column 0.
            Some(_) if position.column == 0 && !selection.is_empty() => position,
            Some(replace) if position.column >= replace.end.column => Position::new(position.line, position.column - replace.end.column + replace.text.chars().count()),
            Some(replace) => Position::new(position.line, position.column.min(replace.text.chars().count())),
            None => position,
        };
        let after = Selection { anchor: moved(selection.anchor), head: moved(selection.head) };
        self.apply_between_stops(replaces, after, command(id))
    }

    /// `paste`: text replaces the selection; a whole line copied from an empty selection goes in above
    /// the cursor's line when nothing is selected (`pasteOnNewLine`).
    pub fn paste(&mut self, text: &str, from_empty_selection: bool) -> Option<ContentChanged> {
        let text = normalize_line_breaks(text);
        let head = self.selection.head;
        let reason = EditReason::Cursor { kind: "paste", detailed_source: "keyboard" };
        if from_empty_selection && self.selection.is_empty() && text.find('\n') == Some(text.len() - 1) {
            let start = Position::new(head.line, 0);
            return self.apply_between_stops(vec![Replace { start, end: start, text }], Selection::caret(Position::new(head.line + 1, head.column)), reason);
        }
        let (start, end) = (self.selection.start(), self.selection.end());
        let caret = end_of(start, &text);
        self.apply_between_stops(vec![Replace { start, end, text }], Selection::caret(caret), reason)
    }

    /// `cut`: the selection, or the cursor's whole line with a line break next to it.
    pub fn cut(&mut self) -> Option<ContentChanged> {
        let lines = &self.document.lines;
        let (start, end) = match self.selection.is_empty() {
            false => (self.selection.start(), self.selection.end()),
            true => {
                let line = self.selection.head.line;
                if line + 1 < lines.len() {
                    (Position::new(line, 0), Position::new(line + 1, 0))
                } else if line > 0 {
                    (cursor::line_end(lines, Position::new(line - 1, 0)), cursor::line_end(lines, Position::new(line, 0)))
                } else {
                    (Position::new(line, 0), cursor::line_end(lines, Position::new(line, 0)))
                }
            }
        };
        self.apply_between_stops(vec![Replace { start, end, text: String::new() }], Selection::caret(start), EditReason::Cursor { kind: "cut", detailed_source: "keyboard" })
    }

    pub fn undo(&mut self) -> Option<ContentChanged> {
        let group = self.history.undo.pop()?;
        let was_dirty = self.is_dirty();
        let mut changes = Vec::new();
        for edit in group.edits.iter().rev() {
            let end = end_of(edit.start, &edit.inserted);
            changes.push(self.replace(edit.start, end, &edit.removed));
        }
        self.history.alternative_version_id = group.alternative_before;
        let selection = group.selection_before;
        self.history.redo.push(group);
        Some(self.finish_undo_redo(changes, selection, true, was_dirty))
    }

    pub fn redo(&mut self) -> Option<ContentChanged> {
        let group = self.history.redo.pop()?;
        let was_dirty = self.is_dirty();
        let mut changes = Vec::new();
        for edit in &group.edits {
            let end = end_of(edit.start, &edit.removed);
            changes.push(self.replace(edit.start, end, &edit.inserted));
        }
        self.history.alternative_version_id = group.alternative_after;
        let selection = group.selection_after;
        self.history.undo.push(group);
        Some(self.finish_undo_redo(changes, selection, false, was_dirty))
    }

    fn finish_undo_redo(&mut self, changes: Vec<ContentChange>, selection: Selection, is_undoing: bool, was_dirty: bool) -> ContentChanged {
        self.version_id += 1;
        self.history.open = false;
        // cursor.ts: a model change the cursor did not make resets the previous edit type.
        self.history.last_kind = EditKind::Other;
        self.after_text_change(selection);
        ContentChanged { changes, version_id: self.version_id, is_undoing, is_redoing: !is_undoing, reason: EditReason::ApplyEdits, was_dirty }
    }

    /// The text as it was written; the saved version is the clean one from now on.
    pub fn save(&mut self) -> std::io::Result<()> {
        let document = &self.document;
        let bom = if document.encoding == "utf8bom" { "\u{FEFF}" } else { "" };
        std::fs::write(&document.path, format!("{bom}{}", document.lines.join(document.eol)))?;
        self.history.saved_alternative_version_id = self.history.alternative_version_id;
        Ok(())
    }

    fn apply_between_stops(&mut self, replaces: Vec<Replace>, selection: Selection, reason: EditReason) -> Option<ContentChanged> {
        self.push_undo_stop();
        let change = self.apply(replaces, selection, reason);
        self.push_undo_stop();
        change
    }

    /// Applies the replaces end to beginning, so each range still holds, as one model change event
    /// added to the open undo group. Nothing happens when every replace is empty.
    fn apply(&mut self, mut replaces: Vec<Replace>, selection: Selection, reason: EditReason) -> Option<ContentChanged> {
        replaces.retain(|replace| replace.start != replace.end || !replace.text.is_empty());
        self.history.last_kind = EditKind::Other;
        if replaces.is_empty() {
            return None;
        }
        replaces.sort_by(|a, b| b.start.cmp(&a.start));
        let was_dirty = self.is_dirty();
        let selection_before = self.selection;
        let mut edits = Vec::new();
        let mut changes = Vec::new();
        for replace in replaces {
            let removed = text_between(&self.document.lines, replace.start, replace.end);
            changes.push(self.replace(replace.start, replace.end, &replace.text));
            edits.push(AppliedEdit { start: replace.start, removed, inserted: replace.text });
        }
        self.version_id += 1;
        let version_id = self.version_id;
        let history = &mut self.history;
        history.redo.clear();
        let alternative_before = history.alternative_version_id;
        history.alternative_version_id = version_id;
        match history.undo.last_mut().filter(|_| history.open) {
            Some(group) => {
                group.edits.extend(edits);
                group.selection_after = selection;
                group.alternative_after = version_id;
            }
            None => history.undo.push(UndoGroup { edits, selection_before, selection_after: selection, alternative_before, alternative_after: version_id }),
        }
        history.open = true;
        self.after_text_change(selection);
        // Editing a preview editor pins it (editorGroupView.ts pins a preview editor once it is dirty).
        self.preview = false;
        Some(ContentChanged { changes, version_id, is_undoing: false, is_redoing: false, reason, was_dirty })
    }

    /// Word highlights stop while the text changes: wordHighlighter.ts ignores non-explicit cursor changes.
    fn after_text_change(&mut self, selection: Selection) {
        self.selection = selection;
        self.leftover = 0;
        self.highlights.clear();
        let indentation = self.document.indentation;
        self.guides = indent_guides(&self.document.lines, indentation.tab_size, indentation.tab_size, self.document.off_side);
    }

    /// Replaces `start..end` with `text` in the lines and describes it in the text before it.
    fn replace(&mut self, start: Position, end: Position, text: &str) -> ContentChange {
        let eol = self.document.eol;
        let lines = &mut self.document.lines;
        let removed = text_between(lines, start, end);
        let change = ContentChange {
            start_line: start.line + 1,
            start_column: utf16_column(&lines[start.line], start.column) + 1,
            end_line: end.line + 1,
            end_column: utf16_column(&lines[end.line], end.column) + 1,
            range_offset: lines[..start.line].iter().map(|line| line.encode_utf16().count() + eol.len()).sum::<usize>() + utf16_column(&lines[start.line], start.column),
            range_length: removed.replace('\n', eol).encode_utf16().count(),
            text: text.replace('\n', eol),
        };
        let prefix: String = lines[start.line].chars().take(start.column).collect();
        let suffix: String = lines[end.line].chars().skip(end.column).collect();
        let joined = prefix + text + &suffix;
        lines.splice(start.line..=end.line, joined.split('\n').map(String::from));
        self.document.tokens.invalidate_from(start.line);
        change
    }
}

pub fn utf16_column(line: &str, column: usize) -> usize {
    line.chars().take(column).map(char::len_utf16).sum()
}

/// The text from `start` to `end`, lines joined by "\n".
pub fn text_between(lines: &[String], start: Position, end: Position) -> String {
    if start.line == end.line {
        return lines[start.line].chars().skip(start.column).take(end.column - start.column).collect();
    }
    let mut text: String = lines[start.line].chars().skip(start.column).collect();
    for line in &lines[start.line + 1..end.line] {
        text.push('\n');
        text.push_str(line);
    }
    text.push('\n');
    text.extend(lines[end.line].chars().take(end.column));
    text
}

/// Where text inserted at `start` ends.
fn end_of(start: Position, text: &str) -> Position {
    match text.rsplit_once('\n') {
        Some((before, last)) => Position::new(start.line + before.matches('\n').count() + 1, last.chars().count()),
        None => Position::new(start.line, start.column + text.chars().count()),
    }
}

/// Pasted and typed text may hold CRLF or CR; the model stores lines.
fn normalize_line_breaks(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::editor::{EditorGroup, Indentation, TextDocument};
    use std::path::PathBuf;

    pub(in crate::editor) fn editor(text: &str, eol: &'static str) -> OpenEditor {
        let document = TextDocument {
            scheme: "file".into(),
            path: PathBuf::from("/w/a.ts"),
            lines: text.split('\n').map(String::from).collect(),
            eol,
            encoding: "utf8",
            language_id: "typescript".into(),
            indentation: Indentation { tab_size: 4, insert_spaces: true },
            off_side: false,
            tokens: Default::default(),
        };
        let mut group = EditorGroup::default();
        group.open(document, true);
        group.editors.remove(0)
    }

    fn at(line: usize, column: usize) -> Position {
        Position::new(line, column)
    }

    fn text(editor: &OpenEditor) -> String {
        editor.document.lines.join("\n")
    }

    #[test]
    fn typing_reports_utf16_changes_in_the_document_eol() {
        let mut editor = editor("hé😀\nx", "\r\n");
        editor.move_to(at(1, 0), false);
        let change = editor.type_text("a\nb").unwrap();
        assert_eq!(text(&editor), "hé😀\na\nbx");
        assert_eq!(change.changes, [ContentChange { start_line: 2, start_column: 1, end_line: 2, end_column: 1, range_offset: 6, range_length: 0, text: "a\r\nb".into() }]);
        assert_eq!((change.version_id, change.reason), (2, TYPED));
        assert_eq!(editor.selection, Selection::caret(at(2, 1)));

        editor.selection = Selection { anchor: at(0, 3), head: at(0, 1) };
        let change = editor.delete_left().unwrap();
        assert_eq!(text(&editor), "h\na\nbx");
        assert_eq!((change.changes[0].start_column, change.changes[0].end_column, change.changes[0].range_length), (2, 5, 3));
    }

    #[test]
    fn words_and_spaces_undo_in_upstream_groups() {
        let mut editor = editor("", "\n");
        for c in "ab  cd".chars() {
            editor.type_text(&c.to_string());
        }
        // The run of spaces is its own step between the words; a char right after one space would join it.
        editor.undo();
        assert_eq!(text(&editor), "ab  ");
        editor.undo();
        assert_eq!(text(&editor), "ab");
        editor.undo();
        assert_eq!(text(&editor), "");
        assert!(!editor.is_dirty());
        let change = editor.redo().unwrap();
        assert!(change.is_redoing && editor.is_dirty());
        assert_eq!((text(&editor), editor.selection.head), ("ab".into(), at(0, 2)));
    }

    #[test]
    fn deletes_group_until_they_cross_a_line_or_change_direction() {
        let mut editor = editor("ab\ncd", "\n");
        editor.move_to(at(1, 2), false);
        editor.delete_left();
        editor.delete_left();
        editor.delete_left();
        assert_eq!(text(&editor), "ab");
        editor.undo();
        assert_eq!(text(&editor), "ab\n");
        editor.undo();
        assert_eq!(text(&editor), "ab\ncd");
        // A cursor move pushes an undo stop between typing runs.
        editor.type_text("x");
        editor.move_to(at(0, 0), false);
        editor.type_text("y");
        editor.undo();
        assert_eq!(text(&editor), "ab\ncdx");
    }

    #[test]
    fn saving_makes_the_current_version_clean() {
        let dir = std::env::temp_dir().join(format!("code-gpui-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut editor = editor("a", "\r\n");
        editor.document.path = dir.join("a.txt");
        editor.document.encoding = "utf8bom";
        editor.move_to(at(0, 1), false);
        editor.enter();
        editor.save().unwrap();
        assert_eq!(std::fs::read(&editor.document.path).unwrap(), b"\xEF\xBB\xBFa\r\n");
        assert!(!editor.is_dirty());
        editor.undo();
        assert!(editor.is_dirty());
        editor.redo();
        assert!(!editor.is_dirty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn enter_keeps_the_indentation_and_tab_indents_blank_lines_like_the_line_above() {
        let mut editor = editor("    let x", "\n");
        editor.move_to(at(0, 9), false);
        editor.enter();
        assert_eq!(editor.document.lines, ["    let x", "    "]);
        editor.type_text("y");
        editor.move_to(at(1, 0), false);
        editor.delete_all_right();
        assert_eq!(editor.document.lines[1], "");
        let change = editor.tab().unwrap();
        assert_eq!((editor.document.lines[1].as_str(), editor.selection.head), ("    ", at(1, 4)));
        assert_eq!(change.reason, command("tab"));
        editor.tab();
        assert_eq!(editor.document.lines[1], "        ");
    }

    #[test]
    fn tab_over_lines_shifts_them_and_keeps_the_selection_on_them() {
        let mut editor = editor("a\n  b\n\nc", "\n");
        editor.selection = Selection { anchor: at(0, 0), head: at(3, 1) };
        editor.tab();
        assert_eq!(editor.document.lines, ["    a", "    b", "", "    c"]);
        assert_eq!(editor.selection, Selection { anchor: at(0, 0), head: at(3, 5) });
        editor.outdent();
        assert_eq!(editor.document.lines, ["a", "b", "", "c"]);
        editor.undo();
        editor.undo();
        assert_eq!(text(&editor), "a\n  b\n\nc");
    }

    #[test]
    fn cut_and_paste_a_whole_line_from_an_empty_selection() {
        let mut editor = editor("one\ntwo\nthree", "\n");
        editor.move_to(at(1, 1), false);
        let (copied, whole_line) = editor.copy_text();
        assert_eq!((copied.as_str(), whole_line), ("two\n", true));
        editor.cut();
        assert_eq!((text(&editor), editor.selection.head), ("one\nthree".into(), at(1, 0)));
        editor.move_to(at(1, 2), false);
        editor.paste(&copied, whole_line);
        assert_eq!((text(&editor), editor.selection.head), ("one\ntwo\nthree".into(), at(2, 2)));
        editor.move_to(at(2, 5), false);
        editor.cut();
        assert_eq!(text(&editor), "one\ntwo");
    }

    #[test]
    fn moves_collapse_selections_and_keep_the_column_through_short_lines() {
        let mut editor = editor("abcd\nx\nabcd", "\n");
        editor.selection = Selection { anchor: at(0, 1), head: at(0, 3) };
        editor.move_horizontally(false, false);
        assert_eq!(editor.selection, Selection::caret(at(0, 1)));
        editor.move_to(at(0, 4), false);
        editor.move_vertically(true, false);
        editor.move_vertically(true, true);
        assert_eq!(editor.selection, Selection { anchor: at(1, 1), head: at(2, 4) });
        editor.select_all();
        assert_eq!(editor.selection.head, at(2, 4));
    }
}
