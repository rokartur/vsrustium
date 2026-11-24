//! `FileWalker` (workbench/services/search/node/fileSearch.ts, ripgrepFileSearch.ts): lists a folder's files with
//! `rg --files`, then applies the query's file pattern, includes and the excludes ripgrep cannot express (sibling
//! clauses, `!` exceptions, absolute patterns).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use files::glob::{self, ExcludeExpression, ExcludeTreeEntry, ExcludeTreeEvaluator, Glob, Options};
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

use crate::query::{Expression, FolderQuery, Query};
use crate::ripgrep::{self, Cancellation, anchor_glob};

/// `fileSearch.ts` `Engine.search` for one query: reports each matching file as its folder and relative path. Returns
/// whether the result limit was hit.
pub fn search(query: &Query, rg: &Path, cancellation: &Cancellation, on_result: &mut dyn FnMut(&Path, &str)) -> Result<bool, String> {
    let mut walker = FileWalker::new(query);
    for folder_query in &query.folder_queries {
        if cancellation.is_cancelled() || walker.is_limit_hit {
            break;
        }
        walker.folder_traversal(folder_query, rg, cancellation, on_result)?;
    }
    Ok(walker.is_limit_hit)
}

struct FileWalker<'a> {
    query: &'a Query,
    file_pattern: String,
    normalized_file_pattern_lowercase: Option<String>,
    include_pattern: Option<Glob>,
    max_results: Option<usize>,
    result_count: usize,
    is_limit_hit: bool,
    folder_excludes: HashMap<PathBuf, FolderExclude>,
    nested_root_folders: HashMap<PathBuf, Vec<String>>,
}

impl<'a> FileWalker<'a> {
    fn new(query: &'a Query) -> Self {
        let options = Options { ignore_case: query.ignore_glob_case, ..Options::default() };
        let file_pattern = query.file_pattern.clone().unwrap_or_default();
        let normalized_file_pattern_lowercase = (!file_pattern.is_empty() && !query.glob_file_pattern).then(|| prepare_query_lowercase(&file_pattern));
        let mut folder_excludes = HashMap::new();
        let mut nested_root_folders = HashMap::new();
        for folder_query in &query.folder_queries {
            let mut expression = folder_query.exclude_pattern.clone().unwrap_or_default();
            expression.extend(query.exclude_pattern.clone().unwrap_or_default());
            let root = &folder_query.folder;
            let mut nested = Vec::new();
            for other in &query.folder_queries {
                if &other.folder != root && is_equal_or_parent(&other.folder, root, query.ignore_glob_case) {
                    let relative = other.folder.strip_prefix(root).unwrap_or(&other.folder).to_string_lossy().into_owned();
                    expression.insert(relative.clone(), Value::Bool(true));
                    nested.push(relative);
                }
            }
            nested_root_folders.insert(root.clone(), nested);
            folder_excludes.insert(root.clone(), FolderExclude::new(expression, root, query.ignore_glob_case));
        }
        FileWalker {
            query,
            file_pattern,
            normalized_file_pattern_lowercase,
            include_pattern: query.include_pattern.as_ref().map(|include| glob::parse_expression(include, options)),
            max_results: query.max_results.filter(|max| *max > 0),
            result_count: 0,
            is_limit_hit: false,
            folder_excludes,
            nested_root_folders,
        }
    }

    /// `folderTraversal`: ripgrep does not descend into excluded folders, so a second run lists what `!` exceptions
    /// re-include and skips what the first run reported.
    fn folder_traversal(&mut self, folder_query: &FolderQuery, rg: &Path, cancellation: &Cancellation, on_result: &mut dyn FnMut(&Path, &str)) -> Result<(), String> {
        let exceptions = self.folder_excludes[&folder_query.folder].exceptions().to_vec();
        if exceptions.is_empty() {
            return self.cmd_traversal(folder_query, rg, cancellation, on_result, None, None);
        }
        let mut seen_paths = HashSet::new();
        self.cmd_traversal(folder_query, rg, cancellation, on_result, Some(&mut seen_paths), None)?;
        if cancellation.is_cancelled() || self.is_limit_hit {
            return Ok(());
        }
        self.cmd_traversal(folder_query, rg, cancellation, on_result, Some(&mut seen_paths), Some(&exceptions))
    }

    // shortcut: no second run without --follow when ripgrep reports symbolic link loops (RipgrepSymlinkLoopDetector);
    // port it once a workspace with looping links needs it.
    fn cmd_traversal(
        &mut self,
        folder_query: &FolderQuery,
        rg: &Path,
        cancellation: &Cancellation,
        on_result: &mut dyn FnMut(&Path, &str),
        mut seen_paths: Option<&mut HashSet<String>>,
        exclude_exceptions: Option<&[String]>,
    ) -> Result<(), String> {
        let root = &folder_query.folder;
        let exclude = &self.folder_excludes[root];
        let (args, sibling_clauses) = rg_args(self.query, folder_query, exclude.expression(), exclude_exceptions);
        let reincluded_filter = exclude_exceptions.map(|_| self.exception_run_filter(folder_query));
        let mut relative_files = Vec::new();
        let finished = ripgrep::run(rg, &args, root, cancellation, &mut |line| {
            let relative_path: String = line.nfc().collect();
            if relative_path.is_empty() {
                return true;
            }
            if let Some(reincluded) = &reincluded_filter
                && (seen_paths.as_ref().is_some_and(|seen| seen.contains(&relative_path)) || !reincluded(&relative_path))
            {
                return true;
            }
            if let Some(seen) = seen_paths.as_mut()
                && (exclude_exceptions.is_some() || self.folder_excludes[root].is_excepted(&relative_path, None))
            {
                seen.insert(relative_path.clone());
            }
            if !sibling_clauses.is_empty() {
                relative_files.push(relative_path);
                return true;
            }
            self.match_file(on_result, root, &self.search_path(folder_query, &relative_path), &relative_path);
            !self.is_limit_hit
        })
        .map_err(|error| error.to_string())?;
        if !finished.got_data && !finished.stderr.is_empty() && rg_error_message_for_display(&finished.stderr).is_some() {
            return Err(format!("command failed: {}", finished.stderr));
        }
        if self.query.exists && exclude_exceptions.is_none() && finished.success {
            self.is_limit_hit = true;
        }
        if !sibling_clauses.is_empty() {
            self.match_directory_tree(folder_query, &relative_files, on_result);
        }
        Ok(())
    }

    /// `createExceptionRunFilter`: an exception run passes no includes to ripgrep and skips nested workspace folders here.
    fn exception_run_filter(&self, folder_query: &FolderQuery) -> impl Fn(&str) -> bool + use<> {
        let exclude = self.folder_excludes[&folder_query.folder].clone();
        let include = match (&self.query.include_pattern, &folder_query.include_pattern) {
            (None, None) => None,
            (global, folder) => {
                let mut expression = global.clone().unwrap_or_default();
                expression.extend(folder.clone().unwrap_or_default());
                Some(glob::parse_expression(&expression, Options { ignore_case: self.query.ignore_glob_case, ..Options::default() }))
            }
        };
        let nested = self.nested_root_folders[&folder_query.folder].clone();
        let ignore_case = self.query.ignore_glob_case;
        move |relative_path| {
            exclude.is_excepted(relative_path, None)
                && include.as_ref().is_none_or(|include| include.matches(relative_path, Some(basename(relative_path)), None).is_some())
                && !nested.iter().any(|folder| is_equal_or_parent(Path::new(relative_path), Path::new(folder), ignore_case))
        }
    }

    /// `addDirectoryEntries` and `matchDirectoryTree`: sibling clauses need each file's siblings, so the listing is
    /// gathered into a tree and walked once ripgrep is done.
    fn match_directory_tree(&mut self, folder_query: &FolderQuery, relative_files: &[String], on_result: &mut dyn FnMut(&Path, &str)) {
        let root = &folder_query.folder;
        let ignore_case = self.query.ignore_glob_case;
        if !self.file_pattern.is_empty()
            && let Some(exact) = relative_files.iter().find(|file| strings_equal(file, &self.file_pattern, ignore_case))
        {
            self.match_file(on_result, root, &self.search_path(folder_query, exact), exact);
        }
        let mut path_to_entries: HashMap<String, Vec<String>> = HashMap::new();
        path_to_entries.insert(".".to_string(), Vec::new());
        for relative_path in relative_files {
            add_entry(&mut path_to_entries, relative_path);
        }
        self.match_directory(folder_query, &path_to_entries, ".", false, on_result);
    }

    fn match_directory(&mut self, folder_query: &FolderQuery, tree: &HashMap<String, Vec<String>>, directory: &str, is_parent_excluded: bool, on_result: &mut dyn FnMut(&Path, &str)) {
        let entries = &tree[directory];
        let basenames: HashSet<&str> = entries.iter().map(|entry| basename(entry)).collect();
        let has_sibling = |name: &str| basenames.contains(name);
        let ignore_case = self.query.ignore_glob_case;
        for relative_path in entries {
            let name = basename(relative_path);
            let sub = tree.contains_key(relative_path.as_str());
            let exclude = &self.folder_excludes[&folder_query.folder];
            let evaluation = glob::evaluate_exclude_tree_entry(
                exclude,
                &ExcludeTreeEntry {
                    path: relative_path,
                    basename: Some(name),
                    is_directory: sub,
                    is_parent_excluded,
                    is_hidden: false,
                    // A file the user typed by name is reported even when a sibling clause excludes it.
                    has_sibling: (!strings_equal(&self.file_pattern, name, ignore_case)).then_some(&has_sibling as &dyn Fn(&str) -> bool),
                },
            );
            if !evaluation.is_visited {
                continue;
            }
            if sub {
                self.match_directory(folder_query, tree, relative_path, evaluation.is_excluded, on_result);
            } else if !strings_equal(relative_path, &self.file_pattern, ignore_case) {
                self.match_file(on_result, &folder_query.folder, &self.search_path(folder_query, relative_path), relative_path);
            }
            if self.is_limit_hit {
                break;
            }
        }
    }

    fn match_file(&mut self, on_result: &mut dyn FnMut(&Path, &str), base: &Path, search_path: &str, relative_path: &str) {
        let included = self.include_pattern.as_ref().is_none_or(|include| include.matches(relative_path, Some(basename(relative_path)), None).is_some());
        if !self.is_file_match(search_path) || !included {
            return;
        }
        self.result_count += 1;
        if self.query.exists || self.max_results.is_some_and(|max| self.result_count > max) {
            self.is_limit_hit = true;
        }
        if !self.is_limit_hit {
            on_result(base, relative_path);
        }
    }

    /// `isFileMatch` with `isFilePatternMatch`: a fuzzy query by default, a glob for `findFiles2`.
    fn is_file_match(&self, search_path: &str) -> bool {
        if self.file_pattern.is_empty() || self.file_pattern == "*" {
            return true;
        }
        match &self.normalized_file_pattern_lowercase {
            Some(query) => fuzzy_contains(search_path, query),
            None => glob::match_pattern(&self.file_pattern, search_path, Options { ignore_case: self.query.ignore_glob_case, ..Options::default() }),
        }
    }

    /// `getSearchPath`: with several folders the folder name leads, so patterns can tell `a/package.json` from `b/package.json`.
    fn search_path(&self, folder_query: &FolderQuery, relative_path: &str) -> String {
        match &folder_query.folder_name {
            Some(name) => format!("{name}/{relative_path}"),
            None => relative_path.to_string(),
        }
    }
}

fn add_entry(tree: &mut HashMap<String, Vec<String>>, relative_path: &str) {
    let directory = match relative_path.rfind('/') {
        Some(slash) => relative_path[..slash].to_string(),
        None => ".".to_string(),
    };
    if !tree.contains_key(&directory) {
        tree.insert(directory.clone(), Vec::new());
        add_entry(tree, &directory);
    }
    tree.get_mut(&directory).expect("inserted above").push(relative_path.to_string());
}

/// `getRgArgs` (ripgrepFileSearch.ts) and the sibling clauses ripgrep cannot evaluate.
fn rg_args(query: &Query, folder_query: &FolderQuery, exclude: &Expression, exclude_exceptions: Option<&[String]>) -> (Vec<String>, Expression) {
    let mut args: Vec<String> = ["--files", "--hidden", "--case-sensitive", "--no-require-git"].map(String::from).to_vec();
    if query.ignore_glob_case {
        args.extend(["--glob-case-insensitive", "--ignore-file-case-insensitive"].map(String::from));
    }
    let mut push_glob = |glob: String| {
        // macOS file names may be stored decomposed, so the glob goes in both forms.
        let decomposed: String = glob.nfd().collect();
        let differs = decomposed != glob;
        args.extend(["-g".to_string(), glob]);
        if differs {
            args.extend(["-g".to_string(), decomposed]);
        }
    };
    let mut sibling_clauses = Expression::new();
    match exclude_exceptions {
        Some(exceptions) => ripgrep::exclude_exception_globs(exceptions).into_iter().for_each(&mut push_glob),
        None => {
            let mut include = query.include_pattern.clone().unwrap_or_default();
            include.extend(folder_query.include_pattern.clone().unwrap_or_default());
            for (key, value) in &include {
                if *value == true && !key.is_empty() {
                    push_glob(anchor_glob(&rg_glob_key(key)));
                }
            }
            for (key, value) in glob::without_exclude_exceptions(exclude) {
                if key.is_empty() {
                    continue;
                }
                let key = rg_glob_key(&key);
                if value == true {
                    push_glob(format!("!{}", anchor_glob(&key)));
                } else if value.get("when").is_some() {
                    sibling_clauses.insert(key, value);
                }
            }
        }
    }
    if exclude_exceptions.is_some() || folder_query.disregard_ignore_files {
        args.push("--no-ignore".into());
    } else if folder_query.disregard_parent_ignore_files {
        args.push("--no-ignore-parent".into());
    }
    if !folder_query.ignore_symlinks {
        args.push("--follow".into());
    }
    if query.exists && exclude_exceptions.is_none() {
        args.push("--quiet".into());
    }
    args.push("--no-config".into());
    if folder_query.disregard_global_ignore_files {
        args.push("--no-ignore-global".into());
    }
    (args, sibling_clauses)
}

/// `globExprsToRgGlobs` without a folder: trailing slashes trimmed, backslashes made slashes.
fn rg_glob_key(key: &str) -> String {
    key.trim_end_matches(['\\', '/']).replace('\\', "/")
}

/// `AbsoluteAndRelativeParsedExpression`: absolute patterns match the full path, the rest the folder-relative one.
#[derive(Clone)]
struct FolderExclude {
    expression: Expression,
    root: PathBuf,
    absolute: Option<Glob>,
    relative: Option<ExcludeExpression>,
}

impl FolderExclude {
    fn new(expression: Expression, root: &Path, ignore_case: bool) -> Self {
        let mut absolute = Expression::new();
        let mut relative = Expression::new();
        for (key, value) in &expression {
            if is_truthy(value) {
                let target = if key.starts_with('/') { &mut absolute } else { &mut relative };
                target.insert(key.clone(), value.clone());
            }
        }
        let options = Options { trim_for_exclusions: true, ignore_case };
        FolderExclude {
            root: root.to_path_buf(),
            absolute: (!absolute.is_empty()).then(|| glob::parse_expression(&absolute, options)),
            relative: (!relative.is_empty()).then(|| glob::parse_exclude_expression(&relative, options)),
            expression,
        }
    }

    fn expression(&self) -> &Expression {
        &self.expression
    }

    fn exceptions(&self) -> &[String] {
        self.relative.as_ref().map(|relative| relative.exceptions.as_slice()).unwrap_or_default()
    }
}

impl ExcludeTreeEvaluator for FolderExclude {
    fn matches_exclude(&self, path: &str, basename: Option<&str>, has_sibling: Option<&dyn Fn(&str) -> bool>) -> bool {
        self.relative.as_ref().is_some_and(|relative| relative.matches_exclude(path, basename, has_sibling))
            || self.absolute.as_ref().is_some_and(|absolute| absolute.matches(&self.root.join(path).to_string_lossy(), basename, has_sibling).is_some())
    }

    fn is_excepted(&self, path: &str, basename: Option<&str>) -> bool {
        self.relative.as_ref().is_some_and(|relative| relative.is_excepted(path, basename))
    }

    fn could_exception_match_beneath(&self, folder_path: &str) -> bool {
        self.relative.as_ref().is_some_and(|relative| relative.could_exception_match_beneath(folder_path))
    }
}

/// JavaScript truthiness of an expression value: `false`, `null`, `0` and `""` turn a pattern off.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::Number(number) => number.as_f64() != Some(0.0),
        Value::String(text) => !text.is_empty(),
        _ => true,
    }
}

/// `rgErrorMsgForDisplay` (fileSearch.ts): stderr that means the search failed, not a warning.
fn rg_error_message_for_display(message: &str) -> Option<String> {
    let lines: Vec<&str> = message.trim().split('\n').collect();
    let first = lines[0].trim();
    let upper_first = |text: &str| {
        let mut chars = text.chars();
        chars.next().map(|first| first.to_uppercase().chain(chars).collect::<String>()).unwrap_or_default()
    };
    if first.starts_with("Error parsing regex") || first.starts_with("Literal ") && first != "Literal '\\n' not allowed." {
        return Some(first.to_string());
    }
    if first.starts_with("regex parse error") {
        return Some(upper_first(lines[lines.len() - 1].trim()));
    }
    if first.starts_with("error parsing glob") || first.starts_with("unsupported encoding") {
        return Some(upper_first(first));
    }
    (first == "Literal '\\n' not allowed.").then(|| "Literal '\\n' currently not supported".to_string())
}

/// `prepareQuery(...).normalizedLowercase`: quotes, wildcards, whitespace, ellipses and a trailing `#` dropped.
fn prepare_query_lowercase(original: &str) -> String {
    let mut normalized: String = original.replace('\\', "/").chars().filter(|char| !matches!(char, '*' | '\u{2026}' | '"') && !char.is_whitespace()).collect();
    if normalized.chars().count() > 1 && normalized.ends_with('#') {
        normalized.pop();
    }
    normalized.to_lowercase()
}

/// `strings.fuzzyContains`: the query's characters appear in the target in order.
fn fuzzy_contains(target: &str, query: &str) -> bool {
    if target.is_empty() || query.is_empty() {
        return false;
    }
    let target = target.to_lowercase();
    let mut rest = target.as_str();
    for char in query.to_lowercase().chars() {
        match rest.find(char) {
            Some(index) => rest = &rest[index + char.len_utf8()..],
            None => return false,
        }
    }
    true
}

fn strings_equal(a: &str, b: &str, ignore_case: bool) -> bool {
    if ignore_case { a.to_lowercase() == b.to_lowercase() } else { a == b }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `extpath.isEqualOrParent`.
fn is_equal_or_parent(path: &Path, candidate: &Path, ignore_case: bool) -> bool {
    if ignore_case {
        let lower = |path: &Path| PathBuf::from(path.to_string_lossy().to_lowercase());
        return lower(path).starts_with(lower(candidate));
    }
    path.starts_with(candidate)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn expression(value: Value) -> Expression {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn rg_args_anchor_includes_and_excludes_and_keep_sibling_clauses() {
        let query = Query { include_pattern: Some(expression(json!({ "src/**": true, "**/*.ts": true }))), ..Query::default() };
        let folder = FolderQuery { folder: "/w".into(), ..FolderQuery::default() };
        let exclude = expression(json!({ "**/node_modules": true, "out/": true, "**/*.js": { "when": "$(basename).ts" }, "!keep": true, "off": false }));
        let (args, siblings) = rg_args(&query, &folder, &exclude, None);
        assert_eq!(
            args,
            [
                "--files", "--hidden", "--case-sensitive", "--no-require-git", "-g", "/src/**", "-g", "**/*.ts", "-g", "!**/node_modules", "-g",
                "!/out", "--follow", "--no-config"
            ]
        );
        assert_eq!(siblings, expression(json!({ "**/*.js": { "when": "$(basename).ts" } })));
    }

    #[test]
    fn rg_args_of_an_exception_run_skip_ignore_files() {
        let folder = FolderQuery { folder: "/w".into(), ignore_symlinks: true, ..FolderQuery::default() };
        let (args, _) = rg_args(&Query::default(), &folder, &Expression::new(), Some(&["**/keep".to_string()]));
        assert_eq!(args[4..], ["-g", "**/keep", "-g", "**/keep/**", "-g", "!**/.git", "-g", "!**/.svn", "-g", "!**/.hg", "-g", "!**/.jj", "--no-ignore", "--no-config"]);
    }

    #[test]
    fn fuzzy_file_pattern() {
        assert!(fuzzy_contains("src/vs/main.ts", "svmain"));
        assert!(!fuzzy_contains("src/main.ts", "mainx"));
        assert_eq!(prepare_query_lowercase("Ma*in \"x\"#"), "mainx");
    }

    #[test]
    fn directory_tree_adds_parent_folders() {
        let mut tree = HashMap::new();
        tree.insert(".".to_string(), Vec::new());
        add_entry(&mut tree, "a/b/c.ts");
        add_entry(&mut tree, "d.ts");
        assert_eq!(tree["."], ["a", "d.ts"]);
        assert_eq!(tree["a"], ["a/b"]);
        assert_eq!(tree["a/b"], ["a/b/c.ts"]);
    }
}
