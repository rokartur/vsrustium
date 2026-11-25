//! editor/contrib/unicodeHighlighter and editor/common/services/unicodeTextModelHighlighter.ts: the characters
//! `editor.unicodeHighlight` marks, with the ambiguous and invisible character tables of base/common/strings.ts.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use super::smart_select::pattern_word_at;
use super::in_place_replace::DEFAULT_WORD_REGEXP;
use super::Position;

#[cfg(test)]
mod tests;

/// `UnicodeHighlighterOptions` with `allowedLocales` already resolved (`_os`, `_vscode`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct UnicodeHighlighterOptions {
    pub non_basic_ascii: bool,
    pub ambiguous_characters: bool,
    pub invisible_characters: bool,
    pub include_comments: bool,
    pub include_strings: bool,
    pub allowed_code_points: Vec<char>,
    pub allowed_locales: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighlightReason {
    NonBasicAscii,
    Invisible,
    Ambiguous,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct UnicodeHighlights {
    /// Line and char range of each highlighted character.
    pub ranges: Vec<(Position, Position)>,
    pub has_more: bool,
    pub ambiguous_character_count: usize,
    pub invisible_character_count: usize,
    pub non_basic_ascii_character_count: usize,
}

/// The highlight cap of `computeUnicodeHighlights`; past it the banner offers to turn the highlight off.
const MAX_RESULT_LENGTH: usize = 1000;

struct UnicodeData {
    /// Locale (`_common`, `_default`, `de`, ...) to confusable code point and the ASCII it looks like.
    ambiguous: HashMap<String, Vec<u32>>,
    invisible: HashSet<u32>,
}

static DATA: LazyLock<UnicodeData> = LazyLock::new(|| {
    let value: serde_json::Value = serde_json::from_str(include_str!("unicode_data.json")).expect("valid unicode data");
    let numbers = |value: &serde_json::Value| value.as_array().expect("array").iter().map(|n| n.as_u64().expect("code point") as u32).collect::<Vec<_>>();
    let ambiguous = value["ambiguous"].as_object().expect("ambiguous map").iter().map(|(locale, pairs)| (locale.clone(), numbers(pairs))).collect();
    UnicodeData { ambiguous, invisible: numbers(&value["invisible"]).into_iter().collect() }
});

fn pairs_to_map(pairs: &[u32]) -> HashMap<u32, u32> {
    pairs.chunks(2).map(|pair| (pair[0], pair[1])).collect()
}

/// strings.ts `AmbiguousCharacters.getInstance`: `_common` merged with the confusables every given locale shares,
/// `_default` when no locale has its own table.
pub fn ambiguous_characters(locales: &[String]) -> HashMap<u32, u32> {
    let data = &DATA.ambiguous;
    let mut filtered: Vec<&str> = locales.iter().map(String::as_str).filter(|locale| !locale.starts_with('_') && data.contains_key(*locale)).collect();
    if filtered.is_empty() {
        filtered = vec!["_default"];
    }
    let mut language_specific: Option<HashMap<u32, u32>> = None;
    for locale in filtered {
        let map = pairs_to_map(&data[locale]);
        language_specific = Some(match language_specific {
            None => map,
            Some(previous) => previous.into_iter().filter(|(key, _)| map.contains_key(key)).collect(),
        });
    }
    let mut map = pairs_to_map(&data["_common"]);
    map.extend(language_specific.unwrap_or_default());
    map
}

pub fn is_invisible_character(c: char) -> bool {
    DATA.invisible.contains(&(c as u32))
}

fn is_allowed_invisible_character(c: char) -> bool {
    matches!(c, ' ' | '\n' | '\t')
}

/// strings.ts `isBasicASCII` for one char.
fn is_basic_ascii(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\x20'..='\x7E')
}

/// `CodePointHighlighter`.
pub struct CodePointHighlighter<'a> {
    options: &'a UnicodeHighlighterOptions,
    pub ambiguous: HashMap<u32, u32>,
}

impl<'a> CodePointHighlighter<'a> {
    pub fn new(options: &'a UnicodeHighlighterOptions) -> Self {
        CodePointHighlighter { options, ambiguous: ambiguous_characters(&options.allowed_locales) }
    }

    /// `getCandidateCodePoints`: whether `c` is worth checking at all.
    fn is_candidate(&self, c: char) -> bool {
        if self.options.non_basic_ascii {
            return !is_basic_ascii(c);
        }
        if self.options.allowed_code_points.contains(&c) {
            return false;
        }
        (self.options.invisible_characters && is_invisible_character(c) && !is_allowed_invisible_character(c))
            || (self.options.ambiguous_characters && self.ambiguous.contains_key(&(c as u32)))
    }

    /// `shouldHighlightNonBasicASCII`.
    pub fn reason(&self, c: char, word_context: Option<&str>) -> Option<HighlightReason> {
        if self.options.allowed_code_points.contains(&c) {
            return None;
        }
        if self.options.non_basic_ascii {
            return Some(HighlightReason::NonBasicAscii);
        }
        let mut has_basic_ascii = false;
        let mut has_non_confusable_non_basic_ascii = false;
        for word_char in word_context.unwrap_or_default().chars() {
            let basic = is_basic_ascii(word_char);
            has_basic_ascii |= basic;
            if !basic && !self.ambiguous.contains_key(&(word_char as u32)) && !is_invisible_character(word_char) {
                has_non_confusable_non_basic_ascii = true;
            }
        }
        // Don't allow mixing weird looking characters with ASCII.
        if !has_basic_ascii && has_non_confusable_non_basic_ascii {
            return None;
        }
        if self.options.invisible_characters && !is_allowed_invisible_character(c) && is_invisible_character(c) {
            return Some(HighlightReason::Invisible);
        }
        if self.options.ambiguous_characters && self.ambiguous.contains_key(&(c as u32)) {
            return Some(HighlightReason::Ambiguous);
        }
        None
    }
}

/// `UnicodeTextModelHighlighter.computeUnicodeHighlights` over `lines` (the document, or a viewport's lines
/// starting at `first_line`).
pub fn compute_unicode_highlights(lines: &[String], first_line: usize, options: &UnicodeHighlighterOptions) -> UnicodeHighlights {
    let highlighter = CodePointHighlighter::new(options);
    let mut result = UnicodeHighlights::default();
    for (index, line) in lines.iter().enumerate() {
        // `mightContainNonBasicASCII` per line: every candidate is outside basic ASCII.
        if line.chars().all(is_basic_ascii) {
            continue;
        }
        for (column, c) in line.chars().enumerate() {
            if !highlighter.is_candidate(c) {
                continue;
            }
            // The word does not count when it ends before the character.
            let word = pattern_word_at(&DEFAULT_WORD_REGEXP, line, column).filter(|word| word.end > column);
            let word: Option<String> = word.map(|word| line.chars().skip(word.start).take(word.len()).collect());
            let Some(reason) = highlighter.reason(c, word.as_deref()) else { continue };
            match reason {
                HighlightReason::Ambiguous => result.ambiguous_character_count += 1,
                HighlightReason::Invisible => result.invisible_character_count += 1,
                HighlightReason::NonBasicAscii => result.non_basic_ascii_character_count += 1,
            }
            if result.ranges.len() >= MAX_RESULT_LENGTH {
                result.has_more = true;
                return result;
            }
            let line = first_line + index;
            result.ranges.push((Position::new(line, column), Position::new(line, column + 1)));
        }
    }
    result
}
