//! Label matching of the quick input list: base/common/filters.ts (`matchesFuzzy` and the filters it
//! chains), base/common/iconLabels.ts (`$(icon)` aware matching) and base/common/comparers.ts
//! (`compareAnything`). Offsets count chars where upstream counts UTF-16 units; both agree outside
//! the astral planes.

use std::cmp::Ordering;
use std::ops::Range;

pub type Matches = Vec<Range<usize>>;

pub fn matches_prefix(word: &str, target: &str) -> Option<Matches> {
    let word: Vec<char> = word.chars().collect();
    let target: Vec<char> = target.chars().collect();
    if target.is_empty() || target.len() < word.len() {
        return None;
    }
    if !word.iter().zip(&target).all(|(a, b)| chars_equal_ignore_case(*a, *b)) {
        return None;
    }
    Some(if word.is_empty() { vec![] } else { vec![0..word.len()] })
}

pub fn matches_contiguous_sub_string(word: &str, target: &str) -> Option<Matches> {
    let word = lower_chars(word);
    if word.is_empty() {
        return Some(vec![0..0]);
    }
    let target = lower_chars(target);
    find(&target, &word, 0).map(|index| vec![index..index + word.len()])
}

pub fn matches_camel_case(word: &str, camel_case_word: &str) -> Option<Matches> {
    let trimmed = camel_case_word.trim();
    if trimmed.is_empty() {
        return None;
    }
    let word: Vec<char> = word.chars().collect();
    if !is_camel_case_pattern(&word) {
        return None;
    }
    let mut target: Vec<char> = trimmed.chars().take(60).collect();
    let analysis = analyze_camel_case_word(&target);
    if !analysis.is_camel_case_word() {
        if !analysis.is_upper_case_word() {
            return None;
        }
        target = target.iter().flat_map(|c| c.to_lowercase()).collect();
    }
    let word: Vec<char> = word.iter().flat_map(|c| c.to_lowercase()).collect();
    let mut start = 0;
    while start < target.len() {
        if let Some(result) = camel_case_from(&word, &target, 0, start) {
            return Some(result);
        }
        start = next_anchor(&target, start + 1);
    }
    None
}

/// `matchesFuzzy(word, target, false)`: a `*` wildcard match first, then prefix, camel case and
/// contiguous substring.
pub fn matches_fuzzy(word: &str, target: &str) -> Option<Matches> {
    if word.is_empty() {
        return Some(vec![0..0]);
    }
    if let Some(range) = wildcard_match(&lower_chars(word), &lower_chars(target)) {
        return Some(vec![range]);
    }
    matches_prefix(word, target).or_else(|| matches_camel_case(word, target)).or_else(|| matches_contiguous_sub_string(word, target))
}

/// A label without its `$(icon)` markers, and for each char left the length of the markers before it.
pub struct ParsedLabel {
    pub text: String,
    pub icon_offsets: Vec<usize>,
}

pub fn parse_label_with_icons(input: &str) -> ParsedLabel {
    let chars: Vec<char> = input.chars().collect();
    let mut text = String::new();
    let mut icon_offsets = Vec::new();
    let mut icons_length = 0;
    let mut index = 0;
    while index < chars.len() {
        if let Some(length) = icon_marker_length(&chars[index..]) {
            icons_length += length;
            index += length;
            continue;
        }
        text.push(chars[index]);
        icon_offsets.push(icons_length);
        index += 1;
    }
    ParsedLabel { text, icon_offsets }
}

pub fn matches_fuzzy_icon_aware(query: &str, target: &ParsedLabel) -> Option<Matches> {
    if target.icon_offsets.is_empty() {
        return matches_fuzzy(query, &target.text);
    }
    let trimmed = target.text.trim_start_matches(' ');
    let leading_whitespace = target.text.chars().count() - trimmed.chars().count();
    let mut matches = matches_fuzzy(query, trimmed)?;
    if query.is_empty() {
        return Some(matches);
    }
    for range in &mut matches {
        let icon_offset = target.icon_offsets[range.start + leading_whitespace] + leading_whitespace;
        *range = range.start + icon_offset..range.end + icon_offset;
    }
    Some(matches)
}

/// `matchesContiguousIconAware` of quickInputList.ts, for `matchOnLabelMode: 'contiguous'`.
pub fn matches_contiguous_icon_aware(query: &str, target: &ParsedLabel) -> Option<Matches> {
    let contiguous = |word: &str, text: &str| {
        let index = find(&lower_chars(text), &lower_chars(word), 0)?;
        Some(vec![index..index + word.chars().count()])
    };
    if target.icon_offsets.is_empty() {
        return contiguous(query, &target.text);
    }
    let trimmed = target.text.trim_start_matches(' ');
    let leading_whitespace = target.text.chars().count() - trimmed.chars().count();
    let mut matches = contiguous(query, trimmed)?;
    for range in &mut matches {
        let icon_offset = target.icon_offsets[range.start + leading_whitespace] + leading_whitespace;
        *range = range.start + icon_offset..range.end + icon_offset;
    }
    Some(matches)
}

/// `compareAnything`: prefix matches first (shorter first), then suffix matches, then file names.
pub fn compare_anything(one: &str, other: &str, look_for: &str) -> Ordering {
    let a = one.to_lowercase();
    let b = other.to_lowercase();
    let a_prefix = a.starts_with(look_for);
    let b_prefix = b.starts_with(look_for);
    if a_prefix != b_prefix {
        return if a_prefix { Ordering::Less } else { Ordering::Greater };
    }
    if a_prefix && a.len() != b.len() {
        return a.len().cmp(&b.len());
    }
    let a_suffix = a.ends_with(look_for);
    let b_suffix = b.ends_with(look_for);
    if a_suffix != b_suffix {
        return if a_suffix { Ordering::Less } else { Ordering::Greater };
    }
    compare_file_names(&a, &b).then_with(|| a.cmp(&b))
}

/// `compareFileNames`: numeric, case-insensitive order; equal names under that order sort by code unit.
/// shortcut: Intl.Collator's locale rules (accents, punctuation weights) are not available in Rust std;
/// digits compare by value and other chars by lowercase code point.
pub fn compare_file_names(a: &str, b: &str) -> Ordering {
    let mut a_chars = a.chars().peekable();
    let mut b_chars = b.chars().peekable();
    loop {
        match (a_chars.peek().copied(), b_chars.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let number = |chars: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = chars.next_if(char::is_ascii_digit) {
                        digits.push(c);
                    }
                    let trimmed = digits.trim_start_matches('0').to_owned();
                    (trimmed.len(), trimmed)
                };
                let ordering = number(&mut a_chars).cmp(&number(&mut b_chars));
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some(x), Some(y)) => {
                let ordering = x.to_lowercase().cmp(y.to_lowercase());
                if ordering != Ordering::Equal {
                    return ordering;
                }
                a_chars.next();
                b_chars.next();
            }
        }
    }
}

fn lower_chars(text: &str) -> Vec<char> {
    text.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect()
}

fn chars_equal_ignore_case(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

fn find(haystack: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    (from..=haystack.len() - needle.len()).find(|start| haystack[*start..start + needle.len()] == *needle)
}

/// The regex `convertSimple2RegExpPattern(word)` with the `i` flag: `*` matches any run of chars except
/// line breaks, greedily; every other char matches itself.
fn wildcard_match(word: &[char], target: &[char]) -> Option<Range<usize>> {
    let segments: Vec<&[char]> = word.split(|c| *c == '*').collect();
    (0..=target.len()).find_map(|start| {
        let first = segments[0];
        if target.len() - start < first.len() || target[start..start + first.len()] != *first {
            return None;
        }
        let end = greedy_rest(&segments[1..], target, start + first.len())?;
        Some(start..end)
    })
}

/// The end of the greedy match of `.*seg1.*seg2...` starting at `from`.
fn greedy_rest(segments: &[&[char]], target: &[char], from: usize) -> Option<usize> {
    let Some((segment, rest)) = segments.split_first() else {
        return Some(from);
    };
    let line_end = target[from..].iter().position(|c| *c == '\n' || *c == '\r').map_or(target.len(), |offset| from + offset);
    (from..=line_end).rev().find_map(|start| {
        if line_end - start < segment.len() || target[start..start + segment.len()] != **segment {
            return None;
        }
        greedy_rest(rest, target, start + segment.len())
    })
}

/// The length of a `$(name)` marker at the start; names are `[A-Za-z0-9~-]+` (iconLabels.ts `_parseIconsRegex`).
fn icon_marker_length(chars: &[char]) -> Option<usize> {
    if chars.len() < 4 || chars[0] != '$' || chars[1] != '(' {
        return None;
    }
    let name_length = chars[2..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == '~' || **c == '-').count();
    (name_length > 0 && chars.get(2 + name_length) == Some(&')')).then_some(name_length + 3)
}

fn is_word_char_anchor(target: &[char], index: usize) -> bool {
    let c = target[index];
    c.is_ascii_uppercase() || c.is_ascii_digit() || (index > 0 && !target[index - 1].is_ascii_alphanumeric())
}

fn next_anchor(target: &[char], start: usize) -> usize {
    (start..target.len()).find(|index| is_word_char_anchor(target, *index)).unwrap_or(target.len())
}

fn camel_case_from(word: &[char], target: &[char], i: usize, j: usize) -> Option<Matches> {
    if i == word.len() {
        return Some(vec![]);
    }
    if j == target.len() || word[i] != target[j].to_lowercase().next().unwrap_or(target[j]) {
        return None;
    }
    let mut result = camel_case_from(word, target, i + 1, j + 1);
    let mut next_upper = j + 1;
    while result.is_none() {
        next_upper = next_anchor(target, next_upper);
        if next_upper >= target.len() {
            break;
        }
        result = camel_case_from(word, target, i + 1, next_upper);
        next_upper += 1;
    }
    result.map(|tail| join(j..j + 1, tail))
}

fn join(head: Range<usize>, mut tail: Matches) -> Matches {
    match tail.first_mut() {
        None => vec![head],
        Some(first) if first.start == head.end => {
            first.start = head.start;
            tail
        }
        Some(_) => {
            tail.insert(0, head);
            tail
        }
    }
}

struct CamelCaseAnalysis {
    upper: f64,
    lower: f64,
    alpha: f64,
    numeric: f64,
}

impl CamelCaseAnalysis {
    fn is_upper_case_word(&self) -> bool {
        self.lower == 0. && self.upper > 0.6
    }

    fn is_camel_case_word(&self) -> bool {
        self.lower > 0.2 && self.upper < 0.8 && self.alpha > 0.6 && self.numeric < 0.2
    }
}

fn analyze_camel_case_word(word: &[char]) -> CamelCaseAnalysis {
    let share = |test: fn(&char) -> bool| word.iter().filter(|c| test(c)).count() as f64 / word.len() as f64;
    CamelCaseAnalysis {
        upper: share(char::is_ascii_uppercase),
        lower: share(char::is_ascii_lowercase),
        alpha: share(char::is_ascii_alphanumeric),
        numeric: share(char::is_ascii_digit),
    }
}

fn is_camel_case_pattern(word: &[char]) -> bool {
    let upper = word.iter().filter(|c| c.is_ascii_uppercase()).count();
    let lower = word.iter().filter(|c| c.is_ascii_lowercase()).count();
    let whitespace = word.iter().filter(|c| matches!(c, ' ' | '\t' | '\n' | '\r')).count();
    if (upper == 0 || lower == 0) && whitespace == 0 { word.len() <= 30 } else { upper <= 5 }
}

#[cfg(test)]
mod tests;
