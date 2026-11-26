//! The interactive window: contrib/interactive (interactive.contribution.ts `_interactive.open`, `interactive.execute`
//! and the input, history and scroll commands; interactiveEditor.ts's input cell below the cell list;
//! interactiveHistoryService.ts) and mainThreadInteractive.ts. Its notebook is an untitled `interactive` notebook shown
//! by the notebook editor; its input is a text model at `vscode-interactive-input:/InteractiveInput-N`.

use gpui::{ClickEvent, Context, Div, MouseButton, MouseDownEvent, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::host_requests::uri_parts;
use super::notebook_editor::CellReveal;
use crate::commands::Outcome;
use crate::editor::TextDocument;
use crate::notebook::{self, History, Interactive, NotebookModel};

pub const INPUT_SCHEME: &str = "vscode-interactive-input";
pub const VIEW_TYPE: &str = "interactive";
/// interactiveEditor.ts `INPUT_CELL_VERTICAL_PADDING`, `INPUT_CELL_HORIZONTAL_PADDING_RIGHT`, `INPUT_EDITOR_PADDING`.
const INPUT_CELL_VERTICAL_PADDING: f32 = 8.;
const INPUT_CELL_HORIZONTAL_PADDING_RIGHT: f32 = 10.;
const INPUT_EDITOR_PADDING: f32 = 8.;
const INPUT_LINE_HEIGHT: f32 = 19.;

impl Workbench {
    pub(super) fn register_interactive_commands(&mut self) {
        self.commands.register("_interactive.open", |workbench, args, window, cx| workbench.open_interactive(&args, window, cx));
        self.commands.register("interactive.execute", |workbench, args, window, cx| workbench.execute_interactive_input(args.first(), window, cx));
        self.commands.register("interactive.input.clear", |workbench, _, _, cx| {
            if let Some(interactive) = workbench.active_interactive() {
                let input = interactive.input.clone();
                workbench.set_interactive_input(&input, "", cx);
            }
            Outcome::done()
        });
        self.commands.register("interactive.history.previous", |workbench, _, _, cx| {
            workbench.navigate_interactive_history(false, cx);
            Outcome::done()
        });
        self.commands.register("interactive.history.next", |workbench, _, _, cx| {
            workbench.navigate_interactive_history(true, cx);
            Outcome::done()
        });
        self.commands.register("interactive.scrollToTop", |workbench, _, _, _| {
            workbench.reveal_interactive_cell(false);
            Outcome::done()
        });
        self.commands.register("interactive.scrollToBottom", |workbench, _, _, _| {
            workbench.reveal_interactive_cell(true);
            Outcome::done()
        });
        self.commands.register("interactive.input.focus", |workbench, _, window, cx| {
            workbench.focus_interactive_input(window, cx);
            Outcome::done()
        });
        // executeActions.ts InterruptNotebook (`cancelNotebookCells`): the running cells go to the kernel's
        // `$cancelCells`, which interrupts a kernel with an interrupt handler.
        self.commands.register("notebook.interruptExecution", |workbench, _, _, _| {
            if let Some((_, kernel, uri, running)) = workbench.active_notebook_execution().filter(|(_, _, _, running)| !running.is_empty()) {
                drop(workbench.host.call("ExtHostNotebookKernels", "$cancelCells", vec![json!(kernel), uri, json!(running)]));
            }
            Outcome::done()
        });
        // executeActions.ts RevealRunningCellAction: the first running cell takes the focus as a container.
        self.commands.register("notebook.revealRunningCell", |workbench, _, window, cx| {
            let Some((key, _, _, running)) = workbench.active_notebook_execution() else { return Outcome::done() };
            let (Some(&handle), Some(tab)) = (running.first(), workbench.active_notebook_editor().map(|editor| editor.tab_model_id)) else { return Outcome::done() };
            let index = workbench.host.notebooks.model(&key).and_then(|model| model.index_of_handle(handle)).expect("a running cell is in its notebook");
            workbench.focus_notebook_cell(tab, handle, false, window, cx);
            workbench.reveal_notebook_cells(tab, index, index + 1, CellReveal::CenterIfOutside);
            Outcome::done()
        });
        // `interactive.history.focus`: the cell list takes the focus.
        self.commands.register("interactive.history.focus", |workbench, _, window, cx| {
            if let Some(editor) = workbench.active_notebook_editor_mut().filter(|editor| editor.input_focused) {
                editor.input_focused = false;
                window.focus(&workbench.notebook_views.list_focus, cx);
                workbench.editors_changed(cx);
            }
            Outcome::done()
        });
    }

    /// `_interactive.open(showOptions, resource, controllerId, title)`: an existing window of an `.interactive`
    /// resource shows again; otherwise a new `untitled:/Interactive-N.interactive` notebook (N past every open
    /// editor's) opens with the kernel preselected, and answers its URIs and notebook editor id.
    fn open_interactive(&mut self, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let show = args.first().cloned().unwrap_or(Value::Null);
        let (column, preserve_focus) = match &show {
            Value::Number(column) => (json!(column), false),
            options => (options["viewColumn"].clone(), options["preserveFocus"] == true),
        };
        let options = json!({ "position": column, "preserveFocus": preserve_focus, "pinned": true });
        let resource = args.get(1).filter(|resource| resource["path"].as_str().is_some_and(|path| path.ends_with(".interactive")));
        if let Some(existing) = resource.and_then(|resource| self.host.notebooks.interactives.iter().find(|interactive| interactive.notebook == notebook::uri_string(resource))) {
            let (key, input) = (existing.notebook.clone(), existing.input.clone());
            let model = self.host.notebooks.model(&key).expect("an interactive window's notebook is open");
            let uri = model.uri.clone();
            return Outcome::Ready(self.show_notebook(&key, &options, window, cx).map(|id| json!({ "notebookUri": uri, "inputUri": input, "notebookEditorId": id })));
        }
        let open: Vec<String> = self.editors.all_editors().map(|editor| editor.document().path.to_string_lossy().into_owned()).collect();
        let mut counter = 1;
        let (uri, input) = loop {
            let uri = json!({ "$mid": 1, "scheme": "untitled", "path": format!("/Interactive-{counter}.interactive") });
            if !open.contains(&notebook::uri_string(&uri)) {
                break (uri, json!({ "$mid": 1, "scheme": INPUT_SCHEME, "path": format!("/InteractiveInput-{counter}") }));
            }
            counter += 1;
        };
        let key = notebook::uri_string(&uri);
        if let Some(controller) = args.get(2).and_then(Value::as_str) {
            // `preselectKernelForNotebook`: bound without telling the kernels.
            if self.host.notebooks.kernels.matching(VIEW_TYPE, &key).0.iter().any(|kernel| kernel.id == controller) {
                drop(self.host.notebooks.kernels.select(Some(controller), VIEW_TYPE, &uri));
            }
        }
        let title = args.get(3).and_then(Value::as_str).map(str::to_owned);
        self.host.notebooks.interactives.retain(|interactive| interactive.notebook != key);
        self.host.notebooks.interactives.push(Interactive { notebook: key.clone(), input: input.clone(), title, history: History::new(""), shown: false, repl: false });
        self.host.notebooks.interactive_window_open = true;
        let opened = self.open_notebook(uri.clone(), VIEW_TYPE.into(), options, window, cx);
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn_in(window, async move |workbench, cx| {
            let editor_id = match opened {
                Outcome::Ready(result) => result,
                Outcome::Pending(receiver) => receiver.await.unwrap_or_else(|_| Err(Value::Null)),
            };
            let answer = workbench.update_in(cx, |workbench, window, cx| {
                let editor_id = editor_id?;
                workbench.create_interactive_input(&key, window, cx);
                if !preserve_focus {
                    workbench.focus_interactive_input(window, cx);
                }
                Ok(json!({ "notebookUri": uri, "inputUri": input, "notebookEditorId": editor_id }))
            });
            let _ = sender.send(answer.unwrap_or_else(|_| Err(Value::Null)));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `resolveInput`: the input model in the kernel's first language (else plain text), the host told first
    /// (`$willAddInteractiveDocument`).
    pub(super) fn create_interactive_input(&mut self, key: &str, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(interactive) = self.host.notebooks.interactives.iter().find(|interactive| interactive.notebook == key) else { return };
        let (input, notebook_uri) = (interactive.input.clone(), self.host.notebooks.model(key).map(|model| model.uri.clone()).unwrap_or_default());
        let (scheme, path) = uri_parts(&input).expect("an input URI");
        if self.editors.model(scheme, &path).is_some() {
            return;
        }
        self.host.send("ExtHostInteractive", "$willAddInteractiveDocument", vec![input.clone(), json!("\n"), json!(crate::languages::PLAINTEXT), notebook_uri]);
        let language = self.interactive_language(key);
        let document = TextDocument::from_text(scheme, &path, "", "utf8".into(), Some(&language), &self.languages, &self.settings.lock().unwrap());
        self.editors.open_model(document);
        self.editors_changed(cx);
    }

    /// The language of the input and of executed cells: the kernel's first supported language, else plain text.
    pub(super) fn interactive_language(&self, key: &str) -> String {
        let view_type = self.host.notebooks.model(key).map_or(VIEW_TYPE, |model| model.view_type.as_str());
        let kernel = self.host.notebooks.kernels.resolve(view_type, key);
        kernel.and_then(|kernel| kernel.supported_languages.first().cloned()).unwrap_or_else(|| crate::languages::PLAINTEXT.into())
    }

    /// `interactive.execute` and `repl.execute` (`executeReplInput`): the input's text, unless blank, becomes a code
    /// cell at the end (a REPL's: before its input cell) that is revealed, executed and selected; the input empties and
    /// its history moves on.
    pub(super) fn execute_interactive_input(&mut self, resource: Option<&Value>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let key = match resource.filter(|resource| resource.is_object()) {
            Some(resource) => notebook::uri_string(resource),
            None => match self.active_interactive() {
                Some(interactive) => interactive.notebook.clone(),
                None => return Outcome::done(),
            },
        };
        let Some(interactive) = self.host.notebooks.interactives.iter().find(|interactive| interactive.notebook == key) else { return Outcome::done() };
        let input = interactive.input.clone();
        let Ok((scheme, path)) = uri_parts(&input) else { return Outcome::done() };
        let Some(value) = self.editors.model(scheme, &path).map(|model| model.document().lines.join(model.document().eol)) else { return Outcome::done() };
        if value.trim().is_empty() {
            return Outcome::done();
        }
        let interactive = self.host.notebooks.interactives.iter_mut().find(|interactive| interactive.notebook == key).expect("found above");
        let repl = interactive.repl;
        interactive.history.replace_last(&value);
        interactive.history.reset_cursor();
        interactive.history.add("");
        self.set_interactive_input(&input, "", cx);
        let Some(model) = self.host.notebooks.model(&key) else { return Outcome::done() };
        let index = model.cells.len() - usize::from(repl);
        let cell = json!({ "cellKind": notebook::CODE, "mime": null, "language": self.interactive_language(&key), "source": value, "outputs": [], "metadata": {} });
        let edit = json!({ "editType": 1, "index": index, "count": 0, "cells": [cell] });
        if let Err(error) = self.apply_notebook_edits(&key, std::slice::from_ref(&edit), cx) {
            return Outcome::Ready(Err(error));
        }
        let handle = self.host.notebooks.model(&key).expect("still open").cells[index].handle;
        if let Some(tab) = self.interactive_tab(&key) {
            self.reveal_notebook_cells(tab, index, index + 1, CellReveal::InView);
        }
        let outcome = self.execute_notebook_cells(&key, vec![handle], cx);
        if let Some(editor) = self.host.notebooks.editors.iter_mut().find(|editor| editor.notebook == key) {
            editor.selections = vec![(index, index + 1)];
            editor.focus = index;
        }
        self.editors_changed(cx);
        window.refresh();
        outcome
    }

    /// `interactive.history.previous`/`next`: the input shows the history entry before or after the current one.
    fn navigate_interactive_history(&mut self, next: bool, cx: &mut Context<Self>) {
        let Some(key) = self.active_interactive().map(|interactive| interactive.notebook.clone()) else { return };
        let interactive = self.host.notebooks.interactives.iter_mut().find(|interactive| interactive.notebook == key).expect("the active one");
        let value = match next {
            true => interactive.history.next().to_owned(),
            false => interactive.history.previous().to_owned(),
        };
        let input = interactive.input.clone();
        // `getPreviousValue` answers null only without a history; an empty entry still replaces the input.
        self.set_interactive_input(&input, &value, cx);
    }

    /// `textModel.setValue`: typing into the input replaces the history's last entry (interactiveEditor.ts
    /// `onDidChangeContent`), unless the value is the one the cursor shows.
    fn set_interactive_input(&mut self, input: &Value, value: &str, cx: &mut Context<Self>) {
        let Ok((scheme, path)) = uri_parts(input) else { return };
        if self.editors.model(scheme, &path).is_none() {
            return;
        }
        self.update_model(scheme, &path, value, false);
        if let Some(model) = self.editors.model_mut(scheme, &path) {
            model.move_to_document_edge(true, false);
        }
        self.editors_changed(cx);
    }

    /// interactiveEditor.ts `onDidChangeContent` of the input: the history's last entry follows what is typed.
    pub(super) fn interactive_input_changed(&mut self) {
        for interactive in &mut self.host.notebooks.interactives {
            let Ok((scheme, path)) = uri_parts(&interactive.input) else { continue };
            let Some(model) = self.editors.model(scheme, &path) else { continue };
            let value = model.document().lines.join(model.document().eol);
            // replEditor.ts leaves the history alone while the input is empty.
            if interactive.history.current() != value && !(interactive.repl && value.is_empty()) {
                interactive.history.replace_last(&value);
            }
        }
    }

    fn reveal_interactive_cell(&mut self, last: bool) {
        let Some(key) = self.active_interactive().map(|interactive| interactive.notebook.clone()) else { return };
        let (Some(tab), Some(model)) = (self.interactive_tab(&key), self.host.notebooks.model(&key)) else { return };
        let count = model.cells.len();
        if count == 0 {
            return;
        }
        let index = if last { count - 1 } else { 0 };
        self.reveal_notebook_cells(tab, index, index + 1, CellReveal::InView);
    }

    /// interactiveEditor.ts and replEditor.ts `_scrollIfNecessary` on a cell's execution state change: the last list
    /// cell scrolls into view with `interactiveWindow.alwaysScrollOnNewCell`, else only when it is the last visible.
    pub(super) fn scroll_interactive_to_executed(&mut self, key: &str, cell_handle: u64) {
        let Some(interactive) = self.host.notebooks.interactives.iter().find(|interactive| interactive.notebook == key) else { return };
        let Some(model) = self.host.notebooks.model(key) else { return };
        let list_cells = model.cells.len() - usize::from(interactive.repl);
        let Some(index) = model.index_of_handle(cell_handle).filter(|index| index + 1 == list_cells) else { return };
        let editor = self.host.notebooks.editors.iter().find(|editor| editor.notebook == key);
        let visible_ranges = editor.and_then(|editor| editor.visible_ranges.as_ref());
        // `_cellAtBottom`: the cell is the last of the visible ranges.
        let at_bottom = visible_ranges.and_then(|ranges| ranges.iter().map(|(_, end)| end.saturating_sub(1)).max()) == Some(index);
        if self.setting("interactiveWindow.alwaysScrollOnNewCell") == false && !at_bottom {
            return;
        }
        if let Some(tab) = self.interactive_tab(key) {
            self.reveal_notebook_cells(tab, index, index + 1, CellReveal::InView);
        }
    }

    /// The input editor takes the focus: the group's code editor edits the input model.
    pub(super) fn focus_interactive_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_notebook_editor_mut() else { return };
        editor.input_focused = true;
        window.focus(&self.editor_focus, cx);
        self.editors_changed(cx);
    }

    /// The active notebook editor's notebook, its kernel's handle, its URI and its running cells' handles
    /// (`getCellExecutionsForNotebook`); None without a kernel.
    pub(super) fn active_notebook_execution(&self) -> Option<(String, u64, Value, Vec<u64>)> {
        let key = self.active_notebook_editor()?.notebook.clone();
        let model = self.host.notebooks.model(&key)?;
        let kernels = &self.host.notebooks.kernels;
        let kernel = kernels.resolve(&model.view_type, &key)?.handle;
        let running = model.cells.iter().map(|cell| cell.handle).filter(|handle| kernels.state(&key, *handle).is_some()).collect();
        Some((key, kernel, model.uri.clone(), running))
    }

    /// The interactive window of the active editor.
    pub(super) fn active_interactive(&self) -> Option<&Interactive> {
        let editor = self.active_notebook_editor()?;
        self.host.notebooks.interactives.iter().find(|interactive| interactive.notebook == editor.notebook)
    }

    /// The tab name of the interactive window of notebook `key`, None for another notebook.
    pub(super) fn interactive_name(&self, key: &str) -> Option<String> {
        let interactive = self.host.notebooks.interactives.iter().find(|interactive| interactive.notebook == key)?;
        Some(notebook::interactive_name(&self.host.notebooks.model(key)?.uri, interactive))
    }

    fn interactive_tab(&self, key: &str) -> Option<u32> {
        self.host.notebooks.editors.iter().find(|editor| editor.notebook == key).map(|editor| editor.tab_model_id)
    }

    /// InteractiveEditorInput `dispose` once no tab shows the window: the host lets the input go
    /// (`$willRemoveInteractiveDocument`) and so does the workbench, with the notebook. ReplEditorInput `dispose`
    /// reverts its notebook softly (no longer dirty, the cells kept) and `removeNotebookEditor` resets
    /// `mostRecentReplEditor` when it named it.
    pub(super) fn release_interactives(&mut self) {
        let shown: Vec<String> = self.host.notebooks.editors.iter().map(|editor| editor.notebook.clone()).collect();
        for interactive in &mut self.host.notebooks.interactives {
            interactive.shown |= shown.contains(&interactive.notebook);
        }
        let (kept, gone): (Vec<Interactive>, Vec<Interactive>) = std::mem::take(&mut self.host.notebooks.interactives).into_iter().partition(|interactive| !interactive.shown || shown.contains(&interactive.notebook));
        self.host.notebooks.interactives = kept;
        for interactive in gone {
            if interactive.repl {
                if self.host.notebooks.most_recent_repl.as_ref() == Some(&interactive.notebook) {
                    self.host.notebooks.most_recent_repl = None;
                }
                if let Some(model) = self.host.notebooks.model_mut(&interactive.notebook).filter(|model| model.dirty) {
                    model.dirty = false;
                    let uri = model.uri.clone();
                    self.host.send("ExtHostNotebookDocuments", "$acceptDirtyStateChanged", vec![uri, json!(false)]);
                }
                continue;
            }
            let Ok((scheme, path)) = uri_parts(&interactive.input) else { continue };
            if self.editors.model(scheme, &path).is_none() {
                continue;
            }
            let notebook_uri = self.host.notebooks.model(&interactive.notebook).map(|model| model.uri.clone()).unwrap_or_default();
            self.host.send("ExtHostInteractive", "$willRemoveInteractiveDocument", vec![interactive.input.clone(), notebook_uri]);
            self.editors.models.retain(|model| !model.document().is(scheme, &path));
        }
    }

    /// interactiveEditor.ts input cell container: under the list, a top border, the run button in the gutter and
    /// the one line input editor between its paddings.
    pub(super) fn interactive_input_cell(&self, model: &NotebookModel, window: &mut Window, cx: &mut Context<Self>) -> Option<Div> {
        let interactive = self.host.notebooks.interactives.iter().find(|interactive| interactive.notebook == model.uri_string())?;
        let (scheme, path) = uri_parts(&interactive.input).ok()?;
        let input = self.editors.model(scheme, &path)?;
        // `.input-cell-container:focus-within`: the keyboard is in the input editor.
        let focused = self.editor_focus.is_focused(window) && self.editors.active().is_some_and(|active| std::ptr::eq(active, input));
        let editor_height = INPUT_LINE_HEIGHT + INPUT_EDITOR_PADDING * 2.;
        let left = notebook::layout::CODE_CELL_LEFT_MARGIN + notebook::layout::CELL_RUN_GUTTER;
        let inactive_border = self.color("notebook.inactiveFocusedCellBorder");
        let top = INPUT_CELL_VERTICAL_PADDING;
        let command = if interactive.repl { "repl.execute" } else { "interactive.execute" };
        let run = div()
            .id("interactive-execute")
            .absolute()
            .left(px(notebook::layout::CODE_CELL_LEFT_MARGIN))
            .top(px(top + INPUT_EDITOR_PADDING - 2.))
            .w(px(notebook::layout::CELL_RUN_GUTTER))
            .flex()
            .justify_center()
            .cursor_pointer()
            .child(self.icon("play", 22., 16., self.color("icon.foreground")))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| drop(workbench.execute_command(command, Vec::new(), window, cx))));
        // interactiveEditor.css: the editor's 1px outline, `notebook.focusedCellBorder` while focused, and its
        // `notebook.cellEditorBackground`.
        let background = self.theme.color("notebook.cellEditorBackground").unwrap_or(self.color("editor.background"));
        let frame = div()
            .absolute()
            .left(px(left - 1.))
            .right(px(INPUT_CELL_HORIZONTAL_PADDING_RIGHT - 1.))
            .top(px(top - 1.))
            .h(px(editor_height + 2.))
            .border_1()
            .border_color(if focused { self.color("notebook.focusedCellBorder") } else { inactive_border })
            .bg(background);
        let editor = div()
            .absolute()
            .left(px(left))
            .right(px(INPUT_CELL_HORIZONTAL_PADDING_RIGHT))
            .top(px(top + INPUT_EDITOR_PADDING))
            .h(px(INPUT_LINE_HEIGHT))
            .flex()
            .overflow_hidden()
            .cursor_text()
            .on_mouse_down(MouseButton::Left, cx.listener(|workbench, _: &MouseDownEvent, window, cx| workbench.focus_interactive_input(window, cx)))
            .child(self.embedded_code_editor(input, window, cx))
            .children(self.interactive_input_hint(input, window));
        // The `gutter` focus indicator: a 3px bar 4px in, `notebook.focusedCellBorder` while focused.
        let indicator = div()
            .absolute()
            .left(px(notebook::layout::FOCUS_INDICATOR_LEFT_MARGIN))
            .top(px(top))
            .w(px(3.))
            .h(px(editor_height))
            .rounded(px(2.))
            .bg(if focused { self.color("notebook.focusedCellBorder") } else { inactive_border });
        let container = div()
            .relative()
            .flex_none()
            .h(px(INPUT_LINE_HEIGHT + INPUT_CELL_VERTICAL_PADDING * 2. + INPUT_EDITOR_PADDING * 2.))
            .border_t_1()
            .border_color(inactive_border)
            .child(indicator)
            .child(run)
            .child(frame)
            .child(editor);
        Some(container)
    }

    /// ReplInputHintContentWidget at line 1, column 1 of an empty input (`_updateInputHint`): "Press <key> to
    /// execute." in `input.placeholderForeground`, italic around the keybinding label, unless
    /// `interactiveWindow.showExecutionHint` is off or line 1 has injected text.
    fn interactive_input_hint(&self, input: &crate::editor::OpenEditor, window: &mut Window) -> Option<Div> {
        let document = input.document();
        let empty = document.lines.len() == 1 && document.lines[0].is_empty();
        if !empty || self.setting("interactiveWindow.showExecutionHint") == false || input.injected_text.iter().any(|text| text.position.line == 0) {
            return None;
        }
        let keybinding = self.input_hint_keybinding()?;
        let metrics = self.editor_metrics(input, window);
        let italic = |text: &str| div().italic().child(text.to_owned());
        let placeholder = self.color("input.placeholderForeground");
        // keybindingLabel.css without styles (the widget passes none): a 1px border in the text color, no background.
        let caps = keybinding.key_caps();
        let last = caps.len().saturating_sub(1);
        let caps = caps.into_iter().enumerate().map(move |(index, cap)| {
            let key = div().px(px(5.)).py(px(3.)).line_height(px(10.)).rounded(px(3.)).border_1().border_color(placeholder).text_size(px(11.)).child(cap);
            key.when(index > 0, |key| key.ml(px(2.))).when(index < last, |key| key.mr(px(2.)))
        });
        Some(
            div()
                .absolute()
                .top_0()
                .left(px(metrics.content_left() + 4.))
                .h(px(metrics.line_height))
                .flex()
                .items_center()
                .whitespace_nowrap()
                .font_family(super::editor_view::EDITOR_FONT)
                .text_size(px(metrics.font_size))
                .text_color(placeholder)
                .child(italic("Press "))
                .child(div().flex().children(caps))
                .child(italic(" to execute. ")),
        )
    }

    /// replInputHintContentWidget.ts `getKeybinding`: `interactive.execute`'s lone Shift+Enter with
    /// `interactiveWindow.executeWithShiftEnter`, else its lone Enter or `python.execInREPLEnter`'s, else its first.
    pub(super) fn input_hint_keybinding(&self) -> Option<crate::keybindings::Chord> {
        let resolver = &self.keybindings.resolver;
        let lone = |command: &str, label: &str| resolver.lookup_keybindings(command).into_iter().find(|item| item.keybinding.len() == 1 && item.keybinding[0].user_settings_label() == label).map(|item| item.keybinding[0].clone());
        let found = match self.setting("interactiveWindow.executeWithShiftEnter") == true {
            true => lone("interactive.execute", "shift+enter"),
            false => lone("interactive.execute", "enter").or_else(|| lone("python.execInREPLEnter", "enter")),
        };
        found.or_else(|| resolver.lookup_keybindings("interactive.execute").first().and_then(|item| item.keybinding.first().cloned()))
    }
}

#[cfg(test)]
mod tests;
