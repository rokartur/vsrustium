//! The search view's results (contrib/search/browser/searchTreeModel): file matches holding the line matches a text
//! search found, their previews, the replace string of a match (services/search/common/replace.ts) and the
//! messages searchView.ts builds from the counts. Columns are UTF-16 code units, as the search reports them.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use search::text_search::SearchRange;
use serde_json::Value;

use crate::editor::{Position, utf16_column};
use crate::editor::find::replace_pattern::replace_with_case_preserved;
use crate::editor::find::search::{SearchParams, escape_regexp_characters, find_matches, find_next_match};
use crate::explorer::compare_file_names;

/// `MatchImpl`: one range of a text search match, with the preview lines it was found in.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    /// Where the match is in the file, 0-based lines and UTF-16 columns.
    pub range: SearchRange,
    preview_lines: Vec<String>,
    preview_range: SearchRange,
}

/// `FileMatchImpl`: a file's matches, sorted by their start.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FileMatch {
    pub path: PathBuf,
    /// The index of the workspace folder the file is in.
    pub folder: usize,
    pub matches: Vec<Match>,
    /// `_removedTextMatches`: dismissed matches a new search of the model leaves out.
    pub removed: Vec<Match>,
    /// `_context`: the surrounding lines a search with context reported, by 1-based line number.
    pub context: BTreeMap<usize, String>,
}

/// `MatchImpl.preview()`: the text around the match as the result row shows it.
#[derive(Debug, PartialEq)]
pub struct Preview {
    pub before: String,
    pub full_before: String,
    pub inside: String,
    pub after: String,
}

/// `SearchResultImpl`: the file matches in `searchMatchComparer` order.
#[derive(Default)]
pub struct SearchResult {
    pub files: Vec<FileMatch>,
}

const MAX_PREVIEW_CHARS: usize = 250;
/// `DEFAULT_MAX_SEARCH_RESULTS`.
const DEFAULT_MAX_SEARCH_RESULTS: usize = 20000;

impl Match {
    pub fn new(preview_lines: Vec<String>, preview_range: SearchRange, range: SearchRange) -> Self {
        Match { range, preview_lines, preview_range }
    }

    /// `_oneLinePreviewText`.
    pub fn text(&self) -> &str {
        &self.preview_lines[self.preview_range.start_line]
    }

    /// `_rangeInPreviewText`: the match's columns in its first preview line, to the line's end when it spans lines.
    fn columns_in_preview(&self) -> (usize, usize) {
        let end = match self.preview_range.start_line == self.preview_range.end_line {
            true => self.preview_range.end_column,
            false => utf16_len(self.text()),
        };
        (self.preview_range.start_column, end)
    }

    /// `fullPreviewLines`: the preview lines the match spans.
    pub fn full_preview_lines(&self) -> &[String] {
        &self.preview_lines[self.preview_range.start_line..=self.preview_range.end_line]
    }

    /// `rangeInPreview`: the match in its preview lines, 0-based lines and UTF-16 columns.
    pub fn range_in_preview(&self) -> &SearchRange {
        &self.preview_range
    }

    /// `getMatchString`.
    pub fn match_string(&self) -> String {
        let (start, end) = self.columns_in_preview();
        utf16_slice(self.text(), start, end)
    }

    /// `MatchImpl.preview`.
    pub fn preview(&self) -> Preview {
        let text = self.text();
        let (start, end) = self.columns_in_preview();
        let full_before = utf16_slice(text, 0, start);
        let before = lcut(&full_before, 26, "\u{2026}");
        let mut chars_remaining = MAX_PREVIEW_CHARS.saturating_sub(utf16_len(&before));
        let inside = utf16_slice(&self.match_string(), 0, chars_remaining);
        chars_remaining = chars_remaining.saturating_sub(utf16_len(&inside));
        let after = utf16_slice(&utf16_slice(text, end, usize::MAX), 0, chars_remaining);
        Preview { before, full_before, inside, after }
    }

    /// `fullMatchText`: the matched text, or every preview line with `include_surrounding`.
    pub fn full_match_text(&self, include_surrounding: bool) -> String {
        if include_surrounding {
            return self.preview_lines.join("\n");
        }
        let range = &self.preview_range;
        let mut lines: Vec<String> = self.preview_lines[range.start_line..=range.end_line].to_vec();
        let last = lines.len() - 1;
        lines[last] = utf16_slice(&lines[last], 0, range.end_column);
        lines[0] = utf16_slice(&lines[0], range.start_column, usize::MAX);
        lines.join("\n")
    }

    /// `MatchImpl.replaceString`: the replacement, retried without `\r` and then with the surrounding preview for a
    /// lookahead; the raw replace pattern when nothing matches (a multiline match).
    pub fn replace_string(&self, pattern: &ReplacePattern, preserve_case: bool) -> String {
        let full = self.full_match_text(false);
        let surrounding = self.full_match_text(true);
        for text in [&full, &full.replace("\r\n", "\n"), &surrounding, &surrounding.replace("\r\n", "\n")] {
            if let Some(replace_string) = pattern.replace_string(text, preserve_case) {
                return replace_string;
            }
        }
        pattern.pattern.clone()
    }
}

impl FileMatch {
    pub fn name(&self) -> String {
        self.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// `updateMatchesForModel`: the matches found again in the model's lines (`editorMatchesToTextSearchResults`), each
    /// previewed by the lines it spans; dismissed ones stay out.
    pub fn update_from_lines(&mut self, lines: &[String], params: &SearchParams) {
        let found = find_matches(lines, params, Position::default()..Position::new(lines.len() - 1, lines[lines.len() - 1].chars().count()), false, DEFAULT_MAX_SEARCH_RESULTS);
        self.matches.clear();
        for found in found {
            let (start, end) = (found.range.start, found.range.end);
            let preview_lines = lines[start.line..=end.line].to_vec();
            let start_column = utf16_column(&lines[start.line], start.column);
            let end_column = utf16_column(&lines[end.line], end.column);
            let preview = SearchRange { start_line: 0, start_column, end_line: end.line - start.line, end_column };
            let range = SearchRange { start_line: start.line, start_column, end_line: end.line, end_column };
            let new = Match::new(preview_lines, preview, range);
            if !self.removed.iter().any(|removed| removed.range == new.range && removed.match_string() == new.match_string()) {
                self.add(new);
            }
        }
    }

    /// `FileMatchImpl.add`: a match with the same range and text is already there.
    fn add(&mut self, new: Match) {
        if self.matches.iter().any(|known| known.range == new.range && known.match_string() == new.match_string()) {
            return;
        }
        let index = self.matches.partition_point(|known| compare_starts(&known.range, &new.range) != Ordering::Greater);
        self.matches.insert(index, new);
    }

    /// `getTextSearchMatchWithModelContext`: the `context` lines of a model before and after each match, never
    /// repeating a line of the match before or reaching into the next one.
    pub fn add_model_context(&mut self, lines: &[String], context: usize) {
        if context == 0 {
            return;
        }
        let mut previous_end = None;
        for (index, found) in self.matches.iter().enumerate() {
            let (start, end) = (found.range.start_line, found.range.end_line);
            let first = start.saturating_sub(context).max(previous_end.map_or(0, |line: usize| line + 1));
            let next_start = self.matches.get(index + 1).map_or(usize::MAX, |next| next.range.start_line);
            let last = (end + context).min(next_start.saturating_sub(1)).min(lines.len() - 1);
            for line in (first..start).chain(end + 1..=last) {
                self.context.insert(line + 1, lines[line].clone());
            }
            previous_end = Some(end);
        }
    }
}

impl SearchResult {
    /// `SearchResultImpl.add`: the `IFileMatch` results the search reported for `path`, each match's range locations
    /// one `Match` (`textSearchResultToMatches`); context lines go to the file's context (`addContext`).
    pub fn add(&mut self, folder: usize, path: &Path, results: &[Value]) {
        let index = self.file_index(folder, path);
        for result in results {
            if let (Some(line_number), Some(text)) = (result["lineNumber"].as_u64(), result["text"].as_str()) {
                self.files[index].context.insert(line_number as usize, text.to_string());
                continue;
            }
            let Some(locations) = result["rangeLocations"].as_array() else { continue };
            let preview_lines: Vec<String> = result["previewText"].as_str().unwrap_or_default().split('\n').map(str::to_string).collect();
            for location in locations {
                let (Some(preview), Some(source)) = (search_range(&location["preview"]), search_range(&location["source"])) else { continue };
                if preview.start_line >= preview_lines.len() || preview.end_line >= preview_lines.len() {
                    continue;
                }
                self.files[index].add(Match::new(preview_lines.clone(), preview, source));
            }
        }
        if self.files[index].matches.is_empty() {
            self.files.remove(index);
        }
    }

    /// `getOpenEditorResults` and the file match bound to its model: the file's matches found in the model's lines; a
    /// file left without matches goes.
    pub fn set_model_matches(&mut self, folder: usize, path: &Path, lines: &[String], params: &SearchParams) {
        let index = self.file_index(folder, path);
        self.files[index].update_from_lines(lines, params);
        if self.files[index].matches.is_empty() {
            self.files.remove(index);
        }
    }

    fn file_index(&mut self, folder: usize, path: &Path) -> usize {
        if let Some(index) = self.files.iter().position(|file| file.path == path) {
            return index;
        }
        let new = FileMatch { path: path.to_path_buf(), folder, ..FileMatch::default() };
        let index = self.files.partition_point(|known| compare_file_matches(known, &new) != Ordering::Greater);
        self.files.insert(index, new);
        index
    }

    /// `count`: the matches of every file.
    pub fn count(&self) -> usize {
        self.files.iter().map(|file| file.matches.len()).sum()
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// `FileMatchImpl.remove`: the file goes when its last match does.
    pub fn remove_match(&mut self, file: usize, index: usize) {
        let removed = self.files[file].matches.remove(index);
        self.files[file].removed.push(removed);
        if self.files[file].matches.is_empty() {
            self.files.remove(file);
        }
    }

    pub fn remove_file(&mut self, file: usize) {
        self.files.remove(file);
    }
}

fn search_range(value: &Value) -> Option<SearchRange> {
    let number = |key: &str| value[key].as_u64().map(|number| number as usize);
    Some(SearchRange { start_line: number("startLineNumber")?, start_column: number("startColumn")?, end_line: number("endLineNumber")?, end_column: number("endColumn")? })
}

fn compare_starts(one: &SearchRange, other: &SearchRange) -> Ordering {
    (one.start_line, one.start_column).cmp(&(other.start_line, other.start_column)).then((one.end_line, one.end_column).cmp(&(other.end_line, other.end_column)))
}

/// `searchMatchComparer` for two file matches in the default sort order: the workspace folder's position, then
/// `comparePaths`, then `compareFileNames`.
pub fn compare_file_matches(one: &FileMatch, other: &FileMatch) -> Ordering {
    one.folder.cmp(&other.folder).then_with(|| compare_paths(&one.path.to_string_lossy(), &other.path.to_string_lossy())).then_with(|| compare_file_names(&one.name(), &other.name()))
}

/// `comparePaths` (base/common/comparers.ts): folder by folder, lowercased, a shallower path first; the file names
/// last with `compareFileNames`.
pub fn compare_paths(one: &str, other: &str) -> Ordering {
    let one_parts: Vec<&str> = one.split('/').collect();
    let other_parts: Vec<&str> = other.split('/').collect();
    let (last_one, last_other) = (one_parts.len() - 1, other_parts.len() - 1);
    for index in 0.. {
        match (index == last_one, index == last_other) {
            (true, true) => return compare_file_names(one_parts[index], other_parts[index]),
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            (false, false) => {}
        }
        let (a, b) = (one_parts[index].to_lowercase(), other_parts[index].to_lowercase());
        // JavaScript compares strings by UTF-16 code units.
        let order = a.encode_utf16().cmp(b.encode_utf16());
        if order != Ordering::Equal {
            return order;
        }
    }
    unreachable!("the loop returns at the last part of the shorter path")
}

/// replace.ts `ReplacePattern` built from a search pattern (`IPatternInfo`).
pub struct ReplacePattern {
    /// The replace string, `\n` `\t` `\\` and `$0` resolved when the search is a regex.
    pub pattern: String,
    has_parameters: bool,
    search: String,
    match_case: bool,
}

/// The search options a replace pattern matches with.
pub struct PatternInfo<'a> {
    pub pattern: &'a str,
    pub is_reg_exp: bool,
    pub is_case_sensitive: bool,
    pub is_word_match: bool,
}

/// One match of the search in a text, in chars: its start and its groups, group 0 first, an unmatched one "".
struct TextMatch {
    start: usize,
    groups: Vec<String>,
}

impl ReplacePattern {
    pub fn new(replace: &str, info: &PatternInfo) -> Self {
        // `strings.createRegExp`: whole words add `\b` next to a word character at either end.
        let mut search = match info.is_reg_exp {
            true => info.pattern.to_string(),
            false => escape_regexp_characters(info.pattern),
        };
        if info.is_word_match {
            if search.chars().next().is_some_and(is_word_char) {
                search.insert_str(0, "\\b");
            }
            if search.chars().last().is_some_and(is_word_char) {
                search.push_str("\\b");
            }
        }
        let (pattern, has_parameters) = match info.is_reg_exp {
            true => parse_replace_string(replace),
            false => (replace.to_string(), false),
        };
        ReplacePattern { pattern, has_parameters, search, match_case: info.is_case_sensitive }
    }

    /// `getReplaceString`: the replacement for the first match in `text`, `None` without one.
    pub fn replace_string(&self, text: &str, preserve_case: bool) -> Option<String> {
        let found = self.first_match(text)?;
        let replace_string = self.build_replace_string(&found.groups, preserve_case);
        if !self.has_parameters {
            return Some(replace_string);
        }
        let replaced = replace_with_case_operations(text, &found, &replace_string);
        let matched = &found.groups[0];
        if matched == text {
            return Some(replaced);
        }
        // The replacement is what took the match's place in the text.
        let text_len = text.chars().count();
        let replaced_len = replaced.chars().count();
        let len = (matched.chars().count() + replaced_len).saturating_sub(text_len);
        Some(replaced.chars().skip(found.start).take(len).collect())
    }

    /// `buildReplaceString`.
    fn build_replace_string(&self, groups: &[String], preserve_case: bool) -> String {
        match preserve_case {
            true => replace_with_case_preserved(Some(groups), &self.pattern),
            false => self.pattern.clone(),
        }
    }

    fn first_match(&self, text: &str) -> Option<TextMatch> {
        let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        let params = SearchParams { search_string: &self.search, is_regex: true, match_case: self.match_case, word_separators: None };
        let found = find_next_match(&lines, &params, Position::new(0, 0), true)?;
        // `find_next_match` wraps around; the first match of the text is at or after the start anyway.
        let start = char_offset(&lines, found.range.start);
        Some(TextMatch { start, groups: found.matches.expect("captured") })
    }
}

fn char_offset(lines: &[String], position: Position) -> usize {
    lines[..position.line].iter().map(|line| line.chars().count() + 1).sum::<usize>() + position.column
}

fn is_word_char(char: char) -> bool {
    char.is_ascii_alphanumeric() || char == '_'
}

/// replace.ts `parseReplaceString`: `\\` `\n` `\t` resolved, `$0` as `$&`; whether the string has `$` parameters.
/// The char after any `\` or `$` is skipped, as upstream's index arithmetic does.
fn parse_replace_string(replace: &str) -> (String, bool) {
    let chars: Vec<char> = replace.chars().collect();
    let len = chars.len();
    let digit = |index: usize, from: char| (from..='9').contains(&chars[index]);
    let mut result = String::new();
    let mut has_parameters = false;
    let mut substr_from = 0;
    let mut i = 0;
    while i < len {
        let char = chars[i];
        if char == '\\' {
            i += 1;
            if i >= len {
                break;
            }
            let replace_with = match chars[i] {
                '\\' => Some('\\'),
                'n' => Some('\n'),
                't' => Some('\t'),
                _ => None,
            };
            if let Some(replace_with) = replace_with {
                result.extend(&chars[substr_from..i - 1]);
                result.push(replace_with);
                substr_from = i + 1;
            }
        }
        if char == '$' {
            i += 1;
            if i >= len {
                break;
            }
            match chars[i] {
                '0' => {
                    result.extend(&chars[substr_from..i - 1]);
                    result.push_str("$&");
                    substr_from = i + 1;
                    has_parameters = true;
                }
                '`' | '\'' => has_parameters = true,
                // `$n` and `$nn` are parameters, `$nnn` is not.
                '1'..='9' => {
                    if i == len - 1 || !digit(i + 1, '0') {
                        has_parameters = true;
                    } else if i + 1 == len - 1 || !digit(i + 2, '0') {
                        has_parameters = true;
                        i += 1;
                    } else {
                        i += 2;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    if substr_from == 0 {
        return (replace.to_string(), has_parameters);
    }
    result.extend(&chars[substr_from..]);
    (result, has_parameters)
}

/// `replaceWithCaseOperations`: `\u` `\U` `\l` `\L` before a `$n` change the case of that group, then the text's
/// first match is replaced as `String.replace` does.
fn replace_with_case_operations(text: &str, found: &TextMatch, replace_string: &str) -> String {
    if !has_case_operation(replace_string) {
        return js_replace(text, found, replace_string);
    }
    let chars: Vec<char> = replace_string.chars().collect();
    let mut result = String::new();
    let mut index = 0;
    // `/([\s\S]*?)((?:\\[uUlL])+?|)(\$[0-9]+)([\s\S]*?)/g`: each `$n` with the case operations right before it.
    while let Some((prefix_end, group_start, group_end)) = next_group_reference(&chars, index) {
        let prefix: String = chars[index..prefix_end].iter().collect();
        let case_ops: Vec<char> = chars[prefix_end..group_start].iter().filter(|char| **char != '\\').copied().collect();
        let reference: String = chars[group_start..group_end].iter().collect();
        index = group_end;
        let group = reference[1..].parse::<usize>().ok().and_then(|number| found.groups.get(number)).filter(|group| !group.is_empty());
        let Some(group) = group.filter(|_| !case_ops.is_empty()) else {
            result.push_str(&prefix);
            result.extend(&chars[prefix_end..group_start]);
            result.push_str(&reference);
            continue;
        };
        result.push_str(&prefix);
        let group: Vec<char> = group.chars().collect();
        let mut position = 0;
        while position < case_ops.len() {
            match case_ops[position] {
                'U' => {
                    result.extend(group.iter().skip(position).flat_map(|char| char.to_uppercase()));
                    position = group.len();
                }
                'L' => {
                    result.extend(group.iter().skip(position).flat_map(|char| char.to_lowercase()));
                    position = group.len();
                }
                'u' => result.extend(group.get(position).into_iter().flat_map(|char| char.to_uppercase())),
                _ => result.extend(group.get(position).into_iter().flat_map(|char| char.to_lowercase())),
            }
            position += 1;
        }
        if position < group.len() {
            result.extend(&group[position..]);
        }
    }
    result.extend(&chars[index..]);
    js_replace(text, found, &result)
}

fn has_case_operation(value: &str) -> bool {
    value.as_bytes().windows(2).any(|pair| pair[0] == b'\\' && matches!(pair[1], b'u' | b'U' | b'l' | b'L'))
}

/// The next `$n` at or after `from`: where the case operations right before it start, and the reference's span.
fn next_group_reference(chars: &[char], from: usize) -> Option<(usize, usize, usize)> {
    let dollar = (from..chars.len()).find(|&index| chars[index] == '$' && chars.get(index + 1).is_some_and(char::is_ascii_digit))?;
    let mut end = dollar + 1;
    while chars.get(end).is_some_and(char::is_ascii_digit) {
        end += 1;
    }
    let mut ops_start = dollar;
    while ops_start >= from + 2 && chars[ops_start - 2] == '\\' && matches!(chars[ops_start - 1], 'u' | 'U' | 'l' | 'L') {
        ops_start -= 2;
    }
    Some((ops_start, dollar, end))
}

/// `text.replace(regex, replacement)` for the first match: `GetSubstitution` reads `$$`, `$&`, `` $` ``, `$'` and
/// `$n`/`$nn` for the groups the regex has.
fn js_replace(text: &str, found: &TextMatch, replacement: &str) -> String {
    let text: Vec<char> = text.chars().collect();
    let match_end = found.start + found.groups[0].chars().count();
    let before: String = text[..found.start].iter().collect();
    let after: String = text[match_end..].iter().collect();
    let group_count = found.groups.len() - 1;
    let chars: Vec<char> = replacement.chars().collect();
    let mut result = before.clone();
    let mut index = 0;
    while index < chars.len() {
        let char = chars[index];
        index += 1;
        if char != '$' || index == chars.len() {
            result.push(char);
            continue;
        }
        match chars[index] {
            '$' => result.push('$'),
            '&' => result.push_str(&found.groups[0]),
            '`' => result.push_str(&before),
            '\'' => result.push_str(&after),
            digit if digit.is_ascii_digit() => {
                let one = digit.to_digit(10).expect("digit") as usize;
                let two = chars.get(index + 1).and_then(|next| next.to_digit(10)).map(|next| one * 10 + next as usize);
                match two.filter(|two| (1..=group_count).contains(two)) {
                    Some(two) => {
                        result.push_str(&found.groups[two]);
                        index += 1;
                    }
                    None if (1..=group_count).contains(&one) => result.push_str(&found.groups[one]),
                    None => {
                        result.push('$');
                        continue;
                    }
                }
            }
            _ => {
                result.push('$');
                continue;
            }
        }
        index += 1;
    }
    result.push_str(&after);
    result
}

/// `strings.lcut`: the end of `text` cut at a word boundary to fewer than `n` UTF-16 units, `prefix` before it.
pub fn lcut(text: &str, n: usize, prefix: &str) -> String {
    let trimmed: Vec<u16> = text.trim_start().encode_utf16().collect();
    if trimmed.len() < n {
        return String::from_utf16_lossy(&trimmed);
    }
    let is_word = |unit: u16| unit < 128 && is_word_char(unit as u8 as char);
    let boundary = |index: usize| {
        let left = index > 0 && is_word(trimmed[index - 1]);
        let right = index < trimmed.len() && is_word(trimmed[index]);
        left != right
    };
    let mut cut = 0;
    for index in 0..=trimmed.len() {
        if !boundary(index) {
            continue;
        }
        if trimmed.len() - index < n {
            break;
        }
        cut = index;
    }
    if cut == 0 {
        return String::from_utf16_lossy(&trimmed);
    }
    format!("{prefix}{}", String::from_utf16_lossy(&trimmed[cut..]).trim_start())
}

pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `text.substring(start, end)` in UTF-16 units, both clamped to the text.
pub fn utf16_slice(text: &str, start: usize, end: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let end = end.min(units.len());
    let start = start.min(end);
    String::from_utf16_lossy(&units[start..end])
}

/// `buildResultCountMessage`.
pub fn result_count_message(results: usize, files: usize) -> String {
    let results_word = if results == 1 { "result" } else { "results" };
    let files_word = if files == 1 { "file" } else { "files" };
    format!("{results} {results_word} in {files} {files_word}")
}

/// `buildReplaceAllConfirmationMessage`: the replace value cut to 10 lines.
pub fn replace_all_confirmation_message(occurrences: usize, files: usize, replace_value: &str) -> String {
    if replace_value.is_empty() {
        return format!("Replace {}?", occurrences_across(occurrences, files));
    }
    let lines: Vec<&str> = replace_value.split('\n').collect();
    let display = match lines.len() > 10 {
        true => format!("{}\n...", lines[..10].join("\n")),
        false => replace_value.to_string(),
    };
    format!("Replace {} with '{display}'?", occurrences_across(occurrences, files))
}

/// `buildAfterReplaceAllMessage`.
pub fn after_replace_all_message(occurrences: usize, files: usize, replace_value: &str) -> String {
    match replace_value.is_empty() {
        true => format!("Replaced {}.", occurrences_across(occurrences, files)),
        false => format!("Replaced {} with '{replace_value}'.", occurrences_across(occurrences, files)),
    }
}

fn occurrences_across(occurrences: usize, files: usize) -> String {
    let occurrence_word = if occurrences == 1 { "occurrence" } else { "occurrences" };
    let file_word = if files == 1 { "file" } else { "files" };
    format!("{occurrences} {occurrence_word} across {files} {file_word}")
}

/// What a link of the search view's message line does (`SearchLinkButton`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MessageLink {
    SearchAgain,
    SearchAgainInAllFiles,
    OpenSettings,
    EnableExcludes,
    DisableOpenEditors,
    OpenInEditor,
    OpenFolder,
}

impl MessageLink {
    pub fn label(self) -> &'static str {
        match self {
            MessageLink::SearchAgain => "Search again",
            MessageLink::SearchAgainInAllFiles => "Search again in all files",
            MessageLink::OpenSettings => "Open Settings",
            MessageLink::EnableExcludes => "enable",
            MessageLink::DisableOpenEditors => "disable",
            MessageLink::OpenInEditor => "Open in editor",
            MessageLink::OpenFolder => "Open Folder",
        }
    }
}

/// `onSearchComplete` without results: what was searched, and the link that follows the message.
pub fn no_results_message(completed: bool, only_open_editors: bool, include: &str, exclude: &str) -> (String, MessageLink) {
    if !completed {
        return ("Search was canceled before any results could be found - ".to_string(), MessageLink::SearchAgain);
    }
    let link = if include.is_empty() && exclude.is_empty() { MessageLink::OpenSettings } else { MessageLink::SearchAgainInAllFiles };
    if only_open_editors {
        let message = match (include.is_empty(), exclude.is_empty()) {
            (false, false) => format!("No results found in open editors matching '{include}' excluding '{exclude}' - "),
            (false, true) => format!("No results found in open editors matching '{include}' - "),
            (true, false) => format!("No results found in open editors excluding '{exclude}' - "),
            (true, true) => "No results found in open editors. Review your configured exclusions and check your gitignore files - ".to_string(),
        };
        return (message, link);
    }
    match (include.is_empty(), exclude.is_empty()) {
        (false, false) => (format!("No results found in '{include}' excluding '{exclude}' - "), link),
        (false, true) => (format!("No results found in '{include}' - "), link),
        (true, false) => (format!("No results found excluding '{exclude}' - "), link),
        (true, true) => ("No results found. Review your configured exclusions and check your gitignore files - ".to_string(), link),
    }
}

#[cfg(test)]
mod tests;
