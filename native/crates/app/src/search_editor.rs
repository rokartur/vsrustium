//! The search editor's text (searchEditorSerialization.ts): the query header a `.code-search` file starts with,
//! parsed back as `extractSearchQueryFromLines`, and the search results serialized as the editor shows them, with
//! the ranges of the matches in that text. Match columns are UTF-16 code units, as the search reports them.

use std::path::Path;

use crate::search_model::{FileMatch, Match, SearchResult, utf16_len};

pub const SEARCH_EDITOR_EXT: &str = ".code-search";

/// `SearchConfiguration` without the notebook filters, which the port does not search.
#[derive(Clone, PartialEq, Debug)]
pub struct SearchConfiguration {
    pub query: String,
    pub files_to_include: String,
    pub files_to_exclude: String,
    pub is_regexp: bool,
    pub is_case_sensitive: bool,
    pub use_exclude_settings_and_ignore_files: bool,
    pub match_whole_word: bool,
    pub context_lines: usize,
    pub show_includes_excludes: bool,
    pub only_open_editors: bool,
}

/// `defaultSearchConfig`.
impl Default for SearchConfiguration {
    fn default() -> Self {
        SearchConfiguration {
            query: String::new(),
            files_to_include: String::new(),
            files_to_exclude: String::new(),
            is_regexp: false,
            is_case_sensitive: false,
            use_exclude_settings_and_ignore_files: true,
            match_whole_word: false,
            context_lines: 0,
            show_includes_excludes: false,
            only_open_editors: false,
        }
    }
}

impl SearchConfiguration {
    /// The configuration with the given `OpenSearchEditorArgs` set over it, its legacy names
    /// (`translateLegacyConfig`) included.
    pub fn with_args(mut self, args: &serde_json::Value) -> Self {
        let text = |keys: [&str; 2]| keys.iter().find_map(|key| args[key].as_str().map(str::to_string));
        let flag = |keys: [&str; 2]| keys.iter().find_map(|key| args[key].as_bool());
        if let Some(query) = args["query"].as_str() {
            self.query = query.to_string();
        }
        self.files_to_include = text(["filesToInclude", "includes"]).unwrap_or(self.files_to_include);
        self.files_to_exclude = text(["filesToExclude", "excludes"]).unwrap_or(self.files_to_exclude);
        self.match_whole_word = flag(["matchWholeWord", "wholeWord"]).unwrap_or(self.match_whole_word);
        self.is_case_sensitive = flag(["isCaseSensitive", "caseSensitive"]).unwrap_or(self.is_case_sensitive);
        self.is_regexp = flag(["isRegexp", "regexp"]).unwrap_or(self.is_regexp);
        self.use_exclude_settings_and_ignore_files = flag(["useExcludeSettingsAndIgnoreFiles", "useIgnores"]).unwrap_or(self.use_exclude_settings_and_ignore_files);
        self.show_includes_excludes = args["showIncludesExcludes"].as_bool().unwrap_or(self.show_includes_excludes);
        self.only_open_editors = args["onlyOpenEditors"].as_bool().unwrap_or(self.only_open_editors);
        self.context_lines = args["contextLines"].as_u64().map_or(self.context_lines, |lines| lines as usize);
        self
    }
}

/// A range of the editor's text: 1-based lines and 1-based UTF-16 columns, as monaco's `Range`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TextRange {
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

impl TextRange {
    fn translate(self, lines: usize) -> Self {
        TextRange { start_line: self.start_line + lines, end_line: self.end_line + lines, ..self }
    }

    /// The range after the model replaced `change` (1-based, UTF-16), as a decoration with
    /// `TrackedRangeStickiness.NeverGrowsWhenTypingAtEdges` tracks it: text typed at an edge stays outside, an edge
    /// inside the replaced text moves to the side that keeps the range from growing.
    pub fn after_change(self, change: &crate::editor::ContentChange) -> Self {
        let start = (change.start_line, change.start_column);
        let end = (change.end_line, change.end_column);
        let text = change.text.replace("\r\n", "\n");
        let inserted_end = match text.rsplit_once('\n') {
            None => (start.0, start.1 + utf16_len(&text)),
            Some((_, last)) => (start.0 + text.matches('\n').count(), utf16_len(last) + 1),
        };
        let shift = |position: (usize, usize)| match position.0 == end.0 {
            true => (inserted_end.0, inserted_end.1 + position.1 - end.1),
            false => (position.0 + inserted_end.0 - end.0, position.1),
        };
        let range_start = (self.start_line, self.start_column);
        let range_end = (self.end_line, self.end_column);
        let new_start = match range_start {
            position if position >= end => shift(position),
            position if position < start => position,
            _ => inserted_end,
        };
        let new_end = match range_end {
            position if position <= start => position,
            position if position >= end => shift(position),
            _ => start,
        }
        .max(new_start);
        TextRange { start_line: new_start.0, start_column: new_start.1, end_line: new_end.0, end_column: new_end.1 }
    }
}

/// `serializeSearchConfiguration`: the header lines, the last one empty.
pub fn serialize_search_configuration(config: &SearchConfiguration) -> String {
    let escaped = config.query.replace('\\', "\\\\").replace('\n', "\\n");
    let mut lines = vec![format!("# Query: {escaped}")];
    if config.is_case_sensitive || config.match_whole_word || config.is_regexp || !config.use_exclude_settings_and_ignore_files {
        let flags = [
            (config.is_case_sensitive, "CaseSensitive"),
            (config.match_whole_word, "WordMatch"),
            (config.is_regexp, "RegExp"),
            (config.only_open_editors, "OpenEditors"),
            (!config.use_exclude_settings_and_ignore_files, "IgnoreExcludeSettings"),
        ];
        let names: Vec<&str> = flags.iter().filter(|(set, _)| *set).map(|(_, name)| *name).collect();
        lines.push(format!("# Flags: {}", names.join(" ")));
    }
    if !config.files_to_include.is_empty() {
        lines.push(format!("# Including: {}", config.files_to_include));
    }
    if !config.files_to_exclude.is_empty() {
        lines.push(format!("# Excluding: {}", config.files_to_exclude));
    }
    if config.context_lines > 0 {
        lines.push(format!("# ContextLines: {}", config.context_lines));
    }
    lines.push(String::new());
    lines.join("\n")
}

/// `extractSearchQueryFromLines`: the `# Key: value` lines of the header; a backslash in the query that escapes
/// neither `n` nor a backslash is an error.
pub fn extract_search_query_from_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Result<SearchConfiguration, String> {
    let mut config = SearchConfiguration::default();
    for line in lines {
        let Some((key, value)) = parse_directive(line) else { continue };
        match key {
            "Query" => config.query = unescape_newlines(value)?,
            "Including" => config.files_to_include = value.to_string(),
            "Excluding" => config.files_to_exclude = value.to_string(),
            "ContextLines" => config.context_lines = value.trim().parse().unwrap_or(0),
            "Flags" => {
                config.is_regexp = value.contains("RegExp");
                config.is_case_sensitive = value.contains("CaseSensitive");
                config.use_exclude_settings_and_ignore_files = !value.contains("IgnoreExcludeSettings");
                config.match_whole_word = value.contains("WordMatch");
                config.only_open_editors = value.contains("OpenEditors");
            }
            _ => {}
        }
    }
    config.show_includes_excludes = !config.files_to_include.is_empty() || !config.files_to_exclude.is_empty() || !config.use_exclude_settings_and_ignore_files;
    Ok(config)
}

/// `/^# ([^:]*): (.*)$/`: JavaScript's `.` stops at line terminators, so a line holding one does not match.
fn parse_directive(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.strip_prefix("# ")?.split_once(':')?;
    let value = value.strip_prefix(' ')?;
    (!value.contains(['\r', '\n', '\u{2028}', '\u{2029}'])).then_some((key, value))
}

fn unescape_newlines(text: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(char) = chars.next() {
        if char != '\\' {
            out.push(char);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('\\') => out.push('\\'),
            _ => return Err("All backslashes in Query string must be escaped (\\\\)".to_string()),
        }
    }
    Ok(out)
}

/// `parseSerializedSearchEditor`: the header up to its first empty line, and the results after it.
pub fn parse_serialized_search_editor(text: &str) -> Result<(SearchConfiguration, String), String> {
    let mut header = Vec::new();
    let mut body = Vec::new();
    let mut in_header = true;
    for line in text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)) {
        if in_header {
            header.push(line);
            in_header = !line.is_empty();
        } else {
            body.push(line);
        }
    }
    Ok((extract_search_query_from_lines(header)?, body.join("\n")))
}

/// The text `serializeSearchResultForEditor` gives and the ranges of its matches.
#[derive(Debug, PartialEq)]
pub struct SerializedResults {
    pub text: String,
    pub match_ranges: Vec<TextRange>,
}

/// `serializeSearchResultForEditor` in the default sort order: the counts line, the limit warning, then each
/// file's label, its match lines (`  <line>: `) and context lines (`  <line>  `), one empty line after each file.
pub fn serialize_search_result_for_editor(result: &SearchResult, limit_hit: bool, label: impl Fn(&Path) -> String) -> SerializedResults {
    let (count, files) = (result.count(), result.file_count());
    let file_count = if files > 1 { format!("{files} files") } else { "1 file".to_string() };
    let result_count = if count > 1 { format!("{count} results") } else { "1 result".to_string() };
    let mut info = vec![if count > 0 { format!("{result_count} - {file_count}") } else { "No Results".to_string() }];
    if limit_hit {
        info.push("The result set only contains a subset of all matches. Be more specific in your search to narrow down the results.".to_string());
    }
    info.push(String::new());
    let mut text = Vec::new();
    let mut match_ranges = Vec::new();
    for file in result.files.iter().filter(|file| !file.matches.is_empty()) {
        let serialized = file_to_search_result_format(file, &label(&file.path));
        match_ranges.extend(serialized.match_ranges.into_iter().map(|range| range.translate(text.len() + info.len())));
        text.extend(serialized.text);
        text.push(String::new());
    }
    info.extend(text);
    SerializedResults { text: info.join("\n"), match_ranges }
}

struct FileSerialization {
    text: Vec<String>,
    match_ranges: Vec<TextRange>,
}

/// `matchesToSearchResultFormat`: line numbers padded to the last match's end line, context lines before the
/// match lines they precede, an empty line where context lines skip ahead.
fn file_to_search_result_format(file: &FileMatch, label: &str) -> FileSerialization {
    let longest_line_number = (file.matches.last().expect("a file with matches").range.end_line + 1).to_string().len();
    let mut text = vec![format!("{label}:")];
    let mut match_ranges = Vec::new();
    let mut line_offsets = std::collections::HashMap::new();
    let mut context = file.context.iter().peekable();
    let mut last_line: Option<usize> = None;
    for found in &file.matches {
        for line in match_to_search_result_format(found, longest_line_number) {
            if !line_offsets.contains_key(&line.line_number) {
                while let Some((&number, context_text)) = context.next_if(|&(&number, _)| number < line.line_number) {
                    if last_line.is_some_and(|last| number != last + 1) {
                        text.push(String::new());
                    }
                    let padding = " ".repeat(longest_line_number.saturating_sub(number.to_string().len()));
                    text.push(format!("  {padding}{number}  {context_text}"));
                    last_line = Some(number);
                }
                line_offsets.insert(line.line_number, text.len());
                text.push(line.line);
                last_line = Some(line.line_number);
            }
            match_ranges.push(line.range.translate(line_offsets[&line.line_number]));
        }
    }
    // Upstream pads only the context lines before a match.
    for (number, context_text) in context {
        text.push(format!("  {number}  {context_text}"));
    }
    FileSerialization { text, match_ranges }
}

struct ResultLine {
    line_number: usize,
    line: String,
    /// On line 1, moved down to the line's place by the caller.
    range: TextRange,
}

/// `matchToSearchResultFormat`: one line per preview line the match spans, each with the part of the match on it.
fn match_to_search_result_format(found: &Match, longest_line_number: usize) -> Vec<ResultLine> {
    let preview_lines = found.full_preview_lines();
    let in_preview = found.range_in_preview();
    let (start_column, end_column) = (in_preview.start_column + 1, in_preview.end_column + 1);
    let single_line = in_preview.start_line == in_preview.end_line;
    let last = preview_lines.len() - 1;
    let mut lines = Vec::new();
    for (index, source_line) in preview_lines.iter().enumerate() {
        let line_number = found.range.start_line + 1 + index;
        let padding = " ".repeat(longest_line_number.saturating_sub(line_number.to_string().len()));
        let prefix = format!("  {padding}{line_number}: ");
        let offset = prefix.len();
        let stripped = source_line.strip_suffix("\r\n").or_else(|| source_line.strip_suffix('\n')).or_else(|| source_line.strip_suffix('\r')).unwrap_or(source_line);
        let line_end = utf16_len(source_line) + 1;
        let (start, end) = match (single_line, index) {
            (true, _) => (start_column, end_column),
            (false, 0) => (start_column, line_end),
            (false, index) if index == last => (1, end_column),
            _ => (1, line_end),
        };
        let range = TextRange { start_line: 1, start_column: start + offset, end_line: 1, end_column: end + offset };
        lines.push(ResultLine { line_number, line: format!("{prefix}{stripped}"), range });
    }
    lines
}

/// `SearchEditorInput.getName`: the backing file's name, else the query cut to 12 characters.
pub fn search_editor_name(backing_file: Option<&Path>, query: &str) -> String {
    if let Some(file) = backing_file {
        let name = file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        return format!("Search: {}", name.strip_suffix(SEARCH_EDITOR_EXT).unwrap_or(&name));
    }
    let query = query.trim();
    if query.is_empty() {
        return "Search".to_string();
    }
    let trimmed = match query.chars().count() < 12 {
        true => query.to_string(),
        false => format!("{}...", query.chars().take(9).collect::<String>()),
    };
    format!("Search: {trimmed}")
}

/// `suggestFileName`: the query with every run of characters other than `\w`, space, `-` and `_` as `_`.
pub fn suggest_file_name(query: &str) -> String {
    let mut name = String::new();
    let mut in_run = false;
    for char in query.chars() {
        let kept = char.is_ascii_alphanumeric() || matches!(char, '_' | ' ' | '-');
        if !kept && !in_run {
            name.push('_');
        }
        if kept {
            name.push(char);
        }
        in_run = !kept;
    }
    if name.is_empty() {
        name.push_str("Search");
    }
    name + SEARCH_EDITOR_EXT
}

#[cfg(test)]
mod tests;
