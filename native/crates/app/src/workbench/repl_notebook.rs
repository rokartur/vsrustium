//! contrib/replNotebook: the REPL editor (replEditorInput.ts, replEditor.ts, repl.contribution.ts). A notebook shown
//! with view type `repl` whose last cell is the input box under the read-only list of the others; interactive.rs
//! draws and runs it as it does the interactive window's input.

use std::path::Path;

use exthost::main_thread::serialized_error;
use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::host_requests::uri_parts;
use crate::commands::Outcome;
use crate::notebook::{self, CODE, History, Interactive, MARKUP, NOTEBOOK_EDITOR_SCHEME};

pub const VIEW_TYPE: &str = "repl";

impl Workbench {
    /// With the REPL input focused: the input's editor, and the list's focused cell editor with the cell's index and
    /// the list's length once the list has a cell (the status bar entries).
    pub(super) fn repl_input_status(&self, window: &Window) -> Option<(&crate::editor::OpenEditor, Option<(Option<&crate::editor::OpenEditor>, usize, usize)>)> {
        let interactive = self.active_interactive().filter(|interactive| interactive.repl)?;
        let input = self.editors.active().filter(|editor| self.editor_focus.is_focused(window) && uri_parts(&interactive.input).is_ok_and(|(scheme, path)| editor.document().is(scheme, &path)))?;
        let notebook = self.active_notebook_editor()?;
        let model = self.host.notebooks.model(&notebook.notebook)?;
        let count = model.cells.len().saturating_sub(1);
        let cells = (count > 0).then(|| (model.cells.get(notebook.focus.min(count - 1)).and_then(|cell| self.cell_model(&cell.uri)), notebook.focus.min(count - 1), count));
        Some((input, cells))
    }

    pub(super) fn register_repl_commands(&mut self) {
        self.commands.register("repl.execute", |workbench, args, window, cx| workbench.execute_interactive_input(args.first(), window, cx));
        self.commands.register("repl.input.focus", |workbench, _, window, cx| {
            if workbench.reveal_repl(window, cx) {
                workbench.focus_interactive_input(window, cx);
            }
            Outcome::done()
        });
        // `repl.focusLastItemExecuted`: the last history cell takes the focus as a container.
        self.commands.register("repl.focusLastItemExecuted", |workbench, _, window, cx| {
            if !workbench.reveal_repl(window, cx) {
                return Outcome::done();
            }
            let Some(editor) = workbench.active_notebook_editor() else { return Outcome::done() };
            let tab = editor.tab_model_id;
            let Some(model) = workbench.host.notebooks.model(&editor.notebook) else { return Outcome::done() };
            if let Some(cell) = model.cells.len().checked_sub(2).map(|index| &model.cells[index]) {
                let handle = cell.handle;
                workbench.focus_notebook_cell(tab, handle, false, window, cx);
            }
            Outcome::done()
        });
    }

    /// `$tryShowNotebookDocument` with view type `repl`: ReplDocumentContribution `createEditorInput` (one editor per
    /// notebook), ReplEditorInput `resolve` (an input box cell appended when the last cell cannot be one) and
    /// `ReplEditor.focus` (the input editor takes the focus).
    pub(super) fn show_repl(&mut self, uri: Value, options: Value, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let key = notebook::uri_string(&uri);
        if self.host.notebooks.model(&key).is_none() {
            return Outcome::Ready(Err(serialized_error("The REPL model could not be resolved")));
        }
        if !self.host.notebooks.interactives.iter().any(|interactive| interactive.notebook == key) {
            if let Err(error) = self.ensure_input_box_cell(&key, cx) {
                return Outcome::Ready(Err(error));
            }
            let model = self.host.notebooks.model(&key).expect("checked above");
            let input = model.cells.last().expect("an input box cell").uri.clone();
            let title = options["label"].as_str().map_or_else(|| repl_label(&model.uri), str::to_owned);
            self.host.notebooks.interactives.push(Interactive { notebook: key.clone(), input, title: Some(title), history: History::new(""), shown: false, repl: true });
            self.host.notebooks.interactive_window_open = true;
        }
        let shown = self.show_notebook(&key, &options, window, cx);
        if shown.is_ok() && options["preserveFocus"] != true {
            self.focus_interactive_input(window, cx);
        }
        Outcome::Ready(shown)
    }

    /// ReplEditorInput `ensureInputBoxCell`: an empty python code cell at the end unless the last cell is a code cell
    /// that never ran.
    fn ensure_input_box_cell(&mut self, key: &str, cx: &mut Context<Self>) -> Result<(), Value> {
        let model = self.host.notebooks.model(key).expect("an open notebook");
        let usable = model.cells.last().is_some_and(|cell| cell.kind != MARKUP && cell.outputs.is_empty() && cell.internal_metadata["executionOrder"].is_null());
        if usable {
            return Ok(());
        }
        let cell = json!({ "cellKind": CODE, "language": "python", "mime": null, "outputs": [], "source": "", "metadata": {} });
        let edit = json!({ "editType": 1, "index": model.cells.len(), "count": 0, "cells": [cell] });
        self.apply_notebook_edits(key, &[edit], cx).map(drop)
    }

    /// The active editor is a REPL editor, else the `mostRecentReplEditor` one's tab becomes active (`openEditor` in
    /// its group); false without either.
    fn reveal_repl(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.active_interactive().is_some_and(|interactive| interactive.repl) {
            return true;
        }
        let Some(key) = self.host.notebooks.most_recent_repl.clone() else { return false };
        for group in self.editors.group_ids_mru() {
            let (tabs, _) = self.editors.group_tabs(group);
            let Some(index) = tabs.iter().position(|tab| tab.document().is(NOTEBOOK_EDITOR_SCHEME, Path::new(&key))) else { continue };
            self.editors.activate_group(group);
            self.editors.activate(index);
            window.focus(&self.notebook_views.list_focus, cx);
            self.editors_changed(cx);
            return true;
        }
        false
    }

    /// A cell of an interactive window's or REPL editor's list, by its text model path (the REPL input cell is not).
    pub(super) fn is_composite_history_cell(&self, path: &Path) -> bool {
        let notebooks = &self.host.notebooks;
        notebooks.interactives.iter().filter_map(|interactive| Some((interactive, notebooks.model(&interactive.notebook)?))).any(|(interactive, model)| {
            model.cells.iter().any(|cell| cell.uri != interactive.input && uri_parts(&cell.uri).is_ok_and(|(_, cell_path)| cell_path == path))
        })
    }

    /// The input editor model of an interactive window or REPL editor (its editor takes simpleEditorOptions.ts).
    pub(super) fn is_interactive_input(&self, editor: &crate::editor::OpenEditor) -> bool {
        let document = editor.document();
        self.host.notebooks.interactives.iter().any(|interactive| uri_parts(&interactive.input).is_ok_and(|(scheme, path)| document.is(scheme, &path)))
    }

    /// The scratchpad capability of a REPL editor on an untitled notebook without
    /// `interactiveWindow.promptToSaveOnClose`: closing it asks nothing.
    pub(super) fn is_scratchpad_repl(&self, key: &str) -> bool {
        let repl = self.host.notebooks.interactives.iter().any(|interactive| interactive.notebook == key && interactive.repl);
        repl && self.host.notebooks.model(key).is_some_and(|model| model.uri["scheme"] == "untitled") && self.setting("interactiveWindow.promptToSaveOnClose") != true
    }
}

/// ReplEditorInput `createEditorLabel`: `REPL - N` for an untitled `Untitled-N.` notebook, else `REPL - <file name>`.
fn repl_label(uri: &Value) -> String {
    let path = uri["path"].as_str().unwrap_or_default();
    if uri["scheme"] == "untitled"
        && let Some((_, rest)) = path.split_once("Untitled-")
    {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        // The regex `Untitled-(\d+).` wants one character after the digits.
        if !digits.is_empty() && rest.len() > digits.len() {
            return format!("REPL - {digits}");
        }
    }
    match path.rsplit('/').next().filter(|name| !name.is_empty()) {
        Some(name) => format!("REPL - {name}"),
        None => "REPL".into(),
    }
}

#[cfg(test)]
mod tests;
