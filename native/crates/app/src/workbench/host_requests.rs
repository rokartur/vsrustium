//! The MainThread* requests the workbench answers itself: mainThreadDocuments.ts, mainThreadEditors.ts with
//! mainThreadEditor.ts, and mainThreadClipboard.ts. They arrive from the session thread through `main_thread.rs`.

use std::path::Path;
use std::time::Instant;

use exthost::main_thread::serialized_error;
use gpui::{ClipboardItem, Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::editor::{
    CursorEdit, EditReason, OpenEditor, Position, Replace, Selection, TextDocument, VerticalReveal, char_column, end_of, expand_snippet, guess_indentation, indent_guides,
    selections_after, text_between, uri_string, word_at,
};
use crate::host_sync::snapshot_id;

/// The requests `host_request` answers; every other one stays with the session.
pub const HOST_REQUESTS: &[(&str, &str)] = &[
    ("MainThreadClipboard", "$readText"),
    ("MainThreadClipboard", "$writeText"),
    ("MainThreadDocuments", "$tryOpenDocument"),
    ("MainThreadDocuments", "$trySaveDocument"),
    ("MainThreadTextEditors", "$tryShowTextDocument"),
    ("MainThreadTextEditors", "$trySetSelections"),
    ("MainThreadTextEditors", "$tryRevealRange"),
    ("MainThreadTextEditors", "$trySetOptions"),
    ("MainThreadTextEditors", "$tryApplyEdits"),
    ("MainThreadTextEditors", "$tryInsertSnippet"),
];

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
            ("MainThreadDocuments", "$tryOpenDocument") => self.try_open_document(arg(0), arg(2), cx),
            ("MainThreadDocuments", "$trySaveDocument") => self.try_save_document(arg(0), cx),
            ("workbench", "providerSaveText") => self.provider_save_bytes(arg(0)),
            ("workbench", "providerSaved") => self.provider_saved(arg(0), cx),
            ("MainThreadTextEditors", "$tryShowTextDocument") => self.try_show_text_document(arg(0), arg(1), window, cx),
            ("MainThreadTextEditors", method) => {
                let id = arg(0).as_str().unwrap_or_default();
                // The group shows one editor at a time: the active one is the only one the host can address.
                if self.editors.active().map(snapshot_id).as_deref() != Some(id) {
                    return Err(serialized_error(&format!("Illegal argument: TextEditor({id})")));
                }
                match method {
                    "$trySetSelections" => self.set_host_selections(arg(1), cx),
                    "$tryRevealRange" => self.reveal_host_range(arg(1), arg(2), cx),
                    "$trySetOptions" => self.set_host_options(arg(1), cx),
                    "$tryApplyEdits" => self.apply_host_edits(arg(1), arg(2), arg(3), cx),
                    "$tryInsertSnippet" => self.insert_host_snippet(arg(1), arg(2), arg(3), arg(4), window, cx),
                    _ => unreachable!("not in HOST_REQUESTS: {method}"),
                }
            }
            _ => unreachable!("not in HOST_REQUESTS: {proxy}.{method}"),
        }
    }

    /// `BoundModelReferenceCollection`: models the host opened and no editor shows are disposed after 3 minutes.
    fn release_host_models(&mut self, cx: &mut Context<Self>) {
        let count = self.editors.models.len();
        self.editors.release_models(Instant::now());
        if self.editors.models.len() != count {
            self.editors_changed(cx);
        }
    }

    /// `$tryOpenDocument`: the model of a file, or of `provider_bytes` an extension's file system provider read,
    /// opened without an editor and kept while the host references it.
    fn try_open_document(&mut self, uri: &Value, provider_bytes: &Value, cx: &mut Context<Self>) -> HostResult {
        let (scheme, path) = uri_parts(uri)?;
        if let Some(model) = self.editors.model_mut(scheme, path) {
            model.host_reference = Some(Instant::now());
            return Ok(uri.clone());
        }
        let document = match (scheme, provider_bytes) {
            (_, Value::Array(_)) => {
                let bytes: Vec<u8> = serde_json::from_value(provider_bytes.clone()).expect("bytes from main_thread.rs");
                TextDocument::from_bytes(scheme, path, &bytes, &self.languages, &self.settings.lock().unwrap())
            }
            ("file", _) => TextDocument::read(path, &self.languages, &self.settings.lock().unwrap())
                .map_err(|error| serialized_error(&format!("cannot open {}. Detail: Unable to read file '{}' ({error})", uri_string("file", path), path.display())))?,
            // shortcut: untitled documents are not opened; they come with the untitled ledger row.
            _ => return Err(serialized_error(&format!("cannot open {scheme}:{}. Detail: Unable to resolve resource {scheme}:{}", path.display(), path.display()))),
        };
        let index = self.editors.open_model(document);
        self.editors.models[index].host_reference = Some(Instant::now());
        self.editors_changed(cx);
        Ok(uri.clone())
    }

    /// `$trySaveDocument`: `textFileService.save`, true once the model is saved; a clean model is not written.
    fn try_save_document(&mut self, uri: &Value, cx: &mut Context<Self>) -> HostResult {
        let (scheme, path) = uri_parts(uri)?;
        let Some(model) = self.editors.model_mut(scheme, path) else { return Ok(json!(false)) };
        if !model.is_dirty() {
            return Ok(json!(true));
        }
        model.save().map_err(|error| serialized_error(&format!("Unable to write file '{}' ({error})", path.display())))?;
        let model = self.editors.model(scheme, path).expect("just saved");
        self.host.send_saved(&self.editors, model, true);
        cx.notify();
        Ok(json!(true))
    }

    /// First half of `$trySaveDocument` through a provider: false without a model, true when clean, else the bytes to write.
    fn provider_save_bytes(&self, uri: &Value) -> HostResult {
        let (scheme, path) = uri_parts(uri)?;
        Ok(match self.editors.model(scheme, path) {
            None => json!(false),
            Some(model) if !model.is_dirty() => json!(true),
            Some(model) => json!(model.saved_bytes()),
        })
    }

    /// Second half: the provider wrote the bytes, so the model is clean.
    fn provider_saved(&mut self, uri: &Value, cx: &mut Context<Self>) -> HostResult {
        let (scheme, path) = uri_parts(uri)?;
        let Some(model) = self.editors.model_mut(scheme, path) else { return Ok(json!(false)) };
        model.mark_saved();
        let model = self.editors.model(scheme, path).expect("just saved");
        self.host.send_saved(&self.editors, model, true);
        cx.notify();
        Ok(json!(true))
    }

    /// `$tryShowTextDocument`: opens the model in the group, preview unless pinned, and returns the editor's id.
    fn try_show_text_document(&mut self, uri: &Value, options: &Value, window: &mut Window, cx: &mut Context<Self>) -> HostResult {
        let (scheme, path) = uri_parts(uri)?;
        let pinned = options["pinned"] == json!(true);
        if let Some(index) = self.editors.editors.iter().position(|editor| editor.document.is(scheme, path)) {
            self.editors.activate(index);
            if pinned {
                self.editors.pin(index);
            }
        } else if let Some(index) = self.editors.models.iter().position(|model| model.document.is(scheme, path)) {
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
            // editorOptions.ts applyTextEditorOptions: no `selectionRevealType` reveals in the center.
            editor.reveal_range(start, end, VerticalReveal::Center, viewport);
        }
        if options["preserveFocus"] != json!(true) {
            window.focus(&self.editor_focus, cx);
        }
        self.editors_changed(cx);
        Ok(json!(snapshot_id(self.editors.active().expect("just shown"))))
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
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("checked by host_request");
        let (start, end) = range(editor, range_value);
        let scroll = editor.scroll;
        editor.reveal_range(start, end, reveal, viewport);
        if editor.scroll != scroll {
            self.editor_scrolled(cx);
        }
        Ok(Value::Null)
    }

    /// `_setIndentConfiguration`: tab size and spaces, or "auto" to detect them again from the text.
    // shortcut: cursorStyle and lineNumbers are not applied; the editor draws a line cursor and line numbers only.
    fn set_host_options(&mut self, options: &Value, cx: &mut Context<Self>) -> HostResult {
        let editor = self.editors.active_mut().expect("checked by host_request");
        let before = editor.document.indentation;
        let mut indentation = before;
        if let Some(insert_spaces) = options["insertSpaces"].as_bool() {
            indentation.insert_spaces = insert_spaces;
        }
        if let Some(tab_size) = options["tabSize"].as_u64() {
            indentation.tab_size = tab_size as usize;
        }
        if options["tabSize"] == json!("auto") || options["insertSpaces"] == json!("auto") {
            indentation = guess_indentation(&editor.document.lines, indentation);
        }
        if indentation == before {
            return Ok(Value::Null);
        }
        editor.document.indentation = indentation;
        editor.guides = indent_guides(&editor.document.lines, indentation.tab_size, indentation.tab_size, editor.document.off_side);
        self.host.send_options(self.editors.active().expect("checked by host_request"));
        cx.notify();
        Ok(Value::Null)
    }

    /// mainThreadEditor.ts `applyEdits`: false when the model moved past the host's version.
    // shortcut: `setEndOfLine` is not applied; the model keeps its line ending.
    fn apply_host_edits(&mut self, version: &Value, edits: &Value, options: &Value, cx: &mut Context<Self>) -> HostResult {
        let editor = self.editors.active_mut().expect("checked by host_request");
        if version.as_u64() != Some(editor.version_id.into()) {
            return Ok(json!(false));
        }
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
        if version.as_u64() != Some(editor.version_id.into()) {
            return Ok(json!(false));
        }
        window.focus(&self.editor_focus, cx);
        let mut ranges: Vec<(Position, Position)> = ranges.as_array().into_iter().flatten().map(|value| range(editor, value)).collect();
        if ranges.is_empty() {
            ranges = editor.selections.iter().map(|selection| (selection.start(), selection.end())).collect();
        }
        let adjust_indentation = options["keepWhitespace"] != json!(true);
        let mut cursor_edits = Vec::new();
        for &(start, end) in &ranges {
            let line_indent: String = editor.document.lines[start.line].chars().take(start.column).take_while(|c| *c == ' ' || *c == '\t').collect();
            let snippet = expand_snippet(template.as_str().unwrap_or_default(), &line_indent, adjust_indentation, editor.document.indentation, |name| {
                snippet_variable(name, editor, start, end, clipboard.as_deref())
            });
            let prefix = |chars: usize| snippet.text.chars().take(chars).collect::<String>();
            let selection = Selection { anchor: end_of(start, &prefix(snippet.selection.start)), head: end_of(start, &prefix(snippet.selection.end)) };
            cursor_edits.push(CursorEdit::new(vec![Replace::new(start, end, snippet.text.clone())], selection));
        }
        let before: Vec<Selection> = ranges.iter().map(|&(start, end)| Selection { anchor: start, head: end }).collect();
        let selections = selections_after(&cursor_edits, &before);
        let edits = cursor_edits.into_iter().flat_map(|edit| edit.replaces).map(|replace| (replace.start, replace.end, replace.text, false)).collect();
        if options["undoStopBefore"] == json!(true) {
            editor.push_undo_stop();
        }
        let change = editor.execute_edits(edits, Some(selections), EditReason::Unknown("snippet"));
        if options["undoStopAfter"] == json!(true) {
            editor.push_undo_stop();
        }
        self.editor_edited(change, true, "snippet", cx);
        Ok(json!(true))
    }
}

/// snippetVariables.ts: the selection, model and file variables; the rest insert their default.
fn snippet_variable(name: &str, editor: &OpenEditor, start: Position, end: Position, clipboard: Option<&str>) -> Option<String> {
    let lines = &editor.document.lines;
    let path = &editor.document.path;
    match name {
        "CLIPBOARD" => clipboard.map(str::to_owned),
        "SELECTION" | "TM_SELECTED_TEXT" => Some(text_between(lines, start, end)).filter(|text| !text.is_empty()),
        "TM_CURRENT_LINE" => Some(lines[end.line].clone()),
        "TM_CURRENT_WORD" => word_at(&lines[end.line], end.column).map(|word| lines[end.line].chars().skip(word.start).take(word.len()).collect()),
        "TM_LINE_INDEX" => Some(end.line.to_string()),
        "TM_LINE_NUMBER" => Some((end.line + 1).to_string()),
        "TM_FILENAME" => Some(editor.document.name()),
        "TM_FILENAME_BASE" => path.file_stem().map(|stem| stem.to_string_lossy().into_owned()),
        "TM_DIRECTORY" => path.parent().map(|parent| parent.to_string_lossy().into_owned()),
        "TM_FILEPATH" => Some(path.to_string_lossy().into_owned()),
        _ => None,
    }
}

fn uri_parts(uri: &Value) -> Result<(&str, &Path), Value> {
    match (uri["scheme"].as_str(), uri["path"].as_str()) {
        (Some(scheme), Some(path)) if !scheme.is_empty() && !path.is_empty() => Ok((scheme, Path::new(path))),
        _ => Err(serialized_error("Invalid uri. Scheme and authority or path must be set.")),
    }
}

/// `IRange`, 1-based with UTF-16 columns, validated against the model as `validateRange` does.
fn range(editor: &OpenEditor, range: &Value) -> (Position, Position) {
    let start = position(editor, &range["startLineNumber"], &range["startColumn"]);
    let end = position(editor, &range["endLineNumber"], &range["endColumn"]);
    if end < start { (end, start) } else { (start, end) }
}

/// `validatePosition`: before the first line is its start, after the last line is its end.
fn position(editor: &OpenEditor, line: &Value, column: &Value) -> Position {
    let lines = &editor.document.lines;
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

