//! Language detection by file path, as `getLanguageIds` does (languagesAssociations.ts) over the
//! `contributes.languages` of the built-in extensions (languagesRegistry.ts `_mergeLanguage`).

use std::collections::HashMap;
use std::path::Path;

use exthost::scan::ScannedExtension;
use serde_json::Value;

use crate::theme::strip_jsonc;

pub const PLAINTEXT: &str = "plaintext";

struct Association {
    id: String,
    filename: Option<String>,
    extension: Option<String>,
    filepattern: Option<String>,
}

pub struct Languages {
    /// In registration order: the core plaintext language first (modesRegistry.ts), then the extensions.
    associations: Vec<Association>,
    names: HashMap<String, String>,
    off_side: HashMap<String, bool>,
}

impl Languages {
    pub fn new(extensions: &[ScannedExtension]) -> Self {
        let mut languages = Languages { associations: Vec::new(), names: HashMap::new(), off_side: HashMap::new() };
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
        // The first alias names the language; a later contribution with aliases renames it, one without keeps the name.
        match strings("aliases").first() {
            Some(alias) => {
                self.names.insert(id.into(), alias.clone());
            }
            None => {
                self.names.entry(id.into()).or_insert_with(|| id.into());
            }
        }
        if let (Some(location), Some(configuration)) = (location, language["configuration"].as_str()) {
            let off_side = std::fs::read_to_string(location.join(configuration))
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&strip_jsonc(&text)).ok())
                .is_some_and(|config| config["folding"]["offSide"].as_bool() == Some(true));
            self.off_side.insert(id.into(), off_side);
        }
    }

    /// The language of a file by its path. No association falls back to plaintext, as the text file
    /// model does with "unknown".
    /// shortcut: no `firstLine` (shebang) detection, add it with a regex crate when a script needs it.
    pub fn language_of(&self, path: &Path) -> &str {
        let path = path.to_string_lossy().to_lowercase();
        let filename = path.rsplit('/').next().unwrap_or(&path);
        let mut pattern_match: Option<&Association> = None;
        let mut extension_match: Option<&Association> = None;
        // Later registrations win, so walk backwards; an exact filename ends the search.
        for association in self.associations.iter().rev() {
            if association.filename.as_ref().is_some_and(|name| name.to_lowercase() == filename) {
                return &association.id;
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
        pattern_match.or(extension_match).map_or(PLAINTEXT, |association| association.id.as_str())
    }

    pub fn name<'a>(&'a self, id: &'a str) -> &'a str {
        self.names.get(id).map_or(id, String::as_str)
    }

    /// `foldingRules.offSide` of the language configuration: blank lines take the indent of the block below.
    pub fn off_side(&self, id: &str) -> bool {
        self.off_side.get(id).copied().unwrap_or(false)
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
            assert_eq!(languages.language_of(Path::new(path)), id, "{path}");
        }
        assert_eq!(languages.name("plaintext"), "Plain Text");
        assert_eq!(languages.name("typescript"), "TypeScript");
        assert!(languages.off_side("python"));
        assert!(!languages.off_side("typescript"));
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
