//! findState.ts: what the find widget searches for and how, and the match count it shows.
//! The option overrides, `isSearching` and `filters` are left out: only notebooks and the search
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
    pub is_regex: bool,
    pub whole_word: bool,
    pub match_case: bool,
    pub preserve_case: bool,
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
        set_if_changed(&mut self.is_regex, new.is_regex, &mut changed.is_regex);
        set_if_changed(&mut self.whole_word, new.whole_word, &mut changed.whole_word);
        set_if_changed(&mut self.match_case, new.match_case, &mut changed.match_case);
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
