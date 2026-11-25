//! Where cursor commands move and what delete commands remove, as cursorMoveOperations.ts,
//! cursorWordOperations.ts and cursorDeleteOperations.ts compute it. Positions are 0-based lines and
//! char columns; upstream counts 1-based UTF-16 columns.

use std::ops::Range;

use super::{Indentation, folding, graphemes};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

impl Position {
    pub fn new(line: usize, column: usize) -> Self {
        Position { line, column }
    }
}

/// The anchor stays where the selection started; the head is the cursor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Position,
    pub head: Position,
}

impl Selection {
    pub fn caret(position: Position) -> Self {
        Selection { anchor: position, head: position }
    }

    pub fn start(&self) -> Position {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> Position {
        self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The head moved to `position`; with `select` the anchor stays, else the selection collapses.
    pub fn moved(&self, position: Position, select: bool) -> Self {
        if select { Selection { anchor: self.anchor, head: position } } else { Selection::caret(position) }
    }
}

/// `CursorCollection.normalize` with `multiCursorMergeOverlapping`: touching cursors merge when one is
/// empty, selections only when they overlap. The older cursor wins and keeps its direction, unless the
/// loser is the last added one. Index 0 is the primary cursor. Returns the indices removed, in order.
pub fn normalize(selections: &mut Vec<Selection>, last_added: &mut usize) -> Vec<usize> {
    let mut removed = Vec::new();
    let mut sorted: Vec<usize> = (0..selections.len()).collect();
    sorted.sort_by_key(|&index| (selections[index].start(), selections[index].end()));
    let mut at = 0;
    while at + 1 < sorted.len() {
        let (current, next) = (selections[sorted[at]], selections[sorted[at + 1]]);
        let merge = match current.is_empty() || next.is_empty() {
            true => next.start() <= current.end(),
            false => next.start() < current.end(),
        };
        if !merge {
            at += 1;
            continue;
        }
        let (winner_at, loser_at) = if sorted[at] < sorted[at + 1] { (at, at + 1) } else { (at + 1, at) };
        let (winner, loser) = (sorted[winner_at], sorted[loser_at]);
        let (winner_selection, loser_selection) = (selections[winner], selections[loser]);
        if winner_selection != loser_selection {
            let start = winner_selection.start().min(loser_selection.start());
            let end = winner_selection.end().max(loser_selection.end());
            let left_to_right = if loser == *last_added {
                *last_added = winner;
                loser_selection.anchor <= loser_selection.head
            } else {
                winner_selection.anchor <= winner_selection.head
            };
            selections[winner] = if left_to_right { Selection { anchor: start, head: end } } else { Selection { anchor: end, head: start } };
        }
        selections.remove(loser);
        removed.push(loser);
        sorted.remove(loser_at);
        for index in &mut sorted {
            if *index > loser {
                *index -= 1;
            }
        }
        if *last_added >= loser {
            *last_added -= 1;
        }
    }
    removed
}

pub fn line_len(lines: &[String], line: usize) -> usize {
    lines[line].chars().count()
}

fn chars(lines: &[String], line: usize) -> Vec<char> {
    lines[line].chars().collect()
}

pub fn last_position(lines: &[String]) -> Position {
    let line = lines.len() - 1;
    Position::new(line, line_len(lines, line))
}

pub fn left(lines: &[String], position: Position) -> Position {
    match position {
        Position { column: 0, line: 0 } => position,
        Position { column: 0, line } => Position::new(line - 1, line_len(lines, line - 1)),
        Position { line, column } => Position::new(line, column - graphemes::prev_char_length(&chars(lines, line), column)),
    }
}

pub fn right(lines: &[String], position: Position) -> Position {
    let text = chars(lines, position.line);
    if position.column < text.len() {
        Position::new(position.line, position.column + graphemes::next_char_length(&text, position.column))
    } else if position.line + 1 < lines.len() {
        Position::new(position.line + 1, 0)
    } else {
        position
    }
}

/// `MoveOperations.vertical` on view lines: keeps the visible column, plus what the line could not hold
/// (`leftover`, negative after a jump to the end of the last line), and moves to the document's
/// edge when there is no line to go to. Lines in `hidden` are folded away and skipped.
pub fn vertical(lines: &[String], hidden: &[Range<usize>], position: Position, leftover: isize, down: bool, tab_size: usize) -> (Position, isize) {
    let visible_of = |position: Position| super::logical_visible_column(&lines[position.line], position.column, tab_size) as isize;
    let visible = visible_of(position) + leftover;
    let last_line = folding::last_shown_line(lines.len(), hidden);
    let last = Position::new(last_line, line_len(lines, last_line));
    let was_at_edge = if down { position == last } else { position == Position::default() };
    let moved = match folding::shown_neighbour(lines.len(), hidden, position.line, down) {
        None if down => last,
        None => Position::default(),
        Some(line) => Position::new(line, column_from_visible(&lines[line], visible.max(0) as usize, tab_size)),
    };
    let leftover = if was_at_edge { 0 } else { visible - visible_of(moved) };
    (moved, leftover)
}

/// `MoveOperations.translateUp`/`translateDown` for one position: `vertical`, but on the first or last
/// line the position stays (`allowMoveOnEdgeLine` false).
pub fn translate(lines: &[String], hidden: &[Range<usize>], position: Position, leftover: isize, down: bool, tab_size: usize) -> (Position, isize) {
    let last_line = folding::last_shown_line(lines.len(), hidden);
    let on_edge_line = if down { position.line == last_line } else { position.line == 0 };
    if !on_edge_line {
        return vertical(lines, hidden, position, leftover, down, tab_size);
    }
    let was_at_edge = if down { position == Position::new(last_line, line_len(lines, last_line)) } else { position == Position::default() };
    (position, if was_at_edge { 0 } else { leftover })
}

/// `CursorColumns.columnFromVisibleColumn`: the column nearest to `visible`, the left one on a tie.
pub fn column_from_visible(line: &str, visible: usize, tab_size: usize) -> usize {
    let mut before = 0;
    for (index, c) in line.chars().enumerate() {
        let after = if c == '\t' { before + tab_size - before % tab_size } else { before + 1 };
        if after >= visible {
            return if after - visible < visible - before { index + 1 } else { index };
        }
        before = after;
    }
    line.chars().count()
}

fn first_non_whitespace(line: &[char]) -> Option<usize> {
    line.iter().position(|&c| c != ' ' && c != '\t')
}

/// `moveToBeginningOfLine`: the first non-blank column, or column 0 when already there.
pub fn line_start(lines: &[String], position: Position) -> Position {
    let first = first_non_whitespace(&chars(lines, position.line)).unwrap_or(0);
    Position::new(position.line, if position.column == first { 0 } else { first })
}

pub fn line_end(lines: &[String], position: Position) -> Position {
    Position::new(position.line, line_len(lines, position.line))
}

/// `deleteLeft`: the selection, else back to the previous indent stop inside the indentation, else the
/// last emoji component or one char (`strings.getLeftDeleteOffset`).
pub fn delete_left_range(lines: &[String], selection: Selection, indentation: Indentation) -> (Position, Position) {
    if !selection.is_empty() {
        return (selection.start(), selection.end());
    }
    let position = selection.head;
    let text = &lines[position.line];
    let indent_end = first_non_whitespace(&text.chars().collect::<Vec<_>>()).unwrap_or(text.chars().count());
    if position.column > 0 && position.column <= indent_end {
        let from = super::visible_column(text, position.column, indentation.tab_size);
        let to = (from - 1) - (from - 1) % indentation.indent_size();
        return (Position::new(position.line, column_from_visible(text, to, indentation.tab_size)), position);
    }
    if position.column == 0 {
        return (left(lines, position), position);
    }
    let column = graphemes::left_delete_offset(position.column, &chars(lines, position.line));
    (Position::new(position.line, column), position)
}

pub fn delete_right_range(lines: &[String], selection: Selection) -> (Position, Position) {
    if !selection.is_empty() {
        return (selection.start(), selection.end());
    }
    (selection.head, right(lines, selection.head))
}

/// `deleteAllLeft`: to the line start, or the line break before it at column 0; a selection widens to its first line start.
pub fn delete_all_left_range(lines: &[String], selection: Selection) -> (Position, Position) {
    let (start, end) = (selection.start(), selection.end());
    if !selection.is_empty() {
        return (Position::new(start.line, 0), end);
    }
    if start.column == 0 {
        return (left(lines, start), start);
    }
    (Position::new(start.line, 0), start)
}

/// `deleteAllRight`: to the line end, or the line break after it at the line end.
pub fn delete_all_right_range(lines: &[String], selection: Selection) -> (Position, Position) {
    if !selection.is_empty() {
        return (selection.start(), selection.end());
    }
    let position = selection.head;
    if position.column == line_len(lines, position.line) {
        return (position, right(lines, position));
    }
    (position, line_end(lines, position))
}

/// `normalizeIndentation`: leading whitespace rewritten in the editor's indent unit.
pub fn normalize_indentation(text: &str, indentation: Indentation) -> String {
    let rest = text.trim_start_matches([' ', '\t']);
    let indent = &text[..text.len() - rest.len()];
    let size = indentation.indent_size();
    let width = super::visible_column(indent, indent.chars().count(), size);
    if indentation.insert_spaces {
        return " ".repeat(width) + rest;
    }
    "\t".repeat(width / size) + &" ".repeat(width % size) + rest
}

/// `_replaceJumpToNextIndent`: spaces up to the next indent stop, or a tab.
pub fn tab_text(line: &str, column: usize, indentation: Indentation) -> String {
    if !indentation.insert_spaces {
        return "\t".into();
    }
    let visible = super::visible_column(line, column, indentation.tab_size);
    " ".repeat(indentation.indent_size() - visible % indentation.indent_size())
}

/// `ShiftCommand.shiftIndent`/`unshiftIndent`: the indentation of a line whose content starts at
/// visible column `visible`, moved one stop right or left.
pub fn shifted_indent(visible: usize, indentation: Indentation, unshift: bool) -> String {
    let size = if indentation.insert_spaces { indentation.indent_size() } else { indentation.tab_size };
    let stop = match unshift {
        true if visible == 0 => 0,
        true => (visible - 1) - (visible - 1) % size,
        false => visible + size - visible % size,
    };
    if indentation.insert_spaces { " ".repeat(stop) } else { "\t".repeat(stop / size) }
}

/// The lines a shift (Tab or Shift+Tab over a selection) touches: a selection ending at column 0 leaves its last line out.
pub fn shift_lines(selection: Selection) -> std::ops::RangeInclusive<usize> {
    let (start, end) = (selection.start(), selection.end());
    let last = if end.column == 0 && end.line > start.line { end.line - 1 } else { end.line };
    start.line..=last
}

/// The indentation edit of one line for a shift, or `None` when the line keeps its indentation. Up to
/// `extra_spaces` spaces ending the indentation stay as content.
pub fn shift_line_edit(line: &str, indentation: Indentation, unshift: bool, indent_empty: bool, extra_spaces: usize) -> Option<(usize, String)> {
    let text: Vec<char> = line.chars().collect();
    let mut indent_end = first_non_whitespace(&text).unwrap_or(text.len());
    if unshift && indent_end == 0 {
        return None;
    }
    if !unshift && text.is_empty() && !indent_empty {
        return None;
    }
    for _ in 0..extra_spaces {
        if indent_end == 0 || text[indent_end - 1] != ' ' {
            break;
        }
        indent_end -= 1;
    }
    if unshift && indent_end == 0 {
        return None;
    }
    let visible = super::visible_column(line, indent_end, indentation.tab_size);
    Some((indent_end, shifted_indent(visible, indentation, unshift)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(String::from).collect()
    }

    fn at(line: usize, column: usize) -> Position {
        Position::new(line, column)
    }

    const SPACES_4: Indentation = Indentation { tab_size: 4, indent_size: None, insert_spaces: true };

    #[test]
    fn normalize_merges_overlapping_cursors_into_the_older_one() {
        let range = |anchor: (usize, usize), head: (usize, usize)| Selection { anchor: at(anchor.0, anchor.1), head: at(head.0, head.1) };
        // Touching selections stay apart; a cursor touching a selection merges into it.
        let mut selections = vec![range((0, 0), (0, 2)), range((0, 2), (0, 4)), Selection::caret(at(0, 4)), Selection::caret(at(1, 0))];
        let mut last_added = 3;
        normalize(&mut selections, &mut last_added);
        assert_eq!(selections, [range((0, 0), (0, 2)), range((0, 2), (0, 4)), Selection::caret(at(1, 0))]);
        assert_eq!(last_added, 2);
        // The older cursor keeps its direction, unless the merged one was added last.
        let mut selections = vec![range((0, 3), (0, 0)), range((0, 2), (0, 5))];
        let mut last_added = 0;
        normalize(&mut selections, &mut last_added);
        assert_eq!(selections, [range((0, 5), (0, 0))]);
        let mut selections = vec![range((0, 3), (0, 0)), range((0, 2), (0, 5))];
        let mut last_added = 1;
        normalize(&mut selections, &mut last_added);
        assert_eq!((selections, last_added), (vec![range((0, 0), (0, 5))], 0));
    }

    #[test]
    fn moves_by_char_across_line_breaks() {
        let text = lines("ab\ncd");
        assert_eq!(left(&text, at(1, 0)), at(0, 2));
        assert_eq!(right(&text, at(0, 2)), at(1, 0));
        assert_eq!(right(&text, at(1, 2)), at(1, 2));
        assert_eq!(left(&text, at(0, 0)), at(0, 0));
    }

    #[test]
    fn moves_step_over_grapheme_clusters() {
        // cursor.test.ts: 'move left/right with surrogate pair', 'grapheme breaking'
        let text = lines("    Third Line🐶\na\u{303}a\u{303}a\u{303}\n辻\u{E0100}辻\u{E0100}\nப\u{BC1}");
        assert_eq!(left(&text, at(0, 15)), at(0, 14));
        assert_eq!(right(&text, at(0, 14)), at(0, 15));
        for line in 1..=3 {
            assert_eq!(right(&text, at(line, 0)), at(line, 2));
            assert_eq!(left(&text, at(line, 2)), at(line, 0));
        }
    }

    #[test]
    fn deletes_step_over_emoji_components_left_and_grapheme_clusters_right() {
        // cursor.test.ts: 'issue #99629: Emoji modifiers in text treated separately when using backspace'
        let text = lines("👶🏾\n👨‍👩🏽‍👧‍👦\ne\u{301}x");
        let delete_left = |position| delete_left_range(&text, Selection::caret(position), SPACES_4).0;
        assert_eq!(delete_left(at(0, 2)), at(0, 0));
        assert_eq!(delete_left(at(1, 8)), at(1, 6));
        assert_eq!(delete_left(at(1, 6)), at(1, 4));
        assert_eq!(delete_left(at(1, 4)), at(1, 1));
        assert_eq!(delete_left(at(1, 1)), at(1, 0));
        assert_eq!(delete_left(at(2, 2)), at(2, 1));
        assert_eq!(delete_left(at(2, 0)), at(1, 8));
        assert_eq!(delete_right_range(&text, Selection::caret(at(2, 0))), (at(2, 0), at(2, 2)));
    }

    #[test]
    fn vertical_moves_keep_the_visible_column_through_short_lines_and_tabs() {
        let text = lines("abcdef\nab\n\tx\nabcdef");
        let (position, leftover) = vertical(&text, &[], at(0, 5), 0, true, 4);
        assert_eq!((position, leftover), (at(1, 2), 3));
        let (position, leftover) = vertical(&text, &[], position, leftover, true, 4);
        // Visible column 5 lies inside "x" after the tab (4..5), so the cursor lands after it.
        assert_eq!((position, leftover), (at(2, 2), 0));
        assert_eq!(vertical(&text, &[], at(3, 6), 0, true, 4), (at(3, 6), 0));
        // Past the first or last line the cursor goes to the edge and comes back to its column.
        assert_eq!(vertical(&text, &[], at(0, 3), 0, false, 4), (at(0, 0), 3));
        assert_eq!(vertical(&text, &[], at(0, 0), 3, true, 4), (at(1, 2), 1));
        assert_eq!(vertical(&text, &[], at(3, 2), 0, true, 4), (at(3, 6), -4));
        assert_eq!(vertical(&text, &[], at(3, 6), -4, false, 4).0, at(2, 0));
    }

    #[test]
    fn vertical_moves_skip_folded_lines_and_stop_at_the_last_shown_line() {
        let text = lines("abc\n  d\n  e\nfgh\n  i");
        assert_eq!(vertical(&text, &[1..3], at(0, 2), 0, true, 4), (at(3, 2), 0));
        assert_eq!(vertical(&text, &[1..3], at(3, 1), 0, false, 4), (at(0, 1), 0));
        assert_eq!(vertical(&text, &[4..5], at(3, 1), 0, true, 4), (at(3, 3), -2));
    }

    #[test]
    fn home_toggles_between_first_non_blank_and_column_zero() {
        let text = lines("    let x");
        assert_eq!(line_start(&text, at(0, 7)), at(0, 4));
        assert_eq!(line_start(&text, at(0, 4)), at(0, 0));
        assert_eq!(line_start(&text, at(0, 0)), at(0, 4));
        assert_eq!(line_end(&text, at(0, 0)), at(0, 9));
    }

    #[test]
    fn delete_left_unindents_inside_the_indentation() {
        let text = lines("      x\n\tx");
        assert_eq!(delete_left_range(&text, Selection::caret(at(0, 6)), SPACES_4), (at(0, 4), at(0, 6)));
        assert_eq!(delete_left_range(&text, Selection::caret(at(0, 4)), SPACES_4), (at(0, 0), at(0, 4)));
        assert_eq!(delete_left_range(&text, Selection::caret(at(0, 7)), SPACES_4), (at(0, 6), at(0, 7)));
        assert_eq!(delete_left_range(&text, Selection::caret(at(1, 0)), SPACES_4), (at(0, 7), at(1, 0)));
        assert_eq!(delete_left_range(&text, Selection::caret(at(1, 1)), SPACES_4), (at(1, 0), at(1, 1)));
    }

    #[test]
    fn delete_all_left_and_right_reach_the_line_edges() {
        let text = lines("abc\ndef");
        assert_eq!(delete_all_left_range(&text, Selection::caret(at(1, 2))), (at(1, 0), at(1, 2)));
        assert_eq!(delete_all_left_range(&text, Selection::caret(at(1, 0))), (at(0, 3), at(1, 0)));
        assert_eq!(delete_all_right_range(&text, Selection::caret(at(0, 1))), (at(0, 1), at(0, 3)));
        assert_eq!(delete_all_right_range(&text, Selection::caret(at(0, 3))), (at(0, 3), at(1, 0)));
    }

    #[test]
    fn shifts_indent_to_the_next_or_previous_stop() {
        let tabs = Indentation { tab_size: 4, indent_size: None, insert_spaces: false };
        assert_eq!(shift_line_edit("  x", SPACES_4, false, false, 0), Some((2, "    ".into())));
        assert_eq!(shift_line_edit("      x", SPACES_4, true, false, 0), Some((6, "    ".into())));
        assert_eq!(shift_line_edit("x", SPACES_4, true, false, 0), None);
        assert_eq!(shift_line_edit("", SPACES_4, false, false, 0), None);
        assert_eq!(shift_line_edit("\tx", tabs, false, false, 0), Some((1, "\t\t".into())));
        assert_eq!(tab_text("ab", 2, SPACES_4), "  ");
        assert_eq!(normalize_indentation("\t  ", SPACES_4), "      ");
        assert_eq!(normalize_indentation("      ", tabs), "\t  ");
        assert_eq!(shift_lines(Selection { anchor: at(1, 0), head: at(3, 0) }), 1..=2);
    }
}
