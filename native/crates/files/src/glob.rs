//! Port of base/common/glob.ts: VS Code's glob dialect (`*` within a segment, `**` across segments, `{a,b}`, `[...]`),
//! the trivial-pattern shortcuts that give `**/name` and `{**/*.a,**/*.b}` their own semantics, expressions with
//! sibling clauses, and exclude expressions with `!` exceptions. Paths are matched as given, on macOS (`/` separator,
//! `isLinux` false). Relative patterns (`IRelativePattern`) are not ported: callers pass folder-relative paths.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

pub const GLOBSTAR: &str = "**";
pub const GLOB_SPLIT: char = '/';

const PATH_REGEX: &str = r"[/\\]";
const NO_PATH_REGEX: &str = r"[^/\\]";

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub trim_for_exclusions: bool,
    pub ignore_case: bool,
}

/// A parsed pattern or expression: `matches` returns the matching pattern's text, like upstream's `ParsedExpression`.
#[derive(Clone, Debug)]
pub struct Glob {
    matcher: Matcher,
    ignore_case: bool,
    /// `basenames` and `patterns`: set on basename patterns so an expression can aggregate them into one lookup.
    basenames: Vec<String>,
    patterns: Vec<String>,
    all_basenames: Vec<String>,
    all_paths: Vec<String>,
}

#[derive(Clone, Debug)]
enum Matcher {
    Null,
    EndsWith { suffix: String, pattern: String },
    Basename { base: String, pattern: String },
    AnyOf { globs: Vec<Glob>, pattern: String },
    Path { target: String, match_path_ends: bool, pattern: String },
    Regex { regex: Regex, pattern: String },
    Aggregate,
    Sibling { inner: Box<Glob>, when: String, pattern: String },
    Expression { globs: Vec<Glob>, requires_siblings: bool },
}

pub fn match_pattern(pattern: &str, path: &str, options: Options) -> bool {
    !pattern.is_empty() && parse(pattern, options).matches(path, None, None).is_some()
}

pub fn parse(pattern: &str, options: Options) -> Glob {
    parse_pattern(pattern, options)
}

/// `parse(expression)`: keys are patterns; `false` disables one, `{ "when": "$(basename).ts" }` makes it depend on a
/// sibling, and any other value enables it.
pub fn parse_expression(expression: &serde_json::Map<String, Value>, options: Options) -> Glob {
    let mut globs = Vec::new();
    for (pattern, value) in expression {
        let glob = parse_expression_pattern(pattern, value, options);
        if !glob.is_empty() {
            globs.push(glob);
        }
    }
    let globs = aggregate_basename_matches(globs, None, options.ignore_case);
    if globs.is_empty() {
        return Glob::null(options.ignore_case);
    }
    let requires_siblings = globs.iter().any(|glob| matches!(glob.matcher, Matcher::Sibling { .. }));
    if !requires_siblings && globs.len() == 1 {
        return globs.into_iter().next().expect("one glob");
    }
    let mut result = Glob::null(options.ignore_case);
    result.all_basenames = globs.iter().find(|glob| !glob.all_basenames.is_empty()).map(|glob| glob.all_basenames.clone()).unwrap_or_default();
    result.all_paths = globs.iter().flat_map(|glob| glob.all_paths.clone()).collect();
    result.matcher = Matcher::Expression { globs, requires_siblings };
    result
}

impl Glob {
    fn null(ignore_case: bool) -> Self {
        Glob { matcher: Matcher::Null, ignore_case, basenames: Vec::new(), patterns: Vec::new(), all_basenames: Vec::new(), all_paths: Vec::new() }
    }

    fn with(matcher: Matcher, ignore_case: bool) -> Self {
        Glob { matcher, ..Glob::null(ignore_case) }
    }

    /// `isEmptyPattern`: the pattern never matches anything.
    pub fn is_empty(&self) -> bool {
        matches!(self.matcher, Matcher::Null)
    }

    /// `getBasenameTerms`.
    pub fn basename_terms(&self) -> &[String] {
        &self.all_basenames
    }

    /// `getPathTerms`.
    pub fn path_terms(&self) -> &[String] {
        &self.all_paths
    }

    pub fn is_match(&self, path: &str) -> bool {
        self.matches(path, None, None).is_some()
    }

    /// The matching pattern, or None. `basename` saves recomputing it; `has_sibling` answers sibling clauses.
    pub fn matches(&self, path: &str, basename: Option<&str>, has_sibling: Option<&dyn Fn(&str) -> bool>) -> Option<&str> {
        self.test(path, basename.filter(|name| !name.is_empty()), None, has_sibling)
    }

    fn test(&self, path: &str, basename: Option<&str>, name: Option<&str>, has_sibling: Option<&dyn Fn(&str) -> bool>) -> Option<&str> {
        match &self.matcher {
            Matcher::Null => None,
            Matcher::EndsWith { suffix, pattern } => self.ends_with(path, suffix).then_some(pattern.as_str()),
            Matcher::Basename { base, pattern } => {
                let hit = match basename {
                    Some(basename) => self.equals(basename, base),
                    None => self.equals(path, base) || self.ends_with(path, &format!("/{base}")) || self.ends_with(path, &format!("\\{base}")),
                };
                hit.then_some(pattern.as_str())
            }
            Matcher::AnyOf { globs, pattern } => globs.iter().any(|glob| glob.test(path, basename, None, None).is_some()).then_some(pattern.as_str()),
            Matcher::Path { target, match_path_ends, pattern } => {
                let hit = self.equals(path, target) || (*match_path_ends && self.ends_with(path, &format!("/{target}")));
                hit.then_some(pattern.as_str())
            }
            Matcher::Regex { regex, pattern } => regex.is_match(path).then_some(pattern.as_str()),
            Matcher::Aggregate => {
                let basename = basename.unwrap_or_else(|| path.rsplit(['/', '\\']).next().unwrap_or(path));
                let index = self.basenames.iter().position(|candidate| self.equals(candidate, basename))?;
                Some(self.patterns[index].as_str())
            }
            Matcher::Sibling { inner, when, pattern } => {
                let has_sibling = has_sibling?;
                inner.test(path, basename, None, None)?;
                let clause = when.replacen("$(basename)", name.unwrap_or_default(), 1);
                has_sibling(&clause).then_some(pattern.as_str())
            }
            Matcher::Expression { globs, requires_siblings } => {
                let mut base = basename.map(str::to_string);
                let mut name: Option<String> = None;
                for glob in globs {
                    if *requires_siblings && matches!(glob.matcher, Matcher::Sibling { .. }) && has_sibling.is_some() {
                        let base = base.get_or_insert_with(|| path_basename(path).to_string());
                        if name.is_none() {
                            name = Some(base[..base.len() - extname(path).len()].to_string());
                        }
                    }
                    if let Some(pattern) = glob.test(path, base.as_deref().filter(|base| !base.is_empty()), name.as_deref(), has_sibling) {
                        return Some(pattern);
                    }
                }
                None
            }
        }
    }

    fn equals(&self, a: &str, b: &str) -> bool {
        if self.ignore_case { a.eq_ignore_ascii_case(b) } else { a == b }
    }

    fn ends_with(&self, text: &str, suffix: &str) -> bool {
        if !self.ignore_case {
            return text.ends_with(suffix);
        }
        text.len() >= suffix.len() && text.as_bytes()[text.len() - suffix.len()..].eq_ignore_ascii_case(suffix.as_bytes())
    }
}

fn path_basename(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or_default()
}

/// posix `extname`: from the last dot of the basename, unless the basename starts with it.
fn extname(path: &str) -> &str {
    let base = path_basename(path);
    match base.rfind('.') {
        Some(index) if index > 0 => &base[index..],
        _ => "",
    }
}

fn stars_to_regex(star_count: usize, is_last_pattern: bool) -> String {
    match star_count {
        0 => String::new(),
        1 => format!("{NO_PATH_REGEX}*?"),
        _ => {
            let tail = if is_last_pattern { format!("|{PATH_REGEX}{NO_PATH_REGEX}+") } else { String::new() };
            format!("(?:{PATH_REGEX}|{NO_PATH_REGEX}+{PATH_REGEX}{tail})*?")
        }
    }
}

pub fn split_glob_aware(pattern: &str, split_char: char) -> Vec<String> {
    let mut segments = Vec::new();
    let mut in_braces = false;
    let mut in_brackets = false;
    let mut current = String::new();
    for char in pattern.chars() {
        if char == split_char && !in_braces && !in_brackets {
            segments.push(std::mem::take(&mut current));
            continue;
        }
        match char {
            '{' => in_braces = true,
            '}' => in_braces = false,
            '[' => in_brackets = true,
            ']' => in_brackets = false,
            _ => {}
        }
        current.push(char);
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments
}

fn escape_char(char: char) -> String {
    regex::escape(char.encode_utf8(&mut [0; 4]))
}

fn parse_regex(pattern: &str) -> String {
    if pattern.is_empty() {
        return String::new();
    }
    let segments = split_glob_aware(pattern, GLOB_SPLIT);
    if segments.iter().all(|segment| segment == GLOBSTAR) {
        return ".*".to_string();
    }
    let mut regex = String::new();
    let mut previous_segment_was_globstar = false;
    for (index, segment) in segments.iter().enumerate() {
        if segment == GLOBSTAR {
            if !previous_segment_was_globstar {
                regex += &stars_to_regex(2, index == segments.len() - 1);
            }
            previous_segment_was_globstar = true;
            continue;
        }
        let mut in_braces = false;
        let mut brace_value = String::new();
        let mut in_brackets = false;
        let mut bracket_value = String::new();
        for char in segment.chars() {
            if char != '}' && in_braces {
                brace_value.push(char);
                continue;
            }
            // `]` is only a literal as the first character in brackets.
            if in_brackets && (char != ']' || bracket_value.is_empty()) {
                bracket_value += &match char {
                    '-' => "-".to_string(),
                    '^' | '!' if bracket_value.is_empty() => "^".to_string(),
                    GLOB_SPLIT => String::new(),
                    _ => escape_char(char),
                };
                continue;
            }
            match char {
                '{' => in_braces = true,
                '[' => in_brackets = true,
                '}' => {
                    let choices: Vec<String> = split_glob_aware(&brace_value, ',').iter().map(|choice| parse_regex(choice)).collect();
                    regex += &format!("(?:{})", choices.join("|"));
                    in_braces = false;
                    brace_value.clear();
                }
                ']' => {
                    regex += &bracket_class(&bracket_value);
                    in_brackets = false;
                    bracket_value.clear();
                }
                '?' => regex += NO_PATH_REGEX,
                '*' => regex += &stars_to_regex(1, false),
                _ => regex += &escape_char(char),
            }
        }
        // The slash split on, unless only a trailing globstar follows: `some/**/*.js` must not match `something`.
        let more_follows = index < segments.len() - 1 && (segments[index + 1] != GLOBSTAR || index + 2 < segments.len());
        if more_follows {
            regex += PATH_REGEX;
        }
        previous_segment_was_globstar = false;
    }
    regex
}

/// JavaScript's `[]` matches nothing and `[^]` any character; Rust's regex rejects both.
fn bracket_class(value: &str) -> String {
    match value {
        "" => "[a&&b]".to_string(),
        "^" => "(?s:.)".to_string(),
        _ => format!("[{value}]"),
    }
}

const WORD: &str = "[A-Za-z0-9_.-]";
static T1: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"^\*\*/\*\.{WORD}+$")).expect("valid"));
static T2: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"^\*\*/({WORD}+)/?$")).expect("valid"));
static T3: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"^\{{\*\*/\*?{WORD}+/?(,\*\*/\*?{WORD}+/?)*\}}$")).expect("valid"));
static T3_2: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^\{{\*\*/\*?{WORD}+(/(\*\*)?)?(,\*\*/\*?{WORD}+(/(\*\*)?)?)*\}}$")).expect("valid"));
static T4: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"^\*\*((/{WORD}+)+)/?$")).expect("valid"));
static T5: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"^({WORD}+(/{WORD}+)*)/?$")).expect("valid"));

fn parse_pattern(pattern: &str, options: Options) -> Glob {
    let ignore_case = options.ignore_case;
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Glob::null(ignore_case);
    }
    let trimmed = if options.trim_for_exclusions && pattern.ends_with("/**") { &pattern[..pattern.len() - 2] } else { pattern };
    let owned = pattern.to_string();
    if T1.is_match(pattern) {
        return Glob::with(Matcher::EndsWith { suffix: pattern[4..].to_string(), pattern: owned }, ignore_case);
    }
    if let Some(captures) = T2.captures(trimmed) {
        let base = captures[1].to_string();
        let mut glob = Glob::with(Matcher::Basename { base: base.clone(), pattern: owned.clone() }, ignore_case);
        glob.basenames = vec![base.clone()];
        glob.patterns = vec![owned];
        glob.all_basenames = vec![base];
        return glob;
    }
    if (if options.trim_for_exclusions { &*T3_2 } else { &*T3 }).is_match(pattern) {
        return trivia3(pattern, options);
    }
    if let Some(captures) = T4.captures(trimmed) {
        return path_glob(captures[1][1..].to_string(), owned, true, ignore_case);
    }
    if let Some(captures) = T5.captures(trimmed) {
        return path_glob(captures[1].to_string(), owned, false, ignore_case);
    }
    let flags = if ignore_case { "(?i)" } else { "" };
    match Regex::new(&format!("{flags}^{}$", parse_regex(pattern))) {
        Ok(regex) => Glob::with(Matcher::Regex { regex, pattern: owned }, ignore_case),
        Err(_) => Glob::null(ignore_case),
    }
}

/// `{**/*.txt,**/*.png}`: each alternative parsed on its own, basename ones aggregated.
fn trivia3(pattern: &str, options: Options) -> Glob {
    let globs: Vec<Glob> = pattern[1..pattern.len() - 1]
        .split(',')
        .map(|alternative| parse_pattern(alternative, options))
        .filter(|glob| !glob.is_empty())
        .collect();
    let mut globs = aggregate_basename_matches(globs, Some(pattern), options.ignore_case);
    match globs.len() {
        0 => return Glob::null(options.ignore_case),
        1 => return globs.pop().expect("one glob"),
        _ => {}
    }
    let mut glob = Glob::null(options.ignore_case);
    glob.all_basenames = globs.iter().find(|glob| !glob.all_basenames.is_empty()).map(|glob| glob.all_basenames.clone()).unwrap_or_default();
    glob.all_paths = globs.iter().flat_map(|glob| glob.all_paths.clone()).collect();
    glob.matcher = Matcher::AnyOf { globs, pattern: pattern.to_string() };
    glob
}

/// `**/something/else` (path ends) and `something/else` (path equals).
fn path_glob(target: String, pattern: String, match_path_ends: bool, ignore_case: bool) -> Glob {
    let all_paths = vec![format!("{}{target}", if match_path_ends { "*/" } else { "./" })];
    Glob { all_paths, ..Glob::with(Matcher::Path { target, match_path_ends, pattern }, ignore_case) }
}

fn parse_expression_pattern(pattern: &str, value: &Value, options: Options) -> Glob {
    if *value == Value::Bool(false) {
        return Glob::null(options.ignore_case);
    }
    let glob = parse_pattern(pattern, options);
    if glob.is_empty() {
        return glob;
    }
    match value.get("when").and_then(Value::as_str) {
        Some(when) => Glob::with(
            Matcher::Sibling { inner: Box::new(glob), when: when.to_string(), pattern: pattern.to_string() },
            options.ignore_case,
        ),
        None => glob,
    }
}

/// Folds every basename pattern into one lookup, placed after the others.
fn aggregate_basename_matches(globs: Vec<Glob>, result: Option<&str>, ignore_case: bool) -> Vec<Glob> {
    if globs.iter().filter(|glob| !glob.basenames.is_empty()).count() < 2 {
        return globs;
    }
    let (with_basenames, mut others): (Vec<Glob>, Vec<Glob>) = globs.into_iter().partition(|glob| !glob.basenames.is_empty());
    let basenames: Vec<String> = with_basenames.iter().flat_map(|glob| glob.basenames.clone()).collect();
    let patterns = match result {
        Some(result) => vec![result.to_string(); basenames.len()],
        None => with_basenames.iter().flat_map(|glob| glob.patterns.clone()).collect(),
    };
    let mut aggregate = Glob::with(Matcher::Aggregate, ignore_case);
    aggregate.all_basenames = basenames.clone();
    aggregate.basenames = basenames;
    aggregate.patterns = patterns;
    others.push(aggregate);
    others
}

const EXCLUDE_EXCEPTION_PREFIX: char = '!';
const VERSION_CONTROL_FOLDERS: [&str; 4] = [".git", ".svn", ".hg", ".jj"];

pub fn is_exclude_exception(pattern: &str) -> bool {
    pattern.trim_start().starts_with(EXCLUDE_EXCEPTION_PREFIX)
}

/// Rewrites a leading `!` so it matches a literal `!` instead of making an exception.
pub fn escape_exclude_exception(pattern: &str) -> String {
    let trimmed = pattern.trim_start();
    match trimmed.strip_prefix(EXCLUDE_EXCEPTION_PREFIX) {
        Some(rest) => format!("{{!}}{rest}"),
        None => pattern.to_string(),
    }
}

/// The enabled `!` keys without the `!`; absolute ones are ignored, exceptions being relative to the searched folder.
pub fn get_exclude_exceptions(expression: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut exceptions = Vec::new();
    for (key, value) in expression {
        if *value != Value::Bool(true) || !is_exclude_exception(key) {
            continue;
        }
        let pattern = key.trim_start()[1..].trim().trim_end_matches(GLOB_SPLIT);
        if !pattern.is_empty() && !pattern.starts_with('/') {
            exceptions.push(pattern.to_string());
        }
    }
    exceptions
}

/// The version control folders exceptions do not look into, unless an exception names the folder.
pub fn get_folders_skipped_by_exclude_exceptions(exceptions: &[String]) -> Vec<String> {
    let named: HashSet<String> = exceptions
        .iter()
        .flat_map(|exception| expand_braces(exception))
        .flat_map(|pattern| split_glob_aware(&pattern, GLOB_SPLIT))
        .collect();
    VERSION_CONTROL_FOLDERS.iter().filter(|folder| !named.contains(**folder)).map(|folder| folder.to_string()).collect()
}

/// `{a,b}/c` into `a/c` and `b/c`; braces do not nest.
fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(start) = pattern.find('{') else { return vec![pattern.to_string()] };
    let Some(end) = pattern[start..].find('}').map(|offset| start + offset) else { return vec![pattern.to_string()] };
    let prefix = &pattern[..start];
    let mut alternatives = split_glob_aware(&pattern[start + 1..end], ',');
    if alternatives.is_empty() {
        alternatives.push(String::new());
    }
    let suffixes = expand_braces(&pattern[end + 1..]);
    alternatives.iter().flat_map(|alternative| suffixes.iter().map(move |suffix| format!("{prefix}{alternative}{suffix}"))).collect()
}

pub fn without_exclude_exceptions(expression: &serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
    expression.iter().filter(|(key, _)| !is_exclude_exception(key)).map(|(key, value)| (key.clone(), value.clone())).collect()
}

/// `IParsedExcludeExpression`: a path is excluded when it matches an exclude pattern and neither it nor a parent
/// folder matches an exception.
#[derive(Clone, Debug)]
pub struct ExcludeExpression {
    pub matches_exclude: Glob,
    pub exceptions: Vec<String>,
    matches_exception: Glob,
    exception_segments: Vec<Vec<Segment>>,
    skipped_folders: HashSet<String>,
}

#[derive(Clone, Debug)]
enum Segment {
    Globstar,
    Never,
    Regex(Regex),
}

pub fn parse_exclude_expression(expression: &serde_json::Map<String, Value>, options: Options) -> ExcludeExpression {
    let exceptions = get_exclude_exceptions(expression);
    let has_exception_keys = expression.keys().any(|key| is_exclude_exception(key));
    let matches_exclude =
        if has_exception_keys { parse_expression(&without_exclude_exceptions(expression), options) } else { parse_expression(expression, options) };
    let mut exception_expression = serde_json::Map::new();
    for exception in &exceptions {
        exception_expression.insert(exception.clone(), Value::Bool(true));
        exception_expression.insert(format!("{exception}{GLOB_SPLIT}{GLOBSTAR}"), Value::Bool(true));
    }
    let matches_exception = parse_expression(&exception_expression, Options { trim_for_exclusions: false, ..options });
    let exception_segments = exceptions.iter().map(|exception| parse_segment_matchers(exception, options.ignore_case)).collect();
    let skipped_folders = get_folders_skipped_by_exclude_exceptions(&exceptions).into_iter().collect();
    ExcludeExpression { matches_exclude, exceptions, matches_exception, exception_segments, skipped_folders }
}

impl ExcludeExpression {
    pub fn matches(&self, path: &str, basename: Option<&str>, has_sibling: Option<&dyn Fn(&str) -> bool>) -> Option<&str> {
        let pattern = self.matches_exclude.matches(path, basename, has_sibling)?;
        (!self.is_excepted(path, basename)).then_some(pattern)
    }

    /// The path or a parent folder matches an exception; paths in version control folders are not excepted.
    pub fn is_excepted(&self, path: &str, basename: Option<&str>) -> bool {
        !self.exceptions.is_empty() && self.matches_exception.matches(path, basename, None).is_some() && !self.is_in_skipped_folder(&to_segments(path))
    }

    /// An exception could match below the folder, so the folder is walked even when excluded.
    pub fn could_exception_match_beneath(&self, folder_path: &str) -> bool {
        let segments = to_segments(folder_path);
        !self.is_in_skipped_folder(&segments) && self.exception_segments.iter().any(|matchers| could_match_beneath(matchers, &segments))
    }

    fn is_in_skipped_folder(&self, segments: &[&str]) -> bool {
        segments.iter().any(|segment| self.skipped_folders.contains(*segment))
    }
}

fn to_segments(path: &str) -> Vec<&str> {
    let trimmed = path.trim_end_matches(GLOB_SPLIT).trim_end_matches('\\');
    if trimmed.is_empty() { Vec::new() } else { trimmed.split(['/', '\\']).collect() }
}

fn parse_segment_matchers(pattern: &str, ignore_case: bool) -> Vec<Segment> {
    split_glob_aware(pattern, GLOB_SPLIT)
        .iter()
        .map(|segment| {
            // A brace group holding a separator or a globstar, `{a/b,c}` or `{**,c}`, can span any number of segments.
            if segment == GLOBSTAR || segment.contains(GLOB_SPLIT) || has_globstar_alternative(segment) {
                return Segment::Globstar;
            }
            let flags = if ignore_case { "(?i)" } else { "" };
            match Regex::new(&format!("{flags}^{}$", parse_regex(segment))) {
                Ok(regex) => Segment::Regex(regex),
                Err(_) => Segment::Never,
            }
        })
        .collect()
}

fn has_globstar_alternative(segment: &str) -> bool {
    static BRACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{([^}]*)\}").expect("valid"));
    BRACES.captures_iter(segment).any(|captures| split_glob_aware(&captures[1], ',').iter().any(|alternative| alternative == GLOBSTAR))
}

/// Whether the pattern could match a path starting with the folder segments.
fn could_match_beneath(matchers: &[Segment], folder: &[&str]) -> bool {
    let mut visited = HashSet::new();
    visit(matchers, folder, 0, 0, &mut visited)
}

fn visit(matchers: &[Segment], folder: &[&str], matcher_index: usize, folder_index: usize, visited: &mut HashSet<(usize, usize)>) -> bool {
    if folder_index == folder.len() {
        return true;
    }
    if matcher_index == matchers.len() || !visited.insert((matcher_index, folder_index)) {
        return false;
    }
    match &matchers[matcher_index] {
        Segment::Globstar => {
            visit(matchers, folder, matcher_index + 1, folder_index, visited) || visit(matchers, folder, matcher_index, folder_index + 1, visited)
        }
        Segment::Never => false,
        Segment::Regex(regex) => regex.is_match(folder[folder_index]) && visit(matchers, folder, matcher_index + 1, folder_index + 1, visited),
    }
}

pub struct ExcludeTreeEntry<'a> {
    pub path: &'a str,
    pub basename: Option<&'a str>,
    pub is_directory: bool,
    /// A parent folder was excluded: its entries stay excluded unless an exception re-includes them.
    pub is_parent_excluded: bool,
    pub is_hidden: bool,
    pub has_sibling: Option<&'a dyn Fn(&str) -> bool>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Evaluation {
    pub is_excluded: bool,
    pub is_visited: bool,
}

/// `IExcludeTreeEvaluator`: what `evaluate_exclude_tree_entry` asks of an exclude expression.
pub trait ExcludeTreeEvaluator {
    fn matches_exclude(&self, path: &str, basename: Option<&str>, has_sibling: Option<&dyn Fn(&str) -> bool>) -> bool;
    fn is_excepted(&self, path: &str, basename: Option<&str>) -> bool;
    fn could_exception_match_beneath(&self, folder_path: &str) -> bool;
}

impl ExcludeTreeEvaluator for ExcludeExpression {
    fn matches_exclude(&self, path: &str, basename: Option<&str>, has_sibling: Option<&dyn Fn(&str) -> bool>) -> bool {
        self.matches_exclude.matches(path, basename, has_sibling).is_some()
    }

    fn is_excepted(&self, path: &str, basename: Option<&str>) -> bool {
        ExcludeExpression::is_excepted(self, path, basename)
    }

    fn could_exception_match_beneath(&self, folder_path: &str) -> bool {
        ExcludeExpression::could_exception_match_beneath(self, folder_path)
    }
}

/// `evaluateExcludeTreeEntry`: an excluded folder is still visited when an exception could match beneath it.
pub fn evaluate_exclude_tree_entry(evaluator: &dyn ExcludeTreeEvaluator, entry: &ExcludeTreeEntry) -> Evaluation {
    let is_matched = entry.is_parent_excluded || evaluator.matches_exclude(entry.path, entry.basename, entry.has_sibling) || entry.is_hidden;
    let is_excluded = is_matched && !evaluator.is_excepted(entry.path, entry.basename);
    Evaluation { is_excluded, is_visited: !is_excluded || (entry.is_directory && evaluator.could_exception_match_beneath(entry.path)) }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn yes(pattern: &str, path: &str, ignore_case: bool) {
        assert!(match_pattern(pattern, path, Options { ignore_case, ..Options::default() }), "{pattern} should match {path}");
    }

    fn no(pattern: &str, path: &str, ignore_case: bool) {
        assert!(!match_pattern(pattern, path, Options { ignore_case, ..Options::default() }), "{pattern} should not match {path}");
    }

    #[test]
    fn simple() {
        let mut p = "node_modules";
        yes(p, "node_modules", false);
        no(p, "node_module", false);
        no(p, "/node_modules", false);
        no(p, "test/node_modules", false);
        p = "test.txt";
        yes(p, "test.txt", false);
        no(p, "test?txt", false);
        no(p, "/text.txt", false);
        no(p, "test/test.txt", false);
        p = "test(.txt";
        yes(p, "test(.txt", false);
        no(p, "test?txt", false);
        p = "qunit";
        yes(p, "qunit", false);
        no(p, "qunit.css", false);
        no(p, "test/qunit", false);
        p = "/DNXConsoleApp/**/*.cs";
        yes(p, "/DNXConsoleApp/Program.cs", false);
        yes(p, "/DNXConsoleApp/foo/Program.cs", false);
        p = "C:/DNXConsoleApp/**/*.cs";
        yes(p, "C:\\DNXConsoleApp\\Program.cs", false);
        yes(p, "C:\\DNXConsoleApp\\foo\\Program.cs", false);
        p = "*";
        yes(p, "", false);
    }

    #[test]
    fn dot_hidden() {
        let mut p = ".*";
        yes(p, ".git", false);
        yes(p, ".hidden.txt", false);
        no(p, "git", false);
        no(p, "hidden.txt", false);
        no(p, "path/.git", false);
        no(p, "path/.hidden.txt", false);
        p = "**/.*";
        yes(p, ".git", false);
        yes(p, "/.git", false);
        yes(p, ".hidden.txt", false);
        no(p, "git", false);
        no(p, "hidden.txt", false);
        yes(p, "path/.git", false);
        yes(p, "path/.hidden.txt", false);
        yes(p, "/path/.git", false);
        yes(p, "/path/.hidden.txt", false);
        no(p, "path/git", false);
        no(p, "pat.h/hidden.txt", false);
        p = "._*";
        yes(p, "._git", false);
        yes(p, "._hidden.txt", false);
        no(p, "git", false);
        no(p, "hidden.txt", false);
        no(p, "path/._git", false);
        no(p, "path/._hidden.txt", false);
        p = "**/._*";
        yes(p, "._git", false);
        yes(p, "._hidden.txt", false);
        no(p, "git", false);
        no(p, "hidden._txt", false);
        yes(p, "path/._git", false);
        yes(p, "path/._hidden.txt", false);
        yes(p, "/path/._git", false);
        yes(p, "/path/._hidden.txt", false);
        no(p, "path/git", false);
        no(p, "pat.h/hidden._txt", false);
    }

    #[test]
    fn file_pattern() {
        let mut p = "*.js";
        yes(p, "foo.js", false);
        no(p, "folder/foo.js", false);
        no(p, "/node_modules/foo.js", false);
        no(p, "foo.jss", false);
        no(p, "some.js/test", false);
        p = "html.*";
        yes(p, "html.js", false);
        yes(p, "html.txt", false);
        no(p, "htm.txt", false);
        p = "*.*";
        yes(p, "html.js", false);
        yes(p, "html.txt", false);
        yes(p, "htm.txt", false);
        no(p, "folder/foo.js", false);
        no(p, "/node_modules/foo.js", false);
        p = "node_modules/test/*.js";
        yes(p, "node_modules/test/foo.js", false);
        no(p, "folder/foo.js", false);
        no(p, "/node_module/test/foo.js", false);
        no(p, "foo.jss", false);
        no(p, "some.js/test", false);
    }

    #[test]
    fn star() {
        let mut p = "node*modules";
        yes(p, "node_modules", false);
        yes(p, "node_super_modules", false);
        no(p, "node_module", false);
        no(p, "/node_modules", false);
        no(p, "test/node_modules", false);
        p = "*";
        yes(p, "html.js", false);
        yes(p, "html.txt", false);
        yes(p, "htm.txt", false);
        no(p, "folder/foo.js", false);
        no(p, "/node_modules/foo.js", false);
    }

    #[test]
    fn file_folder_match() {
        let p = "**/node_modules/**";
        yes(p, "node_modules", false);
        yes(p, "node_modules/", false);
        yes(p, "a/node_modules", false);
        yes(p, "a/node_modules/", false);
        yes(p, "node_modules/foo", false);
        yes(p, "foo/node_modules/foo/bar", false);
        yes(p, "/node_modules", false);
        yes(p, "/node_modules/", false);
        yes(p, "/a/node_modules", false);
        yes(p, "/a/node_modules/", false);
        yes(p, "/node_modules/foo", false);
        yes(p, "/foo/node_modules/foo/bar", false);
    }

    #[test]
    fn questionmark() {
        let mut p = "node?modules";
        yes(p, "node_modules", false);
        no(p, "node_super_modules", false);
        no(p, "node_module", false);
        no(p, "/node_modules", false);
        no(p, "test/node_modules", false);
        p = "?";
        yes(p, "h", false);
        no(p, "html.txt", false);
        no(p, "htm.txt", false);
        no(p, "folder/foo.js", false);
        no(p, "/node_modules/foo.js", false);
    }

    #[test]
    fn globstar() {
        let mut p = "**/*.js";
        yes(p, "foo.js", false);
        yes(p, "/foo.js", false);
        yes(p, "folder/foo.js", false);
        yes(p, "/node_modules/foo.js", false);
        no(p, "foo.jss", false);
        no(p, "some.js/test", false);
        no(p, "/some.js/test", false);
        no(p, "\\some.js\\test", false);
        p = "**/project.json";
        yes(p, "project.json", false);
        yes(p, "/project.json", false);
        yes(p, "some/folder/project.json", false);
        yes(p, "/some/folder/project.json", false);
        no(p, "some/folder/file_project.json", false);
        no(p, "some/folder/fileproject.json", false);
        no(p, "some/rrproject.json", false);
        no(p, "some\\rrproject.json", false);
        p = "test/**";
        yes(p, "test", false);
        yes(p, "test/foo", false);
        yes(p, "test/foo/", false);
        yes(p, "test/foo.js", false);
        yes(p, "test/other/foo.js", false);
        no(p, "est/other/foo.js", false);
        p = "**";
        yes(p, "/", false);
        yes(p, "foo.js", false);
        yes(p, "folder/foo.js", false);
        yes(p, "folder/foo/", false);
        yes(p, "/node_modules/foo.js", false);
        yes(p, "foo.jss", false);
        yes(p, "some.js/test", false);
        p = "test/**/*.js";
        yes(p, "test/foo.js", false);
        yes(p, "test/other/foo.js", false);
        yes(p, "test/other/more/foo.js", false);
        no(p, "test/foo.ts", false);
        no(p, "test/other/foo.ts", false);
        no(p, "test/other/more/foo.ts", false);
        p = "**/**/*.js";
        yes(p, "foo.js", false);
        yes(p, "/foo.js", false);
        yes(p, "folder/foo.js", false);
        yes(p, "/node_modules/foo.js", false);
        no(p, "foo.jss", false);
        no(p, "some.js/test", false);
        p = "**/node_modules/**/*.js";
        no(p, "foo.js", false);
        no(p, "folder/foo.js", false);
        yes(p, "node_modules/foo.js", false);
        yes(p, "/node_modules/foo.js", false);
        yes(p, "node_modules/some/folder/foo.js", false);
        yes(p, "/node_modules/some/folder/foo.js", false);
        no(p, "node_modules/some/folder/foo.ts", false);
        no(p, "foo.jss", false);
        no(p, "some.js/test", false);
        p = "{**/node_modules/**,**/.git/**,**/bower_components/**}";
        yes(p, "node_modules", false);
        yes(p, "/node_modules", false);
        yes(p, "/node_modules/more", false);
        yes(p, "some/test/node_modules", false);
        yes(p, "some\\test\\node_modules", false);
        yes(p, "/some/test/node_modules", false);
        yes(p, "\\some\\test\\node_modules", false);
        yes(p, "C:\\\\some\\test\\node_modules", false);
        yes(p, "C:\\\\some\\test\\node_modules\\more", false);
        yes(p, "bower_components", false);
        yes(p, "bower_components/more", false);
        yes(p, "/bower_components", false);
        yes(p, "some/test/bower_components", false);
        yes(p, "some\\test\\bower_components", false);
        yes(p, "/some/test/bower_components", false);
        yes(p, "\\some\\test\\bower_components", false);
        yes(p, "C:\\\\some\\test\\bower_components", false);
        yes(p, "C:\\\\some\\test\\bower_components\\more", false);
        yes(p, ".git", false);
        yes(p, "/.git", false);
        yes(p, "some/test/.git", false);
        yes(p, "some\\test\\.git", false);
        yes(p, "/some/test/.git", false);
        yes(p, "\\some\\test\\.git", false);
        yes(p, "C:\\\\some\\test\\.git", false);
        no(p, "tempting", false);
        no(p, "/tempting", false);
        no(p, "some/test/tempting", false);
        no(p, "some\\test\\tempting", false);
        no(p, "/some/test/tempting", false);
        no(p, "\\some\\test\\tempting", false);
        no(p, "C:\\\\some\\test\\tempting", false);
        p = "{**/package.json,**/project.json}";
        yes(p, "package.json", false);
        yes(p, "/package.json", false);
        no(p, "xpackage.json", false);
        no(p, "/xpackage.json", false);
    }

    #[test]
    fn issue_41724() {
        let mut p = "some/**/*.js";
        yes(p, "some/foo.js", false);
        yes(p, "some/folder/foo.js", false);
        no(p, "something/foo.js", false);
        no(p, "something/folder/foo.js", false);
        p = "some/**/*";
        yes(p, "some/foo.js", false);
        yes(p, "some/folder/foo.js", false);
        no(p, "something/foo.js", false);
        no(p, "something/folder/foo.js", false);
    }

    #[test]
    fn brace_expansion() {
        let mut p = "*.{html,js}";
        yes(p, "foo.js", false);
        yes(p, "foo.html", false);
        no(p, "folder/foo.js", false);
        no(p, "/node_modules/foo.js", false);
        no(p, "foo.jss", false);
        no(p, "some.js/test", false);
        p = "*.{html}";
        yes(p, "foo.html", false);
        no(p, "foo.js", false);
        no(p, "folder/foo.js", false);
        no(p, "/node_modules/foo.js", false);
        no(p, "foo.jss", false);
        no(p, "some.js/test", false);
        p = "{node_modules,testing}";
        yes(p, "node_modules", false);
        yes(p, "testing", false);
        no(p, "node_module", false);
        no(p, "dtesting", false);
        p = "**/{foo,bar}";
        yes(p, "foo", false);
        yes(p, "bar", false);
        yes(p, "test/foo", false);
        yes(p, "test/bar", false);
        yes(p, "other/more/foo", false);
        yes(p, "other/more/bar", false);
        yes(p, "/foo", false);
        yes(p, "/bar", false);
        yes(p, "/test/foo", false);
        yes(p, "/test/bar", false);
        yes(p, "/other/more/foo", false);
        yes(p, "/other/more/bar", false);
        p = "{foo,bar}/**";
        yes(p, "foo", false);
        yes(p, "bar", false);
        yes(p, "bar/", false);
        yes(p, "foo/test", false);
        yes(p, "bar/test", false);
        yes(p, "bar/test/", false);
        yes(p, "foo/other/more", false);
        yes(p, "bar/other/more", false);
        yes(p, "bar/other/more/", false);
        p = "{**/*.d.ts,**/*.js}";
        yes(p, "foo.js", false);
        yes(p, "testing/foo.js", false);
        yes(p, "testing\\foo.js", false);
        yes(p, "/testing/foo.js", false);
        yes(p, "\\testing\\foo.js", false);
        yes(p, "C:\\testing\\foo.js", false);
        yes(p, "foo.d.ts", false);
        yes(p, "testing/foo.d.ts", false);
        yes(p, "testing\\foo.d.ts", false);
        yes(p, "/testing/foo.d.ts", false);
        yes(p, "\\testing\\foo.d.ts", false);
        yes(p, "C:\\testing\\foo.d.ts", false);
        no(p, "foo.d", false);
        no(p, "testing/foo.d", false);
        no(p, "testing\\foo.d", false);
        no(p, "/testing/foo.d", false);
        no(p, "\\testing\\foo.d", false);
        no(p, "C:\\testing\\foo.d", false);
        p = "{**/*.d.ts,**/*.js,path/simple.jgs}";
        yes(p, "foo.js", false);
        yes(p, "testing/foo.js", false);
        yes(p, "testing\\foo.js", false);
        yes(p, "/testing/foo.js", false);
        yes(p, "path/simple.jgs", false);
        no(p, "/path/simple.jgs", false);
        yes(p, "\\testing\\foo.js", false);
        yes(p, "C:\\testing\\foo.js", false);
        p = "{**/*.d.ts,**/*.js,foo.[0-9]}";
        yes(p, "foo.5", false);
        yes(p, "foo.8", false);
        no(p, "bar.5", false);
        no(p, "foo.f", false);
        yes(p, "foo.js", false);
        p = "prefix/{**/*.d.ts,**/*.js,foo.[0-9]}";
        yes(p, "prefix/foo.5", false);
        yes(p, "prefix/foo.8", false);
        no(p, "prefix/bar.5", false);
        no(p, "prefix/foo.f", false);
        yes(p, "prefix/foo.js", false);
    }

    #[test]
    fn brackets() {
        let mut p = "foo.[0-9]";
        yes(p, "foo.5", false);
        yes(p, "foo.8", false);
        no(p, "bar.5", false);
        no(p, "foo.f", false);
        p = "foo.[^0-9]";
        no(p, "foo.5", false);
        no(p, "foo.8", false);
        no(p, "bar.5", false);
        yes(p, "foo.f", false);
        p = "foo.[!0-9]";
        no(p, "foo.5", false);
        no(p, "foo.8", false);
        no(p, "bar.5", false);
        yes(p, "foo.f", false);
        p = "foo.[0!^*?]";
        no(p, "foo.5", false);
        no(p, "foo.8", false);
        yes(p, "foo.0", false);
        yes(p, "foo.!", false);
        yes(p, "foo.^", false);
        yes(p, "foo.*", false);
        yes(p, "foo.?", false);
        p = "foo[/]bar";
        no(p, "foo/bar", false);
        p = "foo.[[]";
        yes(p, "foo.[", false);
        p = "foo.[]]";
        yes(p, "foo.]", false);
        p = "foo.[][!]";
        yes(p, "foo.]", false);
        yes(p, "foo.[", false);
        yes(p, "foo.!", false);
        p = "foo.[]-]";
        yes(p, "foo.]", false);
        yes(p, "foo.-", false);
    }

    #[test]
    fn full_path() {
        yes("testing/this/foo.txt", "testing/this/foo.txt", false);
    }

    #[test]
    fn ending_path() {
        yes("**/testing/this/foo.txt", "some/path/testing/this/foo.txt", false);
    }

    #[test]
    fn prefix_agnostic() {
        let mut p = "**/*.js";
        yes(p, "foo.js", false);
        yes(p, "/foo.js", false);
        yes(p, "\\foo.js", false);
        yes(p, "testing/foo.js", false);
        yes(p, "testing\\foo.js", false);
        yes(p, "/testing/foo.js", false);
        yes(p, "\\testing\\foo.js", false);
        yes(p, "C:\\testing\\foo.js", false);
        no(p, "foo.ts", false);
        no(p, "testing/foo.ts", false);
        no(p, "testing\\foo.ts", false);
        no(p, "/testing/foo.ts", false);
        no(p, "\\testing\\foo.ts", false);
        no(p, "C:\\testing\\foo.ts", false);
        no(p, "foo.js.txt", false);
        no(p, "testing/foo.js.txt", false);
        no(p, "testing\\foo.js.txt", false);
        no(p, "/testing/foo.js.txt", false);
        no(p, "\\testing\\foo.js.txt", false);
        no(p, "C:\\testing\\foo.js.txt", false);
        no(p, "testing.js/foo", false);
        no(p, "testing.js\\foo", false);
        no(p, "/testing.js/foo", false);
        no(p, "\\testing.js\\foo", false);
        no(p, "C:\\testing.js\\foo", false);
        p = "**/foo.js";
        yes(p, "foo.js", false);
        yes(p, "/foo.js", false);
        yes(p, "\\foo.js", false);
        yes(p, "testing/foo.js", false);
        yes(p, "testing\\foo.js", false);
        yes(p, "/testing/foo.js", false);
        yes(p, "\\testing\\foo.js", false);
        yes(p, "C:\\testing\\foo.js", false);
    }

    #[test]
    fn invalid_glob() {
        let p = "**/*(.js";
        no(p, "foo.js", false);
    }

    fn expr(value: Value) -> serde_json::Map<String, Value> {
        value.as_object().expect("an object").clone()
    }

    fn parsed(value: Value) -> Glob {
        parse_expression(&expr(value), Options::default())
    }

    fn trimmed(value: Value) -> Glob {
        parse_expression(&expr(value), Options { trim_for_exclusions: true, ..Options::default() })
    }

    fn pattern(pattern: &str, path: &str, basename: Option<&str>) -> bool {
        parse(pattern, Options::default()).matches(path, basename, None).is_some()
    }

    fn trimmed_pattern(pattern: &str, path: &str, basename: Option<&str>) -> bool {
        parse(pattern, Options { trim_for_exclusions: true, ..Options::default() }).matches(path, basename, None).is_some()
    }

    fn siblings(names: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |name| names.contains(&name)
    }

    #[test]
    fn expression_support_single() {
        let has_sibling = siblings(&["test.html", "test.txt", "test.ts", "test.js"]);
        let expression = parsed(json!({ "**/*.js": { "when": "$(basename).ts" } }));
        assert_eq!(expression.matches("test.js", None, Some(&has_sibling)), Some("**/*.js"));
        assert_eq!(expression.matches("test.js", None, Some(&|_| false)), None);
        assert_eq!(expression.matches("test.js", None, Some(&|name| name == "te.ts")), None);
        assert_eq!(expression.matches("test.js", None, None), None);
        assert_eq!(parsed(json!({ "**/*.js": { "when": "" } })).matches("test.js", None, Some(&has_sibling)), None);
        assert_eq!(parsed(json!({ "**/*.js": {} })).matches("test.js", None, Some(&has_sibling)), Some("**/*.js"));
        assert_eq!(parsed(json!({})).matches("test.js", None, Some(&has_sibling)), None);
    }

    #[test]
    fn expression_support_multiple() {
        let has_sibling = siblings(&["test.html", "test.txt", "test.ts", "test.js"]);
        let expression = parsed(json!({
            "**/*.js": { "when": "$(basename).ts" },
            "**/*.as": true,
            "**/*.foo": false,
            "**/*.bananas": { "bananas": true },
        }));
        assert_eq!(expression.matches("test.js", None, Some(&has_sibling)), Some("**/*.js"));
        assert_eq!(expression.matches("test.as", None, Some(&has_sibling)), Some("**/*.as"));
        assert_eq!(expression.matches("test.bananas", None, Some(&has_sibling)), Some("**/*.bananas"));
        assert_eq!(expression.matches("test.bananas", None, None), Some("**/*.bananas"));
        assert_eq!(expression.matches("test.foo", None, Some(&has_sibling)), None);
    }

    #[test]
    fn split_glob_aware_skips_braces_and_brackets() {
        assert_eq!(split_glob_aware("foo,bar", ','), ["foo", "bar"]);
        assert_eq!(split_glob_aware("foo", ','), ["foo"]);
        assert_eq!(split_glob_aware("{foo,bar}", ','), ["{foo,bar}"]);
        assert_eq!(split_glob_aware("foo,bar,{foo,bar}", ','), ["foo", "bar", "{foo,bar}"]);
        assert_eq!(split_glob_aware("{foo,bar},foo,bar,{foo,bar}", ','), ["{foo,bar}", "foo", "bar", "{foo,bar}"]);
        assert_eq!(split_glob_aware("[foo,bar]", ','), ["[foo,bar]"]);
        assert_eq!(split_glob_aware("foo,bar,[foo,bar]", ','), ["foo", "bar", "[foo,bar]"]);
        assert_eq!(split_glob_aware("[foo,bar],foo,bar,[foo,bar]", ','), ["[foo,bar]", "foo", "bar", "[foo,bar]"]);
    }

    #[test]
    fn expressions_with_disabled_empty_and_two_globs() {
        assert_eq!(parsed(json!({ "**/*.js": false })).matches("foo.js", None, None), None);
        let two = parsed(json!({ "**/*.j?": true, "**/*.t?": true }));
        assert_eq!(two.matches("foo.js", None, None), Some("**/*.j?"));
        assert_eq!(two.matches("foo.as", None, None), None);
        assert_eq!(parsed(json!({ "": true })).matches("foo.js", None, None), None);
        assert_eq!(parsed(json!({ "**/*.js": 0 })).matches("foo.js", None, None), Some("**/*.js"));
    }

    #[test]
    fn expression_with_non_trivia_glob_issue_144458() {
        for path in ["foo/barp", "foo/bar/ap", "ap", "foo/barp1", "foo/bar/ap1", "ap1", "/foo/barp", "/foo/bar/ap", "/ap", "/foo/barp1", "/foo/bar/ap1", "/ap1"] {
            assert!(!match_pattern("**/p*", path, Options::default()), "{path}");
        }
        for path in ["foo/pbar", "/foo/pbar", "foo/bar/pa", "/p"] {
            assert!(match_pattern("**/p*", path, Options::default()), "{path}");
        }
    }

    #[test]
    fn expression_with_basename_globs() {
        let two = parsed(json!({ "**/bar": true, "**/baz": true }));
        assert_eq!(two.matches("bar", None, None), Some("**/bar"));
        assert_eq!(two.matches("foo", None, None), None);
        assert_eq!(two.matches("foo/bar", None, None), Some("**/bar"));
        assert_eq!(two.matches("foo\\bar", None, None), Some("**/bar"));
        assert_eq!(two.matches("foo/foo", None, None), None);

        let ignore_case = Options { ignore_case: true, ..Options::default() };
        let upper = parse_expression(&expr(json!({ "**/BAR": true, "**/BAZ": true })), ignore_case);
        assert_eq!(upper.matches("bar", None, None), Some("**/BAR"));
        assert_eq!(upper.matches("baz", None, None), Some("**/BAZ"));
        assert_eq!(upper.matches("src/bar", None, None), Some("**/BAR"));
        assert_eq!(upper.matches("BaR", None, None), Some("**/BAR"));
        assert_eq!(parsed(json!({ "**/BAR": true, "**/BAZ": true })).matches("bar", None, None), None);
        assert_eq!(parse_expression(&expr(json!({ "**/bar": true, "**/baz": true })), ignore_case).matches("bar", None, None), Some("**/bar"));

        let has_sibling = siblings(&["foo.ts", "foo.js", "foo", "bar"]);
        let with_siblings = parsed(json!({ "**/bar": true, "**/baz": true, "**/*.js": { "when": "$(basename).ts" } }));
        assert_eq!(with_siblings.matches("bar", None, Some(&has_sibling)), Some("**/bar"));
        assert_eq!(with_siblings.matches("foo", None, Some(&has_sibling)), None);
        assert_eq!(with_siblings.matches("foo/bar", None, Some(&has_sibling)), Some("**/bar"));
        assert_eq!(with_siblings.matches("foo/foo", None, Some(&has_sibling)), None);
        assert_eq!(with_siblings.matches("foo.js", None, Some(&has_sibling)), Some("**/*.js"));
        assert_eq!(with_siblings.matches("bar.js", None, Some(&has_sibling)), None);

        let multiple = parsed(json!({ "**/bar": true, "{**/baz,**/foo}": true }));
        assert_eq!(multiple.matches("bar", None, None), Some("**/bar"));
        assert_eq!(multiple.matches("foo", None, None), Some("{**/baz,**/foo}"));
        assert_eq!(multiple.matches("baz", None, None), Some("{**/baz,**/foo}"));
        assert_eq!(multiple.matches("abc", None, None), None);
    }

    #[test]
    fn falsy_expression_pattern_and_path() {
        assert!(!match_pattern("", "foo", Options::default()));
        assert!(parse("", Options::default()).is_empty());
        for glob in ["foo", "**/*.j?", "**/*.foo", "**/foo", "{**/baz,**/foo}", "{**/*.baz,**/*.foo}"] {
            assert!(!pattern(glob, "", None), "{glob}");
        }
    }

    #[test]
    fn expression_pattern_basename() {
        assert!(!pattern("**/foo", "bar/baz", Some("baz")));
        assert!(pattern("**/foo", "bar/foo", Some("foo")));
        assert!(!pattern("{**/baz,**/foo}", "baz/bar", Some("bar")));
        assert!(pattern("{**/baz,**/foo}", "baz/foo", Some("foo")));
        let has_sibling = siblings(&["foo.ts", "foo.js"]);
        let expression = parsed(json!({ "**/*.js": { "when": "$(basename).ts" } }));
        assert_eq!(expression.matches("bar/baz.js", Some("baz.js"), Some(&has_sibling)), None);
        assert_eq!(expression.matches("bar/foo.js", Some("foo.js"), Some(&has_sibling)), Some("**/*.js"));
    }

    #[test]
    fn expression_pattern_basename_terms() {
        let terms = |glob: &str| parse(glob, Options::default()).basename_terms().to_vec();
        assert!(terms("**/*.foo").is_empty());
        assert_eq!(terms("**/foo"), ["foo"]);
        assert_eq!(terms("**/foo/"), ["foo"]);
        assert_eq!(terms("{**/baz,**/foo}"), ["baz", "foo"]);
        assert_eq!(terms("{**/baz/,**/foo/}"), ["baz", "foo"]);
        let expression = parsed(json!({ "**/foo": true, "{**/bar,**/baz}": true, "{**/bar2/,**/baz2/}": true, "**/bulb": false }));
        assert_eq!(expression.basename_terms(), ["foo", "bar", "baz", "bar2", "baz2"]);
        assert_eq!(parsed(json!({ "**/foo": { "when": "$(basename).zip" }, "**/bar": true })).basename_terms(), ["bar"]);
    }

    #[test]
    fn expression_pattern_optimization_for_basenames() {
        assert!(parse("**/foo/**", Options::default()).basename_terms().is_empty());
        let trim = Options { trim_for_exclusions: true, ..Options::default() };
        assert_eq!(parse("**/foo/**", trim).basename_terms(), ["foo"]);

        assert!(parse("**/*.foo/**", trim).basename_terms().is_empty());
        assert!(trimmed_pattern("**/*.foo/**", "baz/bar.foo/bar/baz", None));
        assert!(trimmed_pattern("**/foo/**", "bar/foo", None));
        assert!(!trimmed_pattern("**/foo/**", "bar/foo/baz", None));
        assert_eq!(parse("{**/baz/**,**/foo/**}", trim).basename_terms(), ["baz", "foo"]);
        assert!(trimmed_pattern("{**/baz/**,**/foo/**}", "bar/baz", None));
        assert!(trimmed_pattern("{**/baz/**,**/foo/**}", "bar/foo", None));

        let expression = trimmed(json!({ "**/foo/**": true, "{**/bar/**,**/baz/**}": true, "**/bulb/**": false }));
        assert_eq!(expression.basename_terms(), ["foo", "bar", "baz"]);
        assert_eq!(expression.matches("bar/foo", None, None), Some("**/foo/**"));
        assert_eq!(expression.matches("foo/bar", None, None), Some("{**/bar/**,**/baz/**}"));
        assert_eq!(expression.matches("bar/nope", None, None), None);

        let has_sibling = siblings(&["baz", "baz.zip", "nope"]);
        let expression = trimmed(json!({ "**/foo/**": { "when": "$(basename).zip" }, "**/bar/**": true }));
        assert_eq!(expression.basename_terms(), ["bar"]);
        assert_eq!(expression.matches("bar/foo", None, None), None);
        assert_eq!(expression.matches("bar/foo/baz", None, Some(&has_sibling)), None);
        assert_eq!(expression.matches("bar/foo/nope", None, Some(&has_sibling)), None);
        assert_eq!(expression.matches("foo/bar", None, None), Some("**/bar/**"));
    }

    #[test]
    fn trailing_slash() {
        assert!(!pattern("**/foo/", "bar/baz", Some("baz")));
        assert!(pattern("**/foo/", "bar/foo", Some("foo")));
        assert!(!pattern("**/*.foo/", "bar/file.baz", Some("file.baz")));
        assert!(pattern("**/*.foo/", "bar/file.foo", Some("file.foo")));
        assert!(!pattern("{**/foo/,**/abc/}", "bar/baz", Some("baz")));
        assert!(pattern("{**/foo/,**/abc/}", "bar/foo", Some("foo")));
        assert!(pattern("{**/foo/,**/abc/}", "bar/abc", Some("abc")));
        assert!(!trimmed_pattern("{**/foo/,**/abc/}", "bar/baz", Some("baz")));
        assert!(trimmed_pattern("{**/foo/,**/abc/}", "bar/foo", Some("foo")));
        assert!(trimmed_pattern("{**/foo/,**/abc/}", "bar/abc", Some("abc")));
    }

    #[test]
    fn expression_pattern_path() {
        assert!(!pattern("**/foo/bar", "foo/baz", Some("baz")));
        assert!(pattern("**/foo/bar", "foo/bar", Some("bar")));
        assert!(pattern("**/foo/bar", "bar/foo/bar", Some("bar")));
        assert!(pattern("**/foo/bar/**", "bar/foo/bar", Some("bar")));
        assert!(pattern("**/foo/bar/**", "bar/foo/bar/baz", Some("baz")));
        assert!(trimmed_pattern("**/foo/bar/**", "bar/foo/bar", Some("bar")));
        assert!(!trimmed_pattern("**/foo/bar/**", "bar/foo/bar/baz", Some("baz")));
        assert!(!pattern("foo/bar", "foo/baz", Some("baz")));
        assert!(pattern("foo/bar", "foo/bar", Some("bar")));
        assert!(pattern("foo/bar/baz", "foo/bar/baz", Some("baz")));
        assert!(!pattern("foo/bar", "bar/foo/bar", Some("bar")));
        assert!(pattern("foo/bar/**", "foo/bar/baz", Some("baz")));
        assert!(trimmed_pattern("foo/bar/**", "foo/bar", Some("bar")));
        assert!(!trimmed_pattern("foo/bar/**", "foo/bar/baz", Some("baz")));
    }

    #[test]
    fn expression_pattern_paths() {
        let terms = |glob: &str| parse(glob, Options::default()).path_terms().to_vec();
        assert!(terms("**/*.foo").is_empty());
        assert!(terms("**/foo").is_empty());
        assert_eq!(terms("**/foo/bar"), ["*/foo/bar"]);
        assert_eq!(terms("**/foo/bar/"), ["*/foo/bar"]);
        let expression = parsed(json!({ "**/foo/bar": true, "**/foo2/bar2": true, "**/bulb": true, "**/bulb2": true, "**/bulb/foo": false }));
        assert_eq!(expression.path_terms(), ["*/foo/bar", "*/foo2/bar2"]);
        assert_eq!(expression.basename_terms(), ["bulb", "bulb2"]);
        let expression = parsed(json!({ "**/foo/bar": { "when": "$(basename).zip" }, "**/bar/foo": true, "**/bar2/foo2": true }));
        assert_eq!(expression.path_terms(), ["*/bar/foo", "*/bar2/foo2"]);
    }

    #[test]
    fn expression_pattern_optimization_for_paths() {
        let trim = Options { trim_for_exclusions: true, ..Options::default() };
        assert!(parse("**/foo/bar/**", Options::default()).path_terms().is_empty());
        assert_eq!(parse("**/foo/bar/**", trim).path_terms(), ["*/foo/bar"]);
        assert!(parse("**/*.foo/bar/**", trim).path_terms().is_empty());
        assert!(trimmed_pattern("**/*.foo/bar/**", "baz/bar.foo/bar/baz", None));
        assert!(trimmed_pattern("**/foo/bar/**", "bar/foo/bar", None));
        assert!(!trimmed_pattern("**/foo/bar/**", "bar/foo/bar/baz", None));

        let expression = trimmed(json!({ "**/foo/bar/**": true, "**/bulb/bar/**": false }));
        assert_eq!(expression.path_terms(), ["*/foo/bar"]);
        assert_eq!(expression.matches("bar/foo/bar", None, None), Some("**/foo/bar/**"));
        assert_eq!(expression.matches("/foo/bar/nope", None, None), None);

        let has_sibling = siblings(&["baz", "baz.zip", "nope"]);
        let expression = trimmed(json!({ "**/foo/123/**": { "when": "$(basename).zip" }, "**/bar/123/**": true }));
        assert_eq!(expression.path_terms(), ["*/bar/123"]);
        assert_eq!(expression.matches("bar/foo/123", None, None), None);
        assert_eq!(expression.matches("bar/foo/123/baz", None, Some(&has_sibling)), None);
        assert_eq!(expression.matches("bar/foo/123/nope", None, Some(&has_sibling)), None);
        assert_eq!(expression.matches("foo/bar/123", None, None), Some("**/bar/123/**"));
    }

    #[test]
    fn is_empty_pattern() {
        assert!(parse("", Options::default()).is_empty());
        assert!(parsed(json!({})).is_empty());
        assert!(parsed(json!({ "": true })).is_empty());
        assert!(parsed(json!({ "**/*.js": false })).is_empty());
    }

    #[test]
    fn case_insensitive_match() {
        let cases = [
            ("PATH/FOO.js", "path/foo.js"),
            ("**/*.JS", "bar/foo.js"),
            ("**/package", "bar/Package"),
            ("{**/*.JS,**/*.TS}", "bar/foo.ts"),
            ("{**/*.JS,**/*.TS}", "bar/foo.js"),
            ("{**/BAR,**/BAZ}", "bar"),
            ("**/FOO/Bar", "bar/foo/bar"),
            ("FOO/Bar", "foo/bar"),
            ("some/*/Random/*/Path.FILE", "some/very/random/unusual/path.file"),
        ];
        for (glob, path) in cases {
            no(glob, path, false);
            yes(glob, path, true);
        }
    }

    fn excluded(expression: Value, paths: &[&str], options: Options) -> Vec<bool> {
        let parsed = parse_exclude_expression(&expr(expression), options);
        paths.iter().map(|path| parsed.matches(path, None, None).is_some()).collect()
    }

    fn beneath(expression: Value, folders: &[&str]) -> Vec<bool> {
        let parsed = parse_exclude_expression(&expr(expression), Options::default());
        folders.iter().map(|folder| parsed.could_exception_match_beneath(folder)).collect()
    }

    #[test]
    fn exceptions_carve_paths_and_their_children_out_of_excludes() {
        let none = Options::default();
        assert_eq!(
            excluded(json!({ "node_modules/*": true, "!node_modules/susa": true }), &["node_modules/a", "node_modules/susa", "node_modules/susa/index.js", "src/a.js"], none),
            [true, false, false, false]
        );
        assert_eq!(excluded(json!({ "*.js": true, "!gulpfile.js": true }), &["a.js", "gulpfile.js"], none), [true, false]);
        assert_eq!(excluded(json!({ ".*": true, "!.editorconfig": true, "!.gitignore": true }), &[".env", ".editorconfig", ".gitignore"], none), [true, false, false]);
        assert_eq!(
            excluded(json!({ "**/.vscode": true, "**/.vscode/*": true, "!**/.vscode/launch.json": true }), &["a/.vscode", "a/.vscode/settings.json", "a/.vscode/launch.json"], none),
            [true, true, false]
        );
        assert_eq!(excluded(json!({ "!*.js": true, "!*.ts": true }), &["a.js", "a.ts", "a.md"], none), [false, false, false]);
        assert_eq!(
            excluded(json!({ "node_modules/*": true, "!node_modules/@types": true, "node_modules/@types/*": true }), &["node_modules/a", "node_modules/@types/node"], none),
            [true, false]
        );
    }

    #[test]
    fn disabled_and_escaped_exception_keys_and_ignore_case() {
        let none = Options::default();
        assert_eq!(excluded(json!({ "*.js": true, "!gulpfile.js": false }), &["gulpfile.js"], none), [true]);
        assert_eq!(excluded(json!({ "{!}notes.txt": true }), &["!notes.txt", "notes.txt"], none), [true, false]);
        assert_eq!(escape_exclude_exception("!notes.txt"), "{!}notes.txt");
        assert_eq!(escape_exclude_exception("notes.txt"), "notes.txt");
        let expression = json!({ "node_modules": true, "node_modules/*": true, "!node_modules/@Types": true });
        assert_eq!(excluded(expression.clone(), &["node_modules/@types"], Options { ignore_case: true, ..none }), [false]);
        assert_eq!(excluded(expression, &["node_modules/@types"], none), [true]);
    }

    #[test]
    fn sibling_clauses_are_subject_to_exceptions() {
        let parsed = parse_exclude_expression(&expr(json!({ "**/*.js": { "when": "$(basename).ts" }, "!keep.js": true })), Options::default());
        let has_sibling = |name: &str| name == "keep.ts" || name == "drop.ts";
        assert_eq!(parsed.matches("keep.js", Some("keep.js"), Some(&has_sibling)), None);
        assert_eq!(parsed.matches("drop.js", Some("drop.js"), Some(&has_sibling)), Some("**/*.js"));
    }

    #[test]
    fn get_exclude_exceptions_ignores_disabled_empty_and_absolute_exceptions() {
        let expression = expr(json!({ "a": true, "!node_modules/@types/**": true, "!**/Scripts/": true, "!src/*.js": true, "!off": false, "!": true, "!/workspace/keep.js": true }));
        assert_eq!(get_exclude_exceptions(&expression), ["node_modules/@types/**", "**/Scripts", "src/*.js"]);
    }

    #[test]
    fn evaluates_exclude_tree_entries() {
        let parsed = parse_exclude_expression(&expr(json!({ "node_modules": true, "!node_modules/@types": true })), Options::default());
        let evaluate = |path: &str, is_directory: bool, is_parent_excluded: bool, is_hidden: bool| {
            evaluate_exclude_tree_entry(&parsed, &ExcludeTreeEntry { path, basename: None, is_directory, is_parent_excluded, is_hidden, has_sibling: None })
        };
        assert_eq!(evaluate("node_modules", true, false, false), Evaluation { is_excluded: true, is_visited: true });
        assert_eq!(evaluate("node_modules/react", true, true, false), Evaluation { is_excluded: true, is_visited: false });
        assert_eq!(evaluate("node_modules/a.js", false, true, false), Evaluation { is_excluded: true, is_visited: false });
        assert_eq!(evaluate("node_modules/@types", true, true, false), Evaluation { is_excluded: false, is_visited: true });
        assert_eq!(evaluate("dist/a.js", false, false, true), Evaluation { is_excluded: true, is_visited: false });
        assert_eq!(evaluate("src/a.js", false, false, false), Evaluation { is_excluded: false, is_visited: true });
    }

    #[test]
    fn expressions_without_exceptions_parse_unchanged() {
        let parsed = parse_exclude_expression(&expr(json!({ "**/node_modules": true })), Options::default());
        assert_eq!(parsed.matches_exclude.basename_terms(), ["node_modules"]);
        assert!(!parsed.is_excepted("node_modules", None));
    }

    #[test]
    fn could_exception_match_beneath() {
        assert_eq!(
            beneath(json!({ "node_modules": true, "!node_modules/@types": true }), &["node_modules", "node_modules/@types", "node_modules/react", "src"]),
            [true, true, false, false]
        );
        assert_eq!(beneath(json!({ "**/*": true, "!**/Scripts": true }), &["a", "a/b/c"]), [true, true]);
        assert_eq!(beneath(json!({ "*.js": true, "!*.ts": true }), &["src"]), [false]);
        assert_eq!(beneath(json!({ "a": true, "!a/*/c/**/d.txt": true }), &["a", "a/b", "a/b/c", "a/b/x", "a/b/c/e/f"]), [true, true, true, false, true]);
        assert_eq!(beneath(json!({ "a": true }), &["a"]), [false]);
        assert_eq!(beneath(json!({ "a": true, "!{a/b,c}/keep.txt": true }), &["a", "a/b", "c"]), [true, true, true]);
        assert_eq!(beneath(json!({ "a": true, "!a/{b/c,d}/keep": true }), &["a/b", "a/b/c"]), [true, true]);
        assert_eq!(beneath(json!({ "**": true, "!{**,foo}/keep.txt": true }), &["a", "a/b"]), [true, true]);
        assert_eq!(beneath(json!({ "node_modules": true, "!**.js": true }), &["node_modules", "node_modules/a/b"]), [false, false]);
        assert_eq!(beneath(json!({ "src": true, "!src/**.keep": true }), &["src", "src/a"]), [true, false]);
    }

    #[test]
    fn exceptions_do_not_re_include_version_control_folders_unless_named() {
        let unnamed = parse_exclude_expression(&expr(json!({ "**/.git": true, "third_party": true, "!third_party/lib": true })), Options::default());
        let named = parse_exclude_expression(&expr(json!({ "**/.git": true, "!.git/hooks": true })), Options::default());
        let excepted: Vec<bool> = ["third_party/lib/a.js", "third_party/lib/.git", "third_party/lib/.git/config"].iter().map(|path| unnamed.is_excepted(path, None)).collect();
        assert_eq!(excepted, [true, false, false]);
        assert_eq!([named.is_excepted(".git/hooks/pre-commit", None), named.is_excepted(".git/config", None)], [true, false]);
        let git_folder = ExcludeTreeEntry { path: "third_party/lib/.git", basename: None, is_directory: true, is_parent_excluded: false, is_hidden: false, has_sibling: None };
        assert_eq!(evaluate_exclude_tree_entry(&unnamed, &git_folder), Evaluation { is_excluded: true, is_visited: false });

        assert_eq!(beneath(json!({ "**/*": true, "!**/Scripts": true }), &[".git", "a/.hg/b", "a"]), [false, false, true]);
        assert_eq!(beneath(json!({ "**/*": true, "!**/Scripts": true, "!.git/hooks": true }), &[".git", "a/.hg/b"]), [true, false]);
        assert_eq!(beneath(json!({ "**/*": true, "!{.git,.hg}/hooks": true }), &[".git", ".hg", ".svn"]), [true, true, false]);
        assert_eq!(beneath(json!({ "**/*": true, "!{*,src}/hooks": true }), &[".git", "src"]), [false, true]);
    }
}