//! snippetSession.ts `OneSnippet` and `SnippetSession` over the model: the placeholders of the snippets one insert
//! wrote (one per cursor), grouped by index, tracked through edits, and the selections each move gives.

use std::ops::Range;

use super::folding::tracked;
use super::{Position, Selection};
use super::edit::end_of;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug)]
pub struct Placeholder {
    pub start: Position,
    pub end: Position,
    /// The final tab stop (`$0`).
    pub is_final: bool,
    /// Whether it held text when inserted: one that collapsed since is skipped by a move.
    had_text: bool,
}

/// The placeholders of one index across the snippets, and its choice options.
#[derive(Debug)]
struct Group {
    placeholders: Vec<Placeholder>,
    choice: Option<Vec<String>>,
}

/// Where each replacement's text starts once all of them are written: applied last to first, each edit moves the
/// starts after it.
pub fn starts_after_edits(replaced: &[(Position, Position, &str)]) -> Vec<Position> {
    let mut order: Vec<usize> = (0..replaced.len()).collect();
    order.sort_by_key(|index| std::cmp::Reverse(replaced[*index].0));
    let mut starts: Vec<Position> = replaced.iter().map(|(start, _, _)| *start).collect();
    for (done, &index) in order.iter().enumerate() {
        let (start, end, text) = replaced[index];
        let new_end = end_of(start, text);
        for &later in &order[..done] {
            starts[later] = tracked(starts[later], start, end, new_end, false);
        }
    }
    starts
}

#[derive(Debug)]
pub struct SnippetSession {
    groups: Vec<Group>,
    /// `_placeholderGroupsIdx`: none before the first move.
    current: Option<usize>,
    /// The model's alternative version when inserted: undoing back to it ends the session.
    pub version: u32,
}

/// One inserted snippet: where its text starts in the model after the edits, its text, and its placeholders and
/// choices in char offsets of that text.
pub struct Inserted<'a> {
    pub start: Position,
    pub text: &'a str,
    pub stops: &'a [(u32, Range<usize>)],
    pub choices: &'a [(u32, Vec<String>)],
}

impl SnippetSession {
    /// `groupBy(placeholders, Placeholder.compareByIndex)`: ascending indexes, the final tab stop last.
    pub fn new(snippets: &[Inserted], version: u32) -> Self {
        let mut indexes: Vec<u32> = snippets.iter().flat_map(|snippet| snippet.stops.iter().map(|(index, _)| *index)).collect();
        indexes.sort_by_key(|index| if *index == 0 { u32::MAX } else { *index });
        indexes.dedup();
        let groups = indexes
            .into_iter()
            .map(|index| {
                let mut placeholders = Vec::new();
                let mut choice = None;
                for snippet in snippets {
                    let position = |chars: usize| end_of(snippet.start, &snippet.text.chars().take(chars).collect::<String>());
                    for (_, range) in snippet.stops.iter().filter(|(stop, _)| *stop == index) {
                        placeholders.push(Placeholder { start: position(range.start), end: position(range.end), is_final: index == 0, had_text: !range.is_empty() });
                    }
                    choice = choice.or_else(|| snippet.choices.iter().find(|(stop, _)| *stop == index).map(|(_, options)| options.clone()));
                }
                Group { placeholders, choice }
            })
            .collect();
        SnippetSession { groups, current: None, version }
    }

    pub fn has_placeholder(&self) -> bool {
        !self.groups.is_empty()
    }

    /// `move`: the next (or previous) group's ranges as selections; a group whose text was deleted is skipped.
    pub fn move_to(&mut self, forward: bool) -> Vec<Selection> {
        let can_move = match (forward, self.current) {
            (true, None) => !self.groups.is_empty(),
            (true, Some(current)) => current + 1 < self.groups.len(),
            (false, current) => current.is_some_and(|current| current > 0),
        };
        if !can_move {
            return self.selections();
        }
        self.current = Some(match (forward, self.current) {
            (true, None) => 0,
            (true, Some(current)) => current + 1,
            (false, current) => current.expect("checked above") - 1,
        });
        let collapsed = self.groups[self.current.expect("set above")].placeholders.iter().any(|placeholder| placeholder.had_text && placeholder.start == placeholder.end);
        if collapsed && !self.is_at_last() && (forward || !self.is_at_first()) {
            return self.move_to(forward);
        }
        self.selections()
    }

    fn selections(&self) -> Vec<Selection> {
        let Some(current) = self.current else { return Vec::new() };
        self.groups[current].placeholders.iter().map(|placeholder| Selection { anchor: placeholder.start, head: placeholder.end }).collect()
    }

    pub fn is_at_first(&self) -> bool {
        self.current.is_none_or(|current| current == 0)
    }

    pub fn is_at_last(&self) -> bool {
        self.current == Some(self.groups.len() - 1)
    }

    /// `isSelectionWithinPlaceholders`: each selection lies in a placeholder of the current group.
    pub fn selections_within(&self, selections: &[Selection]) -> bool {
        let Some(current) = self.current else { return false };
        let group = &self.groups[current];
        selections.len() >= group.placeholders.len().min(1) && selections.iter().all(|selection| group.placeholders.iter().any(|placeholder| placeholder.start <= selection.start() && selection.end() <= placeholder.end))
    }

    /// `activeChoice`: the options of the current group and its first range.
    pub fn active_choice(&self) -> Option<(&[String], Position, Position)> {
        let group = &self.groups[self.current?];
        let placeholder = group.placeholders.first()?;
        Some((group.choice.as_deref()?, placeholder.start, placeholder.end))
    }

    /// The placeholders to draw: each with whether its group is the current one.
    pub fn placeholders(&self) -> impl Iterator<Item = (&Placeholder, bool)> {
        self.groups.iter().enumerate().flat_map(move |(index, group)| group.placeholders.iter().map(move |placeholder| (placeholder, Some(index) == self.current)))
    }

    /// The decorations' stickiness through the replace of `start..end` with `text`: the current group's
    /// placeholders grow when typing at their edges (`AlwaysGrowsWhenTypingAtEdges`), the others and the final tab
    /// stop never do.
    pub fn track(&mut self, start: Position, end: Position, text: &str) {
        let new_end = end_of(start, text);
        for (index, group) in self.groups.iter_mut().enumerate() {
            let grows = Some(index) == self.current;
            for placeholder in &mut group.placeholders {
                let grows = grows && !placeholder.is_final;
                placeholder.start = tracked(placeholder.start, start, end, new_end, grows);
                placeholder.end = tracked(placeholder.end, start, end, new_end, !grows).max(placeholder.start);
            }
        }
    }
}
