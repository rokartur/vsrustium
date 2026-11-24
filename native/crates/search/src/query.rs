//! `QueryBuilder` (workbench/services/search/common/queryBuilder.ts): the file and text queries of the search
//! service, built from the builder options the extension host sends and the folders' settings.

use std::path::{Path, PathBuf};

use files::glob;
use serde_json::{Map, Value};

pub type Expression = Map<String, Value>;

#[derive(Clone, Debug, Default)]
pub struct FolderQuery {
    pub folder: PathBuf,
    /// Set when several folders are searched, so file patterns can match the folder name.
    pub folder_name: Option<String>,
    /// `files.exclude` and `search.exclude` of the folder; upstream keeps one copy per exclude root, all equal.
    pub exclude_pattern: Option<Expression>,
    pub include_pattern: Option<Expression>,
    pub file_encoding: Option<String>,
    pub disregard_ignore_files: bool,
    pub disregard_global_ignore_files: bool,
    pub disregard_parent_ignore_files: bool,
    pub ignore_symlinks: bool,
}

#[derive(Clone, Debug, Default)]
pub struct PatternInfo {
    pub pattern: String,
    pub is_reg_exp: bool,
    pub is_word_match: bool,
    pub is_case_sensitive: bool,
    pub is_multiline: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct PreviewOptions {
    pub match_lines: usize,
    pub chars_per_line: usize,
}

/// `DEFAULT_TEXT_SEARCH_PREVIEW_OPTIONS`.
pub const DEFAULT_PREVIEW_OPTIONS: PreviewOptions = PreviewOptions { match_lines: 100, chars_per_line: 10000 };

#[derive(Clone, Debug, Default)]
pub struct Query {
    pub folder_queries: Vec<FolderQuery>,
    pub include_pattern: Option<Expression>,
    pub exclude_pattern: Option<Expression>,
    pub ignore_glob_case: bool,
    pub max_results: Option<usize>,
    pub file_pattern: Option<String>,
    /// `shouldGlobMatchFilePattern`: the file pattern is a glob, not a fuzzy query.
    pub glob_file_pattern: bool,
    pub exists: bool,
    pub content_pattern: Option<PatternInfo>,
    pub preview_options: Option<PreviewOptions>,
    pub surrounding_context: usize,
    pub max_file_size: Option<u64>,
}

/// Reads a setting for a folder; `Value::Null` when unset.
pub type Setting<'a> = &'a dyn Fn(&Path, &str) -> Value;

/// `QueryBuilder.file`.
pub fn file_query(folders: &[PathBuf], options: &Value, setting: Setting) -> Query {
    let mut query = common_query(folders, options, setting);
    query.file_pattern = options["filePattern"].as_str().map(|pattern| pattern.trim().to_string());
    query.exists = options["exists"] == true;
    query.glob_file_pattern = options["shouldGlobSearch"] == true;
    query
}

/// `QueryBuilder.text`; upstream also copies `editor.wordSeparators` onto the pattern, which ripgrep does not read.
pub fn text_query(pattern: &Value, folders: &[PathBuf], options: &Value, setting: Setting) -> Query {
    let mut query = common_query(folders, options, setting);
    query.content_pattern = Some(content_pattern(pattern, options));
    query.preview_options = options["previewOptions"].as_object().map(|preview| PreviewOptions {
        match_lines: preview["matchLines"].as_u64().unwrap_or_default() as usize,
        chars_per_line: preview["charsPerLine"].as_u64().unwrap_or_default() as usize,
    });
    query.max_file_size = options["maxFileSize"].as_u64();
    query.surrounding_context = options["surroundingContext"].as_u64().unwrap_or_default() as usize;
    query
}

/// `getContentPattern`.
fn content_pattern(input: &Value, options: &Value) -> PatternInfo {
    let flag = |key: &str| input[key] == true;
    let mut pattern = input["pattern"].as_str().unwrap_or_default().to_string();
    let is_reg_exp = flag("isRegExp");
    if is_reg_exp {
        pattern = pattern.replace("\r\n", "\\n").replace('\n', "\\n");
    }
    let smart_case_upper = options["isSmartCase"] == true && contains_uppercase_character(&pattern, is_reg_exp);
    PatternInfo {
        is_case_sensitive: smart_case_upper || flag("isCaseSensitive"),
        is_multiline: flag("isMultiline") || (is_reg_exp && is_multiline_regex_source(&pattern)),
        is_word_match: flag("isWordMatch"),
        is_reg_exp,
        pattern,
    }
}

/// `strings.containsUppercaseCharacter`.
fn contains_uppercase_character(target: &str, ignore_escaped_chars: bool) -> bool {
    let mut text = String::new();
    let mut chars = target.chars();
    while let Some(char) = chars.next() {
        if ignore_escaped_chars && char == '\\' {
            chars.next();
            continue;
        }
        text.push(char);
    }
    text.to_lowercase() != text
}

/// `isMultilineRegexSource` (editor/common/model/textModelSearch.ts).
pub fn is_multiline_regex_source(source: &str) -> bool {
    let mut chars = source.chars();
    while let Some(char) = chars.next() {
        match char {
            '\n' => return true,
            '\\' => match chars.next() {
                Some('n' | 'r' | 'W') => return true,
                Some(_) => {}
                None => break,
            },
            _ => {}
        }
    }
    false
}

/// `commonQuery`, without search paths: the API never sets `expandPatterns`.
fn common_query(folders: &[PathBuf], options: &Value, setting: Setting) -> Query {
    let exclude_patterns: Vec<String> = match &options["excludePattern"] {
        Value::Array(builders) => builders.iter().flat_map(|builder| strings(&builder["pattern"])).collect(),
        pattern => strings(pattern),
    };
    let include_pattern = pattern_list_to_expression(&strings(&options["includePattern"]));
    let exclude_pattern = pattern_list_to_expression(&exclude_patterns).map(|expression| escape_exclude_exception_keys(&expression));
    let include_folder_name = folders.len() > 1;
    Query {
        folder_queries: folders.iter().map(|folder| folder_query_for_root(folder, options, setting, include_folder_name)).collect(),
        include_pattern,
        exclude_pattern,
        ignore_glob_case: options["ignoreGlobCase"] == true,
        max_results: options["maxResults"].as_u64().map(|max| max as usize),
        ..Query::default()
    }
}

/// `handleIncludeExclude` input: a string or a list, empty entries dropped, backslashes made slashes.
fn strings(pattern: &Value) -> Vec<String> {
    let list = match pattern {
        Value::String(pattern) => vec![pattern.as_str()],
        Value::Array(patterns) => patterns.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    list.into_iter().filter(|pattern| !pattern.is_empty()).map(|pattern| pattern.replace('\\', "/")).collect()
}

fn pattern_list_to_expression(patterns: &[String]) -> Option<Expression> {
    (!patterns.is_empty()).then(|| patterns.iter().map(|pattern| (pattern.clone(), Value::Bool(true))).collect())
}

/// `escapeExcludeExceptionKeys`: a `!` in a pattern the caller typed is a literal, not an exception.
fn escape_exclude_exception_keys(expression: &Expression) -> Expression {
    expression.iter().map(|(key, value)| (glob::escape_exclude_exception(key), value.clone())).collect()
}

/// `getFolderQueryForRoot`.
fn folder_query_for_root(folder: &Path, options: &Value, setting: Setting, include_folder_name: bool) -> FolderQuery {
    let flag = |key: &str| options[key].as_bool();
    let search = |key: &str| setting(folder, &format!("search.{key}")) == true;
    let settings_excludes = match options["disregardExcludeSettings"] == true {
        true => None,
        false => excludes(folder, setting, options["disregardSearchExcludeSettings"] != true),
    };
    FolderQuery {
        folder: folder.to_path_buf(),
        folder_name: include_folder_name.then(|| folder.file_name().unwrap_or_default().to_string_lossy().into_owned()),
        exclude_pattern: settings_excludes.filter(|expression| !expression.is_empty()),
        include_pattern: None,
        file_encoding: setting(folder, "files.encoding").as_str().map(str::to_string),
        disregard_ignore_files: flag("disregardIgnoreFiles").unwrap_or_else(|| !search("useIgnoreFiles")),
        disregard_global_ignore_files: flag("disregardGlobalIgnoreFiles").unwrap_or_else(|| !search("useGlobalIgnoreFiles")),
        disregard_parent_ignore_files: flag("disregardParentIgnoreFiles").unwrap_or_else(|| !search("useParentIgnoreFiles")),
        ignore_symlinks: flag("ignoreSymlinks").unwrap_or_else(|| !search("followSymlinks")),
    }
}

/// `getExcludes` (search/common/search.ts): `files.exclude` overlaid with `search.exclude`.
fn excludes(folder: &Path, setting: Setting, include_search_excludes: bool) -> Option<Expression> {
    let file_excludes = setting(folder, "files.exclude").as_object().cloned();
    let search_excludes = include_search_excludes.then(|| setting(folder, "search.exclude").as_object().cloned()).flatten();
    match (file_excludes, search_excludes) {
        (Some(mut all), Some(search)) => {
            all.extend(search);
            Some(all)
        }
        (file, search) => file.or(search),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn settings(_: &Path, key: &str) -> Value {
        match key {
            "files.exclude" => json!({ "**/.git": true, "**/files-exclude/**": true }),
            "search.exclude" => json!({ "**/node_modules": true, "**/.git": false }),
            "search.useIgnoreFiles" | "search.followSymlinks" => json!(true),
            _ => Value::Null,
        }
    }

    #[test]
    fn file_query_merges_setting_excludes_and_keeps_the_callers_apart() {
        let options = json!({ "includePattern": "**/image.png", "excludePattern": [{ "pattern": "**/sub/**" }, { "pattern": "!x" }], "maxResults": 5 });
        let query = file_query(&[PathBuf::from("/w")], &options, &settings);
        assert_eq!(query.include_pattern, Some(json!({ "**/image.png": true }).as_object().unwrap().clone()));
        assert_eq!(query.exclude_pattern, Some(json!({ "**/sub/**": true, "{!}x": true }).as_object().unwrap().clone()));
        assert_eq!(query.max_results, Some(5));
        let folder = &query.folder_queries[0];
        assert_eq!(folder.exclude_pattern, Some(json!({ "**/.git": false, "**/files-exclude/**": true, "**/node_modules": true }).as_object().unwrap().clone()));
        assert!(!folder.disregard_ignore_files && folder.disregard_parent_ignore_files && !folder.ignore_symlinks);
        assert_eq!(folder.folder_name, None);
    }

    #[test]
    fn options_override_settings() {
        let options = json!({ "disregardSearchExcludeSettings": true, "disregardIgnoreFiles": true, "ignoreSymlinks": true });
        let query = file_query(&[PathBuf::from("/a"), PathBuf::from("/b")], &options, &settings);
        let folder = &query.folder_queries[1];
        assert_eq!(folder.exclude_pattern, Some(json!({ "**/.git": true, "**/files-exclude/**": true }).as_object().unwrap().clone()));
        assert!(folder.disregard_ignore_files && folder.ignore_symlinks);
        assert_eq!(folder.folder_name.as_deref(), Some("b"));
        let query = file_query(&[PathBuf::from("/a")], &json!({ "disregardExcludeSettings": true }), &settings);
        assert_eq!(query.folder_queries[0].exclude_pattern, None);
    }

    #[test]
    fn content_pattern_follows_smart_case_and_multiline() {
        let pattern = content_pattern(&json!({ "pattern": "Foo", "isRegExp": false }), &json!({ "isSmartCase": true }));
        assert!(pattern.is_case_sensitive && !pattern.is_multiline);
        let pattern = content_pattern(&json!({ "pattern": "\\Woo", "isRegExp": true }), &json!({ "isSmartCase": true }));
        assert!(!pattern.is_case_sensitive && pattern.is_multiline);
        let pattern = content_pattern(&json!({ "pattern": "a\nb", "isRegExp": true }), &json!({}));
        assert_eq!(pattern.pattern, "a\\nb");
        assert!(pattern.is_multiline);
    }
}
