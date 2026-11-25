//! smartSelect.ts: Expand and Shrink Selection over the ranges of the selection range providers, the
//! word (wordSelections.ts) and bracket (bracketSelections.ts) ones. Inside, a column is upstream's
//! 1-based column, where 0 is the column before the line that `getLineFirstNonWhitespaceColumn` gives a
//! blank line; lines stay 0-based.
//! shortcut: no extension `SelectionRangeProvider`s (MainThreadLanguageFeatures, milestone 13), so every
//! language gets the word and bracket ranges upstream gives a language without a provider.

use std::cmp::Reverse;
use std::collections::{HashMap, VecDeque};

use super::cursor::{Position, Selection};
use super::{OpenEditor, word_at};
use crate::brackets::PairBracket;
use crate::languages::Languages;

/// A range from its start to its end, columns 1-based.
type Span = (Position, Position);

/// `editor.smartSelect`.
#[derive(Clone, Copy)]
pub struct Options {
    pub select_leading_and_trailing_whitespace: bool,
    pub select_subwords: bool,
}

/// `SelectionRanges`: one cursor's ranges, its selection first, and the one it selects.
#[derive(Clone, Debug)]
pub struct SelectionRanges {
    index: usize,
    ranges: Vec<Span>,
}

impl SelectionRanges {
    /// `mov`: the next range out or in, past ranges equal to the current one; the last one stays.
    fn moved(&self, forward: bool) -> usize {
        let mut index = self.index;
        loop {
            let next = if forward { index + 1 } else { index.wrapping_sub(1) };
            if next >= self.ranges.len() {
                return index;
            }
            if self.ranges[next] != self.ranges[index] {
                return next;
            }
            index = next;
        }
    }
}

impl OpenEditor {
    /// `SmartSelectController.run`: the ranges are computed once per cursor and kept while the selections
    /// are the ones the last run set (upstream drops them on `onDidChangeCursorPosition`).
    pub fn smart_select(&mut self, forward: bool, languages: &Languages, options: Options) {
        let mut states = match self.smart_select.take() {
            Some((states, selections)) if selections == self.selections => states,
            _ => self.selection_ranges(languages, options),
        };
        for state in &mut states {
            state.index = state.moved(forward);
        }
        let validated = |position: Position| Position::new(position.line, position.column.saturating_sub(1));
        let selections = states.iter().map(|state| {
            let (start, end) = state.ranges[state.index];
            Selection { anchor: validated(start), head: validated(end) }
        });
        self.set_selections(selections.collect());
        self.smart_select = Some((states, self.selections.clone()));
    }

    fn selection_ranges(&self, languages: &Languages, options: Options) -> Vec<SelectionRanges> {
        let lines = &self.document.lines;
        let brackets = self.document.tokens.pairs(languages, lines, &self.document.language_id).brackets;
        let brackets = char_brackets(lines, &brackets);
        let word_pattern = languages.rules(&self.document.language_id).and_then(|rules| rules.word_pattern.as_ref());
        let one_based = |position: Position| Position::new(position.line, position.column + 1);
        let mut states = Vec::new();
        for selection in &self.selections {
            let (start, end) = (one_based(selection.start()), one_based(selection.end()));
            let mut ranges = provide_selection_ranges(lines, &brackets, word_pattern, one_based(selection.head), options);
            ranges.retain(|range| contains_position(*range, start) && contains_position(*range, end));
            ranges.insert(0, (start, end));
            states.push(SelectionRanges { index: 0, ranges });
        }
        states
    }
}

/// `provideSelectionRanges` with the bracket and word providers: the ranges around `position`, inner first,
/// each holding the one before, with the lines' leading and trailing whitespace as extra steps.
fn provide_selection_ranges(lines: &[String], brackets: &[CharBracket], word_pattern: Option<&onig::Regex>, position: Position, options: Options) -> Vec<Span> {
    let mut raw = bracket_ranges(lines, brackets, position);
    raw.extend(word_ranges(lines, word_pattern, position, options.select_subwords));
    raw.retain(|range| contains_position(*range, position));
    raw.sort_by_key(|&(start, end)| (Reverse(start), end));
    let mut ranges: Vec<Span> = Vec::new();
    for range in raw {
        if ranges.last().is_none_or(|&last| contains_range(range, last) && range != last) {
            ranges.push(range);
        }
    }
    if !options.select_leading_and_trailing_whitespace {
        return ranges;
    }
    with_whitespace_ranges(lines, &ranges)
}

/// `selectLeadingAndTrailingWhitespace`: between two ranges on different lines, the lines of the inner one
/// without and with their leading and trailing whitespace, each only when the outer range holds it.
fn with_whitespace_ranges(lines: &[String], ranges: &[Span]) -> Vec<Span> {
    let Some(&first) = ranges.first() else { return Vec::new() };
    let mut with_trivia = vec![first];
    for pair in ranges.windows(2) {
        let (previous, current) = (pair[0], pair[1]);
        if previous.0.line != current.0.line || previous.1.line != current.1.line {
            let no_whitespace = (Position::new(previous.0.line, first_non_whitespace_column(&lines[previous.0.line])), Position::new(previous.1.line, last_non_whitespace_column(&lines[previous.1.line])));
            if contains_range(no_whitespace, previous) && no_whitespace != previous && contains_range(current, no_whitespace) && current != no_whitespace {
                with_trivia.push(no_whitespace);
            }
            let full = (Position::new(previous.0.line, 1), Position::new(previous.1.line, max_column(&lines[previous.1.line])));
            if contains_range(full, previous) && full != no_whitespace && contains_range(current, full) && current != full {
                with_trivia.push(full);
            }
        }
        with_trivia.push(current);
    }
    with_trivia
}

/// `WordSelectionRangeProvider`: the subword (camelCase hump, or between `_` and `-`), the word, a line of
/// only whitespace, the whole document.
fn word_ranges(lines: &[String], word_pattern: Option<&onig::Regex>, position: Position, select_subwords: bool) -> Vec<Span> {
    let mut ranges = Vec::new();
    let line = &lines[position.line];
    let at = |column: usize| Position::new(position.line, column + 1);
    let word = match word_pattern {
        Some(pattern) => pattern_word_at(pattern, line, position.column - 1),
        None => word_at(line, position.column - 1),
    };
    if let Some(word) = word {
        let chars: Vec<char> = line.chars().collect();
        if select_subwords
            && let Some(subword) = subword(&chars[word.clone()], position.column - 1 - word.start)
        {
            ranges.push((at(word.start + subword.start), at(word.start + subword.end)));
        }
        ranges.push((at(word.start), at(word.end)));
    }
    if !line.is_empty() && first_non_whitespace_column(line) == 0 {
        ranges.push((Position::new(position.line, 1), Position::new(position.line, max_column(line))));
    }
    let last = lines.len() - 1;
    ranges.push((Position::new(0, 1), Position::new(last, max_column(&lines[last]))));
    ranges
}

/// `getWordAtText` with a language's `wordPattern`: the first match that touches char `column`, in chars.
/// shortcut: matches over the whole line, where upstream scans a 50-char window back from the cursor (`windowSize`).
fn pattern_word_at(pattern: &onig::Regex, line: &str, column: usize) -> Option<std::ops::Range<usize>> {
    let chars = |offset: usize| line[..offset].chars().count();
    pattern.find_iter(line).map(|(start, end)| chars(start)..chars(end)).filter(|word| !word.is_empty()).find(|word| word.start <= column && column <= word.end)
}

/// `_addInWordRanges`: the part of `word` around `offset` between `_`, `-` and lower-to-upper case steps.
fn subword(word: &[char], offset: usize) -> Option<std::ops::Range<usize>> {
    let is_separator = |c: Option<char>| matches!(c, Some('_' | '-'));
    let is_lower = |c: Option<char>| c.is_some_and(|c| c.is_ascii_lowercase());
    let is_upper = |c: Option<char>| c.is_some_and(|c| c.is_ascii_uppercase());
    let mut last = None;
    let mut start = offset as isize;
    while start >= 0 {
        let c = word.get(start as usize).copied();
        if (start as usize != offset && is_separator(c)) || (is_lower(c) && is_upper(last)) {
            break;
        }
        last = c;
        start -= 1;
    }
    let start = (start + 1) as usize;
    let mut end = offset;
    while end < word.len() {
        let c = Some(word[end]);
        if (is_upper(c) && is_lower(last)) || is_separator(c) {
            break;
        }
        last = c;
        end += 1;
    }
    (start < end).then_some(start..end)
}

/// A bracket of a pair over char columns, 1-based.
struct CharBracket<'a> {
    range: Span,
    opening: bool,
    key: &'a str,
}

fn char_brackets<'a>(lines: &[String], brackets: &'a [PairBracket]) -> Vec<CharBracket<'a>> {
    let column = |line: usize, offset: usize| lines[line][..offset].chars().count() + 1;
    brackets
        .iter()
        .map(|bracket| {
            let (line, range) = &bracket.range;
            let range = (Position::new(*line, column(*line, range.start)), Position::new(*line, column(*line, range.end)));
            CharBracket { range, opening: bracket.opening, key: &bracket.key }
        })
        .collect()
}

/// `BracketSelectionRangeProvider`: right of `position` the closing brackets nothing opens there, then left
/// of it the opening brackets they close, each pair inside and out, inner pairs first.
/// shortcut: runs to the document's ends in one go, where upstream yields every 30 ms and stops after 2 rounds.
fn bracket_ranges(lines: &[String], brackets: &[CharBracket], position: Position) -> Vec<Span> {
    let mut closings: HashMap<&str, VecDeque<Span>> = HashMap::new();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for bracket in brackets.iter().filter(|bracket| bracket.range.1 > position) {
        let count = counts.entry(bracket.key).or_default();
        match (bracket.opening, *count) {
            (true, _) => *count += 1,
            (false, 0) => closings.entry(bracket.key).or_default().push_back(bracket.range),
            (false, _) => *count -= 1,
        }
    }
    let mut ranges = Vec::new();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for bracket in brackets.iter().rev().filter(|bracket| bracket.range.0 < position) {
        let count = counts.entry(bracket.key).or_default();
        match (bracket.opening, *count) {
            (false, _) => *count += 1,
            (true, 0) => {
                let Some(list) = closings.get_mut(bracket.key) else { continue };
                let closing = list.pop_front().expect("an empty list is removed");
                if list.is_empty() {
                    closings.remove(bracket.key);
                }
                let outer = (bracket.range.0, closing.1);
                ranges.push((bracket.range.1, closing.0));
                ranges.push(outer);
                bracket_leading(lines, outer, &mut ranges);
            }
            (true, _) => *count -= 1,
        }
    }
    ranges
}

/// `_addBracketLeading`: a block's brackets with the text before `{` on its line, or with the line above
/// a `{` on its own line.
fn bracket_leading(lines: &[String], (start, end): Span, ranges: &mut Vec<Span>) {
    if start.line == end.line {
        return;
    }
    let column = first_non_whitespace_column(&lines[start.line]);
    if column != 0 && column != start.column {
        ranges.push((Position::new(start.line, column), end));
        ranges.push((Position::new(start.line, 1), end));
    }
    let Some(above) = start.line.checked_sub(1) else { return };
    let column = first_non_whitespace_column(&lines[above]);
    if column == start.column && column != last_non_whitespace_column(&lines[above]) {
        ranges.push((Position::new(above, column), end));
        ranges.push((Position::new(above, 1), end));
    }
}

fn contains_position((start, end): Span, position: Position) -> bool {
    start <= position && position <= end
}

fn contains_range(outer: Span, inner: Span) -> bool {
    outer.0 <= inner.0 && inner.1 <= outer.1
}

/// `getLineFirstNonWhitespaceColumn`: 0 for a line of only spaces and tabs.
fn first_non_whitespace_column(line: &str) -> usize {
    line.chars().position(|c| c != ' ' && c != '\t').map_or(0, |index| index + 1)
}

/// `getLineLastNonWhitespaceColumn`: the column after the last char that is not a space or tab, 0 for none.
fn last_non_whitespace_column(line: &str) -> usize {
    let chars: Vec<char> = line.chars().collect();
    chars.iter().rposition(|&c| c != ' ' && c != '\t').map_or(0, |index| index + 2)
}

fn max_column(line: &str) -> usize {
    line.chars().count() + 1
}

#[cfg(test)]
mod tests;
