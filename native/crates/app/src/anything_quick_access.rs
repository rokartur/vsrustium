//! The file picks of Quick Open (workbench/contrib/search/browser/anythingQuickAccess.ts): editor history and
//! file search results scored with the fuzzy scorer, and `extractRangeFromFilter` (contrib/search/common/search.ts).

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::filters::Matches;
use crate::fuzzy_scorer::{self, FuzzyScorerCache, PreparedQuery, ScorerItem};
use crate::quick_input::path_label;

/// `AnythingQuickAccessProvider.MAX_RESULTS`, also the `maxResults` of its file search.
pub const MAX_RESULTS: usize = 512;

/// `IRange`, 1-based.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TextRange {
    pub start_line_number: usize,
    pub start_column: usize,
    pub end_line_number: usize,
    pub end_column: usize,
}

// JS `\d` is ASCII only.
static LINE_COLON_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s?[#:\(](?:line )?([0-9]*)(?:[#:,]([0-9]*))?(?:-([0-9]*)(?:[#:,]([0-9]*))?)?\)?:?\s*$").expect("a valid pattern"));

/// `extractRangeFromFilter`: the filter without a trailing `:line:column` (or `#`, `(`, `-` range) suffix, and that range.
pub fn extract_range_from_filter(filter: &str, unless: &[&str]) -> Option<(String, TextRange)> {
    let blocked = unless.iter().any(|value| match filter.find(value) {
        Some(0) => true,
        Some(position) => !LINE_COLON_PATTERN.is_match(&filter[position + value.len()..]),
        None => false,
    });
    if filter.is_empty() || blocked {
        return None;
    }
    let captures = LINE_COLON_PATTERN.captures(filter)?;
    let number = |group: usize| captures.get(group).and_then(|text| text.as_str().parse::<usize>().ok());
    let range = match number(1) {
        Some(line) => {
            let column = number(2).unwrap_or(1);
            match number(3) {
                Some(end_line) => TextRange { start_line_number: line, start_column: column, end_line_number: end_line, end_column: number(4).unwrap_or(1) },
                None => TextRange { start_line_number: line, start_column: column, end_line_number: line, end_column: column },
            }
        }
        // "something:" or "something#" without a number opens at the start of the file.
        None => TextRange { start_line_number: 1, start_column: 1, end_line_number: 1, end_column: 1 },
    };
    let start = captures.get(0).expect("the whole match").start();
    Some((filter[..start].to_owned(), range))
}

/// An `IAnythingQuickPickItem` of a file: `createAnythingPick` for a resource.
#[derive(Clone, PartialEq, Debug)]
pub struct FilePick {
    pub path: PathBuf,
    pub label: String,
    /// `labelService.getUriLabel(dirname(resource), { relative: true })`; empty for a file at the workspace root.
    pub description: String,
    pub label_highlights: Matches,
    pub description_highlights: Matches,
}

impl FilePick {
    pub fn new(path: PathBuf, workspace: Option<&Path>) -> FilePick {
        let label = path.file_name().map_or_else(|| path.to_string_lossy().into_owned(), |name| name.to_string_lossy().into_owned());
        let description = path_label(path.parent().unwrap_or(&path), workspace);
        FilePick { path, label, description, label_highlights: Vec::new(), description_highlights: Vec::new() }
    }

    /// `quickPickItemScorerAccessor`, or the label only one editor history uses without path separators in the query.
    fn scorer_item(&self, skip_description: bool) -> ScorerItem<'_> {
        ScorerItem {
            label: &self.label,
            description: Some(self.description.as_str()).filter(|description| !skip_description && !description.is_empty()),
            path: self.path.to_str(),
        }
    }
}

/// `getEditorHistoryPicks`: every entry without a query, else the ones that match, sorted by score unless
/// `search.quickOpen.history.filterSortOrder` is `recency`.
pub fn editor_history_picks(history: &[PathBuf], query: &PreparedQuery, workspace: Option<&Path>, include_history: bool, sort_by_recency: bool, cache: &mut FuzzyScorerCache) -> Vec<FilePick> {
    let mut picks: Vec<FilePick> = history.iter().map(|path| FilePick::new(path.clone(), workspace)).collect();
    if query.piece.normalized.is_empty() {
        return picks;
    }
    if !include_history {
        return Vec::new();
    }
    let skip_description = !query.contains_path_separator;
    picks.retain_mut(|pick| {
        let score = fuzzy_scorer::score_item_fuzzy(Some(&pick.scorer_item(skip_description)), query, false, cache);
        pick.label_highlights = score.label_match.unwrap_or_default();
        pick.description_highlights = score.description_match.unwrap_or_default();
        score.score > 0
    });
    if !sort_by_recency {
        picks.sort_by(|a, b| fuzzy_scorer::compare_items_by_fuzzy_score(&a.scorer_item(skip_description), &b.scorer_item(skip_description), query, false, cache));
    }
    picks
}

/// A file the search lists: relative to its workspace folder, or absolute for an editor outside the workspace
/// (`IRawFileMatch.relativePath` of `extraFiles`).
#[derive(Clone, Debug)]
pub struct FileEntry {
    pub relative_path: String,
    pub path: PathBuf,
}

/// `getFileSearchResults` over the file list the query cache loaded: the first value of a multi-value query
/// searches, the full query again when that hits the result limit.
pub fn file_search_results(files: &[FileEntry], query: &PreparedQuery, cache: &mut FuzzyScorerCache) -> Vec<PathBuf> {
    let multiple = query.values.as_ref().filter(|values| values.len() > 1);
    let file_pattern = multiple.map_or(&query.piece.original, |values| &values[0].original);
    let (mut results, limit_hit) = sorted_file_search(files, file_pattern, cache);
    if limit_hit && multiple.is_some() {
        let (additional, _) = sorted_file_search(files, &query.piece.original, cache);
        for path in additional {
            if !results.contains(&path) {
                results.push(path);
            }
        }
    }
    results
}

/// rawSearchService.ts `doSortedSearch` from the cache: `isFilePatternMatch`, then the `maxResults` best by
/// `FileMatchItemAccessor`.
fn sorted_file_search(files: &[FileEntry], file_pattern: &str, cache: &mut FuzzyScorerCache) -> (Vec<PathBuf>, bool) {
    let query = fuzzy_scorer::prepare_query(file_pattern);
    let mut matches: Vec<&FileEntry> = files.iter().filter(|file| search::file_search::fuzzy_contains(&file.relative_path, &query.piece.normalized_lowercase)).collect();
    let limit_hit = matches.len() > MAX_RESULTS;
    matches.sort_by(|a, b| fuzzy_scorer::compare_items_by_fuzzy_score(&file_match_item(a), &file_match_item(b), &query, true, cache));
    matches.truncate(MAX_RESULTS);
    (matches.into_iter().map(|file| file.path.clone()).collect(), limit_hit)
}

/// `FileMatchItemAccessor`: `basename` and `dirname` of the relative path; dirname is "." for a bare name and "/"
/// for a root file.
fn file_match_item(file: &FileEntry) -> ScorerItem<'_> {
    let (description, label) = match file.relative_path.rsplit_once('/') {
        Some(("", label)) => ("/", label),
        Some(parts) => parts,
        None => (".", file.relative_path.as_str()),
    };
    ScorerItem { label, description: Some(description), path: Some(file.relative_path.as_str()) }
}

/// `getAdditionalPicks` for files: the best `MAX_RESULTS` picks by score, with their highlights.
pub fn additional_picks(mut picks: Vec<FilePick>, query: &PreparedQuery, cache: &mut FuzzyScorerCache) -> Vec<FilePick> {
    // `top` keeps equal items in their order, as a stable sort does.
    picks.sort_by(|a, b| compare_picks(a, b, query, cache));
    picks.truncate(MAX_RESULTS);
    picks.retain_mut(|pick| {
        let score = fuzzy_scorer::score_item_fuzzy(Some(&pick.scorer_item(false)), query, true, cache);
        pick.label_highlights = score.label_match.unwrap_or_default();
        pick.description_highlights = score.description_match.unwrap_or_default();
        score.score > 0
    });
    picks
}

fn compare_picks(a: &FilePick, b: &FilePick, query: &PreparedQuery, cache: &mut FuzzyScorerCache) -> Ordering {
    fuzzy_scorer::compare_items_by_fuzzy_score(&a.scorer_item(false), &b.scorer_item(false), query, true, cache)
}

/// `getFilePicks` over the loaded file list: an absolute path names the only pick, else the search results and
/// the files the query names relative to a folder, without the editor history entries, scored by `getAdditionalPicks`.
pub fn file_picks(files: &[FileEntry], query: &PreparedQuery, history: &[FilePick], workspace: Option<&Path>, home: &str, cache: &mut FuzzyScorerCache) -> Vec<FilePick> {
    if query.piece.normalized.is_empty() {
        return Vec::new();
    }
    let in_history = |path: &PathBuf| history.iter().any(|pick| &pick.path == path);
    if let Some(path) = absolute_path_result(query, home) {
        if in_history(&path) {
            return Vec::new();
        }
        // The fuzzy scorer may not match a `~` query, so the pick shows fully highlighted.
        let mut pick = FilePick::new(path, workspace);
        pick.label_highlights = vec![0..pick.label.chars().count()];
        pick.description_highlights = if pick.description.is_empty() { Vec::new() } else { vec![0..pick.description.chars().count()] };
        return vec![pick];
    }
    let mut results = file_search_results(files, query, cache);
    if let Some(relative) = relative_path_results(query, workspace.as_slice()) {
        results.retain(|path| !relative.contains(path));
        results.extend(relative);
    }
    results.retain(|path| !in_history(path));
    additional_picks(results.into_iter().map(|path| FilePick::new(path, workspace)).collect(), query, cache)
}

/// `getAbsolutePathFileResult`: a query naming an existing file by absolute path (`~` for home) is the only result.
pub fn absolute_path_result(query: &PreparedQuery, home: &str) -> Option<PathBuf> {
    if !query.contains_path_separator {
        return None;
    }
    let path = PathBuf::from(untildify(&query.piece.original, home));
    (path.is_absolute() && path.is_file()).then(|| match_filename_casing(&path))
}

/// `getRelativePathFileResults`: a query naming an existing file relative to a workspace folder finds it even
/// when the search excludes it.
pub fn relative_path_results(query: &PreparedQuery, folders: &[&Path]) -> Option<Vec<PathBuf>> {
    if !query.contains_path_separator || Path::new(&query.piece.original).is_absolute() {
        return None;
    }
    let found = folders.iter().map(|folder| folder.join(&query.piece.original)).filter(|path| path.is_file());
    Some(found.map(|path| match_filename_casing(&path)).collect())
}

/// `untildify`: a leading `~` is the home folder.
pub fn untildify(path: &str, home: &str) -> String {
    match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{home}{rest}"),
        _ => path.to_owned(),
    }
}

/// `matchFilenameCasing`: the file name as the folder lists it, since macOS paths ignore case.
pub fn match_filename_casing(path: &Path) -> PathBuf {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else { return path.to_path_buf() };
    let name = name.to_string_lossy().to_lowercase();
    let listed = std::fs::read_dir(parent).ok().and_then(|entries| entries.flatten().find(|entry| entry.file_name().to_string_lossy().to_lowercase() == name));
    listed.map_or_else(|| path.to_path_buf(), |entry| parent.join(entry.file_name()))
}

#[cfg(test)]
mod tests;
