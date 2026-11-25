//! findState.ts: what the find widget searches for and how, and the match count it shows.
//! `isSearching`, `filters` and the preserve case override are left out: only notebooks and the search
//! editor set them.

use std::ops::Range;

use crate::editor::Position;

/// `FindReplaceState`.
#[derive(Clone, Debug, PartialEq)]
pub struct FindState {
    pub search_string: String,
    pub replace_string: String,
    pub is_revealed: bool,
    pub is_replace_revealed: bool,
    /// The effective options, the overrides applied (the `isRegex`, `wholeWord`, `matchCase` getters).
    pub is_regex: bool,
    pub whole_word: bool,
    pub match_case: bool,
    pub preserve_case: bool,
    /// The user's (is_regex, whole_word, match_case) while a multi-cursor session overrides them.
    overridden: Option<(bool, bool, bool)>,
    pub search_scope: Option<Vec<Range<Position>>>,
    /// The 1-based index of the current match, 0 when the selection is on none.
    pub matches_position: usize,
    pub matches_count: usize,
    pub current_match: Option<Range<Position>>,
    /// `editor.find.loop`: next and previous wrap around.
    pub is_loop: bool,
}

impl Default for FindState {
    fn default() -> Self {
        FindState {
            search_string: String::new(),
            replace_string: String::new(),
            is_revealed: false,
            is_replace_revealed: false,
            is_regex: false,
            whole_word: false,
            match_case: false,
            preserve_case: false,
            overridden: None,
            search_scope: None,
            matches_position: 0,
            matches_count: 0,
            current_match: None,
            is_loop: true,
        }
    }
}

/// `INewFindReplaceState`: the fields to change.
#[derive(Clone, Debug, Default)]
pub struct NewFindState {
    pub search_string: Option<String>,
    pub replace_string: Option<String>,
    pub is_revealed: Option<bool>,
    pub is_replace_revealed: Option<bool>,
    pub is_regex: Option<bool>,
    pub whole_word: Option<bool>,
    pub match_case: Option<bool>,
    pub preserve_case: Option<bool>,
    /// `Some(None)` clears the scope.
    pub search_scope: Option<Option<Vec<Range<Position>>>>,
    pub is_loop: Option<bool>,
    /// The overrides a multi-cursor session sets (`isRegexOverride` false, `wholeWordOverride` and
    /// `matchCaseOverride` true); any change without them clears them.
    pub override_options: bool,
}

/// `FindReplaceStateChangedEvent`: what a change touched.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FindStateChanged {
    pub move_cursor: bool,
    pub update_history: bool,
    pub search_string: bool,
    pub replace_string: bool,
    pub is_revealed: bool,
    pub is_replace_revealed: bool,
    pub is_regex: bool,
    pub whole_word: bool,
    pub match_case: bool,
    pub preserve_case: bool,
    pub search_scope: bool,
    pub is_loop: bool,
}

/// `MATCHES_LIMIT`: decorations past this many are not added; navigation searches the model instead.
pub const MATCHES_LIMIT: usize = 19999;

impl FindState {
    /// `change`: `None` when nothing changed, else what did.
    pub fn change(&mut self, new: NewFindState, move_cursor: bool, update_history: bool) -> Option<FindStateChanged> {
        let mut changed = FindStateChanged { move_cursor, update_history, ..Default::default() };
        set_if_changed(&mut self.search_string, new.search_string, &mut changed.search_string);
        set_if_changed(&mut self.replace_string, new.replace_string, &mut changed.replace_string);
        set_if_changed(&mut self.is_revealed, new.is_revealed, &mut changed.is_revealed);
        set_if_changed(&mut self.is_replace_revealed, new.is_replace_revealed, &mut changed.is_replace_revealed);
        let (mut is_regex, mut whole_word, mut match_case) = self.actual_options();
        is_regex = new.is_regex.unwrap_or(is_regex);
        whole_word = new.whole_word.unwrap_or(whole_word);
        match_case = new.match_case.unwrap_or(match_case);
        self.overridden = new.override_options.then_some((is_regex, whole_word, match_case));
        let effective = if new.override_options { (false, true, true) } else { (is_regex, whole_word, match_case) };
        set_if_changed(&mut self.is_regex, Some(effective.0), &mut changed.is_regex);
        set_if_changed(&mut self.whole_word, Some(effective.1), &mut changed.whole_word);
        set_if_changed(&mut self.match_case, Some(effective.2), &mut changed.match_case);
        set_if_changed(&mut self.preserve_case, new.preserve_case, &mut changed.preserve_case);
        if let Some(scope) = new.search_scope {
            // Upstream's check: a new scope counts as unchanged only when each of its ranges differs from some old one.
            let unchanged = scope.as_ref().is_some_and(|scope| scope.iter().all(|range| self.search_scope.as_ref().is_some_and(|old| old.iter().any(|existing| existing != range))));
            if !unchanged {
                self.search_scope = scope;
                changed.search_scope = true;
            }
        }
        set_if_changed(&mut self.is_loop, new.is_loop, &mut changed.is_loop);
        let nothing = FindStateChanged { move_cursor, update_history, ..Default::default() };
        (changed != nothing).then_some(changed)
    }

    /// (`actualIsRegex`, `actualWholeWord`, `actualMatchCase`): the options without the overrides.
    pub fn actual_options(&self) -> (bool, bool, bool) {
        self.overridden.unwrap_or((self.is_regex, self.whole_word, self.match_case))
    }

    /// `changeMatchInfo`: `current_match` `None` keeps the current match.
    pub fn change_match_info(&mut self, matches_position: usize, matches_count: usize, current_match: Option<Range<Position>>) {
        self.matches_position = if matches_count == 0 { 0 } else { matches_position.min(matches_count) };
        self.matches_count = matches_count;
        if current_match.is_some() {
            self.current_match = current_match;
        }
    }

    pub fn can_navigate_back(&self) -> bool {
        self.can_navigate_in_loop() || self.matches_position != 1
    }

    pub fn can_navigate_forward(&self) -> bool {
        self.can_navigate_in_loop() || self.matches_position < self.matches_count
    }

    fn can_navigate_in_loop(&self) -> bool {
        self.is_loop || self.matches_count >= MATCHES_LIMIT
    }
}

fn set_if_changed<T: PartialEq>(field: &mut T, new: Option<T>, changed: &mut bool) {
    if let Some(new) = new.filter(|new| new != field) {
        *field = new;
        *changed = true;
    }
}
