//! multicursor.ts `MultiCursorSession` and `MultiCursorSelectionController`: Add Selection to Next Find Match
//! (Cmd+D), its previous and move variants, Select All Occurrences and Change All Occurrences.
//! Upstream ends a session from event listeners; here `end_session_if_stale` checks the same conditions when the
//! workbench sees a cursor move and before each action, and the workbench calls `end_session` on a text blur.

use std::ops::Range;

use super::controller::FindController;
use super::search::{FindMatch, SearchParams, find_matches_in_ranges, find_next_match, find_previous_match};
use super::state::NewFindState;
use crate::editor::cursor::last_position;
use crate::editor::edit::text_between;
use crate::editor::view::VerticalReveal;
use crate::editor::{OpenEditor, Position, Selection, Viewport, WORD_SEPARATORS, word_at};

/// What the actions read outside the find controller: `editor.hasTextFocus()` and `editor.selectedTextMatchMode`.
#[derive(Clone, Debug)]
pub struct MultiCursorContext {
    pub text_focused: bool,
    pub match_mode: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MultiCursorSession {
    /// An empty cursor started it: the find state searches whole words, case-sensitively, without regex, until it ends.
    disconnected: bool,
    uses_find_options: bool,
    search_text: String,
    whole_word: bool,
    match_case: bool,
    current_match: Option<Selection>,
    /// What the session last saw; a change to any of them is an event that ends it upstream.
    model_id: u32,
    version_id: u32,
    selections: Vec<Selection>,
    find_options: (bool, bool),
    match_mode: String,
}

/// `isFindWidgetSearch`: the find widget owns the search while it has the focus, is shown and has a search string.
fn is_find_widget_search(find: &FindController, context: &MultiCursorContext) -> bool {
    !context.text_focused && find.state.is_revealed && !find.state.search_string.is_empty()
}

/// `getSelectionSearchOptions`: (whole word, match case, uses find options).
fn selection_search_options(find: &FindController, match_mode: &str) -> (bool, bool, bool) {
    match match_mode {
        "findOptions" => (find.state.whole_word, find.state.match_case, true),
        mode => (false, mode == "caseSensitive", false),
    }
}

/// `MultiCursorSession.create`: `None` for an empty selection outside a word.
fn create_session(editor: &OpenEditor, find: &FindController, context: &MultiCursorContext) -> Option<MultiCursorSession> {
    let new_session = |disconnected, uses_find_options, search_text, whole_word, match_case, current_match| MultiCursorSession {
        disconnected,
        uses_find_options,
        search_text,
        whole_word,
        match_case,
        current_match,
        model_id: editor.model_id,
        version_id: editor.version_id,
        selections: editor.selections.clone(),
        find_options: (find.state.match_case, find.state.whole_word),
        match_mode: context.match_mode.clone(),
    };
    if is_find_widget_search(find, context) {
        return Some(new_session(false, true, find.state.search_string.clone(), find.state.whole_word, find.state.match_case, None));
    }
    let disconnected = editor.selections.len() == 1 && editor.selection().is_empty();
    let (whole_word, match_case, uses_find_options) = match disconnected {
        true => (true, true, true),
        false => selection_search_options(find, &context.match_mode),
    };
    let selection = editor.selection();
    if !selection.is_empty() {
        let text = text_between(&editor.document.lines, selection.start(), selection.end());
        return Some(new_session(disconnected, uses_find_options, text, whole_word, match_case, None));
    }
    let line = &editor.document.lines[selection.start().line];
    let word = word_at(line, selection.start().column)?;
    let text = line.chars().skip(word.start).take(word.len()).collect();
    let current = Selection { anchor: Position::new(selection.start().line, word.start), head: Position::new(selection.start().line, word.end) };
    Some(new_session(disconnected, uses_find_options, text, whole_word, match_case, Some(current)))
}

impl MultiCursorSession {
    fn params(&self) -> SearchParams<'_> {
        SearchParams { search_string: &self.search_text, is_regex: false, match_case: self.match_case, word_separators: self.whole_word.then_some(WORD_SEPARATORS) }
    }

    /// `_getNextMatch` and `_getPreviousMatch`: the word under an empty cursor first, then the match after the last
    /// added selection's end or before its start.
    fn next_match(&mut self, editor: &OpenEditor, next: bool) -> Option<Selection> {
        if let Some(current) = self.current_match.take() {
            return Some(current);
        }
        let last = editor.selections[editor.selections.len() - 1];
        let lines = &editor.document.lines;
        let found = match next {
            true => find_next_match(lines, &self.params(), last.end(), false),
            false => find_previous_match(lines, &self.params(), last.start(), false),
        }?;
        Some(Selection { anchor: found.range.start, head: found.range.end })
    }
}

/// `_beginSessionIfNeeded`: a new session puts its text in the find state, with the overrides when it is disconnected.
fn begin_session_if_needed(find: &mut FindController, editor: &mut OpenEditor, viewport: Viewport, context: &MultiCursorContext) {
    if find.multicursor.is_some() {
        return;
    }
    let Some(session) = create_session(editor, find, context) else { return };
    let new = NewFindState { search_string: Some(session.search_text.clone()), override_options: session.disconnected, ..Default::default() };
    find.change(editor, viewport, new, false);
    find.multicursor = Some(MultiCursorSession { find_options: (find.state.match_case, find.state.whole_word), ..session });
}

/// The events that end a session upstream: a selection change or an edit it did not make, another model, a match
/// case or whole word change for a session on the find options, a `selectedTextMatchMode` change.
pub fn end_session_if_stale(find: &mut FindController, editor: &mut OpenEditor, viewport: Viewport, match_mode: &str) {
    let Some(session) = &find.multicursor else { return };
    let options_changed = session.uses_find_options && session.find_options != (find.state.match_case, find.state.whole_word);
    let moved = (session.model_id, session.version_id, &session.selections) != (editor.model_id, editor.version_id, &editor.selections);
    if moved || options_changed || session.match_mode != match_mode {
        end_session(find, editor, viewport);
    }
}

/// `_endSession`: a disconnected session gives the find state its own options back.
pub fn end_session(find: &mut FindController, editor: &mut OpenEditor, viewport: Viewport) {
    if find.multicursor.take().is_some_and(|session| session.disconnected) {
        find.change(editor, viewport, NewFindState::default(), false);
    }
}

/// `_setSelections` while `_ignoreSelectionChange`: the session keeps going.
fn set_selections(find: &mut FindController, editor: &mut OpenEditor, selections: Vec<Selection>) {
    editor.set_selections(selections);
    if let Some(session) = &mut find.multicursor {
        session.selections = editor.selections.clone();
    }
}

/// `addSelectionToNextFindMatch`, `addSelectionToPreviousFindMatch`, `moveSelectionToNextFindMatch` and
/// `moveSelectionToPreviousFindMatch`: `add` keeps the last selection, else the match replaces it. The match is
/// revealed in the center when it is outside the viewport.
pub fn selection_to_find_match(find: &mut FindController, editor: &mut OpenEditor, viewport: Viewport, context: &MultiCursorContext, next: bool, add: bool) {
    end_session_if_stale(find, editor, viewport, &context.match_mode);
    // Several selections holding different text, outside a session: Cmd+D only expands the empty ones to their word.
    if next && add && find.multicursor.is_none() && editor.selections.len() > 1 {
        let match_case = match is_find_widget_search(find, context) {
            true => find.state.match_case,
            false => selection_search_options(find, &context.match_mode).1,
        };
        if !selections_contain_same_text(editor, match_case) {
            let expanded = editor.selections.iter().map(|selection| expand_empty_to_word(editor, *selection)).collect();
            editor.set_selections(expanded);
            return;
        }
    }
    begin_session_if_needed(find, editor, viewport, context);
    let Some(session) = &mut find.multicursor else { return };
    let Some(found) = session.next_match(editor, next) else { return };
    let mut selections = editor.selections.clone();
    if !add {
        selections.pop();
    }
    selections.push(found);
    set_selections(find, editor, selections);
    editor.reveal_range(found.start(), found.end(), VerticalReveal::CenterIfOutsideViewport, viewport);
}

/// `selectAll` (Select All Occurrences, Change All Occurrences): a regex search of the shown find widget, else the
/// session's text; the match touching the primary selection becomes the primary one.
pub fn select_all(find: &mut FindController, editor: &mut OpenEditor, viewport: Viewport, context: &MultiCursorContext) {
    end_session_if_stale(find, editor, viewport, &context.match_mode);
    let allow_find_regex = context.match_mode == "findOptions" || !context.text_focused;
    let state = &find.state;
    let find_regex = allow_find_regex && state.is_revealed && !state.search_string.is_empty() && state.is_regex;
    if !find_regex {
        begin_session_if_needed(find, editor, viewport, context);
    }
    let state = &find.state;
    let lines = &editor.document.lines;
    let scope = state.search_scope.clone().unwrap_or_else(|| vec![Position::default()..last_position(lines)]);
    let mut matches = match (&find.multicursor, find_regex) {
        (_, true) => {
            let params = SearchParams { search_string: &state.search_string, is_regex: true, match_case: state.match_case, word_separators: state.whole_word.then_some(WORD_SEPARATORS) };
            find_matches_in_ranges(lines, &params, scope, false, usize::MAX)
        }
        (Some(session), false) => find_matches_in_ranges(lines, &session.params(), scope, false, usize::MAX),
        (None, false) => return,
    };
    if matches.is_empty() {
        return;
    }
    let primary = editor.selection();
    // `Range.intersectRanges`: ranges that only touch intersect too.
    if let Some(index) = matches.iter().position(|found| found.range.start.max(primary.start()) <= found.range.end.min(primary.end())) {
        matches.swap(0, index);
    }
    set_selections(find, editor, matches.into_iter().map(|FindMatch { range, .. }| Selection { anchor: range.start, head: range.end }).collect());
}

/// `modelRangesContainSameText`: every selection after the first is non-empty and holds the first one's text.
fn selections_contain_same_text(editor: &OpenEditor, match_case: bool) -> bool {
    let text = |selection: &Selection| {
        let text = text_between(&editor.document.lines, selection.start(), selection.end());
        if match_case { text } else { text.to_lowercase() }
    };
    let first = text(&editor.selections[0]);
    editor.selections[1..].iter().all(|selection| !selection.is_empty() && text(selection) == first)
}

/// `_expandEmptyToWord`.
fn expand_empty_to_word(editor: &OpenEditor, selection: Selection) -> Selection {
    if !selection.is_empty() {
        return selection;
    }
    let line = selection.head.line;
    match word_at(&editor.document.lines[line], selection.head.column) {
        Some(word) => Selection { anchor: Position::new(line, word.start), head: Position::new(line, word.end) },
        None => selection,
    }
}

/// `editor.selectionHighlight`, `editor.selectionHighlightMultiline` and `editor.selectionHighlightMaxLength`.
#[derive(Clone, Copy, Debug)]
pub struct SelectionHighlightOptions {
    pub enabled: bool,
    pub multiline: bool,
    pub max_length: usize,
}

/// What `SelectionHighlighterState` searches for.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectionHighlightQuery {
    search_text: String,
    match_case: bool,
    whole_word: bool,
}

/// `SelectionHighlighter._createState`: the session's search, or the one a new session would make, unless the
/// selection is empty, blank, too long, or the shown find widget highlights the same matches.
pub fn selection_highlight_query(find: &FindController, editor: &OpenEditor, context: &MultiCursorContext, options: SelectionHighlightOptions) -> Option<SelectionHighlightQuery> {
    // multicursor.ts `_update`: searching the whole of a model too large for tokenization would block the UI.
    if !options.enabled || editor.document.too_large_for_tokenization {
        return None;
    }
    let selection = editor.selection();
    if !options.multiline && selection.start().line != selection.end().line {
        return None;
    }
    let state = &find.state;
    let created;
    let session = match &find.multicursor {
        Some(session) => session,
        None => {
            if editor.selections.len() > 1 && !selections_contain_same_text(editor, state.match_case) {
                return None;
            }
            created = create_session(editor, find, context)?;
            &created
        }
    };
    if session.current_match.is_some() || session.search_text.chars().all(|c| c == ' ' || c == '\t') {
        return None;
    }
    if options.max_length > 0 && session.search_text.encode_utf16().count() > options.max_length {
        return None;
    }
    if state.is_revealed {
        let fold = |text: &str| if state.match_case { text.to_string() } else { text.to_lowercase() };
        if fold(&state.search_string) == fold(&session.search_text) && session.match_case == state.match_case && session.whole_word == state.whole_word && !state.is_regex {
            return None;
        }
    }
    Some(SelectionHighlightQuery { search_text: session.search_text.clone(), match_case: session.match_case, whole_word: session.whole_word })
}

/// `SelectionHighlighterState.findMatches` and `_setState`: the first `LIMIT_FIND_COUNT` (999) matches, without
/// those equal to a selection or overlapping a non-empty one. None in a model too large for tokenization.
pub fn selection_highlight_ranges(editor: &OpenEditor, query: &SelectionHighlightQuery) -> Vec<Range<Position>> {
    let lines = &editor.document.lines;
    if lines.len() > 300_000 || lines.iter().map(|line| line.len() + 1).sum::<usize>() > 20 * 1024 * 1024 {
        return Vec::new();
    }
    let params = SearchParams { search_string: &query.search_text, is_regex: false, match_case: query.match_case, word_separators: query.whole_word.then_some(WORD_SEPARATORS) };
    let all = find_matches_in_ranges(lines, &params, vec![Position::default()..last_position(lines)], false, 999);
    let mut selections: Vec<Range<Position>> = editor.selections.iter().map(|selection| selection.start()..selection.end()).collect();
    selections.sort_by_key(|range| (range.start, range.end));
    let mut matches = Vec::new();
    let mut j = 0;
    for found in all {
        let range = found.range;
        while j < selections.len() && (selections[j].start, selections[j].end) < (range.start, range.end) {
            j += 1;
        }
        if selections.get(j) == Some(&range) {
            j += 1;
            continue;
        }
        // `Range.areIntersecting`: ranges that only touch do not intersect.
        let overlaps = |selection: &Range<Position>| selection.start != selection.end && selection.start < range.end && range.start < selection.end;
        if !selections.get(j).is_some_and(overlaps) {
            matches.push(range);
        }
    }
    matches
}

#[cfg(test)]
mod tests;
