//! bracketMatching.ts `jumpToBracket`, `selectToBracket` and `removeBrackets` on the model's bracket pairs.

use super::OpenEditor;
use super::cursor::{Position, Selection};
use super::edit::{ContentChanged, EditReason};
use crate::brackets::{BracketRange, DocumentBrackets};
use crate::languages::Languages;

impl OpenEditor {
    /// Each cursor to the other bracket of the pair it touches, else to the closing bracket around it, else to
    /// the next bracket; a cursor with none collapses where it starts.
    pub fn jump_to_bracket(&mut self, languages: &Languages) {
        let brackets = self.bracket_pairs(languages);
        let document = self.document();
        let lines = &document.lines;
        let mut selections = Vec::new();
        for selection in &self.selections {
            let position = selection.start();
            let (line, offset) = byte_position(lines, position);
            let target = match brackets.match_bracket(line, offset) {
                Some([open, close]) if contains(open, line, offset) && !contains(close, line, offset) => Some(close),
                Some([open, close]) if contains(close, line, offset) => Some(open),
                Some(_) => None,
                None => match brackets.enclosing_brackets(line, offset) {
                    Some([_, close]) => Some(close),
                    None => brackets.next_bracket(line, offset),
                },
            };
            selections.push(Selection::caret(target.map_or(position, |bracket| start(lines, bracket))));
        }
        drop(document);
        self.set_selections(selections);
    }

    /// Each selection over the pair its start touches, else the pair around it, else the pair of the next
    /// bracket; `select_brackets` false leaves the brackets out. Backwards from a closing bracket.
    pub fn select_to_bracket(&mut self, languages: &Languages, select_brackets: bool) {
        let brackets = self.bracket_pairs(languages);
        let document = self.document();
        let lines = &document.lines;
        let mut selections = Vec::new();
        for selection in &self.selections {
            let position = selection.start();
            let (line, offset) = byte_position(lines, position);
            let pair = brackets.match_bracket(line, offset).or_else(|| brackets.enclosing_brackets(line, offset)).or_else(|| {
                let (next_line, next) = brackets.next_bracket(line, offset)?;
                brackets.match_bracket(*next_line, next.start)
            });
            let Some(mut pair) = pair else { continue };
            pair.sort_by_key(|(line, range)| (*line, range.start));
            let [open, close] = pair;
            let (mut from, mut to) = match select_brackets {
                true => (start(lines, open), end(lines, close)),
                false => (end(lines, open), start(lines, close)),
            };
            if contains(close, line, offset) {
                (from, to) = (to, from);
            }
            selections.push(Selection { anchor: from, head: to });
        }
        drop(document);
        if !selections.is_empty() {
            self.set_selections(selections);
        }
    }

    /// Deletes the pair each cursor touches or sits in, one undo stop around each pair.
    pub fn remove_brackets(&mut self, languages: &Languages) -> Vec<ContentChanged> {
        let mut changes = Vec::new();
        for position in self.selections.iter().map(|selection| selection.head).collect::<Vec<_>>() {
            let brackets = self.bracket_pairs(languages);
            let document = self.document();
            let lines = &document.lines;
            let (line, offset) = byte_position(lines, position);
            let Some([open, close]) = brackets.match_bracket(line, offset).or_else(|| brackets.enclosing_brackets(line, offset)) else { continue };
            let edits = [open, close].map(|bracket| (start(lines, bracket), end(lines, bracket), String::new(), false)).into();
            drop(document);
            self.push_undo_stop();
            changes.extend(self.execute_edits(edits, None, EditReason::Unknown("editor.action.removeBrackets")));
            self.push_undo_stop();
        }
        changes
    }

    fn bracket_pairs(&self, languages: &Languages) -> DocumentBrackets {
        self.document().tokens.pairs(languages, &self.document().lines, &self.document().language_id)
    }
}

/// `Range.containsPosition`: the ends included.
fn contains((bracket_line, range): &BracketRange, line: usize, offset: usize) -> bool {
    *bracket_line == line && range.start <= offset && offset <= range.end
}

fn byte_position(lines: &[String], position: Position) -> (usize, usize) {
    let text = &lines[position.line];
    (position.line, text.char_indices().nth(position.column).map_or(text.len(), |(offset, _)| offset))
}

fn start(lines: &[String], (line, range): &BracketRange) -> Position {
    Position::new(*line, lines[*line][..range.start].chars().count())
}

fn end(lines: &[String], (line, range): &BracketRange) -> Position {
    Position::new(*line, lines[*line][..range.end].chars().count())
}

#[cfg(test)]
mod tests;
