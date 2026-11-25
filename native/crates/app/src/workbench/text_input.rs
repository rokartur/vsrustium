//! Keyboard and text input of the code editor: keys no keybinding took, and the platform text input
//! (IME, dead keys) through GPUI's input handler. Keybindings run before this (workbench/keyboard.rs).

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use gpui::{Bounds, Context, EntityInputHandler, KeyDownEvent, Pixels, UTF16Selection, Window};

use config::configuration::Overrides;
use exthost::main_thread_files::resource;
use futures::channel::oneshot;

use super::Workbench;
use super::auto_save::SaveReason;
use super::search_editor_view::SEARCH_EDITOR_SCHEME;
use serde_json::json;

use crate::editor::save_participants::SaveParticipants;
use crate::editor::{ContentChanged, Position, uri_string, utf16_column};
use crate::local_history;
use crate::host_sync::FileJob;

impl Workbench {
    /// Enter reaches the editor as typed text, the line break the browser's textarea would insert. codeEditorWidget.ts
    /// `trigger` runs `Handler.Type` itself, not through the command service, so typing never waits for an activation.
    pub(super) fn editor_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if event.keystroke.key != "enter" || modifiers.platform || modifiers.control || modifiers.alt || self.marked_text.is_some() {
            return;
        }
        cx.stop_propagation();
        let typing = self.commands.get("type").expect("editor_commands registers type");
        drop(typing(self, vec![json!({ "text": "\n" })], window, cx));
    }

    /// Tells the host about a content change or a cursor move, and reveals the cursor (cursor.ts `revealAll`).
    pub(super) fn editor_edited(&mut self, change: Option<ContentChanged>, moved: bool, source: &str, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("only the active editor is edited");
        let scroll = editor.scroll;
        editor.reveal_cursors(source, viewport);
        let scrolled = editor.scroll != scroll;
        self.find_editor_edited(change.as_ref());
        match &change {
            Some(change) => self.host.send_content_change(&self.editors, self.editors.active_index().expect("only the active editor is edited"), change),
            None if moved => self.host.send_selection(self.editors.active().expect("still open"), source),
            None => {}
        }
        // The edit can unfold a region or change the line count, which changes the visible lines.
        if change.is_some() {
            self.host.send_visible_ranges(&self.editors);
            let document = &self.editors.active().expect("still open").document;
            let (scheme, path) = (document.scheme.clone(), document.path.clone());
            self.search_model_changed(&scheme, &path);
            if let Some(change) = &change {
                self.search_editor_edited(&path, change);
            }
            self.schedule_keybinding_marks(cx);
        }
        if scrolled {
            self.editor_scrolled(cx);
        }
        self.snippet_state_changed(cx);
        self.suggest_editor_changed(change.is_some(), source, cx);
        self.parameter_hints_editor_changed(source, cx);
        if moved || change.is_some() {
            self.code_actions_cursor_changed(change.is_some(), cx);
        }
        if change.is_some() {
            self.code_lens_content_changed(cx);
        }
        cx.notify();
    }

    /// Writes the editor's model, clean or not, and tells the host it was saved; a model of an extension's file
    /// system provider is written through the host thread. The receiver answers whether the write succeeded.
    /// shortcut: no `$participateInSave` (only `$trySaveDocument` runs it), no format on save or code actions on save,
    /// no conflict check against the file on disk.
    pub(super) fn save_editor(&mut self, index: usize, cx: &mut Context<Self>) -> oneshot::Receiver<bool> {
        let document = &self.editors.editors[index].document;
        let (scheme, path) = (document.scheme.clone(), document.path.clone());
        self.save_model(scheme, path, SaveReason::Explicit, cx)
    }

    /// `save_editor` for any open model, one without an editor too.
    pub(super) fn save_model(&mut self, scheme: String, path: PathBuf, reason: SaveReason, cx: &mut Context<Self>) -> oneshot::Receiver<bool> {
        if scheme == SEARCH_EDITOR_SCHEME {
            return self.save_search_editor(path, cx);
        }
        self.run_save_participants(&scheme, &path, reason);
        let (sender, receiver) = oneshot::channel();
        let editor = self.editors.model(&scheme, &path).expect("saving an open model");
        let version = editor.alternative_version_id();
        let bytes = editor.saved_bytes();
        if scheme == "file" {
            let written = std::fs::write(&path, bytes).map_err(|error| error.to_string());
            let _ = sender.send(self.finish_save(&scheme, &path, version, written, cx));
            return receiver;
        }
        let write = self.host.run_files(FileJob::Write(resource(&editor.document.uri()), bytes));
        cx.spawn(async move |workbench, cx| {
            let written = match write.await {
                Ok(result) => result.map(drop).map_err(|error| error.to_string()),
                Err(_) => Err("Canceled".to_string()),
            };
            let saved = workbench.update(cx, |workbench, cx| workbench.finish_save(&scheme, &path, version, written, cx));
            let _ = sender.send(saved.unwrap_or(false));
        })
        .detach();
        receiver
    }

    /// saveParticipants.ts: the text participants of the model's language and resource, their changes sent to the host.
    pub(super) fn run_save_participants(&mut self, scheme: &str, path: &Path, reason: SaveReason) {
        // `findEditor`: only the group's one code editor, showing the active model, has cursors.
        let shown = self.editors.active().is_some_and(|editor| editor.document.is(scheme, path));
        let model = self.editors.model_mut(scheme, path).expect("saving an open model");
        let spared_cursors = match reason == SaveReason::Auto && shown {
            true => model.selections.iter().map(|selection| selection.head).collect(),
            false => Vec::new(),
        };
        let settings = self.settings.lock().unwrap();
        let overrides = Overrides { resource: Some(path), override_identifier: Some(&model.document.language_id) };
        let on = |key: &str| settings.get_value(Some(key), &overrides) == Some(json!(true));
        let participants = SaveParticipants {
            trim_trailing_whitespace: on("files.trimTrailingWhitespace"),
            trim_in_regex_and_strings: settings.get_value(Some("files.trimTrailingWhitespaceInRegexAndStrings"), &overrides) != Some(json!(false)),
            insert_final_newline: on("files.insertFinalNewline"),
            trim_final_newlines: on("files.trimFinalNewlines"),
            spared_cursors,
        };
        drop(settings);
        let changes = model.run_save_participants(&participants);
        for change in &changes {
            self.send_model_edit(scheme, path, change);
        }
        if !changes.is_empty() {
            self.host.send_visible_ranges(&self.editors);
        }
    }

    /// The model is clean up to the version that was written, and the host hears it was saved.
    pub(super) fn finish_save(&mut self, scheme: &str, path: &Path, version: u32, written: Result<(), String>, cx: &mut Context<Self>) -> bool {
        if let Err(error) = written {
            // shortcut: textFileSaveErrorHandler.ts's generic error without its Retry, Save As and Revert actions.
            let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
            self.notify_error(format!("Failed to save '{name}': {error}"));
            cx.notify();
            return false;
        }
        // An editor closed while its model was written has nothing left to mark.
        let Some(model) = self.editors.model_mut(scheme, path) else { return true };
        let was_dirty = model.is_dirty();
        model.mark_saved_at(version);
        let became_clean = was_dirty && !model.is_dirty();
        let model = self.editors.model(scheme, path).expect("found above");
        self.host.send_saved(&self.editors, model, became_clean);
        // storedFileWorkingCopy.ts saves only a dirty model, and only those saves reach the history tracker.
        if scheme == "file" && was_dirty {
            self.add_local_history_entry(path, cx);
        }
        cx.notify();
        true
    }

    /// workingCopyHistoryTracker.ts `onDidSave`: the saved file goes into the local history with its modification
    /// time. shortcut: every save has the "File Saved" source; the undo stack's label as the source
    /// (`resolveSourceFromUndoRedo`) shows only in the Timeline, which is not ported.
    fn add_local_history_entry(&self, path: &Path, cx: &mut Context<Self>) {
        let Ok(metadata) = std::fs::metadata(path) else { return };
        let settings = self.settings.lock().unwrap();
        let Some(limits) = local_history::tracked(&settings, path, metadata.len()) else { return };
        let history_home = settings.user_home().join("History");
        drop(settings);
        let timestamp = metadata.modified().ok().and_then(|modified| modified.duration_since(UNIX_EPOCH).ok()).map_or(0, |since| since.as_millis() as i64);
        let (resource, path) = (uri_string("file", path), path.to_path_buf());
        cx.background_executor().spawn(async move {
            if let Err(error) = local_history::add_entry(&history_home, &resource, &path, None, timestamp, &limits) {
                eprintln!("local history of {}: {error}", path.display());
            }
        })
        .detach();
    }

    /// A UTF-16 range of the primary cursor's line, the text the input handler works on, as the chars
    /// before and after the cursor it replaces (`replacePrevCharCnt`, `replaceNextCharCnt`).
    fn input_range(&self, range: Range<usize>) -> (usize, usize) {
        let editor = self.editors.active().expect("input goes to the active editor");
        let head = editor.selection().head;
        let line = &editor.document.lines[head.line];
        let (start, end) = (column_at_utf16(line, range.start), column_at_utf16(line, range.end));
        (head.column.saturating_sub(start), end.saturating_sub(head.column))
    }
}

/// The platform's text input sees the cursor's line as the whole text.
/// shortcut: the IME sees one line, so reconversion across lines and candidate window placement
/// (`bounds_for_range`) are not supported; the candidate window opens at the system's default spot.
impl EntityInputHandler for Workbench {
    fn text_for_range(&mut self, range: Range<usize>, adjusted_range: &mut Option<Range<usize>>, window: &mut Window, _: &mut Context<Self>) -> Option<String> {
        if let Some(id) = self.focused_input(window) {
            return self.input_text_for_range(id, range, adjusted_range);
        }
        let editor = self.editors.active()?;
        let units: Vec<u16> = editor.document.lines[editor.selection().head.line].encode_utf16().collect();
        let range = range.start.min(units.len())..range.end.min(units.len());
        *adjusted_range = Some(range.clone());
        Some(String::from_utf16_lossy(&units[range]))
    }

    fn selected_text_range(&mut self, _: bool, window: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        if let Some(id) = self.focused_input(window) {
            return self.input_selected_range(id);
        }
        let editor = self.editors.active()?;
        let selection = editor.selection();
        let line = &editor.document.lines[selection.head.line];
        let head = utf16_column(line, selection.head.column);
        let anchor = if selection.anchor.line == selection.head.line { utf16_column(line, selection.anchor.column) } else { head };
        Some(UTF16Selection { range: anchor.min(head)..anchor.max(head), reversed: head < anchor })
    }

    fn marked_text_range(&self, window: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        if let Some(id) = self.focused_input(window) {
            return self.input_view(id).marked.clone();
        }
        self.marked_text.clone()
    }

    fn unmark_text(&mut self, window: &mut Window, _: &mut Context<Self>) {
        if let Some(id) = self.focused_input(window) {
            self.input_view_mut(id).marked = None;
            return;
        }
        self.marked_text = None;
    }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.focused_input(window) {
            return self.input_replace(id, range, text, false, window, cx);
        }
        if self.edit_blocked(true, cx) {
            self.marked_text = None;
            return;
        }
        let range = range.or(self.marked_text.take()).map(|range| self.input_range(range));
        let Some((previous, next)) = range else {
            let overtype = self.overtype;
            self.run_indenting(|editor, auto| editor.type_keyboard(text, auto, overtype), cx);
            return;
        };
        let editor = self.editors.active_mut().expect("input goes to the active editor");
        let change = editor.type_replacing(text, previous, next, "type");
        self.editor_edited(change, false, "keyboard", cx);
        if self.overtype {
            let editor = self.editors.active_mut().expect("input goes to the active editor");
            let change = editor.overtype_composed(text);
            self.editor_edited(change, false, "keyboard", cx);
        }
    }

    /// IME composition: the marked text replaces the previous one and stays marked.
    /// shortcut: the caret goes to the end of the marked text, whatever selection the IME asks for.
    fn replace_and_mark_text_in_range(&mut self, range: Option<Range<usize>>, new_text: &str, _: Option<Range<usize>>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.focused_input(window) {
            return self.input_replace(id, range, new_text, true, window, cx);
        }
        if self.edit_blocked(true, cx) {
            return;
        }
        let (previous, next) = range.or(self.marked_text.clone()).map_or((0, 0), |range| self.input_range(range));
        let editor = self.editors.active_mut().expect("input goes to the active editor");
        let primary = editor.selection();
        let start = if primary.is_empty() { Position::new(primary.head.line, primary.head.column.saturating_sub(previous)) } else { primary.start() };
        let change = editor.type_replacing(new_text, previous, next, "compositionType");
        let marked_start = utf16_column(&editor.document.lines[start.line], start.column);
        self.marked_text = (!new_text.is_empty()).then(|| marked_start..marked_start + new_text.encode_utf16().count());
        self.editor_edited(change, false, "keyboard", cx);
    }

    fn bounds_for_range(&mut self, _: Range<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(&mut self, _: gpui::Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

/// The char column at a UTF-16 offset of the line; an offset inside a surrogate pair rounds up.
fn column_at_utf16(line: &str, offset: usize) -> usize {
    let mut units = 0;
    for (column, c) in line.chars().enumerate() {
        if units >= offset {
            return column;
        }
        units += c.len_utf16();
    }
    line.chars().count()
}
