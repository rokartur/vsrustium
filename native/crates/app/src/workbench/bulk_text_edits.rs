//! The workbench's part of `workspace.applyEdit` (contrib/bulkEdit/browser/bulkTextEdits.ts), and what
//! TextFileEditorModelManager does to the text models when a file operation ran.

use std::path::{Path, PathBuf};

use exthost::main_thread::serialized_error;
use gpui::Context;
use serde_json::{Value, json};

use super::Workbench;
use super::format::more_minimal_edits;
use super::host_requests::{range, snippet_edits, uri_parts};
use crate::editor::{ContentChanged, EditReason, Position, TextDocument, expand_snippet, uri_string};
use crate::local_history;

impl Workbench {
    /// `BulkTextEdits.apply`: each resource's edits are one model change, made through the active editor when it shows
    /// the model; nothing applies when a model moved past the version the host edited.
    pub fn apply_workspace_text_edits(&mut self, edits: &Value, provider_files: &Value, cx: &mut Context<Self>) -> Result<Value, Value> {
        let mut resources: Vec<(&str, PathBuf, Vec<&Value>)> = Vec::new();
        for edit in edits.as_array().into_iter().flatten() {
            let (scheme, path) = uri_parts(&edit["resource"])?;
            match resources.iter_mut().find(|(known_scheme, known_path, _)| *known_scheme == scheme && *known_path == path) {
                Some((_, _, group)) => group.push(edit),
                None => resources.push((scheme, path, vec![edit])),
            }
        }
        let mut opened = false;
        for (scheme, path, _) in &resources {
            if self.editors.model(scheme, path).is_none() {
                let document = self.resolve_document(scheme, path, provider_files)?;
                self.editors.open_model(document);
                opened = true;
            }
        }
        if opened {
            self.editors_changed(cx);
        }
        for (scheme, path, group) in &resources {
            let version_id = self.editors.model(scheme, path).expect("resolved above").version_id;
            if group.iter().any(|edit| edit["versionId"].as_u64().is_some_and(|expected| expected != u64::from(version_id))) {
                return Err(serialized_error(&format!("{} has changed in the meantime", uri_string(scheme, path))));
            }
        }
        let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
        for (scheme, ref path, group) in resources {
            let in_active_editor = self.editors.active().is_some_and(|editor| editor.document.is(scheme, path));
            let model = self.editors.model_mut(scheme, path).expect("resolved above");
            let mut eol = None;
            // `ModelEditTask.addEdit`: (start, end, text, insertAsSnippet, keepWhitespace).
            let mut replaces = Vec::new();
            for edit in group {
                let text_edit = &edit["textEdit"];
                eol = text_edit["eol"].as_u64().or(eol);
                let text = text_edit["text"].as_str().unwrap_or_default();
                let (start, end) = match text_edit["range"].is_null() {
                    true => (Position::new(0, 0), document_end(&model.document.lines)),
                    false => range(model, &text_edit["range"]),
                };
                if (text_edit["range"].is_null() || start == end) && text.is_empty() {
                    continue;
                }
                replaces.push((start, end, text, text_edit["insertAsSnippet"] == json!(true), text_edit["keepWhitespace"] == json!(true), !edit["metadata"].is_null()));
            }
            model.push_undo_stop();
            // `EditorEditTask`: the snippet controller takes every edit when one is a snippet; without the editor a snippet is its text.
            let change = if in_active_editor && replaces.iter().any(|replace| replace.3) {
                let templates: Vec<String> = replaces.iter().map(|&(_, _, text, is_snippet, _, _)| if is_snippet { text.to_string() } else { escape_snippet(text) }).collect();
                let snippets: Vec<_> = replaces.iter().zip(&templates).map(|(&(start, end, _, _, keep_whitespace, _), template)| (start, end, template.as_str(), keep_whitespace)).collect();
                let (edits, selections, _) = snippet_edits(model, &snippets, clipboard.as_deref());
                model.execute_edits(edits, Some(selections), EditReason::Unknown("snippet"))
            } else if in_active_editor {
                // `_createEditsTasks`: the active editor's edits are made more minimal, in runs split by edits with metadata.
                let mut edits = Vec::new();
                let mut run = Vec::new();
                for &(start, end, text, _, _, has_metadata) in &replaces {
                    if has_metadata {
                        edits.extend(more_minimal_edits(&model.document.lines, std::mem::take(&mut run)));
                        edits.push((start, end, text.to_string()));
                    } else {
                        run.push((start, end, text.to_string()));
                    }
                }
                edits.extend(more_minimal_edits(&model.document.lines, run));
                model.execute_edits(edits.into_iter().map(|(start, end, text)| (start, end, text, true)).collect(), None, EditReason::ApplyEdits)
            } else {
                let indentation = model.document.indentation;
                let edits = replaces
                    .iter()
                    .map(|&(start, end, text, is_snippet, _, _)| {
                        let text = if is_snippet { expand_snippet(text, "", false, indentation, |_| None).text } else { text.to_string() };
                        (start, end, text, true)
                    })
                    .collect();
                model.execute_edits(edits, None, EditReason::ApplyEdits)
            };
            let eol_change = eol.and_then(|sequence| model.push_eol(sequence));
            model.push_undo_stop();
            for change in [change, eol_change].iter().flatten() {
                self.send_model_edit(scheme, path, change);
            }
        }
        self.host.send_visible_ranges(&self.editors);
        cx.notify();
        Ok(Value::Null)
    }

    /// `ITextModelService.createModelReference`: a file from disk, or what the session read from an extension's provider.
    fn resolve_document(&self, scheme: &str, path: &Path, provider_files: &Value) -> Result<TextDocument, Value> {
        let settings = self.settings.lock().unwrap();
        if scheme == "untitled" {
            return Ok(TextDocument::untitled(path, "", None, &self.languages, &settings));
        }
        if scheme == "file" {
            return TextDocument::read(path, &self.languages, &settings).map_err(|error| serialized_error(&format!("Unable to read file '{}' ({error})", path.display())));
        }
        let file = provider_files.as_array().into_iter().flatten().find(|file| uri_parts(&file["uri"]).is_ok_and(|(file_scheme, file_path)| file_scheme == scheme && file_path == path));
        let Some(file) = file else {
            return Err(serialized_error(&format!("Unable to resolve resource {}", uri_string(scheme, path))));
        };
        let bytes: Vec<u8> = serde_json::from_value(file["bytes"].clone()).expect("bytes from main_thread_bulk_edits.rs");
        Ok(TextDocument::from_bytes(scheme, path, &bytes, None, &self.languages, &settings))
    }

    /// `onDidRunWorkingCopyFileOperation` of TextFileEditorModelManager: the models of a moved resource and its children
    /// continue at the target with their text and dirty state; the model of a created file reverts to its new contents.
    // shortcut: a delete keeps the model dirty where upstream soft-reverts it, and a model already open at a move's target
    // stays beside the moved one; both wait for the editor's deleted and conflict states.
    pub fn models_after_file_operation(&mut self, operation: u64, pairs: &Value, cx: &mut Context<Self>) {
        let mut changed = false;
        for pair in pairs.as_array().into_iter().flatten() {
            let Ok((scheme, ref target)) = uri_parts(&pair["target"]) else { continue };
            match operation {
                // FileOperation.Create
                0 => {
                    if let Ok(bytes) = serde_json::from_value::<Vec<u8>>(pair["bytes"].clone()) {
                        changed |= self.revert_model(scheme, target, &bytes);
                    }
                }
                // FileOperation.Move
                2 => {
                    if let Ok((source_scheme, ref source)) = uri_parts(&pair["source"]) {
                        changed |= self.move_models(source_scheme, source, scheme, target);
                        self.move_custom_editors(source_scheme, source, scheme, target);
                        if source_scheme == "file" && scheme == "file" {
                            self.move_local_history(source, target, cx);
                        }
                    }
                }
                _ => {}
            }
        }
        if changed {
            self.editors_changed(cx);
        }
        self.resolve_custom_editors(cx);
    }

    fn move_models(&mut self, source_scheme: &str, source: &Path, target_scheme: &str, target: &Path) -> bool {
        let mut moved = false;
        for model in self.editors.editors.iter_mut().chain(&mut self.editors.models) {
            let document = &mut model.document;
            let Some(rest) = document.path.strip_prefix(source).ok().filter(|_| document.scheme == source_scheme) else { continue };
            document.path = if rest.as_os_str().is_empty() { target.to_path_buf() } else { target.join(rest) };
            document.scheme = target_scheme.to_string();
            moved = true;
        }
        moved
    }

    /// workingCopyHistoryTracker.ts `onDidRunWorkingCopyFileOperation` for a move: the history of the moved files
    /// follows them to `target`.
    fn move_local_history(&self, source: &Path, target: &Path, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        let history_home = settings.lock().unwrap().user_home().join("History");
        let (source, target) = (source.to_path_buf(), target.to_path_buf());
        let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64);
        cx.background_executor()
            .spawn(async move {
                let limits = |file: &Path| local_history::limits(&settings.lock().unwrap(), file);
                if let Err(error) = local_history::move_entries(&history_home, &source, &target, timestamp, limits) {
                    eprintln!("local history of {}: {error}", source.display());
                }
            })
            .detach();
    }

    /// `TextFileEditorModel.revert`: the model takes the text of `bytes` and is clean.
    fn revert_model(&mut self, scheme: &str, path: &Path, bytes: &[u8]) -> bool {
        let document = TextDocument::from_bytes(scheme, path, bytes, None, &self.languages, &self.settings.lock().unwrap());
        let Some(model) = self.editors.model_mut(scheme, path) else { return false };
        let end = document_end(&model.document.lines);
        model.push_undo_stop();
        let change = model.execute_edits(vec![(Position::new(0, 0), end, document.lines.join("\n"), false)], None, EditReason::ApplyEdits);
        model.push_undo_stop();
        model.mark_saved();
        if let Some(change) = change {
            self.send_model_edit(scheme, path, &change);
        }
        true
    }

    /// The change of a model edited without the keyboard: with the cursor when the active editor shows it.
    pub(super) fn send_model_edit(&mut self, scheme: &str, path: &Path, change: &ContentChanged) {
        match self.editors.active_index().filter(|&index| self.editors.editors[index].document.is(scheme, path)) {
            Some(index) => self.host.send_content_change(&self.editors, index, change),
            None => self.host.send_model_change(&self.editors, self.editors.model(scheme, path).expect("just edited"), change),
        }
        self.search_model_changed(scheme, path);
    }
}

pub(super) fn document_end(lines: &[String]) -> Position {
    let last = lines.len() - 1;
    Position::new(last, lines[last].chars().count())
}

/// `SnippetParser.escape`: the text inserts as it is.
pub(super) fn escape_snippet(text: &str) -> String {
    text.replace('\\', "\\\\").replace('$', "\\$").replace('}', "\\}")
}
