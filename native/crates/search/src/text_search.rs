//! Text search: `TextSearchManager` (search/common/textSearchManager.ts) over `RipgrepTextSearchEngine`
//! (search/node/ripgrepTextSearchEngine.ts), with the result shapes of search/common/search.ts.

use std::collections::HashSet;
use std::path::Path;

use files::glob::{self, ExcludeExpression, Glob, Options};
use serde_json::{Value, json};

use crate::query::{DEFAULT_PREVIEW_OPTIONS, Expression, FolderQuery, PatternInfo, PreviewOptions, Query};
use crate::ripgrep::{self, Cancellation, anchor_glob, perform_brace_expansion_for_ripgrep};

/// `DEFAULT_MAX_SEARCH_RESULTS`.
const DEFAULT_MAX_SEARCH_RESULTS: usize = 20000;

const SEARCH_ELIDED_PREFIX: &str = "⟪ ";
const SEARCH_ELIDED_SUFFIX: &str = " characters skipped ⟫";

/// One ripgrep result before it is grouped by file: a match with its ranges, or a line of context.
#[derive(Clone, Debug, PartialEq)]
pub enum TextSearchResult {
    Match { relative_path: String, preview_text: String, ranges: Vec<RangeLocation> },
    Context { relative_path: String, text: String, line_number: usize },
}

/// `SearchRangeSetPairing`: where a match is in the file and in its preview text, 0-based lines, UTF-16 columns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangeLocation {
    pub source: SearchRange,
    pub preview: SearchRange,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SearchRange {
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

/// `TextSearchManager.search`: reports each file's results as an `IFileMatch` (`resource` is left to the caller, which
/// gets the file's absolute path). Returns whether the result limit was hit.
pub fn search(query: &Query, rg: &Path, cancellation: &Cancellation, on_file_match: &mut dyn FnMut(&Path, Vec<Value>)) -> Result<bool, String> {
    let pattern = query.content_pattern.clone().unwrap_or_default();
    let max_results = query.max_results.unwrap_or(DEFAULT_MAX_SEARCH_RESULTS);
    let mut result_count = 0;
    let mut is_limit_hit = false;
    let mut limit_hit = false;
    let mut collector = Collector::default();
    for folder_query in &query.folder_queries {
        if cancellation.is_cancelled() || is_limit_hit {
            break;
        }
        let tester = QueryGlobTester::new(query, folder_query);
        let options = FolderOptions::new(query, folder_query);
        let folder = &folder_query.folder;
        let mut on_result = |result: TextSearchResult| {
            if is_limit_hit {
                return false;
            }
            let relative_path = match &result {
                TextSearchResult::Match { relative_path, .. } | TextSearchResult::Context { relative_path, .. } => relative_path.clone(),
            };
            if !tester.included_in_query(folder, &relative_path) {
                return true;
            }
            let mut result = result;
            if let TextSearchResult::Match { ranges, .. } = &mut result {
                if result_count + ranges.len() > max_results {
                    is_limit_hit = true;
                    ranges.truncate(max_results - result_count);
                }
                result_count += ranges.len();
                if ranges.is_empty() {
                    return false;
                }
            }
            collector.add(folder, result, on_file_match);
            !is_limit_hit
        };
        limit_hit |= search_folder(&pattern, &options, query, rg, cancellation, &mut on_result)?;
    }
    collector.flush(on_file_match);
    Ok(is_limit_hit || limit_hit)
}

/// `provideTextSearchResultsWithRgOptions`: ripgrep does not descend into excluded folders, so a second run searches
/// what `!` exceptions re-include and skips the files the first run reported.
fn search_folder(
    pattern: &PatternInfo,
    options: &FolderOptions,
    query: &Query,
    rg: &Path,
    cancellation: &Cancellation,
    on_result: &mut dyn FnMut(TextSearchResult) -> bool,
) -> Result<bool, String> {
    if options.exclude_exceptions.is_empty() {
        return search_with_ripgrep(pattern, options, query, rg, cancellation, None, on_result);
    }
    let mut reported_paths = HashSet::new();
    let limit_hit = search_with_ripgrep(pattern, options, query, rg, cancellation, None, &mut |result| {
        reported_paths.insert(result_path(&result).to_string());
        on_result(result)
    })?;
    if limit_hit || cancellation.is_cancelled() {
        return Ok(limit_hit);
    }
    let exceptions: Expression = options.exclude_exceptions.iter().map(|exception| (format!("!{exception}"), Value::Bool(true))).collect();
    let exceptions = glob::parse_exclude_expression(&exceptions, Options { ignore_case: options.ignore_glob_case, ..Options::default() });
    let is_reincluded = |path: &str| {
        exceptions.is_excepted(path, None) && !options.nested_folders.iter().any(|folder| path == folder || path.starts_with(&format!("{folder}/")))
    };
    let unlimited = Query { max_results: Some(usize::MAX), ..query.clone() };
    search_with_ripgrep(pattern, options, &unlimited, rg, cancellation, Some(&options.exclude_exceptions), &mut |result| {
        let path = result_path(&result);
        if reported_paths.contains(path) || !is_reincluded(path) {
            return true;
        }
        on_result(result)
    })
}

fn result_path(result: &TextSearchResult) -> &str {
    match result {
        TextSearchResult::Match { relative_path, .. } | TextSearchResult::Context { relative_path, .. } => relative_path,
    }
}

// shortcut: no second run without --follow when ripgrep reports symbolic link loops (RipgrepSymlinkLoopDetector);
// port it once a workspace with looping links needs it.
fn search_with_ripgrep(
    pattern: &PatternInfo,
    options: &FolderOptions,
    query: &Query,
    rg: &Path,
    cancellation: &Cancellation,
    exclude_exceptions: Option<&[String]>,
    on_result: &mut dyn FnMut(TextSearchResult) -> bool,
) -> Result<bool, String> {
    if pattern.pattern.is_empty() {
        return Ok(false);
    }
    let args = rg_args(pattern, options, query.max_file_size, query.surrounding_context, exclude_exceptions);
    let preview = query.preview_options.unwrap_or(DEFAULT_PREVIEW_OPTIONS);
    let mut parser = RipgrepParser::new(query.max_results.unwrap_or(DEFAULT_MAX_SEARCH_RESULTS), preview);
    let mut stopped = false;
    let finished = ripgrep::run(rg, &args, &options.folder, cancellation, &mut |line| {
        let results = parser.handle_line(line);
        for result in results {
            if !on_result(result) {
                stopped = true;
                return false;
            }
        }
        !parser.hit_limit
    })
    .map_err(|error| error.to_string())?;
    if !stopped && !finished.got_data && !finished.stderr.is_empty() && let Some(message) = rg_error_message_for_display(&finished.stderr) {
        return Err(message);
    }
    Ok(parser.hit_limit)
}

/// `TextSearchProviderFolderOptionsRipgrep` from `getSearchOptionsForFolder`.
struct FolderOptions {
    folder: std::path::PathBuf,
    includes: Vec<String>,
    excludes: Vec<String>,
    use_ignore_files_local: bool,
    use_ignore_files_parent: bool,
    use_ignore_files_global: bool,
    follow_symlinks: bool,
    encoding: String,
    ignore_glob_case: bool,
    exclude_exceptions: Vec<String>,
    /// Folders of the query inside this one; their files are searched on their own.
    nested_folders: Vec<String>,
}

impl FolderOptions {
    fn new(query: &Query, folder_query: &FolderQuery) -> Self {
        let merged = |global: &Option<Expression>, folder: &Option<Expression>| {
            let mut merged = global.clone().unwrap_or_default();
            merged.extend(folder.clone().unwrap_or_default());
            merged
        };
        let enabled = |expression: &Expression| expression.iter().filter(|(_, value)| **value == true).map(|(key, _)| key.clone()).collect::<Vec<_>>();
        let excludes = merged(&query.exclude_pattern, &folder_query.exclude_pattern);
        let exclude_exceptions = glob::get_exclude_exceptions(&excludes);
        let folder = &folder_query.folder;
        let nested_folders = match exclude_exceptions.is_empty() {
            true => Vec::new(),
            false => query
                .folder_queries
                .iter()
                .filter(|other| &other.folder != folder && other.folder.starts_with(folder))
                .map(|other| other.folder.strip_prefix(folder).unwrap_or(&other.folder).to_string_lossy().into_owned())
                .collect(),
        };
        FolderOptions {
            folder: folder.clone(),
            includes: enabled(&merged(&query.include_pattern, &folder_query.include_pattern)),
            excludes: enabled(&excludes).into_iter().filter(|pattern| !glob::is_exclude_exception(pattern)).collect(),
            use_ignore_files_local: !folder_query.disregard_ignore_files,
            use_ignore_files_parent: !folder_query.disregard_parent_ignore_files,
            use_ignore_files_global: !folder_query.disregard_global_ignore_files,
            follow_symlinks: !folder_query.ignore_symlinks,
            encoding: folder_query.file_encoding.clone().unwrap_or_default(),
            ignore_glob_case: query.ignore_glob_case,
            exclude_exceptions,
            nested_folders,
        }
    }
}

/// `getRgArgs` (ripgrepTextSearchEngine.ts).
fn rg_args(pattern: &PatternInfo, options: &FolderOptions, max_file_size: Option<u64>, surrounding_context: usize, exclude_exceptions: Option<&[String]>) -> Vec<String> {
    let mut args: Vec<String> = vec!["--hidden".into(), "--no-require-git".into()];
    args.push(if pattern.is_case_sensitive { "--case-sensitive" } else { "--ignore-case" }.into());
    if options.ignore_glob_case {
        args.extend(["--glob-case-insensitive".into(), "--ignore-file-case-insensitive".into()]);
    }
    let push_glob = |args: &mut Vec<String>, glob: String| args.extend(["-g".to_string(), glob]);
    match exclude_exceptions {
        Some(exceptions) => ripgrep::exclude_exception_globs(exceptions).into_iter().for_each(|glob| push_glob(&mut args, glob)),
        None => {
            let (double_star, others): (Vec<&String>, Vec<&String>) = options.includes.iter().partition(|include| include.starts_with("**"));
            if !others.is_empty() {
                push_glob(&mut args, "!*".into());
                let mut unique = Vec::new();
                for other in others {
                    if !unique.contains(&other) {
                        unique.push(other);
                    }
                }
                for other in unique {
                    for component in spread_glob_components(other) {
                        push_glob(&mut args, anchor_glob(&component));
                    }
                }
            }
            for include in double_star {
                push_glob(&mut args, include.clone());
            }
            for exclude in &options.excludes {
                push_glob(&mut args, format!("!{}", anchor_glob(exclude)));
            }
        }
    }
    if let Some(max_file_size) = max_file_size {
        args.extend(["--max-filesize".into(), max_file_size.to_string()]);
    }
    if options.use_ignore_files_local && exclude_exceptions.is_none() {
        if !options.use_ignore_files_parent {
            args.push("--no-ignore-parent".into());
        }
    } else {
        args.push("--no-ignore".into());
    }
    if options.follow_symlinks {
        args.push("--follow".into());
    }
    if !options.encoding.is_empty() && options.encoding != "utf8" {
        args.extend(["--encoding".into(), options.encoding.clone()]);
    }
    let mut text = pattern.pattern.clone();
    let mut is_reg_exp = pattern.is_reg_exp;
    if text == "--" {
        is_reg_exp = true;
        text = "\\-\\-".into();
    }
    if pattern.is_multiline && !is_reg_exp {
        text = escape_reg_exp_characters(&text);
        is_reg_exp = true;
    }
    args.push("--crlf".into());
    if is_reg_exp {
        text = unicode_escapes_to_pcre2(&text);
        args.extend(["--engine".into(), "auto".into()]);
    }
    let mut after_double_dashes = None;
    if pattern.is_word_match {
        args.extend(["--regexp".into(), whole_word_regexp(&text, is_reg_exp)]);
    } else if is_reg_exp {
        args.extend(["--regexp".into(), fix_newline(&fix_regex_newline(&text))]);
    } else {
        after_double_dashes = Some(text);
        args.push("--fixed-strings".into());
    }
    args.push("--no-config".into());
    if !options.use_ignore_files_global {
        args.push("--no-ignore-global".into());
    }
    args.push("--json".into());
    if pattern.is_multiline {
        args.push("--multiline".into());
    }
    if surrounding_context > 0 {
        args.extend(["--before-context".into(), surrounding_context.to_string(), "--after-context".into(), surrounding_context.to_string()]);
    }
    args.push("--".into());
    // The query goes after `--` in case it starts with a dash.
    if let Some(text) = after_double_dashes {
        args.push(text);
    }
    args.push(".".into());
    args
}

/// `spreadGlobComponents`: `foo/*bar/x` gives `foo`, `foo/*bar`, `foo/*bar/x`, so ripgrep walks the folders on the way.
fn spread_glob_components(glob: &str) -> Vec<String> {
    perform_brace_expansion_for_ripgrep(glob)
        .iter()
        .flat_map(|glob| {
            let components = glob::split_glob_aware(glob, '/');
            (1..=components.len()).map(move |end| components[..end].join("/")).collect::<Vec<_>>()
        })
        .collect()
}

/// `strings.escapeRegExpCharacters`.
fn escape_reg_exp_characters(value: &str) -> String {
    let mut escaped = String::new();
    for char in value.chars() {
        if "\\{}*+?|^$.[]()".contains(char) {
            escaped.push('\\');
        }
        escaped.push(char);
    }
    escaped
}

/// `createRegExp(..., { wholeWord: true }).source`: `\b` added on each side that starts or ends with a word character.
fn whole_word_regexp(pattern: &str, is_reg_exp: bool) -> String {
    let mut source = if is_reg_exp { pattern.to_string() } else { escape_reg_exp_characters(pattern) };
    let is_word = |char: Option<char>| char.is_some_and(|char| char.is_ascii_alphanumeric() || char == '_');
    if is_word(source.chars().next()) {
        source.insert_str(0, "\\b");
    }
    if is_word(source.chars().last()) {
        source.push_str("\\b");
    }
    source
}

/// `unicodeEscapesToPCRE2`: `\u1234` and `\u{1234}` become `\x{1234}`, unless the backslash is escaped.
pub fn unicode_escapes_to_pcre2(pattern: &str) -> String {
    let plain = regex::Regex::new(r"(?i)((?:[^\\]|^)(?:\\\\)*)\\u([a-z0-9]{4})").expect("valid regex");
    let braces = regex::Regex::new(r"(?i)((?:[^\\]|^)(?:\\\\)*)\\u\{([a-z0-9]{4})\}").expect("valid regex");
    let mut pattern = pattern.to_string();
    for regex in [plain, braces] {
        while regex.is_match(&pattern) {
            pattern = regex.replace_all(&pattern, r"${1}\x{${2}}").into_owned();
        }
    }
    pattern
}

/// `fixNewline`: a literal newline matches `\r\n` too.
pub fn fix_newline(pattern: &str) -> String {
    pattern.replace('\n', "\\r?\\n")
}

/// `fixRegexNewline`: `\n` in a regex matches `\r\n` too. Inside a character class the class is rewritten, inside a
/// lookbehind nothing changes (#100569), and a pattern that does not parse is left alone.
pub fn fix_regex_newline(pattern: &str) -> String {
    let Some(parsed) = parse_regex(pattern) else { return pattern.to_string() };
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for newline in &parsed.newlines {
        if newline.in_lookbehind {
            continue;
        }
        match &newline.class {
            None => {
                let text = if newline.quantified { "(?:\\r?\\n)" } else { "\\r?\\n" };
                edits.push((newline.start, newline.start + 2, text.to_string()));
            }
            Some(class) if class.in_range => {}
            Some(class) => {
                if edits.iter().any(|(start, ..)| *start == class.start) {
                    continue;
                }
                let content_start = class.start + if class.negate { 2 } else { 1 };
                let other = format!("{}{}", &pattern[content_start..newline.start], &pattern[newline.start + 2..class.end - 1]);
                let text = match (class.negate, class.quantified) {
                    (true, true) if other.is_empty() => ".".to_string(),
                    (true, true) => format!("[^{other}]"),
                    (true, false) if other.is_empty() => "(?!\\r?\\n)".to_string(),
                    (true, false) => format!("(?!\\r?\\n|[{other}])"),
                    (false, _) if other.is_empty() => "\\r?\\n".to_string(),
                    (false, _) => format!("(?:[{other}]|\\r?\\n)"),
                };
                edits.push((class.start, class.end, text));
            }
        }
    }
    let mut output = String::new();
    let mut last = 0;
    for (start, end, text) in edits {
        output.push_str(&pattern[last..start]);
        output.push_str(&text);
        last = end;
    }
    output.push_str(&pattern[last..]);
    output
}

struct ParsedRegex {
    newlines: Vec<Newline>,
}

struct Newline {
    /// Byte offset of the `\n` escape.
    start: usize,
    in_lookbehind: bool,
    /// The `\n` or a group around it is quantified.
    quantified: bool,
    class: Option<NewlineClass>,
}

struct NewlineClass {
    start: usize,
    end: usize,
    negate: bool,
    quantified: bool,
    /// The `\n` is an end of a range such as `[\n-a]`.
    in_range: bool,
}

/// Finds each `\n` escape with the context `fixRegexNewline` needs; `None` when the brackets do not balance.
fn parse_regex(pattern: &str) -> Option<ParsedRegex> {
    struct Group {
        lookbehind: bool,
        newlines: Vec<usize>,
    }
    let bytes = pattern.as_bytes();
    let mut groups: Vec<Group> = Vec::new();
    let mut newlines: Vec<Newline> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                let next = *bytes.get(index + 1)?;
                if next == b'n' {
                    newlines.push(Newline {
                        start: index,
                        in_lookbehind: groups.iter().any(|group| group.lookbehind),
                        quantified: is_quantifier_at(bytes, index + 2),
                        class: None,
                    });
                    for group in &mut groups {
                        group.newlines.push(newlines.len() - 1);
                    }
                }
                index += 1 + pattern[index + 1..].chars().next()?.len_utf8();
            }
            b'[' => {
                let start = index;
                let negate = bytes.get(index + 1) == Some(&b'^');
                index += if negate { 2 } else { 1 };
                let mut atoms: Vec<(usize, bool)> = Vec::new();
                while *bytes.get(index)? != b']' {
                    let is_newline = bytes[index] == b'\\' && bytes.get(index + 1) == Some(&b'n');
                    atoms.push((index, is_newline));
                    index += if bytes[index] == b'\\' { 1 + pattern[index + 1..].chars().next()?.len_utf8() } else { pattern[index..].chars().next()?.len_utf8() };
                }
                let end = index + 1;
                let quantified = is_quantifier_at(bytes, end);
                for (position, &(atom_start, is_newline)) in atoms.iter().enumerate() {
                    if !is_newline {
                        continue;
                    }
                    let is_dash = |at: Option<&(usize, bool)>| at.is_some_and(|(start, _)| bytes[*start] == b'-');
                    let in_range = (is_dash(atoms.get(position + 1)) && position + 2 < atoms.len()) || (position >= 2 && is_dash(atoms.get(position - 1)));
                    newlines.push(Newline {
                        start: atom_start,
                        in_lookbehind: groups.iter().any(|group| group.lookbehind),
                        quantified: false,
                        class: Some(NewlineClass { start, end, negate, quantified, in_range }),
                    });
                }
                index = end;
            }
            b'(' => {
                groups.push(Group { lookbehind: pattern[index..].starts_with("(?<=") || pattern[index..].starts_with("(?<!"), newlines: Vec::new() });
                index += 1;
            }
            b')' => {
                let group = groups.pop()?;
                if is_quantifier_at(bytes, index + 1) {
                    for newline in group.newlines {
                        if newlines[newline].class.is_none() {
                            newlines[newline].quantified = true;
                        }
                    }
                }
                index += 1;
            }
            _ => index += pattern[index..].chars().next()?.len_utf8(),
        }
    }
    groups.is_empty().then_some(ParsedRegex { newlines })
}

/// `*`, `+`, `?` or `{n}`, `{n,}`, `{n,m}` at `index`.
fn is_quantifier_at(bytes: &[u8], index: usize) -> bool {
    match bytes.get(index) {
        Some(b'*' | b'+' | b'?') => true,
        Some(b'{') => {
            let rest = &bytes[index + 1..];
            let Some(close) = rest.iter().position(|byte| *byte == b'}') else { return false };
            let inner = &rest[..close];
            let mut parts = inner.splitn(2, |byte| *byte == b',');
            let min = parts.next().unwrap_or_default();
            !min.is_empty() && min.iter().all(u8::is_ascii_digit) && parts.next().is_none_or(|max| max.iter().all(u8::is_ascii_digit))
        }
        _ => false,
    }
}

/// `rgErrorMsgForDisplay` (ripgrepTextSearchEngine.ts): stderr that means the search failed, not a warning.
fn rg_error_message_for_display(message: &str) -> Option<String> {
    let lines: Vec<&str> = message.split('\n').collect();
    let first = lines[0].trim();
    let upper_first = |text: &str| {
        let mut chars = text.chars();
        chars.next().map(|first| first.to_uppercase().chain(chars).collect::<String>()).unwrap_or_default()
    };
    if lines.iter().any(|line| line.starts_with("regex parse error")) {
        let mut error = "Regex parse error".to_string();
        if let Some(pcre2) = lines.iter().find(|line| line.starts_with("PCRE2:")) {
            let message = pcre2.replacen("PCRE2:", "", 1);
            if message.contains(':') {
                error.push(':');
                error.push_str(message.split(':').nth(1).unwrap_or_default());
            }
        }
        return Some(error);
    }
    if let Some(encoding) = first.find("grep config error: unknown encoding: ") {
        return Some(format!("Unknown encoding: {}", &first[encoding + "grep config error: unknown encoding: ".len()..]));
    }
    if first.starts_with("error parsing glob") || first.starts_with("the literal") {
        return Some(upper_first(first));
    }
    first.starts_with("PCRE2: error compiling pattern").then(|| first.to_string())
}

/// `RipgrepParser`: turns `rg --json` lines into results, stopping after `max_results` ranges.
struct RipgrepParser {
    max_results: usize,
    preview: PreviewOptions,
    num_results: usize,
    hit_limit: bool,
}

impl RipgrepParser {
    fn new(max_results: usize, preview: PreviewOptions) -> Self {
        RipgrepParser { max_results, preview, num_results: 0, hit_limit: false }
    }

    fn handle_line(&mut self, line: &str) -> Vec<TextSearchResult> {
        let line = line.trim();
        if self.hit_limit || line.is_empty() {
            return Vec::new();
        }
        let Ok(message) = serde_json::from_str::<Value>(line) else { return Vec::new() };
        let data = &message["data"];
        // `URI.joinPath(rootFolder, path)` drops the `./` ripgrep puts before each path when it searches `.`.
        let path = bytes_or_text(&data["path"]);
        let relative_path = path.strip_prefix("./").unwrap_or(&path).to_string();
        match message["type"].as_str() {
            Some("match") => vec![self.create_text_search_match(data, relative_path)],
            Some("context") => {
                let text = bytes_or_text(&data["lines"]);
                let text = text.strip_suffix("\r\n").or_else(|| text.strip_suffix('\n')).unwrap_or(&text);
                let start_line = data["line_number"].as_u64().unwrap_or_default() as usize;
                text.split('\n')
                    .enumerate()
                    .map(|(offset, line)| TextSearchResult::Context { relative_path: relative_path.clone(), text: line.to_string(), line_number: start_line + offset })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    fn create_text_search_match(&mut self, data: &Value, relative_path: String) -> TextSearchResult {
        let line_number = (data["line_number"].as_u64().unwrap_or(1) as usize).saturating_sub(1);
        let full_text = bytes_or_text(&data["lines"]);
        let full_bytes = full_text.as_bytes();
        let mut submatches: Vec<(usize, usize, String)> = data["submatches"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|submatch| (submatch["start"].as_u64().unwrap_or_default() as usize, submatch["end"].as_u64().unwrap_or_default() as usize, bytes_or_text(&submatch["match"])))
            .collect();
        // Some regexes match a line without reporting submatches (#100569).
        if submatches.is_empty() {
            submatches.push(match full_text.chars().next() {
                Some(first) => (0, 1, first.to_string()),
                None => (0, 0, String::new()),
            });
        }
        let mut prev_match_end = 0;
        let mut prev_match_end_col = 0;
        let mut prev_match_end_line = line_number;
        let mut ranges = Vec::new();
        for (start, end, match_text) in submatches {
            if self.hit_limit {
                break;
            }
            self.num_results += 1;
            if self.num_results >= self.max_results {
                self.hit_limit = true;
            }
            let in_between = String::from_utf8_lossy(full_bytes.get(prev_match_end..start).unwrap_or_default());
            let (in_between_lines, in_between_last) = lines_and_last_line_length(&in_between);
            let start_col = if in_between_lines > 0 { in_between_last } else { in_between_last + prev_match_end_col };
            let (match_lines, match_last) = lines_and_last_line_length(&match_text);
            let start_line = in_between_lines + prev_match_end_line;
            let end_line = match_lines + start_line;
            let end_col = if match_lines > 0 { match_last } else { match_last + start_col };
            prev_match_end = end;
            prev_match_end_col = end_col;
            prev_match_end_line = end_line;
            ranges.push(SearchRange { start_line, start_column: start_col, end_line, end_column: end_col });
        }
        let (preview_text, ranges) = text_search_match(&full_text, &ranges, self.preview);
        TextSearchResult::Match { relative_path, preview_text, ranges }
    }
}

/// `bytesOrTextToString`: ripgrep sends text that is not UTF-8 as base64 `bytes`.
fn bytes_or_text(value: &Value) -> String {
    match value["bytes"].as_str() {
        Some(bytes) => String::from_utf8_lossy(&decode_base64(bytes)).into_owned(),
        None => value["text"].as_str().unwrap_or_default().to_string(),
    }
}

fn decode_base64(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bytes = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0;
    for char in text.bytes() {
        let Some(value) = ALPHABET.iter().position(|letter| *letter == char) else { continue };
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
        }
    }
    bytes
}

/// `getNumLinesAndLastNewlineLength`, the length in UTF-16 code units as JavaScript counts columns.
fn lines_and_last_line_length(text: &str) -> (usize, usize) {
    let lines = text.matches('\n').count();
    let last_line = text.rsplit('\n').next().unwrap_or(text);
    (lines, utf16_len(last_line))
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `new TextSearchMatch(text, ranges, previewOptions)`: a one-line preview is cut to `charsPerLine` around each match,
/// with long gaps elided; anything else keeps the full text with ranges relative to its first line.
fn text_search_match(text: &str, ranges: &[SearchRange], preview: PreviewOptions) -> (String, Vec<RangeLocation>) {
    let single_line = ranges.iter().all(|range| range.start_line == ranges[0].start_line && range.end_line == ranges[0].start_line);
    if preview.match_lines == 1 && single_line {
        let text: Vec<u16> = first_line(text).encode_utf16().collect();
        let slice = |start: usize, end: usize| String::from_utf16_lossy(&text[start.min(text.len())..end.clamp(start.min(text.len()), text.len())]);
        let elided_min_len = (utf16_len(SEARCH_ELIDED_PREFIX) + utf16_len(SEARCH_ELIDED_SUFFIX) + 5) * 2;
        let leading_chars = preview.chars_per_line / 5;
        let mut result = String::new();
        let mut shift: isize = 0;
        let mut last_end = 0;
        let mut locations = Vec::new();
        for range in ranges {
            let preview_start = range.start_column.saturating_sub(leading_chars);
            let preview_end = range.start_column + preview.chars_per_line;
            if preview_start > last_end + leading_chars + elided_min_len {
                let elision = format!("{SEARCH_ELIDED_PREFIX}{}{SEARCH_ELIDED_SUFFIX}", preview_start - last_end);
                result.push_str(&elision);
                result.push_str(&slice(preview_start, preview_end));
                shift += preview_start as isize - (last_end + utf16_len(&elision)) as isize;
            } else {
                result.push_str(&slice(last_end, preview_end));
            }
            last_end = preview_end;
            let column = |column: usize| (column as isize - shift) as usize;
            locations.push(RangeLocation {
                source: *range,
                preview: SearchRange { start_line: 0, start_column: column(range.start_column), end_line: 0, end_column: column(range.end_column) },
            });
        }
        return (result, locations);
    }
    let first = ranges[0].start_line;
    let locations = ranges
        .iter()
        .map(|range| RangeLocation {
            source: *range,
            preview: SearchRange { start_line: range.start_line - first, start_column: range.start_column, end_line: range.end_line - first, end_column: range.end_column },
        })
        .collect();
    (text.to_string(), locations)
}

/// `strings.getNLines(text, 1)`: the text up to its first line break.
fn first_line(text: &str) -> &str {
    match text.find('\n') {
        Some(index) => text[..index].strip_suffix('\r').unwrap_or(&text[..index]),
        None => text,
    }
}

/// `QueryGlobTester.includedInQuery`: ripgrep's globs can match more than VS Code's, so each result is checked again.
struct QueryGlobTester {
    excludes: ExcludeExpression,
    include: Option<Glob>,
    needs_siblings: bool,
}

impl QueryGlobTester {
    fn new(query: &Query, folder_query: &FolderQuery) -> Self {
        let options = Options { ignore_case: query.ignore_glob_case, ..Options::default() };
        let mut exclude = query.exclude_pattern.clone().unwrap_or_default();
        exclude.extend(folder_query.exclude_pattern.clone().unwrap_or_default());
        let include = match (&query.include_pattern, &folder_query.include_pattern) {
            (None, None) => None,
            (global, folder) => {
                let mut include = global.clone().unwrap_or_default();
                include.extend(folder.clone().unwrap_or_default());
                Some(glob::parse_expression(&include, options))
            }
        };
        let needs_siblings = exclude.values().any(|value| value.get("when").is_some());
        QueryGlobTester { excludes: glob::parse_exclude_expression(&exclude, options), include, needs_siblings }
    }

    fn included_in_query(&self, folder: &Path, relative_path: &str) -> bool {
        let basename = relative_path.rsplit('/').next();
        let siblings: Option<HashSet<String>> = self.needs_siblings.then(|| {
            let directory = folder.join(relative_path);
            let directory = directory.parent().unwrap_or(folder);
            std::fs::read_dir(directory).into_iter().flatten().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect()
        });
        let has_sibling = |name: &str| siblings.as_ref().is_some_and(|siblings| siblings.contains(name));
        let has_sibling = siblings.as_ref().map(|_| &has_sibling as &dyn Fn(&str) -> bool);
        if self.excludes.matches(relative_path, basename, has_sibling).is_some() {
            return false;
        }
        self.include.as_ref().is_none_or(|include| include.matches(relative_path, basename, has_sibling).is_some())
    }
}

/// `TextSearchResultsCollector`: consecutive results of one file become one `IFileMatch`.
#[derive(Default)]
struct Collector {
    current: Option<(std::path::PathBuf, Vec<Value>)>,
}

impl Collector {
    fn add(&mut self, folder: &Path, result: TextSearchResult, on_file_match: &mut dyn FnMut(&Path, Vec<Value>)) {
        let path = folder.join(result_path(&result));
        if self.current.as_ref().is_some_and(|(current, _)| *current != path) {
            self.flush(on_file_match);
        }
        let (_, results) = self.current.get_or_insert_with(|| (path, Vec::new()));
        results.push(frontend_result(&result));
    }

    fn flush(&mut self, on_file_match: &mut dyn FnMut(&Path, Vec<Value>)) {
        if let Some((path, results)) = self.current.take() {
            on_file_match(&path, results);
        }
    }
}

/// `extensionResultToFrontendResult`: `ITextSearchMatch` or `ITextSearchContext`.
fn frontend_result(result: &TextSearchResult) -> Value {
    let range = |range: &SearchRange| {
        json!({ "startLineNumber": range.start_line, "startColumn": range.start_column, "endLineNumber": range.end_line, "endColumn": range.end_column })
    };
    match result {
        TextSearchResult::Match { preview_text, ranges, .. } => json!({
            "previewText": preview_text,
            "rangeLocations": ranges.iter().map(|location| json!({ "preview": range(&location.preview), "source": range(&location.source) })).collect::<Vec<_>>(),
        }),
        TextSearchResult::Context { text, line_number, .. } => json!({ "text": text, "lineNumber": line_number }),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn unicode_escapes() {
        let cases = [
            ("\\u1234", "\\x{1234}"),
            ("\\u1234\\u0001", "\\x{1234}\\x{0001}"),
            ("foo\\u1234bar", "foo\\x{1234}bar"),
            ("\\\\\\u1234", "\\\\\\x{1234}"),
            ("foo\\\\\\u1234", "foo\\\\\\x{1234}"),
            ("\\u{1234}", "\\x{1234}"),
            ("\\u{1234}\\u{0001}", "\\x{1234}\\x{0001}"),
            ("foo\\u{1234}bar", "foo\\x{1234}bar"),
            ("[\\u00A0-\\u00FF]", "[\\x{00A0}-\\x{00FF}]"),
            ("foo\\u{123456}7bar", "foo\\u{123456}7bar"),
            ("\\u123", "\\u123"),
            ("foo", "foo"),
            ("", ""),
        ];
        for (input, expected) in cases {
            assert_eq!(unicode_escapes_to_pcre2(input), expected, "{input}");
        }
    }

    #[test]
    fn fix_regex_newline_src() {
        let cases = [
            ("foo", "foo"),
            ("invalid(", "invalid("),
            ("fo\\no", "fo\\r?\\no"),
            ("f\\no\\no", "f\\r?\\no\\r?\\no"),
            ("f[a-z\\n1]", "f(?:[a-z1]|\\r?\\n)"),
            ("f[\\n-a]", "f[\\n-a]"),
            ("(?<=\\n)\\w", "(?<=\\n)\\w"),
            ("fo\\n+o", "fo(?:\\r?\\n)+o"),
            ("fo[^\\n]o", "fo(?!\\r?\\n)o"),
            ("fo[^\\na-z]o", "fo(?!\\r?\\n|[a-z])o"),
            ("foo[^\\n]+o", "foo.+o"),
            ("foo[^\\nzq]+o", "foo[^zq]+o"),
            ("fo[^\\S\\n]*o", "fo[^\\S]*o"),
            ("fo[^\\S\\n]{3,}o", "fo[^\\S]{3,}o"),
        ];
        for (input, expected) in cases {
            assert_eq!(fix_regex_newline(input), expected, "{input}");
        }
    }

    #[test]
    fn fix_regex_newline_re() {
        let cases = [
            ("foo", "foo", true),
            ("foo\\n", "foo\r\n", true),
            ("foo\\n\\n", "foo\n\n", true),
            ("foo\\n\\n", "foo\r\n\r\n", true),
            ("foo\\n", "foo\n", true),
            ("foo\\nabc", "foo\r\nabc", true),
            ("foo\\nabc", "foo\nabc", true),
            ("foo\\r\\n", "foo\r\n", true),
            ("foo\\n+abc", "foo\r\nabc", true),
            ("foo\\n+abc", "foo\n\n\nabc", true),
            ("foo\\n+abc", "foo\r\n\r\n\r\nabc", true),
            ("foo[\\n-9]+abc", "foo1abc", true),
        ];
        for (input, text, matches) in cases {
            let fixed = fix_regex_newline(input);
            assert_eq!(regex::Regex::new(&fixed).unwrap().is_match(text), matches, "{input} => {fixed}");
        }
    }

    #[test]
    fn fix_newline_matching() {
        let cases =
            [("foo", "foo", true), ("foo\n", "foo\r\n", true), ("foo\n", "foo\n", true), ("foo\nabc", "foo\r\nabc", true), ("foo\nabc", "foo\nabc", true), ("foo\r\n", "foo\r\n", true), ("foo\nbarc", "foobar", false), ("foobar", "foo\nbar", false)];
        for (input, text, matches) in cases {
            assert_eq!(regex::Regex::new(&fix_newline(input)).unwrap().is_match(text), matches, "{input}");
        }
    }

    fn rg_match(path: &str, text: &str, line_number: usize, ranges: &[(usize, usize)]) -> String {
        let submatches: Vec<Value> = ranges.iter().map(|(start, end)| json!({ "start": start, "end": end, "match": { "text": &text[*start..*end] } })).collect();
        json!({ "type": "match", "data": { "path": { "text": path }, "lines": { "text": text }, "line_number": line_number, "absolute_offset": 0, "submatches": submatches } }).to_string()
    }

    fn range(start_line: usize, start_column: usize, end_line: usize, end_column: usize) -> SearchRange {
        SearchRange { start_line, start_column, end_line, end_column }
    }

    fn parse(lines: &[String]) -> Vec<TextSearchResult> {
        let mut parser = RipgrepParser::new(1000, DEFAULT_PREVIEW_OPTIONS);
        lines.iter().flat_map(|line| parser.handle_line(line)).collect()
    }

    fn found(path: &str, text: &str, ranges: &[(SearchRange, SearchRange)]) -> TextSearchResult {
        TextSearchResult::Match {
            relative_path: path.into(),
            preview_text: text.into(),
            ranges: ranges.iter().map(|(preview, source)| RangeLocation { preview: *preview, source: *source }).collect(),
        }
    }

    #[test]
    fn parser_single_and_multiple_results() {
        assert_eq!(parse(&[rg_match("file1.js", "foobar", 4, &[(3, 6)])]), [found("file1.js", "foobar", &[(range(0, 3, 0, 6), range(3, 3, 3, 6))])]);
        assert_eq!(
            parse(&[rg_match("file1.js", "foobar", 4, &[(3, 6)]), rg_match("app/file2.js", "foobar", 4, &[(3, 6)])]),
            [found("file1.js", "foobar", &[(range(0, 3, 0, 6), range(3, 3, 3, 6))]), found("app/file2.js", "foobar", &[(range(0, 3, 0, 6), range(3, 3, 3, 6))])]
        );
    }

    #[test]
    fn parser_drops_the_dot_folder() {
        assert_eq!(parse(&[rg_match("./sub/file1.js", "foobar", 4, &[(3, 6)])]), [found("sub/file1.js", "foobar", &[(range(0, 3, 0, 6), range(3, 3, 3, 6))])]);
    }

    #[test]
    fn parser_empty_result() {
        assert_eq!(
            parse(&[rg_match("file1.js", "foobar", 4, &[]), rg_match("file1.js", "", 5, &[])]),
            [found("file1.js", "foobar", &[(range(0, 0, 0, 1), range(3, 0, 3, 1))]), found("file1.js", "", &[(range(0, 0, 0, 0), range(4, 0, 4, 0))])]
        );
    }

    #[test]
    fn parser_multiple_submatches() {
        assert_eq!(
            parse(&[rg_match("file1.js", "foobarbazquux", 4, &[(0, 4), (6, 10)])]),
            [found("file1.js", "foobarbazquux", &[(range(0, 0, 0, 4), range(3, 0, 3, 4)), (range(0, 6, 0, 10), range(3, 6, 3, 10))])]
        );
        assert_eq!(
            parse(&[rg_match("file1.js", "foo\nbar\nbaz\nquux", 4, &[(0, 5), (8, 13)])]),
            [found("file1.js", "foo\nbar\nbaz\nquux", &[(range(0, 0, 1, 1), range(3, 0, 4, 1)), (range(2, 0, 3, 1), range(5, 0, 6, 1))])]
        );
    }

    fn folder_options(includes: &[&str], excludes: &[&str]) -> FolderOptions {
        FolderOptions {
            folder: "/some/folder".into(),
            includes: includes.iter().map(|include| include.to_string()).collect(),
            excludes: excludes.iter().map(|exclude| exclude.to_string()).collect(),
            use_ignore_files_local: false,
            use_ignore_files_parent: false,
            use_ignore_files_global: false,
            follow_symlinks: false,
            encoding: "utf8".into(),
            ignore_glob_case: false,
            exclude_exceptions: Vec::new(),
            nested_folders: Vec::new(),
        }
    }

    #[test]
    fn rg_args_simple_includes() {
        let pattern = PatternInfo { pattern: "test".into(), ..PatternInfo::default() };
        let cases: [(&[&str], &[&str]); 4] = [
            (&["a/*", "b/*"], &["-g", "!*", "-g", "/a", "-g", "/a/*", "-g", "/b", "-g", "/b/*"]),
            (&["**/a/*", "b/*"], &["-g", "!*", "-g", "/b", "-g", "/b/*", "-g", "**/a/*"]),
            (&["**/a/*", "**/b/*"], &["-g", "**/a/*", "-g", "**/b/*"]),
            (&["foo/*bar/something/**"], &["-g", "!*", "-g", "/foo", "-g", "/foo/*bar", "-g", "/foo/*bar/something", "-g", "/foo/*bar/something/**"]),
        ];
        for (includes, from_includes) in cases {
            let mut expected = vec!["--hidden", "--no-require-git", "--ignore-case"];
            expected.extend(from_includes);
            expected.extend(["--no-ignore", "--crlf", "--fixed-strings", "--no-config", "--no-ignore-global", "--json", "--", "test", "."]);
            assert_eq!(rg_args(&pattern, &folder_options(includes, &[]), None, 0, None), expected);
        }
    }

    #[test]
    fn rg_args_exclude_exception_run() {
        let pattern = PatternInfo { pattern: "test".into(), ..PatternInfo::default() };
        let mut options = folder_options(&["src/**"], &["node_modules"]);
        options.use_ignore_files_local = true;
        options.use_ignore_files_parent = true;
        assert_eq!(
            rg_args(&pattern, &options, None, 0, Some(&["node_modules/@types".to_string()])),
            [
                "--hidden", "--no-require-git", "--ignore-case", "-g", "!*", "-g", "/node_modules/", "-g", "/node_modules/@types", "-g",
                "/node_modules/@types/**", "-g", "!**/.git", "-g", "!**/.svn", "-g", "!**/.hg", "-g", "!**/.jj", "--no-ignore", "--crlf",
                "--fixed-strings", "--no-config", "--no-ignore-global", "--json", "--", "test", "."
            ]
        );
    }

    fn one_line_preview_text(text: &str, source: SearchRange, chars_per_line: usize) -> String {
        let (preview, locations) = text_search_match(text, &[source], PreviewOptions { match_lines: 1, chars_per_line });
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].source, source);
        let preview: Vec<u16> = preview.encode_utf16().collect();
        let end = locations[0].preview.end_column.min(preview.len());
        String::from_utf16_lossy(&preview[locations[0].preview.start_column.min(end)..end])
    }

    #[test]
    fn text_search_match_one_line_previews() {
        let long = "long text very long text long text very long text long text very long text long text very long text long text very long text";
        assert_eq!(one_line_preview_text("", range(5, 0, 5, 0), 100), "");
        assert_eq!(one_line_preview_text("foo bar", range(5, 4, 5, 7), 100), "bar");
        assert_eq!(one_line_preview_text("long text very long text foo", range(5, 25, 5, 28), 100), "foo");
        assert_eq!(one_line_preview_text(&format!("foo {long}"), range(5, 0, 5, 3), 100), "foo");
        assert_eq!(
            one_line_preview_text("long text very long text long foo text very long text long text very long text long text very long text long text very long text", range(5, 30, 5, 33), 100),
            "foo"
        );
        assert_eq!(one_line_preview_text("foo bar", range(0, 4, 0, 7), 1), "b");
    }

    #[test]
    fn text_search_match_one_line_of_multiline_match() {
        let (preview, locations) = text_search_match("foo bar\nfoo bar", &[range(5, 4, 6, 3)], PreviewOptions { match_lines: 1, chars_per_line: 10000 });
        assert_eq!(preview, "foo bar\nfoo bar");
        assert_eq!(locations, [RangeLocation { source: range(5, 4, 6, 3), preview: range(0, 4, 1, 3) }]);
    }

    #[test]
    fn text_search_match_compacts_multiple_ranges_on_long_lines() {
        let text = "foo bar 123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890 foo bar baz bar";
        let (preview, locations) = text_search_match(text, &[range(5, 4, 5, 7), range(5, 133, 5, 136), range(5, 141, 5, 144)], PreviewOptions { match_lines: 1, chars_per_line: 10 });
        assert_eq!(locations.iter().map(|location| location.preview).collect::<Vec<_>>(), [range(0, 4, 0, 7), range(0, 42, 0, 45), range(0, 50, 0, 53)]);
        assert_eq!(preview, "foo bar 123456⟪ 117 characters skipped ⟫o bar baz bar");
    }

    #[test]
    fn text_search_match_trims_line_endings() {
        let options = PreviewOptions { match_lines: 1, chars_per_line: 10000 };
        assert_eq!(text_search_match("foo bar\n", &[range(5, 3, 5, 5)], options).0, "foo bar");
        assert_eq!(text_search_match("foo bar\r\n", &[range(5, 3, 5, 5)], options).0, "foo bar");
    }
}
