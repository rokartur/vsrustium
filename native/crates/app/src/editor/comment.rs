//! The comment actions of editor/contrib/comment/browser: comment.ts, lineCommentCommand.ts and
//! blockCommentCommand.ts. Columns count chars where upstream counts UTF-16 code units.

use super::edit::{Command, command};
use super::{ContentChanged, OpenEditor, Position, Replace, Selection};
use crate::languages::{Comments, Languages};

/// lineCommentCommand.ts `Type`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineCommentKind {
    Toggle,
    ForceAdd,
    ForceRemove,
}

impl OpenEditor {
    /// `CommentLineAction.run`: a `LineCommentCommand` per selection between undo stops; of two selections
    /// sharing a line, the later added one leaves that line alone.
    pub fn comment_lines(&mut self, kind: LineCommentKind, insert_space: bool, ignore_empty_lines: bool, languages: &Languages) -> Option<ContentChanged> {
        let id = match kind {
            LineCommentKind::Toggle => "editor.action.commentLine",
            LineCommentKind::ForceAdd => "editor.action.addCommentLine",
            LineCommentKind::ForceRemove => "editor.action.removeCommentLine",
        };
        let indent_size = self.document().indentation.indent_size();
        let comments_at = |line: usize, column: usize| self.comments_at(languages, line, column);
        let commands = line_comment_commands(&self.document().lines, &self.selections, indent_size, kind, insert_space, ignore_empty_lines, &comments_at);
        self.push_undo_stop();
        let change = self.execute_commands(commands, command(id));
        self.push_undo_stop();
        change
    }

    /// `BlockCommentAction.run`: a `BlockCommentCommand` per selection between undo stops.
    pub fn block_comment(&mut self, insert_space: bool, languages: &Languages) -> Option<ContentChanged> {
        let comments_at = |line: usize, column: usize| self.comments_at(languages, line, column);
        let commands = self.selections.iter().map(|&selection| block_comment_command(&self.document().lines, selection, insert_space, &comments_at)).collect();
        self.push_undo_stop();
        let change = self.execute_commands(commands, command("editor.action.blockComment"));
        self.push_undo_stop();
        change
    }

    /// The comment tokens of the language at a position (`getLanguageIdAtPosition`), an embedded one included.
    // shortcut: the line is tokenized when needed, where upstream's `tokenizeIfCheap` skips long or far lines.
    fn comments_at<'a>(&self, languages: &'a Languages, line: usize, column: usize) -> Comments<'a> {
        let text = &self.document().lines[line];
        let offset = text.char_indices().nth(column).map_or(text.len(), |(offset, _)| offset);
        match self.document().tokens.language_at(&self.document().lines, line, offset) {
            Some(language) => languages.comments(&language),
            None => languages.comments(&self.document().language_id),
        }
    }
}

type CommentsAt<'a, 'b> = &'b dyn Fn(usize, usize) -> Comments<'a>;

fn line_comment_commands(lines: &[String], selections: &[Selection], indent_size: usize, kind: LineCommentKind, insert_space: bool, ignore_empty_lines: bool, comments_at: CommentsAt) -> Vec<Command> {
    let mut sorted: Vec<(Selection, usize, bool)> = selections.iter().enumerate().map(|(index, &selection)| (selection, index, false)).collect();
    sorted.sort_by_key(|(selection, ..)| (selection.start(), selection.end()));
    let mut previous = 0;
    for current in 1..sorted.len() {
        if sorted[previous].0.end().line != sorted[current].0.start().line {
            continue;
        }
        if sorted[previous].1 < sorted[current].1 {
            sorted[current].2 = true;
        } else {
            sorted[previous].2 = true;
            previous = current;
        }
    }
    // Upstream hands the commands over in this sorted order, so command `i` gets the cursor state of cursor `i`.
    sorted.into_iter().map(|(selection, _, ignore_first_line)| line_comment_command(lines, selection, indent_size, kind, insert_space, ignore_empty_lines, ignore_first_line, comments_at)).collect()
}

/// `ILinePreflightData`: whether the line is left alone, the comment token, where it goes or is, and how many
/// chars to delete when removing it.
#[derive(Clone, Debug, PartialEq)]
struct LinePreflight<'a> {
    ignore: bool,
    comment: &'a str,
    offset: usize,
    length: usize,
}

/// `LineCommentCommand.getEditOperations` and `computeCursorState`: line comments when the language at the
/// first line's start has a line comment token, else the block comment command on the whole lines.
#[allow(clippy::too_many_arguments)]
fn line_comment_command(lines: &[String], selection: Selection, indent_size: usize, kind: LineCommentKind, insert_space: bool, ignore_empty_lines: bool, ignore_first_line: bool, comments_at: CommentsAt) -> Command {
    let (start, end) = (selection.start(), selection.end());
    if start.line == end.line && ignore_first_line {
        let mut edits = Vec::new();
        if start.line + 1 < lines.len() {
            edits.push(Replace::new(Position::new(start.line, line_len(lines, start.line)), Position::new(start.line + 1, 0), "\n"));
        }
        return Command { edits, tracked: Some(selection), cursor_state: Box::new(|tracked, _| tracked.expect("tracked")) };
    }
    let mut s = selection;
    let move_end_down = start.line < end.line && end.column == 0;
    if move_end_down {
        s = with_end(s, Position::new(end.line - 1, line_len(lines, end.line - 1)));
    }
    let comments = comments_at(s.start().line, 0);
    let Some(token) = comments.line else {
        return block_fallback(lines, s, insert_space, move_end_down, comments);
    };
    let mut preflight = vec![LinePreflight { ignore: false, comment: token, offset: 0, length: token.chars().count() }; s.end().line - s.start().line + 1];
    let should_remove = analyze_lines(kind, insert_space, lines, &mut preflight, s.start().line, ignore_empty_lines, ignore_first_line, comments.line_no_indent);
    let edits = match should_remove {
        true => remove_line_comment_edits(&preflight, s.start().line),
        false => {
            normalize_insertion_point(lines, &mut preflight, s.start().line, indent_size);
            add_line_comment_edits(&preflight, s.start().line, insert_space)
        }
    };
    let cursor = s.head;
    let mut delta_column = 0;
    for edit in &edits {
        if edit.start == edit.end && edit.start == cursor && line_len(lines, cursor.line) == cursor.column {
            delta_column = edit.text.chars().count();
        }
    }
    Command { edits, tracked: Some(s), cursor_state: tracked_cursor_state(move_end_down, delta_column) }
}

/// `computeCursorState` of the line comment command: the tracked selection, its end moved to the next line's
/// start when the selection ended there, both columns moved by `delta_column`.
fn tracked_cursor_state(move_end_down: bool, delta_column: usize) -> Box<dyn FnOnce(Option<Selection>, &[(Position, Position)]) -> Selection> {
    Box::new(move |tracked, _| {
        let mut result = tracked.expect("the line comment command tracks its selection");
        if move_end_down {
            result = with_end(result, Position::new(result.end().line + 1, 0));
        }
        Selection { anchor: Position::new(result.anchor.line, result.anchor.column + delta_column), head: Position::new(result.head.line, result.head.column + delta_column) }
    })
}

/// `_analyzeLines`: fills in which lines count and where their comment token goes; answers whether the
/// comments are removed (every counted line has one, for a toggle).
#[allow(clippy::too_many_arguments)]
fn analyze_lines(kind: LineCommentKind, insert_space: bool, lines: &[String], preflight: &mut [LinePreflight], start_line: usize, ignore_empty_lines: bool, ignore_first_line: bool, no_indent: bool) -> bool {
    let mut only_whitespace_lines = true;
    let mut should_remove = kind != LineCommentKind::ForceAdd;
    for (index, data) in preflight.iter_mut().enumerate() {
        let line = start_line + index;
        if index == 0 && ignore_first_line {
            data.ignore = true;
            continue;
        }
        let content: Vec<char> = lines[line].chars().collect();
        let Some(content_start) = first_non_whitespace_index(&content) else {
            data.ignore = ignore_empty_lines;
            data.offset = if no_indent { 0 } else { content.len() };
            continue;
        };
        only_whitespace_lines = false;
        let offset = if no_indent { 0 } else { content_start };
        data.ignore = false;
        data.offset = offset;
        if should_remove && !has_needle_at_offset(&content, data.comment, offset as isize) {
            match kind {
                LineCommentKind::Toggle => should_remove = false,
                LineCommentKind::ForceAdd => {}
                LineCommentKind::ForceRemove => data.ignore = true,
            }
        }
        if should_remove && insert_space {
            let comment_end = content_start + data.length;
            if comment_end < content.len() && content[comment_end] == ' ' {
                data.length += 1;
            }
        }
    }
    if kind == LineCommentKind::Toggle && only_whitespace_lines {
        should_remove = false;
        for data in preflight.iter_mut() {
            data.ignore = false;
        }
    }
    should_remove
}

fn remove_line_comment_edits(preflight: &[LinePreflight], start_line: usize) -> Vec<Replace> {
    let counted = preflight.iter().enumerate().filter(|(_, data)| !data.ignore);
    counted.map(|(index, data)| Replace::new(Position::new(start_line + index, data.offset), Position::new(start_line + index, data.offset + data.length), "")).collect()
}

fn add_line_comment_edits(preflight: &[LinePreflight], start_line: usize, insert_space: bool) -> Vec<Replace> {
    let after = if insert_space { " " } else { "" };
    let counted = preflight.iter().enumerate().filter(|(_, data)| !data.ignore);
    counted.map(|(index, data)| Replace::new(Position::new(start_line + index, data.offset), Position::new(start_line + index, data.offset), format!("{}{after}", data.comment))).collect()
}

/// `_normalizeInsertionPoint`: the tokens go in the same visible column, the smallest indentation rounded
/// down to a multiple of `indent_size`.
fn normalize_insertion_point(lines: &[String], preflight: &mut [LinePreflight], start_line: usize, indent_size: usize) {
    let next_visible_column = |column: usize, is_tab: bool| if is_tab { column + (indent_size - column % indent_size) } else { column + 1 };
    let mut min_visible_column = usize::MAX;
    for (index, data) in preflight.iter().enumerate() {
        if data.ignore {
            continue;
        }
        let mut visible_column = 0;
        for c in lines[start_line + index].chars().take(data.offset) {
            if visible_column >= min_visible_column {
                break;
            }
            visible_column = next_visible_column(visible_column, c == '\t');
        }
        min_visible_column = min_visible_column.min(visible_column);
    }
    if min_visible_column == usize::MAX {
        return;
    }
    min_visible_column = min_visible_column / indent_size * indent_size;
    for (index, data) in preflight.iter_mut().enumerate() {
        if data.ignore {
            continue;
        }
        let content: Vec<char> = lines[start_line + index].chars().collect();
        let mut visible_column = 0;
        let mut j = 0;
        while visible_column < min_visible_column && j < data.offset {
            visible_column = next_visible_column(visible_column, content[j] == '\t');
            j += 1;
        }
        data.offset = if visible_column > min_visible_column { j - 1 } else { j };
    }
}

/// `_executeBlockComment`: the line comment command of a language without a line comment token removes the
/// block comment around the selection, or wraps the selected lines (the line's text for a caret) in one.
fn block_fallback(lines: &[String], s: Selection, insert_space: bool, move_end_down: bool, comments: Comments) -> Command {
    let Some((start_token, end_token)) = comments.block else {
        return Command { edits: Vec::new(), tracked: None, cursor_state: Box::new(|_, _| unreachable!("a command without edits keeps its selection")) };
    };
    let mut delta_column = 0;
    let edits = match remove_block_comment_around(lines, s, start_token, end_token) {
        Some(edits) => edits,
        None => {
            let (start, end) = (s.start(), s.end());
            let range = match s.is_empty() {
                true => {
                    let content: Vec<char> = lines[start.line].chars().collect();
                    let first = first_non_whitespace_index(&content).unwrap_or(content.len());
                    (Position::new(start.line, first), Position::new(start.line, content.len()))
                }
                false => {
                    let content: Vec<char> = lines[start.line].chars().collect();
                    // `getLineFirstNonWhitespaceColumn` is 0 for a blank line, which the model clamps to its start.
                    let first = first_non_whitespace_index(&content).unwrap_or(0);
                    (Position::new(start.line, first), Position::new(end.line, line_len(lines, end.line)))
                }
            };
            let edits = add_block_comment_edits(range, start_token, end_token, insert_space);
            if edits.len() == 1 {
                delta_column = start_token.chars().count() + 1;
            }
            edits
        }
    };
    Command { edits, tracked: Some(s), cursor_state: tracked_cursor_state(move_end_down, delta_column) }
}

/// `_attemptRemoveBlockComment`: the block comment tokens around the selection, near its ends or on its line.
fn remove_block_comment_around(lines: &[String], s: Selection, start_token: &str, end_token: &str) -> Option<Vec<Replace>> {
    let (start, end) = (s.start(), s.end());
    let (mut start_line, mut end_line) = (start.line, end.line);
    let (start_chars, end_chars): (Vec<char>, Vec<char>) = (start_token.chars().collect(), end_token.chars().collect());
    let line = |line: usize| lines[line].chars().collect::<Vec<char>>();
    let first_non_whitespace_column = first_non_whitespace_index(&line(start.line)).map_or(0, |index| index + 1);
    let allowed_before_column = end_chars.len() + first_non_whitespace_column.max(start.column + 1);
    let mut start_index = last_index_of(&line(start_line), &start_chars, allowed_before_column as isize - 1);
    let mut end_index = index_of(&line(end_line), &end_chars, end.column as isize - start_chars.len() as isize);
    if let (Some(found), None) = (start_index, end_index) {
        end_index = index_of(&line(start_line), &end_chars, (found + start_chars.len()) as isize);
        end_line = start_line;
    }
    if let (None, Some(found)) = (start_index, end_index) {
        start_index = last_index_of(&line(end_line), &start_chars, found as isize);
        start_line = end_line;
    }
    if s.is_empty() && (start_index.is_none() || end_index.is_none()) {
        start_index = index_of(&line(start_line), &start_chars, 0);
        if let Some(found) = start_index {
            end_index = index_of(&line(start_line), &end_chars, (found + start_chars.len()) as isize);
        }
    }
    let (mut start_length, mut end_length) = (start_chars.len(), end_chars.len());
    if let Some(found) = start_index
        && line(start_line).get(found + start_length) == Some(&' ')
    {
        start_length += 1;
    }
    if let Some(found) = end_index
        && found > 0
        && line(end_line)[found - 1] == ' '
    {
        end_length += 1;
        end_index = Some(found - 1);
    }
    let (start_index, end_index) = (start_index?, end_index?);
    Some(remove_block_comment_edits((Position::new(start_line, start_index + start_length), Position::new(end_line, end_index)), start_length, end_length))
}

/// `BlockCommentCommand`: removes the block comment tokens next to the selection's ends, else wraps the
/// selection in them; the cursor selects the text between the tokens.
fn block_comment_command(lines: &[String], selection: Selection, insert_space: bool, comments_at: CommentsAt) -> Command {
    let (start, end) = (selection.start(), selection.end());
    let Some((start_token, end_token)) = comments_at(start.line, start.column).block else {
        return Command { edits: Vec::new(), tracked: None, cursor_state: Box::new(|_, _| unreachable!("a command without edits keeps its selection")) };
    };
    let start_text: Vec<char> = lines[start.line].chars().collect();
    let end_text: Vec<char> = lines[end.line].chars().collect();
    let (start_chars, end_chars): (Vec<char>, Vec<char>) = (start_token.chars().collect(), end_token.chars().collect());
    let mut start_index = last_index_of(&start_text, &start_chars, (start.column + start_chars.len()) as isize);
    let mut end_index = index_of(&end_text, &end_chars, end.column as isize - end_chars.len() as isize);
    if let (Some(found_start), Some(found_end)) = (start_index, end_index) {
        let after_start = found_start + start_chars.len();
        // String.substring swaps its bounds when the end comes first.
        let encloses_an_end_token = match start.line == end.line {
            true => contains(&start_text[after_start.min(found_end)..after_start.max(found_end)], &end_chars),
            false => contains(&start_text[after_start.min(start_text.len())..], &end_chars) || contains(&end_text[..found_end], &end_chars),
        };
        if encloses_an_end_token {
            start_index = None;
            end_index = None;
        }
    }
    let mut used_end_token = None;
    let edits = match (start_index, end_index) {
        (Some(start_index), Some(mut end_index)) => {
            let (mut start_length, mut end_length) = (start_chars.len(), end_chars.len());
            if insert_space && start_text.get(start_index + start_length) == Some(&' ') {
                start_length += 1;
            }
            if insert_space && end_index > 0 && end_text[end_index - 1] == ' ' {
                end_length += 1;
                end_index -= 1;
            }
            remove_block_comment_edits((Position::new(start.line, start_index + start_length), Position::new(end.line, end_index)), start_length, end_length)
        }
        _ => {
            let edits = add_block_comment_edits((start, end), start_token, end_token, insert_space);
            if edits.len() == 1 {
                used_end_token = Some(end_chars.len());
            }
            edits
        }
    };
    let cursor_state = Box::new(move |_, inverse: &[(Position, Position)]| {
        if let [start_token, end_token] = inverse {
            return Selection { anchor: start_token.1, head: end_token.0 };
        }
        let end = inverse[0].1;
        // The caret goes between the tokens, before the space in front of the end token.
        let column = end.column - used_end_token.map_or(0, |length| length + 1);
        Selection::caret(Position::new(end.line, column))
    });
    Command { edits, tracked: None, cursor_state }
}

/// `_createRemoveBlockCommentOperations`: deletes the tokens before and after the range, or both at once
/// when nothing lies between them.
fn remove_block_comment_edits((start, end): (Position, Position), start_length: usize, end_length: usize) -> Vec<Replace> {
    let before = Position::new(start.line, start.column - start_length);
    let after = Position::new(end.line, end.column + end_length);
    if start == end {
        return vec![Replace::new(before, after, "")];
    }
    vec![Replace::new(before, start, ""), Replace::new(end, after, "")]
}

/// `_createAddBlockCommentOperations`: inserts the tokens around the range, or both with two spaces between
/// them into an empty one.
fn add_block_comment_edits((start, end): (Position, Position), start_token: &str, end_token: &str, insert_space: bool) -> Vec<Replace> {
    let space = if insert_space { " " } else { "" };
    if start == end {
        return vec![Replace::new(start, end, format!("{start_token}  {end_token}"))];
    }
    vec![Replace::new(start, start, format!("{start_token}{space}")), Replace::new(end, end, format!("{space}{end_token}"))]
}

/// `BlockCommentCommand._haystackHasNeedleAtOffset`: ASCII letters match either case.
fn has_needle_at_offset(haystack: &[char], needle: &str, offset: isize) -> bool {
    if offset < 0 {
        return false;
    }
    let needle: Vec<char> = needle.chars().collect();
    let offset = offset as usize;
    offset + needle.len() <= haystack.len() && haystack[offset..offset + needle.len()].iter().zip(&needle).all(|(a, b)| a.eq_ignore_ascii_case(b))
}

/// `Selection.setEndPosition`: the end moves, the direction stays.
fn with_end(selection: Selection, end: Position) -> Selection {
    let start = selection.start();
    if selection.head < selection.anchor { Selection { anchor: end, head: start } } else { Selection { anchor: start, head: end } }
}

fn line_len(lines: &[String], line: usize) -> usize {
    lines[line].chars().count()
}

fn first_non_whitespace_index(line: &[char]) -> Option<usize> {
    line.iter().position(|&c| c != ' ' && c != '\t')
}

/// `String.indexOf(needle, from)`, in chars.
fn index_of(haystack: &[char], needle: &[char], from: isize) -> Option<usize> {
    let last = haystack.len().checked_sub(needle.len())?;
    (from.max(0) as usize..=last).find(|&index| haystack[index..].starts_with(needle))
}

/// `String.lastIndexOf(needle, from)`, in chars.
fn last_index_of(haystack: &[char], needle: &[char], from: isize) -> Option<usize> {
    let last = haystack.len().checked_sub(needle.len())?;
    (0..=(from.max(0) as usize).min(last)).rev().find(|&index| haystack[index..].starts_with(needle))
}

fn contains(haystack: &[char], needle: &[char]) -> bool {
    index_of(haystack, needle, 0).is_some()
}

#[cfg(test)]
mod tests;
