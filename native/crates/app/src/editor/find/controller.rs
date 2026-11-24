//! findController.ts `CommonFindController` with the find actions' run functions: starting the find,
//! seeding the search string, next and previous match, the toggles, replace and the scope.
//! The widget owns focus and tells the controller whether the find input has it.

use super::model::FindModel;
use super::search::escape_regexp_characters;
use super::state::{FindState, FindStateChanged, NewFindState};
use crate::editor::cursor::line_len;
use crate::editor::edit::text_between;
use crate::editor::{ContentChanged, OpenEditor, Position, Viewport, word_at};

/// `SEARCH_STRING_MAX_LENGTH`.
const SEARCH_STRING_MAX_LENGTH: usize = 524288;

/// `editor.find.seedSearchStringFromSelection`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SeedFromSelection {
    Never,
    Always,
    Selection,
}

/// `editor.find.autoFindInSelection`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AutoFindInSelection {
    Never,
    Always,
    Multiline,
}

/// `editor.find` options the controller reads. `globalFindClipboard` (the macOS find pasteboard) is
/// not ported: it defaults to off.
#[derive(Clone, Copy, Debug)]
pub struct FindOptions {
    pub seed_search_string_from_selection: SeedFromSelection,
    pub auto_find_in_selection: AutoFindInSelection,
    pub is_loop: bool,
    pub close_on_result: bool,
}

impl Default for FindOptions {
    fn default() -> Self {
        FindOptions { seed_search_string_from_selection: SeedFromSelection::Always, auto_find_in_selection: AutoFindInSelection::Never, is_loop: true, close_on_result: false }
    }
}

/// `IFindStartOptions.seedSearchStringFromSelection`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seed {
    None,
    Single,
    Multiple,
}

/// `FindStartFocusAction`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FocusAction {
    NoFocusChange,
    FocusFindInput,
    FocusReplaceInput,
}

/// `IFindStartOptions` without `shouldAnimate`, which only the widget reads, and the clipboard seed.
#[derive(Clone, Copy, Debug)]
pub struct StartOptions {
    pub force_reveal_replace: bool,
    pub seed_search_string_from_selection: Seed,
    pub seed_search_string_from_non_empty_selection: bool,
    pub should_focus: FocusAction,
    pub update_search_scope: bool,
    pub is_loop: bool,
}

/// The options persisted per workspace as `editor.matchCase`, `editor.wholeWord`, `editor.isRegex`
/// and `editor.preserveCase` (`saveQueryState`, `loadQueryState`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QueryOptions {
    pub match_case: bool,
    pub whole_word: bool,
    pub is_regex: bool,
    pub preserve_case: bool,
}

/// `getSelectionSearchString`: a single-line selection, or the word at an empty cursor.
pub fn selection_search_string(editor: &OpenEditor, seed: Seed, non_empty_only: bool) -> Option<String> {
    let selection = editor.selection();
    let (start, end) = (selection.start(), selection.end());
    if !(seed == Seed::Single && start.line == end.line || seed == Seed::Multiple) {
        return None;
    }
    let lines = &editor.document.lines;
    if selection.is_empty() {
        let word = word_at(&lines[start.line], start.column).filter(|_| !non_empty_only)?;
        return Some(lines[start.line].chars().skip(word.start).take(word.len()).collect());
    }
    // shortcut: lines join with "\n", not the model EOL; the port's search runs on "\n"-joined text.
    let text = text_between(lines, start, end);
    (text.encode_utf16().count() < SEARCH_STRING_MAX_LENGTH).then_some(text)
}

/// `CommonFindController` and `FindController._start`.
#[derive(Debug)]
pub struct FindController {
    pub state: FindState,
    pub model: Option<FindModel>,
    pub options: FindOptions,
    /// `findInputFocussed`, set by the widget.
    pub find_input_focused: bool,
}

impl FindController {
    /// The controller with the stored query options (`loadQueryState`).
    pub fn new(options: FindOptions, query: QueryOptions) -> Self {
        let mut controller = FindController { state: FindState::default(), model: None, options, find_input_focused: false };
        controller.state.change(query_change(query), false, true);
        controller
    }

    /// The options to persist after a change (`saveQueryState`).
    pub fn query_options(&self) -> QueryOptions {
        QueryOptions { match_case: self.state.match_case, whole_word: self.state.whole_word, is_regex: self.state.is_regex, preserve_case: self.state.preserve_case }
    }

    /// `state.change` and its listeners: the model researches, and a hidden widget drops the model.
    pub fn change(&mut self, editor: &mut OpenEditor, viewport: Viewport, new: NewFindState, move_cursor: bool) -> Option<FindStateChanged> {
        let changed = self.state.change(new, move_cursor, true)?;
        if changed.is_revealed && !self.state.is_revealed {
            self.model = None;
        }
        if let Some(model) = &mut self.model {
            model.state_changed(editor, &mut self.state, viewport, changed);
        }
        Some(changed)
    }

    /// `onDidChangeModel`: the scope clears, the options reload, and a visible find restarts.
    pub fn editor_changed(&mut self, editor: &mut OpenEditor, viewport: Viewport, query: QueryOptions) {
        let restart = self.state.is_revealed;
        self.model = None;
        self.change(editor, viewport, NewFindState { search_scope: Some(None), ..query_change(query) }, false);
        if restart {
            let opts = StartOptions { force_reveal_replace: false, seed_search_string_from_selection: Seed::None, seed_search_string_from_non_empty_selection: false, should_focus: FocusAction::NoFocusChange, update_search_scope: false, is_loop: self.options.is_loop };
            self.start(editor, viewport, opts, NewFindState::default());
        }
    }

    /// A cursor move the user or a command made: the model's next search starts there.
    pub fn cursor_moved(&mut self, editor: &OpenEditor) {
        if let Some(model) = &mut self.model {
            model.cursor_moved(editor);
        }
    }

    /// An edit the model did not make: the matches follow the text.
    pub fn content_changed(&mut self, editor: &mut OpenEditor, viewport: Viewport, change: &ContentChanged) {
        if let Some(model) = &mut self.model {
            model.content_changed(editor, &mut self.state, viewport, change);
        }
    }

    pub fn close_find_widget(&mut self, editor: &mut OpenEditor, viewport: Viewport) {
        self.change(editor, viewport, NewFindState { is_revealed: Some(false), search_scope: Some(None), ..Default::default() }, false);
    }

    pub fn toggle_case_sensitive(&mut self, editor: &mut OpenEditor, viewport: Viewport) {
        let match_case = !self.state.match_case;
        self.change(editor, viewport, NewFindState { match_case: Some(match_case), ..Default::default() }, false);
    }

    pub fn toggle_whole_words(&mut self, editor: &mut OpenEditor, viewport: Viewport) {
        let whole_word = !self.state.whole_word;
        self.change(editor, viewport, NewFindState { whole_word: Some(whole_word), ..Default::default() }, false);
    }

    pub fn toggle_regex(&mut self, editor: &mut OpenEditor, viewport: Viewport) {
        let is_regex = !self.state.is_regex;
        self.change(editor, viewport, NewFindState { is_regex: Some(is_regex), ..Default::default() }, false);
    }

    pub fn toggle_preserve_case(&mut self, editor: &mut OpenEditor, viewport: Viewport) {
        let preserve_case = !self.state.preserve_case;
        self.change(editor, viewport, NewFindState { preserve_case: Some(preserve_case), ..Default::default() }, false);
    }

    /// `toggleSearchScope`: off, or the non-empty selections, one ending at a line start stopping at the line before.
    pub fn toggle_search_scope(&mut self, editor: &mut OpenEditor, viewport: Viewport) {
        if self.state.search_scope.is_some() {
            self.change(editor, viewport, NewFindState { search_scope: Some(None), ..Default::default() }, true);
            return;
        }
        let lines = &editor.document.lines;
        let mut scopes = Vec::new();
        for selection in &editor.selections {
            let (start, mut end) = (selection.start(), selection.end());
            if end.column == 0 && end.line > start.line {
                end = Position::new(end.line - 1, line_len(lines, end.line - 1));
            }
            if start != end {
                scopes.push(start..end);
            }
        }
        if !scopes.is_empty() {
            self.change(editor, viewport, NewFindState { search_scope: Some(Some(scopes)), ..Default::default() }, true);
        }
    }

    /// `setSearchString`: escaped when the search is a regex.
    pub fn set_search_string(&mut self, editor: &mut OpenEditor, viewport: Viewport, search_string: &str) {
        let search_string = if self.state.is_regex { escape_regexp_characters(search_string) } else { search_string.to_string() };
        self.change(editor, viewport, NewFindState { search_string: Some(search_string), ..Default::default() }, false);
    }

    /// `_start`: reveals the widget, seeds the search string, sets the scope and binds the model.
    pub fn start(&mut self, editor: &mut OpenEditor, viewport: Viewport, mut opts: StartOptions, new: NewFindState) {
        let selection = editor.selection();
        opts.update_search_scope |= match self.options.auto_find_in_selection {
            AutoFindInSelection::Always => true,
            AutoFindInSelection::Never => false,
            AutoFindInSelection::Multiline => selection.start().line != selection.end().line,
        };
        self.model = None;
        let mut changes = NewFindState { is_revealed: Some(true), ..new };
        match opts.seed_search_string_from_selection {
            Seed::Single => {
                if let Some(seed) = selection_search_string(editor, Seed::Single, opts.seed_search_string_from_non_empty_selection).filter(|seed| !seed.is_empty()) {
                    changes.search_string = Some(if self.state.is_regex { escape_regexp_characters(&seed) } else { seed });
                }
            }
            Seed::Multiple if !opts.update_search_scope => {
                if let Some(seed) = selection_search_string(editor, Seed::Multiple, false).filter(|seed| !seed.is_empty()) {
                    changes.search_string = Some(seed);
                }
            }
            _ => {}
        }
        if opts.force_reveal_replace || changes.is_replace_revealed == Some(true) {
            changes.is_replace_revealed = Some(true);
        } else if !self.state.is_revealed {
            changes.is_replace_revealed = Some(false);
        }
        if opts.update_search_scope && editor.selections.iter().any(|selection| !selection.is_empty()) {
            changes.search_scope = Some(Some(editor.selections.iter().map(|selection| selection.start()..selection.end()).collect()));
        }
        changes.is_loop = Some(opts.is_loop);
        self.change(editor, viewport, changes, false);
        self.model = Some(FindModel::new(editor, &mut self.state, viewport));
    }

    /// `StartFindAction` (Cmd+F): the find input gets focus.
    pub fn start_find(&mut self, editor: &mut OpenEditor, viewport: Viewport) -> FocusAction {
        let opts = StartOptions { should_focus: FocusAction::FocusFindInput, ..self.seeded_start(false) };
        self.start(editor, viewport, opts, NewFindState::default());
        opts.should_focus
    }

    /// `StartFindReplaceAction` (Alt+Cmd+F): the replace input gets focus when the find input has a
    /// search string from it or from the selection.
    pub fn start_find_replace(&mut self, editor: &mut OpenEditor, viewport: Viewport) -> FocusAction {
        let selection = editor.selection();
        let seed = !selection.is_empty() && selection.start().line == selection.end().line && self.options.seed_search_string_from_selection != SeedFromSelection::Never && !self.find_input_focused;
        let should_focus = if self.find_input_focused || seed { FocusAction::FocusReplaceInput } else { FocusAction::FocusFindInput };
        let opts = StartOptions {
            force_reveal_replace: true,
            seed_search_string_from_selection: if seed { Seed::Single } else { Seed::None },
            should_focus,
            ..self.seeded_start(false)
        };
        self.start(editor, viewport, opts, NewFindState::default());
        should_focus
    }

    /// `StartFindWithSelectionAction` (Cmd+E): the selection, multi-line too, becomes the search string.
    pub fn start_find_with_selection(&mut self, editor: &mut OpenEditor, viewport: Viewport) {
        let opts = StartOptions { force_reveal_replace: false, seed_search_string_from_selection: Seed::Multiple, seed_search_string_from_non_empty_selection: false, should_focus: FocusAction::NoFocusChange, update_search_scope: false, is_loop: self.options.is_loop };
        self.start(editor, viewport, opts, NewFindState::default());
    }

    /// The start options of the find actions: seeding from the selection when `only_when_empty` finds no search string yet.
    fn seeded_start(&self, only_when_empty: bool) -> StartOptions {
        let seed = (!only_when_empty || self.state.search_string.is_empty()) && self.options.seed_search_string_from_selection != SeedFromSelection::Never;
        StartOptions {
            force_reveal_replace: false,
            seed_search_string_from_selection: if seed { Seed::Single } else { Seed::None },
            seed_search_string_from_non_empty_selection: self.options.seed_search_string_from_selection == SeedFromSelection::Selection,
            should_focus: FocusAction::NoFocusChange,
            update_search_scope: false,
            is_loop: self.options.is_loop,
        }
    }

    /// `matchFindAction` (Cmd+G, Shift+Cmd+G): without a model, starts the find first, unfocused.
    pub fn match_find_action(&mut self, editor: &mut OpenEditor, viewport: Viewport, next: bool) {
        let was_visible = self.state.is_revealed;
        if self.run_match(editor, viewport, next, was_visible) {
            return;
        }
        let opts = self.seeded_start(true);
        self.start(editor, viewport, opts, NewFindState::default());
        self.run_match(editor, viewport, next, was_visible);
    }

    /// One step of `matchFindAction`: whether the selection landed on a match.
    fn run_match(&mut self, editor: &mut OpenEditor, viewport: Viewport, next: bool, was_visible: bool) -> bool {
        let previous = editor.selection();
        if !self.move_to_match(editor, viewport, next) || editor.selection() == previous {
            return false;
        }
        editor.push_undo_stop();
        if self.options.close_on_result && was_visible && self.find_input_focused {
            self.close_find_widget(editor, viewport);
        }
        true
    }

    /// `moveToNextMatch` / `moveToPrevMatch`: false without a model.
    fn move_to_match(&mut self, editor: &mut OpenEditor, viewport: Viewport, next: bool) -> bool {
        let Some(model) = &mut self.model else { return false };
        match next {
            true => model.move_to_next_match(editor, &mut self.state, viewport),
            false => model.move_to_prev_match(editor, &mut self.state, viewport),
        }
        true
    }

    /// `SelectionMatchFindAction` (Cmd+F3, Shift+Cmd+F3): the selection or word becomes the search string.
    pub fn selection_match_find_action(&mut self, editor: &mut OpenEditor, viewport: Viewport, next: bool) {
        if let Some(seed) = selection_search_string(editor, Seed::Single, false).filter(|seed| !seed.is_empty()) {
            self.set_search_string(editor, viewport, &seed);
        }
        if self.move_to_match(editor, viewport, next) {
            return;
        }
        let opts = StartOptions { seed_search_string_from_selection: Seed::None, seed_search_string_from_non_empty_selection: false, ..self.seeded_start(false) };
        self.start(editor, viewport, opts, NewFindState::default());
        self.move_to_match(editor, viewport, next);
    }

    /// `goToMatch`: the match at a 0-based index.
    pub fn go_to_match(&mut self, editor: &mut OpenEditor, viewport: Viewport, index: usize) {
        if let Some(model) = &mut self.model {
            model.move_to_match(editor, &mut self.state, viewport, index);
        }
    }

    pub fn replace(&mut self, editor: &mut OpenEditor, viewport: Viewport) -> Option<ContentChanged> {
        self.model.as_mut()?.replace(editor, &mut self.state, viewport)
    }

    pub fn replace_all(&mut self, editor: &mut OpenEditor, viewport: Viewport) -> Option<ContentChanged> {
        self.model.as_mut()?.replace_all(editor, &mut self.state, viewport)
    }

    pub fn select_all_matches(&mut self, editor: &mut OpenEditor) {
        if let Some(model) = &mut self.model {
            model.select_all_matches(editor, &self.state);
        }
    }

}

fn query_change(query: QueryOptions) -> NewFindState {
    NewFindState { match_case: Some(query.match_case), whole_word: Some(query.whole_word), is_regex: Some(query.is_regex), preserve_case: Some(query.preserve_case), ..Default::default() }
}

#[cfg(test)]
mod tests;
