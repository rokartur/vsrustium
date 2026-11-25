//! snippetsService.ts and the snippet completion support of suggest.ts: the snippet files of the extensions, the
//! user's snippets folder and the workspace folders' `.vscode`, read again when they change on disk, and their
//! snippets offered next to the providers' items.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::OpenEditor;
use crate::snippets::{MARK_SNIPPET_AS_USED, Snippet, SnippetFile, SnippetGetOptions, SnippetSource, add_extension_snippet_files, filter_and_sort_snippets, folder_snippet_file, resolve_snippet_completion, snippet_completions};

/// The handle the in-process snippet completion support answers to.
pub(super) const SNIPPET_PROVIDER: u64 = u64::MAX - 2;
const IGNORED_KEY: &str = "snippets.ignoredSnippets";
const USAGE_KEY: &str = "snippets.usageTimestamps";

/// The snippet files by path, with the modification time they were read at.
#[derive(Default)]
pub(super) struct SnippetFiles {
    files: Vec<(PathBuf, Option<SystemTime>, SnippetFile)>,
}

/// A snippets file to read: where, its source, its default scopes and its extension's label and folder.
struct FileSource {
    location: PathBuf,
    source: SnippetSource,
    default_scopes: Option<Vec<String>>,
    extension: Option<(String, PathBuf)>,
}

impl Workbench {
    pub(super) fn register_snippet_completions(&mut self) {
        self.commands.register(MARK_SNIPPET_AS_USED, |workbench, args, _, _| {
            if let Some(id) = args.first().and_then(Value::as_str) {
                workbench.update_snippet_usage(id);
            }
            Outcome::done()
        });
    }

    /// `SnippetUsageTimestamps.updateUsageTimestamp`: the snippet moves to the end with the current time; the last
    /// 100 are kept.
    fn update_snippet_usage(&mut self, id: &str) {
        let mut storage = self.profile_storage.lock().unwrap();
        let mut usages: Vec<(String, i64)> = storage.get(USAGE_KEY).and_then(|raw| serde_json::from_str(raw).ok()).unwrap_or_default();
        usages.retain(|(other, _)| other != id);
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64);
        usages.push((id.to_string(), now));
        let start = usages.len().saturating_sub(100);
        storage.set(USAGE_KEY, json!(usages[start..]).to_string());
    }

    /// The files `_initExtensionSnippets`, `_initUserSnippets` and `_initWorkspaceSnippets` track.
    fn snippet_file_sources(&self) -> Vec<FileSource> {
        let mut sources = Vec::new();
        for extension in &self.extensions {
            let contributions = extension.description["contributes"]["snippets"].as_array().map(Vec::as_slice).unwrap_or_default();
            if contributions.is_empty() {
                continue;
            }
            let description = &extension.description;
            let label = description["displayName"].as_str().filter(|name| !name.is_empty()).or(description["name"].as_str()).unwrap_or_default().to_string();
            let name = description["name"].as_str().unwrap_or_default();
            let mut locations = Vec::new();
            for error in add_extension_snippet_files(&mut locations, name, &extension.location, contributions, |language| self.languages.is_registered(language)) {
                eprintln!("[{}]: {error}", extension.id);
            }
            sources.extend(locations.into_iter().map(|file| FileSource { location: file.location, source: SnippetSource::Extension, default_scopes: file.default_scopes, extension: Some((label.clone(), extension.location.clone())) }));
        }
        let user_folder = self.settings.lock().unwrap().user_home().join("snippets");
        sources.extend(folder_sources(&user_folder, SnippetSource::User));
        for folder in self.search_folders() {
            sources.extend(folder_sources(&folder.join(".vscode"), SnippetSource::Workspace));
        }
        sources
    }

    /// The snippet files, read again where their modification time changed (the file watchers of upstream).
    fn refresh_snippet_files(&mut self) {
        let mut files = Vec::new();
        for source in self.snippet_file_sources() {
            let modified = std::fs::metadata(&source.location).and_then(|metadata| metadata.modified()).ok();
            let known = self.snippet_files.files.iter().position(|(path, time, file)| *path == source.location && *time == modified && file.default_scopes == source.default_scopes);
            if let Some(index) = known {
                files.push(self.snippet_files.files.swap_remove(index));
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&source.location) else { continue };
            let extension = source.extension.as_ref().map(|(label, root)| (label.as_str(), root.as_path()));
            let file = SnippetFile::parse(source.source, &source.location, source.default_scopes, extension, &text);
            files.push((source.location, modified, file));
        }
        self.snippet_files.files = files;
    }

    /// `SnippetCompletionProvider.provideCompletionItems` at the cursor, each item resolved right away (its
    /// documentation is the code block `resolve` adds). Hidden with `editor.snippetSuggestions: none`.
    pub(super) fn snippet_suggestions(&mut self, trigger_character: Option<&str>) -> Value {
        let Some(editor) = self.editors.active() else { return Value::Null };
        if self.suggest_setting(editor, "editor.snippetSuggestions") == json!("none") {
            return Value::Null;
        }
        self.refresh_snippet_files();
        let editor = self.editors.active().expect("checked above");
        let head = editor.selection().head;
        let language = snippet_language(editor, head.line, head.column, &self.languages);
        let storage = self.profile_storage.lock().unwrap();
        let ignored: HashSet<String> = storage.get(IGNORED_KEY).and_then(|raw| serde_json::from_str(raw).ok()).unwrap_or_default();
        let usages: Vec<(String, i64)> = storage.get(USAGE_KEY).and_then(|raw| serde_json::from_str(raw).ok()).unwrap_or_default();
        drop(storage);
        let usages: HashMap<String, i64> = usages.into_iter().collect();
        let mut bucket: Vec<&Snippet> = Vec::new();
        if self.languages.is_registered(&language) {
            for (_, _, file) in &self.snippet_files.files {
                file.select(&language, &mut bucket);
            }
        }
        let path = (editor.document().scheme == "file").then(|| editor.document().path.to_string_lossy().into_owned());
        let snippets = filter_and_sort_snippets(bucket, path.as_deref(), SnippetGetOptions::default(), &ignored, &usages);
        let pairs = self.languages.rules(&language).map(|rules| rules.auto_closing_pairs.clone()).unwrap_or_default();
        let text = &editor.document().lines[head.line];
        let mut list = snippet_completions(text, head.line, head.column, trigger_character, &pairs, &snippets);
        for item in list["b"].as_array_mut().into_iter().flatten() {
            let index = item["x"].take().as_u64().unwrap_or(0) as usize;
            item["d"] = resolve_snippet_completion(snippets[index])["d"].take();
        }
        list
    }
}

/// `_initFolderSnippets`: the snippet files directly in `folder`.
fn folder_sources(folder: &Path, source: SnippetSource) -> Vec<FileSource> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries.filter_map(|entry| entry.ok()).map(|entry| entry.path()).collect();
    paths.sort();
    paths.iter().filter_map(|path| folder_snippet_file(path, source)).map(|file| FileSource { location: file.location, source, default_scopes: file.default_scopes, extension: None }).collect()
}

/// `_getLanguageIdAtPosition`: the embedded language at the cursor when it is a known one, else the model's.
fn snippet_language(editor: &OpenEditor, line: usize, column: usize, languages: &crate::languages::Languages) -> String {
    let document = editor.document();
    let text = &document.lines[line];
    let offset = text.char_indices().nth(column).map_or(text.len(), |(offset, _)| offset);
    let embedded = document.tokens.language_at(&document.lines, line, offset.saturating_sub(1));
    match embedded {
        Some(language) if languages.is_registered(&language) => language.to_string(),
        _ => editor.document().language_id.clone(),
    }
}
