//! Label matching of the quick input list: base/common/filters.ts (`matchesFuzzy` and the filters it
//! chains), base/common/iconLabels.ts (`$(icon)` aware matching) and base/common/comparers.ts
//! (`compareAnything`). Offsets count chars where upstream counts UTF-16 units; both agree outside
//! the astral planes.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::Range;

use icu_normalizer::DecomposingNormalizerBorrowed;

mod korean;
mod korean_tables;

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

pub fn matches_base_contiguous_sub_string(word: &str, target: &str) -> Option<Matches> {
    if word.is_empty() {
        return Some(vec![0..0]);
    }
    if word.encode_utf16().count() > target.encode_utf16().count() {
        return None;
    }
    let word: Vec<char> = try_normalize_to_base(word).chars().collect();
    let target: Vec<char> = try_normalize_to_base(target).chars().collect();
    find(&target, &word, 0).map(|index| vec![index..index + word.len()])
}

/// `matchesWords(word, target, false)`: the word's chars match starts of the target's words in order, like
/// "gp" or "g p" in "Git: Pull"; any separator matches any other.
pub fn matches_words(word: &str, target: &str) -> Option<Matches> {
    matches_words_with(word, target, false)
}

/// `matchesWords(word, target, true)`: the word matches whole consecutive words of the target, "pul" in "Git: Pull".
pub fn matches_contiguous_words(word: &str, target: &str) -> Option<Matches> {
    matches_words_with(word, target, true)
}

fn matches_words_with(word: &str, target: &str, contiguous: bool) -> Option<Matches> {
    if target.is_empty() {
        return None;
    }
    let word: Vec<char> = try_normalize_to_base(word).chars().collect();
    let target: Vec<char> = try_normalize_to_base(target).chars().collect();
    // Memoized per (word index, target index): separators match each other, so the search can branch a lot (#309582).
    let mut memo = HashMap::new();
    let mut target_index = 0;
    while target_index < target.len() {
        if let Some(result) = matches_words_from(&word, &target, 0, target_index, contiguous, &mut memo) {
            return Some(result);
        }
        target_index = next_word(&target, target_index + 1);
    }
    None
}

/// `tryNormalizeToBase`: lowercase, without combining accents unless dropping them changes the length.
pub fn try_normalize_to_base(text: &str) -> String {
    let decomposed = DecomposingNormalizerBorrowed::new_nfd().normalize(text);
    let without_accents: String = decomposed.chars().filter(|c| !('\u{300}'..='\u{36f}').contains(c)).collect();
    let utf16_length = |text: &str| text.encode_utf16().count();
    let base = if utf16_length(&without_accents) == utf16_length(text) { without_accents.as_str() } else { text };
    base.to_lowercase()
}

fn matches_words_from(word: &[char], target: &[char], word_index: usize, target_index: usize, contiguous: bool, memo: &mut HashMap<(usize, usize), Option<Matches>>) -> Option<Matches> {
    if word_index == word.len() {
        return Some(vec![]);
    }
    if target_index == target.len() {
        return None;
    }
    if let Some(cached) = memo.get(&(word_index, target_index)) {
        return cached.clone();
    }
    let computed = matches_words_compute(word, target, word_index, target_index, contiguous, memo);
    memo.insert((word_index, target_index), computed.clone());
    computed
}

fn matches_words_compute(word: &[char], target: &[char], word_index: usize, target_index: usize, contiguous: bool, memo: &mut HashMap<(usize, usize), Option<Matches>>) -> Option<Matches> {
    let code = word[word_index];
    let alternate: Option<Vec<char>> = korean::korean_alt_chars(code).map(|keys| keys.chars().collect());
    let mut target_index_offset = 0;
    if !characters_match(code, target[target_index]) {
        let alternate = alternate.as_ref()?;
        let all_match = alternate.iter().enumerate().all(|(k, alt)| target.get(target_index + k).is_some_and(|c| characters_match(*alt, *c)));
        if !all_match {
            return None;
        }
        target_index_offset += alternate.len() - 1;
    }
    let mut next_word_index = target_index + target_index_offset + 1;
    let mut result = matches_words_from(word, target, word_index + 1, next_word_index, contiguous, memo);
    while result.is_none() && !contiguous {
        next_word_index = next_word(target, next_word_index);
        if next_word_index >= target.len() {
            break;
        }
        result = matches_words_from(word, target, word_index + 1, next_word_index, contiguous, memo);
        next_word_index += 1;
    }
    let mut result = result?;
    // Chars that differ matched as separators or Korean keys; separators stay out of the highlights.
    if code != target[target_index] {
        let Some(alternate) = &alternate else { return Some(result) };
        if alternate.iter().enumerate().any(|(k, alt)| target.get(target_index + k) != Some(alt)) {
            return Some(result);
        }
    }
    let head = target_index..target_index + target_index_offset + 1;
    match result.first_mut() {
        Some(first) if first.start == head.end => first.start = head.start,
        _ => result.insert(0, head),
    }
    Some(result)
}

/// `isWordSeparator`: whitespace and the punctuation of written text.
fn is_word_separator(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | '`' | '\'' | '"' | '-' | '/' | ';' | ':' | ',' | '.' | '?' | '!')
}

fn characters_match(a: char, b: char) -> bool {
    a == b || (is_word_separator(a) && is_word_separator(b))
}

fn next_word(target: &[char], start: usize) -> usize {
    (start..target.len()).find(|i| is_word_separator(target[*i]) || (*i > 0 && is_word_separator(target[*i - 1]))).unwrap_or(target.len())
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

/// `FuzzyScore`: the score, the word offset matching started at, and the matched columns from the
/// last to the first, relative to that offset.
#[derive(Clone, Debug, PartialEq)]
pub struct FuzzyScore {
    pub score: i32,
    pub word_start: usize,
    pub positions: Vec<usize>,
}

#[derive(Clone, Copy)]
pub struct FuzzyScoreOptions {
    pub first_match_can_be_weak: bool,
    pub boost_full_match: bool,
}

impl Default for FuzzyScoreOptions {
    fn default() -> Self {
        Self { first_match_can_be_weak: false, boost_full_match: true }
    }
}

pub fn create_matches(score: Option<&FuzzyScore>) -> Matches {
    let Some(score) = score else {
        return vec![];
    };
    let mut matches: Matches = vec![];
    for position in score.positions.iter().rev() {
        let pos = position + score.word_start;
        match matches.last_mut() {
            Some(last) if last.end == pos => last.end = pos + 1,
            _ => matches.push(pos..pos + 1),
        }
    }
    matches
}

const FUZZY_MAX_LEN: usize = 128;
const ARROW_DIAG: u8 = 1;
const ARROW_LEFT: u8 = 2;
const ARROW_LEFT_LEFT: u8 = 3;

/// `fuzzyScore` of filters.ts: the matrix of pattern rows by word columns, at most 128 of each.
pub fn fuzzy_score(pattern: &str, pattern_low: &str, pattern_start: usize, word: &str, word_low: &str, word_start: usize, options: FuzzyScoreOptions) -> Option<FuzzyScore> {
    let pattern: Vec<char> = pattern.chars().collect();
    let pattern_low: Vec<char> = pattern_low.chars().collect();
    let word: Vec<char> = word.chars().collect();
    let word_low: Vec<char> = word_low.chars().collect();
    let pattern_len = pattern.len().min(FUZZY_MAX_LEN);
    let word_len = word.len().min(FUZZY_MAX_LEN);

    if pattern_start >= pattern_len || word_start >= word_len || pattern_len - pattern_start > word_len - word_start {
        return None;
    }

    let mut min_word_match_pos = vec![0usize; pattern_len];
    if !is_pattern_in_word(&pattern_low, pattern_start, pattern_len, &word_low, word_start, word_len, &mut min_word_match_pos) {
        return None;
    }
    let mut max_word_match_pos = vec![0usize; pattern_len];
    let mut pattern_pos = pattern_len;
    let mut word_pos = word_len;
    while pattern_pos > pattern_start && word_pos > word_start {
        if pattern_low[pattern_pos - 1] == word_low[word_pos - 1] {
            max_word_match_pos[pattern_pos - 1] = word_pos - 1;
            pattern_pos -= 1;
        }
        word_pos -= 1;
    }

    let columns = word_len - word_start + 1;
    let rows = pattern_len - pattern_start + 1;
    let mut table = vec![vec![0i32; columns]; rows];
    let mut diag = vec![vec![0i32; columns]; rows];
    let mut arrows = vec![vec![0u8; columns]; rows];
    let mut has_strong_first_match = false;

    for (row, pattern_pos) in (pattern_start..pattern_len).enumerate().map(|(index, pos)| (index + 1, pos)) {
        let min_pos = min_word_match_pos[pattern_pos];
        let max_pos = max_word_match_pos[pattern_pos];
        let next_max_pos = if pattern_pos + 1 < pattern_len { max_word_match_pos[pattern_pos + 1] } else { word_len };

        for word_pos in min_pos..next_max_pos {
            let column = word_pos - word_start + 1;
            let score = (word_pos <= max_pos)
                .then(|| fuzzy_char_score(&pattern, &pattern_low, pattern_pos, pattern_start, &word, &word_low, word_pos, word_len, word_start, diag[row - 1][column - 1] == 0, &mut has_strong_first_match))
                .flatten();

            let diag_score = score.map(|score| score + table[row - 1][column - 1]);
            let can_come_left = word_pos > min_pos;
            let left_score = if can_come_left { table[row][column - 1] + if diag[row][column - 1] > 0 { -5 } else { 0 } } else { 0 };
            let can_come_left_left = word_pos > min_pos + 1 && diag[row][column - 1] > 0;
            let left_left_score = if can_come_left_left { table[row][column - 2] + if diag[row][column - 2] > 0 { -5 } else { 0 } } else { 0 };

            if can_come_left_left && (!can_come_left || left_left_score >= left_score) && diag_score.is_none_or(|diag_score| left_left_score >= diag_score) {
                table[row][column] = left_left_score;
                arrows[row][column] = ARROW_LEFT_LEFT;
                diag[row][column] = 0;
            } else if can_come_left && diag_score.is_none_or(|diag_score| left_score >= diag_score) {
                table[row][column] = left_score;
                arrows[row][column] = ARROW_LEFT;
                diag[row][column] = 0;
            } else {
                table[row][column] = diag_score.expect("fuzzy_score: no path into a reachable cell");
                arrows[row][column] = ARROW_DIAG;
                diag[row][column] = diag[row - 1][column - 1] + 1;
            }
        }
    }

    if !has_strong_first_match && !options.first_match_can_be_weak {
        return None;
    }

    let mut row = rows - 1;
    let mut column = columns as isize - 1;
    let mut result = FuzzyScore { score: table[row][column as usize], word_start, positions: vec![] };
    let mut backwards_diag_length = 0;
    let mut max_match_column = 0;

    while row >= 1 {
        let mut diag_column = column;
        loop {
            match arrows[row][diag_column as usize] {
                ARROW_LEFT_LEFT => diag_column -= 2,
                ARROW_LEFT => diag_column -= 1,
                _ => break,
            }
            if diag_column < 1 {
                break;
            }
        }

        if backwards_diag_length > 1
            && pattern_low[pattern_start + row - 1] == word_low[word_start + column as usize - 1]
            && !is_upper_case_at(&word, &word_low, diag_column + word_start as isize - 1)
            && backwards_diag_length + 1 > diag[row][diag_column as usize]
        {
            diag_column = column;
        }

        backwards_diag_length = if diag_column == column { backwards_diag_length + 1 } else { 1 };
        if max_match_column == 0 {
            max_match_column = diag_column;
        }
        row -= 1;
        column = diag_column - 1;
        result.positions.push(column as usize);
    }

    if word_len - word_start == pattern_len && options.boost_full_match {
        result.score += 2;
    }
    result.score -= (max_match_column - pattern_len as isize) as i32;
    Some(result)
}

fn is_pattern_in_word(pattern_low: &[char], mut pattern_pos: usize, pattern_len: usize, word_low: &[char], mut word_pos: usize, word_len: usize, min_word_match_pos: &mut [usize]) -> bool {
    while pattern_pos < pattern_len && word_pos < word_len {
        if pattern_low[pattern_pos] == word_low[word_pos] {
            min_word_match_pos[pattern_pos] = word_pos;
            pattern_pos += 1;
        }
        word_pos += 1;
    }
    pattern_pos == pattern_len
}

#[allow(clippy::too_many_arguments)]
fn fuzzy_char_score(pattern: &[char], pattern_low: &[char], pattern_pos: usize, pattern_start: usize, word: &[char], word_low: &[char], word_pos: usize, word_len: usize, word_start: usize, new_match_start: bool, first_match_strong: &mut bool) -> Option<i32> {
    if pattern_low[pattern_pos] != word_low[word_pos] {
        return None;
    }
    let at = word_pos as isize;
    let same_case_score = if pattern[pattern_pos] == word[word_pos] { 7 } else { 5 };
    let mut score = 1;
    let mut is_gap_location = false;
    if word_pos == pattern_pos - pattern_start {
        score = same_case_score;
    } else if is_upper_case_at(word, word_low, at) && (word_pos == 0 || !is_upper_case_at(word, word_low, at - 1)) {
        score = same_case_score;
        is_gap_location = true;
    } else if is_fuzzy_separator_at(word_low, at) && (word_pos == 0 || !is_fuzzy_separator_at(word_low, at - 1)) {
        score = 5;
    } else if is_fuzzy_separator_at(word_low, at - 1) || is_whitespace_at(word_low, at - 1) {
        score = 5;
        is_gap_location = true;
    }

    if score > 1 && pattern_pos == pattern_start {
        *first_match_strong = true;
    }
    if !is_gap_location {
        is_gap_location = is_upper_case_at(word, word_low, at) || is_fuzzy_separator_at(word_low, at - 1) || is_whitespace_at(word_low, at - 1);
    }

    if pattern_pos == pattern_start {
        if word_pos > word_start {
            score -= if is_gap_location { 3 } else { 5 };
        }
    } else if new_match_start {
        score += if is_gap_location { 2 } else { 0 };
    } else {
        score += if is_gap_location { 0 } else { 1 };
    }

    if word_pos + 1 == word_len {
        score -= if is_gap_location { 3 } else { 5 };
    }
    Some(score)
}

/// `word[pos] !== wordLow[pos]`; out of range compares `undefined` with itself.
fn is_upper_case_at(word: &[char], word_low: &[char], pos: isize) -> bool {
    pos >= 0 && word.get(pos as usize) != word_low.get(pos as usize)
}

fn is_fuzzy_separator_at(value: &[char], index: isize) -> bool {
    if index < 0 {
        return false;
    }
    let Some(&c) = value.get(index as usize) else {
        return false;
    };
    matches!(c, '_' | '-' | '.' | ' ' | '/' | '\\' | '\'' | '"' | ':' | '$' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}') || is_emoji_imprecise(c as u32)
}

fn is_whitespace_at(value: &[char], index: isize) -> bool {
    index >= 0 && matches!(value.get(index as usize), Some(' ' | '\t'))
}

/// strings.ts `isEmojiImprecise`.
fn is_emoji_imprecise(x: u32) -> bool {
    (0x1F1E6..=0x1F1FF).contains(&x)
        || x == 8986
        || x == 8987
        || x == 9200
        || x == 9203
        || (9728..=10175).contains(&x)
        || x == 11088
        || x == 11093
        || (127744..=128591).contains(&x)
        || (128640..=128764).contains(&x)
        || (128992..=129008).contains(&x)
        || (129280..=129535).contains(&x)
        || (129648..=129782).contains(&x)
}
#[cfg(test)]
mod tests;
#[cfg(test)]
mod fuzzy_score_tests;
