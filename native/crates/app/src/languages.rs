//! Language detection by file path, as `getLanguageIds` does (languagesAssociations.ts) over the
//! `contributes.languages` of the built-in extensions (languagesRegistry.ts `_mergeLanguage`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use exthost::scan::ScannedExtension;
use serde_json::Value;

use crate::brackets::LanguageBrackets;
use crate::editor::auto_indent::LanguageRules;
use crate::editor::folding::FoldingMarkers;
use crate::theme::strip_jsonc;

pub const PLAINTEXT: &str = "plaintext";

struct Association {
    id: String,
    filename: Option<String>,
    extension: Option<String>,
    filepattern: Option<String>,
}

/// `ICommentsConfiguration`: the line comment token, whether it goes in the first column, the block comment tokens.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Comments<'a> {
    pub line: Option<&'a str>,
    pub line_no_indent: bool,
    pub block: Option<(&'a str, &'a str)>,
}

pub struct Languages {
    /// In registration order: the core plaintext language first (modesRegistry.ts), then the extensions.
    associations: Vec<Association>,
    /// Language ids in the order they were first registered.
    ids: Vec<String>,
    names: HashMap<String, String>,
    off_side: HashMap<String, bool>,
    folding_markers: HashMap<String, Arc<FoldingMarkers>>,
    block_comments: HashMap<String, (String, String)>,
    /// `comments.lineComment`: the token and `noIndent`.
    line_comments: HashMap<String, (String, bool)>,
    /// `firstLine` patterns in registration order, anchored at the line start (languagesRegistry.ts).
    first_lines: Vec<(String, onig::Regex)>,
    pub grammars: textmate::Registry,
    pub brackets: HashMap<String, LanguageBrackets>,
    rules: HashMap<String, LanguageRules>,
}

impl Languages {
    pub fn new(extensions: &[ScannedExtension]) -> Self {
        let grammars = textmate::Registry::new(extensions.iter().map(|extension| (extension.location.as_path(), &extension.description)));
        let mut languages = Languages { associations: Vec::new(), ids: Vec::new(), names: HashMap::new(), off_side: HashMap::new(), folding_markers: HashMap::new(), block_comments: HashMap::new(), line_comments: HashMap::new(), first_lines: Vec::new(), grammars, brackets: HashMap::new(), rules: HashMap::new() };
        languages.register(&serde_json::json!({ "id": PLAINTEXT, "extensions": [".txt"], "aliases": ["Plain Text", "text"] }), None);
        for extension in extensions {
            for language in extension.description["contributes"]["languages"].as_array().into_iter().flatten() {
                languages.register(language, Some(&extension.location));
            }
        }
        languages
    }

    fn register(&mut self, language: &Value, location: Option<&Path>) {
        let Some(id) = language["id"].as_str() else { return };
        let strings = |key: &str| language[key].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
        let association = |filename, extension, filepattern| Association { id: id.into(), filename, extension, filepattern };
        for extension in strings("extensions") {
            self.associations.push(association(None, Some(extension), None));
        }
        for filename in strings("filenames") {
            self.associations.push(association(Some(filename), None, None));
        }
        for pattern in strings("filenamePatterns") {
            self.associations.push(association(None, None, Some(pattern)));
        }
        if !self.ids.iter().any(|known| known == id) {
            self.ids.push(id.into());
        }
        // The first alias names the language and a later contribution with aliases renames it; one without aliases
        // keeps the name, and empty `aliases` leave the language without one.
        match language["aliases"].as_array() {
            Some(aliases) if aliases.is_empty() => {}
            Some(aliases) => {
                let name = aliases[0].as_str().filter(|alias| !alias.is_empty()).unwrap_or(id);
                self.names.insert(id.into(), name.into());
            }
            None => {
                self.names.entry(id.into()).or_insert_with(|| id.into());
            }
        }
        if let Some(first_line) = language["firstLine"].as_str().filter(|pattern| !pattern.is_empty()) {
            let anchored = if first_line.starts_with('^') { first_line.to_string() } else { format!("^{first_line}") };
            match onig::Regex::new(&anchored) {
                Ok(regex) => self.first_lines.push((id.into(), regex)),
                Err(error) => eprintln!("[{id}]: Invalid regular expression `{anchored}`: {error}"),
            }
        }
        if let (Some(location), Some(configuration)) = (location, language["configuration"].as_str()) {
            let config = std::fs::read_to_string(location.join(configuration))
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&strip_jsonc(&text)).ok())
                .unwrap_or_default();
            self.configure(id, &config);
        }
    }

    /// `LanguageConfigurationFileHandler._handleConfig`: the language configuration file of a language.
    pub(crate) fn configure(&mut self, id: &str, config: &Value) {
        self.off_side.insert(id.into(), config["folding"]["offSide"].as_bool() == Some(true));
        if let (Some(start), Some(end)) = (config["comments"]["blockComment"][0].as_str(), config["comments"]["blockComment"][1].as_str()) {
            self.block_comments.insert(id.into(), (start.into(), end.into()));
        }
        // languageConfigurationRegistry.ts: an empty token is no line comment.
        let line_comment = &config["comments"]["lineComment"];
        let token = line_comment.as_str().or_else(|| line_comment["comment"].as_str()).filter(|token| !token.is_empty());
        if let Some(token) = token {
            self.line_comments.insert(id.into(), (token.into(), line_comment["noIndent"].as_bool() == Some(true)));
        }
        let markers = &config["folding"]["markers"];
        if let (Some(start), Some(end)) = (marker_pattern(&markers["start"]), marker_pattern(&markers["end"])) {
            match FoldingMarkers::new(start, end) {
                Ok(markers) => {
                    self.folding_markers.insert(id.into(), Arc::new(markers));
                }
                Err(error) => eprintln!("[{id}]: Invalid regular expression in `folding.markers`: {error}"),
            }
        }
        self.brackets.insert(id.into(), LanguageBrackets::new(id, config));
        self.rules.insert(id.into(), LanguageRules::new(id, config));
    }

    /// The language of a file by its path, else by the `firstLine` pattern its first line matches. No
    /// association falls back to plaintext, as the text file model does with "unknown".
    pub fn language_of<'a>(&'a self, path: &Path, first_line: &str) -> &'a str {
        self.guess_language(path, first_line).unwrap_or(PLAINTEXT)
    }

    /// `guessLanguageIdByFilepathOrFirstLine`: `None` when no association matches.
    pub fn guess_language<'a>(&'a self, path: &Path, first_line: &str) -> Option<&'a str> {
        let path = path.to_string_lossy().to_lowercase();
        let filename = path.rsplit('/').next().unwrap_or(&path);
        let mut pattern_match: Option<&Association> = None;
        let mut extension_match: Option<&Association> = None;
        // Later registrations win, so walk backwards; an exact filename ends the search.
        for association in self.associations.iter().rev() {
            if association.filename.as_ref().is_some_and(|name| name.to_lowercase() == filename) {
                return Some(&association.id);
            }
            if let Some(pattern) = &association.filepattern {
                let longer = pattern_match.is_none_or(|best| pattern.len() > best.filepattern.as_ref().unwrap().len());
                let target = if pattern.contains('/') { path.as_str() } else { filename };
                if longer && glob_matches(&pattern.to_lowercase(), target) {
                    pattern_match = Some(association);
                }
            }
            if let Some(extension) = &association.extension {
                let longer = extension_match.is_none_or(|best| extension.len() > best.extension.as_ref().unwrap().len());
                if longer && filename.ends_with(&extension.to_lowercase()) {
                    extension_match = Some(association);
                }
            }
        }
        if let Some(association) = pattern_match.or(extension_match) {
            return Some(&association.id);
        }
        // getAssociationByFirstline: the last registered pattern wins, over the first 1000 chars (FIRST_LINE_DETECTION_LENGTH_LIMIT).
        let first_line: String = first_line.chars().take(1000).collect();
        let first_line = first_line.strip_prefix('\u{FEFF}').unwrap_or(&first_line);
        self.first_lines.iter().rev().find(|(_, regex)| !first_line.is_empty() && regex.find(first_line).is_some()).map(|(id, _)| id.as_str())
    }

    /// The `files.associations` language of a path (`userConfigured` associations): the longest pattern that
    /// matches, against the whole path when the pattern has a `/`, else the file name, ignoring case.
    pub fn configured_language(associations: &Value, path: &Path) -> Option<String> {
        let path = path.to_string_lossy().to_lowercase();
        let filename = path.rsplit('/').next().unwrap_or(&path);
        let mut best: Option<(&str, &str)> = None;
        for (pattern, id) in associations.as_object()? {
            let Some(id) = id.as_str() else { continue };
            let target = if pattern.contains('/') { path.as_str() } else { filename };
            if best.is_none_or(|(longest, _)| pattern.len() > longest.len()) && glob_matches(&pattern.to_lowercase(), target) {
                best = Some((pattern, id));
            }
        }
        best.map(|(_, id)| id.to_string())
    }

    pub fn is_registered(&self, id: &str) -> bool {
        self.ids.iter().any(|known| known == id)
    }

    pub fn name<'a>(&'a self, id: &'a str) -> &'a str {
        self.names.get(id).map_or(id, String::as_str)
    }

    /// `getSortedRegisteredLanguageNames`: each name with its language, the last language registered under a name,
    /// sorted ignoring case.
    pub fn sorted_names(&self) -> Vec<(&str, &str)> {
        let mut by_name: Vec<(&str, &str)> = Vec::new();
        for id in &self.ids {
            let Some(name) = self.names.get(id) else { continue };
            by_name.retain(|(known, _)| known != name);
            by_name.push((name, id));
        }
        by_name.sort_by_key(|(name, _)| name.to_lowercase());
        by_name
    }

    /// `foldingRules.offSide` of the language configuration: blank lines take the indent of the block below.
    pub fn off_side(&self, id: &str) -> bool {
        self.off_side.get(id).copied().unwrap_or(false)
    }

    /// `foldingRules.markers` of the language configuration.
    pub fn folding_markers(&self, id: &str) -> Option<Arc<FoldingMarkers>> {
        self.folding_markers.get(id).cloned()
    }

    /// `comments.blockComment` start and end tokens of the language configuration.
    pub fn block_comment(&self, id: &str) -> Option<(&str, &str)> {
        self.block_comments.get(id).map(|(start, end)| (start.as_str(), end.as_str()))
    }

    /// The onEnter, indentation and electric character rules of the language configuration.
    pub fn rules(&self, id: &str) -> Option<&LanguageRules> {
        self.rules.get(id)
    }

    /// `getLanguageConfiguration(id).comments`.
    pub fn comments(&self, id: &str) -> Comments<'_> {
        let line = self.line_comments.get(id);
        Comments { line: line.map(|(token, _)| token.as_str()), line_no_indent: line.is_some_and(|(_, no_indent)| *no_indent), block: self.block_comment(id) }
    }
}

/// languageConfigurationExtensionPoint.ts `_parseRegex`: a pattern string, or `{ pattern, flags }`.
pub(crate) fn marker_pattern(value: &Value) -> Option<(&str, &str)> {
    match value {
        Value::String(pattern) => Some((pattern, "")),
        Value::Object(object) => Some((object.get("pattern")?.as_str()?, object.get("flags").and_then(Value::as_str).unwrap_or(""))),
        _ => None,
    }
}

/// The glob forms the built-in `filenamePatterns` use: `*` within a path segment, `**` across segments.
/// shortcut: no `?`, `[...]` or `{a,b}`; built-in extensions use none, user `files.associations` would.
fn glob_matches(pattern: &str, text: &str) -> bool {
    if let Some(rest) = pattern.strip_prefix("**/") {
        return glob_matches(rest, text) || text.char_indices().any(|(i, c)| c == '/' && glob_matches(rest, &text[i + 1..]));
    }
    if let Some(rest) = pattern.strip_prefix("**") {
        return (0..=text.len()).filter(|&i| text.is_char_boundary(i)).any(|i| glob_matches(rest, &text[i..]));
    }
    if let Some(rest) = pattern.strip_prefix('*') {
        let segment_end = text.find('/').unwrap_or(text.len());
        return (0..=segment_end).filter(|&i| text.is_char_boundary(i)).any(|i| glob_matches(rest, &text[i..]));
    }
    match (pattern.chars().next(), text.chars().next()) {
        (None, None) => true,
        (Some(p), Some(t)) if p == t => glob_matches(&pattern[p.len_utf8()..], &text[t.len_utf8()..]),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_languages_like_the_reference() {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let extensions = exthost::scan::scan_builtin_extensions(&app_root.join("extensions")).unwrap();
        let languages = Languages::new(&extensions);
        let cases = [
            ("/w/notes.txt", "plaintext"),
            ("/w/README.md", "markdown"),
            ("/w/src/main.ts", "typescript"),
            ("/w/package.json", "json"),
            ("/w/tsconfig.json", "jsonc"),
            ("/w/tsconfig.base.json", "jsonc"),
            ("/w/Dockerfile", "dockerfile"),
            ("/w/Dockerfile.dev", "dockerfile"),
            ("/w/.git/config", "properties"),
            ("/w/no-extension", "plaintext"),
        ];
        for (path, id) in cases {
            assert_eq!(languages.language_of(Path::new(path), ""), id, "{path}");
        }
        let first_lines = [
            ("/w/run", "#!/usr/bin/env python3", "python"),
            ("/w/run", "#!/bin/bash -e", "shellscript"),
            ("/w/run", "#!/usr/bin/env node", "javascript"),
            ("/w/run", "#!/usr/bin/env -S deno run", "typescript"),
            ("/w/run", "\u{FEFF}<?xml version=\"1.0\"?>", "xml"),
            ("/w/run", "my class Foo {}", "raku"),
            ("/w/run", "hello", "plaintext"),
            ("/w/notes.txt", "#!/bin/bash", "plaintext"),
        ];
        for (path, first_line, id) in first_lines {
            assert_eq!(languages.language_of(Path::new(path), first_line), id, "{first_line}");
        }
        assert_eq!(languages.name("plaintext"), "Plain Text");
        assert_eq!(languages.name("typescript"), "TypeScript");
        assert!(languages.off_side("python"));
        assert!(!languages.off_side("typescript"));
        let css = languages.folding_markers("css").expect("css has #region markers");
        assert_eq!(css.classify("/* #region Colors */"), (true, false));
        assert_eq!(css.classify("  /* #endregion */"), (false, true));
        assert!(languages.folding_markers("plaintext").is_none());
        assert_eq!(languages.block_comment("python"), Some(("\"\"\"", "\"\"\"")));
        assert_eq!(languages.comments("typescript"), Comments { line: Some("//"), line_no_indent: false, block: Some(("/*", "*/")) });
        assert_eq!(languages.comments("plaintext"), Comments::default());
    }

    #[test]
    fn globs_match_within_and_across_segments() {
        assert!(glob_matches("*docker*compose*.yml", "my-docker-compose.yml"));
        assert!(glob_matches("**/.git/config", "/a/b/.git/config"));
        assert!(glob_matches("**/user/snippets/*.json", "/x/user/snippets/a.json"));
        assert!(!glob_matches("**/user/snippets/*.json", "/x/user/snippets/a/b.json"));
        assert!(!glob_matches("tsconfig.*.json", "tsconfig.json"));
    }
}
