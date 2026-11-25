//! Terminal completions and their sort keys: terminalCompletionItem.ts `ITerminalCompletion` and
//! `TerminalCompletionItem`.

use serde_json::Value;

use crate::filters::FuzzyScore;

#[cfg(test)]
mod tests;

/// `TerminalCompletionItemKind`, numbered as the extension host sends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalCompletionItemKind {
    File = 0,
    Folder = 1,
    Method = 2,
    Alias = 3,
    Argument = 4,
    Option = 5,
    OptionValue = 6,
    Flag = 7,
    SymbolicLinkFile = 8,
    SymbolicLinkFolder = 9,
    Commit = 10,
    Branch = 11,
    Tag = 12,
    Stash = 13,
    Remote = 14,
    PullRequest = 15,
    PullRequestDone = 16,
    InlineSuggestion = 100,
    InlineSuggestionAlwaysOnTop = 101,
}

impl TerminalCompletionItemKind {
    pub fn from_number(number: u64) -> Option<Self> {
        use TerminalCompletionItemKind::*;
        let kind = match number {
            0 => File,
            1 => Folder,
            2 => Method,
            3 => Alias,
            4 => Argument,
            5 => Option,
            6 => OptionValue,
            7 => Flag,
            8 => SymbolicLinkFile,
            9 => SymbolicLinkFolder,
            10 => Commit,
            11 => Branch,
            12 => Tag,
            13 => Stash,
            14 => Remote,
            15 => PullRequest,
            16 => PullRequestDone,
            100 => InlineSuggestion,
            101 => InlineSuggestionAlwaysOnTop,
            _ => return None,
        };
        Some(kind)
    }
}

/// `ITerminalCompletion`.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalCompletion {
    pub label: String,
    /// The label arrived as a plain string, not a `CompletionItemLabel` object.
    pub string_label: bool,
    /// `CompletionItemLabel.description` and `.detail`, when the label is an object.
    pub label_description: Option<String>,
    pub label_detail: Option<String>,
    pub provider: String,
    pub detail: Option<String>,
    /// A plain string or the `value` of an `IMarkdownString`.
    pub documentation: Option<String>,
    pub kind: Option<TerminalCompletionItemKind>,
    /// The codicon id of the item's `ThemeIcon`; the widget falls back to the kind's icon.
    pub icon: Option<String>,
    /// Chars of the prompt value to replace, start inclusive, end exclusive.
    pub replacement_range: Option<(usize, usize)>,
    pub input_data: Option<String>,
    pub is_file_override: Option<bool>,
    pub is_keyword: bool,
}

impl TerminalCompletion {
    pub fn new(label: impl Into<String>, provider: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            string_label: true,
            label_description: None,
            label_detail: None,
            provider: provider.into(),
            detail: None,
            documentation: None,
            kind: None,
            icon: None,
            replacement_range: None,
            input_data: None,
            is_file_override: None,
            is_keyword: false,
        }
    }

    /// An `ITerminalCompletionItemDto` from the extension host, with `replacementRange` in UTF-16 units of
    /// `prompt_value`. `None` when the label is missing.
    pub fn from_dto(dto: &Value, provider: &str, prompt_value: &str) -> Option<Self> {
        let mut completion = match &dto["label"] {
            Value::String(label) => Self::new(label.as_str(), provider),
            Value::Object(label) => Self {
                string_label: false,
                label_description: label.get("description").and_then(Value::as_str).map(String::from),
                label_detail: label.get("detail").and_then(Value::as_str).map(String::from),
                ..Self::new(label.get("label")?.as_str()?, provider)
            },
            _ => return None,
        };
        completion.detail = dto["detail"].as_str().map(String::from);
        completion.documentation = match &dto["documentation"] {
            Value::String(text) => Some(text.clone()),
            Value::Object(markdown) => markdown.get("value").and_then(Value::as_str).map(String::from),
            _ => None,
        };
        completion.kind = dto["kind"].as_u64().and_then(TerminalCompletionItemKind::from_number);
        completion.icon = dto["icon"]["id"].as_str().map(String::from);
        completion.is_keyword = dto["isKeyword"].as_bool() == Some(true);
        let utf16_offset = |index: usize| dto["replacementRange"][index].as_u64().and_then(|offset| usize::try_from(offset).ok());
        if let (Some(start), Some(end)) = (utf16_offset(0), utf16_offset(1)) {
            completion.replacement_range = Some((utf16_to_chars(prompt_value, start), utf16_to_chars(prompt_value, end)));
        }
        Some(completion)
    }

    fn is_file(&self) -> bool {
        self.kind == Some(TerminalCompletionItemKind::File) || self.is_file_override == Some(true)
    }
}

/// `TerminalCompletionItem`: a completion with the lowercase variants filtering and sorting read.
#[derive(Clone, Debug)]
pub struct TerminalCompletionItem {
    pub completion: TerminalCompletion,
    pub label_low: String,
    /// `label_low` without the file extension.
    pub label_low_exclude_file_ext: String,
    /// `label_low` with `/` separators and no trailing separator for folders.
    pub label_low_normalized_path: String,
    pub file_ext_low: String,
    /// 1 for labels of only punctuation or whose basename starts with `_`.
    pub punctuation_penalty: u8,
    pub score: FuzzyScore,
    pub idx: usize,
    pub word: String,
    pub is_invalid: bool,
}

impl TerminalCompletionItem {
    /// The constructor; `path_separator` is the terminal's, detected from the label when `None`.
    pub fn new(completion: TerminalCompletion, path_separator: Option<char>) -> Self {
        let mut label_low = completion.label.to_lowercase();
        let windows_style = path_separator.unwrap_or(if label_low.contains('\\') { '\\' } else { '/' }) == '\\';
        let mut label_low_exclude_file_ext = label_low.clone();
        let mut label_low_normalized_path = label_low.clone();
        let mut file_ext_low = String::new();
        let is_folder = completion.kind == Some(TerminalCompletionItemKind::Folder);

        // Branches count as paths so `origin/main` is not filtered out (vscode#255864).
        if (completion.is_file() || completion.kind == Some(TerminalCompletionItemKind::Branch)) && windows_style {
            label_low = label_low.replace('/', "\\");
        }
        if completion.is_file() {
            // Dotfiles have no extension.
            if let Some(ext_index) = label_low.rfind('.').filter(|index| *index > 0) {
                label_low_exclude_file_ext = label_low[..ext_index].to_owned();
                file_ext_low = label_low[ext_index + 1..].to_owned();
            }
        }
        if completion.is_file() || is_folder {
            if windows_style {
                label_low_normalized_path = label_low.replace('\\', "/");
            }
            if is_folder && label_low_normalized_path.ends_with('/') {
                label_low_normalized_path.pop();
            }
        }
        let punctuation_penalty = u8::from(should_penalize_for_punctuation(&label_low_exclude_file_ext));
        Self {
            completion,
            label_low,
            label_low_exclude_file_ext,
            label_low_normalized_path,
            file_ext_low,
            punctuation_penalty,
            score: FuzzyScore::default_score(),
            idx: 0,
            word: String::new(),
            is_invalid: false,
        }
    }
}

fn should_penalize_for_punctuation(label: &str) -> bool {
    let trimmed = label.trim_end_matches('/');
    let basename = trimmed.rsplit_once('/').map_or(trimmed, |(_, basename)| basename);
    basename.starts_with('_') || (!label.is_empty() && label.chars().all(|c| "[]{}().,;:!?/\\-_@#~*%^=$".contains(c)))
}

fn utf16_to_chars(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (index, c) in text.chars().enumerate() {
        if units >= offset {
            return index;
        }
        units += c.len_utf16();
    }
    text.chars().count()
}
