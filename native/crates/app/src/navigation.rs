//! The editor navigation stacks of historyService.ts (`EditorNavigationStacks`, `EditorNavigationStack`,
//! `EditorSelectionState`): where Go Back and Go Forward lead, and the last edit and navigation locations.

use std::path::PathBuf;

use crate::editor::Position;

/// `EditorPaneSelectionChangeReason`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Programmatic,
    User,
    Edit,
    Navigation,
    Jump,
}

impl Reason {
    /// textEditor.ts `onDidChangeCursorPosition`: the cursor change's source.
    pub fn of_source(source: &str) -> Self {
        match source {
            "api" => Reason::Programmatic,
            "code.navigation" => Reason::Navigation,
            "code.jump" => Reason::Jump,
            _ => Reason::User,
        }
    }
}

/// `GoFilter`: which stack a command walks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    None,
    Edits,
    Navigation,
}

/// `IEditorNavigationStackEntry`: the editor by its resource, its group and the selection's line and cursor.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub group: u32,
    pub scheme: String,
    pub path: PathBuf,
    /// The selection start line and the cursor, as `TextEditorPaneSelection` keeps them.
    pub selection: Option<(Position, Position)>,
}

/// textEditor.ts `TEXT_EDITOR_SELECTION_THRESHOLD`.
const SELECTION_THRESHOLD: usize = 10;
/// `EditorNavigationStack.MAX_STACK_SIZE`.
const MAX_STACK_SIZE: usize = 50;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Compare {
    Identical,
    Similar,
    Different,
}

/// `TextEditorPaneSelection.compare`: by the lower of the selection's start line and its cursor line.
fn compare(a: (Position, Position), b: (Position, Position)) -> Compare {
    let line = |(start, cursor): (Position, Position)| start.line.min(cursor.line);
    let (a, b) = (line(a), line(b));
    match a.abs_diff(b) {
        0 => Compare::Identical,
        distance if distance < SELECTION_THRESHOLD => Compare::Similar,
        _ => Compare::Different,
    }
}

fn same_editor(a: &Entry, b: &Entry) -> bool {
    a.group == b.group && a.scheme == b.scheme && a.path == b.path
}

/// `shouldReplaceStackEntry`.
fn should_replace(entry: &Entry, candidate: &Entry) -> bool {
    if !same_editor(entry, candidate) {
        return false;
    }
    match (entry.selection, candidate.selection) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(a), Some(b)) => compare(a, b) == Compare::Identical,
    }
}

/// `EditorSelectionState.justifiesNewNavigationEntry`.
fn justifies_new_entry(current: &(Entry, Reason), candidate: &Entry, reason: Reason) -> bool {
    if !same_editor(&current.0, candidate) {
        return true;
    }
    let (Some(a), Some(b)) = (current.0.selection, candidate.selection) else { return true };
    match compare(a, b) {
        Compare::Similar => matches!(reason, Reason::Navigation | Reason::Jump),
        Compare::Different => true,
        Compare::Identical => false,
    }
}

/// `EditorNavigationStack`.
#[derive(Default)]
pub struct Stack {
    pub entries: Vec<Entry>,
    index: Option<usize>,
    previous: Option<usize>,
    /// `currentSelectionState`: the last editor and selection seen, which the next change compares with.
    current: Option<(Entry, Reason)>,
}

impl Stack {
    /// `notifyNavigation`, outside a navigation of this stack.
    pub fn notify(&mut self, candidate: Entry, reason: Reason) {
        if candidate.selection.is_none() {
            if self.current_entry().is_some_and(|entry| same_editor(entry, &candidate)) {
                return;
            }
            self.add_or_replace(candidate, false);
            self.current = None;
            return;
        }
        // `onSelectionAwareEditorNavigation`.
        let justified = self.current.as_ref().is_none_or(|current| justifies_new_entry(current, &candidate, reason));
        self.add_or_replace(candidate.clone(), !justified);
        self.current = Some((candidate, reason));
    }

    /// The selection a navigation of this stack lands on becomes the one the next change compares with.
    pub fn notify_navigated(&mut self, candidate: Entry, reason: Reason) {
        self.current = Some((candidate, reason));
    }

    fn current_entry(&self) -> Option<&Entry> {
        self.entries.get(self.index?)
    }

    /// `addOrReplace`.
    fn add_or_replace(&mut self, candidate: Entry, force_replace: bool) {
        if let Some(index) = self.index.filter(|&index| force_replace || should_replace(&self.entries[index], &candidate)) {
            self.entries[index] = candidate;
            return;
        }
        let insert_at = self.index.map_or(0, |index| index + 1);
        self.entries.truncate(insert_at);
        self.entries.push(candidate);
        if self.entries.len() > MAX_STACK_SIZE {
            self.entries.remove(0);
            self.previous = self.previous.and_then(|previous| previous.checked_sub(1));
        } else {
            self.previous = self.index;
            self.index = Some(insert_at);
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.index.is_some_and(|index| index > 0)
    }

    pub fn can_go_forward(&self) -> bool {
        self.index.map_or(!self.entries.is_empty(), |index| index + 1 < self.entries.len())
    }

    pub fn can_go_last(&self) -> bool {
        !self.entries.is_empty()
    }

    /// `goBack`, `goForward`, `goLast`, `goPrevious`: the entry to open, the index moved to it.
    pub fn go_back(&mut self) -> Option<Entry> {
        let index = self.index?.checked_sub(1)?;
        self.go_to(index)
    }

    pub fn go_forward(&mut self) -> Option<Entry> {
        let index = self.index.map_or(0, |index| index + 1);
        self.go_to(index)
    }

    pub fn go_last(&mut self) -> Option<Entry> {
        let index = self.entries.len().checked_sub(1)?;
        self.go_to(index)
    }

    pub fn go_previous(&mut self) -> Option<Entry> {
        match self.previous {
            Some(previous) => self.go_to(previous),
            None => self.go_back(),
        }
    }

    fn go_to(&mut self, index: usize) -> Option<Entry> {
        let entry = self.entries.get(index)?.clone();
        self.previous = self.index;
        self.index = Some(index);
        Some(entry)
    }

    /// The entry the stack stands on when the editor is not on it already (`maybeGoCurrent` of the filtered stacks).
    pub fn current_if_away(&self, at: &Entry) -> Option<Entry> {
        let entry = self.current_entry()?;
        let away = !same_editor(entry, at) || matches!((entry.selection, at.selection), (Some(a), Some(b)) if compare(a, b) != Compare::Identical);
        away.then(|| entry.clone())
    }

    /// `remove`, then `flatten`: entries of a closed group or deleted resource go, neighbors that merge become one.
    pub fn remove(&mut self, gone: impl Fn(&Entry) -> bool) {
        self.entries.retain(|entry| !gone(entry));
        let mut flat: Vec<Entry> = Vec::new();
        for entry in self.entries.drain(..) {
            match flat.last() {
                Some(last) if should_replace(last, &entry) => *flat.last_mut().expect("checked") = entry,
                _ => flat.push(entry),
            }
        }
        self.entries = flat;
        self.index = self.entries.len().checked_sub(1);
        self.previous = None;
        if self.current.as_ref().is_some_and(|(entry, _)| gone(entry)) {
            self.current = None;
        }
    }
}

/// `EditorNavigationStacks`: every change, the edits, and the navigations and jumps.
#[derive(Default)]
pub struct Stacks {
    pub selections: Stack,
    pub edits: Stack,
    pub navigations: Stack,
    /// The filter of the stack a navigation is running on (`navigating`): its own stack takes no entries then.
    pub navigating: Option<Filter>,
}

impl Stacks {
    pub fn stack_mut(&mut self, filter: Filter) -> &mut Stack {
        match filter {
            Filter::None => &mut self.selections,
            Filter::Edits => &mut self.edits,
            Filter::Navigation => &mut self.navigations,
        }
    }

    pub fn stack(&self, filter: Filter) -> &Stack {
        match filter {
            Filter::None => &self.selections,
            Filter::Edits => &self.edits,
            Filter::Navigation => &self.navigations,
        }
    }

    /// `EditorNavigationStacks.handleActiveEditorSelectionChange` and `handleActiveEditorChange`.
    pub fn notify(&mut self, candidate: Entry, reason: Option<Reason>) {
        let navigating = self.navigating;
        let previous = self.selections.current_entry().cloned();
        for filter in [Filter::None, Filter::Edits, Filter::Navigation] {
            let takes = match filter {
                Filter::None => true,
                Filter::Edits => reason == Some(Reason::Edit),
                Filter::Navigation => matches!(reason, Some(Reason::Navigation | Reason::Jump)) && navigating != Some(Filter::None),
            };
            if !takes {
                continue;
            }
            let stack = self.stack_mut(filter);
            if navigating == Some(filter) {
                stack.notify_navigated(candidate.clone(), reason.unwrap_or(Reason::Programmatic));
                continue;
            }
            // A jump adds where it jumped from first.
            if filter == Filter::Navigation && reason == Some(Reason::Jump) && let Some(source) = previous.clone() {
                stack.notify(source, Reason::Jump);
            }
            stack.notify(candidate.clone(), reason.unwrap_or(Reason::Programmatic));
        }
    }

    pub fn remove(&mut self, gone: impl Fn(&Entry) -> bool) {
        self.selections.remove(&gone);
        self.edits.remove(&gone);
        self.navigations.remove(&gone);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(path: &str, line: usize) -> Entry {
        Entry { group: 0, scheme: "file".into(), path: PathBuf::from(path), selection: Some((Position::new(line, 0), Position::new(line, 0))) }
    }

    fn lines(stack: &Stack) -> Vec<usize> {
        stack.entries.iter().map(|entry| entry.selection.unwrap().0.line + 1).collect()
    }

    #[test]
    fn user_selection_changes_within_ten_lines_replace_the_entry() {
        // historyService.test.ts 'in-editor text selection changes (user)': lines 1, 15, 16, 17, 30, 40.
        let mut stacks = Stacks::default();
        for line in [1, 15, 16, 17, 30, 40] {
            stacks.notify(at("/a", line - 1), Some(Reason::User));
        }
        assert_eq!(lines(&stacks.selections), [1, 17, 30, 40]);
        assert!(stacks.navigations.entries.is_empty());
        assert!(stacks.edits.entries.is_empty());
    }

    #[test]
    fn back_forward_previous_and_last_walk_the_stack() {
        let mut stack = Stack::default();
        assert!(!stack.can_go_back() && !stack.can_go_forward() && !stack.can_go_last());
        stack.notify(Entry { selection: None, ..at("/a", 0) }, Reason::Programmatic);
        stack.notify(Entry { selection: None, ..at("/a", 0) }, Reason::Programmatic);
        assert_eq!(stack.entries.len(), 1);
        stack.notify(Entry { selection: None, ..at("/b", 0) }, Reason::Programmatic);
        assert!(stack.can_go_back() && !stack.can_go_forward());
        assert_eq!(stack.go_back().unwrap().path, PathBuf::from("/a"));
        assert!(stack.can_go_forward());
        assert_eq!(stack.go_forward().unwrap().path, PathBuf::from("/b"));
        assert_eq!(stack.go_previous().unwrap().path, PathBuf::from("/a"));
        assert_eq!(stack.go_previous().unwrap().path, PathBuf::from("/b"));
        assert_eq!(stack.go_last().unwrap().path, PathBuf::from("/b"));
    }

    #[test]
    fn a_similar_jump_or_navigation_still_adds_an_entry() {
        // #143833.
        let mut stacks = Stacks::default();
        stacks.notify(at("/a", 0), Some(Reason::User));
        stacks.notify(at("/a", 4), Some(Reason::Navigation));
        assert_eq!(lines(&stacks.selections), [1, 5]);
        stacks.notify(at("/a", 7), Some(Reason::Jump));
        assert_eq!(lines(&stacks.selections), [1, 5, 8]);
    }

    #[test]
    fn a_jump_records_where_it_came_from_and_edits_have_their_own_stack() {
        let mut stacks = Stacks::default();
        stacks.notify(at("/a", 0), Some(Reason::User));
        stacks.notify(at("/a", 40), Some(Reason::Jump));
        assert_eq!(lines(&stacks.navigations), [1, 41]);
        stacks.notify(at("/a", 20), Some(Reason::Edit));
        assert_eq!(lines(&stacks.edits), [21]);
        assert_eq!(stacks.edits.go_last().unwrap().selection.unwrap().0.line, 20);
    }

    #[test]
    fn navigating_a_stack_adds_nothing_to_it() {
        let mut stacks = Stacks::default();
        stacks.notify(at("/a", 0), Some(Reason::User));
        stacks.notify(at("/a", 30), Some(Reason::User));
        let back = stacks.selections.go_back().unwrap();
        stacks.navigating = Some(Filter::None);
        stacks.notify(back, Some(Reason::Programmatic));
        stacks.navigating = None;
        assert_eq!(lines(&stacks.selections), [1, 31]);
        assert!(stacks.selections.can_go_forward());
    }

    #[test]
    fn removing_entries_merges_their_neighbors() {
        // 'editor navigation stack - mutations': removing a resource and a group.
        let mut stack = Stack::default();
        for (path, line) in [("/a", 0), ("/b", 0), ("/a", 0), ("/c", 30)] {
            stack.notify(at(path, line), Reason::User);
        }
        stack.remove(|entry| entry.path == PathBuf::from("/b"));
        assert_eq!(stack.entries.iter().map(|entry| entry.path.to_string_lossy().into_owned()).collect::<Vec<_>>(), ["/a", "/c"]);
        assert!(stack.can_go_back() && !stack.can_go_forward());
        stack.remove(|entry| entry.group == 0);
        assert!(!stack.can_go_last());
    }

    #[test]
    fn the_stack_keeps_fifty_entries() {
        let mut stack = Stack::default();
        for index in 0..60 {
            stack.notify(at(&format!("/{index}"), 0), Reason::User);
        }
        assert_eq!(stack.entries.len(), MAX_STACK_SIZE);
        assert_eq!(stack.entries[0].path, PathBuf::from("/10"));
        assert!(!stack.can_go_forward());
    }
}
