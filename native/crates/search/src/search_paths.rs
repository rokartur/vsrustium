//! The search view's include and exclude inputs (queryBuilder.ts `parseSearchPaths` and `expandPatterns`):
//! comma separated globs, where `./folder` and absolute paths name the folders to search in.

use std::path::{Component, Path, PathBuf};

use files::glob;
use serde_json::Value;

use crate::query::{Expression, FolderQuery, Query, Setting, escape_exclude_exception_keys, folder_query_for_root, path_included_in_query, text_query};

/// A workspace folder: `./name` in an input names it in a multi-root workspace.
pub struct WorkspaceFolder<'a> {
    pub path: &'a Path,
    pub name: &'a str,
}

/// `ISearchPathsInfo`.
#[derive(Debug, Default, PartialEq)]
pub struct SearchPathsInfo {
    pub search_paths: Vec<SearchPathPattern>,
    pub pattern: Option<Expression>,
}

/// `ISearchPathPattern`.
#[derive(Debug, PartialEq)]
pub struct SearchPathPattern {
    pub search_path: PathBuf,
    pub pattern: Option<Expression>,
}

/// The workspace a search path resolves in.
pub struct Workspace<'a> {
    pub folders: &'a [WorkspaceFolder<'a>],
    /// `WorkbenchState.FOLDER`: relative search paths resolve against the one folder.
    pub single_folder: bool,
    /// `pathService.resolvedUserHome`, for a leading `~`.
    pub home: Option<&'a str>,
}

/// `QueryBuilder.text` with `expandPatterns`, as the search view builds its query from its inputs' text.
pub fn expanded_text_query(pattern: &Value, workspace: &Workspace, include: &str, exclude: &str, options: &Value, setting: Setting) -> Result<Query, String> {
    let paths: Vec<PathBuf> = workspace.folders.iter().map(|folder| folder.path.to_path_buf()).collect();
    let mut query = text_query(pattern, &paths, options, setting);
    let includes = parse_search_paths(&include.replace('\\', "/"), workspace)?;
    let excludes = parse_search_paths(&exclude.replace('\\', "/"), workspace)?;
    let excludes = SearchPathsInfo {
        pattern: excludes.pattern.as_ref().map(escape_exclude_exception_keys),
        search_paths: excludes.search_paths.into_iter().map(|path| SearchPathPattern { pattern: path.pattern.as_ref().map(escape_exclude_exception_keys), ..path }).collect(),
    };
    query.using_search_paths = !includes.search_paths.is_empty();
    let folder_queries = match includes.search_paths.is_empty() {
        true => query.folder_queries,
        false => includes
            .search_paths
            .into_iter()
            .map(|search_path| {
                let mut folder_query = folder_query_for_root(&search_path.search_path, options, setting, false);
                folder_query.include_pattern = search_path.pattern;
                folder_query
            })
            .collect(),
    };
    // getFolderQueryForRoot: an exclude search path without a pattern drops its folder, one with a pattern adds to its excludes.
    query.folder_queries = folder_queries
        .into_iter()
        .filter_map(|mut folder_query| {
            let Some(excluded) = excludes.search_paths.iter().find(|search_path| search_path.search_path == folder_query.folder) else { return Some(folder_query) };
            let pattern = excluded.pattern.clone()?;
            folder_query.exclude_pattern.get_or_insert_with(Expression::new).extend(pattern);
            Some(folder_query)
        })
        .collect();
    query.include_pattern = includes.pattern;
    query.exclude_pattern = excludes.pattern;
    Ok(query)
}

/// `commonQuery` with `onlyOpenEditors` (`commonQueryFromFileList`): only the open editors the query includes are
/// searched, each under its workspace folder or its parent folder.
pub fn restrict_to_open_editors(query: &mut Query, open_editors: &[PathBuf], workspace: &Workspace) {
    let mut folder_queries: Vec<FolderQuery> = Vec::new();
    for file in open_editors.iter().filter(|file| path_included_in_query(query, file)) {
        let workspace_folder = workspace.folders.iter().map(|folder| folder.path).find(|folder| file.starts_with(folder));
        let Some(root) = workspace_folder.or_else(|| file.parent()) else { continue };
        let relative = file.strip_prefix(root).expect("root contains the file").to_string_lossy().replace('\\', "/");
        let index = match folder_queries.iter().position(|folder_query| folder_query.folder == root) {
            Some(index) => index,
            None => {
                folder_queries.push(FolderQuery { folder: root.to_path_buf(), include_pattern: Some(Expression::new()), ..FolderQuery::default() });
                folder_queries.len() - 1
            }
        };
        folder_queries[index].include_pattern.as_mut().expect("set above").insert(escape_glob_pattern(&relative), Value::Bool(true));
    }
    query.exclude_pattern = folder_queries.is_empty().then(|| [("**/*".to_string(), Value::Bool(true))].into_iter().collect());
    query.folder_queries = folder_queries;
    query.include_pattern = Some(Expression::new());
    query.using_search_paths = true;
}

/// `escapeGlobPattern`.
fn escape_glob_pattern(path: &str) -> String {
    let mut escaped = String::new();
    for char in path.chars() {
        match char {
            '?' | '*' | '[' | ']' => escaped.push_str(&format!("[{char}]")),
            char => escaped.push(char),
        }
    }
    escaped
}

/// `parseSearchPaths`: globs become `**/glob` and `**/glob/**`, search paths resolve to folders with a pattern in them.
pub fn parse_search_paths(pattern: &str, workspace: &Workspace) -> Result<SearchPathsInfo, String> {
    let mut search_paths = Vec::new();
    let mut expressions = Vec::new();
    for segment in glob::split_glob_aware(pattern, ',') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let segment = match workspace.home {
            Some(home) => untildify(segment, home),
            None => segment.to_string(),
        };
        if is_search_path(&segment) {
            search_paths.push(segment);
            continue;
        }
        let mut glob = segment.trim_end_matches('/').trim_end_matches('\\').to_string();
        if glob.starts_with('.') {
            glob.insert(0, '*');
        }
        expressions.extend(expand_global_glob(&glob));
    }
    let pattern = (!expressions.is_empty()).then(|| expressions.into_iter().map(|glob| (glob, Value::Bool(true))).collect());
    Ok(SearchPathsInfo { search_paths: expand_search_path_patterns(&search_paths, workspace)?, pattern })
}

/// An absolute path, or one that starts with `./`, `../`, `.\` or `..\`.
fn is_search_path(segment: &str) -> bool {
    if Path::new(segment).is_absolute() {
        return true;
    }
    let rest = segment.strip_prefix("..").or_else(|| segment.strip_prefix('.'));
    rest.is_some_and(|rest| rest.is_empty() || rest.starts_with(['/', '\\']))
}

/// `untildify`: a leading `~` is the user's home.
fn untildify(path: &str, home: &str) -> String {
    match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => format!("{home}{rest}"),
        _ => path.to_string(),
    }
}

/// `expandGlobalGlob`; ripgrep cannot read nested `{}`, so the two globs stay apart (#32761).
fn expand_global_glob(pattern: &str) -> [String; 2] {
    [format!("**/{pattern}/**"), format!("**/{pattern}")].map(|glob| glob.replace("**/**", "**"))
}

/// `expandSearchPathPatterns`: one entry per folder, its patterns merged in the order they came.
fn expand_search_path_patterns(search_paths: &[String], workspace: &Workspace) -> Result<Vec<SearchPathPattern>, String> {
    let mut expanded: Vec<SearchPathPattern> = Vec::new();
    for search_path in search_paths {
        let (path_portion, glob_portion) = split_glob_from_path(search_path);
        let glob_portion = glob_portion.map(|glob| normalize_glob_pattern(&glob));
        for (folder, folder_pattern) in expand_one_search_path(&path_portion, workspace)? {
            for pattern in resolve_one_search_path_pattern(folder_pattern, glob_portion.as_deref()) {
                match expanded.iter_mut().find(|known| known.search_path == folder) {
                    Some(known) => {
                        if let Some(pattern) = pattern {
                            known.pattern.get_or_insert_with(Expression::new).insert(pattern, Value::Bool(true));
                        }
                    }
                    None => expanded.push(SearchPathPattern { search_path: folder.clone(), pattern: pattern.map(|pattern| [(pattern, Value::Bool(true))].into_iter().collect()) }),
                }
            }
        }
    }
    Ok(expanded)
}

/// `expandOneSearchPath`: the folders a search path names, each with the pattern inside it.
fn expand_one_search_path(search_path: &str, workspace: &Workspace) -> Result<Vec<(PathBuf, Option<String>)>, String> {
    if Path::new(search_path).is_absolute() {
        return Ok(vec![(normalize(Path::new(search_path)), None)]);
    }
    if workspace.single_folder {
        let folder = workspace.folders[0].path;
        let search_path = search_path.replace('\\', "/");
        if search_path.starts_with("../") || search_path == ".." {
            return Ok(vec![(normalize(&folder.join(search_path)), None)]);
        }
        let pattern = normalize_glob_pattern(&search_path);
        return Ok(vec![(folder.to_path_buf(), (!pattern.is_empty()).then_some(pattern))]);
    }
    if search_path == "./" || search_path == ".\\" {
        return Ok(Vec::new());
    }
    let without_dot_slash = search_path.strip_prefix("./").or_else(|| search_path.strip_prefix(".\\")).unwrap_or(search_path);
    let matches: Vec<_> = workspace
        .folders
        .iter()
        .filter_map(|folder| {
            let rest = without_dot_slash.strip_prefix(folder.name)?;
            match rest.strip_prefix('/') {
                Some(pattern) => Some((folder.path.to_path_buf(), (!pattern.is_empty()).then(|| normalize_glob_pattern(pattern)))),
                None if rest.is_empty() => Some((folder.path.to_path_buf(), None)),
                None => None,
            }
        })
        .collect();
    if !matches.is_empty() {
        return Ok(matches);
    }
    Err(format!("Workspace folder does not exist: {}", probable_folder_name(search_path)))
}

/// `/\.[\/\\](.+)[\/\\]?/`: everything after the first `./`, the greedy group keeping a trailing slash.
fn probable_folder_name(search_path: &str) -> &str {
    let bytes = search_path.as_bytes();
    (1..bytes.len().saturating_sub(1)).find(|&index| bytes[index - 1] == b'.' && matches!(bytes[index], b'/' | b'\\')).map_or(search_path, |index| &search_path[index + 1..])
}

/// `resolveOneSearchPathPattern`: a pattern also matches everything under it.
fn resolve_one_search_path_pattern(folder_pattern: Option<String>, glob_portion: Option<&str>) -> Vec<Option<String>> {
    let pattern = match (folder_pattern, glob_portion) {
        (Some(folder_pattern), Some(glob)) => Some(format!("{folder_pattern}/{glob}")),
        (folder_pattern, glob) => folder_pattern.or(glob.map(str::to_string)),
    };
    match pattern {
        Some(pattern) if !pattern.ends_with("**") => vec![Some(pattern.clone()), Some(format!("{pattern}/**"))],
        pattern => vec![pattern],
    }
}

/// `splitGlobFromPath`: the path up to the last separator before the first glob character, and the glob after it.
fn split_glob_from_path(search_path: &str) -> (String, Option<String>) {
    let Some(glob_index) = search_path.find(['*', '{', '}', '(', ')', '[', ']', '?']) else { return (search_path.to_string(), None) };
    // `/[/|\\][^/\\]*$/` also takes a `|` as the separator when no slash comes before the glob.
    let before_glob = &search_path[..glob_index];
    let Some(slash) = before_glob.rfind(['/', '\\']).or_else(|| before_glob.find('|')) else { return (search_path.to_string(), None) };
    let mut path_portion = search_path[..slash].to_string();
    if !path_portion.contains(['/', '\\']) {
        path_portion.push('/');
    }
    (path_portion, Some(search_path[slash + 1..].to_string()))
}

/// `normalizeGlobPattern`: slashes, no leading `./`, no trailing slashes.
fn normalize_glob_pattern(pattern: &str) -> String {
    let pattern = pattern.replace('\\', "/");
    pattern.strip_prefix("./").unwrap_or(&pattern).trim_end_matches('/').to_string()
}

/// `path.normalize` of an absolute path: `.` and `..` resolved without touching the disk.
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            component => normalized.push(component),
        }
    }
    normalized
}

#[cfg(test)]
mod tests;
