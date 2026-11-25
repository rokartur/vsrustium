//! Editing an open editor's text at every cursor: the cursor commands of coreCommands.ts and
//! cursorTypeOperations.ts, the undo stack of editStack.ts, and the content change events
//! mainThreadDocuments.ts forwards to the extension host. Line breaks inside edit text are "\n";
//! the host hears them in the document's EOL.

use std::cmp::Reverse;

use super::auto_indent::{AutoIndent, Model};
use super::cursor::{self, Position, Selection};
use super::{OpenEditor, anchor_select, folding, indent_guides};

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
    /// `EditSources.eolChange` of `setEOL`: the line ending changed, the text did not.
    EolChange,
}

const TYPED: EditReason = EditReason::Cursor { kind: "type", detailed_source: "keyboard" };

pub(super) fn command(id: &'static str) -> EditReason {
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
    /// The version id the last undo left: a model at it changed last by an undo (`isUndoing`).
    undone_version_id: u32,
}

impl Default for History {
    fn default() -> Self {
        History { undo: Vec::new(), redo: Vec::new(), open: false, last_kind: EditKind::Other, alternative_version_id: 1, saved_alternative_version_id: 1, undone_version_id: 0 }
    }
}

#[derive(Debug)]
struct UndoGroup {
    edits: Vec<AppliedEdit>,
    selections_before: Vec<Selection>,
    selections_after: Vec<Selection>,
    alternative_before: u32,
    alternative_after: u32,
}

#[derive(Debug)]
struct AppliedEdit {
    start: Position,
    removed: String,
    inserted: String,
}

#[derive(Clone, Debug)]
pub struct Replace {
    pub start: Position,
    pub end: Position,
    pub text: String,
}

impl Replace {
    pub fn new(start: Position, end: Position, text: impl Into<String>) -> Self {
        Replace { start, end, text: text.into() }
    }

    fn is_noop(&self) -> bool {
        self.start == self.end && self.text.is_empty()
    }
}

/// What one cursor's command edits and where that cursor ends up, given in the text after its own
/// edits (`ICommand.getEditOperations` and `computeCursorState`); edits of cursors before it move it.
#[derive(Clone, Debug)]
pub struct CursorEdit {
    pub replaces: Vec<Replace>,
    pub after: Selection,
}

/// An `ICommand` for `execute_commands`: its edits in the order it adds them, the selection it tracks
/// (`trackSelection`), and its `computeCursorState` from that selection after the edits and the ranges its
/// edits' texts take then (the inverse edit operations, in its order).
pub struct Command {
    pub edits: Vec<Replace>,
    pub tracked: Option<Selection>,
    pub cursor_state: Box<dyn FnOnce(Option<Selection>, &[(Position, Position)]) -> Selection>,
}

impl CursorEdit {
    pub fn new(replaces: Vec<Replace>, after: Selection) -> Self {
        CursorEdit { replaces, after }
    }

    fn replace(start: Position, end: Position, text: String, caret: Position) -> Self {
        CursorEdit { replaces: vec![Replace { start, end, text }], after: Selection::caret(caret) }
    }
}

impl OpenEditor {
    pub fn is_dirty(&self) -> bool {
        // untitledTextEditorModel.ts: dirty with an associated file path or any text.
        if self.document.scheme == "untitled" {
            return self.associated_file || self.document.lines != [""];
        }
        self.history.alternative_version_id != self.history.saved_alternative_version_id
    }

    /// The primary cursor's selection (`getSelection`).
    pub fn selection(&self) -> Selection {
        self.selections[0]
    }

    /// `pushStackElement`: the next edit starts a new undo group.
    pub fn push_undo_stop(&mut self) {
        self.history.open = false;
    }

    /// `setSelections`: the first one is the primary cursor; overlapping ones merge. The vertical leftovers are dropped.
    pub fn set_selections(&mut self, selections: Vec<Selection>) {
        self.set_cursor_states(selections, Vec::new());
    }

    /// `setCursorStates`: an undo stop, the cursors with their (anchor, head) vertical leftovers, then
    /// `normalize` and the word highlights at the primary head. A cursor beyond the old count is the
    /// last added one (`_setSecondaryStates`).
    pub(super) fn set_cursor_states(&mut self, selections: Vec<Selection>, mut leftovers: Vec<(isize, isize)>) {
        self.push_undo_stop();
        let previous = self.selections.clone();
        self.last_added = match selections.len() > self.selections.len() {
            true => selections.len() - 1,
            false => self.last_added.min(selections.len() - 1),
        };
        self.selections = selections;
        for index in cursor::normalize(&mut self.selections, &mut self.last_added) {
            if index < leftovers.len() {
                leftovers.remove(index);
            }
        }
        self.leftovers = leftovers;
        if self.selections != previous {
            self.record_cursor_state(previous);
        }
        self.unfold_at_hidden_cursors();
        self.refresh_highlights();
    }

    /// `getLastAddedCursorIndex`.
    pub(super) fn last_added(&self) -> usize {
        self.last_added
    }

    /// `moveTo` from a click or the API: one cursor, the secondary ones go.
    pub fn move_to(&mut self, head: Position, select: bool) {
        let selection = self.selection().moved(head, select);
        self.set_selections(vec![selection]);
    }

    /// Moves every cursor's head with `step`, which also gets and returns the head's vertical leftover;
    /// a selecting move keeps the anchor's (`SingleCursorState.move`).
    fn move_cursors(&mut self, select: bool, step: impl Fn(&[String], Selection, isize) -> (Position, isize)) {
        let mut leftovers = Vec::new();
        let mut selections = Vec::new();
        for (index, &selection) in self.selections.iter().enumerate() {
            let (anchor_leftover, head_leftover) = self.leftover(index);
            let (head, leftover) = step(&self.document.lines, selection, head_leftover);
            let head = folding::shown_position(&self.document.lines, self.folding.hidden(), head, head > selection.head);
            selections.push(selection.moved(head, select));
            leftovers.push((if select { anchor_leftover } else { leftover }, leftover));
        }
        self.set_cursor_states(selections, leftovers);
    }

    /// The (anchor, head) vertical leftovers of cursor `index`.
    pub(super) fn leftover(&self, index: usize) -> (isize, isize) {
        self.leftovers.get(index).copied().unwrap_or_default()
    }

    /// `cursorLeft`/`cursorRight`: a selection collapses to its start or end before the cursor moves.
    pub fn move_horizontally(&mut self, right: bool, select: bool) {
        self.move_cursors(select, |lines, selection, _| {
            let head = match (select || selection.is_empty(), right) {
                (true, false) => cursor::left(lines, selection.head),
                (true, true) => cursor::right(lines, selection.head),
                (false, false) => selection.start(),
                (false, true) => selection.end(),
            };
            (head, 0)
        });
    }

    /// `cursorUp`/`cursorDown`: from a selection's start or end, keeping the visible column through short lines.
    pub fn move_vertically(&mut self, down: bool, select: bool) {
        let tab_size = self.document.indentation.tab_size;
        let hidden = self.folding.hidden().to_vec();
        self.move_cursors(select, |lines, selection, leftover| {
            let from = match (select || selection.is_empty(), down) {
                (true, _) => selection.head,
                (false, false) => selection.start(),
                (false, true) => selection.end(),
            };
            cursor::vertical(lines, &hidden, from, leftover, down, tab_size)
        });
    }

    /// `cursorWordStartLeft`/`cursorWordEndRight` from the head.
    pub fn move_word(&mut self, right: bool, select: bool) {
        self.move_cursors(select, |lines, selection, _| (if right { cursor::word_right(lines, selection.head) } else { cursor::word_left(lines, selection.head) }, 0));
    }

    /// `cursorWordPartLeft`/`cursorWordPartRight` from the head (wordPartOperations.ts).
    pub fn move_word_part(&mut self, right: bool, select: bool) {
        self.move_cursors(select, |lines, selection, _| (if right { cursor::word_part_right(lines, selection.head, super::WORD_SEPARATORS) } else { cursor::word_part_left(lines, selection.head, super::WORD_SEPARATORS) }, 0));
    }

    /// `cursorHome`/`cursorEnd`; Home toggles between the first non-blank character and column 0.
    pub fn move_to_line_edge(&mut self, end: bool, select: bool) {
        self.move_cursors(select, |lines, selection, _| (if end { cursor::line_end(lines, selection.head) } else { cursor::line_start(lines, selection.head) }, 0));
    }

    /// `cursorTop`/`cursorBottom`: the primary cursor only.
    pub fn move_to_document_edge(&mut self, end: bool, select: bool) {
        let head = if end { self.last_shown_position() } else { Position::default() };
        self.move_to(head, select);
    }

    pub fn select_all(&mut self) {
        self.set_selections(vec![Selection { anchor: Position::default(), head: cursor::last_position(&self.document.lines) }]);
    }

    /// `CursorMoveCommands.expandLineSelection`: each selection from its start line's start to the start of the line
    /// after its end, or to the end of the last line.
    pub fn expand_line_selection(&mut self) {
        let lines = &self.document.lines;
        let selections = self
            .selections
            .iter()
            .map(|selection| {
                let end = selection.end().line;
                let head = if end == lines.len() - 1 { cursor::last_position(lines) } else { Position::new(end + 1, 0) };
                Selection { anchor: Position::new(selection.start().line, 0), head }
            })
            .collect();
        self.push_undo_stop();
        self.set_selections(selections);
    }

    /// The word highlights of the primary selection (wordHighlighter.ts `_run`): none for a multi-line selection
    /// or one that reaches past the word at its start.
    pub fn refresh_highlights(&mut self) {
        let selection = self.selection();
        let (start, end) = (selection.start(), selection.end());
        let text = &self.document.lines[start.line];
        let word = super::word_at(text, start.column).filter(|word| start.line == end.line && word.start <= start.column && end.column <= word.end);
        self.highlights = match word {
            Some(range) => super::word_occurrences(&self.document.lines, &text.chars().skip(range.start).take(range.len()).collect::<String>()),
            None => Vec::new(),
        };
    }

    /// `getPlainTextToCopy` with `emptySelectionClipboard`: each selection in document order, an empty
    /// one as its whole line with the line break, once per line. Several pieces also travel apart as
    /// `multicursorText`; `isFromEmptySelection` holds for one empty cursor only.
    pub fn copy_text(&self) -> Copied {
        let lines = &self.document.lines;
        let eol = self.document.eol;
        let mut selections = self.selections.clone();
        selections.sort_by_key(|selection| (selection.start(), selection.end()));
        let any_empty = selections.iter().any(Selection::is_empty);
        let mut pieces = Vec::new();
        let mut previous_line = None;
        for selection in selections {
            let line = selection.start().line;
            if !selection.is_empty() {
                pieces.push(text_between(lines, selection.start(), selection.end()).replace('\n', eol));
            } else if any_empty && previous_line != Some(line) {
                pieces.push(format!("{}{eol}", lines[line]));
            }
            previous_line = Some(line);
        }
        let from_empty_selection = self.selections.len() == 1 && self.selection().is_empty();
        let multicursor_text = (pieces.len() > 1).then(|| pieces.clone());
        Copied { text: pieces.join(eol), from_empty_selection, multicursor_text }
    }

    /// `type`: text replaces every selection. Spaces and other text group into undo steps as upstream does.
    pub fn type_text(&mut self, text: &str) -> Option<ContentChanged> {
        self.type_replacing(text, 0, 0, "type")
    }

    /// `compositionType`: at every empty cursor the text replaces `previous` chars before it and `next`
    /// after it, as IME composition does with its marked text; a selection is replaced whole.
    pub fn type_replacing(&mut self, text: &str, previous: usize, next: usize, kind: &'static str) -> Option<ContentChanged> {
        let line_len = |line| cursor::line_len(&self.document.lines, line);
        let ranges: Vec<_> = self
            .selections
            .iter()
            .map(|selection| {
                let (mut start, mut end) = (selection.start(), selection.end());
                if selection.is_empty() {
                    start.column = start.column.saturating_sub(previous);
                    end.column = (end.column + next).min(line_len(end.line));
                }
                (start, end)
            })
            .collect();
        self.type_into(text, ranges, kind)
    }

    /// `SimpleCharacterTypeOperation` in overtype input mode (`ReplaceOvertypeCommand`): the text also replaces as many
    /// chars after each selection, up to its line's end.
    pub fn type_overtyping(&mut self, text: &str) -> Option<ContentChanged> {
        let text = normalize_line_breaks(text);
        let ranges = self.selections.iter().map(|selection| (selection.start(), overtype_end(&self.document.lines, *selection, &text))).collect();
        self.type_into(&text, ranges, "type")
    }

    /// `CompositionEndOvertypeOperation`: in overtype input mode the text an IME composed then removes as many chars
    /// after each cursor, up to its line's end.
    pub fn overtype_composed(&mut self, composed: &str) -> Option<ContentChanged> {
        let count = composed.chars().count();
        let lines = &self.document.lines;
        let edits = self.each_cursor(|selection| {
            let head = selection.head;
            let end = Position::new(head.line, (head.column + count).min(cursor::line_len(lines, head.line)));
            CursorEdit::replace(head, end, String::new(), head)
        });
        self.apply_cursor_edits(edits, TYPED)
    }

    /// The typed text replaces `ranges`, one per selection; spaces and other text group into undo steps as upstream does.
    fn type_into(&mut self, text: &str, ranges: Vec<(Position, Position)>, kind: &'static str) -> Option<ContentChanged> {
        let text = normalize_line_breaks(text);
        let last_kind = self.history.last_kind;
        let typing = match text.as_str() {
            " " if last_kind.is_space() => EditKind::TypingConsecutiveSpace,
            " " => EditKind::TypingFirstSpace,
            _ => EditKind::TypingOther,
        };
        if stop_between(last_kind, typing) {
            self.push_undo_stop();
        }
        let edits = ranges.into_iter().map(|(start, end)| CursorEdit::replace(start, end, text.clone(), end_of(start, &text))).collect();
        let change = self.apply_cursor_edits(edits, EditReason::Cursor { kind, detailed_source: "keyboard" });
        self.history.last_kind = typing;
        change
    }

    /// `type` from the keyboard: `typeWithInterceptors` per char, so a line break goes through Enter, a char
    /// the indentation rules outdent reindents its line, and a closing bracket takes its opening line's indent.
    /// Each char is its own model change, as upstream.
    pub fn type_keyboard(&mut self, text: &str, auto: AutoIndent, overtype: bool) -> Vec<ContentChanged> {
        let mut changes = Vec::new();
        for ch in normalize_line_breaks(text).chars() {
            let ch = ch.to_string();
            let change = match ch.as_str() {
                "\n" => self.enter(auto),
                _ => match self.auto_indent_type(&ch, auto).or_else(|| self.electric_type(&ch, auto)) {
                    Some(change) => change,
                    None if overtype => self.type_overtyping(&ch),
                    None => self.type_text(&ch),
                },
            };
            changes.extend(change);
        }
        changes
    }

    /// `AutoIndentOperation`: typing `ch` reindents every cursor's line, or `None` when one keeps its indentation.
    fn auto_indent_type(&mut self, ch: &str, auto: AutoIndent) -> Option<Option<ContentChanged>> {
        let model = auto.model(&self.document);
        let mut edits = Vec::new();
        for selection in &self.selections {
            let (start, end, text, caret) = model.type_edit(selection.start(), selection.end(), ch)?;
            edits.push(CursorEdit::replace(start, end, text, caret));
        }
        self.push_undo_stop();
        let change = self.apply_cursor_edits(edits, TYPED);
        self.history.last_kind = EditKind::TypingOther;
        Some(change)
    }

    /// `InterceptorElectricCharOperation`, for one cursor only: an undo stop after the reindented line.
    fn electric_type(&mut self, ch: &str, auto: AutoIndent) -> Option<Option<ContentChanged>> {
        let [selection] = self.selections[..] else { return None };
        if !selection.is_empty() {
            return None;
        }
        let (start, end, text, caret) = auto.model(&self.document).electric_edit(selection.head, ch)?;
        let change = self.apply_cursor_edits(vec![CursorEdit::replace(start, end, text, caret)], TYPED);
        self.push_undo_stop();
        self.history.last_kind = EditKind::TypingOther;
        Some(change)
    }

    /// `EnterOperation`: a line break at each cursor, indented by the language's onEnter and indentation rules.
    pub fn enter(&mut self, auto: AutoIndent) -> Option<ContentChanged> {
        self.push_undo_stop();
        let model = auto.model(&self.document);
        let edits = self.each_cursor(|selection| {
            let (start, end, text, caret) = model.enter_edit(selection.start(), selection.end());
            CursorEdit::replace(start, end, text, caret)
        });
        let change = self.apply_cursor_edits(edits, TYPED);
        self.history.last_kind = EditKind::TypingOther;
        change
    }

    pub fn delete_left(&mut self) -> Option<ContentChanged> {
        let indentation = self.document.indentation;
        self.delete(|lines, selection| cursor::delete_left_range(lines, selection, indentation), Some(EditKind::DeletingLeft), command("deleteLeft"))
    }

    pub fn delete_right(&mut self) -> Option<ContentChanged> {
        self.delete(cursor::delete_right_range, Some(EditKind::DeletingRight), command("deleteRight"))
    }

    pub fn delete_word_left(&mut self) -> Option<ContentChanged> {
        self.delete(cursor::delete_word_left_range, None, command("deleteWordLeft"))
    }

    pub fn delete_word_right(&mut self) -> Option<ContentChanged> {
        self.delete(cursor::delete_word_right_range, None, command("deleteWordRight"))
    }

    pub fn delete_word_part_left(&mut self) -> Option<ContentChanged> {
        self.delete(|lines, selection| cursor::delete_word_part_left_range(lines, selection, super::WORD_SEPARATORS), None, command("deleteWordPartLeft"))
    }

    pub fn delete_word_part_right(&mut self) -> Option<ContentChanged> {
        self.delete(|lines, selection| cursor::delete_word_part_right_range(lines, selection, super::WORD_SEPARATORS), None, command("deleteWordPartRight"))
    }

    /// Deletes each cursor's range. Consecutive char deletes (`kind`) in one direction undo together
    /// until one crosses a line break; the other deletes are undo steps of their own.
    fn delete(&mut self, range: impl Fn(&[String], Selection) -> (Position, Position), kind: Option<EditKind>, reason: EditReason) -> Option<ContentChanged> {
        let lines = &self.document.lines;
        let edits = self.each_cursor(|selection| {
            let (start, end) = range(lines, selection);
            CursorEdit::replace(start, end, String::new(), start)
        });
        let Some(kind) = kind else { return self.apply_between_stops(edits, reason) };
        let crosses_line = edits.iter().any(|edit| edit.replaces[0].start.line != edit.replaces[0].end.line);
        if self.history.last_kind != kind || crosses_line {
            self.push_undo_stop();
        }
        let change = self.apply_cursor_edits(edits, reason);
        self.history.last_kind = kind;
        change
    }

    /// `TabOperation.getCommands` per cursor: a blank line takes the indentation the language's rules give it,
    /// an empty or in-line selection jumps to the next indent stop, a selection over whole lines indents them.
    pub fn tab(&mut self, auto: AutoIndent) -> Option<ContentChanged> {
        let lines = &self.document.lines;
        let indentation = self.document.indentation;
        let model = auto.model(&self.document);
        let edits = self.each_cursor(|selection| {
            let (start, end) = (selection.start(), selection.end());
            let whole_lines = start.line != end.line || (start.column == 0 && end.column == cursor::line_len(lines, end.line));
            if !selection.is_empty() && whole_lines {
                return shift_edit(&model, selection, false);
            }
            let line = &lines[start.line];
            if selection.is_empty() && line.chars().all(|c| c == ' ' || c == '\t') {
                let indent = cursor::normalize_indentation(&model.tab_indent(start.line).unwrap_or_else(|| "\t".into()), indentation);
                if !line.starts_with(&indent) {
                    let line_start = Position::new(start.line, 0);
                    let caret = end_of(line_start, &indent);
                    return CursorEdit::replace(line_start, Position::new(start.line, cursor::line_len(lines, start.line)), indent, caret);
                }
            }
            let text = cursor::tab_text(line, start.column, indentation);
            let caret = end_of(start, &text);
            CursorEdit::replace(start, end, text, caret)
        });
        self.apply_between_stops(edits, command("tab"))
    }

    /// Shift+Tab: `ShiftCommand` unshift over each cursor's lines.
    pub fn outdent(&mut self, auto: AutoIndent) -> Option<ContentChanged> {
        self.shift(true, auto, command("outdent"))
    }

    /// `editor.action.indentLines` / `outdentLines`: `ShiftCommand` over each cursor's lines.
    pub fn shift(&mut self, unshift: bool, auto: AutoIndent, reason: EditReason) -> Option<ContentChanged> {
        let model = auto.model(&self.document);
        let edits = self.each_cursor(|selection| shift_edit(&model, selection, unshift));
        self.apply_between_stops(edits, reason)
    }

    /// `paste`: text replaces each selection. A whole line copied from an empty selection goes in above
    /// each empty cursor (`pasteOnNewLine`); with as many pieces or lines as cursors, each cursor gets one
    /// (`multiCursorPaste` "spread"). With `overtype` (overtype input mode and `editor.overtypeOnPaste`) the text also
    /// replaces as many chars after the selection.
    pub fn paste(&mut self, text: &str, from_empty_selection: bool, multicursor_text: Option<&[String]>, overtype: bool) -> Option<ContentChanged> {
        let text = normalize_line_breaks(text);
        let reason = EditReason::Cursor { kind: "paste", detailed_source: "keyboard" };
        if let Some(pieces) = self.distributed_paste(&text, from_empty_selection, multicursor_text) {
            let mut order: Vec<usize> = (0..self.selections.len()).collect();
            order.sort_by_key(|&index| (self.selections[index].start(), self.selections[index].end()));
            let mut edits = vec![CursorEdit::new(Vec::new(), Selection::default()); order.len()];
            for (index, piece) in order.into_iter().zip(pieces) {
                let selection = self.selections[index];
                let end = if overtype { overtype_end(&self.document.lines, selection, &piece) } else { selection.end() };
                edits[index] = CursorEdit::replace(selection.start(), end, piece.clone(), end_of(selection.start(), &piece));
            }
            return self.apply_between_stops(edits, reason);
        }
        let on_new_line = from_empty_selection && text.find('\n') == Some(text.len() - 1);
        let edits = self.each_cursor(|selection| {
            let head = selection.head;
            if on_new_line && selection.is_empty() {
                let start = Position::new(head.line, 0);
                return CursorEdit::replace(start, start, text.clone(), Position::new(head.line + 1, head.column));
            }
            let end = if overtype { overtype_end(&self.document.lines, selection, &text) } else { selection.end() };
            CursorEdit::replace(selection.start(), end, text.clone(), end_of(selection.start(), &text))
        });
        self.apply_between_stops(edits, reason)
    }

    /// `_distributePasteToCursors`.
    fn distributed_paste(&self, text: &str, from_empty_selection: bool, multicursor_text: Option<&[String]>) -> Option<Vec<String>> {
        let count = self.selections.len();
        if count == 1 {
            return None;
        }
        if let Some(pieces) = multicursor_text.filter(|pieces| pieces.len() == count) {
            return Some(pieces.iter().map(|piece| normalize_line_breaks(piece)).collect());
        }
        if from_empty_selection {
            return None;
        }
        let lines: Vec<String> = text.strip_suffix('\n').unwrap_or(text).split('\n').map(String::from).collect();
        (lines.len() == count).then_some(lines)
    }

    /// `cut`: each selection, or an empty cursor's whole line with a line break next to it.
    pub fn cut(&mut self) -> Option<ContentChanged> {
        let lines = &self.document.lines;
        let edits = self.each_cursor(|selection| {
            let (start, end) = match selection.is_empty() {
                false => (selection.start(), selection.end()),
                true => {
                    let line = selection.head.line;
                    if line + 1 < lines.len() {
                        (Position::new(line, 0), Position::new(line + 1, 0))
                    } else if line > 0 {
                        (cursor::line_end(lines, Position::new(line - 1, 0)), cursor::line_end(lines, Position::new(line, 0)))
                    } else {
                        (Position::new(line, 0), cursor::line_end(lines, Position::new(line, 0)))
                    }
                }
            };
            CursorEdit::replace(start, end, String::new(), start)
        });
        self.apply_between_stops(edits, EditReason::Cursor { kind: "cut", detailed_source: "keyboard" })
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
        let selections = group.selections_before.clone();
        self.history.redo.push(group);
        let change = self.finish_undo_redo(changes, selections, true, was_dirty);
        self.history.undone_version_id = self.version_id;
        Some(change)
    }

    pub fn last_change_was_undo(&self) -> bool {
        self.history.undone_version_id == self.version_id
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
        let selections = group.selections_after.clone();
        self.history.undo.push(group);
        Some(self.finish_undo_redo(changes, selections, false, was_dirty))
    }

    fn finish_undo_redo(&mut self, changes: Vec<ContentChange>, selections: Vec<Selection>, is_undoing: bool, was_dirty: bool) -> ContentChanged {
        self.version_id += 1;
        self.history.open = false;
        // cursor.ts: a model change the cursor did not make resets the previous edit type.
        self.history.last_kind = EditKind::Other;
        self.after_text_change(selections);
        ContentChanged { changes, version_id: self.version_id, is_undoing, is_redoing: !is_undoing, reason: EditReason::ApplyEdits, was_dirty }
    }

    /// The text as it was written; the saved version is the clean one from now on.
    pub fn save(&mut self) -> std::io::Result<()> {
        // A content provider's model has no file to write; models of extension file system providers save through the host.
        if self.document.scheme != "file" {
            return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, format!("no file system provider for {}", self.document.scheme)));
        }
        std::fs::write(&self.document.path, self.saved_bytes())?;
        self.mark_saved();
        Ok(())
    }

    /// The document as saving writes it: its EOL, in its encoding, with the BOM that encoding takes.
    pub fn saved_bytes(&self) -> Vec<u8> {
        let document = &self.document;
        files::encoding::encode(&document.lines.join(document.eol), &document.encoding, files::encoding::has_bom(&document.encoding))
    }

    /// `pushEOL`: the model takes the line ending of `EndOfLineSequence` (0 is LF) and is dirty.
    // shortcut: the change is not on the undo stack, so undo skips it; add it when undo groups take non-text elements.
    pub fn push_eol(&mut self, sequence: u64) -> Option<ContentChanged> {
        let eol = if sequence == 1 { "\r\n" } else { "\n" };
        if self.document.eol == eol {
            return None;
        }
        let was_dirty = self.is_dirty();
        self.document.eol = eol;
        self.version_id += 1;
        self.history.open = false;
        self.history.alternative_version_id = self.version_id;
        Some(ContentChanged { changes: Vec::new(), version_id: self.version_id, is_undoing: false, is_redoing: false, reason: EditReason::EolChange, was_dirty })
    }

    pub fn mark_saved(&mut self) {
        self.history.saved_alternative_version_id = self.history.alternative_version_id;
    }

    /// The version a save marks clean once its write is done, so edits made while it writes stay dirty.
    pub fn alternative_version_id(&self) -> u32 {
        self.history.alternative_version_id
    }

    pub fn mark_saved_at(&mut self, alternative_version_id: u32) {
        self.history.saved_alternative_version_id = alternative_version_id;
    }

    /// `codeEditorWidget.executeEdits`: each edit is a range, its text, and `forceMoveMarkers`. Without
    /// `end_selections` every selection follows the edits as the cursor's tracked range does.
    pub fn execute_edits(&mut self, edits: Vec<(Position, Position, String, bool)>, end_selections: Option<Vec<Selection>>, reason: EditReason) -> Option<ContentChanged> {
        // Cursor selections are tracked with `AlwaysGrowsWhenTypingAtEdges`: the start sticks to the character before it, the end does not.
        let end = match end_selections {
            Some(selections) => EndSelections::Given(selections),
            None => EndSelections::Tracked { selections: self.selections.iter().map(|&selection| (selection, [true, false])).collect(), collapse_grown_carets: true },
        };
        self.execute_tracked(edits, end, reason)
    }

    /// `executeCommand` of a command whose cursor state is `trackSelection` of the primary selection
    /// (replaceAllCommand.ts): the primary cursor alone stays.
    pub fn execute_edits_tracking_primary(&mut self, edits: Vec<(Position, Position, String)>, reason: EditReason) -> Option<ContentChanged> {
        let primary = self.selection();
        let sticks = track_selection_sticks(&self.document.lines, primary);
        let edits = edits.into_iter().map(|(start, end, text)| (start, end, text, false)).collect();
        self.execute_tracked(edits, EndSelections::Tracked { selections: vec![(primary, sticks)], collapse_grown_carets: false }, reason)
    }

    /// `CommandExecutor.executeCommands`: command `i` belongs to cursor `i`. A cursor whose edits overlap an
    /// earlier cursor's loses them and is removed; when the first cursor loses, nothing happens. A cursor without
    /// edits keeps its selection.
    pub fn execute_commands(&mut self, mut commands: Vec<Command>, reason: EditReason) -> Option<ContentChanged> {
        for command in &mut commands {
            command.edits.retain(|edit| !edit.is_noop());
        }
        if commands.iter().all(|command| command.edits.is_empty()) {
            return None;
        }
        let mut losers = losers(commands.iter().map(|command| command.edits.as_slice()));
        if losers.contains(&0) {
            return None;
        }
        for &loser in &losers {
            commands[loser].edits.clear();
        }
        let eol_len = self.document.eol.len();
        let lines = &self.document.lines;
        let offset = |position| document_offset(lines, eol_len, position);
        let markers: Vec<Option<([usize; 2], [bool; 2])>> =
            commands.iter().map(|command| command.tracked.map(|selection| ([offset(selection.start()), offset(selection.end())], track_selection_sticks(lines, selection)))).collect();
        // `_getInverseEditRanges`: each edit's text in the new text, the edits taken by their start in the old one.
        let mut by_start: Vec<(usize, usize, usize, usize, usize)> = Vec::new();
        for (index, command) in commands.iter().enumerate() {
            for (minor, edit) in command.edits.iter().enumerate() {
                by_start.push((offset(edit.start), offset(edit.end), edit.text.replace('\n', self.document.eol).encode_utf16().count(), index, minor));
            }
        }
        by_start.sort_by_key(|&(start, end, ..)| (start, end));
        let mut inverse: Vec<(usize, usize, usize, usize)> = Vec::new();
        let mut delta = 0isize;
        for (start, end, inserted, index, minor) in by_start {
            let new_start = (start as isize + delta) as usize;
            inverse.push((index, minor, new_start, new_start + inserted));
            delta += inserted as isize - (end - start) as isize;
        }
        inverse.sort_by_key(|&(index, minor, ..)| (index, minor));
        // Edits at one position apply last to first, so the first one's text ends up first.
        let replaces: Vec<Replace> = commands.iter().flat_map(|command| command.edits.iter().cloned()).rev().collect();
        let before = self.selections.clone();
        let change = self.apply(replaces, before.clone(), reason)?;
        let lines = &self.document.lines;
        let position = |offset| document_position(lines, eol_len, offset);
        let mut selections = Vec::new();
        for (index, (command, marker)) in commands.into_iter().zip(markers).enumerate() {
            if command.edits.is_empty() {
                selections.push(before[index]);
                continue;
            }
            let tracked = command.tracked.zip(marker).map(|(selection, ([mut start, mut end], [start_sticks, end_sticks]))| {
                for change in &change.changes {
                    let (from, to, inserted) = (change.range_offset, change.range_offset + change.range_length, change.text.encode_utf16().count());
                    start = accept_edit(start, start_sticks, from, to, inserted, false);
                    end = accept_edit(end, end_sticks, from, to, inserted, false);
                }
                let (start, end) = (position(start), position(end));
                if selection.head < selection.anchor { Selection { anchor: end, head: start } } else { Selection { anchor: start, head: end } }
            });
            let ranges: Vec<(Position, Position)> = inverse.iter().filter(|edit| edit.0 == index).map(|&(_, _, start, end)| (position(start), position(end))).collect();
            selections.push((command.cursor_state)(tracked, &ranges));
        }
        losers.sort_unstable_by(|a, b| b.cmp(a));
        for loser in losers {
            selections.remove(loser);
        }
        self.after_text_change(selections);
        self.history.undo.last_mut().expect("the edit just pushed its group").selections_after = self.selections.clone();
        Some(change)
    }

    fn execute_tracked(&mut self, edits: Vec<(Position, Position, String, bool)>, end: EndSelections, reason: EditReason) -> Option<ContentChanged> {
        let mut edits: Vec<_> = edits.into_iter().filter(|(start, end, text, _)| start != end || !text.is_empty()).collect();
        // Edits at one position apply last to first, so the first one's text ends up first (`_doApplyEdits` sorts ties by sortIndex).
        edits.reverse();
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
        let eol_len = self.document.eol.len();
        let lines = &self.document.lines;
        let tracked: &[(Selection, [bool; 2])] = match &end {
            EndSelections::Given(_) => &[],
            EndSelections::Tracked { selections, .. } => selections,
        };
        let markers: Vec<[usize; 2]> = tracked.iter().map(|(selection, _)| [document_offset(lines, eol_len, selection.start()), document_offset(lines, eol_len, selection.end())]).collect();
        let force_moves: Vec<bool> = edits.iter().map(|edit| edit.3).collect();
        let replaces = edits.into_iter().map(|(start, end, text, _)| Replace { start, end, text: normalize_line_breaks(&text) }).collect();
        let change = self.apply(replaces, self.selections.clone(), reason)?;
        let selections = match end {
            // `Cursor.setStates` validates the computed states, so a cursor past the deleted lines lands on the last.
            EndSelections::Given(selections) => selections
                .into_iter()
                .map(|selection| Selection { anchor: valid_position(&self.document.lines, selection.anchor), head: valid_position(&self.document.lines, selection.head) })
                .collect(),
            EndSelections::Tracked { selections: tracked, collapse_grown_carets } => tracked
                .iter()
                .zip(markers)
                .map(|(&(selection, [start_sticks, end_sticks]), [mut start, mut end])| {
                    for (change, &force_move) in change.changes.iter().zip(&force_moves) {
                        let (from, to, inserted) = (change.range_offset, change.range_offset + change.range_length, change.text.encode_utf16().count());
                        start = accept_edit(start, start_sticks, from, to, inserted, force_move);
                        end = accept_edit(end, end_sticks, from, to, inserted, force_move);
                    }
                    let (mut start, end) = (document_position(&self.document.lines, eol_len, start), document_position(&self.document.lines, eol_len, end));
                    // `readSelectionFromMarkers`: a caret whose range grew does not select the text.
                    if collapse_grown_carets && selection.is_empty() {
                        start = end;
                    }
                    if selection.head < selection.anchor { Selection { anchor: end, head: start } } else { Selection { anchor: start, head: end } }
                })
                .collect(),
        };
        self.after_text_change(selections);
        self.history.undo.last_mut().expect("the edit just pushed its group").selections_after = self.selections.clone();
        Some(change)
    }

    /// One edit per cursor, in the order of `selections`.
    pub fn each_cursor(&self, edit: impl Fn(Selection) -> CursorEdit) -> Vec<CursorEdit> {
        self.selections.iter().map(|&selection| edit(selection)).collect()
    }

    pub fn apply_between_stops(&mut self, edits: Vec<CursorEdit>, reason: EditReason) -> Option<ContentChanged> {
        self.push_undo_stop();
        let change = self.apply_cursor_edits(edits, reason);
        self.push_undo_stop();
        change
    }

    /// `_executeEditOperation`: the edits of every cursor as one model change. Cursors whose edits overlap
    /// an other cursor's lose them, the later cursor first (`_getLoserCursorMap`), and keep their selection.
    pub fn apply_cursor_edits(&mut self, mut edits: Vec<CursorEdit>, reason: EditReason) -> Option<ContentChanged> {
        for (edit, selection) in edits.iter_mut().zip(&self.selections) {
            edit.replaces.retain(|replace| !replace.is_noop());
            if edit.replaces.is_empty() {
                edit.after = *selection;
            }
        }
        for loser in losers(edits.iter().map(|edit| edit.replaces.as_slice())) {
            edits[loser] = CursorEdit::new(Vec::new(), self.selections[loser]);
        }
        let selections = selections_after(&edits, &self.selections);
        let replaces = edits.into_iter().flat_map(|edit| edit.replaces).collect();
        self.apply(replaces, selections, reason)
    }

    /// Applies the replaces end to beginning, so each range still holds, as one model change event
    /// added to the open undo group. Nothing happens when every replace is empty.
    fn apply(&mut self, mut replaces: Vec<Replace>, selections: Vec<Selection>, reason: EditReason) -> Option<ContentChanged> {
        replaces.retain(|replace| !replace.is_noop());
        self.history.last_kind = EditKind::Other;
        if replaces.is_empty() {
            return None;
        }
        replaces.sort_by_key(|replace| Reverse(replace.start));
        let was_dirty = self.is_dirty();
        let selections_before = self.selections.clone();
        let mut edits = Vec::new();
        let mut changes = Vec::new();
        for replace in replaces {
            let removed = text_between(&self.document.lines, replace.start, replace.end);
            changes.push(self.replace(replace.start, replace.end, &replace.text));
            edits.push(AppliedEdit { start: replace.start, removed, inserted: replace.text });
        }
        self.version_id += 1;
        let version_id = self.version_id;
        self.after_text_change(selections);
        let selections_after = self.selections.clone();
        let history = &mut self.history;
        history.redo.clear();
        let alternative_before = history.alternative_version_id;
        history.alternative_version_id = version_id;
        match history.undo.last_mut().filter(|_| history.open) {
            Some(group) => {
                group.edits.extend(edits);
                group.selections_after = selections_after;
                group.alternative_after = version_id;
            }
            None => history.undo.push(UndoGroup { edits, selections_before, selections_after, alternative_before, alternative_after: version_id }),
        }
        history.open = true;
        // Editing a preview editor pins it (editorGroupView.ts pins a preview editor once it is dirty).
        self.preview = false;
        Some(ContentChanged { changes, version_id, is_undoing: false, is_redoing: false, reason, was_dirty })
    }

    /// Word highlights stop while the text changes: wordHighlighter.ts ignores non-explicit cursor changes.
    fn after_text_change(&mut self, selections: Vec<Selection>) {
        self.selections = selections;
        self.clear_cursor_states();
        cursor::normalize(&mut self.selections, &mut self.last_added);
        self.leftovers.clear();
        self.highlights.clear();
        let indentation = self.document.indentation;
        self.guides = indent_guides(&self.document.lines, indentation.tab_size, indentation.indent_size(), self.document.off_side);
        self.refresh_folding_ranges();
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
        self.folding.accept_replace(lines, start, end, text);
        if let Some(anchor) = self.selection_anchor {
            self.selection_anchor = Some(anchor_select::anchor_after_edit(anchor, &self.document.lines, start, end, text));
        }
        self.track_markers(start, end, text);
        self.track_view_zones(start, end, text);
        let lines = &mut self.document.lines;
        let prefix: String = lines[start.line].chars().take(start.column).collect();
        let suffix: String = lines[end.line].chars().skip(end.column).collect();
        let joined = prefix + text + &suffix;
        lines.splice(start.line..=end.line, joined.split('\n').map(String::from));
        self.document.tokens.invalidate_from(start.line);
        change
    }
}

/// What copy puts on the clipboard (`ClipboardStoredMetadata`).
#[derive(Debug, PartialEq)]
pub struct Copied {
    pub text: String,
    pub from_empty_selection: bool,
    pub multicursor_text: Option<Vec<String>>,
}

/// `ShiftCommand`: every line of the selection moves one indent stop; a multi-line selection skips empty lines.
fn shift_edit(model: &Model, selection: Selection, unshift: bool) -> CursorEdit {
    let indent_empty = selection.start().line == selection.end().line;
    let mut replaces = Vec::new();
    let mut extra_spaces = 0;
    for line in cursor::shift_lines(selection) {
        extra_spaces = model.shift_extra_spaces(line, extra_spaces);
        let Some((indent_end, indent)) = cursor::shift_line_edit(&model.lines[line], model.indentation, unshift, indent_empty, extra_spaces) else { continue };
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
    CursorEdit::new(replaces, after)
}

/// `_getLoserCursorMap`: with the edits sorted by their end, last first, an edit starting before the end
/// of the next one overlaps it; the cursor with the higher index loses all its edits.
fn losers<'a>(edits: impl Iterator<Item = &'a [Replace]>) -> Vec<usize> {
    let mut operations: Vec<(usize, Position, Position)> = edits.enumerate().flat_map(|(index, replaces)| replaces.iter().map(move |replace| (index, replace.start, replace.end))).collect();
    operations.sort_by_key(|&(_, start, end)| Reverse((end, start)));
    let mut losers = Vec::new();
    let mut at = 1;
    while at < operations.len() {
        let (previous, current) = (operations[at - 1], operations[at]);
        if previous.0 == current.0 || previous.1 >= current.2 {
            at += 1;
            continue;
        }
        let loser = previous.0.max(current.0);
        losers.push(loser);
        operations.retain(|operation| operation.0 != loser);
        at = at.saturating_sub(1).max(1);
    }
    losers
}

/// Each cursor's selection after all edits: its own edits are in `after`, and every edit of an other
/// cursor that starts before its own moves it.
pub fn selections_after(edits: &[CursorEdit], before: &[Selection]) -> Vec<Selection> {
    let mut all: Vec<(usize, &Replace)> = edits.iter().enumerate().flat_map(|(index, edit)| edit.replaces.iter().map(move |replace| (index, replace))).collect();
    all.sort_by_key(|edit| Reverse(edit.1.start));
    edits
        .iter()
        .enumerate()
        .map(|(index, edit)| {
            let own_start = edit.replaces.iter().map(|replace| replace.start).min().unwrap_or(before[index].start());
            let mut after = edit.after;
            for (_, replace) in all.iter().filter(|(other, replace)| *other != index && replace.start < own_start) {
                after = Selection { anchor: shifted(after.anchor, replace), head: shifted(after.head, replace) };
            }
            after
        })
        .collect()
}

/// Where a position at or after `replace` goes once it is applied.
fn shifted(position: Position, replace: &Replace) -> Position {
    let new_end = end_of(replace.start, &replace.text);
    if position < replace.end {
        return new_end;
    }
    if position.line == replace.end.line {
        return Position::new(new_end.line, new_end.column + position.column - replace.end.column);
    }
    Position::new(position.line + new_end.line - replace.end.line, position.column)
}

/// A decoration's model offsets through a change, with the default `AlwaysGrowsWhenTypingAtEdges`.
pub(super) fn track_decoration([mut start, mut end]: [usize; 2], change: &ContentChanged) -> [usize; 2] {
    for change in &change.changes {
        let (from, to, inserted) = (change.range_offset, change.range_offset + change.range_length, change.text.encode_utf16().count());
        start = accept_edit(start, true, from, to, inserted, false);
        end = accept_edit(end, false, from, to, inserted, false);
    }
    [start, end]
}

/// `ICommand`'s `trackSelection` stickiness as whether the start and the end stick to the character before
/// them: a caret keeps to the text before it at the end of its line, else to the text after it; a selection
/// never grows (`NeverGrowsWhenTypingAtEdges`).
fn track_selection_sticks(lines: &[String], selection: Selection) -> [bool; 2] {
    match selection.is_empty() {
        true => [selection.head.column == cursor::line_len(lines, selection.head.line); 2],
        false => [false, true],
    }
}

/// The selections after `execute_tracked`: given, or each one tracked through the edits with whether its
/// start and end stick to the character before them.
enum EndSelections {
    Given(Vec<Selection>),
    Tracked { selections: Vec<(Selection, [bool; 2])>, collapse_grown_carets: bool },
}

/// intervalTree.ts `nodeAcceptEdit` for one marker: where an offset goes when `start..end` becomes `inserted` code units.
fn accept_edit(marker: usize, stick_to_previous: bool, start: usize, end: usize, inserted: usize, force_move: bool) -> usize {
    #[derive(PartialEq)]
    enum Move {
        MarkerDefined,
        Forced,
        ForcedStay,
    }
    let stays_before = |check: usize, semantics: Move| match marker.cmp(&check) {
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Greater => false,
        std::cmp::Ordering::Equal => match semantics {
            Move::Forced => false,
            Move::ForcedStay => true,
            Move::MarkerDefined => stick_to_previous,
        },
    };
    let deleted = end - start;
    let common = deleted.min(inserted);
    let semantics = if force_move { Move::Forced } else if deleted > 0 { Move::ForcedStay } else { Move::MarkerDefined };
    if stays_before(start, semantics) {
        return marker;
    }
    if common > 0 && !force_move && stays_before(start + common, if deleted > inserted { Move::ForcedStay } else { Move::MarkerDefined }) {
        return marker;
    }
    if stays_before(end, if force_move { Move::Forced } else { Move::MarkerDefined }) {
        return start + inserted;
    }
    (marker + inserted).saturating_sub(deleted)
}

/// The model offset of a position: UTF-16 code units, each line break counted as the document's EOL.
pub(super) fn document_offset(lines: &[String], eol_len: usize, position: Position) -> usize {
    lines[..position.line].iter().map(|line| line.encode_utf16().count() + eol_len).sum::<usize>() + utf16_column(&lines[position.line], position.column)
}

pub(super) fn document_position(lines: &[String], eol_len: usize, mut offset: usize) -> Position {
    for (index, line) in lines.iter().enumerate() {
        let length = line.encode_utf16().count();
        if offset <= length || index == lines.len() - 1 {
            return Position::new(index, char_column(line, offset.min(length)));
        }
        offset -= length + eol_len;
    }
    unreachable!("a document has at least one line")
}

/// The char column at a UTF-16 column; one inside a surrogate pair lands after the pair.
pub fn char_column(line: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (index, c) in line.chars().enumerate() {
        if units >= utf16 {
            return index;
        }
        units += c.len_utf16();
    }
    line.chars().count()
}

pub fn utf16_column(line: &str, column: usize) -> usize {
    line.chars().take(column).map(char::len_utf16).sum()
}

/// The text from `start` to `end`, lines joined by "\n".
/// `TextModel.validatePosition`: a line past the document becomes the end of the last line.
fn valid_position(lines: &[String], position: Position) -> Position {
    if position.line >= lines.len() {
        return cursor::last_position(lines);
    }
    Position::new(position.line, position.column.min(cursor::line_len(lines, position.line)))
}

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
/// replaceCommand.ts `ReplaceOvertypeCommand`: the selection's end moved past as many chars as `text` has (one
/// fewer for a selection), never past its line's end.
fn overtype_end(lines: &[String], selection: Selection, text: &str) -> Position {
    let end = selection.end();
    let delta = (text.chars().count() + usize::from(selection.is_empty())).saturating_sub(1);
    Position::new(end.line, (end.column + delta).min(cursor::line_len(lines, end.line)))
}

pub fn end_of(start: Position, text: &str) -> Position {
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
    use crate::editor::auto_indent::Strategy;
    use crate::editor::{EditorGroup, Indentation, TextDocument};
    use crate::languages::Languages;
    use std::path::PathBuf;

    /// `TestLanguageConfigurationService` with no language registered.
    pub(in crate::editor) fn no_rules(strategy: Strategy) -> AutoIndent<'static> {
        AutoIndent { languages: Box::leak(Box::new(Languages::new(&[]))), strategy }
    }

    #[test]
    fn executed_edits_carry_the_selection_as_its_tracked_range() {
        let api = EditReason::Unknown("MainThreadTextEditor");
        // TextEditorEdit.insert forces markers to move: the caret ends after the text.
        let mut inserted = editor("", "\n");
        inserted.execute_edits(vec![(Position::new(0, 0), Position::new(0, 0), "Hello".into(), true)], None, api);
        assert_eq!(inserted.selection(), Selection::caret(Position::new(0, 5)));
        // A replace at the caret grows its tracked range over the new text; `readSelectionFromMarkers` collapses it to the end.
        let mut replaced = editor("ab\ncd", "\r\n");
        replaced.move_to(Position::new(1, 1), false);
        replaced.execute_edits(vec![(Position::new(1, 1), Position::new(1, 1), "x\ny".into(), false)], None, api);
        assert_eq!(replaced.selection(), Selection::caret(Position::new(2, 1)));
        // Text deleted around the selection collapses it to the deletion's start; edits after it leave it alone.
        let mut deleted = editor("one two three", "\n");
        deleted.move_to(Position::new(0, 5), false);
        deleted.move_to(Position::new(0, 6), true);
        let edits = vec![(Position::new(0, 4), Position::new(0, 8), String::new(), false), (Position::new(0, 8), Position::new(0, 13), "3".into(), false)];
        deleted.execute_edits(edits, None, api);
        assert_eq!((deleted.document.lines[0].as_str(), deleted.selection()), ("one 3", Selection::caret(Position::new(0, 4))));
    }

    #[test]
    fn edits_at_one_position_insert_in_their_order() {
        // workspace.test.ts #80688: two inserts at 0,0 in one edit read 'import1;import2;'.
        let mut model = editor("", "\n");
        let at_start = |text: &str| (Position::new(0, 0), Position::new(0, 0), text.to_string(), true);
        model.execute_edits(vec![at_start("import1;"), at_start("import2;")], None, EditReason::ApplyEdits);
        assert_eq!(model.document.lines, ["import1;import2;"]);
    }

    #[test]
    fn push_eol_changes_the_line_ending_and_dirties_the_model() {
        let mut model = editor("foo\nbar", "\n");
        assert!(model.push_eol(0).is_none());
        let change = model.push_eol(1).expect("LF to CRLF");
        assert_eq!((change.version_id, change.changes.len(), model.document.eol, model.is_dirty()), (2, 0, "\r\n", true));
    }

    pub(in crate::editor) fn editor(text: &str, eol: &'static str) -> OpenEditor {
        let document = TextDocument {
            scheme: "file".into(),
            path: PathBuf::from("/w/a.ts"),
            lines: text.split('\n').map(String::from).collect(),
            eol,
            encoding: "utf8".into(),
            language_id: "typescript".into(),
            indentation: Indentation { tab_size: 4, indent_size: None, insert_spaces: true },
            off_side: false,
            folding_markers: None,
            block_comment: None,
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
        assert_eq!(editor.selection(), Selection::caret(at(2, 1)));

        editor.set_selections(vec![Selection { anchor: at(0, 3), head: at(0, 1) }]);
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
        assert_eq!((text(&editor), editor.selection().head), ("ab".into(), at(0, 2)));
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
        editor.document.encoding = "utf8bom".into();
        editor.move_to(at(0, 1), false);
        editor.enter(no_rules(Strategy::Full));
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
    fn enter_keeps_the_indentation_and_tab_indents_a_blank_line_without_rules_by_one_tab() {
        let auto = no_rules(Strategy::Full);
        let mut editor = editor("    let x", "\n");
        editor.move_to(at(0, 9), false);
        editor.enter(auto);
        assert_eq!(editor.document.lines, ["    let x", "    "]);
        editor.type_text("y");
        editor.move_to(at(1, 0), false);
        editor.delete_all_right();
        assert_eq!(editor.document.lines[1], "");
        let change = editor.tab(auto).unwrap();
        assert_eq!((editor.document.lines[1].as_str(), editor.selection().head), ("    ", at(1, 4)));
        assert_eq!(change.reason, command("tab"));
        editor.tab(auto);
        assert_eq!(editor.document.lines[1], "        ");
    }

    #[test]
    fn tab_over_lines_shifts_them_and_keeps_the_selection_on_them() {
        let mut editor = editor("a\n  b\n\nc", "\n");
        editor.set_selections(vec![Selection { anchor: at(0, 0), head: at(3, 1) }]);
        editor.tab(no_rules(Strategy::Full));
        assert_eq!(editor.document.lines, ["    a", "    b", "", "    c"]);
        assert_eq!(editor.selection(), Selection { anchor: at(0, 0), head: at(3, 5) });
        editor.outdent(no_rules(Strategy::Full));
        assert_eq!(editor.document.lines, ["a", "b", "", "c"]);
        editor.undo();
        editor.undo();
        assert_eq!(text(&editor), "a\n  b\n\nc");
    }

    #[test]
    fn cut_and_paste_a_whole_line_from_an_empty_selection() {
        let mut editor = editor("one\ntwo\nthree", "\n");
        editor.move_to(at(1, 1), false);
        let copied = editor.copy_text();
        assert_eq!((copied.text.as_str(), copied.from_empty_selection, copied.multicursor_text.is_none()), ("two\n", true, true));
        editor.cut();
        assert_eq!((text(&editor), editor.selection().head), ("one\nthree".into(), at(1, 0)));
        editor.move_to(at(1, 2), false);
        editor.paste(&copied.text, copied.from_empty_selection, None, false);
        assert_eq!((text(&editor), editor.selection().head), ("one\ntwo\nthree".into(), at(2, 2)));
        editor.move_to(at(2, 5), false);
        editor.cut();
        assert_eq!(text(&editor), "one\ntwo");
    }

    #[test]
    fn moves_collapse_selections_and_keep_the_column_through_short_lines() {
        let mut editor = editor("abcd\nx\nabcd", "\n");
        editor.set_selections(vec![Selection { anchor: at(0, 1), head: at(0, 3) }]);
        editor.move_horizontally(false, false);
        assert_eq!(editor.selection(), Selection::caret(at(0, 1)));
        editor.move_to(at(0, 4), false);
        editor.move_vertically(true, false);
        editor.move_vertically(true, true);
        assert_eq!(editor.selection(), Selection { anchor: at(1, 1), head: at(2, 4) });
        editor.select_all();
        assert_eq!(editor.selection().head, at(2, 4));
    }

    fn range(anchor: (usize, usize), head: (usize, usize)) -> Selection {
        Selection { anchor: at(anchor.0, anchor.1), head: at(head.0, head.1) }
    }

    #[test]
    fn multi_cursor_paste_spreads_pieces_or_lines_over_the_cursors() {
        // cursor.test.ts issue #4996: multicursorText pieces go one per cursor.
        let mut pieces = editor("line1\nline2\nline3", "\n");
        pieces.set_selections(vec![range((0, 0), (0, 0)), range((1, 0), (1, 0))]);
        pieces.paste("a\nb\nc\nd", false, Some(&["a\nb".into(), "c\nd".into()]), false);
        assert_eq!(text(&pieces), "a\nbline1\nc\ndline2\nline3");
        // multiCursorPasteCases: as many lines as cursors spread, a trailing line break ignored; other counts paste whole.
        let cases: [(&str, &[Selection], &str, &str); 4] = [
            ("test\ntest\ntest\ntest", &[range((0, 0), (0, 4)), range((1, 0), (1, 4)), range((2, 0), (2, 4)), range((3, 0), (3, 4))], "aaa\nbbb\nccc\n", "aaa\nbbb\nccc\n\naaa\nbbb\nccc\n\naaa\nbbb\nccc\n\naaa\nbbb\nccc\n"),
            ("test\ntest\ntest\ntest", &[range((0, 0), (0, 4)), range((1, 0), (1, 4)), range((2, 0), (2, 4)), range((3, 0), (3, 4))], "aaa\r\nbbb\r\nccc\r\nddd\r\n", "aaa\nbbb\nccc\nddd"),
            ("line1\nline2\nline3", &[range((0, 0), (0, 0)), range((1, 0), (1, 0)), range((2, 0), (2, 0))], "a\nb\nc", "aline1\nbline2\ncline3"),
            ("line1\nline2\nline3", &[range((0, 0), (0, 0)), range((1, 0), (1, 0)), range((2, 0), (2, 0))], "a\nb\nc\n", "aline1\nbline2\ncline3"),
        ];
        for (before, selections, pasted, expected) in cases {
            let mut lines = editor(before, "\n");
            lines.set_selections(selections.to_vec());
            lines.paste(pasted, false, None, false);
            assert_eq!(text(&lines), expected, "pasting {pasted:?}");
        }
    }

    #[test]
    fn overtype_mode_replaces_the_characters_after_the_cursor_up_to_the_line_end() {
        // cursor.test.ts suite 'Overtype Mode'.
        let typed = |selection, typed: &str| {
            let mut editor = editor("123456789\n123456789", "\n");
            editor.set_selections(vec![selection]);
            editor.type_keyboard(typed, no_rules(Strategy::None), true);
            text(&editor)
        };
        assert_eq!(typed(range((0, 2), (0, 2)), "a"), "12a456789\n123456789");
        assert_eq!(typed(range((0, 8), (0, 8)), "bbb"), "12345678bbb\n123456789");
        assert_eq!(typed(range((0, 4), (1, 2)), "cc"), "1234cc456789");
        let pasted = |selection, pasted: &str| {
            let mut editor = editor("123456789\n123456789", "\n");
            editor.set_selections(vec![selection]);
            editor.paste(pasted, false, None, true);
            text(&editor)
        };
        assert_eq!(pasted(range((0, 4), (0, 4)), "cc"), "1234cc789\n123456789");
        assert_eq!(pasted(range((0, 4), (0, 4)), "dddddddd"), "1234dddddddd\n123456789");
        assert_eq!(pasted(range((0, 4), (1, 2)), "cc"), "1234cc456789");
        assert_eq!(pasted(range((0, 4), (0, 4)), "aaaaaaa\nbbbbbbb"), "1234aaaaaaa\nbbbbbbb\n123456789");
        let mut composed = editor("123456789\n123456789", "\n");
        composed.set_selections(vec![range((0, 4), (0, 4))]);
        composed.type_replacing("セ", 0, 0, "type");
        assert_eq!(text(&composed), "1234セ56789\n123456789");
        composed.overtype_composed("セ");
        assert_eq!(text(&composed), "1234セ6789\n123456789");
    }

    #[test]
    fn typing_over_several_selections_undoes_as_one_step() {
        // cursor.test.ts issue #93585, with the cursors given bottom first.
        let mut editor = editor("hello world\nhello world", "\n");
        editor.set_selections(vec![range((1, 6), (1, 11)), range((0, 6), (0, 11))]);
        editor.type_text("no");
        assert_eq!(text(&editor), "hello no\nhello no");
        assert_eq!(editor.selections, [Selection::caret(at(1, 8)), Selection::caret(at(0, 8))]);
        editor.undo();
        assert_eq!(text(&editor), "hello world\nhello world");
        assert_eq!(editor.selections, [range((1, 6), (1, 11)), range((0, 6), (0, 11))]);
    }

    #[test]
    fn every_cursor_moves_and_edits_with_the_line_shifts_of_the_ones_before() {
        // cursor.test.ts issue #105730: each selection collapses to its own start.
        let mut moved = editor("asdfghjkl, asdfghjkl, asdfghjkl, ", "\n");
        moved.set_selections(vec![range((0, 9), (0, 11)), range((0, 20), (0, 22)), range((0, 31), (0, 33))]);
        moved.move_horizontally(false, false);
        assert_eq!(moved.selections, [Selection::caret(at(0, 9)), Selection::caret(at(0, 20)), Selection::caret(at(0, 31))]);
        // An Enter at each cursor puts the later cursors on new lines.
        let mut split = editor("ab\ncd", "\n");
        split.set_selections(vec![range((0, 1), (0, 1)), range((1, 1), (1, 1))]);
        split.enter(no_rules(Strategy::Full));
        assert_eq!(text(&split), "a\nb\nc\nd");
        assert_eq!(split.selections, [Selection::caret(at(1, 0)), Selection::caret(at(3, 0))]);
        // Touching deletes both apply, then the cursors meet and merge.
        let mut touching = editor("abc", "\n");
        touching.set_selections(vec![range((0, 2), (0, 2)), range((0, 3), (0, 3))]);
        touching.delete_left();
        assert_eq!(text(&touching), "a");
        assert_eq!(touching.selections, [Selection::caret(at(0, 1))]);
        // Overlapping deletes: the later cursor loses its edit (`_getLoserCursorMap`) and only follows the text.
        let mut overlapping = editor("ab cd", "\n");
        overlapping.set_selections(vec![range((0, 4), (0, 4)), range((0, 5), (0, 5))]);
        overlapping.delete_word_left();
        assert_eq!(text(&overlapping), "ab d");
        assert_eq!(overlapping.selections, [Selection::caret(at(0, 3)), Selection::caret(at(0, 4))]);
    }

    /// lineSelection.test.ts (columns 0-based here).
    #[test]
    fn expand_line_selection_grows_by_whole_lines() {
        let mut editor = editor("    \tMy First Line\t \n\tMy Second Line\n    Third Line🐶\n\n1", "\n");
        for column in [0, 1, 4, 18, 19, 20] {
            editor.set_selections(vec![Selection::caret(at(0, column))]);
            editor.expand_line_selection();
            assert_eq!(editor.selections, [range((0, 0), (1, 0))]);
        }
        for expected in [at(2, 0), at(3, 0), at(4, 0), at(4, 1), at(4, 1)] {
            editor.expand_line_selection();
            assert_eq!(editor.selections, [Selection { anchor: at(0, 0), head: expected }]);
        }
    }

    #[test]
    fn copy_joins_selections_in_document_order_and_keeps_the_pieces() {
        let mut editor = editor("one\ntwo\nthree", "\r\n");
        editor.set_selections(vec![range((2, 0), (2, 2)), range((0, 0), (1, 1))]);
        let copied = editor.copy_text();
        assert_eq!(copied.text, "one\r\nt\r\nth");
        assert_eq!(copied.multicursor_text, Some(vec!["one\r\nt".to_string(), "th".to_string()]));
        assert!(!copied.from_empty_selection);
        // Two empty cursors on one line copy it once; several empty cursors are not "from empty selection".
        editor.set_selections(vec![range((1, 0), (1, 0)), range((1, 2), (1, 2))]);
        let copied = editor.copy_text();
        assert_eq!((copied.text.as_str(), copied.from_empty_selection), ("two\r\n", false));
    }
}
