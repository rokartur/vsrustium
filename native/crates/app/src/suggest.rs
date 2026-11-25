//! The completion items of one suggest session and their filtering: suggest.ts `CompletionItem`,
//! completionModel.ts `CompletionModel` and suggestMemory.ts `Memory.select`.

use std::cmp::Ordering;

use serde_json::Value;

use crate::editor::Position;
use crate::filters::{FuzzyScore, FuzzyScoreOptions, any_score, fuzzy_score, fuzzy_score_graceful_aggressive};

#[cfg(test)]
mod tests;

/// `CompletionItemKind.Property`, `Text`, `Snippet`.
pub const KIND_PROPERTY: u64 = 9;
pub const KIND_TEXT: u64 = 18;
pub const KIND_SNIPPET: u64 = 28;
/// `CompletionItemTag.Deprecated`.
pub const TAG_DEPRECATED: u64 = 1;
/// `CompletionItemInsertTextRule.InsertAsSnippet`.
pub const INSERT_AS_SNIPPET: u64 = 4;

/// One suggestion with the fields `_inflateSuggestDto` reads, normalized as suggest.ts's constructor does.
#[derive(Clone, Debug)]
pub struct CompletionItem {
    /// The `$registerCompletionsProvider` handle.
    pub provider: u64,
    pub position: Position,
    pub label: String,
    /// `CompletionItemLabel.detail` and `.description`, when the label is an object.
    pub label_detail: Option<String>,
    pub label_description: Option<String>,
    pub string_label: bool,
    pub kind: u64,
    pub deprecated: bool,
    pub detail: Option<String>,
    pub documentation: Value,
    pub sort_text: String,
    pub filter_text: Option<String>,
    pub preselect: bool,
    pub insert_text: String,
    pub insert_text_rules: u64,
    pub edit_start: Position,
    pub edit_insert_end: Position,
    pub edit_replace_end: Position,
    pub commit_characters: Vec<char>,
    pub additional_text_edits: Value,
    /// `{ id, arguments }` of the command run after the insert.
    pub command: Option<Value>,
    /// The `ChainedCacheId` `$resolveCompletionItem` takes.
    pub id: Value,
    /// The provider's list was `isIncomplete`.
    pub incomplete: bool,
    pub is_invalid: bool,
    pub resolved: bool,
    label_low: String,
    filter_text_low: Option<String>,
    pub score: FuzzyScore,
    /// The word the item was scored against.
    pub word: String,
    pub index: usize,
}

impl CompletionItem {
    /// `_inflateSuggestDto` then suggest.ts `CompletionItem`'s constructor: `range` converts a host range to model
    /// positions; a missing range takes `default_ranges` (insert, replace) and a missing sort text the label.
    pub fn from_dto(dto: &Value, provider: u64, supports_resolve: bool, incomplete: bool, position: Position, default_ranges: &Value, range: impl Fn(&Value) -> (Position, Position)) -> Self {
        let (label, label_detail, label_description, string_label) = match &dto["a"] {
            Value::String(label) => (label.clone(), None, None, true),
            label => (label["label"].as_str().unwrap_or_default().to_owned(), label["detail"].as_str().map(String::from), label["description"].as_str().map(String::from), false),
        };
        let command = dto["o"].as_str().map(|id| {
            let arguments = match dto["n"].as_str() {
                Some(ident) => Value::Array(vec![Value::String(ident.to_owned())]),
                None => dto["p"].clone(),
            };
            serde_json::json!({ "id": id, "title": "", "arguments": arguments })
        });
        let range_value = if dto["j"].is_object() { &dto["j"] } else { default_ranges };
        let (edit_start, edit_insert_end, edit_replace_end, mut is_invalid) = match range_value.get("insert") {
            Some(insert) => {
                let (insert_start, insert_end) = range(insert);
                let (replace_start, replace_end) = range(&range_value["replace"]);
                let invalid = insert_start.line != insert_end.line || replace_start.line != replace_end.line || insert_start.line != position.line || replace_start.line != position.line || insert_start.column != replace_start.column;
                (insert_start, insert_end, replace_end, invalid)
            }
            None => {
                let (start, end) = range(range_value);
                (start, end, end, start.line != end.line || start.line != position.line)
            }
        };
        is_invalid |= label.is_empty();
        let filter_text = dto["f"].as_str().map(String::from);
        Self {
            provider,
            position,
            label_low: label.to_lowercase(),
            label_detail,
            label_description,
            string_label,
            kind: dto["b"].as_u64().unwrap_or(KIND_PROPERTY),
            deprecated: dto["m"].as_array().is_some_and(|tags| tags.contains(&Value::from(TAG_DEPRECATED))),
            detail: dto["c"].as_str().map(String::from),
            documentation: dto["d"].clone(),
            sort_text: dto["e"].as_str().filter(|text| !text.is_empty()).unwrap_or(&label).to_owned(),
            filter_text_low: filter_text.as_ref().map(|text| text.to_lowercase()),
            filter_text,
            preselect: dto["g"] == Value::Bool(true),
            insert_text: dto["h"].as_str().unwrap_or(&label).to_owned(),
            insert_text_rules: dto["i"].as_u64().unwrap_or(0),
            edit_start,
            edit_insert_end,
            edit_replace_end,
            commit_characters: dto["k"].as_str().map(|chars| chars.chars().collect()).unwrap_or_default(),
            additional_text_edits: dto["l"].clone(),
            command,
            id: dto["x"].clone(),
            incomplete,
            is_invalid,
            resolved: !supports_resolve,
            label,
            score: FuzzyScore::default_score(),
            word: String::new(),
            index: 0,
        }
    }

    /// `mixin(suggestion, newSuggestion, true)` of a `$resolveCompletionItem` answer: the fields it carries replace
    /// the item's, the range stays.
    pub fn merge_resolved(&mut self, dto: &Value) {
        if let Some(detail) = dto["c"].as_str() {
            self.detail = Some(detail.to_owned());
        }
        if !dto["d"].is_null() {
            self.documentation = dto["d"].clone();
        }
        if let Some(text) = dto["h"].as_str() {
            self.insert_text = text.to_owned();
        }
        if let Some(rules) = dto["i"].as_u64() {
            self.insert_text_rules = rules;
        }
        if !dto["l"].is_null() {
            self.additional_text_edits = dto["l"].clone();
        }
        if let Some(id) = dto["o"].as_str() {
            let arguments = match dto["n"].as_str() {
                Some(ident) => Value::Array(vec![Value::String(ident.to_owned())]),
                None => dto["p"].clone(),
            };
            self.command = Some(serde_json::json!({ "id": id, "title": "", "arguments": arguments }));
        }
        if let Some(chars) = dto["k"].as_str() {
            self.commit_characters = chars.chars().collect();
        }
        self.resolved = true;
    }

    /// suggestWidgetDetails.ts `canExpandCompletionItem`: there is detail beyond the label or documentation.
    pub fn can_expand(&self) -> bool {
        let detail = self.detail.as_deref().filter(|detail| !detail.is_empty());
        let has_documentation = match &self.documentation {
            Value::String(text) => !text.is_empty(),
            Value::Object(markdown) => markdown.get("value").and_then(Value::as_str).is_some_and(|value| !value.is_empty()),
            _ => false,
        };
        has_documentation || detail.is_some_and(|detail| detail != self.label)
    }
}

/// `LineContext` as the model filters with it.
#[derive(Clone, Debug, PartialEq)]
pub struct LineContext {
    /// The line's chars before the cursor.
    pub leading_line_content: String,
    /// Chars typed (positive) or deleted since the session started.
    pub character_count_delta: isize,
}

/// `editor.snippetSuggestions`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SnippetSort {
    Top,
    Bottom,
    Inline,
}

#[derive(Clone, Copy, PartialEq)]
enum Refilter {
    Nothing,
    All,
    Incremental,
}

pub struct CompletionModel {
    items: Vec<CompletionItem>,
    column: usize,
    line_context: LineContext,
    filtered: Vec<CompletionItem>,
    refilter: Refilter,
    filter_graceful: bool,
    snippet_sort: SnippetSort,
    fuzzy_options: FuzzyScoreOptions,
    /// `stats.pLabelLen`: the 85th percentile of the filtered label lengths.
    pub label_length_percentile: usize,
}

impl CompletionModel {
    pub fn new(items: Vec<CompletionItem>, column: usize, line_context: LineContext, filter_graceful: bool, snippet_sort: SnippetSort, fuzzy_options: FuzzyScoreOptions) -> Self {
        Self { items, column, line_context, filtered: Vec::new(), refilter: Refilter::All, filter_graceful, snippet_sort, fuzzy_options, label_length_percentile: 0 }
    }

    pub fn line_context(&self) -> &LineContext {
        &self.line_context
    }

    /// The `lineContext` setter: more chars than before filter the filtered items again, anything else all items.
    pub fn set_line_context(&mut self, value: LineContext) {
        if self.line_context != value {
            self.refilter = if self.line_context.character_count_delta < value.character_count_delta && self.refilter != Refilter::All { Refilter::Incremental } else { Refilter::All };
            self.line_context = value;
        }
    }

    pub fn items(&mut self) -> &[CompletionItem] {
        if self.refilter != Refilter::Nothing {
            self.create_cached_state();
        }
        &self.filtered
    }

    /// The items as last filtered, for drawing: `items` filters again after a line context change.
    pub fn filtered(&self) -> &[CompletionItem] {
        &self.filtered
    }

    /// The filtered item at `index`, mutable for resolving.
    pub fn item_mut(&mut self, index: usize) -> Option<&mut CompletionItem> {
        self.filtered.get_mut(index)
    }

    /// `getItemsByProvider`: every valid item, filtered or not, by provider handle.
    pub fn provider_items(&self) -> Vec<(u64, Vec<CompletionItem>)> {
        let mut by_provider: Vec<(u64, Vec<CompletionItem>)> = Vec::new();
        for item in self.items.iter().filter(|item| !item.is_invalid) {
            match by_provider.iter_mut().find(|(provider, _)| *provider == item.provider) {
                Some((_, items)) => items.push(item.clone()),
                None => by_provider.push((item.provider, vec![item.clone()])),
            }
        }
        by_provider
    }

    /// `getIncompleteProvider`.
    pub fn incomplete_providers(&self) -> Vec<u64> {
        self.provider_items().into_iter().filter(|(_, items)| items[0].incomplete).map(|(provider, _)| provider).collect()
    }

    /// `_createCachedState`: scores each item against the word before the cursor, drops the ones that do not match,
    /// sorts the rest.
    fn create_cached_state(&mut self) {
        let LineContext { leading_line_content, character_count_delta } = &self.line_context;
        let leading: Vec<char> = leading_line_content.chars().collect();
        let source = match self.refilter {
            Refilter::All => self.items.clone(),
            _ => std::mem::take(&mut self.filtered),
        };
        let graceful = self.filter_graceful && source.len() <= 2000;
        let mut target = Vec::new();
        let mut label_lengths = Vec::new();
        for (index, mut item) in source.into_iter().enumerate() {
            if item.is_invalid {
                continue;
            }
            let overwrite_before = item.position.column as isize - item.edit_start.column as isize;
            let word_len = (overwrite_before + character_count_delta - (item.position.column as isize - self.column as isize)).max(0) as usize;
            let word: String = leading[leading.len().saturating_sub(word_len)..].iter().collect();
            let word_low = word.to_lowercase();
            item.word = word.clone();
            if word_len == 0 {
                item.score = FuzzyScore::default_score();
            } else {
                let word_chars: Vec<char> = word.chars().collect();
                let mut word_pos = 0;
                while (word_pos as isize) < overwrite_before && word_chars.get(word_pos).is_some_and(|c| *c == ' ' || *c == '\t') {
                    word_pos += 1;
                }
                let score = |pattern: &str, pattern_low: &str, target: &str, target_low: &str| match graceful {
                    true => fuzzy_score_graceful_aggressive(pattern, pattern_low, word_pos, target, target_low, 0, self.fuzzy_options),
                    false => fuzzy_score(pattern, pattern_low, word_pos, target, target_low, 0, self.fuzzy_options),
                };
                if word_pos >= word_chars.len() {
                    item.score = FuzzyScore::default_score();
                } else if let Some(filter_text) = &item.filter_text {
                    let Some(matched) = score(&word, &word_low, filter_text, item.filter_text_low.as_deref().unwrap_or_default()) else { continue };
                    if filter_text.to_lowercase() == item.label_low {
                        item.score = matched;
                    } else {
                        item.score = any_score(&word, &word_low, word_pos, &item.label, &item.label_low, 0);
                        item.score.score = matched.score;
                    }
                } else {
                    let Some(matched) = score(&word, &word_low, &item.label, &item.label_low) else { continue };
                    item.score = matched;
                }
            }
            item.index = index;
            label_lengths.push(item.label.chars().count());
            target.push(item);
        }
        let snippet_sort = self.snippet_sort;
        target.sort_by(|a, b| compare_items(a, b, snippet_sort));
        self.filtered = target;
        self.refilter = Refilter::Nothing;
        label_lengths.sort_unstable();
        // `quickSelect(length - .85, ...)` picks the element at index `floor(length - .85)`, the last one.
        self.label_length_percentile = label_lengths.last().copied().unwrap_or(0);
    }
}

/// `_compareCompletionItems` with `_compareCompletionItemsSnippetsUp/Down`: snippets first or last, then score,
/// then the order the providers gave (distance is 0 without `editor.suggest.localityBonus`).
fn compare_items(a: &CompletionItem, b: &CompletionItem, snippet_sort: SnippetSort) -> Ordering {
    if a.kind != b.kind && snippet_sort != SnippetSort::Inline {
        let snippets_first = snippet_sort == SnippetSort::Top;
        if a.kind == KIND_SNIPPET {
            return if snippets_first { Ordering::Less } else { Ordering::Greater };
        }
        if b.kind == KIND_SNIPPET {
            return if snippets_first { Ordering::Greater } else { Ordering::Less };
        }
    }
    b.score.score.cmp(&a.score.score).then(a.index.cmp(&b.index))
}

/// suggest.ts `getSuggestionComparator` with `defaultComparator`: sort text, then label, then kind; snippets first
/// or last as `editor.snippetSuggestions` says. Items are sorted with it before they reach the model.
pub fn compare_provided(a: &CompletionItem, b: &CompletionItem, snippet_sort: SnippetSort) -> Ordering {
    if a.kind != b.kind && snippet_sort != SnippetSort::Inline {
        let snippets_first = snippet_sort == SnippetSort::Top;
        if a.kind == KIND_SNIPPET {
            return if snippets_first { Ordering::Less } else { Ordering::Greater };
        }
        if b.kind == KIND_SNIPPET {
            return if snippets_first { Ordering::Greater } else { Ordering::Less };
        }
    }
    let by_sort_text = a.sort_text.to_lowercase().cmp(&b.sort_text.to_lowercase());
    let by_label = || a.label.cmp(&b.label);
    by_sort_text.then_with(by_label).then(a.kind.cmp(&b.kind))
}

/// `Memory.select` of `editor.suggestSelection: first`: the first preselected item among the top-scored ones, else
/// the first.
pub fn select_first(items: &[CompletionItem]) -> usize {
    let Some(top) = items.first().map(|item| item.score.score) else { return 0 };
    items.iter().take_while(|item| item.score.score == top).position(|item| item.preselect).unwrap_or(0)
}
