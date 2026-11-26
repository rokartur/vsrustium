//! The MainThread* requests the workbench answers itself: mainThreadDocuments.ts, mainThreadEditors.ts with
//! mainThreadEditor.ts, and mainThreadClipboard.ts. They arrive from the session thread through `main_thread.rs`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use config::indentation::creation_options;
use exthost::main_thread::serialized_error;
use exthost::main_thread_files::resource;
use exthost::settings_file_system;
use files::encoding;
use gpui::{ClipboardItem, Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::auto_save::SaveReason;
use super::bulk_text_edits::document_end;
use super::snippet_controller::SnippetInsert;
use crate::editor::{
    CursorEdit, EditReason, OpenEditor, Position, Replace, Selection, SnippetText, TextDocument, VerticalReveal, char_column, end_of, expand_snippet,
    selections_after, text_between, uri_string, word_at,
};
use crate::commands::Outcome;
use crate::host_sync::snapshot_id;

/// The requests `host_request` answers; every other one stays with the session.
pub const HOST_REQUESTS: &[(&str, &str)] = &[
    ("MainThreadClipboard", "$readText"),
    ("MainThreadClipboard", "$writeText"),
    ("MainThreadDocuments", "$tryOpenDocument"),
    ("MainThreadDocuments", "$trySaveDocument"),
    ("MainThreadDocuments", "$tryCreateDocument"),
    ("MainThreadDocumentContentProviders", "$onVirtualDocumentChange"),
    ("MainThreadDocumentDiff", "$computeDocumentDiff"),
    ("MainThreadLanguages", "$changeLanguage"),
    ("MainThreadLanguages", "$tokensAtPosition"),
    ("MainThreadLanguages", "$computeFullSyntaxHighlighting"),
    ("MainThreadTextEditors", "$tryShowTextDocument"),
    ("MainThreadTextEditors", "$trySetSelections"),
    ("MainThreadTextEditors", "$tryRevealRange"),
    ("MainThreadTextEditors", "$trySetOptions"),
    ("MainThreadTextEditors", "$tryApplyEdits"),
    ("MainThreadTextEditors", "$tryInsertSnippet"),
    ("MainThreadTextEditors", "$registerTextEditorDecorationType"),
    ("MainThreadTextEditors", "$removeTextEditorDecorationType"),
    ("MainThreadTextEditors", "$trySetDecorations"),
    ("MainThreadTextEditors", "$trySetDecorationsFast"),
    ("MainThreadPower", "$getSystemIdleState"),
    ("MainThreadPower", "$getSystemIdleTime"),
    ("MainThreadPower", "$getCurrentThermalState"),
    ("MainThreadPower", "$isOnBatteryPower"),
    ("MainThreadPower", "$startPowerSaveBlocker"),
    ("MainThreadPower", "$stopPowerSaveBlocker"),
    ("MainThreadPower", "$isPowerSaveBlockerStarted"),
];

#[cfg(test)]
mod tests;

type HostResult = Result<Value, Value>;

impl Workbench {
    /// Answers one of `HOST_REQUESTS`: `Ok` is the reply, `Err` a `SerializedError`.
    pub fn host_request(&mut self, proxy: &str, method: &str, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> HostResult {
        let null = Value::Null;
        let arg = |index: usize| args.get(index).unwrap_or(&null);
        self.release_host_models(cx);
        match (proxy, method) {
            ("MainThreadClipboard", "$readText") => Ok(json!(cx.read_from_clipboard().and_then(|item| item.text()).unwrap_or_default())),
            ("MainThreadClipboard", "$writeText") => {
                cx.write_to_clipboard(ClipboardItem::new_string(arg(0).as_str().unwrap_or_default().to_string()));
                Ok(null)
            }
            ("MainThreadPower", method) => crate::power::request(method, args).map_err(|error| serialized_error(&error)),
            ("MainThreadDocuments", "$tryOpenDocument") => self.try_open_document(arg(0), arg(1), arg(2), arg(3) == true, cx),
            ("MainThreadDocuments", "$trySaveDocument") => self.try_save_document(arg(0), cx),
            ("MainThreadDocuments", "$tryCreateDocument") => self.try_create_document(arg(0), cx),
            ("workbench", "isDirty") => uri_parts(arg(0)).map(|(scheme, path)| json!(self.editors.model(scheme, &path).is_some_and(|model| model.is_dirty()))),
            ("workbench", "hasModel") => uri_parts(arg(0)).map(|(scheme, path)| json!(self.editors.model(scheme, &path).is_some())),
            ("MainThreadDocumentContentProviders", "$onVirtualDocumentChange") => self.virtual_document_change(arg(0), arg(1), cx),
            ("MainThreadDocumentDiff", "$computeDocumentDiff") => Ok(self.compute_document_diff(arg(0), arg(1), arg(2) == true, arg(3).as_u64().unwrap_or(0), arg(4) == true)),
            ("MainThreadLanguages", "$changeLanguage") => self.change_host_language(arg(0), arg(1), cx),
            ("MainThreadLanguages", "$tokensAtPosition") => uri_parts(arg(0)).map(|(scheme, path)| self.tokens_at_position(scheme, &path, arg(1))),
            ("MainThreadLanguages", "$computeFullSyntaxHighlighting") => Ok(self.full_syntax_highlighting(arg(0).as_str().unwrap_or_default(), arg(1).as_str().unwrap_or_default())),
            ("workbench", "providerSaveText") => self.provider_save_bytes(arg(0)),
            ("workbench", "providerSaved") => self.provider_saved(arg(0), cx),
            ("MainThreadTextEditors", "$tryShowTextDocument") => self.try_show_text_document(arg(0), arg(1), window, cx),
            ("MainThreadTextEditors", "$registerTextEditorDecorationType") => {
                self.editors.decoration_types.insert(arg(1).as_str().unwrap_or_default().to_string(), arg(2).clone());
                cx.notify();
                Ok(null)
            }
            ("MainThreadTextEditors", "$removeTextEditorDecorationType") => {
                let key = arg(0).as_str().unwrap_or_default();
                self.editors.decoration_types.remove(key);
                let others = self.editors.other_groups.iter_mut().flat_map(|group| &mut group.editors);
                for editor in self.editors.editors.iter_mut().chain(self.editors.models.iter_mut()).chain(others) {
                    editor.text_decorations.retain(|decoration| decoration.key != key);
                }
                cx.notify();
                Ok(null)
            }
            ("MainThreadTextEditors", method) => {
                let id = arg(0).as_str().unwrap_or_default();
                // Each group shows one editor at a time: a group's active editor is the only one the host can address.
                let groups = self.groups_in_grid_order();
                let Some(group) = groups.into_iter().find(|&group| self.editors.group_active(group).is_some_and(|(editor, _)| snapshot_id(editor) == id)) else {
                    return Err(serialized_error(&format!("Illegal argument: TextEditor({id})")));
                };
                self.with_group(group, |workbench| match method {
                    "$trySetSelections" => workbench.set_host_selections(arg(1), cx),
                    "$tryRevealRange" => workbench.reveal_host_range(arg(1), arg(2), cx),
                    "$trySetOptions" => workbench.set_host_options(arg(1), cx),
                    "$tryApplyEdits" => workbench.apply_host_edits(arg(1), arg(2), arg(3), cx),
                    "$tryInsertSnippet" => workbench.insert_host_snippet(arg(1), arg(2), arg(3), arg(4), window, cx),
                    "$trySetDecorations" | "$trySetDecorationsFast" => {
                        let key = arg(1).as_str().unwrap_or_default();
                        let decorations = match method {
                            "$trySetDecorations" => crate::editor::decoration_types::decorations_from_options(key, arg(2)),
                            _ => crate::editor::decoration_types::decorations_from_numbers(key, arg(2)),
                        };
                        let editor = workbench.editors.active_mut().expect("the group's active editor has the id");
                        editor.text_decorations.retain(|decoration| decoration.key != key);
                        editor.text_decorations.extend(decorations);
                        cx.notify();
                        Ok(Value::Null)
                    }
                    _ => unreachable!("not in HOST_REQUESTS: {method}"),
                })
            }
            _ => unreachable!("not in HOST_REQUESTS: {proxy}.{method}"),
        }
    }

    /// `BoundModelReferenceCollection`: models the host opened and no editor shows are disposed after 3 minutes.
    pub(super) fn release_host_models(&mut self, cx: &mut Context<Self>) {
        let count = self.editors.models.len();
        let scm_inputs = self.scm_input_models();
        // A notebook cell's text model lives as long as its notebook.
        self.editors.release_models_except(Instant::now(), |model| model.document().scheme == crate::notebook::CELL_SCHEME || scm_inputs.iter().any(|(scheme, path)| model.document().is(scheme, path)));
        if self.editors.models.len() != count {
            self.editors_changed(cx);
        }
    }

    /// `$tryOpenDocument`: the model of a file, of the bytes an extension's file system provider read
    /// (`provider_content` an array) or of a content provider's text (a string), opened without an editor and kept
    /// while the host references it. `options.encoding` decodes a new model, or a clean open one again.
    pub(super) fn try_open_document(&mut self, uri: &Value, options: &Value, provider_content: &Value, ignore_path_case: bool, cx: &mut Context<Self>) -> HostResult {
        let (scheme, ref path) = uri_parts(uri)?;
        // uriIdentityService.ts `asCanonicalUri`: where the provider ignores path case, the casing opened first wins.
        if ignore_path_case {
            let lowercase = path.to_string_lossy().to_lowercase();
            let models = self.editors.editors.iter().chain(&self.editors.models);
            let first_casing = models.map(|model| model.document()).find(|document| document.scheme == scheme && document.path != *path && document.path.to_string_lossy().to_lowercase() == lowercase).map(|document| document.uri());
            if let Some(uri) = first_casing {
                return self.try_open_document(&uri, options, provider_content, false, cx);
            }
        }
        let requested = options["encoding"].as_str();
        if let Some(model) = self.editors.model_mut(scheme, path) {
            model.text_model_mut().host_reference = Some(Instant::now());
            if let Some(requested) = requested {
                self.set_model_encoding(uri, scheme, path, requested, cx)
                    .map_err(|detail| serialized_error(&format!("cannot open {}. Detail: {detail}", uri_string(scheme, path))))?;
            }
            return Ok(uri.clone());
        }
        let document = match (scheme, provider_content) {
            (_, Value::Array(_)) => {
                let bytes: Vec<u8> = serde_json::from_value(provider_content.clone()).expect("bytes from main_thread.rs");
                TextDocument::from_bytes(scheme, path, &bytes, requested, &self.languages, &self.settings.lock().unwrap())
            }
            (_, Value::String(text)) => TextDocument::from_text(scheme, path, text, encoding::UTF8.into(), None, &self.languages, &self.settings.lock().unwrap()),
            ("file", _) => {
                let bytes = std::fs::read(path)
                    .map_err(|error| serialized_error(&format!("cannot open {}. Detail: Unable to read file '{}' ({error})", uri_string("file", path), path.display())))?;
                TextDocument::from_bytes(scheme, path, &bytes, requested, &self.languages, &self.settings.lock().unwrap())
            }
            // mainThreadDocuments.ts `_handleUntitledScheme`: an untitled model for the path, never over an existing file.
            ("untitled", _) if path.exists() => return Err(serialized_error(&format!("cannot open {}. Detail: file already exists", uri_string(scheme, path)))),
            ("untitled", _) => {
                let mut document = TextDocument::untitled(path, "", None, &self.languages, &self.settings.lock().unwrap());
                document.encoding = requested.map_or(document.encoding, str::to_string);
                document
            }
            ("vscode", _) => {
                let text = match self.preferences_content(path) {
                    Some(text) => text,
                    None => {
                        let bytes = settings_file_system::read_file(&self.settings.lock().unwrap(), &resource(uri))
                            .map_err(|error| serialized_error(&format!("cannot open {}. Detail: {error}", uri_string(scheme, path))))?;
                        String::from_utf8(bytes).expect("schemas are JSON text")
                    }
                };
                TextDocument::from_text(scheme, path, &text, encoding::UTF8.into(), None, &self.languages, &self.settings.lock().unwrap())
            }
            // textModelResolverService.ts `ensureResolvedTextModelContent`: an inmemory resource resolves only to a model it has.
            ("inmemory", _) => {
                let uri = uri_string(scheme, path);
                return Err(serialized_error(&format!("cannot open {uri}. Detail: Unable to resolve text model content for resource {uri}")));
            }
            _ => return Err(serialized_error(&format!("cannot open {scheme}:{}. Detail: Unable to resolve resource {scheme}:{}", path.display(), path.display()))),
        };
        let index = self.editors.open_model(document);
        let model = &mut self.editors.models[index];
        model.text_model_mut().host_reference = Some(Instant::now());
        model.text_model_mut().associated_file = scheme == "untitled";
        self.editors_changed(cx);
        // `_handleUntitledScheme` ends in `_doCreateUntitled`, which marks the host's document dirty.
        if scheme == "untitled" {
            self.host.send("ExtHostDocuments", "$acceptDirtyStateChanged", vec![uri.clone(), json!(true)]);
        }
        Ok(uri.clone())
    }

    /// `setEncoding(encoding, EncodingMode.Decode)` of an open model: an untitled model takes the encoding, a clean
    /// file model is read again in it (`forceResolveFromFile`) and takes the encoding the read settled on.
    pub(super) fn set_model_encoding(&mut self, uri: &Value, scheme: &str, path: &Path, requested: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let model = self.editors.model_mut(scheme, path).expect("the caller found the model");
        if model.document().encoding == requested {
            return Ok(());
        }
        if scheme == "untitled" {
            model.document_mut().encoding = requested.to_string();
            self.host.send("ExtHostDocuments", "$acceptEncodingChanged", vec![uri.clone(), json!(requested)]);
            return Ok(());
        }
        if model.is_dirty() {
            return Err("Cannot re-open a dirty text document with different encoding. Save it first.".into());
        }
        // shortcut: a model of an extension's file system provider keeps its text, as the GPUI thread cannot read through the provider; it takes the encoding.
        let bytes = match scheme {
            "file" => Some(std::fs::read(path).map_err(|error| format!("Unable to read file '{}' ({error})", path.display()))?),
            _ => None,
        };
        let settings = self.settings.lock().unwrap();
        let resource = (scheme == "file").then_some(path);
        let detected = bytes.as_deref().and_then(|bytes| crate::encoding_guess::detect(bytes, &settings, resource, false));
        let resolved = encoding::read_encoding(&settings, resource, Some(requested), detected.as_deref());
        drop(settings);
        if model.document().encoding != resolved {
            model.document_mut().encoding = resolved.clone();
            self.host.send("ExtHostDocuments", "$acceptEncodingChanged", vec![uri.clone(), json!(resolved)]);
        }
        if let Some(bytes) = bytes {
            self.replace_model_text(scheme, path, encoding::decode(&bytes, &resolved), true, cx);
        }
        Ok(())
    }

    /// mainThreadLanguages.ts `$changeLanguage`: a registered language for the model, opened when needed.
    fn change_host_language(&mut self, uri: &Value, language_id: &Value, cx: &mut Context<Self>) -> HostResult {
        let language_id = language_id.as_str().unwrap_or_default();
        if !self.languages.is_registered(language_id) {
            return Err(serialized_error(&format!("Unknown language id: {language_id}")));
        }
        let (scheme, ref path) = uri_parts(uri)?;
        if self.editors.model(scheme, path).is_none() {
            self.try_open_document(uri, &Value::Null, &Value::Null, false, cx)?;
        }
        self.text_files.preferred_languages.insert((scheme.to_string(), path.clone()));
        self.set_model_language(scheme, path, language_id, cx);
        Ok(Value::Null)
    }

    /// mainThreadLanguages.ts `$tokensAtPosition`: the standard type and range of the token at the position, none
    /// for a model that is not open.
    fn tokens_at_position(&self, scheme: &str, path: &Path, position: &Value) -> Value {
        let Some(editor) = self.editors.model(scheme, path) else { return Value::Null };
        let document = editor.document();
        let line_number = position["lineNumber"].as_u64().unwrap_or(1) as usize;
        let Some(line) = document.lines.get(line_number.saturating_sub(1)) else { return Value::Null };
        let index = line_number - 1;
        let column = crate::editor::char_column(line, (position["column"].as_u64().unwrap_or(1) as usize).saturating_sub(1));
        let offset = line.char_indices().nth(column).map_or(line.len(), |(offset, _)| offset);
        let mut start = 0;
        for (end, token_type, _) in document.tokens.line_tokens(&document.lines, index) {
            if end > offset || end >= line.len() {
                let utf16 = |byte: usize| line[..byte.min(line.len())].encode_utf16().count() + 1;
                return json!({ "type": token_type as u8, "range": { "startLineNumber": line_number, "startColumn": utf16(start), "endLineNumber": line_number, "endColumn": utf16(end) } });
            }
            start = end;
        }
        Value::Null
    }

    /// `$computeFullSyntaxHighlighting`: the source's TextMate tokens in the language (by id or name) as UTF-16 lengths
    /// with their theme foreground index and font style, a 1-long token per newline, and the theme's color map.
    fn full_syntax_highlighting(&self, source: &str, language: &str) -> Value {
        let color_map: Vec<String> = self.theme.tokens.color_map().iter().map(|color| css_hex_alpha(color)).collect();
        let language = if self.languages.is_registered(language) { Some(language) } else { self.languages.id_by_name(language) };
        let Some(grammar) = language.and_then(|language| self.languages.grammars.grammar(language)) else {
            let tokens = if source.is_empty() { json!([]) } else { json!([{ "length": source.encode_utf16().count(), "foreground": 0, "fontStyle": 0 }]) };
            return json!({ "tokens": tokens, "colorMap": color_map });
        };
        let mut tokens: Vec<Value> = Vec::new();
        let mut state = None;
        let lines: Vec<&str> = source.split('\n').collect();
        for (index, line) in lines.iter().enumerate() {
            let Ok((line_tokens, next)) = grammar.tokenize_line(line, state.as_ref()) else { break };
            state = Some(next);
            // tokenizeLine2 emits a token where the metadata changes, so neighbors with one style are one token.
            let mut runs: Vec<(usize, usize, i32)> = Vec::new();
            for token in &line_tokens {
                let style = self.theme.tokens.style(&token.scopes);
                let font_style = style.font_style.max(0);
                let length = line[token.start.min(line.len())..token.end.min(line.len())].encode_utf16().count();
                match runs.last_mut() {
                    Some(last) if last.1 == style.foreground && last.2 == font_style => last.0 += length,
                    _ => runs.push((length, style.foreground, font_style)),
                }
            }
            tokens.extend(runs.into_iter().filter(|run| run.0 > 0).map(|(length, foreground, font_style)| json!({ "length": length, "foreground": foreground, "fontStyle": font_style })));
            if index + 1 < lines.len() {
                tokens.push(json!({ "length": 1, "foreground": 0, "fontStyle": 0 }));
            }
        }
        json!({ "tokens": tokens, "colorMap": color_map })
    }

    /// `$tryCreateDocument`: a new untitled model named `Untitled-<n>` with the first free n.
    fn try_create_document(&mut self, options: &Value, cx: &mut Context<Self>) -> HostResult {
        let path = (1..).map(|n| PathBuf::from(format!("Untitled-{n}"))).find(|path| self.editors.model("untitled", path).is_none()).expect("a free name");
        let content = options["content"].as_str().unwrap_or_default();
        let language = self.untitled_language(options["language"].as_str().map(str::to_string));
        let mut document = TextDocument::untitled(&path, content, language.as_deref(), &self.languages, &self.settings.lock().unwrap());
        document.encoding = options["encoding"].as_str().map_or(document.encoding, str::to_string);
        let uri = document.uri();
        let index = self.editors.open_model(document);
        self.editors.models[index].text_model_mut().host_reference = Some(Instant::now());
        self.editors_changed(cx);
        // mainThreadDocuments.ts `_doCreateUntitled` tells the host the new document is dirty, whatever the model says.
        self.host.send("ExtHostDocuments", "$acceptDirtyStateChanged", vec![uri.clone(), json!(true)]);
        Ok(uri)
    }

    /// `$onVirtualDocumentChange`: a content provider's new text replaces the model's.
    fn virtual_document_change(&mut self, uri: &Value, value: &Value, cx: &mut Context<Self>) -> HostResult {
        let (scheme, ref path) = uri_parts(uri)?;
        if self.editors.model(scheme, path).is_some() {
            self.replace_model_text(scheme, path, value.as_str().unwrap_or_default().to_string(), true, cx);
        }
        Ok(Value::Null)
    }

    /// New text for a model in one edit, after which the model is clean with `clean`: a content provider's model is
    /// no text file model, and a file read again matches the disk.
    // shortcut: one edit over the whole model where upstream applies computeMoreMinimalEdits; port the diff worker when an extension needs the smaller events.
    pub(super) fn replace_model_text(&mut self, scheme: &str, path: &Path, text: String, clean: bool, cx: &mut Context<Self>) {
        let model = self.editors.model_mut(scheme, path).expect("the caller found the model");
        let end = document_end(&model.document().lines);
        let change = model.execute_edits(vec![(Position::new(0, 0), end, text, false)], None, EditReason::ApplyEdits);
        if clean {
            model.mark_saved();
        }
        if let Some(change) = change {
            self.send_model_edit(scheme, path, &change);
            cx.notify();
        }
    }

    /// `$trySaveDocument`: `textFileService.save`, true once the model is saved; a clean model is not written.
    fn try_save_document(&mut self, uri: &Value, cx: &mut Context<Self>) -> HostResult {
        let (scheme, ref path) = uri_parts(uri)?;
        let Some(model) = self.editors.model_mut(scheme, path) else { return Ok(json!(false)) };
        // textFileService.ts `save`: an untitled model saves as its associated file, or as the path the save dialog picks.
        if scheme == "untitled" {
            if model.text_model().associated_file && (self.skip_dialogs || !path.exists()) {
                return Ok(json!(self.do_save_as(scheme, path, path.clone(), Default::default(), cx)));
            }
            // shortcut: this request answers on the session thread's wait, which cannot outlast a dialog; the save goes
            // on after the dialogs and the answer is false. Move the request to the forwarded ones to await it.
            drop(self.save_model(scheme.to_string(), path.clone(), SaveReason::Explicit, cx));
            return Ok(json!(false));
        }
        let saved = self.save_model(scheme.to_string(), path.clone(), SaveReason::Explicit, cx);
        Ok(json!(super::text_files::saved_now(saved) == Some(true)))
    }

    /// First half of `$trySaveDocument` through a provider: false without a model, true when clean, else the bytes to write.
    fn provider_save_bytes(&mut self, uri: &Value) -> HostResult {
        let (scheme, ref path) = uri_parts(uri)?;
        match self.editors.model(scheme, path) {
            None => return Ok(json!(false)),
            Some(model) if !model.is_dirty() => return Ok(json!(true)),
            Some(_) => self.run_save_participants(scheme, path, SaveReason::Explicit, super::text_input::SavePhase::All),
        }
        Ok(json!(self.editors.model(scheme, path).expect("participants keep the model").saved_bytes()))
    }

    /// Second half: the provider wrote the bytes, so the model is clean.
    fn provider_saved(&mut self, uri: &Value, cx: &mut Context<Self>) -> HostResult {
        let (scheme, ref path) = uri_parts(uri)?;
        let Some(model) = self.editors.model_mut(scheme, path) else { return Ok(json!(false)) };
        model.mark_saved();
        let model = self.editors.model(scheme, path).expect("just saved");
        self.host.send_saved(&self.editors, model, true);
        cx.notify();
        Ok(json!(true))
    }

    /// `$tryShowTextDocument`: opens the model in the group, preview unless pinned, and returns the editor's id.
    pub(super) fn try_show_text_document(&mut self, uri: &Value, options: &Value, window: &mut Window, cx: &mut Context<Self>) -> HostResult {
        // A notebook cell's document shows in its notebook's editor.
        if uri["scheme"] == crate::notebook::CELL_SCHEME
            && let Some(shown) = self.show_notebook_of_cell(uri, options, window, cx)
        {
            return shown;
        }
        let (scheme, ref path) = uri_parts(uri)?;
        self.activate_column(&options["position"]);
        let pinned = options["pinned"] == json!(true);
        if let Some(index) = self.editors.editors.iter().position(|editor| editor.document().is(scheme, path)) {
            self.editors.activate(index);
            if pinned {
                self.editors.pin(index);
            }
        } else if let Some(index) = self.editors.models.iter().position(|model| model.document().is(scheme, path)) {
            // editorGroupView.ts pins a dirty editor when it opens.
            let preview = !pinned && !self.editors.models[index].is_dirty();
            self.editors.show_model(index, preview);
        } else if scheme == "file" {
            let document = TextDocument::read(path, &self.languages, &self.settings.lock().unwrap()).map_err(|error| serialized_error(&format!("Unable to read file '{}' ({error})", path.display())))?;
            self.editors.open(document, !pinned);
        } else {
            return Err(serialized_error(&format!("Unable to resolve resource {scheme}:{}", path.display())));
        }
        self.marked_text = None;
        if !options["selection"].is_null() {
            let viewport = self.editors.viewport;
            let editor = self.editors.active_mut().expect("just shown");
            let (start, end) = range(editor, &options["selection"]);
            editor.move_to(start, false);
            editor.move_to(end, true);
            // editorOptions.ts applyTextEditorOptions: `TextEditorSelectionRevealType`, the center without one.
            let reveal = match options["selectionRevealType"].as_u64() {
                Some(1) => VerticalReveal::CenterIfOutsideViewport,
                Some(2) => VerticalReveal::NearTop,
                Some(3) => VerticalReveal::NearTopIfOutsideViewport,
                _ => VerticalReveal::Center,
            };
            editor.reveal_range(start, end, reveal, viewport);
        }
        if options["preserveFocus"] != json!(true) {
            window.focus(&self.editor_focus, cx);
        }
        self.editors_changed(cx);
        Ok(json!(snapshot_id(self.editors.active().expect("just shown"))))
    }

    /// editorCommands.ts `_workbench.open`: a resource in the editor group, a web link in the browser.
    /// `api_open_editor` of a resource no model holds yet and no file is: its text comes from the extension host's
    /// content or file system provider first (textFileEditorModel.ts reads through `fileService`).
    pub(super) fn api_open_editor_resolving(&mut self, args: Vec<Value>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let unresolved = match uri_parts(args.first().unwrap_or(&Value::Null)) {
            Ok((scheme, path)) => !matches!(scheme, "file" | "untitled" | "command") && self.editors.model(scheme, &path).is_none(),
            Err(_) => false,
        };
        if !unresolved {
            return Outcome::Ready(self.api_open_editor(&args, window, cx));
        }
        let content = self.host.call("workbench", "resolveTextContent", vec![args[0].clone()]);
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn_in(window, async move |workbench, cx| {
            let content = content.await.ok().and_then(Result::ok);
            let result = workbench.update_in(cx, |workbench, window, cx| {
                if let Some(content) = content {
                    workbench.try_open_document(&args[0], &Value::Null, &content, false, cx)?;
                }
                workbench.api_open_editor(&args, window, cx)
            });
            drop(sender.send(result.unwrap_or_else(|_| Err(serialized_error("Canceled")))));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    pub(super) fn api_open_editor(&mut self, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> HostResult {
        let resource = args.first().unwrap_or(&Value::Null);
        if let Some(link) = resource.as_str() {
            // extHostApiCommands.ts lets only http and https strings through; the opener service shows them in the browser.
            if !matches!(link.split(':').next(), Some("http" | "https")) {
                return Err(serialized_error(&format!("Unable to open '{link}'")));
            }
            drop(self.open_link(super::opener::Target::Text(link.to_owned()), super::opener::OpenOptions::default(), window, cx));
            return Ok(Value::Null);
        }
        let (scheme, _) = uri_parts(resource)?;
        match scheme {
            // "do not allow to execute commands from here"
            "command" => return Ok(Value::Null),
            // An untitled resource with a path opens as an untitled editor saved to that file.
            "untitled" => drop(self.try_open_document(resource, &Value::Null, &Value::Null, false, cx)?),
            _ => {}
        }
        let column_and_options = args.get(1).unwrap_or(&Value::Null);
        let mut options = column_and_options[1].clone();
        // `_workbench.open` and `_workbench.openWith` carry the column in the options already.
        if (options.is_object() || options.is_null()) && options["position"].is_null() {
            options["position"] = column_and_options[0].clone();
        }
        self.try_show_text_document(resource, &options, window, cx)?;
        Ok(Value::Null)
    }

    /// `setSelections`: one cursor per selection, the first the primary one; nothing scrolls.
    fn set_host_selections(&mut self, selections: &Value, cx: &mut Context<Self>) -> HostResult {
        let editor = self.editors.active_mut().expect("checked by host_request");
        let selections: Vec<Selection> = selections
            .as_array()
            .into_iter()
            .flatten()
            .map(|selection| Selection {
                anchor: position(editor, &selection["selectionStartLineNumber"], &selection["selectionStartColumn"]),
                head: position(editor, &selection["positionLineNumber"], &selection["positionColumn"]),
            })
            .collect();
        if selections.is_empty() {
            return Ok(Value::Null);
        }
        editor.set_selections(selections);
        self.host.send_selection(self.editors.active().expect("checked by host_request"), "api");
        cx.notify();
        Ok(Value::Null)
    }

    /// mainThreadEditor.ts `revealRange`: `TextEditorRevealType` Default, InCenter, InCenterIfOutsideViewport, AtTop.
    fn reveal_host_range(&mut self, range_value: &Value, reveal_type: &Value, cx: &mut Context<Self>) -> HostResult {
        let reveal = match reveal_type.as_u64() {
            Some(1) => VerticalReveal::Center,
            Some(2) => VerticalReveal::CenterIfOutsideViewport,
            Some(3) => VerticalReveal::Top,
            _ => VerticalReveal::Simple,
        };
        // mainThreadEditor.ts `_revealRange` scrolls with `ScrollType.Smooth`.
        let scrolled = self.scroll_smoothly(cx, |editor, viewport| {
            let (start, end) = range(editor, range_value);
            editor.reveal_range(start, end, reveal, viewport);
        });
        if scrolled {
            self.editor_scrolled(cx);
        }
        Ok(Value::Null)
    }

    /// `_setIndentConfiguration`: tab size, indent size and spaces, or "auto" to detect them again from the text.
    // shortcut: cursorStyle and lineNumbers are not applied; the editor draws a line cursor and line numbers only.
    fn set_host_options(&mut self, options: &Value, cx: &mut Context<Self>) -> HostResult {
        let editor = self.editors.active_mut().expect("checked by host_request");
        let before = editor.document().indentation;
        if options["tabSize"] == json!("auto") || options["insertSpaces"] == json!("auto") {
            let created = creation_options(&self.settings.lock().unwrap().configuration, &editor.document().path, &editor.document().language_id);
            let insert_spaces = options["insertSpaces"].as_bool().unwrap_or(created.insert_spaces);
            let tab_size = options["tabSize"].as_u64().map_or(created.tab_size, |size| size as usize);
            editor.detect_indentation(insert_spaces, tab_size);
        } else {
            let mut indentation = before;
            if let Some(insert_spaces) = options["insertSpaces"].as_bool() {
                indentation.insert_spaces = insert_spaces;
            }
            if let Some(tab_size) = options["tabSize"].as_u64() {
                indentation.tab_size = tab_size as usize;
            }
            match &options["indentSize"] {
                Value::Number(size) => indentation.indent_size = size.as_u64().map(|size| size as usize),
                Value::String(size) if size == "tabSize" => indentation.indent_size = None,
                _ => {}
            }
            editor.set_indentation(indentation);
        }
        if editor.document().indentation != before {
            self.host.send_options(editor);
            cx.notify();
        }
        Ok(Value::Null)
    }

    /// mainThreadEditor.ts `applyEdits`: false when the model moved past the host's version.
    fn apply_host_edits(&mut self, version: &Value, edits: &Value, options: &Value, cx: &mut Context<Self>) -> HostResult {
        let editor = self.editors.active_mut().expect("checked by host_request");
        if version.as_u64() != Some(editor.text_model().version_id.into()) {
            return Ok(json!(false));
        }
        if let Some(eol_change) = options["setEndOfLine"].as_u64().and_then(|sequence| editor.push_eol(sequence)) {
            self.host.send_model_change(&self.editors, self.editors.active().expect("checked by host_request"), &eol_change);
        }
        let editor = self.editors.active_mut().expect("checked by host_request");
        let edits = edits
            .as_array()
            .into_iter()
            .flatten()
            .map(|edit| {
                let (start, end) = range(editor, &edit["range"]);
                (start, end, edit["text"].as_str().unwrap_or_default().to_string(), edit["forceMoveMarkers"] == json!(true))
            })
            .collect();
        if options["undoStopBefore"] == json!(true) {
            editor.push_undo_stop();
        }
        let change = editor.execute_edits(edits, None, EditReason::Unknown("MainThreadTextEditor"));
        if options["undoStopAfter"] == json!(true) {
            editor.push_undo_stop();
        }
        if change.is_some() {
            self.editor_edited(change, true, "MainThreadTextEditor", cx);
        }
        Ok(json!(true))
    }

    /// mainThreadEditor.ts `insertSnippet`: the snippet replaces every range, and each copy's first tab stop is selected.
    fn insert_host_snippet(&mut self, version: &Value, template: &Value, ranges: &Value, options: &Value, window: &mut Window, cx: &mut Context<Self>) -> HostResult {
        let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
        let editor = self.editors.active_mut().expect("checked by host_request");
        if version.as_u64() != Some(editor.text_model().version_id.into()) {
            return Ok(json!(false));
        }
        window.focus(&self.editor_focus, cx);
        let mut ranges: Vec<(Position, Position)> = ranges.as_array().into_iter().flatten().map(|value| range(editor, value)).collect();
        if ranges.is_empty() {
            ranges = editor.selections.iter().map(|selection| (selection.start(), selection.end())).collect();
        }
        let template = template.as_str().unwrap_or_default();
        let keep_whitespace = options["keepWhitespace"] == json!(true);
        let snippets: Vec<_> = ranges.into_iter().map(|(start, end)| (start, end, template, keep_whitespace)).collect();
        let insert = SnippetInsert { snippets: &snippets, clipboard: clipboard.as_deref(), undo_stop_before: options["undoStopBefore"] == json!(true), undo_stop_after: options["undoStopAfter"] == json!(true), reason: "snippet" };
        self.insert_snippets(insert, cx);
        Ok(json!(true))
    }
}

/// `SnippetSession`: each template, with whitespace adjusted unless kept, replaces its range; the selections are
/// each copy's first tab stop. The expanded templates come back in the order given.
pub(super) fn snippet_edits(editor: &OpenEditor, snippets: &[(Position, Position, &str, bool)], clipboard: Option<&str>) -> (Vec<(Position, Position, String, bool)>, Vec<Selection>, Vec<SnippetText>) {
    let mut cursor_edits = Vec::new();
    let mut texts = Vec::new();
    for &(start, end, template, keep_whitespace) in snippets {
        let line_indent: String = editor.document().lines[start.line].chars().take(start.column).take_while(|c| *c == ' ' || *c == '\t').collect();
        let snippet = expand_snippet(template, &line_indent, !keep_whitespace, editor.document().indentation, |name| snippet_variable(name, editor, start, end, clipboard));
        let prefix = |chars: usize| snippet.text.chars().take(chars).collect::<String>();
        let selection = Selection { anchor: end_of(start, &prefix(snippet.selection.start)), head: end_of(start, &prefix(snippet.selection.end)) };
        cursor_edits.push(CursorEdit::new(vec![Replace::new(start, end, snippet.text.clone())], selection));
        texts.push(snippet);
    }
    let before: Vec<Selection> = snippets.iter().map(|&(start, end, _, _)| Selection { anchor: start, head: end }).collect();
    let selections = selections_after(&cursor_edits, &before);
    let edits = cursor_edits.into_iter().flat_map(|edit| edit.replaces).map(|replace| (replace.start, replace.end, replace.text, false)).collect();
    (edits, selections, texts)
}

/// `Color.Format.CSS.formatHexA`: `#rrggbbaa` in lower case; the empty first entry stays empty.
fn css_hex_alpha(color: &str) -> String {
    match color.len() {
        0 => String::new(),
        7 => format!("{}ff", color.to_lowercase()),
        _ => color.to_lowercase(),
    }
}

/// snippetVariables.ts: the selection, model and file variables; the rest insert their default.
fn snippet_variable(name: &str, editor: &OpenEditor, start: Position, end: Position, clipboard: Option<&str>) -> Option<String> {
    let lines = &editor.document().lines;
    let path = &editor.document().path;
    match name {
        "CLIPBOARD" => clipboard.map(str::to_owned),
        "SELECTION" | "TM_SELECTED_TEXT" => Some(text_between(lines, start, end)).filter(|text| !text.is_empty()),
        "TM_CURRENT_LINE" => Some(lines[end.line].clone()),
        "TM_CURRENT_WORD" => word_at(&lines[end.line], end.column).map(|word| lines[end.line].chars().skip(word.start).take(word.len()).collect()),
        "TM_LINE_INDEX" => Some(end.line.to_string()),
        "TM_LINE_NUMBER" => Some((end.line + 1).to_string()),
        "TM_FILENAME" => Some(editor.document().name()),
        "TM_FILENAME_BASE" => path.file_stem().map(|stem| stem.to_string_lossy().into_owned()),
        "TM_DIRECTORY" => path.parent().map(|parent| parent.to_string_lossy().into_owned()),
        "TM_FILEPATH" => Some(path.to_string_lossy().into_owned()),
        _ => None,
    }
}

/// The scheme and path a model is keyed by; the authority of a non-file URI leads its path as `//authority`,
/// which `TextDocument::uri` splits off again.
pub(super) fn uri_parts(uri: &Value) -> Result<(&str, PathBuf), Value> {
    let part = |key: &str| uri[key].as_str().unwrap_or_default();
    let (scheme, authority, path, query) = (part("scheme"), part("authority"), part("path"), part("query"));
    // editor.rs `split_query`: a non-file query stays in the model's path.
    let mut query = if query.is_empty() || scheme == "file" { String::new() } else { format!("?{query}") };
    // editor.rs `split_fragment`: a notebook cell's fragment too.
    if scheme == crate::notebook::CELL_SCHEME {
        query.push('#');
        query.push_str(part("fragment"));
    }
    match (scheme, authority) {
        ("", _) => Err(serialized_error("Invalid uri. Scheme and authority or path must be set.")),
        ("file", _) | (_, "") if path.is_empty() => Err(serialized_error("Invalid uri. Scheme and authority or path must be set.")),
        ("file", _) | (_, "") => Ok((scheme, PathBuf::from(format!("{path}{query}")))),
        _ => Ok((scheme, PathBuf::from(format!("//{authority}{path}{query}")))),
    }
}

/// `IRange`, 1-based with UTF-16 columns, validated against the model as `validateRange` does.
pub(super) fn range(editor: &OpenEditor, range: &Value) -> (Position, Position) {
    let start = position(editor, &range["startLineNumber"], &range["startColumn"]);
    let end = position(editor, &range["endLineNumber"], &range["endColumn"]);
    if end < start { (end, start) } else { (start, end) }
}

/// `validatePosition`: before the first line is its start, after the last line is its end.
fn position(editor: &OpenEditor, line: &Value, column: &Value) -> Position {
    let lines = &editor.document().lines;
    let line_number = line.as_f64().unwrap_or(1.);
    if line_number < 1. {
        return Position::new(0, 0);
    }
    if line_number > lines.len() as f64 {
        let last = lines.len() - 1;
        return Position::new(last, lines[last].chars().count());
    }
    let line = line_number as usize - 1;
    let column = (column.as_f64().unwrap_or(1.) as usize).saturating_sub(1);
    Position::new(line, char_column(&lines[line], column))
}

