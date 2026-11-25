//! Filtering and sorting of terminal completions: simpleCompletionModel.ts `SimpleCompletionModel` with
//! terminalCompletionModel.ts's compare function.

use std::cmp::Ordering;

use crate::command_palette::locale_compare;
use crate::filters::{FuzzyScore, FuzzyScoreOptions, fuzzy_score};

use super::completion_item::{TerminalCompletionItem, TerminalCompletionItemKind};

#[cfg(test)]
mod tests;

/// `LineContext`.
#[derive(Clone, Debug, PartialEq)]
pub struct LineContext {
    /// The prompt's chars before the cursor.
    pub leading_line_content: String,
    /// Chars typed since the completions were requested.
    pub character_count_delta: usize,
}

impl LineContext {
    pub fn new(leading_line_content: impl Into<String>, character_count_delta: usize) -> Self {
        Self { leading_line_content: leading_line_content.into(), character_count_delta }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Refilter {
    Nothing,
    All,
    Incremental,
}

/// `TerminalCompletionModel`.
pub struct TerminalCompletionModel {
    items: Vec<TerminalCompletionItem>,
    line_context: LineContext,
    filtered: Option<Vec<TerminalCompletionItem>>,
    refilter: Refilter,
    /// `stats.pLabelLen`: the 85th percentile of the filtered label lengths.
    label_length_percentile: usize,
}

impl TerminalCompletionModel {
    pub fn new(items: Vec<TerminalCompletionItem>, line_context: LineContext) -> Self {
        Self { items, line_context, filtered: None, refilter: Refilter::All, label_length_percentile: 0 }
    }

    /// The filtered and sorted items.
    pub fn items(&mut self) -> &[TerminalCompletionItem] {
        self.ensure_cached_state();
        self.filtered.as_deref().expect("set by create_cached_state")
    }

    /// The items as last filtered by `items`, for drawing without filtering again.
    pub fn filtered_items(&self) -> &[TerminalCompletionItem] {
        self.filtered.as_deref().unwrap_or_default()
    }

    /// `stats.pLabelLen`.
    pub fn label_length_percentile(&mut self) -> usize {
        self.ensure_cached_state();
        self.label_length_percentile
    }

    pub fn line_context(&self) -> &LineContext {
        &self.line_context
    }

    /// The `lineContext` setter: more chars than before filter the filtered items again, anything else all items.
    pub fn set_line_context(&mut self, value: LineContext) {
        if self.line_context != value {
            let more_chars = self.line_context.character_count_delta < value.character_count_delta;
            self.refilter = if more_chars && self.filtered.is_some() { Refilter::Incremental } else { Refilter::All };
            self.line_context = value;
        }
    }

    pub fn force_refilter_all(&mut self) {
        self.refilter = Refilter::All;
    }

    /// terminalSuggestAddon.ts `_refreshInlineCompletion`: the inline suggestion item (provider
    /// `core:inlineSuggestion`) takes its new contents, then everything is filtered again.
    pub fn set_inline_item(&mut self, item: TerminalCompletionItem) {
        match self.items.iter_mut().find(|existing| existing.completion.provider == item.completion.provider) {
            Some(existing) => *existing = TerminalCompletionItem { idx: existing.idx, ..item },
            None => self.items.push(item),
        }
        self.force_refilter_all();
    }

    fn ensure_cached_state(&mut self) {
        if self.refilter != Refilter::Nothing {
            self.create_cached_state();
        }
    }

    /// `_createCachedState`. `filterGraceful` is never set upstream, so items are scored with `fuzzyScore` only.
    fn create_cached_state(&mut self) {
        let leading: Vec<char> = self.line_context.leading_line_content.chars().collect();
        let character_count_delta = self.line_context.character_count_delta;
        let source = match self.refilter {
            Refilter::All => self.items.clone(),
            _ => self.filtered.take().expect("incremental refilter needs filtered items"),
        };
        let options = FuzzyScoreOptions { first_match_can_be_weak: true, ..FuzzyScoreOptions::default() };
        let mut target = Vec::new();
        let mut label_lengths = Vec::new();
        for (index, mut item) in source.into_iter().enumerate() {
            if item.is_invalid {
                continue;
            }
            let overwrite_before = item.completion.replacement_range.map_or(0, |(start, end)| end.saturating_sub(start));
            let word_len = overwrite_before + character_count_delta;
            let word: String = leading[leading.len().saturating_sub(word_len)..].iter().collect();
            item.word = word.clone();
            if word_len == 0 {
                item.score = FuzzyScore::default_score();
            } else {
                let word_chars: Vec<char> = word.chars().collect();
                let mut word_pos = 0;
                while word_pos < overwrite_before && word_chars.get(word_pos).is_some_and(|c| *c == ' ' || *c == '\t') {
                    word_pos += 1;
                }
                if word_pos >= word_len {
                    item.score = FuzzyScore::default_score();
                } else {
                    // The word is empty when the replacement range reaches before the line context.
                    match fuzzy_score(&word, &word.to_lowercase(), word_pos, &item.completion.label, &item.label_low, 0, options) {
                        Some(matched) => item.score = matched,
                        None if word.is_empty() => item.score = FuzzyScore::default_score(),
                        None => continue,
                    }
                }
            }
            item.idx = index;
            label_lengths.push(item.completion.label.chars().count());
            target.push(item);
        }
        let leading_line_content = &self.line_context.leading_line_content;
        target.sort_by(|a, b| compare_completions(leading_line_content, a, b));
        self.filtered = Some(target);
        self.refilter = Refilter::Nothing;
        // `quickSelect(length - .85, ...)` truncates to the last index: the longest label.
        self.label_length_percentile = label_lengths.into_iter().max().unwrap_or(0);
    }
}

/// terminalCompletionModel.ts `compareCompletionsFn`.
fn compare_completions(leading_line_content: &str, a: &TerminalCompletionItem, b: &TerminalCompletionItem) -> Ordering {
    use TerminalCompletionItemKind::*;
    let (a_kind, b_kind) = (a.completion.kind, b.completion.kind);
    if a_kind != b_kind {
        if a_kind == Some(InlineSuggestionAlwaysOnTop) {
            return Ordering::Less;
        }
        if b_kind == Some(InlineSuggestionAlwaysOnTop) {
            return Ordering::Greater;
        }
    }

    let score = b.score.score.cmp(&a.score.score);
    if score != Ordering::Equal {
        return score;
    }

    if a_kind != b_kind {
        if a_kind == Some(InlineSuggestion) {
            return Ordering::Less;
        }
        if b_kind == Some(InlineSuggestion) {
            return Ordering::Greater;
        }
    }

    // `__init__/` and `;` sort below other labels.
    if a.punctuation_penalty != b.punctuation_penalty {
        return a.punctuation_penalty.cmp(&b.punctuation_penalty);
    }

    // Files of the same name sort by extension, but only at the start of the command line.
    let is_arg = leading_line_content.contains(' ');
    if !is_arg && a_kind == Some(File) && b_kind == Some(File) {
        if a.label_low_exclude_file_ext != b.label_low_exclude_file_ext {
            return compare_ignoring_punctuation(&a.label_low_exclude_file_ext, &b.label_low_exclude_file_ext);
        }
        let length = a.label_low_exclude_file_ext.chars().count().cmp(&b.label_low_exclude_file_ext.chars().count());
        if length != Ordering::Equal {
            return length;
        }
        let extension = file_ext_score(&b.file_ext_low).total_cmp(&file_ext_score(&a.file_ext_low));
        if extension != Ordering::Equal {
            return extension;
        }
        let extension_length = a.file_ext_low.chars().count().cmp(&b.file_ext_low.chars().count());
        if extension_length != Ordering::Equal {
            return extension_length;
        }
    }

    // shortcut: any `git` command line boosts main and master, not only branch completions (upstream HACK).
    if a_kind == Some(Argument) && b_kind == Some(Argument) && starts_with_git(leading_line_content) {
        let is_main_or_master = |item: &TerminalCompletionItem| matches!(item.completion.label.as_str(), "main" | "master");
        match (is_main_or_master(a), is_main_or_master(b)) {
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
    }

    // Methods with more details sort first.
    if a_kind == Some(Method) && b_kind == Some(Method) {
        let has_description = |item: &TerminalCompletionItem| !item.completion.string_label && item.completion.label_description.as_deref().is_some_and(|description| !description.is_empty());
        let mut score: i32 = match (has_description(a), has_description(b)) {
            (true, false) => -2,
            (false, true) => 2,
            _ => 0,
        };
        let details = |item: &TerminalCompletionItem| {
            let detail = item.completion.detail.as_deref().is_some_and(|detail| !detail.is_empty());
            let documentation = item.completion.documentation.is_some();
            i32::from(detail) + 2 * i32::from(documentation)
        };
        score += details(b) - details(a);
        if score != 0 {
            return score.cmp(&0);
        }
    }

    // `vscode/` sorts before `vscode-.../`: shallower first, then a prefix before its extensions.
    if a_kind == Some(Folder) && b_kind == Some(Folder) && !a.label_low_normalized_path.is_empty() && !b.label_low_normalized_path.is_empty() {
        let depth = |item: &TerminalCompletionItem| item.label_low_normalized_path.matches('/').count();
        let depth = depth(a).cmp(&depth(b));
        if depth != Ordering::Equal {
            return depth;
        }
        if b.label_low_normalized_path.starts_with(&a.label_low_normalized_path) {
            return Ordering::Less;
        }
        if a.label_low_normalized_path.starts_with(&b.label_low_normalized_path) {
            return Ordering::Greater;
        }
    }

    // Methods and aliases first, then arguments, other kinds, and resources last.
    if a_kind != b_kind {
        let is_method_or_alias = |kind| kind == Some(Method) || kind == Some(Alias);
        let is_resource = |kind| matches!(kind, Some(File | Folder | SymbolicLinkFile | SymbolicLinkFolder));
        if is_method_or_alias(a_kind) && !is_method_or_alias(b_kind) {
            return Ordering::Less;
        }
        if is_method_or_alias(b_kind) && !is_method_or_alias(a_kind) {
            return Ordering::Greater;
        }
        if a_kind == Some(Argument) {
            return Ordering::Less;
        }
        if b_kind == Some(Argument) {
            return Ordering::Greater;
        }
        if is_resource(a_kind) && !is_resource(b_kind) {
            return Ordering::Greater;
        }
        if is_resource(b_kind) && !is_resource(a_kind) {
            return Ordering::Less;
        }
    }

    // Ignoring punctuation mixes dotfiles in rather than putting them all on top.
    compare_ignoring_punctuation(&a.label_low, &b.label_low)
}

/// `localeCompare(b, undefined, { ignorePunctuation: true })`: whitespace and Unicode punctuation do not count.
/// shortcut: ASCII punctuation only; non-ASCII punctuation still counts, fine for file names and flags.
fn compare_ignoring_punctuation(a: &str, b: &str) -> Ordering {
    let strip = |text: &str| -> String { text.chars().filter(|c| !c.is_whitespace() && !"!\"#%&'()*,-./:;?@[\\]_{}".contains(*c)).collect() };
    locale_compare(&strip(a), &strip(b))
}

/// `/^\s*git\b/`.
fn starts_with_git(line: &str) -> bool {
    line.trim_start().strip_prefix("git").is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
}

/// `fileExtScore` with the non-Windows table: script extensions a shell runs directly rank first.
fn file_ext_score(extension: &str) -> f64 {
    match extension {
        "ps1" | "sh" | "bash" | "zsh" | "fish" | "py" | "pl" => 0.05,
        "csh" | "ksh" => 0.04,
        "bat" | "cmd" | "exe" => -0.05,
        _ => 0.0,
    }
}
