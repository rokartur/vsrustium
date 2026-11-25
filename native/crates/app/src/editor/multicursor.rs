//! Multi-cursor commands: adding and removing cursors from the keyboard and the mouse (multicursor.ts,
//! coreCommands.ts `CreateCursor`, `_lastCursor*`, `removeSecondaryCursors`, `cancelSelection`) and
//! column selection (cursorColumnSelection.ts).

use super::cursor::{self, Position, Selection};
use super::{OpenEditor, logical_visible_column, word_at};

/// What a command scrolls into view: all cursors (`revealAllCursors`), one position
/// (`revealTopMostCursor`, `revealBottomMostCursor`), or nothing (`setSelections`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reveal {
    Nothing,
    All,
    Position(Position),
    /// `revealPositionInCenterIfOutsideViewport`, as the folding actions reveal.
    CenterIfOutside(Position),
}

/// `IColumnSelectData`: the corner a column selection started from and the one it goes to, in visible columns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnSelectData {
    pub from_line: usize,
    pub from_visible: usize,
    pub to_line: usize,
    pub to_visible: usize,
}

impl OpenEditor {
    /// `insertCursorAbove`/`insertCursorBelow` (`addCursorUp`/`addCursorDown` on logical lines): every
    /// cursor stays and gets a copy one line up or down, anchor and head moved by `translateUp`/`translateDown`.
    pub fn insert_cursors_vertically(&mut self, down: bool) -> Reveal {
        let tab_size = self.document.indentation.tab_size;
        let lines = &self.document.lines;
        let mut selections = Vec::new();
        let mut leftovers = Vec::new();
        for (index, &selection) in self.selections.iter().enumerate() {
            let (anchor_leftover, head_leftover) = self.leftover(index);
            let (anchor, anchor_leftover_after) = cursor::translate(lines, self.folding.hidden(), selection.anchor, anchor_leftover, down, tab_size);
            let (head, head_leftover_after) = cursor::translate(lines, self.folding.hidden(), selection.head, head_leftover, down, tab_size);
            selections.extend([selection, Selection { anchor, head }]);
            leftovers.extend([(anchor_leftover, head_leftover), (anchor_leftover_after, head_leftover_after)]);
        }
        self.set_cursor_states(selections, leftovers);
        let heads = self.selections.iter().map(|selection| selection.head);
        Reveal::Position(if down { heads.max() } else { heads.min() }.expect("an editor has a cursor"))
    }

    /// `insertCursorAtEndOfEachLineSelected`: a cursor at the end of every line a selection spans, and at
    /// its end unless that is a line start. Empty selections give none; with none at all nothing changes.
    pub fn insert_cursors_at_line_ends(&mut self) -> Reveal {
        let mut carets = Vec::new();
        for selection in self.selections.iter().filter(|selection| !selection.is_empty()) {
            let (start, end) = (selection.start(), selection.end());
            for line in start.line..end.line {
                carets.push(Selection::caret(cursor::line_end(&self.document.lines, Position::new(line, 0))));
            }
            if end.column > 0 {
                carets.push(Selection::caret(end));
            }
        }
        if !carets.is_empty() {
            self.set_selections(carets);
        }
        Reveal::Nothing
    }

    /// `addCursorsToBottom`/`addCursorsToTop`: the primary selection's columns on every line from its
    /// start line to the last or first line, clamped to each line.
    pub fn add_cursors_to_edge(&mut self, bottom: bool) -> Reveal {
        let (start, end) = (self.selection().start(), self.selection().end());
        let lines: Vec<usize> = if bottom { (start.line..self.document.lines.len()).collect() } else { (0..=start.line).rev().collect() };
        let selections = lines
            .into_iter()
            .map(|line| {
                let len = cursor::line_len(&self.document.lines, line);
                Selection { anchor: Position::new(line, start.column.min(len)), head: Position::new(line, end.column.min(len)) }
            })
            .collect();
        self.set_selections(selections);
        Reveal::Nothing
    }

    /// `FocusNextCursor`/`FocusPreviousCursor`: the cursors rotate, so the next or the previous one becomes primary.
    pub fn focus_next_cursor(&mut self, next: bool) -> Reveal {
        let mut selections = self.selections.clone();
        match next {
            true => selections.rotate_left(1),
            false => selections.rotate_right(1),
        }
        self.set_selections(selections);
        Reveal::Position(self.selection().head)
    }

    /// `removeSecondaryCursors` (Escape with several cursors): only the primary one stays.
    pub fn remove_secondary_cursors(&mut self) -> Option<Reveal> {
        if self.selections.len() < 2 {
            return None;
        }
        self.set_selections(vec![self.selection()]);
        Some(Reveal::All)
    }

    /// `cancelSelection` (Escape with a selection): the primary cursor collapses to its head.
    pub fn cancel_selection(&mut self) -> Option<Reveal> {
        if self.selections.iter().all(Selection::is_empty) {
            return None;
        }
        self.set_selections(vec![Selection::caret(self.selection().head)]);
        Some(Reveal::All)
    }

    /// `createCursor` (Alt+click): a cursor at `position`, or with several cursors, the removal of the
    /// first one whose selection holds it.
    pub fn toggle_cursor(&mut self, position: Position) {
        let mut selections = self.selections.clone();
        let holding = selections.iter().position(|selection| selection.start() <= position && position <= selection.end());
        match holding {
            Some(index) if selections.len() > 1 => {
                selections.remove(index);
            }
            _ => selections.push(Selection::caret(position)),
        }
        self.set_selections(selections);
    }

    /// `_lastCursorMoveToSelect` (Alt+drag): the cursor added last extends its selection to `position`.
    pub fn last_cursor_select_to(&mut self, position: Position) {
        let index = self.last_added();
        self.set_last_cursor(index, self.selections[index].moved(position, true));
    }

    /// `_lastCursorWordSelect` (Alt+double click): the cursor added last selects the word at `position`,
    /// the same word a double click selects.
    pub fn last_cursor_select_word(&mut self, position: Position) {
        let word = word_at(&self.document.lines[position.line], position.column).unwrap_or(position.column..position.column);
        let index = self.last_added();
        self.set_last_cursor(index, Selection { anchor: Position::new(position.line, word.start), head: Position::new(position.line, word.end) });
    }

    /// `_lastCursorLineSelect` (Alt+triple click): the cursor added last selects the line with its line break.
    pub fn last_cursor_select_line(&mut self, line: usize) {
        let end = match line + 1 < self.document.lines.len() {
            true => Position::new(line + 1, 0),
            false => cursor::line_end(&self.document.lines, Position::new(line, 0)),
        };
        let index = self.last_added();
        self.set_last_cursor(index, Selection { anchor: Position::new(line, 0), head: end });
    }

    fn set_last_cursor(&mut self, index: usize, selection: Selection) {
        let mut selections = self.selections.clone();
        selections[index] = selection;
        let leftovers = (0..selections.len()).map(|other| if other == index { (0, 0) } else { self.leftover(other) }).collect();
        self.set_cursor_states(selections, leftovers);
    }

    /// `getCursorColumnSelectData`: the last column selection while its cursors are unchanged, else the
    /// primary cursor from its anchor to its head.
    pub fn column_select_data(&self) -> ColumnSelectData {
        if let Some((data, selections)) = &self.column_select
            && *selections == self.selections
        {
            return *data;
        }
        let tab_size = self.document.indentation.tab_size;
        let Selection { anchor, head } = self.selection();
        let visible = |position: Position| logical_visible_column(&self.document.lines[position.line], position.column, tab_size);
        ColumnSelectData { from_line: anchor.line, from_visible: visible(anchor), to_line: head.line, to_visible: visible(head) }
    }

    /// `ColumnSelection.columnSelect`: a cursor on every line from `from_line` to `to_line` selecting the
    /// visible columns between, skipping lines that end before the block; when every line does, a cursor at
    /// each line's end. Reveals the cursor at the end the selection grew towards.
    pub fn column_select(&mut self, data: ColumnSelectData) -> Reveal {
        let ColumnSelectData { from_line, from_visible, to_line, to_visible } = data;
        let tab_size = self.document.indentation.tab_size;
        let reversed = from_line > to_line;
        let lines: Vec<usize> = if reversed { (to_line..=from_line).rev().collect() } else { (from_line..=to_line).collect() };
        let mut selections = Vec::new();
        for &line in &lines {
            let text = &self.document.lines[line];
            let start = cursor::column_from_visible(text, from_visible, tab_size);
            let end = cursor::column_from_visible(text, to_visible, tab_size);
            let (visible_start, visible_end) = (logical_visible_column(text, start, tab_size), logical_visible_column(text, end, tab_size));
            if from_visible < to_visible && (visible_start > to_visible || visible_end < from_visible) {
                continue;
            }
            if from_visible > to_visible && (visible_end > from_visible || visible_start < to_visible) {
                continue;
            }
            selections.push(Selection { anchor: Position::new(line, start), head: Position::new(line, end) });
        }
        if selections.is_empty() {
            selections = lines.iter().map(|&line| Selection::caret(cursor::line_end(&self.document.lines, Position::new(line, 0)))).collect();
        }
        self.set_selections(selections);
        self.column_select = Some((data, self.selections.clone()));
        let heads = self.selections.iter().map(|selection| selection.head);
        Reveal::Position(if reversed { heads.min() } else { heads.max() }.expect("a column selection has a cursor"))
    }

    /// `cursorColumnSelectLeft`/`Right` (Cmd+Shift+Alt+Left/Right): the moving corner one visible column
    /// over, no further right than the longest line's end.
    pub fn column_select_horizontally(&mut self, right: bool) -> Reveal {
        let mut data = self.column_select_data();
        if !right {
            data.to_visible = data.to_visible.saturating_sub(1);
            return self.column_select(data);
        }
        let tab_size = self.document.indentation.tab_size;
        let max_visible = (data.from_line.min(data.to_line)..=data.from_line.max(data.to_line))
            .map(|line| {
                let text = &self.document.lines[line];
                logical_visible_column(text, text.chars().count(), tab_size)
            })
            .max()
            .expect("a range of lines");
        if data.to_visible < max_visible {
            data.to_visible += 1;
        }
        self.column_select(data)
    }

    /// `cursorColumnSelectUp`/`Down` and their `PageUp`/`PageDown` forms: the moving corner `count` lines over.
    pub fn column_select_vertically(&mut self, down: bool, count: usize) -> Reveal {
        let mut data = self.column_select_data();
        data.to_line = if down { (data.to_line + count).min(self.document.lines.len() - 1) } else { data.to_line.saturating_sub(count) };
        self.column_select(data)
    }

    /// toggleColumnSelection.ts after `editor.columnSelection` flipped: turned on, the primary selection becomes a
    /// column selection from its anchor to its head; turned off, the column selection's corners become one selection.
    pub fn column_selection_mode_changed(&mut self, on: bool) -> Reveal {
        if on {
            self.column_select = None;
            return self.column_select(self.column_select_data());
        }
        let data = self.column_select_data();
        let tab_size = self.document.indentation.tab_size;
        let position = |line: usize, visible: usize| Position::new(line, cursor::column_from_visible(&self.document.lines[line], visible, tab_size));
        let selection = Selection { anchor: position(data.from_line, data.from_visible), head: position(data.to_line, data.to_visible) };
        self.set_selections(vec![selection]);
        Reveal::Nothing
    }

    /// `columnSelect` from the mouse (Shift+Alt+click, Shift+Alt+drag, middle button): the block from where
    /// the last column selection started, or with `extend` false from the pointer, to the pointer.
    pub fn column_select_to_pointer(&mut self, line: usize, visible: usize, extend: bool) -> Reveal {
        let (from_line, from_visible) = match extend {
            true => {
                let data = self.column_select_data();
                (data.from_line, data.from_visible)
            }
            false => (line, visible),
        };
        self.column_select(ColumnSelectData { from_line, from_visible, to_line: line, to_visible: visible })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str) -> OpenEditor {
        crate::editor::edit::tests::editor(text, "\n")
    }

    fn at(line: usize, column: usize) -> Position {
        Position::new(line, column)
    }

    fn range(anchor: (usize, usize), head: (usize, usize)) -> Selection {
        Selection { anchor: at(anchor.0, anchor.1), head: at(head.0, head.1) }
    }

    #[test]
    fn focus_next_and_previous_cursor_rotate_the_primary_cursor() {
        let mut lines = editor("a\nb\nc");
        let carets = [Selection::caret(at(0, 0)), Selection::caret(at(1, 0)), Selection::caret(at(2, 0))];
        lines.set_selections(carets.to_vec());
        assert_eq!(lines.focus_next_cursor(true), Reveal::Position(at(1, 0)));
        assert_eq!(lines.selections, [carets[1], carets[2], carets[0]]);
        assert_eq!(lines.focus_next_cursor(false), Reveal::Position(at(0, 0)));
        assert_eq!(lines.selections, carets);
    }

    #[test]
    fn column_selection_mode_turns_the_selection_into_a_block_and_back() {
        let mut lines = editor("abcd\nab\nabcd");
        lines.set_selections(vec![range((0, 1), (2, 3))]);
        assert_eq!(lines.column_selection_mode_changed(true), Reveal::Position(at(2, 3)));
        assert_eq!(lines.selections, [range((0, 1), (0, 3)), range((1, 1), (1, 2)), range((2, 1), (2, 3))]);
        assert_eq!(lines.column_selection_mode_changed(false), Reveal::Nothing);
        assert_eq!(lines.selections, [range((0, 1), (2, 3))]);
    }

    #[test]
    fn insert_cursor_below_keeps_the_column_through_short_lines() {
        let mut lines = editor("abcdef\nab\nabcdef");
        lines.set_selections(vec![Selection::caret(at(0, 5))]);
        assert_eq!(lines.insert_cursors_vertically(true), Reveal::Position(at(1, 2)));
        assert_eq!(lines.insert_cursors_vertically(true), Reveal::Position(at(2, 5)));
        assert_eq!(lines.selections, [Selection::caret(at(0, 5)), Selection::caret(at(1, 2)), Selection::caret(at(2, 5))]);
        // On the last line the copies land on the cursors and merge away.
        lines.insert_cursors_vertically(true);
        assert_eq!(lines.selections.len(), 3);
        // A selection is copied with its anchor and head.
        let mut selected = editor("abc\nabc");
        selected.set_selections(vec![range((0, 1), (0, 3))]);
        assert_eq!(selected.insert_cursors_vertically(false), Reveal::Position(at(0, 3)));
        selected.insert_cursors_vertically(true);
        assert_eq!(selected.selections, [range((0, 1), (0, 3)), range((1, 1), (1, 3))]);
    }

    #[test]
    fn insert_cursors_at_line_ends_of_each_selected_line() {
        let mut lines = editor("one\ntwo\nthree");
        lines.set_selections(vec![range((0, 1), (2, 2))]);
        lines.insert_cursors_at_line_ends();
        assert_eq!(lines.selections, [Selection::caret(at(0, 3)), Selection::caret(at(1, 3)), Selection::caret(at(2, 2))]);
        // A selection ending at a line start gives no cursor there; an empty one gives none at all.
        let mut to_start = editor("one\ntwo");
        to_start.set_selections(vec![range((0, 0), (1, 0))]);
        to_start.insert_cursors_at_line_ends();
        assert_eq!(to_start.selections, [Selection::caret(at(0, 3))]);
        to_start.insert_cursors_at_line_ends();
        assert_eq!(to_start.selections, [Selection::caret(at(0, 3))]);
    }

    #[test]
    fn add_cursors_to_bottom_and_top_clamp_the_columns_to_each_line() {
        let mut lines = editor("abcd\na\nabcd");
        lines.set_selections(vec![range((1, 0), (1, 1))]);
        lines.add_cursors_to_edge(true);
        assert_eq!(lines.selections, [range((1, 0), (1, 1)), range((2, 0), (2, 1))]);
        lines.set_selections(vec![Selection::caret(at(2, 3))]);
        lines.add_cursors_to_edge(false);
        assert_eq!(lines.selections, [Selection::caret(at(2, 3)), Selection::caret(at(1, 1)), Selection::caret(at(0, 3))]);
    }

    #[test]
    fn escape_removes_the_secondary_cursors_then_the_selection() {
        let mut lines = editor("abc\nabc");
        lines.set_selections(vec![range((0, 0), (0, 2)), Selection::caret(at(1, 1))]);
        assert_eq!(lines.remove_secondary_cursors(), Some(Reveal::All));
        assert_eq!(lines.selections, [range((0, 0), (0, 2))]);
        assert_eq!(lines.remove_secondary_cursors(), None);
        assert_eq!(lines.cancel_selection(), Some(Reveal::All));
        assert_eq!(lines.selections, [Selection::caret(at(0, 2))]);
        assert_eq!(lines.cancel_selection(), None);
    }

    #[test]
    fn alt_click_toggles_a_cursor_and_alt_drag_extends_the_last_added_one() {
        let mut lines = editor("abc\nabc\nabc");
        lines.toggle_cursor(at(1, 1));
        lines.toggle_cursor(at(2, 1));
        assert_eq!(lines.selections.len(), 3);
        lines.last_cursor_select_to(at(2, 3));
        assert_eq!(lines.selections[2], range((2, 1), (2, 3)));
        // Clicking inside a cursor's selection removes that cursor; with one cursor left it adds instead.
        lines.toggle_cursor(at(2, 2));
        assert_eq!(lines.selections, [Selection::caret(at(0, 0)), Selection::caret(at(1, 1))]);
        lines.last_cursor_select_word(at(1, 1));
        assert_eq!(lines.selections[1], range((1, 0), (1, 3)));
        lines.toggle_cursor(at(1, 2));
        assert_eq!(lines.selections, [Selection::caret(at(0, 0))]);
        lines.toggle_cursor(at(0, 0));
        assert_eq!(lines.selections, [Selection::caret(at(0, 0))]);
        lines.last_cursor_select_line(1);
        assert_eq!(lines.selections, [range((1, 0), (2, 0))]);
    }

    #[test]
    fn column_select_keeps_its_corner_and_skips_short_lines() {
        // cursorColumnSelection.test.ts style: Cmd+Shift+Alt+Right then Down from (0, 1).
        let mut lines = editor("abcdef\nab\n\nabcdefgh");
        lines.set_selections(vec![Selection::caret(at(0, 1))]);
        lines.column_select_horizontally(true);
        lines.column_select_horizontally(true);
        assert_eq!(lines.selections, [range((0, 1), (0, 3))]);
        assert_eq!(lines.column_select_vertically(true, 3), Reveal::Position(at(3, 3)));
        // "ab" ends inside the block and the empty line before it.
        assert_eq!(lines.selections, [range((0, 1), (0, 3)), range((1, 1), (1, 2)), range((3, 1), (3, 3))]);
        assert_eq!(lines.column_select_data(), ColumnSelectData { from_line: 0, from_visible: 1, to_line: 3, to_visible: 3 });
        // Any other cursor change ends the column selection.
        lines.move_horizontally(true, false);
        assert_eq!(lines.column_select_data().from_line, lines.selection().head.line);
        // A block right of every line becomes a cursor at each line's end.
        let mut past = editor("ab\na");
        past.column_select(ColumnSelectData { from_line: 0, from_visible: 5, to_line: 1, to_visible: 7 });
        assert_eq!(past.selections, [Selection::caret(at(0, 2)), Selection::caret(at(1, 1))]);
        // Upwards it reveals the top cursor; the mouse starts a block at the pointer.
        assert_eq!(past.column_select_to_pointer(1, 1, false), Reveal::Position(at(1, 1)));
        assert_eq!(past.column_select_to_pointer(0, 0, true), Reveal::Position(at(0, 0)));
        assert_eq!(past.selections, [range((1, 1), (1, 0)), range((0, 1), (0, 0))]);
    }
}
