//! findModel.ts with findDecorations.ts: the matches of the find state in an editor, the current one,
//! moving between them, replace and replace all. The caller holds the `FindState` and tells the model
//! about state changes, explicit cursor moves and content changes, the events upstream listens to.

use std::ops::Range;

use super::replace_pattern::{ReplacePattern, parse_replace_string};
use super::search::{FindMatch, SearchParams, find_matches_in_ranges, find_next_match, find_previous_match};
use super::state::{FindState, FindStateChanged, MATCHES_LIMIT};
use crate::editor::cursor::{last_position, line_len};
use crate::editor::edit::{document_offset, document_position, track_decoration};
use crate::editor::{ContentChanged, EditReason, OpenEditor, Position, Selection, VerticalReveal, Viewport, WORD_SEPARATORS, end_of, text_between};

/// `FindModelBoundToEditorModel` and the decorations it keeps.
#[derive(Debug, Default)]
pub struct FindModel {
    /// The find match decorations, in model order.
    pub matches: Vec<Range<Position>>,
    /// The match drawn as the current one (`_highlightedDecorationId`).
    pub highlighted: Option<usize>,
    /// The current match's lines, drawn with `editor.rangeHighlightBackground`.
    pub range_highlight: Option<Range<Position>>,
    /// The find scope decorations as model offsets, so they follow edits.
    scopes: Vec<[usize; 2]>,
    /// Where the next search starts: the cursor when it last moved by the user or an edit.
    start_position: Position,
    /// `editor.find.cursorMoveOnType`.
    pub cursor_move_on_type: bool,
}

impl FindModel {
    /// Binds to the editor and searches with the state's scope.
    pub fn new(editor: &mut OpenEditor, state: &mut FindState, viewport: Viewport) -> Self {
        let mut model = FindModel { start_position: editor.selection().head, cursor_move_on_type: true, ..Default::default() };
        let scope = state.search_scope.clone();
        model.research(editor, state, viewport, false, Some(scope));
        model
    }

    /// `_onStateChanged`.
    pub fn state_changed(&mut self, editor: &mut OpenEditor, state: &mut FindState, viewport: Viewport, changed: FindStateChanged) {
        if !(changed.search_string || changed.is_replace_revealed || changed.is_regex || changed.whole_word || changed.match_case || changed.search_scope) {
            return;
        }
        let scope = changed.search_scope.then(|| state.search_scope.clone());
        self.research(editor, state, viewport, changed.move_cursor, scope);
    }

    /// The cursor moved explicitly, by undo or by redo: the next search starts there.
    pub fn cursor_moved(&mut self, editor: &OpenEditor) {
        self.set_start_position(editor.selection().head);
    }

    /// `onDidChangeModelContent` from an edit the model did not make itself.
    pub fn content_changed(&mut self, editor: &mut OpenEditor, state: &mut FindState, viewport: Viewport, change: &ContentChanged) {
        self.track_scopes(change);
        self.set_start_position(editor.selection().head);
        self.research(editor, state, viewport, false, None);
    }

    pub fn find_scopes(&self, editor: &OpenEditor) -> Option<Vec<Range<Position>>> {
        let (lines, eol_len) = (&editor.document().lines, editor.document().eol.len());
        let scopes: Vec<_> = self.scopes.iter().map(|&[start, end]| document_position(lines, eol_len, start)..document_position(lines, eol_len, end)).collect();
        (!scopes.is_empty()).then_some(scopes)
    }

    fn find_scope(&self, editor: &OpenEditor) -> Option<Range<Position>> {
        self.find_scopes(editor).map(|mut scopes| scopes.swap_remove(0))
    }

    fn track_scopes(&mut self, change: &ContentChanged) {
        for scope in &mut self.scopes {
            *scope = track_decoration(*scope, change);
        }
    }

    /// `research`: `new_scope` `Some` replaces the scope decorations, `None` keeps them.
    fn research(&mut self, editor: &mut OpenEditor, state: &mut FindState, viewport: Viewport, move_cursor: bool, new_scope: Option<Option<Vec<Range<Position>>>>) {
        let scopes = match new_scope {
            Some(scopes) => scopes,
            None => self.find_scopes(editor),
        };
        // A multi-line scope covers whole lines; one ending at a line start stops at the line before.
        let document = editor.document();
        let lines = &document.lines;
        let scopes = scopes.map(|scopes| {
            scopes
                .into_iter()
                .map(|scope| {
                    if scope.start.line == scope.end.line {
                        return scope;
                    }
                    let end_line = if scope.end.column == 0 { scope.end.line - 1 } else { scope.end.line };
                    Position::new(scope.start.line, 0)..Position::new(end_line, line_len(lines, end_line))
                })
                .collect::<Vec<_>>()
        });
        let matches = self.find_matches(editor, state, scopes.clone(), false, MATCHES_LIMIT);
        self.set(editor, matches.into_iter().map(|found| found.range).collect(), scopes);

        let selection = editor.selection();
        let selection_range = selection.start()..selection.end();
        let mut position = self.current_matches_position(&selection_range);
        if position == 0 && !self.matches.is_empty() {
            // The match after the selection, 1-based, is the one before it in the count.
            position = self.matches.partition_point(|range| (range.start, range.end) < (selection_range.start, selection_range.end));
        }
        state.change_match_info(position, self.matches.len(), None);
        if move_cursor && self.cursor_move_on_type {
            drop(document);
            self.move_to_next_match_after(editor, state, viewport, self.start_position);
        }
    }

    /// `FindDecorations.set`.
    fn set(&mut self, editor: &OpenEditor, matches: Vec<Range<Position>>, scopes: Option<Vec<Range<Position>>>) {
        self.matches = matches;
        self.highlighted = None;
        self.range_highlight = None;
        let (lines, eol_len) = (&editor.document().lines, editor.document().eol.len());
        self.scopes = scopes.into_iter().flatten().map(|scope| [document_offset(lines, eol_len, scope.start), document_offset(lines, eol_len, scope.end)]).collect();
    }

    /// `getCurrentMatchesPosition`: the 1-based index of the first match touching `range`, else 0.
    fn current_matches_position(&self, range: &Range<Position>) -> usize {
        self.matches.iter().position(|found| found.start <= range.end && range.start <= found.end).map_or(0, |index| index + 1)
    }

    fn set_start_position(&mut self, position: Position) {
        self.start_position = position;
        self.set_current_find_match(None, &[]);
    }

    /// `FindDecorations.setCurrentFindMatch`: highlights `found` and returns its 1-based index, 0 for none.
    fn set_current_find_match(&mut self, found: Option<&Range<Position>>, lines: &[String]) -> usize {
        self.highlighted = found.and_then(|found| self.matches.iter().position(|range| range == found));
        self.range_highlight = self.highlighted.map(|index| {
            let range = self.matches[index].clone();
            // A match ending at a line start highlights the lines up to the one before.
            match range.start.line != range.end.line && range.end.column == 0 {
                true => range.start..Position::new(range.end.line - 1, line_len(lines, range.end.line - 1)),
                false => range,
            }
        });
        self.highlighted.map_or(0, |index| index + 1)
    }

    /// `matchBeforePosition`: the last match ending at or before `position`, else the last one.
    fn match_before_position(&self, position: Position) -> Option<Range<Position>> {
        let before = self.matches.iter().rev().find(|range| range.end.line < position.line || (range.end.line == position.line && range.end.column <= position.column));
        before.or(self.matches.last()).cloned()
    }

    /// `matchAfterPosition`: the first match starting at or after `position`, else the first one.
    fn match_after_position(&self, position: Position) -> Option<Range<Position>> {
        let after = self.matches.iter().find(|range| range.start.line > position.line || (range.start.line == position.line && range.start.column >= position.column));
        after.or(self.matches.first()).cloned()
    }

    /// `_cannotFind`: with no match, the scope is revealed.
    fn cannot_find(&self, editor: &mut OpenEditor, state: &FindState, viewport: Viewport) -> bool {
        if state.matches_count > 0 {
            return false;
        }
        if let Some(scope) = self.find_scope(editor) {
            editor.reveal_range(scope.start, scope.end, VerticalReveal::CenterIfOutsideViewport, viewport);
        }
        true
    }

    /// `_setCurrentFindMatch`: selects the match and reveals it.
    fn select_match(&mut self, editor: &mut OpenEditor, state: &mut FindState, viewport: Viewport, found: Range<Position>) {
        let position = self.set_current_find_match(Some(&found), &editor.document().lines);
        state.change_match_info(position, self.matches.len(), Some(found.clone()));
        editor.set_selections(vec![Selection { anchor: found.start, head: found.end }]);
        editor.reveal_range(found.start, found.end, VerticalReveal::CenterIfOutsideViewport, viewport);
    }

    /// `_prevSearchPosition`: one char back, or the end of the line before with `^` or `$` in a regex.
    fn previous_search_position(state: &FindState, lines: &[String], before: Position) -> Position {
        let line_stops = state.is_regex && (state.search_string.contains('^') || state.search_string.contains('$'));
        if line_stops || before.column == 0 {
            let line = if before.line == 0 { lines.len() - 1 } else { before.line - 1 };
            return Position::new(line, line_len(lines, line));
        }
        Position::new(before.line, before.column - 1)
    }

    /// `_nextSearchPosition`: one char on, or the start of the next line with `^` or `$` in a regex.
    fn next_search_position(state: &FindState, lines: &[String], after: Position) -> Position {
        let line_stops = state.is_regex && (state.search_string.contains('^') || state.search_string.contains('$'));
        if line_stops || after.column == line_len(lines, after.line) {
            let line = if after.line == lines.len() - 1 { 0 } else { after.line + 1 };
            return Position::new(line, 0);
        }
        Position::new(after.line, after.column + 1)
    }

    pub fn move_to_prev_match(&mut self, editor: &mut OpenEditor, state: &mut FindState, viewport: Viewport) {
        let before = editor.selection().start();
        self.move_to_prev_match_before(editor, state, viewport, before, false);
    }

    /// `_moveToPrevMatch`.
    fn move_to_prev_match_before(&mut self, editor: &mut OpenEditor, state: &mut FindState, viewport: Viewport, mut before: Position, is_recursed: bool) {
        if !state.can_navigate_back() {
            if let Some(found) = self.match_after_position(before) {
                self.select_match(editor, state, viewport, found);
            }
            return;
        }
        if self.matches.len() < MATCHES_LIMIT {
            let mut previous = self.match_before_position(before);
            if previous.as_ref().is_some_and(|range| range.is_empty() && range.start == before) {
                before = Self::previous_search_position(state, &editor.document().lines, before);
                previous = self.match_before_position(before);
            }
            if let Some(found) = previous {
                self.select_match(editor, state, viewport, found);
            }
            return;
        }
        if self.cannot_find(editor, state, viewport) {
            return;
        }
        let search_range = self.search_range(editor);
        if search_range.end < before || before < search_range.start {
            before = search_range.end;
        }
        let params = search_params(state);
        let document = editor.document();
        let lines = &document.lines;
        let mut previous = find_previous_match(lines, &params, before, false);
        if previous.as_ref().is_some_and(|found| found.range.is_empty() && found.range.start == before) {
            before = Self::previous_search_position(state, lines, before);
            previous = find_previous_match(lines, &params, before, false);
        }
        drop(document);
        let Some(previous) = previous else { return };
        if !is_recursed && !contains(&search_range, &previous.range) {
            return self.move_to_prev_match_before(editor, state, viewport, previous.range.start, true);
        }
        self.select_match(editor, state, viewport, previous.range);
    }

    pub fn move_to_next_match(&mut self, editor: &mut OpenEditor, state: &mut FindState, viewport: Viewport) {
        let after = editor.selection().end();
        self.move_to_next_match_after(editor, state, viewport, after);
    }

    /// `_moveToNextMatch`.
    fn move_to_next_match_after(&mut self, editor: &mut OpenEditor, state: &mut FindState, viewport: Viewport, mut after: Position) {
        if !state.can_navigate_forward() {
            if let Some(found) = self.match_before_position(after) {
                self.select_match(editor, state, viewport, found);
            }
            return;
        }
        if self.matches.len() < MATCHES_LIMIT {
            let mut next = self.match_after_position(after);
            if next.as_ref().is_some_and(|range| range.is_empty() && range.start == after) {
                after = Self::next_search_position(state, &editor.document().lines, after);
                next = self.match_after_position(after);
            }
            if let Some(found) = next {
                self.select_match(editor, state, viewport, found);
            }
            return;
        }
        if let Some(next) = self.next_match(editor, state, viewport, after, false, true, false) {
            self.select_match(editor, state, viewport, next.range);
        }
    }

    /// `_getNextMatch`: the next match in the search range from `after`, searching the model.
    #[allow(clippy::too_many_arguments)]
    fn next_match(&self, editor: &mut OpenEditor, state: &FindState, viewport: Viewport, mut after: Position, capture_matches: bool, force_move: bool, is_recursed: bool) -> Option<FindMatch> {
        if self.cannot_find(editor, state, viewport) {
            return None;
        }
        let search_range = self.search_range(editor);
        if search_range.end < after || after < search_range.start {
            after = search_range.start;
        }
        let params = search_params(state);
        let document = editor.document();
        let lines = &document.lines;
        let mut next = find_next_match(lines, &params, after, capture_matches);
        if force_move && next.as_ref().is_some_and(|found| found.range.is_empty() && found.range.start == after) {
            after = Self::next_search_position(state, lines, after);
            next = find_next_match(lines, &params, after, capture_matches);
        }
        let next = next?;
        if !is_recursed && !contains(&search_range, &next.range) {
            drop(document);
            return self.next_match(editor, state, viewport, next.range.end, capture_matches, force_move, true);
        }
        Some(next)
    }

    /// `_getSearchRange`: the first scope, else the whole model.
    fn search_range(&self, editor: &OpenEditor) -> Range<Position> {
        self.find_scope(editor).unwrap_or_else(|| Position::default()..last_position(&editor.document().lines))
    }

    /// `moveToMatch`: the match at a 0-based index.
    pub fn move_to_match(&mut self, editor: &mut OpenEditor, state: &mut FindState, viewport: Viewport, index: usize) {
        if let Some(found) = self.matches.get(index).cloned() {
            self.select_match(editor, state, viewport, found);
        }
    }

    fn replace_pattern(state: &FindState) -> ReplacePattern {
        match state.is_regex {
            true => parse_replace_string(&state.replace_string),
            false => ReplacePattern::from_static(&state.replace_string),
        }
    }

    /// `replace`: replaces the selected match and moves on, or selects the next match first.
    pub fn replace(&mut self, editor: &mut OpenEditor, state: &mut FindState, viewport: Viewport) -> Option<ContentChanged> {
        if state.matches_count == 0 {
            return None;
        }
        let pattern = Self::replace_pattern(state);
        let selection = editor.selection();
        let next = self.next_match(editor, state, viewport, selection.start(), true, false, false)?;
        if selection.start() != next.range.start || selection.end() != next.range.end {
            self.set_start_position(editor.selection().head);
            self.select_match(editor, state, viewport, next.range);
            return None;
        }
        let replace_string = pattern.build(next.matches.as_deref(), state.preserve_case);
        let (start, end) = (selection.start(), selection.end());
        let caret = end_of(start, &replace_string);
        let change = self.execute(editor, viewport, |editor| editor.execute_edits(vec![(start, end, replace_string.clone(), false)], Some(vec![Selection::caret(caret)]), REPLACE));
        // Upstream adds the replace string's length to the start column, also for a multi-line one.
        self.set_start_position(Position::new(start.line, start.column + replace_string.chars().count()));
        self.research(editor, state, viewport, true, None);
        change
    }

    /// `_findMatches`: matches in the scopes, else in the whole model.
    fn find_matches(&self, editor: &OpenEditor, state: &FindState, scopes: Option<Vec<Range<Position>>>, capture_matches: bool, limit: usize) -> Vec<FindMatch> {
        let lines = &editor.document().lines;
        let ranges = scopes.unwrap_or_else(|| vec![Position::default()..last_position(lines)]);
        find_matches_in_ranges(lines, &search_params(state), ranges, capture_matches, limit)
    }

    /// `replaceAll`: without a find scope and with `MATCHES_LIMIT` matches or more `_largeReplaceAll`, else
    /// `_regularReplaceAll`: one edit per match, the primary selection tracked.
    pub fn replace_all(&mut self, editor: &mut OpenEditor, state: &mut FindState, viewport: Viewport) -> Option<ContentChanged> {
        if state.matches_count == 0 {
            return None;
        }
        let pattern = Self::replace_pattern(state);
        let scopes = self.find_scopes(editor);
        let large = scopes.is_none() && state.matches_count >= MATCHES_LIMIT;
        let matches = self.find_matches(editor, state, scopes, pattern.has_replacement_patterns() || state.preserve_case, usize::MAX);
        if large {
            let change = self.execute(editor, viewport, |editor| large_replace_all(editor, &pattern, matches, state.preserve_case));
            self.research(editor, state, viewport, false, None);
            return change;
        }
        let mut edits: Vec<(Position, Position, String)> = Vec::new();
        // `ReplaceAllCommand` joins matches that touch into one edit.
        for found in matches {
            let text = pattern.build(found.matches.as_deref(), state.preserve_case);
            match edits.last_mut() {
                Some(last) if last.1 == found.range.start => {
                    last.1 = found.range.end;
                    last.2.push_str(&text);
                }
                _ => edits.push((found.range.start, found.range.end, text)),
            }
        }
        let change = self.execute(editor, viewport, |editor| editor.execute_edits_tracking_primary(edits, REPLACE_ALL));
        self.research(editor, state, viewport, false, None);
        change
    }

    /// `selectAllMatches`: a cursor per match, the selected match staying primary.
    pub fn select_all_matches(&mut self, editor: &mut OpenEditor, state: &FindState) {
        if state.matches_count == 0 {
            return;
        }
        let matches = self.find_matches(editor, state, self.find_scopes(editor), false, usize::MAX);
        let mut selections: Vec<Selection> = matches.into_iter().map(|found| Selection { anchor: found.range.start, head: found.range.end }).collect();
        let primary = editor.selection();
        if let Some(index) = selections.iter().position(|selection| selection.start() == primary.start() && selection.end() == primary.end()) {
            selections.remove(index);
            selections.insert(0, primary);
        }
        editor.set_selections(selections);
    }

    /// `_executeEditorCommand`: the edit between undo stops; the scopes follow it, the start position does not.
    /// cursor.ts `_executeEdit` reveals the cursors when the edit moved them, before the research reveals the match.
    fn execute(&mut self, editor: &mut OpenEditor, viewport: Viewport, edit: impl FnOnce(&mut OpenEditor) -> Option<ContentChanged>) -> Option<ContentChanged> {
        let selections = editor.selections.clone();
        editor.push_undo_stop();
        let change = edit(editor);
        editor.push_undo_stop();
        if let Some(change) = &change {
            self.track_scopes(change);
        }
        if editor.selections != selections {
            editor.reveal_cursors("keyboard", viewport);
        }
        change
    }
}

/// `_largeReplaceAll`: the whole text with every match replaced, as one edit that keeps the primary selection
/// (`ReplaceCommandThatPreservesSelection`).
fn large_replace_all(editor: &mut OpenEditor, pattern: &ReplacePattern, matches: Vec<FindMatch>, preserve_case: bool) -> Option<ContentChanged> {
    let document = editor.document();
    let lines = &document.lines;
    let end = last_position(lines);
    let mut text = String::new();
    let mut copied_to = Position::default();
    for found in matches {
        text.push_str(&text_between(lines, copied_to, found.range.start));
        text.push_str(&pattern.build(found.matches.as_deref(), preserve_case));
        copied_to = found.range.end;
    }
    text.push_str(&text_between(lines, copied_to, end));
    let selection = editor.selection();
    drop(document);
    editor.execute_edits(vec![(Position::default(), end, text, false)], Some(vec![selection]), REPLACE_ALL)
}

const REPLACE: EditReason = EditReason::Cursor { kind: "executeCommands", detailed_source: "replace" };
const REPLACE_ALL: EditReason = EditReason::Cursor { kind: "executeCommands", detailed_source: "replaceAll" };

/// The model search the state asks for; whole words use `editor.wordSeparators`.
pub fn search_params(state: &FindState) -> SearchParams<'_> {
    SearchParams { search_string: &state.search_string, is_regex: state.is_regex, match_case: state.match_case, word_separators: state.whole_word.then_some(WORD_SEPARATORS) }
}

/// `Range.containsRange`.
fn contains(outer: &Range<Position>, inner: &Range<Position>) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

#[cfg(test)]
mod tests;
