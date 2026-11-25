//! Snippet files and snippet completions (snippetsFile.ts, snippetsService.ts, snippetCompletionProvider.ts).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::editor::{Indentation, expand_snippet, utf16_column, word_at};
use crate::filters::is_pattern_in_word;
use crate::suggest::{INSERT_AS_SNIPPET, KIND_SNIPPET};
use crate::theme::strip_jsonc;

pub const MARK_SNIPPET_AS_USED: &str = "_snippet.markAsUsed";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SnippetSource {
    User = 1,
    Workspace = 2,
    Extension = 3,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snippet {
    pub is_file_template: bool,
    pub scopes: Vec<String>,
    pub name: String,
    pub prefix: String,
    pub description: String,
    // shortcut: no `SnippetBodyInsights` (bogus-variable rewrite, isTrivial, needsClipboard): `body` is the code snippet;
    // port them when the snippet parser exposes its marker tree.
    pub body: String,
    /// The label shown in the detail: the extension's name, "Workspace Snippet", "Global User Snippet" or "User Snippet".
    pub source: String,
    pub snippet_source: SnippetSource,
    pub snippet_identifier: String,
    pub include: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
}

impl Snippet {
    /// `usesSelectionVariable`: the body has `$SELECTION` or `$TM_SELECTED_TEXT`.
    pub fn uses_selection(&self) -> bool {
        crate::editor::snippet::variable_names(&self.body).iter().any(|name| name == "SELECTION" || name == "TM_SELECTED_TEXT")
    }

    /// `needsClipboard`: the body has `$CLIPBOARD`.
    pub fn needs_clipboard(&self) -> bool {
        crate::editor::snippet::variable_names(&self.body).iter().any(|name| name == "CLIPBOARD")
    }

    /// `Snippet.isFileIncluded`: exclude globs first, then include globs, case-insensitive; a pattern with a '/'
    /// matches the whole path, any other the file name.
    pub fn is_file_included(&self, path: &str) -> bool {
        let file_name = path.rsplit('/').next().unwrap_or(path);
        let options = files::glob::Options { ignore_case: true, ..files::glob::Options::default() };
        let matches = |pattern: &String| files::glob::match_pattern(pattern, if pattern.contains('/') { path } else { file_name }, options);
        if let Some(exclude) = &self.exclude
            && exclude.iter().any(matches)
        {
            return false;
        }
        match &self.include {
            Some(include) => include.iter().any(matches),
            None => true,
        }
    }
}

#[derive(Debug)]
pub struct SnippetFile {
    pub source: SnippetSource,
    pub location: PathBuf,
    pub default_scopes: Option<Vec<String>>,
    pub is_global_snippets: bool,
    pub is_user_snippets: bool,
    pub data: Vec<Snippet>,
}

impl SnippetFile {
    /// `new SnippetFile(...).load()`: `extension` is the extension's label (displayName or name) and folder. A file
    /// that is not a JSON object, or that upstream's parse throws on, has no snippets.
    pub fn parse(source: SnippetSource, location: &Path, default_scopes: Option<Vec<String>>, extension: Option<(&str, &Path)>, text: &str) -> SnippetFile {
        let mut file = SnippetFile {
            source,
            location: location.to_path_buf(),
            default_scopes,
            is_global_snippets: location.extension().is_some_and(|extension| extension == "code-snippets"),
            is_user_snippets: extension.is_none(),
            data: Vec::new(),
        };
        let Ok(Value::Object(entries)) = serde_json::from_str::<Value>(&strip_jsonc(text)) else { return file };
        let mut data = Vec::new();
        for (name, scope_or_template) in &entries {
            let parsed = match scope_or_template {
                Value::Object(snippet) if is_truthy(&snippet.get("body")) => file.parse_snippet(name, snippet, extension, &mut data),
                _ => js_entries(scope_or_template).and_then(|entries| entries.into_iter().try_for_each(|(name, template)| file.parse_template(&name, template, extension, &mut data))),
            };
            if parsed.is_none() {
                return file;
            }
        }
        file.data = data;
        file
    }

    /// `select`: user language files (`<lang>.json`) by their name, every other file by the snippets' scopes.
    pub fn select<'a>(&'a self, selector: &str, bucket: &mut Vec<&'a Snippet>) {
        if self.is_global_snippets || !self.is_user_snippets {
            self.scope_select(selector, bucket);
        } else if self.location.file_name().is_some_and(|name| name.to_string_lossy() == format!("{selector}.json")) {
            bucket.extend(&self.data);
        }
    }

    /// `_scopeSelect`: unscoped snippets match every selector; "a.b" also selects the snippets of "a".
    fn scope_select<'a>(&'a self, selector: &str, bucket: &mut Vec<&'a Snippet>) {
        for snippet in &self.data {
            if snippet.scopes.is_empty() || snippet.scopes.iter().any(|scope| scope == selector) {
                bucket.push(snippet);
            }
        }
        if let Some(index) = selector.rfind('.') {
            self.scope_select(&selector[..index], bucket);
        }
    }

    /// `_parseSnippet` of a value inside a group: anything but an object adds nothing, `null` throws (`None`).
    fn parse_template(&self, name: &str, template: &Value, extension: Option<(&str, &Path)>, bucket: &mut Vec<Snippet>) -> Option<()> {
        match template {
            Value::Object(snippet) => self.parse_snippet(name, snippet, extension, bucket),
            Value::Null => None,
            _ => Some(()),
        }
    }

    /// `_parseSnippet`: one `Snippet` per prefix; `None` where upstream throws (a prefix that is not a string).
    fn parse_snippet(&self, name: &str, snippet: &Map<String, Value>, extension: Option<(&str, &Path)>, bucket: &mut Vec<Snippet>) -> Option<()> {
        let prefixes = match snippet.get("prefix") {
            prefix if !is_truthy(&prefix) => vec![String::new()],
            Some(Value::String(prefix)) => vec![prefix.clone()],
            Some(Value::Array(prefixes)) => prefixes.iter().map(|prefix| prefix.as_str().map(String::from)).collect::<Option<_>>()?,
            _ => return None,
        };
        let body = match snippet.get("body") {
            Some(Value::Array(lines)) => join(lines, "\n"),
            Some(Value::String(body)) => body.clone(),
            _ => return Some(()),
        };
        let description = match snippet.get("description") {
            Some(Value::Array(lines)) => join(lines, "\n"),
            description if is_truthy(&description) => description.map(js_string).unwrap_or_default(),
            _ => String::new(),
        };
        let scopes = match (&self.default_scopes, snippet.get("scope")) {
            (Some(scopes), _) => scopes.clone(),
            (None, Some(Value::String(scope))) => scope.split(',').map(str::trim).filter(|scope| !scope.is_empty()).map(String::from).collect(),
            _ => Vec::new(),
        };
        let source = match extension {
            Some((label, _)) => label.to_owned(),
            None if self.source == SnippetSource::Workspace => "Workspace Snippet".to_owned(),
            None if self.is_global_snippets => "Global User Snippet".to_owned(),
            None => "User Snippet".to_owned(),
        };
        let snippet_identifier = match extension {
            Some((_, root)) => format!("{}/{name}", self.location.strip_prefix(root).map_or("undefined".into(), |relative| relative.to_string_lossy())),
            None => format!("{}/{name}", self.location.file_name().unwrap_or_default().to_string_lossy()),
        };
        for prefix in prefixes {
            bucket.push(Snippet {
                is_file_template: is_truthy(&snippet.get("isFileTemplate")),
                scopes: scopes.clone(),
                name: name.to_owned(),
                prefix,
                description: description.clone(),
                body: body.clone(),
                source: source.clone(),
                snippet_source: self.source,
                snippet_identifier: snippet_identifier.clone(),
                include: globs(snippet.get("include")),
                exclude: globs(snippet.get("exclude")),
            });
        }
        Some(())
    }
}

/// JavaScript truthiness of a JSON value; a missing key is `undefined`.
fn is_truthy(value: &Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|number| number != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(_) => true,
    }
}

/// `Object.entries` of a non-snippet value: an object's members, an array's or a string's items by index, nothing
/// for numbers and booleans, a throw (`None`) for `null`.
fn js_entries(value: &Value) -> Option<Vec<(String, &Value)>> {
    match value {
        Value::Object(members) => Some(members.iter().map(|(name, value)| (name.clone(), value)).collect()),
        Value::Array(items) => Some(items.iter().enumerate().map(|(index, value)| (index.to_string(), value)).collect()),
        Value::Null => None,
        _ => Some(Vec::new()),
    }
}

/// `Array.prototype.join`: `null` items are empty.
fn join(items: &[Value], separator: &str) -> String {
    items.iter().map(|item| if item.is_null() { String::new() } else { js_string(item) }).collect::<Vec<_>>().join(separator)
}

fn js_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => join(items, ","),
        Value::Object(_) => "[object Object]".to_owned(),
        other => other.to_string(),
    }
}

/// `include`/`exclude`: a string or an array of them.
fn globs(value: Option<&Value>) -> Option<Vec<String>> {
    match value? {
        Value::String(pattern) if !pattern.is_empty() => Some(vec![pattern.clone()]),
        Value::Array(patterns) => Some(patterns.iter().filter_map(Value::as_str).filter(|pattern| !pattern.is_empty()).map(String::from).collect()),
        _ => None,
    }
}

/// Where a snippets file is and the scopes its snippets get regardless of their `scope`.
#[derive(Debug, PartialEq)]
pub struct SnippetFileLocation {
    pub location: PathBuf,
    pub default_scopes: Option<Vec<String>>,
}

/// `_addSnippetFile`: a user `<lang>.json` file is scoped to `<lang>`, a `.code-snippets` file is not, anything
/// else is no snippets file.
pub fn folder_snippet_file(path: &Path, source: SnippetSource) -> Option<SnippetFileLocation> {
    let extension = path.extension()?;
    if source == SnippetSource::User && extension == "json" {
        let language = path.file_name()?.to_string_lossy().replacen(".json", "", 1);
        return Some(SnippetFileLocation { location: path.to_path_buf(), default_scopes: Some(vec![language]) });
    }
    (extension == "code-snippets").then(|| SnippetFileLocation { location: path.to_path_buf(), default_scopes: None })
}

/// `_initExtensionSnippets` for one extension's `contributes.snippets` entries: each valid one (`toValidSnippet`)
/// adds a file to `files` or a language to the file already there. Returns the collector's errors.
pub fn add_extension_snippet_files(files: &mut Vec<SnippetFileLocation>, extension_name: &str, extension_root: &Path, contributions: &[Value], is_registered_language: impl Fn(&str) -> bool) -> Vec<String> {
    let mut errors = Vec::new();
    for contribution in contributions {
        let (language, location) = match to_valid_snippet(extension_name, extension_root, contribution, &is_registered_language) {
            Ok(valid) => valid,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        let Some(file) = files.iter_mut().find(|file| file.location == location) else {
            files.push(SnippetFileLocation { location, default_scopes: language.map(|language| vec![language]) });
            continue;
        };
        match &mut file.default_scopes {
            // Upstream pushes `undefined` for a missing language; "" matches no language either.
            Some(scopes) => scopes.push(language.unwrap_or_default()),
            None => file.default_scopes = Some(Vec::new()),
        }
    }
    errors
}

/// `snippetExt.toValidSnippet`: the contributed language (when a non-empty string) and the file's location.
fn to_valid_snippet(extension_name: &str, extension_root: &Path, snippet: &Value, is_registered_language: &impl Fn(&str) -> bool) -> Result<(Option<String>, PathBuf), String> {
    let shown = |value: Option<&Value>| value.map_or("undefined".to_owned(), |value| value.as_str().map_or_else(|| value.to_string(), String::from));
    let path = snippet.get("path").and_then(Value::as_str).filter(|path| !path.trim().is_empty());
    let Some(path) = path else {
        return Err(format!("Expected string in `contributes.{extension_name}.path`. Provided value: {}", shown(snippet.get("path"))));
    };
    let language = snippet.get("language").and_then(Value::as_str).filter(|language| !language.trim().is_empty());
    if language.is_none() && !path.ends_with(".code-snippets") {
        return Err(format!("When omitting the language, the value of `contributes.{extension_name}.path` must be a `.code-snippets`-file. Provided value: {path}"));
    }
    if let Some(language) = language
        && !is_registered_language(language)
    {
        return Err(format!("Unknown language in `contributes.{extension_name}.language`. Provided value: {language}"));
    }
    let mut location = extension_root.to_path_buf();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                location.pop();
            }
            segment => location.push(segment),
        }
    }
    if !location.starts_with(extension_root) {
        return Err(format!(
            "Expected `contributes.{extension_name}.path` ({}) to be included inside extension's folder ({}). This might make the extension non-portable.",
            location.display(),
            extension_root.display()
        ));
    }
    let language = snippet.get("language").and_then(Value::as_str).filter(|language| !language.is_empty()).map(String::from);
    Ok((language, location))
}

/// `ISnippetGetOptions`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SnippetGetOptions {
    pub include_disabled_snippets: bool,
    pub include_no_prefix_snippets: bool,
    pub no_recency_sort: bool,
    pub file_template_snippets: Option<bool>,
}

/// `_filterAndSortSnippets`: `ignored` holds the disabled snippets' identifiers, `usage_timestamps` when each was
/// last inserted; the most recently used come first, then `_compareSnippet`.
pub fn filter_and_sort_snippets<'a>(snippets: Vec<&'a Snippet>, path: Option<&str>, options: SnippetGetOptions, ignored: &HashSet<String>, usage_timestamps: &HashMap<String, i64>) -> Vec<&'a Snippet> {
    let mut result: Vec<&Snippet> = snippets
        .into_iter()
        .filter(|snippet| !snippet.prefix.is_empty() || options.include_no_prefix_snippets)
        .filter(|snippet| !ignored.contains(&snippet.snippet_identifier) || options.include_disabled_snippets)
        .filter(|snippet| options.file_template_snippets.is_none_or(|file_template| file_template == snippet.is_file_template))
        .filter(|snippet| path.is_none_or(|path| snippet.is_file_included(path)))
        .collect();
    let used_at = |snippet: &Snippet| usage_timestamps.get(&snippet.snippet_identifier).copied().unwrap_or(-1);
    result.sort_by(|a, b| {
        let recency = if options.no_recency_sort { std::cmp::Ordering::Equal } else { used_at(b).cmp(&used_at(a)) };
        recency.then_with(|| (a.snippet_source, &a.source, &a.name).cmp(&(b.snippet_source, &b.source, &b.name)))
    });
    result
}

/// `SnippetCompletionProvider.provideCompletionItems` as an extension host completion list: `line` is the 0-based
/// line, `column` the cursor's char column in `line_text`; `trigger_character` is set for a trigger-character
/// request; `snippets` are the selected and filtered ones, each item's `x` is its index in them.
pub fn snippet_completions(line_text: &str, line: usize, column: usize, trigger_character: Option<&str>, auto_closing_pairs: &[(String, String)], snippets: &[&Snippet]) -> Value {
    let line_low = lower_chars(line_text);
    let word = word_at(line_text, column).unwrap_or(column..column);
    let anchors = snippet_positions(line_text, word.clone(), &line_low[..word.end]);
    let trigger_low = trigger_character.map(lower_chars).unwrap_or_default();

    let mut remaining: Vec<usize> = Vec::new();
    for (index, snippet) in snippets.iter().enumerate() {
        if !remaining.iter().any(|&seen| std::ptr::eq(snippets[seen], *snippet)) {
            remaining.push(index);
        }
    }
    let mut suggestions = Vec::new();
    remaining.retain(|&index| {
        let snippet = snippets[index];
        let prefix_low = lower_chars(&snippet.prefix);
        if trigger_character.is_some() && !prefix_low.starts_with(&trigger_low) {
            return true;
        }
        let candidate = anchors.iter().find(|(_, anchor_low)| {
            let whitespace_only_for_whitespace = !starts_with_whitespace(anchor_low) || starts_with_whitespace(&prefix_low);
            whitespace_only_for_whitespace && is_pattern_in_word(anchor_low, 0, anchor_low.len(), &prefix_low, 0, prefix_low.len(), &mut vec![0; anchor_low.len()])
        });
        let Some(&(start, _)) = candidate else { return true };

        let prefix_rest_len = prefix_low.len() - (column - start);
        // `compareSubstring` reads past the line's end as NaN, which never differs.
        let ends_with_prefix_rest = (0..prefix_rest_len).all(|i| line_low.get(column + i).is_none_or(|c| *c == prefix_low[column - start + i]));
        let mut end = if ends_with_prefix_rest { column + prefix_rest_len } else { column };
        if let Some(&next) = line_low.get(column) {
            let eats_auto_closed = auto_closing_pairs.iter().any(|(open, close)| {
                close.chars().eq([next]) && open.chars().eq([line_low[start]]) && snippet.prefix.starts_with(open.as_str()) && snippet.prefix.chars().last() == close.chars().next()
            });
            if eats_auto_closed {
                end += 1;
            }
        }
        suggestions.push((index, start..column, start..end));
        false
    });

    let ends_in_whitespace = column > 0 && line_low.get(column - 1).is_some_and(|c| c.is_whitespace());
    if trigger_low.is_empty() && (ends_in_whitespace || line_low.is_empty()) {
        for index in remaining {
            let prefix_low = lower_chars(&snippets[index].prefix);
            let replace_end = if line_low[column..].starts_with(&prefix_low) { column + prefix_low.len() } else { column };
            suggestions.push((index, column..column, column..replace_end));
        }
    }

    // `_disambiguateSnippets` compares label objects by identity, so upstream never relabels; only its sort remains.
    suggestions.sort_by(|a, b| snippets[a.0].prefix.cmp(&snippets[b.0].prefix));
    let range = |chars: std::ops::Range<usize>| {
        json!({ "startLineNumber": line + 1, "startColumn": utf16_column(line_text, chars.start) + 1, "endLineNumber": line + 1, "endColumn": utf16_column(line_text, chars.end) + 1 })
    };
    let items: Vec<Value> = suggestions
        .into_iter()
        .map(|(index, insert, replace)| {
            let mut item = snippet_completion(snippets[index], json!({ "insert": range(insert), "replace": range(replace) }));
            item["x"] = json!(index);
            item
        })
        .collect();
    json!({ "b": items })
}

/// `SnippetCompletion` as the extension host's completion item DTO.
pub fn snippet_completion(snippet: &Snippet, range: Value) -> Value {
    json!({
        "a": { "label": snippet.prefix, "description": snippet.name },
        "b": KIND_SNIPPET,
        "c": format!("{} ({})", if snippet.description.is_empty() { &snippet.name } else { &snippet.description }, snippet.source),
        "e": format!("{}-{}", if snippet.snippet_source == SnippetSource::Extension { "z" } else { "a" }, snippet.prefix),
        "h": snippet.body,
        "i": INSERT_AS_SNIPPET,
        "j": range,
        "o": MARK_SNIPPET_AS_USED,
        "p": [snippet.snippet_identifier],
    })
}

/// `SnippetCompletion.resolve`: the documentation is a code block of the text the snippet inserts.
pub fn resolve_snippet_completion(snippet: &Snippet) -> Value {
    let indentation = Indentation { tab_size: 4, indent_size: None, insert_spaces: true };
    let text = expand_snippet(&snippet.body, "", false, indentation, |_| None).text;
    json!({ "d": { "value": format!("\n{}\n", escaped_code_block_fence(&text, "")) } })
}

/// `appendEscapedMarkdownCodeBlockFence`: a fence longer than any backtick run starting a line of `code`.
fn escaped_code_block_fence(code: &str, language: &str) -> String {
    let longest = code
        .split(['\n', '\r'])
        .filter_map(|line| {
            let indent = line.len() - line.trim_start_matches(' ').len();
            (indent <= 3).then(|| line[indent..].chars().take_while(|&c| c == '`').count())
        })
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(if longest >= 3 { longest + 1 } else { 3 });
    format!("{fence}{language}\n{code}\n{fence}")
}

/// `_computeSnippetPositions`: every word start before the cursor's word, the char after each word, every other
/// char, then the cursor's word; each with the lowercased line from there to the end of the cursor's word.
fn snippet_positions<'a>(line_text: &str, word: std::ops::Range<usize>, line_with_word_low: &'a [char]) -> Vec<(usize, &'a [char])> {
    let tail = |start: usize| &line_with_word_low[start.min(line_with_word_low.len())..];
    let mut result = Vec::new();
    let mut column = 0;
    while column < word.start {
        result.push((column, tail(column)));
        match word_at(line_text, column) {
            Some(found) => {
                result.push((found.end, tail(found.end)));
                column = found.end + 1;
            }
            None => column += 1,
        }
    }
    if !word.is_empty() || result.is_empty() {
        result.push((word.start, tail(word.start)));
    }
    result
}

/// `toLowerCase` per char, one char each so indices stay those of the line.
fn lower_chars(text: &str) -> Vec<char> {
    text.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect()
}

fn starts_with_whitespace(text: &[char]) -> bool {
    text.first().is_some_and(|c| c.is_whitespace())
}

#[cfg(test)]
mod tests;
