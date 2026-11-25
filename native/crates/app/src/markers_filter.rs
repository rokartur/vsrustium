//! markersFilterOptions.ts and markersTreeViewer.ts `Filter`: the Problems view's filter text as `@source:` filters,
//! include and exclude globs and a fuzzy text filter, and which resources and markers it keeps.

use std::sync::LazyLock;

use files::glob::{ExcludeExpression, Options, parse_exclude_expression, split_glob_aware};
use regex::Regex;
use serde_json::{Map, Value};

use crate::filters::{FuzzyScoreOptions, Matches, create_matches, fuzzy_score, matches_fuzzy};
use crate::markers::{ERROR, INFO, Marker, WARNING};

static SOURCE_FILTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)(!)?@source:("[^"]*"|[^\s,]+)(\s*)"#).unwrap());

/// The `files.exclude` expressions of the "Hide Excluded Files" filter: per workspace folder, or global without one.
pub enum FilesExclude {
    ByRoot(Vec<(String, Map<String, Value>)>),
    Global(Map<String, Value>),
}

/// `ResourceGlobMatcher`: the expression of the folder containing the path, else the global one on the whole path.
pub struct ResourceGlobMatcher {
    global: ExcludeExpression,
    by_root: Vec<(String, ExcludeExpression)>,
}

impl ResourceGlobMatcher {
    fn new(global: &Map<String, Value>, by_root: &[(String, Map<String, Value>)]) -> ResourceGlobMatcher {
        let parse = |expression| parse_exclude_expression(expression, Options::default());
        ResourceGlobMatcher { global: parse(global), by_root: by_root.iter().map(|(root, expression)| (root.trim_end_matches('/').to_owned(), parse(expression))).collect() }
    }

    /// `matches(resource)` for the resource's path: the longest root containing it (`TernarySearchTree.findSubstr`).
    pub fn matches(&self, path: &str) -> bool {
        let root = self.by_root.iter().filter(|(root, _)| path.strip_prefix(root.as_str()).is_some_and(|rest| rest.starts_with('/'))).max_by_key(|(root, _)| root.len());
        if let Some((root, expression)) = root {
            let relative = &path[root.len() + 1..];
            if !relative.is_empty() && expression.matches(relative, None, None).is_some() {
                return true;
            }
        }
        self.global.matches(path, None, None).is_some()
    }
}

pub struct FilterOptions {
    pub show_warnings: bool,
    pub show_errors: bool,
    pub show_infos: bool,
    pub text: String,
    pub negate: bool,
    pub excludes: ResourceGlobMatcher,
    pub includes: ResourceGlobMatcher,
    pub include_source_filters: Vec<String>,
    pub exclude_source_filters: Vec<String>,
}

impl FilterOptions {
    pub fn new(filter: &str, files_exclude: FilesExclude, show_warnings: bool, show_errors: bool, show_infos: bool) -> FilterOptions {
        let mut filter = filter.trim().to_owned();
        let (mut by_root, mut excludes) = match files_exclude {
            FilesExclude::ByRoot(by_root) => (by_root, Map::new()),
            FilesExclude::Global(expression) => (Vec::new(), expression),
        };
        for (_, expression) in &mut by_root {
            // #103631: a pattern also matches below a folder it names.
            let patterns: Vec<(String, Value)> = expression.iter().filter(|(pattern, _)| !pattern.ends_with("/**")).map(|(pattern, value)| (pattern.clone(), value.clone())).collect();
            for (pattern, value) in patterns {
                expression.insert(format!("{}/**", pattern.trim_end_matches('/')), value);
            }
        }

        let mut include_source_filters = Vec::new();
        let mut exclude_source_filters = Vec::new();
        while let Some(found) = SOURCE_FILTER.captures(&filter) {
            let source = &found[2];
            let source = source.strip_prefix('"').and_then(|source| source.strip_suffix('"')).unwrap_or(source).to_lowercase();
            match found.get(1).is_some() {
                true => exclude_source_filters.push(source),
                false => include_source_filters.push(source),
            }
            let range = found.get(0).unwrap().range();
            filter = format!("{}{}", &filter[..range.start], &filter[range.end..]).trim().to_owned();
        }

        let negate = filter.starts_with('!');
        let text = filter.trim_start_matches('!').trim().to_owned();
        let mut includes = Map::new();
        for part in split_glob_aware(&filter, ',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            match part.strip_prefix('!') {
                Some(_) => {
                    let pattern = part.trim_start_matches('!');
                    if !pattern.is_empty() {
                        set_pattern(&mut excludes, pattern);
                    }
                }
                None => set_pattern(&mut includes, part),
            }
        }
        FilterOptions {
            show_warnings,
            show_errors,
            show_infos,
            text,
            negate,
            excludes: ResourceGlobMatcher::new(&excludes, &by_root),
            includes: ResourceGlobMatcher::new(&includes, &[]),
            include_source_filters,
            exclude_source_filters,
        }
    }

    pub fn matches_source_filters(&self, source: Option<&str>) -> bool {
        if self.include_source_filters.is_empty() && self.exclude_source_filters.is_empty() {
            return true;
        }
        let source = source.map(str::to_lowercase);
        if source.as_ref().is_some_and(|source| self.exclude_source_filters.contains(source)) {
            return false;
        }
        if !self.include_source_filters.is_empty() {
            return source.is_some_and(|source| self.include_source_filters.contains(&source));
        }
        true
    }
}

/// `setPattern`: ".js" means "*.js"; the pattern matches a file or a folder anywhere.
fn set_pattern(expression: &mut Map<String, Value>, pattern: &str) {
    let pattern = if pattern.starts_with('.') { format!("*{pattern}") } else { pattern.to_owned() };
    expression.insert(format!("**/{pattern}/**"), Value::Bool(true));
    expression.insert(format!("**/{pattern}"), Value::Bool(true));
}

/// `matchesFuzzy2`: fuzzy score with a weak first match and a boosted full match.
pub fn matches_fuzzy2(pattern: &str, word: &str) -> Option<Matches> {
    let options = FuzzyScoreOptions { first_match_can_be_weak: true, boost_full_match: true };
    fuzzy_score(pattern, &pattern.to_lowercase(), 0, word, &word.to_lowercase(), 0, options).map(|score| create_matches(Some(&score)))
}

/// `TreeVisibility` of the tree filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Hidden,
    Visible,
    /// Visible when a child is.
    Recurse,
}

/// The highlights of a kept row (`FilterData`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Highlights {
    pub uri: Matches,
    pub lines: Vec<Matches>,
    pub source: Matches,
    pub code: Matches,
    pub message: Matches,
}

/// `filterResourceMarkers`: globs first, then the text on the file name; else the markers decide.
pub fn filter_resource(options: &FilterOptions, path: &str, basename: &str) -> (Visibility, Highlights) {
    if options.excludes.matches(path) {
        return (Visibility::Hidden, Highlights::default());
    }
    if options.includes.matches(path) {
        return (Visibility::Visible, Highlights::default());
    }
    if !options.text.is_empty() && !options.negate {
        if let Some(uri) = matches_fuzzy2(&options.text, basename) {
            return (Visibility::Visible, Highlights { uri, ..Highlights::default() });
        }
    }
    (Visibility::Recurse, Highlights::default())
}

/// `filterMarker`: severity and source filters, then the text on the message lines, the source and the code.
pub fn filter_marker(options: &FilterOptions, marker: &Marker, parent: Visibility) -> (Visibility, Highlights) {
    let severity = match marker.severity {
        ERROR => options.show_errors,
        WARNING => options.show_warnings,
        INFO => options.show_infos,
        _ => false,
    };
    if !severity || !options.matches_source_filters(marker.source.as_deref()) {
        return (Visibility::Hidden, Highlights::default());
    }
    if options.text.is_empty() {
        return (Visibility::Visible, Highlights::default());
    }
    let lines: Vec<Matches> = marker_lines(&marker.message).iter().map(|line| matches_fuzzy(&options.text, line).unwrap_or_default()).collect();
    let source = marker.source.as_deref().and_then(|source| matches_fuzzy2(&options.text, source));
    let code = marker_code(marker).and_then(|code| matches_fuzzy2(&options.text, &code));
    let matched = source.is_some() || code.is_some() || lines.iter().any(|line| !line.is_empty());
    let highlights = Highlights { lines, source: source.unwrap_or_default(), code: code.unwrap_or_default(), ..Highlights::default() };
    negatable(options, matched, parent, highlights)
}

/// `filterRelatedInformation`: the text on the related resource's file name and the message's basename.
pub fn filter_related(options: &FilterOptions, basename: &str, message: &str, parent: Visibility) -> (Visibility, Highlights) {
    if options.text.is_empty() {
        return (Visibility::Visible, Highlights::default());
    }
    let uri = matches_fuzzy2(&options.text, basename);
    let message_basename = message.rsplit(['/', '\\']).next().unwrap_or(message);
    let message = matches_fuzzy(&options.text, message_basename);
    let matched = uri.is_some() || message.is_some();
    negatable(options, matched, parent, Highlights { uri: uri.unwrap_or_default(), message: message.unwrap_or_default(), ..Highlights::default() })
}

/// A match shows unless negated; negation hides matches and shows the rest, only where the parent left it open.
fn negatable(options: &FilterOptions, matched: bool, parent: Visibility, highlights: Highlights) -> (Visibility, Highlights) {
    if matched && !options.negate {
        return (Visibility::Visible, highlights);
    }
    if options.negate && parent == Visibility::Recurse {
        return (if matched { Visibility::Hidden } else { Visibility::Visible }, Highlights::default());
    }
    (parent, Highlights::default())
}

/// `Marker.lines`: the message split at line breaks (`splitLines`).
pub fn marker_lines(message: &str) -> Vec<&str> {
    message.split("\r\n").flat_map(|line| line.split(['\r', '\n'])).collect()
}

/// The marker's code as text: a string, or `{ value, target }`'s value.
pub fn marker_code(marker: &Marker) -> Option<String> {
    match &marker.code {
        Value::String(code) => Some(code.clone()),
        Value::Object(code) => code.get("value").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
