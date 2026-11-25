//! The Problems view's filter: markersFilterOptions.ts `FilterOptions` and markersTreeViewer.ts `Filter` (which
//! resources and markers the tree shows, with the text matches it highlights).

use std::sync::Arc;

use files::glob::{self, ExcludeExpression, Options};
use regex::Regex;
use serde_json::{Map, Value};

use crate::filters::{FuzzyScoreOptions, Matches, create_matches, fuzzy_score, matches_fuzzy};
use crate::markers::{ERROR, INFO, Marker, WARNING};

/// `files.exclude` per workspace folder (folder path, expression), or one expression without folders.
pub enum FilesExclude {
    Roots(Vec<(String, Map<String, Value>)>),
    Global(Map<String, Value>),
}

pub struct FilterOptions {
    pub show_warnings: bool,
    pub show_errors: bool,
    pub show_infos: bool,
    pub text: String,
    pub negate: bool,
    pub include_source_filters: Vec<String>,
    pub exclude_source_filters: Vec<String>,
    excludes: ResourceGlobMatcher,
    includes: ResourceGlobMatcher,
}

/// `ResourceGlobMatcher`: a folder's own expression on the path inside it, then the global one on the whole path.
struct ResourceGlobMatcher {
    global: ExcludeExpression,
    roots: Vec<(String, ExcludeExpression)>,
}

impl ResourceGlobMatcher {
    fn new(global: &Map<String, Value>, roots: &[(String, Map<String, Value>)]) -> Self {
        let roots = roots.iter().map(|(root, expression)| (root.trim_end_matches('/').to_string(), glob::parse_exclude_expression(expression, Options::default()))).collect();
        Self { global: glob::parse_exclude_expression(global, Options::default()), roots }
    }

    fn matches(&self, path: &str) -> bool {
        // TernarySearchTree `findSubstr`: the longest root containing the path.
        let root = self.roots.iter().filter(|(root, _)| path.strip_prefix(root.as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))).max_by_key(|(root, _)| root.len());
        if let Some((root, expression)) = root {
            let relative = path[root.len()..].trim_start_matches('/');
            if !relative.is_empty() && expression.matches(relative, None, None).is_some() {
                return true;
            }
        }
        self.global.matches(path, None, None).is_some()
    }
}

/// `SOURCE_FILTER_REGEX`.
static SOURCE_FILTER: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| Regex::new(r#"(?i)(!)?@source:("[^"]*"|[^\s,]+)(\s*)"#).unwrap());

impl FilterOptions {
    pub fn new(filter: &str, files_exclude: FilesExclude, show_warnings: bool, show_errors: bool, show_infos: bool) -> Self {
        let mut filter = filter.trim().to_string();
        let (mut roots, mut excludes) = match files_exclude {
            FilesExclude::Roots(roots) => (roots, Map::new()),
            FilesExclude::Global(expression) => (Vec::new(), expression),
        };
        for (_, expression) in &mut roots {
            let patterns: Vec<(String, Value)> = expression.iter().filter(|(pattern, _)| !pattern.ends_with("/**")).map(|(pattern, value)| (pattern.clone(), value.clone())).collect();
            for (pattern, value) in patterns {
                // Append `/**` to match a parent folder (#103631).
                expression.insert(format!("{}/**", pattern.trim_end_matches('/')), value);
            }
        }

        let mut include_source_filters = Vec::new();
        let mut exclude_source_filters = Vec::new();
        while let Some(found) = SOURCE_FILTER.captures(&filter) {
            let mut source = &found[2];
            if source.len() >= 2 && source.starts_with('"') && source.ends_with('"') {
                source = &source[1..source.len() - 1];
            }
            match found.get(1).is_some() {
                true => exclude_source_filters.push(source.to_lowercase()),
                false => include_source_filters.push(source.to_lowercase()),
            }
            let whole = found.get(0).unwrap().range();
            filter = format!("{}{}", &filter[..whole.start], &filter[whole.end..]).trim().to_string();
        }

        let negate = filter.starts_with('!');
        let text = if negate { filter.trim_start_matches('!') } else { &filter }.trim().to_string();
        let mut includes = Map::new();
        if !filter.is_empty() {
            for part in glob::split_glob_aware(&filter, ',').iter().map(|part| part.trim()).filter(|part| !part.is_empty()) {
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
        }

        Self {
            show_warnings,
            show_errors,
            show_infos,
            text,
            negate,
            include_source_filters,
            exclude_source_filters,
            excludes: ResourceGlobMatcher::new(&excludes, &roots),
            includes: ResourceGlobMatcher::new(&includes, &[]),
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

/// `setPattern`: ".js" becomes "*.js", matched as a file or a folder anywhere.
fn set_pattern(expression: &mut Map<String, Value>, pattern: &str) {
    let pattern = if pattern.starts_with('.') { format!("*{pattern}") } else { pattern.to_string() };
    expression.insert(format!("**/{pattern}/**"), Value::Bool(true));
    expression.insert(format!("**/{pattern}"), Value::Bool(true));
}

/// `FilterOptions._filter`: `matchesFuzzy2`.
pub fn matches_fuzzy2(pattern: &str, word: &str) -> Option<Matches> {
    let score = fuzzy_score(pattern, &pattern.to_lowercase(), 0, word, &word.to_lowercase(), 0, FuzzyScoreOptions { first_match_can_be_weak: true, boost_full_match: true })?;
    Some(create_matches(Some(&score)))
}

/// A shown marker with its matches: per message line, in the source and in the code.
pub struct FilteredMarker {
    pub marker: Arc<Marker>,
    pub line_matches: Vec<Matches>,
    pub source_matches: Matches,
    pub code_matches: Matches,
}

/// A shown resource: the matches in its file name and its shown markers.
pub struct FilteredResource {
    pub uri_matches: Matches,
    pub markers: Vec<FilteredMarker>,
}

/// `TreeVisibility` of a resource for its markers.
#[derive(Clone, Copy, PartialEq)]
enum Parent {
    Visible,
    Recurse,
}

impl FilterOptions {
    /// `Filter.filter` over one resource (`path` is the URI path, `name` its basename): `None` hides it, as the tree
    /// does a `Recurse` resource without a shown marker.
    pub fn filter_resource(&self, path: &str, name: &str, markers: &[Arc<Marker>]) -> Option<FilteredResource> {
        if self.excludes.matches(path) {
            return None;
        }
        let (parent, uri_matches) = if self.includes.matches(path) {
            (Parent::Visible, Vec::new())
        } else if !self.text.is_empty() && !self.negate && let Some(matches) = matches_fuzzy2(&self.text, name) {
            (Parent::Visible, matches)
        } else {
            (Parent::Recurse, Vec::new())
        };
        let markers: Vec<FilteredMarker> = markers.iter().filter_map(|marker| self.filter_marker(marker, parent)).collect();
        (parent == Parent::Visible || !markers.is_empty()).then_some(FilteredResource { uri_matches, markers })
    }

    fn filter_marker(&self, marker: &Arc<Marker>, parent: Parent) -> Option<FilteredMarker> {
        let matches_severity = match marker.severity {
            ERROR => self.show_errors,
            WARNING => self.show_warnings,
            INFO => self.show_infos,
            _ => false,
        };
        if !matches_severity || !self.matches_source_filters(marker.source.as_deref()) {
            return None;
        }
        let shown = |line_matches, source_matches, code_matches| Some(FilteredMarker { marker: marker.clone(), line_matches, source_matches, code_matches });
        if self.text.is_empty() {
            return shown(Vec::new(), Vec::new(), Vec::new());
        }
        let line_matches: Vec<Matches> = marker.message.split('\n').map(|line| matches_fuzzy(&self.text, line).unwrap_or_default()).collect();
        let source_matches = marker.source.as_deref().and_then(|source| matches_fuzzy2(&self.text, source));
        let code_matches = marker_code(marker).and_then(|code| matches_fuzzy2(&self.text, &code));
        let matched = source_matches.is_some() || code_matches.is_some() || line_matches.iter().any(|matches| !matches.is_empty());
        match (matched, self.negate) {
            (true, false) => shown(line_matches, source_matches.unwrap_or_default(), code_matches.unwrap_or_default()),
            (true, true) if parent == Parent::Recurse => None,
            (false, true) if parent == Parent::Recurse => shown(Vec::new(), Vec::new(), Vec::new()),
            _ => (parent == Parent::Visible).then(|| FilteredMarker { marker: marker.clone(), line_matches: Vec::new(), source_matches: Vec::new(), code_matches: Vec::new() }),
        }
    }
}

/// A table row (`MarkerTableItem`) with its matches.
pub struct TableMarker {
    pub marker: Arc<Marker>,
    pub source_matches: Matches,
    pub code_matches: Matches,
    pub message_matches: Matches,
    pub file_matches: Matches,
}

impl FilterOptions {
    /// markersTable.ts `reset` for one marker of the resource at `path`, `file_label` its relative label.
    pub fn filter_table_marker(&self, path: &str, file_label: &str, marker: &Arc<Marker>) -> Option<TableMarker> {
        let row = |source_matches, code_matches, message_matches, file_matches| Some(TableMarker { marker: marker.clone(), source_matches, code_matches, message_matches, file_matches });
        if self.excludes.matches(path) {
            return None;
        }
        if self.includes.matches(path) {
            return row(Vec::new(), Vec::new(), Vec::new(), Vec::new());
        }
        let matches_severity = match marker.severity {
            ERROR => self.show_errors,
            WARNING => self.show_warnings,
            INFO => self.show_infos,
            _ => false,
        };
        if !matches_severity || !self.matches_source_filters(marker.source.as_deref()) {
            return None;
        }
        if self.text.is_empty() {
            return row(Vec::new(), Vec::new(), Vec::new(), Vec::new());
        }
        let source_matches = marker.source.as_deref().and_then(|source| matches_fuzzy2(&self.text, source));
        let code_matches = marker_code(marker).and_then(|code| matches_fuzzy2(&self.text, &code));
        let message_matches = matches_fuzzy(&self.text, &marker.message);
        let file_matches = matches_fuzzy(&self.text, file_label);
        let matched = source_matches.is_some() || code_matches.is_some() || message_matches.is_some() || file_matches.is_some();
        if matched == self.negate {
            return None;
        }
        row(source_matches.unwrap_or_default(), code_matches.unwrap_or_default(), message_matches.unwrap_or_default(), file_matches.unwrap_or_default())
    }
}

/// `IMarker.code`: a string, or `{ value, target }`.
pub fn marker_code(marker: &Marker) -> Option<String> {
    match &marker.code {
        Value::String(code) => Some(code.clone()),
        Value::Object(code) => code.get("value").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
