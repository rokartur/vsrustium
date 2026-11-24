//! base/common/fuzzyScorer.ts: the Quick Open scorer of labels, descriptions and file paths. Offsets
//! count chars where upstream counts UTF-16 units; both agree outside the astral planes.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::filters::{self, FuzzyScoreOptions, Matches};

const NO_MATCH: i32 = 0;
const PATH_IDENTITY_SCORE: i32 = 1 << 18;
const LABEL_PREFIX_SCORE_THRESHOLD: i32 = 1 << 17;
const LABEL_SCORE_THRESHOLD: i32 = 1 << 16;
const SEP: char = '/';
const MULTIPLE_QUERY_VALUES_SEPARATOR: char = ' ';

/// `FuzzyScore`: the score and the matched target positions in ascending order.
pub type FuzzyScore = (i32, Vec<usize>);

pub fn score_fuzzy(target: &str, query: &str, query_lower: &str, allow_non_contiguous_matches: bool) -> FuzzyScore {
    if target.is_empty() || query.is_empty() {
        return (NO_MATCH, vec![]);
    }
    let target: Vec<char> = target.chars().collect();
    let query: Vec<char> = query.chars().collect();
    let query_lower: Vec<char> = query_lower.chars().collect();
    if target.len() < query.len() {
        return (NO_MATCH, vec![]);
    }
    let target_lower = lower_chars(&target);
    do_score_fuzzy(&query, &query_lower, &target, &target_lower, allow_non_contiguous_matches)
}

fn do_score_fuzzy(query: &[char], query_lower: &[char], target: &[char], target_lower: &[char], allow_non_contiguous_matches: bool) -> FuzzyScore {
    let query_length = query.len();
    let target_length = target.len();
    let mut scores = vec![0i32; query_length * target_length];
    let mut matches = vec![0i32; query_length * target_length];

    // Rows are query chars, columns target chars; each cell keeps the best score so far and the
    // length of the consecutive match ending there.
    for query_index in 0..query_length {
        let query_index_offset = query_index * target_length;
        for target_index in 0..target_length {
            let current_index = query_index_offset + target_index;
            let left_score = if target_index > 0 { scores[current_index - 1] } else { 0 };
            let diag_index = (query_index > 0 && target_index > 0).then(|| current_index - target_length - 1);
            let diag_score = diag_index.map_or(0, |index| scores[index]);
            let matches_sequence_length = diag_index.map_or(0, |index| matches[index]);

            // Past the first query char only a cell with a diagonal score may match, so the query
            // matches in order ("de" on "ede" must not take the first "e").
            let score = if diag_score == 0 && query_index > 0 {
                0
            } else {
                compute_char_score(query[query_index], query_lower[query_index], target, target_lower, target_index, matches_sequence_length)
            };

            let is_valid_score = score != 0 && diag_score + score >= left_score;
            if is_valid_score && (allow_non_contiguous_matches || query_index > 0 || target_lower[target_index..].starts_with(query_lower)) {
                matches[current_index] = matches_sequence_length + 1;
                scores[current_index] = diag_score + score;
            } else {
                matches[current_index] = NO_MATCH;
                scores[current_index] = left_score;
            }
        }
    }

    let mut positions = vec![];
    let mut query_index = query_length as isize - 1;
    let mut target_index = target_length as isize - 1;
    while query_index >= 0 && target_index >= 0 {
        let current_index = query_index as usize * target_length + target_index as usize;
        if matches[current_index] == NO_MATCH {
            target_index -= 1;
        } else {
            positions.push(target_index as usize);
            query_index -= 1;
            target_index -= 1;
        }
    }
    positions.reverse();
    (scores[query_length * target_length - 1], positions)
}

fn compute_char_score(query_char: char, query_lower_char: char, target: &[char], target_lower: &[char], target_index: usize, matches_sequence_length: i32) -> i32 {
    if !consider_as_equal(query_lower_char, target_lower[target_index]) {
        return 0;
    }
    let mut score = 1;

    // Sequences up to 3 get the full bonus (6), the rest half of it.
    if matches_sequence_length > 0 {
        score += matches_sequence_length.min(3) * 6 + (matches_sequence_length - 3).max(0) * 3;
    }
    if query_char == target[target_index] {
        score += 1;
    }
    if target_index == 0 {
        score += 8;
    } else {
        let separator_bonus = score_separator_at_pos(target[target_index - 1]);
        if separator_bonus > 0 {
            score += separator_bonus;
        } else if target[target_index].is_ascii_uppercase() && matches_sequence_length == 0 {
            // Camel case only outside a contiguous sequence: NPE boosts NullPointerException, HTTP not HTTP.
            score += 2;
        }
    }
    score
}

fn consider_as_equal(a: char, b: char) -> bool {
    a == b || (matches!(a, '/' | '\\') && matches!(b, '/' | '\\'))
}

fn score_separator_at_pos(c: char) -> i32 {
    match c {
        '/' | '\\' => 5,
        '_' | '-' | '.' | ' ' | '\'' | '"' | ':' => 4,
        _ => 0,
    }
}

/// `FuzzyScore2`: the score (None for no match) and the matches, used for symbols.
pub type FuzzyScore2 = (Option<i32>, Matches);

pub fn score_fuzzy2(target: &str, query: &PreparedQuery, pattern_start: usize, word_start: usize) -> FuzzyScore2 {
    match &query.values {
        Some(values) if values.len() > 1 => {
            let mut total_score = 0;
            let mut total_matches = vec![];
            for piece in values {
                let (Some(score), matches) = do_score_fuzzy2_single(target, piece, pattern_start, word_start) else {
                    return (None, vec![]);
                };
                total_score += score;
                total_matches.extend(matches);
            }
            (Some(total_score), normalize_matches(total_matches))
        }
        _ => do_score_fuzzy2_single(target, &query.piece, pattern_start, word_start),
    }
}

fn do_score_fuzzy2_single(target: &str, query: &PreparedQueryPiece, pattern_start: usize, word_start: usize) -> FuzzyScore2 {
    let target_lower: String = lower_chars(&target.chars().collect::<Vec<_>>()).into_iter().collect();
    let options = FuzzyScoreOptions { first_match_can_be_weak: true, boost_full_match: true };
    match filters::fuzzy_score(&query.normalized, &query.normalized_lowercase, pattern_start, target, &target_lower, word_start, options) {
        Some(score) => (Some(score.score), filters::create_matches(Some(&score))),
        None => (None, vec![]),
    }
}

/// `IItemScore`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemScore {
    pub score: i32,
    pub label_match: Option<Matches>,
    pub description_match: Option<Matches>,
}

/// What `IItemAccessor` reads from an item: an empty label means the item has none.
#[derive(Clone, Copy, Debug)]
pub struct ScorerItem<'a> {
    pub label: &'a str,
    pub description: Option<&'a str>,
    pub path: Option<&'a str>,
}

/// `FuzzyScorerCache`, keyed by what upstream hashes: label, description, the contiguous flag and
/// the normalized query values.
pub type FuzzyScorerCache = HashMap<(String, Option<String>, bool, String, Vec<(bool, String)>), ItemScore>;

pub fn score_item_fuzzy(item: Option<&ScorerItem>, query: &PreparedQuery, allow_non_contiguous_matches: bool, cache: &mut FuzzyScorerCache) -> ItemScore {
    let Some(item) = item else {
        return ItemScore::default();
    };
    if query.piece.normalized.is_empty() || item.label.is_empty() {
        return ItemScore::default();
    }
    let values = match &query.values {
        Some(values) => values.iter().map(|value| (value.expect_contiguous_match, value.normalized.clone())).collect(),
        None => vec![(query.piece.expect_contiguous_match, query.piece.normalized.clone())],
    };
    let key = (item.label.to_owned(), item.description.map(str::to_owned), allow_non_contiguous_matches, query.piece.normalized.clone(), values);
    if let Some(cached) = cache.get(&key) {
        return cached.clone();
    }
    let item_score = do_score_item_fuzzy(item.label, item.description, item.path, query, allow_non_contiguous_matches);
    cache.insert(key, item_score.clone());
    item_score
}

fn do_score_item_fuzzy(label: &str, description: Option<&str>, path: Option<&str>, query: &PreparedQuery, allow_non_contiguous_matches: bool) -> ItemScore {
    let prefer_label_matches = path.is_none_or(str::is_empty) || !query.contains_path_separator;

    if path.is_some_and(|path| !path.is_empty() && equals_ignore_case(&query.piece.path_normalized, path)) {
        return ItemScore {
            score: PATH_IDENTITY_SCORE,
            label_match: Some(vec![0..label.chars().count()]),
            description_match: description.filter(|description| !description.is_empty()).map(|description| vec![0..description.chars().count()]),
        };
    }

    match &query.values {
        Some(values) if values.len() > 1 => do_score_item_fuzzy_multiple(label, description, path, values, prefer_label_matches, allow_non_contiguous_matches),
        _ => do_score_item_fuzzy_single(label, description, path, &query.piece, prefer_label_matches, allow_non_contiguous_matches),
    }
}

fn do_score_item_fuzzy_multiple(label: &str, description: Option<&str>, path: Option<&str>, query: &[PreparedQueryPiece], prefer_label_matches: bool, allow_non_contiguous_matches: bool) -> ItemScore {
    let mut total_score = 0;
    let mut total_label_matches = vec![];
    let mut total_description_matches = vec![];
    for piece in query {
        let score = do_score_item_fuzzy_single(label, description, path, piece, prefer_label_matches, allow_non_contiguous_matches);
        if score.score == NO_MATCH {
            return ItemScore::default();
        }
        total_score += score.score;
        total_label_matches.extend(score.label_match.unwrap_or_default());
        total_description_matches.extend(score.description_match.unwrap_or_default());
    }
    ItemScore {
        score: total_score,
        label_match: Some(normalize_matches(total_label_matches)),
        description_match: Some(normalize_matches(total_description_matches)),
    }
}

fn do_score_item_fuzzy_single(label: &str, description: Option<&str>, path: Option<&str>, query: &PreparedQueryPiece, prefer_label_matches: bool, allow_non_contiguous_matches: bool) -> ItemScore {
    let description = description.filter(|description| !description.is_empty());
    let allow_non_contiguous = allow_non_contiguous_matches && !query.expect_contiguous_match;

    if prefer_label_matches || description.is_none() {
        let (label_score, label_positions) = score_fuzzy(label, &query.normalized, &query.normalized_lowercase, allow_non_contiguous);
        if label_score != 0 {
            // A prefix match on the label wins over matches elsewhere in it, and a shorter label
            // wins among prefix matches ("window" prefers window.ts over windowActions.ts).
            let label_prefix_match = filters::matches_prefix(&query.normalized, label);
            let base_score = match label_prefix_match {
                Some(_) => {
                    let prefix_length_boost = (query.normalized.chars().count() as f64 / label.chars().count() as f64 * 100.).round() as i32;
                    LABEL_PREFIX_SCORE_THRESHOLD + prefix_length_boost
                }
                None => LABEL_SCORE_THRESHOLD,
            };
            return ItemScore {
                score: base_score + label_score,
                label_match: Some(label_prefix_match.unwrap_or_else(|| create_matches(&label_positions))),
                description_match: None,
            };
        }
    }

    let Some(description) = description else {
        return ItemScore::default();
    };
    let description_prefix = if path.is_some_and(|path| !path.is_empty()) { format!("{description}{SEP}") } else { description.to_owned() };
    let description_prefix_length = description_prefix.chars().count();
    let description_and_label = format!("{description_prefix}{label}");
    let (label_description_score, label_description_positions) = score_fuzzy(&description_and_label, &query.normalized, &query.normalized_lowercase, allow_non_contiguous);
    if label_description_score == 0 {
        return ItemScore::default();
    }

    let mut label_match = vec![];
    let mut description_match = vec![];
    for h in create_matches(&label_description_positions) {
        if h.start < description_prefix_length && h.end > description_prefix_length {
            label_match.push(0..h.end - description_prefix_length);
            description_match.push(h.start..description_prefix_length);
        } else if h.start >= description_prefix_length {
            label_match.push(h.start - description_prefix_length..h.end - description_prefix_length);
        } else {
            description_match.push(h);
        }
    }
    ItemScore { score: label_description_score, label_match: Some(label_match), description_match: Some(description_match) }
}

fn create_matches(offsets: &[usize]) -> Matches {
    let mut matches: Matches = vec![];
    for &pos in offsets {
        match matches.last_mut() {
            Some(last) if last.end == pos => last.end += 1,
            _ => matches.push(pos..pos + 1),
        }
    }
    matches
}

/// Sorts by start and merges overlapping or touching matches.
fn normalize_matches(mut matches: Matches) -> Matches {
    matches.sort_by_key(|range| range.start);
    let mut normalized: Matches = vec![];
    for range in matches {
        match normalized.last_mut() {
            Some(current) if current.end >= range.start && range.end >= current.start => {
                current.start = current.start.min(range.start);
                current.end = current.end.max(range.end);
            }
            _ => normalized.push(range),
        }
    }
    normalized
}

pub fn compare_items_by_fuzzy_score(item_a: &ScorerItem, item_b: &ScorerItem, query: &PreparedQuery, allow_non_contiguous_matches: bool, cache: &mut FuzzyScorerCache) -> Ordering {
    let item_score_a = score_item_fuzzy(Some(item_a), query, allow_non_contiguous_matches, cache);
    let item_score_b = score_item_fuzzy(Some(item_b), query, allow_non_contiguous_matches, cache);
    let score_a = item_score_a.score;
    let score_b = item_score_b.score;

    // 1.) identity matches have highest score
    if (score_a == PATH_IDENTITY_SCORE || score_b == PATH_IDENTITY_SCORE) && score_a != score_b {
        return if score_a == PATH_IDENTITY_SCORE { Ordering::Less } else { Ordering::Greater };
    }

    // 2.) matches on label are considered higher compared to label+description matches
    if score_a > LABEL_SCORE_THRESHOLD || score_b > LABEL_SCORE_THRESHOLD {
        if score_a != score_b {
            return score_b.cmp(&score_a);
        }
        // Prefer compact label matches, except among prefix matches where longer prefixes win.
        if score_a < LABEL_PREFIX_SCORE_THRESHOLD && score_b < LABEL_PREFIX_SCORE_THRESHOLD {
            let compared_by_match_length = compare_by_match_length(item_score_a.label_match.as_deref(), item_score_b.label_match.as_deref());
            if compared_by_match_length != Ordering::Equal {
                return compared_by_match_length;
            }
        }
        let label_a_length = item_a.label.chars().count();
        let label_b_length = item_b.label.chars().count();
        if label_a_length != label_b_length {
            return label_a_length.cmp(&label_b_length);
        }
    }

    // 3.) compare by score in label+description
    if score_a != score_b {
        return score_b.cmp(&score_a);
    }

    // 4.) scores are identical: prefer matches in label over non-label matches
    let a_has_label_matches = item_score_a.label_match.as_ref().is_some_and(|matches| !matches.is_empty());
    let b_has_label_matches = item_score_b.label_match.as_ref().is_some_and(|matches| !matches.is_empty());
    if a_has_label_matches != b_has_label_matches {
        return if a_has_label_matches { Ordering::Less } else { Ordering::Greater };
    }

    // 5.) scores are identical: prefer more compact matches (label and description)
    let a_match_distance = compute_label_and_description_match_distance(item_a, &item_score_a);
    let b_match_distance = compute_label_and_description_match_distance(item_b, &item_score_b);
    if a_match_distance != 0 && b_match_distance != 0 && a_match_distance != b_match_distance {
        return if b_match_distance > a_match_distance { Ordering::Less } else { Ordering::Greater };
    }

    // 6.) scores are identical: start to use the fallback compare
    fallback_compare(item_a, item_b, query)
}

fn compute_label_and_description_match_distance(item: &ScorerItem, score: &ItemScore) -> isize {
    let label_match = score.label_match.as_deref().unwrap_or_default();
    let description_match = score.description_match.as_deref().unwrap_or_default();
    let mut match_start: isize = -1;
    let mut match_end: isize = -1;
    if let Some(first) = description_match.first() {
        match_start = first.start as isize;
    } else if let Some(first) = label_match.first() {
        match_start = first.start as isize;
    }
    if let Some(last) = label_match.last() {
        match_end = last.end as isize;
        if !description_match.is_empty() {
            if let Some(description) = item.description {
                match_end += description.chars().count() as isize;
            }
        }
    } else if let Some(last) = description_match.last() {
        match_end = last.end as isize;
    }
    match_end - match_start
}

fn compare_by_match_length(matches_a: Option<&[std::ops::Range<usize>]>, matches_b: Option<&[std::ops::Range<usize>]>) -> Ordering {
    let matches_a = matches_a.unwrap_or_default();
    let matches_b = matches_b.unwrap_or_default();
    match (matches_a.is_empty(), matches_b.is_empty()) {
        (true, true) => Ordering::Equal,
        (_, true) => Ordering::Less,
        (true, _) => Ordering::Greater,
        _ => {
            let length_a = matches_a[matches_a.len() - 1].end - matches_a[0].start;
            let length_b = matches_b[matches_b.len() - 1].end - matches_b[0].start;
            length_a.cmp(&length_b)
        }
    }
}

fn fallback_compare(item_a: &ScorerItem, item_b: &ScorerItem, query: &PreparedQuery) -> Ordering {
    let description_length = |item: &ScorerItem| item.description.map_or(0, |description| description.chars().count());
    let length_a = item_a.label.chars().count() + description_length(item_a);
    let length_b = item_b.label.chars().count() + description_length(item_b);
    if length_a != length_b {
        return length_a.cmp(&length_b);
    }

    let path_a = item_a.path.filter(|path| !path.is_empty());
    let path_b = item_b.path.filter(|path| !path.is_empty());
    if let (Some(path_a), Some(path_b)) = (path_a, path_b) {
        let (length_a, length_b) = (path_a.chars().count(), path_b.chars().count());
        if length_a != length_b {
            return length_a.cmp(&length_b);
        }
    }

    let look_for = &query.piece.normalized;
    if item_a.label != item_b.label {
        return filters::compare_anything(item_a.label, item_b.label, look_for);
    }
    if let (Some(description_a), Some(description_b)) = (item_a.description.filter(|d| !d.is_empty()), item_b.description.filter(|d| !d.is_empty())) {
        if description_a != description_b {
            return filters::compare_anything(description_a, description_b, look_for);
        }
    }
    if let (Some(path_a), Some(path_b)) = (path_a, path_b) {
        if path_a != path_b {
            return filters::compare_anything(path_a, path_b, look_for);
        }
    }
    Ordering::Equal
}

/// `IPreparedQueryPiece`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreparedQueryPiece {
    pub original: String,
    pub original_lowercase: String,
    /// The original with backslashes turned into slashes.
    pub path_normalized: String,
    /// `path_normalized` without whitespace, wildcards, quotes, ellipsis and a trailing `#`.
    pub normalized: String,
    pub normalized_lowercase: String,
    /// The query is wrapped in quotes: it must be a substring, no fuzzy matching.
    pub expect_contiguous_match: bool,
}

/// `IPreparedQuery`: the whole query as a piece, plus its pieces split at spaces.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreparedQuery {
    pub piece: PreparedQueryPiece,
    pub values: Option<Vec<PreparedQueryPiece>>,
    pub contains_path_separator: bool,
}

pub fn prepare_query(original: &str) -> PreparedQuery {
    let piece = prepare_piece(original);
    let contains_path_separator = piece.path_normalized.contains(SEP);
    let mut values: Option<Vec<PreparedQueryPiece>> = None;
    if original.contains(MULTIPLE_QUERY_VALUES_SEPARATOR) {
        for original_piece in original.split(MULTIPLE_QUERY_VALUES_SEPARATOR) {
            let piece = prepare_piece(original_piece);
            if !piece.normalized.is_empty() {
                values.get_or_insert_default().push(piece);
            }
        }
    }
    PreparedQuery { piece, values, contains_path_separator }
}

pub fn piece_to_query(pieces: &[PreparedQueryPiece]) -> PreparedQuery {
    let originals: Vec<&str> = pieces.iter().map(|piece| piece.original.as_str()).collect();
    prepare_query(&originals.join(" "))
}

fn prepare_piece(original: &str) -> PreparedQueryPiece {
    let path_normalized = original.replace('\\', "/");
    let mut normalized: String = path_normalized.chars().filter(|c| !matches!(c, '*' | '\u{2026}' | '"') && !is_js_whitespace(*c)).collect();
    if normalized.chars().count() > 1 && normalized.ends_with('#') {
        normalized.pop();
    }
    PreparedQueryPiece {
        original: original.to_owned(),
        original_lowercase: original.to_lowercase(),
        normalized_lowercase: normalized.to_lowercase(),
        path_normalized,
        normalized,
        expect_contiguous_match: original.len() > 0 && original.starts_with('"') && original.ends_with('"'),
    }
}

/// JavaScript's `\s`.
fn is_js_whitespace(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

fn lower_chars(chars: &[char]) -> Vec<char> {
    chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect()
}

fn equals_ignore_case(a: &str, b: &str) -> bool {
    a.chars().count() == b.chars().count() && a.chars().zip(b.chars()).all(|(a, b)| a == b || a.to_lowercase().eq(b.to_lowercase()))
}

#[cfg(test)]
mod tests;
