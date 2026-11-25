//! editor/contrib/linesOperations/browser: the actions of linesOperations.ts with copyLinesCommand.ts,
//! moveLinesCommand.ts and sortLinesCommand.ts. Columns count chars where upstream counts UTF-16 code units.

use std::collections::HashSet;
use std::sync::LazyLock;

use icu_collator::options::CollatorOptions;
use icu_collator::{Collator, CollatorBorrowed, CollatorPreferences};
use regex::Regex;

use super::auto_indent::{self, AutoIndent, Model};
use super::edit::{Command, command};
use super::{ContentChanged, EditReason, OpenEditor, Position, Replace, Selection, cursor, text_between, word_at};

/// The case actions of linesOperations.ts (`AbstractCaseAction`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CaseTransform {
    Upper,
    Lower,
    Title,
    Snake,
    Camel,
    Pascal,
    Kebab,
}

impl CaseTransform {
    pub const ALL: [(&str, CaseTransform); 7] = [
        ("editor.action.transformToUppercase", CaseTransform::Upper),
        ("editor.action.transformToLowercase", CaseTransform::Lower),
        ("editor.action.transformToTitlecase", CaseTransform::Title),
        ("editor.action.transformToSnakecase", CaseTransform::Snake),
        ("editor.action.transformToCamelcase", CaseTransform::Camel),
        ("editor.action.transformToPascalcase", CaseTransform::Pascal),
        ("editor.action.transformToKebabcase", CaseTransform::Kebab),
    ];

    fn id(self) -> &'static str {
        Self::ALL.iter().find(|(_, transform)| *transform == self).expect("every transform has an id").0
    }
}

impl OpenEditor {
    /// `AbstractCopyLinesAction.run`: a `CopyLinesCommand` per selection, sorted by start; of two selections
    /// sharing a line, the later added one copies nothing.
    pub fn copy_lines(&mut self, down: bool) -> Option<ContentChanged> {
        let id = if down { "editor.action.copyLinesDownAction" } else { "editor.action.copyLinesUpAction" };
        let selections = &self.selections;
        let mut order: Vec<usize> = (0..selections.len()).collect();
        order.sort_by_key(|&index| (selections[index].start(), selections[index].end()));
        let mut ignored = vec![false; selections.len()];
        let mut previous = order[0];
        for &current in &order[1..] {
            if selections[previous].end().line != selections[current].start().line {
                continue;
            }
            if previous < current {
                ignored[current] = true;
            } else {
                ignored[previous] = true;
                previous = current;
            }
        }
        let commands = order.iter().map(|&index| copy_lines_command(&self.document().lines, selections[index], down, ignored[index])).collect();
        self.run_commands_between_stops(commands, id)
    }

    /// `DuplicateSelectionAction.run`: an empty selection copies its line down, a selection inserts its text
    /// after itself and selects the copy.
    pub fn duplicate_selection(&mut self) -> Option<ContentChanged> {
        let document = self.document();
        let lines = &document.lines;
        let commands = self
            .selections
            .iter()
            .map(|&selection| match selection.is_empty() {
                true => copy_lines_command(lines, selection, true, false),
                false => replace_command_that_selects_text(selection.end(), text_between(lines, selection.start(), selection.end())),
            })
            .collect();
        drop(document);
        self.run_commands_between_stops(commands, "editor.action.duplicateSelection")
    }

    /// `AbstractMoveLinesAction.run`: a `MoveLinesCommand` per selection.
    pub fn move_lines(&mut self, down: bool, auto: AutoIndent) -> Option<ContentChanged> {
        let document = self.document();
        let model = auto.model(&document);
        let id = if down { "editor.action.moveLinesDownAction" } else { "editor.action.moveLinesUpAction" };
        let mut empty_line_moves = Vec::new();
        let commands = self
            .selections
            .iter()
            .map(|&selection| {
                let (command, empty_line_move) = move_lines_command(&model, selection, down);
                empty_line_moves.push(empty_line_move);
                command
            })
            .collect();
        drop(document);
        let selections_before = self.selections.clone();
        let change = self.run_commands_between_stops(commands, id);
        // Upstream moves an empty line over an empty line with a no-op edit, which still runs `computeCursorState`.
        if empty_line_moves.iter().any(Option::is_some) && self.selections.len() == selections_before.len() {
            let current = if change.is_some() { self.selections.clone() } else { selections_before };
            let moved = current.into_iter().zip(empty_line_moves).map(|(selection, moved)| moved.unwrap_or(selection)).collect();
            self.set_selections(moved);
        }
        change
    }

    /// `AbstractSortLinesAction.run`: one selection on one line sorts the whole document; nothing happens when
    /// any selection has nothing to sort or is sorted already.
    pub fn sort_lines(&mut self, descending: bool) -> Option<ContentChanged> {
        let id = if descending { "editor.action.sortLinesDescending" } else { "editor.action.sortLinesAscending" };
        let document = self.document();
        let lines = &document.lines;
        let selections = whole_document_for_one_line(lines, &self.selections);
        let can_run = |selection| sort_data(lines, selection, descending).is_some_and(|(_, _, before, after)| before != after);
        if !selections.iter().all(|&selection| can_run(selection)) {
            return None;
        }
        let commands = selections.into_iter().map(|selection| sort_lines_command(lines, selection, descending)).collect();
        drop(document);
        self.run_commands_between_stops(commands, id)
    }

    /// `DeleteDuplicateLinesAction.run`: each selection's lines keep the first of equal lines and end up
    /// selected; one selection on one line works on the whole document and keeps the cursors.
    pub fn delete_duplicate_lines(&mut self) -> Option<ContentChanged> {
        let document = self.document();
        let lines = &document.lines;
        if lines.len() == 1 && lines[0].is_empty() {
            return None;
        }
        let one_line = self.selections.len() == 1 && self.selections[0].start().line == self.selections[0].end().line;
        let selections = whole_document_for_one_line(lines, &self.selections);
        let mut edits = Vec::new();
        let mut end_selections = Vec::new();
        let mut lines_deleted = 0;
        for selection in selections {
            let (start, end) = (selection.start().line, selection.end().line);
            let mut seen = HashSet::new();
            let kept: Vec<&str> = lines[start..=end].iter().map(String::as_str).filter(|line| seen.insert(*line)).collect();
            edits.push((Position::new(start, 0), Position::new(end, cursor::line_len(lines, end)), kept.join("\n"), false));
            let first = start - lines_deleted;
            let last = kept.last().expect("a selection holds a line");
            end_selections.push(Selection { anchor: Position::new(first, 0), head: Position::new(first + kept.len() - 1, last.chars().count()) });
            lines_deleted += end - start + 1 - kept.len();
        }
        drop(document);
        self.execute_edits_between_stops(edits, (!one_line).then_some(end_selections), "editor.action.removeDuplicateLines")
    }

    /// `ReverseLinesAction.run`: each selection's lines in reverse order, without a last empty line of the
    /// document; one selection on one line reverses the whole document. Cursors move with their lines.
    pub fn reverse_lines(&mut self) -> Option<ContentChanged> {
        let document = self.document();
        let lines = &document.lines;
        let original = self.selections.clone();
        let selections = whole_document_for_one_line(lines, &self.selections);
        let mut edits = Vec::new();
        let mut end_selections = Vec::new();
        for (selection, original) in selections.into_iter().zip(original) {
            let mut end_line = selection.end().line;
            if selection.start().line < end_line && selection.end().column == 0 {
                end_line -= 1;
            }
            let mut start = Position::new(selection.start().line, 0);
            let mut end = Position::new(end_line, cursor::line_len(lines, end_line));
            if end_line == lines.len() - 1 && lines[end_line].is_empty() {
                let above = end_line.checked_sub(1)?;
                end = Position::new(above, cursor::line_len(lines, above));
            }
            // `Range` keeps its start before its end.
            if end < start {
                (start, end) = (end, start);
            }
            let reversed: Vec<&str> = lines[start.line..=end.line].iter().rev().map(String::as_str).collect();
            edits.push((start, end, reversed.join("\n"), false));
            let moved_line = |line: usize| if line <= end.line { end.line + start.line - line } else { line };
            end_selections.push(match original.is_empty() {
                true => Selection::caret(Position::new(moved_line(original.head.line), original.head.column)),
                false => Selection { anchor: Position::new(moved_line(original.anchor.line), original.anchor.column), head: Position::new(moved_line(original.head.line), original.head.column) },
            });
        }
        drop(document);
        self.execute_edits_between_stops(edits, Some(end_selections), "editor.action.reverseLines")
    }

    /// `DeleteLinesAction.run`: the lines of the selections, merged where they touch, each with a line break;
    /// each cursor keeps its column on the line that takes their place.
    pub fn delete_lines(&mut self) -> Option<ContentChanged> {
        let document = self.document();
        let lines = &document.lines;
        if lines.len() == 1 && lines[0].is_empty() {
            return None;
        }
        let mut edits = Vec::new();
        let mut end_selections = Vec::new();
        let mut lines_deleted = 0;
        for (first, last, column) in lines_to_remove(&self.selections) {
            let (mut start, mut end) = (Position::new(first, 0), Position::new(last, cursor::line_len(lines, last)));
            if last < lines.len() - 1 {
                end = Position::new(last + 1, 0);
            } else if first > 0 {
                start = Position::new(first - 1, cursor::line_len(lines, first - 1));
            }
            edits.push((start, end, String::new(), false));
            end_selections.push(Selection::caret(Position::new(start.line - lines_deleted, column)));
            lines_deleted += last - first + 1;
        }
        drop(document);
        self.execute_edits_between_stops(edits, Some(end_selections), "editor.action.deleteLines")
    }

    /// `IndentLinesAction.run`: `TypeOperations.indent`, a `ShiftCommand` over each cursor's lines.
    pub fn indent_lines(&mut self, auto: AutoIndent) -> Option<ContentChanged> {
        self.shift(false, auto, command("editor.action.indentLines"))
    }

    /// `InsertLineBeforeAction.run`: Enter at the end of the line above each cursor; above the first line the
    /// new line is empty. Only an undo stop before, as upstream.
    pub fn insert_line_before(&mut self, auto: AutoIndent) -> Option<ContentChanged> {
        let document = self.document();
        let model = auto.model(&document);
        let commands = self
            .selections
            .iter()
            .map(|selection| match selection.head.line {
                0 => Command { edits: vec![Replace::new(Position::new(0, 0), Position::new(0, 0), "\n")], tracked: None, cursor_state: Box::new(|_, ranges| Selection::caret(ranges[0].0)) },
                line => enter_at_line_end(&model, line - 1),
            })
            .collect();
        drop(document);
        self.push_undo_stop();
        self.execute_commands(commands, command("editor.action.insertLineBefore"))
    }

    /// `InsertLineAfterAction.run`: Enter at the end of each cursor's line.
    pub fn insert_line_after(&mut self, auto: AutoIndent) -> Option<ContentChanged> {
        let document = self.document();
        let model = auto.model(&document);
        let commands = self.selections.iter().map(|selection| enter_at_line_end(&model, selection.head.line)).collect();
        drop(document);
        self.push_undo_stop();
        self.execute_commands(commands, command("editor.action.insertLineAfter"))
    }

    /// `DeleteAllLeftAction.run`: each cursor's text to its line start, or the line break before it at column 1;
    /// a selection widens to its first line's start. Ranges that touch merge, the primary cursor comes first.
    pub fn delete_all_left(&mut self) -> Option<ContentChanged> {
        let document = self.document();
        let lines = &document.lines;
        let mut ranges: Vec<(Position, Position)> = self.selections.iter().map(|&selection| cursor::delete_all_left_range(lines, selection)).collect();
        ranges.sort();
        let ranges = merge_touching(ranges);
        let primary = self.selection();
        let mut carets = Vec::new();
        let mut deleted_lines = 0;
        for &(start, end) in &ranges {
            let line = if end.column == 0 && deleted_lines > 0 { start.line - deleted_lines } else { start.line };
            deleted_lines += end.line - start.line;
            carets.push((Selection::caret(Position::new(line, start.column)), intersects((start, end), (primary.start(), primary.end()))));
        }
        let end_selections = primary_first(carets);
        let edits = ranges.into_iter().map(|(start, end)| (start, end, String::new(), false)).collect();
        drop(document);
        self.execute_edits_between_stops(edits, Some(end_selections), "deleteAllLeft")
    }

    /// `DeleteAllRightAction.run`: each cursor's text to its line end, or the line break after it at the line end.
    pub fn delete_all_right(&mut self) -> Option<ContentChanged> {
        let document = self.document();
        let lines = &document.lines;
        let mut ranges: Vec<(Position, Position)> = self.selections.iter().map(|&selection| cursor::delete_all_right_range(lines, selection)).collect();
        ranges.sort();
        let ranges = merge_touching(ranges);
        let primary = self.selection();
        let carets = ranges.iter().map(|&(start, end)| (Selection::caret(start), intersects((start, end), (primary.start(), primary.end())))).collect();
        let end_selections = primary_first(carets);
        let edits = ranges.into_iter().map(|(start, end)| (start, end, String::new(), false)).collect();
        drop(document);
        self.execute_edits_between_stops(edits, Some(end_selections), "deleteAllRight")
    }

    /// `JoinLinesAction.run`: each selection's lines, or a cursor's line and the next, become one line, the
    /// indentation of the joined lines replaced by one space.
    pub fn join_lines(&mut self) -> Option<ContentChanged> {
        let document = self.document();
        let lines = &document.lines;
        let mut primary = self.selection();
        let mut selections = self.selections.clone();
        selections.sort_by_key(|selection| (selection.start(), selection.end()));
        let mut reduced = Vec::new();
        let mut previous = selections[0];
        for &current in &selections[1..] {
            let (previous_end, current_start) = (previous.end().line, current.start().line);
            if previous.is_empty() && previous_end == current_start {
                if primary == previous {
                    primary = current;
                }
                previous = current;
            } else if current_start > previous_end + usize::from(previous.is_empty()) {
                reduced.push(previous);
                previous = current;
            } else {
                previous = Selection { anchor: previous.start(), head: current.end() };
            }
        }
        reduced.push(previous);

        let mut edits = Vec::new();
        let mut others = Vec::new();
        let mut end_primary = primary;
        let mut line_offset = 0;
        for selection in reduced {
            let (start, end) = (selection.start(), selection.end());
            let start_line = start.line;
            let end_offset_from_line_end = cursor::line_len(lines, end.line) as isize - end.column as isize - 1;
            let end_line = match selection.is_empty() || start.line == end.line {
                true if start.line + 1 < lines.len() => start_line + 1,
                true => start.line,
                false => end.line,
            };
            let mut joined = lines[start_line].clone();
            let mut column_delta = 0;
            for line in &lines[start_line + 1..=end_line] {
                let Some(first_non_whitespace) = line.find(|c| c != ' ' && c != '\t') else {
                    column_delta = 0;
                    continue;
                };
                let mut insert_space = !joined.is_empty();
                if insert_space && (joined.ends_with(' ') || joined.ends_with('\t')) {
                    insert_space = false;
                    joined = format!("{} ", joined.trim_end_matches(|c: char| c.is_whitespace() || c == '\u{feff}'));
                }
                let without_indent = &line[first_non_whitespace..];
                joined = format!("{joined}{}{without_indent}", if insert_space { " " } else { "" });
                column_delta = without_indent.chars().count() + usize::from(insert_space);
            }
            let delete = (Position::new(start_line, 0), Position::new(end_line, cursor::line_len(lines, end_line)));
            if delete.0 != delete.1 {
                let joined_len = joined.chars().count();
                let result = match (selection.is_empty(), start.line == end.line) {
                    (true, _) => Selection::caret(Position::new(start_line - line_offset, joined_len - column_delta)),
                    (false, true) => Selection { anchor: Position::new(start.line - line_offset, start.column), head: Position::new(end.line - line_offset, end.column) },
                    (false, false) => Selection {
                        anchor: Position::new(start.line - line_offset, start.column),
                        head: Position::new(start.line - line_offset, (joined_len as isize - end_offset_from_line_end - 1) as usize),
                    },
                };
                edits.push((delete.0, delete.1, joined, false));
                if intersects(delete, (primary.start(), primary.end())) {
                    end_primary = result;
                } else {
                    others.push(result);
                }
            }
            line_offset += end_line - start_line;
        }
        let end_selections = std::iter::once(end_primary).chain(others).collect();
        drop(document);
        self.execute_edits_between_stops(edits, Some(end_selections), "editor.action.joinLines")
    }

    /// `TransposeAction.run`: each cursor swaps the characters around it and moves right; at a line end it swaps
    /// the last character with the line break.
    pub fn transpose(&mut self) -> Option<ContentChanged> {
        let document = self.document();
        let lines = &document.lines;
        let mut commands = Vec::new();
        for selection in &self.selections {
            if !selection.is_empty() {
                continue;
            }
            let at = selection.head;
            let line_len = cursor::line_len(lines, at.line);
            let start = Position::new(at.line, at.column.saturating_sub(1));
            if at.column >= line_len {
                if at.line == lines.len() - 1 {
                    continue;
                }
                let end = Position::new(at.line + 1, 0);
                let swapped: String = text_between(lines, start, end).chars().rev().collect();
                commands.push(Command { edits: vec![Replace::new(start, end, swapped)], tracked: None, cursor_state: Box::new(|_, ranges| Selection::caret(ranges[0].1)) });
            } else {
                let end = Position::new(at.line, at.column + 1);
                let swapped: String = text_between(lines, start, end).chars().rev().collect();
                let caret = Selection::caret(end);
                commands.push(Command { edits: vec![Replace::new(start, end, swapped)], tracked: Some(caret), cursor_state: Box::new(|tracked, _| tracked.expect("tracked")) });
            }
        }
        // `executeCommands` gives command `i` to cursor `i`, also when an earlier cursor had none; the rest keep theirs.
        while commands.len() < self.selections.len() {
            commands.push(Command { edits: Vec::new(), tracked: None, cursor_state: Box::new(|_, _| unreachable!("a command without edits keeps its selection")) });
        }
        drop(document);
        self.run_commands_between_stops(commands, "editor.action.transpose")
    }

    /// `AbstractCaseAction.run`: each selection's text, or the word at an empty cursor, in the new case.
    pub fn transform_case(&mut self, transform: CaseTransform) -> Option<ContentChanged> {
        let document = self.document();
        let lines = &document.lines;
        let mut edits = Vec::new();
        for selection in &self.selections {
            let (start, end) = match selection.is_empty() {
                true => {
                    let Some(word) = word_at(&lines[selection.head.line], selection.head.column) else { continue };
                    (Position::new(selection.head.line, word.start), Position::new(selection.head.line, word.end))
                }
                false => (selection.start(), selection.end()),
            };
            edits.push((start, end, modify_case(&text_between(lines, start, end), transform), false));
        }
        drop(document);
        self.execute_edits_between_stops(edits, None, transform.id())
    }

    pub(super) fn run_commands_between_stops(&mut self, commands: Vec<Command>, id: &'static str) -> Option<ContentChanged> {
        self.push_undo_stop();
        let change = self.execute_commands(commands, command(id));
        self.push_undo_stop();
        change
    }

    fn execute_edits_between_stops(&mut self, edits: Vec<(Position, Position, String, bool)>, end_selections: Option<Vec<Selection>>, id: &'static str) -> Option<ContentChanged> {
        self.push_undo_stop();
        let change = self.execute_edits(edits, end_selections, EditReason::Unknown(id));
        self.push_undo_stop();
        change
    }
}

/// `CopyLinesCommand`: the lines of `selection` once more above or below them; `noop` rewrites the line break
/// after them, so the cursor keeps its place.
fn copy_lines_command(lines: &[String], selection: Selection, down: bool, noop: bool) -> Command {
    let mut s = selection;
    let (mut start_delta, mut end_delta) = (0, 0);
    if s.start().line < s.end().line && s.end().column == 0 {
        end_delta = 1;
        let above = s.end().line - 1;
        s = with_end(s, Position::new(above, cursor::line_len(lines, above)));
    }
    let (start, end) = (s.start(), s.end());
    let source = lines[start.line..=end.line].join("\n");
    if source.is_empty() && down {
        start_delta += 1;
        end_delta += 1;
    }
    let line_end = Position::new(end.line, cursor::line_len(lines, end.line));
    let edits = match (noop, down) {
        // At the last line the range ends at the document end, as `validateRange` makes it: a no-op.
        (true, _) if end.line + 1 == lines.len() => Vec::new(),
        (true, _) => vec![Replace::new(line_end, Position::new(end.line + 1, 0), "\n")],
        (false, false) => vec![Replace::new(line_end, line_end, format!("\n{source}"))],
        (false, true) => vec![Replace::new(Position::new(start.line, 0), Position::new(start.line, 0), format!("{source}\n"))],
    };
    let right_to_left = selection.head < selection.anchor;
    let cursor_state = Box::new(move |tracked: Option<Selection>, _: &[(Position, Position)]| {
        let tracked = tracked.expect("tracked");
        if start_delta == 0 && end_delta == 0 {
            return tracked;
        }
        let (mut start, mut end) = (tracked.start(), tracked.end());
        if start_delta != 0 {
            start = Position::new(start.line + start_delta, 0);
        }
        if end_delta != 0 {
            end = Position::new(end.line + end_delta, 0);
        }
        if right_to_left { Selection { anchor: end, head: start } } else { Selection { anchor: start, head: end } }
    });
    Command { edits, tracked: Some(s), cursor_state }
}

/// `ReplaceCommandThatSelectsText`: inserts `text` at `at` and selects it.
fn replace_command_that_selects_text(at: Position, text: String) -> Command {
    Command { edits: vec![Replace::new(at, at, text)], tracked: None, cursor_state: Box::new(|_, ranges| Selection { anchor: ranges[0].0, head: ranges[0].1 }) }
}

/// `MoveLinesCommand`, and where an empty line moved over an empty line puts the cursor, which no edit carries.
fn move_lines_command(model: &Model, selection: Selection, down: bool) -> (Command, Option<Selection>) {
    let lines = model.lines;
    let keep = || Command { edits: Vec::new(), tracked: Some(selection), cursor_state: Box::new(|tracked, _| tracked.expect("tracked")) };
    if (down && selection.end().line == lines.len() - 1) || (!down && selection.start().line == 0) {
        return (keep(), None);
    }
    let mut move_end_down = false;
    let mut shrink_end = false;
    let mut s = selection;
    if s.start().line < s.end().line && s.end().column == 0 {
        move_end_down = true;
        let above = s.end().line - 1;
        s = with_end(s, Position::new(above, cursor::line_len(lines, above)));
    }
    let line_end = |line: usize| Position::new(line, cursor::line_len(lines, line));
    let mut edits = Vec::new();
    if s.start().line == s.end().line && lines[s.start().line].is_empty() {
        let line = s.start().line;
        let other = if down { line + 1 } else { line - 1 };
        let target = Selection::caret(Position::new(other, 0));
        if lines[other].is_empty() {
            return (keep(), Some(target));
        }
        edits.push(Replace::new(Position::new(line, 0), Position::new(line, 0), lines[other].clone()));
        edits.push(Replace::new(Position::new(other, 0), line_end(other), ""));
        s = target;
    } else if down {
        let moving = s.end().line + 1;
        edits.push(Replace::new(line_end(moving - 1), line_end(moving), ""));
        let inserting = match model.indents_moved_lines(s.start().line, s.end().line) {
            true => moved_down_line(model, s, moving),
            false => lines[moving].clone(),
        };
        edits.push(Replace::new(Position::new(s.start().line, 0), Position::new(s.start().line, 0), format!("{inserting}\n")));
        if model.indents_moved_lines(s.start().line, s.end().line) {
            let offset = model.enter_rule_offset_moving_down(s.start().line, moving, &inserting).or_else(|| {
                let map = |line: usize| match line {
                    line if line == s.start().line => moving,
                    line if line > s.start().line && line <= s.end().line + 1 => line - 1,
                    line => line,
                };
                let moved = model.virtual_model(&map, Some((s.start().line, inserting.clone(), model.processed_line(moving))));
                let indent = moved.good_indent(&model.language_at(moving, 0), s.start().line + 1)?;
                Some(indent_offset(model, &indent, s.start().line))
            });
            shrink_end = indent_moving_block(model, s, offset.unwrap_or(0), &mut edits);
        }
    } else {
        let moving = s.start().line - 1;
        edits.push(Replace::new(Position::new(moving, 0), Position::new(moving + 1, 0), ""));
        edits.push(Replace::new(line_end(s.end().line), line_end(s.end().line), format!("\n{}", lines[moving])));
        if model.indents_moved_lines(s.start().line, s.end().line) {
            let offset = model.enter_rule_offset(s.start().line, s.start().line.checked_sub(2)).or_else(|| {
                let map = |line: usize| if line == moving { s.start().line } else { line };
                let moved = model.virtual_model(&map, None);
                let indent = moved.good_indent(&model.language_at(s.start().line, 0), moving)?;
                Some(indent_offset(model, &indent, s.start().line))
            });
            shrink_end = indent_moving_block(model, s, offset.unwrap_or(0), &mut edits);
        }
    }
    let cursor_state = Box::new(move |tracked: Option<Selection>, _: &[(Position, Position)]| {
        let mut tracked = tracked.expect("tracked");
        if move_end_down {
            tracked = with_end(tracked, Position::new(tracked.end().line + 1, 0));
        }
        if shrink_end && tracked.start().line < tracked.end().line {
            tracked = with_end(tracked, Position::new(tracked.end().line, 1));
        }
        tracked
    });
    (Command { edits, tracked: Some(s), cursor_state }, None)
}

/// The line moving down over `s`, reindented for its new place above it: by the onEnter rule of the line
/// above, else by the indentation rules.
fn moved_down_line(model: &Model, s: Selection, moving: usize) -> String {
    let text = &model.lines[moving];
    let indentation = model.indentation;
    let old = auto_indent::space_count(auto_indent::leading_whitespace(text), indentation.tab_size) as isize;
    let reindented = |count: isize| auto_indent::generate_indent(count, indentation) + text.trim_start();
    if let Some(offset) = model.enter_rule_offset(moving, s.start().line.checked_sub(1)) {
        return reindented(old + offset);
    }
    let map = |line: usize| if line == s.start().line { moving } else { line };
    let moved = model.virtual_model(&map, None);
    match moved.good_indent(&model.language_at(moving, 0), s.start().line) {
        Some(indent) if auto_indent::space_count(&indent, indentation.tab_size) as isize != old => reindented(auto_indent::space_count(&indent, indentation.tab_size) as isize),
        _ => text.clone(),
    }
}

/// How many columns `indent` is wider than `line`'s indentation.
fn indent_offset(model: &Model, indent: &str, line: usize) -> isize {
    let tab_size = model.indentation.tab_size;
    auto_indent::space_count(indent, tab_size) as isize - auto_indent::space_count(auto_indent::leading_whitespace(&model.lines[line]), tab_size) as isize
}

/// `getIndentEditsOfMovingBlock`: each line of `s` indented by `offset` columns. Answers whether the selection
/// end, inside the last line's indentation that goes, moves to that line's second column.
fn indent_moving_block(model: &Model, s: Selection, offset: isize, edits: &mut Vec<Replace>) -> bool {
    if offset == 0 {
        return false;
    }
    let mut shrink_end = false;
    for line in s.start().line..=s.end().line {
        let original = auto_indent::leading_whitespace(&model.lines[line]);
        let indent = auto_indent::generate_indent(auto_indent::space_count(original, model.indentation.tab_size) as isize + offset, model.indentation);
        if indent != original {
            edits.push(Replace::new(Position::new(line, 0), Position::new(line, original.chars().count()), indent.clone()));
            if line == s.end().line && s.end().column <= original.chars().count() && indent.is_empty() {
                shrink_end = true;
            }
        }
    }
    shrink_end
}

/// `EnterOperation._enter` at the end of `line`, the cursor where Enter leaves it.
fn enter_at_line_end(model: &Model, line: usize) -> Command {
    let at = Position::new(line, cursor::line_len(model.lines, line));
    let (start, end, text, caret) = model.enter_edit(at, at);
    let lines_down = caret.line - start.line;
    let cursor_state = Box::new(move |_: Option<Selection>, ranges: &[(Position, Position)]| Selection::caret(Position::new(ranges[0].0.line + lines_down, caret.column)));
    Command { edits: vec![Replace::new(start, end, text)], tracked: None, cursor_state }
}

/// `Selection.setEndPosition`: the same direction with a new end.
fn with_end(selection: Selection, end: Position) -> Selection {
    let start = selection.start();
    if selection.head < selection.anchor { Selection { anchor: end, head: start } } else { Selection { anchor: start, head: end } }
}

/// `SortLinesCommand`: the selection's lines sorted, the selection tracked.
fn sort_lines_command(lines: &[String], selection: Selection, descending: bool) -> Command {
    let edits = match sort_data(lines, selection, descending) {
        Some((start, end, _, after)) => vec![Replace::new(Position::new(start, 0), Position::new(end, cursor::line_len(lines, end)), after.join("\n"))],
        None => Vec::new(),
    };
    Command { edits, tracked: Some(selection), cursor_state: Box::new(|tracked, _| tracked.expect("tracked")) }
}

/// One selection on one line stands for the whole document.
fn whole_document_for_one_line(lines: &[String], selections: &[Selection]) -> Vec<Selection> {
    if selections.len() == 1 && selections[0].start().line == selections[0].end().line {
        let last = lines.len() - 1;
        return vec![Selection { anchor: Position::new(0, 0), head: Position::new(last, cursor::line_len(lines, last)) }];
    }
    selections.to_vec()
}

/// `getSortData`: the first and last line to sort, without a last line the selection ends at the start of,
/// and the lines before and after sorting with `Intl.Collator().compare`.
fn sort_data(lines: &[String], selection: Selection, descending: bool) -> Option<(usize, usize, Vec<&str>, Vec<&str>)> {
    static COLLATOR: LazyLock<CollatorBorrowed<'static>> =
        LazyLock::new(|| Collator::try_new(CollatorPreferences::default(), CollatorOptions::default()).expect("the root collation is compiled in"));
    let start = selection.start().line;
    let end = match selection.end().column {
        0 => selection.end().line.checked_sub(1)?,
        _ => selection.end().line,
    };
    if start >= end {
        return None;
    }
    let before: Vec<&str> = lines[start..=end].iter().map(String::as_str).collect();
    let mut after = before.clone();
    after.sort_by(|a, b| COLLATOR.compare(a, b));
    if descending {
        after.reverse();
    }
    Some((start, end, before, after))
}

/// `_getLinesToRemove`: (first line, last line, cursor column) per selection, a selection ending at a line start
/// without that line, sorted and merged where they touch or overlap.
fn lines_to_remove(selections: &[Selection]) -> Vec<(usize, usize, usize)> {
    let mut operations: Vec<(usize, usize, usize)> = selections
        .iter()
        .map(|selection| {
            let (start, end) = (selection.start(), selection.end());
            let last = if start.line < end.line && end.column == 0 { end.line - 1 } else { end.line };
            (start.line, last, selection.head.column)
        })
        .collect();
    operations.sort_by_key(|&(first, last, _)| (first, last));
    let mut merged = Vec::new();
    let mut previous = operations[0];
    for &operation in &operations[1..] {
        if previous.1 + 1 >= operation.0 {
            previous.1 = operation.1;
        } else {
            merged.push(previous);
            previous = operation;
        }
    }
    merged.push(previous);
    merged
}

/// The delete-all actions' merge: sorted ranges that intersect or touch become one (`Range.plusRange`).
fn merge_touching(mut ranges: Vec<(Position, Position)>) -> Vec<(Position, Position)> {
    let mut merged = Vec::new();
    for index in 0..ranges.len() - 1 {
        let (range, next) = (ranges[index], ranges[index + 1]);
        if intersects(range, next) {
            ranges[index + 1] = (range.0.min(next.0), range.1.max(next.1));
        } else {
            merged.push(range);
        }
    }
    merged.push(ranges[ranges.len() - 1]);
    merged
}

/// `_getEndCursorState`: the caret of the range that meets the primary selection first, the rest in order.
fn primary_first(carets: Vec<(Selection, bool)>) -> Vec<Selection> {
    let primary = carets.iter().rev().find(|(_, meets_primary)| *meets_primary).map(|(caret, _)| *caret);
    primary.into_iter().chain(carets.into_iter().filter(|(_, meets_primary)| !meets_primary).map(|(caret, _)| caret)).collect()
}

/// `Range.intersectRanges(...) !== null`: ranges that touch intersect.
fn intersects(one: (Position, Position), other: (Position, Position)) -> bool {
    one.0.max(other.0) <= one.1.min(other.1)
}

/// The `_modifyText` of each case action.
fn modify_case(text: &str, transform: CaseTransform) -> String {
    static TITLE_BOUNDARY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)(^|[^\p{L}\p{N}']|((^|\P{L})'))\p{L}").expect("valid"));
    static CASE_BOUNDARY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\p{Ll})(\p{Lu})").expect("valid"));
    static SNAKE_SINGLE_LETTERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\p{Lu}|\p{N})(\p{Lu})(\p{Ll})").expect("valid"));
    static KEBAB_SINGLE_LETTERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\p{Lu}|\p{N})(\p{Lu}\p{Ll})").expect("valid"));
    static UNDERSCORE_BOUNDARY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\S)(_)(\S)").expect("valid"));
    static SINGLE_LINE_WORD_BOUNDARY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[_\s-]+").expect("valid"));
    static MULTI_LINE_WORD_BOUNDARY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[_-]+").expect("valid"));
    static VALID_WORD_START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^(\p{Lu}[^\p{Lu}])").expect("valid"));
    static PASCAL_WORD_BOUNDARY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[_ \t-]").expect("valid"));
    static UPPER_CASE_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\p{Lu}+$").expect("valid"));
    match transform {
        CaseTransform::Upper => text.to_uppercase(),
        CaseTransform::Lower => text.to_lowercase(),
        CaseTransform::Title => TITLE_BOUNDARY.replace_all(&text.to_lowercase(), |captures: &regex::Captures| captures[0].to_uppercase()).into_owned(),
        CaseTransform::Snake => {
            let text = CASE_BOUNDARY.replace_all(text, "${1}_${2}");
            SNAKE_SINGLE_LETTERS.replace_all(&text, "${1}_${2}${3}").to_lowercase()
        }
        CaseTransform::Camel => {
            let boundary = if text.contains(['\r', '\n']) { &MULTI_LINE_WORD_BOUNDARY } else { &SINGLE_LINE_WORD_BOUNDARY };
            let mut words = boundary.split(text);
            let first = words.next().expect("split gives a first piece");
            let mut camel = VALID_WORD_START.replace_all(first, |captures: &regex::Captures| captures[0].to_lowercase()).into_owned();
            for word in words {
                camel.push_str(&upper_first(word));
            }
            camel
        }
        CaseTransform::Pascal => {
            let mut pascal = String::new();
            // `split(/(?<=\.)/)`: each piece ends after a dot.
            for piece in text.split_inclusive('.') {
                for word in PASCAL_WORD_BOUNDARY.split(piece) {
                    let normalized = upper_first(word);
                    let all_caps = normalized.chars().count() > 1 && UPPER_CASE_WORD.is_match(&normalized);
                    match all_caps {
                        true => {
                            let mut chars = normalized.chars();
                            let first = chars.next().expect("more than one char");
                            pascal.push(first);
                            pascal.push_str(&chars.as_str().to_lowercase());
                        }
                        false => pascal.push_str(&normalized),
                    }
                }
            }
            pascal
        }
        CaseTransform::Kebab => {
            let text = UNDERSCORE_BOUNDARY.replace_all(text, "${1}-${3}");
            let text = CASE_BOUNDARY.replace_all(&text, "${1}-${2}");
            KEBAB_SINGLE_LETTERS.replace_all(&text, "${1}-${2}").to_lowercase()
        }
    }
}

/// `word.substring(0, 1).toLocaleUpperCase() + word.substring(1)`.
fn upper_first(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests;
