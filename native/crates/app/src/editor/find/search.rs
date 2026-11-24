//! textModelSearch.ts: the matches of a search string in the lines, line by line or, for a multiline
//! search, over the lines joined by "\n". Oniguruma stands in for the JavaScript RegExp; positions are
//! 0-based lines and char columns where upstream counts 1-based UTF-16 columns.

use std::ops::Range;

use onig::{Region, RegexOptions, SearchOptions, Syntax};

use crate::editor::Position;
use crate::editor::cursor::last_position;

/// `LIMIT_FIND_COUNT`.
const LIMIT_FIND_COUNT: usize = 999;

pub struct SearchParams<'a> {
    pub search_string: &'a str,
    pub is_regex: bool,
    pub match_case: bool,
    /// The word separators when only whole words match.
    pub word_separators: Option<&'a str>,
}

pub struct SearchData {
    regex: onig::Regex,
    /// The regex source, the search string escaped when it is not a regex.
    pub source: String,
    pub multiline: bool,
    pub ignore_case: bool,
    pub word_separators: Option<String>,
    /// The search string when a plain substring search finds the same matches.
    pub simple_search: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FindMatch {
    pub range: Range<Position>,
    /// The match and its groups, an unmatched group as "", when the caller captures them.
    pub matches: Option<Vec<String>>,
}

impl SearchParams<'_> {
    /// `parseSearchRequest`: `None` for an empty search string or an invalid regex.
    pub fn parse_search_request(&self) -> Option<SearchData> {
        if self.search_string.is_empty() {
            return None;
        }
        let multiline = match self.is_regex {
            true => is_multiline_regex_source(self.search_string),
            false => self.search_string.contains('\n'),
        };
        let source = match self.is_regex {
            true => self.search_string.to_string(),
            false => escape_regexp_characters(self.search_string),
        };
        let mut options = RegexOptions::REGEX_OPTION_CAPTURE_GROUP;
        if !self.match_case {
            options |= RegexOptions::REGEX_OPTION_IGNORECASE;
        }
        let regex = onig::Regex::with_options(&unicode_escapes_as_hex(&source), options, Syntax::ruby()).ok()?;
        let mut can_use_simple_search = !self.is_regex && !multiline;
        if can_use_simple_search && self.search_string.to_lowercase() != self.search_string.to_uppercase() {
            can_use_simple_search = self.match_case;
        }
        Some(SearchData {
            regex,
            source,
            multiline,
            ignore_case: !self.match_case,
            word_separators: self.word_separators.map(str::to_string),
            simple_search: can_use_simple_search.then(|| self.search_string.to_string()),
        })
    }
}

/// `escapeRegExpCharacters` (strings.ts).
fn escape_regexp_characters(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        if "\\{}*+?|^$.[]()".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// JavaScript's `\uHHHH` and `\u{H...}` as Oniguruma's `\x{H...}`, which the Ruby syntax reads.
fn unicode_escapes_as_hex(source: &str) -> String {
    let mut result = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(index) = rest.find('\\') {
        result.push_str(&rest[..index]);
        rest = &rest[index..];
        let after = rest.strip_prefix("\\u").unwrap_or_default();
        let hex = match after.strip_prefix('{') {
            Some(braced) => braced.find('}').map(|end| (&braced[..end], end + 2)),
            None => after.get(..4).filter(|hex| hex.len() == 4 && hex.chars().all(|c| c.is_ascii_hexdigit())).map(|hex| (hex, 4)),
        };
        match hex {
            Some((hex, length)) if !hex.is_empty() => {
                result.push_str(&format!("\\x{{{hex}}}"));
                rest = &after[length..];
            }
            _ => {
                let escape = rest.chars().take(2).map(char::len_utf8).sum();
                result.push_str(&rest[..escape]);
                rest = &rest[escape..];
            }
        }
    }
    result.push_str(rest);
    result
}

/// `isMultilineRegexSource`: the source holds a line feed, or `\n`, `\r` or `\W`, which can match one.
pub fn is_multiline_regex_source(search_string: &str) -> bool {
    let mut chars = search_string.chars();
    while let Some(c) = chars.next() {
        match c {
            '\n' => return true,
            '\\' => match chars.next() {
                Some('n' | 'r' | 'W') => return true,
                Some(_) => {}
                None => break,
            },
            _ => {}
        }
    }
    false
}

/// `TextModelSearch.findMatches`.
pub fn find_matches(lines: &[String], params: &SearchParams, search_range: Range<Position>, capture_matches: bool, limit: usize) -> Vec<FindMatch> {
    let Some(data) = params.parse_search_request() else { return Vec::new() };
    let mut result = Vec::new();
    if data.multiline {
        find_matches_multiline(lines, search_range, &data, capture_matches, limit, &mut result);
        return result;
    }
    let (start, end) = (search_range.start, search_range.end);
    if start.line == end.line {
        let text = char_slice(&lines[start.line], start.column, Some(end.column));
        find_matches_in_line(&data, text, start.line, start.column, capture_matches, limit, &mut result);
        return result;
    }
    find_matches_in_line(&data, char_slice(&lines[start.line], start.column, None), start.line, start.column, capture_matches, limit, &mut result);
    for line in start.line + 1..end.line {
        if result.len() >= limit {
            return result;
        }
        find_matches_in_line(&data, &lines[line], line, 0, capture_matches, limit, &mut result);
    }
    if result.len() < limit {
        find_matches_in_line(&data, char_slice(&lines[end.line], 0, Some(end.column)), end.line, 0, capture_matches, limit, &mut result);
    }
    result
}

/// `_doFindMatchesMultiline`: matches in the range's text, lines joined by "\n".
fn find_matches_multiline(lines: &[String], search_range: Range<Position>, data: &SearchData, capture_matches: bool, limit: usize, result: &mut Vec<FindMatch>) {
    let text = MultilineText::new(lines, search_range.start, search_range.end);
    let mut searcher = Searcher::new(data, 0);
    while let Some(region) = searcher.next(&text.text) {
        result.push(text.find_match(&region, capture_matches));
        if result.len() >= limit {
            return;
        }
    }
}

/// `_findMatchesInLine`: `text` is the part of `line` from char column `delta`.
fn find_matches_in_line(data: &SearchData, text: &str, line: usize, delta: usize, capture_matches: bool, limit: usize, result: &mut Vec<FindMatch>) {
    if let Some(search) = data.simple_search.as_deref().filter(|_| !capture_matches) {
        let mut from = 0;
        while let Some(found) = text[from..].find(search).map(|index| index + from) {
            from = found + search.len();
            if data.word_separators.as_deref().is_none_or(|separators| is_valid_match(separators, text, found, from)) {
                let start = delta + text[..found].chars().count();
                let range = Position::new(line, start)..Position::new(line, start + search.chars().count());
                result.push(FindMatch { range, matches: None });
                if result.len() >= limit {
                    return;
                }
            }
        }
        return;
    }
    let mut searcher = Searcher::new(data, 0);
    while let Some(region) = searcher.next(text) {
        result.push(line_match(text, line, delta, &region, capture_matches));
        if result.len() >= limit {
            return;
        }
    }
}

/// `TextModelSearch.findNextMatch`: the first match at or after `start`, wrapping around the end.
pub fn find_next_match(lines: &[String], params: &SearchParams, start: Position, capture_matches: bool) -> Option<FindMatch> {
    let data = params.parse_search_request()?;
    if data.multiline {
        return find_next_match_multiline(lines, start, &data, capture_matches);
    }
    let text = &lines[start.line];
    if let Some(found) = first_match_in_line(&data, text, start.line, start.column, capture_matches) {
        return Some(found);
    }
    for i in 1..=lines.len() {
        let line = (start.line + i) % lines.len();
        if let Some(found) = first_match_in_line(&data, &lines[line], line, 0, capture_matches) {
            return Some(found);
        }
    }
    None
}

fn find_next_match_multiline(lines: &[String], start: Position, data: &SearchData, capture_matches: bool) -> Option<FindMatch> {
    let text = MultilineText::new(lines, Position::new(start.line, 0), last_position(lines));
    let mut searcher = Searcher::new(data, byte_offset(&lines[start.line], start.column));
    if let Some(region) = searcher.next(&text.text) {
        return Some(text.find_match(&region, capture_matches));
    }
    if start != Position::default() {
        return find_next_match_multiline(lines, Position::default(), data, capture_matches);
    }
    None
}

/// `_findFirstMatchInLine`.
fn first_match_in_line(data: &SearchData, text: &str, line: usize, from_column: usize, capture_matches: bool) -> Option<FindMatch> {
    let mut searcher = Searcher::new(data, byte_offset(text, from_column));
    let region = searcher.next(text)?;
    Some(line_match(text, line, 0, &region, capture_matches))
}

/// `TextModelSearch.findPreviousMatch`: the last match ending at or before `start`, wrapping around the start.
pub fn find_previous_match(lines: &[String], params: &SearchParams, start: Position, capture_matches: bool) -> Option<FindMatch> {
    let data = params.parse_search_request()?;
    if data.multiline {
        return find_previous_match_multiline(lines, start, &data, capture_matches);
    }
    let text = char_slice(&lines[start.line], 0, Some(start.column));
    if let Some(found) = last_match_in_line(&data, text, start.line, capture_matches) {
        return Some(found);
    }
    let count = lines.len();
    for i in 1..=count {
        let line = (count + start.line - i) % count;
        if let Some(found) = last_match_in_line(&data, &lines[line], line, capture_matches) {
            return Some(found);
        }
    }
    None
}

fn find_previous_match_multiline(lines: &[String], start: Position, data: &SearchData, capture_matches: bool) -> Option<FindMatch> {
    let mut matches = Vec::new();
    find_matches_multiline(lines, Position::default()..start, data, capture_matches, 10 * LIMIT_FIND_COUNT, &mut matches);
    if let Some(last) = matches.pop() {
        return Some(last);
    }
    let end = last_position(lines);
    if start != end {
        return find_previous_match_multiline(lines, end, data, capture_matches);
    }
    None
}

/// `_findLastMatchInLine`.
fn last_match_in_line(data: &SearchData, text: &str, line: usize, capture_matches: bool) -> Option<FindMatch> {
    let mut searcher = Searcher::new(data, 0);
    let mut best = None;
    while let Some(region) = searcher.next(text) {
        best = Some(line_match(text, line, 0, &region, capture_matches));
    }
    best
}

fn line_match(text: &str, line: usize, delta: usize, region: &Region, capture_matches: bool) -> FindMatch {
    let (start, end) = region.pos(0).expect("group 0 of a match");
    let start_column = delta + text[..start].chars().count();
    let range = Position::new(line, start_column)..Position::new(line, start_column + text[start..end].chars().count());
    FindMatch { range, matches: capture_matches.then(|| captured(text, region)) }
}

/// `createFindMatch`'s match array, an unmatched group as "".
fn captured(text: &str, region: &Region) -> Vec<String> {
    (0..region.len()).map(|group| region.pos(group).map_or(String::new(), |(start, end)| text[start..end].to_string())).collect()
}

/// The lines from `start` to `end` joined by "\n", and the line start of each to map a match back.
struct MultilineText {
    text: String,
    start: Position,
    /// Byte offsets in `text` where each line after the first starts.
    line_starts: Vec<usize>,
}

impl MultilineText {
    fn new(lines: &[String], start: Position, end: Position) -> Self {
        let text = crate::editor::text_between(lines, start, end);
        let line_starts = text.match_indices('\n').map(|(index, _)| index + 1).collect();
        MultilineText { text, start, line_starts }
    }

    fn position_at(&self, offset: usize) -> Position {
        let breaks = self.line_starts.partition_point(|&line_start| line_start <= offset);
        if breaks == 0 {
            return Position::new(self.start.line, self.start.column + self.text[..offset].chars().count());
        }
        let line_start = self.line_starts[breaks - 1];
        Position::new(self.start.line + breaks, self.text[line_start..offset].chars().count())
    }

    fn find_match(&self, region: &Region, capture_matches: bool) -> FindMatch {
        let (start, end) = region.pos(0).expect("group 0 of a match");
        FindMatch { range: self.position_at(start)..self.position_at(end), matches: capture_matches.then(|| captured(&self.text, region)) }
    }
}

/// `Searcher`: JavaScript's global `RegExp.exec` from `lastIndex`, stepping over an empty match it
/// already returned and skipping matches that are not whole words.
struct Searcher<'a> {
    data: &'a SearchData,
    last_index: usize,
    previous: Option<(usize, usize)>,
}

impl<'a> Searcher<'a> {
    fn new(data: &'a SearchData, last_index: usize) -> Self {
        Searcher { data, last_index, previous: None }
    }

    fn next(&mut self, text: &str) -> Option<Region> {
        loop {
            if self.previous.is_some_and(|(_, end)| end == text.len()) || self.last_index > text.len() {
                return None;
            }
            let mut region = Region::new();
            self.data.regex.search_with_options(text, self.last_index, text.len(), SearchOptions::SEARCH_OPTION_NONE, Some(&mut region))?;
            let (start, end) = region.pos(0).expect("group 0 of a match");
            self.last_index = end;
            if self.previous == Some((start, end)) {
                if start != end {
                    return None;
                }
                self.last_index += text[end..].chars().next().map_or(1, char::len_utf8);
                continue;
            }
            self.previous = Some((start, end));
            if self.data.word_separators.as_deref().is_none_or(|separators| is_valid_match(separators, text, start, end)) {
                return Some(region);
            }
        }
    }
}

/// `isValidMatch`: the match from byte `start` to `end` of `text` starts and ends at word boundaries.
fn is_valid_match(separators: &str, text: &str, start: usize, end: usize) -> bool {
    let (before, matched, after) = (&text[..start], &text[start..end], &text[end..]);
    let left = before.chars().next_back().is_none_or(|c| !is_regular(separators, c) || matched.chars().next().is_some_and(|c| !is_regular(separators, c)));
    let right = after.chars().next().is_none_or(|c| !is_regular(separators, c) || matched.chars().next_back().is_some_and(|c| !is_regular(separators, c)));
    left && right
}

/// `WordCharacterClass.Regular`: not a separator, space or tab. A line break is a boundary too.
fn is_regular(separators: &str, c: char) -> bool {
    !separators.contains(c) && !matches!(c, ' ' | '\t' | '\r' | '\n')
}

fn char_slice(text: &str, start: usize, end: Option<usize>) -> &str {
    let from = byte_offset(text, start);
    match end {
        Some(end) => &text[from..byte_offset(text, end)],
        None => &text[from..],
    }
}

fn byte_offset(text: &str, column: usize) -> usize {
    text.char_indices().nth(column).map_or(text.len(), |(index, _)| index)
}

#[cfg(test)]
mod tests;
