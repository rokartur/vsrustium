//! computeGhostText.ts, ghostText.ts, singleTextEditHelpers.ts and inlineCompletionIsVisible.ts: which part of an
//! inline completion's edit shows as ghost text at the cursor. Columns count chars (upstream counts UTF-16 units).

use diff::lcs_diff::{DiffChange, lcs_diff};

use crate::editor::Position;

/// `TextReplacement`: `start..end` of the model replaced by `text` (`\n` line breaks).
#[derive(Clone, Debug, PartialEq)]
pub struct Replacement {
    pub start: Position,
    pub end: Position,
    pub text: String,
}

/// `GhostTextPart`: text shown before the char at `column` of the ghost text's line; `preview` parts are italic.
#[derive(Clone, Debug, PartialEq)]
pub struct GhostTextPart {
    pub column: usize,
    pub text: String,
    pub preview: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GhostText {
    pub line: usize,
    pub parts: Vec<GhostTextPart>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GhostTextMode {
    Prefix,
    Subword,
    SubwordSmart,
}

impl GhostText {
    /// `render(documentText, debug)`: the text with each part inserted in brackets, for tests.
    #[cfg(test)]
    pub fn render(&self, lines: &[String]) -> String {
        let mut lines = lines.to_vec();
        let line: Vec<char> = lines[self.line].chars().collect();
        let mut out = String::new();
        let mut last = 0;
        for part in &self.parts {
            out.extend(&line[last..part.column]);
            out.push('[');
            out.push_str(&part.text);
            out.push(']');
            last = part.column;
        }
        out.extend(&line[last..]);
        lines[self.line] = out;
        lines.join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.parts.iter().all(|part| part.text.is_empty())
    }
}

fn text_in(lines: &[String], start: Position, end: Position) -> String {
    crate::editor::text_between(lines, start, end)
}

/// `TextLength.ofText(text).addToPosition(position)`.
fn position_after(position: Position, text: &str) -> Position {
    match text.rfind('\n') {
        None => Position::new(position.line, position.column + text.chars().count()),
        Some(index) => Position::new(position.line + text.matches('\n').count(), text[index + 1..].chars().count()),
    }
}

/// `singleTextRemoveCommonPrefix`.
pub fn remove_common_prefix(edit: &Replacement, lines: &[String]) -> Replacement {
    let text = edit.text.replace("\r\n", "\n");
    let replaced = text_in(lines, edit.start, edit.end);
    let common: String = replaced.chars().zip(text.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a).collect();
    let start = position_after(edit.start, &common);
    Replacement { start, end: edit.end, text: text.chars().skip(common.chars().count()).collect() }
}

fn leading_whitespace(text: &str) -> &str {
    &text[..text.len() - text.trim_start_matches([' ', '\t']).len()]
}

/// `computeGhostText`.
pub fn compute_ghost_text(edit: &Replacement, lines: &[String], mode: GhostTextMode, cursor: Option<Position>, preview_suffix_length: usize) -> Option<GhostText> {
    let mut e = remove_common_prefix(edit, lines);
    if e.end.line != e.start.line {
        return None;
    }
    let source_line = &lines[e.start.line];
    let indentation_length = leading_whitespace(source_line).chars().count();
    if e.start.column <= indentation_length {
        let added_indentation_length = leading_whitespace(&e.text).chars().count();
        let replaced_indentation: String = source_line.chars().skip(e.start.column).take(indentation_length - e.start.column).collect();
        let replaced_length = replaced_indentation.chars().count();
        let new_start = match e.start.column + replaced_length <= e.end.column {
            true => Position::new(e.start.line, e.start.column + replaced_length),
            false => e.end,
        };
        let text = match e.text.strip_prefix(&replaced_indentation) {
            Some(rest) => rest.to_owned(),
            None => e.text.chars().skip(added_indentation_length).collect(),
        };
        e = Replacement { start: new_start, end: e.end, text };
    }
    let replaced: Vec<char> = text_in(lines, e.start, e.end).chars().collect();
    let text: Vec<char> = e.text.chars().collect();
    let changes = caching_diff(&replaced, &text)?;
    if mode == GhostTextMode::Prefix {
        let inserts: Vec<&DiffChange> = changes.iter().filter(|change| change.original_length == 0).collect();
        if inserts.len() > 1 || (inserts.len() == 1 && inserts[0].original_start != replaced.len()) {
            return None;
        }
    }
    let preview_start = text.len().saturating_sub(preview_suffix_length);
    let mut parts = Vec::new();
    for change in &changes {
        let insert_column = e.start.column + change.original_start + change.original_length;
        if mode == GhostTextMode::SubwordSmart && cursor.is_some_and(|cursor| cursor.line == e.start.line && insert_column < cursor.column) {
            return None;
        }
        if change.original_length > 0 {
            return None;
        }
        if change.modified_length == 0 {
            continue;
        }
        let modified_end = change.modified_start + change.modified_length;
        let non_preview_end = change.modified_start.max(modified_end.min(preview_start));
        let non_preview: String = text[change.modified_start..non_preview_end].iter().collect();
        let italic: String = text[non_preview_end..change.modified_start.max(modified_end)].iter().collect();
        if !non_preview.is_empty() {
            parts.push(GhostTextPart { column: insert_column, text: non_preview, preview: false });
        }
        if !italic.is_empty() {
            parts.push(GhostTextPart { column: insert_column, text: italic, preview: true });
        }
    }
    Some(GhostText { line: e.start.line, parts })
}

/// `cachingDiff` without its cache: the smart bracket diff, or the plain one when it deletes fewer chars.
fn caching_diff(original: &[char], modified: &[char]) -> Option<Vec<DiffChange>> {
    let mut changes = smart_diff(original, modified, true)?;
    let deleted = deleted_chars(&changes);
    if deleted > 0
        && let Some(plain) = smart_diff(original, modified, false)
        && deleted_chars(&plain) < deleted
    {
        changes = plain;
    }
    Some(changes)
}

fn deleted_chars(changes: &[DiffChange]) -> usize {
    changes.iter().map(|change| change.original_length).sum()
}

/// `smartDiff`: parentheses become unique codes per group and level, so `if ()` matches `if (f() = 1)` as
/// `if (       )`.
pub fn smart_diff(original: &[char], modified: &[char], smart_bracket_matching: bool) -> Option<Vec<DiffChange>> {
    if original.len() > 5000 || modified.len() > 5000 {
        return None;
    }
    let max_code = original.iter().chain(modified).map(|&c| c as i32).max().unwrap_or(0);
    let elements = |source: &[char]| -> Vec<i32> {
        let (mut level, mut group) = (0, 0);
        let mut codes = Vec::with_capacity(source.len());
        for &c in source {
            codes.push(match c {
                '(' if smart_bracket_matching => {
                    let code = max_code + 2 * (group * 100 + level) + 1;
                    level += 1;
                    code
                }
                ')' if smart_bracket_matching => {
                    level = (level - 1).max(0);
                    let code = max_code + 2 * (group * 100 + level) + 2;
                    if level == 0 {
                        group += 1;
                    }
                    code
                }
                c => c as i32,
            });
        }
        codes
    };
    Some(lcs_diff(&elements(original), &elements(modified), false))
}

/// `inlineCompletionIsVisible`: the completion, less its common prefix with the model, starts on the cursor's line,
/// the text before the cursor is a prefix of it and the text after the cursor a subsequence of the rest.
pub fn inline_completion_is_visible(edit: &Replacement, original_start: Option<Position>, lines: &[String], cursor: Position) -> bool {
    let minimized = remove_common_prefix(edit, lines);
    if original_start.is_some_and(|start| start != edit.start) || cursor.line != minimized.start.line || (minimized.start == minimized.end && minimized.text.is_empty()) {
        return false;
    }
    let original: Vec<char> = text_in(lines, minimized.start, minimized.end).chars().collect();
    let filter: Vec<char> = minimized.text.chars().collect();
    let cursor_index = cursor.column.saturating_sub(minimized.start.column);
    let split = |chars: &[char]| -> (String, String) {
        let index = cursor_index.min(chars.len());
        (chars[..index].iter().collect(), chars[index..].iter().collect())
    };
    let (mut filter_before, mut filter_after) = split(&filter);
    let (mut original_before, mut original_after) = split(&original);
    let indent_column = leading_whitespace(&lines[minimized.start.line]).chars().count();
    if minimized.start.column <= indent_column {
        original_before = original_before.trim_start().to_owned();
        if original_before.is_empty() {
            original_after = original_after.trim_start().to_owned();
        }
        filter_before = filter_before.trim_start().to_owned();
        if filter_before.is_empty() {
            filter_after = filter_after.trim_start().to_owned();
        }
    }
    filter_before.starts_with(&original_before) && matches_sub_string(&original_after, &filter_after)
}

/// filters.ts `matchesSubString`: `word`'s chars appear in `target` in order, ignoring case.
fn matches_sub_string(word: &str, target: &str) -> bool {
    let mut target = target.chars().flat_map(char::to_lowercase);
    word.chars().flat_map(char::to_lowercase).all(|c| target.any(|t| t == c))
}

#[cfg(test)]
mod tests;
