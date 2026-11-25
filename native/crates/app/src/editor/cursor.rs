//! Where cursor commands move and what delete commands remove, as cursorMoveOperations.ts,
//! cursorWordOperations.ts and cursorDeleteOperations.ts compute it. Positions are 0-based lines and
//! char columns; upstream counts 1-based UTF-16 columns.
//! shortcut: columns step over chars, not grapheme clusters; a ZWJ emoji takes several steps. Port
//! strings.getLeftDeleteOffset and the grapheme iterator when that matters.

use std::ops::Range;

use super::{Indentation, WORD_SEPARATORS, folding};

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
        Position { line, column } => Position::new(line, column - 1),
    }
}

pub fn right(lines: &[String], position: Position) -> Position {
    if position.column < line_len(lines, position.line) {
        Position::new(position.line, position.column + 1)
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

#[derive(Clone, Copy, Debug, PartialEq)]
enum CharClass {
    Regular,
    Whitespace,
    Separator,
}

/// `WordCharacterClassifier` with the default `editor.wordSeparators`: only space and tab are whitespace.
fn class(c: char, separators: &str) -> CharClass {
    match c {
        ' ' | '\t' => CharClass::Whitespace,
        c if separators.contains(c) => CharClass::Separator,
        _ => CharClass::Regular,
    }
}

/// A run of regular chars or of separators; `next` is the class of the char that ended it.
#[derive(Clone, Copy, Debug)]
struct Word {
    start: usize,
    end: usize,
    kind: CharClass,
    next: CharClass,
}

/// A word of `kind` ends at whitespace or at a char of the other kind.
fn ends_word(kind: CharClass, class: CharClass) -> bool {
    class == CharClass::Whitespace || (class != kind && kind != CharClass::Whitespace)
}

/// `_doFindPreviousWordOnLine`: the word that ends before `column`.
fn previous_word(line: &[char], column: usize, separators: &str) -> Option<Word> {
    let mut kind = CharClass::Whitespace;
    for index in (0..column).rev() {
        let class = class(line[index], separators);
        if kind != CharClass::Whitespace && ends_word(kind, class) {
            return Some(Word { start: index + 1, end: end_of_word(line, kind, index + 1, separators), kind, next: class });
        }
        if class != CharClass::Whitespace {
            kind = class;
        }
    }
    (kind != CharClass::Whitespace).then(|| Word { start: 0, end: end_of_word(line, kind, 0, separators), kind, next: CharClass::Whitespace })
}

fn end_of_word(line: &[char], kind: CharClass, start: usize, separators: &str) -> usize {
    (start..line.len()).find(|&index| ends_word(kind, class(line[index], separators))).unwrap_or(line.len())
}

/// `_doFindNextWordOnLine`: the word that starts at or after `column`.
fn next_word(line: &[char], column: usize, separators: &str) -> Option<Word> {
    let mut kind = CharClass::Whitespace;
    for (index, &c) in line.iter().enumerate().skip(column) {
        let class = class(c, separators);
        if kind != CharClass::Whitespace && ends_word(kind, class) {
            return Some(Word { start: start_of_word(line, kind, index, separators), end: index, kind, next: class });
        }
        if class != CharClass::Whitespace {
            kind = class;
        }
    }
    (kind != CharClass::Whitespace).then(|| Word { start: start_of_word(line, kind, line.len(), separators), end: line.len(), kind, next: CharClass::Whitespace })
}

/// The start of the word that ends at `end`.
fn start_of_word(line: &[char], kind: CharClass, end: usize, separators: &str) -> usize {
    (0..end).rev().find(|&index| ends_word(kind, class(line[index], separators))).map_or(0, |index| index + 1)
}

/// A lone separator before a regular char ("." in "a.b") is skipped by word moves.
fn is_lone_separator(word: &Word) -> bool {
    word.kind == CharClass::Separator && word.end - word.start == 1 && word.next == CharClass::Regular
}

/// `cursorWordLeft` (WordStartFast): to the start of the previous word, from the end of the previous line at column 0.
pub fn word_left(lines: &[String], position: Position) -> Position {
    let Position { mut line, mut column } = position;
    if column == 0 && line > 0 {
        line -= 1;
        column = line_len(lines, line);
    }
    let text = chars(lines, line);
    let mut word = previous_word(&text, column, WORD_SEPARATORS);
    if let Some(found) = word.filter(is_lone_separator) {
        word = previous_word(&text, found.start, WORD_SEPARATORS);
    }
    Position::new(line, word.map_or(0, |word| word.start))
}

/// `cursorWordRight` (WordEnd): to the end of the next word, from the start of the next line at the line end.
pub fn word_right(lines: &[String], position: Position) -> Position {
    word_end_right(lines, position, WORD_SEPARATORS)
}

fn word_end_right(lines: &[String], position: Position, separators: &str) -> Position {
    let Position { mut line, mut column } = position;
    if column == line_len(lines, line) && line + 1 < lines.len() {
        line += 1;
        column = 0;
    }
    let text = chars(lines, line);
    let mut word = next_word(&text, column, separators);
    if let Some(found) = word.filter(is_lone_separator) {
        word = next_word(&text, found.end, separators);
    }
    Position::new(line, word.map_or(text.len(), |word| word.end))
}

/// `moveWordLeft` with `WordStart` (word ends at the start of the previous word) or `WordEnd` (at the end
/// of the previous word), from the end of the previous line at column 0.
fn word_stop_left(lines: &[String], position: Position, at_end: bool, separators: &str) -> Position {
    let Position { mut line, mut column } = position;
    if column == 0 && line > 0 {
        line -= 1;
        column = line_len(lines, line);
    }
    let text = chars(lines, line);
    let mut word = previous_word(&text, column, separators);
    if !at_end {
        return Position::new(line, word.map_or(0, |word| word.start));
    }
    if let Some(found) = word.filter(|word| column <= word.end) {
        word = previous_word(&text, found.start, separators);
    }
    Position::new(line, word.map_or(0, |word| word.end))
}

/// `moveWordRight` with `WordStart`: to the start of the next word, from the start of the next line at the line end.
fn word_start_right(lines: &[String], position: Position, separators: &str) -> Position {
    let Position { mut line, mut column } = position;
    let moved_down = column == line_len(lines, line) && line + 1 < lines.len();
    if moved_down {
        line += 1;
        column = 0;
    }
    let text = chars(lines, line);
    let mut word = next_word(&text, column, separators);
    if let Some(found) = word.filter(|word| !moved_down && column >= word.start) {
        word = next_word(&text, found.end, separators);
    }
    Position::new(line, word.map_or(text.len(), |word| word.start))
}

/// `_moveWordPartLeft`: back to the previous snake_case, kebab-case or camelCase boundary.
fn word_part_boundary_left(lines: &[String], position: Position) -> Position {
    let Position { line, column } = position;
    if column == 0 {
        return if line > 0 { Position::new(line - 1, line_len(lines, line - 1)) } else { position };
    }
    let text = chars(lines, line);
    let boundary = (1..column).rev().find(|&index| is_word_part_start(&text, index, false));
    Position::new(line, boundary.unwrap_or(0))
}

/// `_moveWordPartRight`: on to the next snake_case, kebab-case or camelCase boundary.
fn word_part_boundary_right(lines: &[String], position: Position) -> Position {
    let Position { line, column } = position;
    let len = line_len(lines, line);
    if column == len {
        return if line + 1 < lines.len() { Position::new(line + 1, 0) } else { position };
    }
    let text = chars(lines, line);
    let boundary = (column + 1..len).find(|&index| is_word_part_start(&text, index, true));
    Position::new(line, boundary.unwrap_or(len))
}

/// Whether a word part starts between `text[index - 1]` and `text[index]`. Moving right stops before
/// `_` and `-`, moving left after them.
fn is_word_part_start(text: &[char], index: usize, right: bool) -> bool {
    let (left, here) = (text[index - 1], text[index]);
    let snake_or_kebab = match right {
        true => (left != '_' && here == '_') || (left != '-' && here == '-'),
        false => (left == '_' && here != '_') || (left == '-' && here != '-'),
    };
    let camel = (left.is_ascii_lowercase() || left.is_ascii_digit()) && here.is_ascii_uppercase();
    // thisIsACamelCaseWithOneLetterWords
    let one_letter = left.is_ascii_uppercase() && here.is_ascii_uppercase() && text.get(index + 1).is_some_and(|next| next.is_ascii_lowercase() || next.is_ascii_digit());
    snake_or_kebab || camel || one_letter
}

/// `WordPartOperations.moveWordPartLeft`: the nearest of the previous word start, word end and word part boundary.
pub fn word_part_left(lines: &[String], position: Position, separators: &str) -> Position {
    let candidates = [word_stop_left(lines, position, false, separators), word_stop_left(lines, position, true, separators), word_part_boundary_left(lines, position)];
    candidates.into_iter().max().expect("three candidates")
}

/// `WordPartOperations.moveWordPartRight`: the nearest of the next word start, word end and word part boundary.
pub fn word_part_right(lines: &[String], position: Position, separators: &str) -> Position {
    let candidates = [word_start_right(lines, position, separators), word_end_right(lines, position, separators), word_part_boundary_right(lines, position)];
    candidates.into_iter().min().expect("three candidates")
}

/// `deleteWordLeft` with `WordEnd` and the whitespace heuristics: back to the end of the previous word.
fn delete_word_end_left_range(lines: &[String], selection: Selection, separators: &str) -> (Position, Position) {
    if !selection.is_empty() {
        return (selection.start(), selection.end());
    }
    let position = selection.head;
    let text = chars(lines, position.line);
    let blanks = text[..position.column].iter().rev().take_while(|&&c| c == ' ' || c == '\t').count();
    if blanks >= 2 {
        return (Position::new(position.line, position.column - blanks), position);
    }
    let mut word = previous_word(&text, position.column, separators);
    if let Some(found) = word.filter(|word| position.column <= word.end) {
        word = previous_word(&text, found.start, separators);
    }
    let start = match word {
        Some(word) => Position::new(position.line, word.end),
        None if position.column > 0 => Position::new(position.line, 0),
        None => left(lines, position),
    };
    (start, position)
}

/// `deleteWordRight` with `WordStart` and the whitespace heuristics: on to the start of the next word.
fn delete_word_start_right_range(lines: &[String], selection: Selection, separators: &str) -> (Position, Position) {
    if !selection.is_empty() {
        return (selection.start(), selection.end());
    }
    let position = selection.head;
    let text = chars(lines, position.line);
    let blanks = text[position.column..].iter().take_while(|&&c| c == ' ' || c == '\t').count();
    if blanks > 0 {
        return (position, Position::new(position.line, position.column + blanks));
    }
    let mut word = next_word(&text, position.column, separators);
    if let Some(found) = word.filter(|word| position.column >= word.start) {
        word = next_word(&text, found.end, separators);
    }
    let end = match word {
        Some(word) => Position::new(position.line, word.start),
        None if position.column < text.len() || position.line + 1 == lines.len() => Position::new(position.line, text.len()),
        None => {
            let next = chars(lines, position.line + 1);
            Position::new(position.line + 1, next_word(&next, 0, separators).map_or(next.len(), |word| word.start))
        }
    };
    (position, end)
}

/// `WordPartOperations.deleteWordPartLeft`: of the word start, word end and word part deletes, the one
/// with the last end (`compareRangesUsingEnds`), the nearest start among those.
pub fn delete_word_part_left_range(lines: &[String], selection: Selection, separators: &str) -> (Position, Position) {
    let part = match selection.is_empty() {
        true => (word_part_boundary_left(lines, selection.head), selection.head),
        false => (selection.start(), selection.end()),
    };
    let mut candidates = [delete_word_start_left_range(lines, selection, separators), delete_word_end_left_range(lines, selection, separators), part];
    candidates.sort_by_key(|&(start, end)| (end, start));
    candidates[2]
}

/// `WordPartOperations.deleteWordPartRight`: of the word start, word end and word part deletes, the one
/// with the first start (`compareRangesUsingStarts`), the nearest end among those.
pub fn delete_word_part_right_range(lines: &[String], selection: Selection, separators: &str) -> (Position, Position) {
    let part = match selection.is_empty() {
        true => (selection.head, word_part_boundary_right(lines, selection.head)),
        false => (selection.start(), selection.end()),
    };
    let mut candidates = [delete_word_start_right_range(lines, selection, separators), delete_word_end_right_range(lines, selection, separators), part];
    candidates.sort();
    candidates[0]
}

/// `deleteLeft`: the selection, else one char, else back to the previous indent stop inside the indentation.
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
    (left(lines, position), position)
}

pub fn delete_right_range(lines: &[String], selection: Selection) -> (Position, Position) {
    if !selection.is_empty() {
        return (selection.start(), selection.end());
    }
    (selection.head, right(lines, selection.head))
}

/// `deleteWordLeft` (WordStart, whitespace heuristics): two or more blanks before the cursor go
/// alone, else back to the start of the previous word.
pub fn delete_word_left_range(lines: &[String], selection: Selection) -> (Position, Position) {
    delete_word_start_left_range(lines, selection, WORD_SEPARATORS)
}

fn delete_word_start_left_range(lines: &[String], selection: Selection, separators: &str) -> (Position, Position) {
    if !selection.is_empty() {
        return (selection.start(), selection.end());
    }
    let position = selection.head;
    let text = chars(lines, position.line);
    let blanks = text[..position.column].iter().rev().take_while(|&&c| c == ' ' || c == '\t').count();
    if blanks >= 2 {
        return (Position::new(position.line, position.column - blanks), position);
    }
    let start = match previous_word(&text, position.column, separators) {
        Some(word) => Position::new(position.line, word.start),
        None if position.column > 0 => Position::new(position.line, 0),
        None => left(lines, position),
    };
    (start, position)
}

/// `deleteWordRight` (WordEnd, whitespace heuristics): blanks after the cursor go alone, else to
/// the end of the next word; at the line end, to the start of the first word of the next line.
pub fn delete_word_right_range(lines: &[String], selection: Selection) -> (Position, Position) {
    delete_word_end_right_range(lines, selection, WORD_SEPARATORS)
}

fn delete_word_end_right_range(lines: &[String], selection: Selection, separators: &str) -> (Position, Position) {
    if !selection.is_empty() {
        return (selection.start(), selection.end());
    }
    let position = selection.head;
    let text = chars(lines, position.line);
    let blanks = text[position.column..].iter().take_while(|&&c| c == ' ' || c == '\t').count();
    if blanks > 0 {
        return (position, Position::new(position.line, position.column + blanks));
    }
    let end = match next_word(&text, position.column, separators) {
        Some(word) => Position::new(position.line, word.end),
        None if position.column < text.len() || position.line + 1 == lines.len() => Position::new(position.line, text.len()),
        None => {
            let next = chars(lines, position.line + 1);
            Position::new(position.line + 1, next_word(&next, 0, separators).map_or(next.len(), |word| word.start))
        }
    };
    (position, end)
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
    fn word_moves_stop_like_upstream() {
        let text = lines("const fooBar = this.value;\n  next");
        assert_eq!(word_right(&text, at(0, 0)), at(0, 5));
        assert_eq!(word_right(&text, at(0, 5)), at(0, 12));
        assert_eq!(word_right(&text, at(0, 12)), at(0, 14));
        // A lone "." before a word is skipped: this.value moves to the end of "value".
        assert_eq!(word_right(&text, at(0, 19)), at(0, 25));
        assert_eq!(word_right(&text, at(0, 26)), at(1, 6));
        assert_eq!(word_left(&text, at(0, 25)), at(0, 20));
        assert_eq!(word_left(&text, at(0, 20)), at(0, 15));
        assert_eq!(word_left(&text, at(0, 15)), at(0, 13));
        // ";" after a word is a lone separator too, so the move goes on to the start of "value".
        assert_eq!(word_left(&text, at(1, 0)), at(0, 20));
    }

    /// wordTestUtils.ts `deserializePipePositions`: the text without `|` and the positions they marked.
    fn without_pipes(expected: &str) -> Vec<String> {
        lines(&expected.replace('|', ""))
    }

    /// wordTestUtils.ts `serializePipePositions`.
    fn with_pipes(text: &[String], mut stops: Vec<Position>) -> String {
        stops.sort();
        let mut out = Vec::new();
        for (line, content) in text.iter().enumerate() {
            let mut piped = String::new();
            for (column, c) in content.chars().chain(std::iter::once('\0')).enumerate() {
                if stops.contains(&at(line, column)) {
                    piped.push('|');
                }
                if c != '\0' {
                    piped.push(c);
                }
            }
            out.push(piped);
        }
        out.join("\n")
    }

    /// wordTestUtils.ts `testRepeatedActionAndExtractPositions` for a cursor move, until `last`.
    fn move_stops(expected: &str, from: Position, last: Position, separators: &str, step: fn(&[String], Position, &str) -> Position) -> String {
        let text = without_pipes(expected);
        let mut position = from;
        let mut stops = Vec::new();
        while position != last || stops.is_empty() {
            position = step(&text, position, separators);
            stops.push(position);
            assert!(stops.len() < 1000, "endless loop at {position:?}");
        }
        with_pipes(&text, stops)
    }

    /// The same for a one-line delete: each stop is where the deleted text began in the original line.
    fn delete_stops(expected: &str, right: bool, separators: &str, range: fn(&[String], Selection, &str) -> (Position, Position)) -> String {
        let original = without_pipes(expected);
        let mut text = original.clone();
        let mut position = if right { at(0, 0) } else { at(0, line_len(&text, 0)) };
        let mut stops = Vec::new();
        while !text[0].is_empty() {
            let (start, end) = range(&text, Selection::caret(position), separators);
            let mut chars: Vec<char> = text[0].chars().collect();
            chars.drain(start.column..end.column);
            text[0] = chars.into_iter().collect();
            position = start;
            let original_len = line_len(&original, 0);
            stops.push(if right { at(0, original_len - line_len(&text, 0)) } else { position });
            assert!(stops.len() < 1000, "endless loop at {position:?}");
        }
        with_pipes(&original, stops)
    }

    /// wordPartOperations.test.ts; the #158667 cases (the last of each list) leave `-` out of the separators.
    #[test]
    fn word_part_moves_and_deletes_stop_like_upstream() {
        let kebab_separators = "!\"#&'()*+,./:;<=>?@[\\]^`{|}\u{b7}";
        let separators = |expected: &str| if expected.contains("kebab") { kebab_separators } else { WORD_SEPARATORS };
        let end = |expected: &str| {
            let text = without_pipes(expected);
            at(text.len() - 1, line_len(&text, text.len() - 1))
        };
        let left_cases = [
            "|start| |line|\n|this|Is|A|Camel|Case|Var|  |this_|is_|a_|snake_|case_|var| |THIS_|IS_|CAPS_|SNAKE| |this_|IS|Mixed|Use|\n|end| |line",
            "|myvar| |=| |'|demonstration|     |of| |selection| |with| |space|'",
            "|myvar| |=| |'|demonstration_____|of| |selection| |with| |space|'",
            "|foo_|bar",
            "|this-|is-|a-|kebab-|case-|var| |THIS-|IS-|CAPS-|KEBAB| |this-|IS|Mixed|Use",
        ];
        for expected in left_cases {
            assert_eq!(move_stops(expected, end(expected), at(0, 0), separators(expected), word_part_left), expected);
        }
        let right_cases = [
            "start| |line|\n|this|Is|A|Camel|Case|Var|  |this|_is|_a|_snake|_case|_var| |THIS|_IS|_CAPS|_SNAKE| |this|_IS|Mixed|Use|\n|end| |line|",
            "myvar| |=| |'|demonstration|     |of| |selection| |with| |space|'|",
            "myvar| |=| |'|demonstration|_____of| |selection| |with| |space|'|",
            ";| |--| |1|\n|;|        |--| |2|\n|;|    |#|3|\n|;|   |#|4|",
            "foo|_bar|",
            "this|-is|-a|-kebab|-case|-var| |THIS|-IS|-CAPS|-KEBAB| |this|-IS|Mixed|Use|",
        ];
        for expected in right_cases {
            assert_eq!(move_stops(expected, at(0, 0), end(expected), separators(expected), word_part_right), expected);
        }
        let delete_left = [
            "|   |/*| |Just| |some| |text| |a|+=| |3| |+|5|-|3| |*/|  |this|Is|A|Camel|Case|Var|  |this_|is_|a_|snake_|case_|var| |THIS_|IS_|CAPS_|SNAKE| |this_|IS|Mixed|Use",
            "|this-|is-|a-|kebab-|case-|var| |THIS-|IS-|CAPS-|KEBAB| |this-|IS|Mixed|Use",
        ];
        for expected in delete_left {
            assert_eq!(delete_stops(expected, false, separators(expected), delete_word_part_left_range), expected);
        }
        let delete_right = [
            "   |/*| |Just| |some| |text| |a|+=| |3| |+|5|-|3| |*/|  |this|Is|A|Camel|Case|Var|  |this|_is|_a|_snake|_case|_var| |THIS|_IS|_CAPS|_SNAKE| |this|_IS|Mixed|Use|",
            "this|-is|-a|-kebab|-case|-var| |THIS|-IS|-CAPS|-KEBAB| |this|-IS|Mixed|Use|",
        ];
        for expected in delete_right {
            assert_eq!(delete_stops(expected, true, separators(expected), delete_word_part_right_range), expected);
        }
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
    fn word_deletes_use_the_whitespace_heuristics() {
        let text = lines("let a =   b;\nnext word");
        assert_eq!(delete_word_left_range(&text, Selection::caret(at(0, 10))), (at(0, 7), at(0, 10)));
        assert_eq!(delete_word_left_range(&text, Selection::caret(at(0, 5))), (at(0, 4), at(0, 5)));
        assert_eq!(delete_word_left_range(&text, Selection::caret(at(0, 4))), (at(0, 0), at(0, 4)));
        assert_eq!(delete_word_left_range(&text, Selection::caret(at(1, 0))), (at(0, 12), at(1, 0)));
        assert_eq!(delete_word_right_range(&text, Selection::caret(at(0, 7))), (at(0, 7), at(0, 10)));
        assert_eq!(delete_word_right_range(&text, Selection::caret(at(0, 0))), (at(0, 0), at(0, 3)));
        assert_eq!(delete_word_right_range(&text, Selection::caret(at(0, 12))), (at(0, 12), at(1, 0)));
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
