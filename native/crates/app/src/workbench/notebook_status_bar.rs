//! The cell status bar items: contributed ones (mainThreadNotebook.ts `$registerNotebookCellStatusBarItemProvider`,
//! contributedStatusBarItemController.ts) and the built-in execution state and timer
//! (executionStatusBarItemController.ts), drawn left and right of a cell's status bar (cellStatusPart.ts).

use std::collections::HashMap;

use futures::channel::oneshot;
use gpui::{ClickEvent, Context, Div, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::editor_status::{StatusPick, label_item};
use crate::quick_input::{Entry, Item, Kind, QuickInput};
use crate::notebook::kernels::ExecutionState;
use crate::notebook::{self, NotebookModel};

/// notebookBrowser.ts `CHANGE_CELL_LANGUAGE`.
const CHANGE_CELL_LANGUAGE: &str = "notebook.cell.changeLanguage";

struct Provider {
    handle: u64,
    event_handle: Option<u64>,
    view_type: String,
}

/// The cell's items of the last answer, the `cacheId`s to release when they are replaced, and the request count
/// that drops an answer overtaken by a newer request.
#[derive(Default)]
struct CellItems {
    items: Vec<Value>,
    caches: Vec<Value>,
    request: u64,
}

#[derive(Default)]
pub struct CellStatusBars {
    providers: Vec<Provider>,
    /// By notebook URI string and cell handle.
    cells: HashMap<(String, u64), CellItems>,
}

impl CellStatusBars {
    /// The contributed items of a cell, as its providers last answered.
    pub(super) fn items(&self, key: &str, handle: u64) -> &[Value] {
        self.cells.get(&(key.to_owned(), handle)).map_or(&[], |cell| &cell.items)
    }
}

impl Workbench {
    pub(super) fn register_cell_status_bar_provider(&mut self, handle: u64, event_handle: Option<u64>, view_type: String, cx: &mut Context<Self>) {
        self.notebook_views.status_bars.providers.push(Provider { handle, event_handle, view_type });
        self.update_all_cell_status_bars(cx);
    }

    pub(super) fn unregister_cell_status_bar_provider(&mut self, handle: u64, cx: &mut Context<Self>) {
        self.notebook_views.status_bars.providers.retain(|provider| provider.handle != handle);
        self.update_all_cell_status_bars(cx);
    }

    /// `$emitCellStatusBarEvent`: the provider's `onDidChangeCellStatusBarItems`.
    pub(super) fn cell_status_bar_event(&mut self, event_handle: u64, cx: &mut Context<Self>) {
        if self.notebook_views.status_bars.providers.iter().any(|provider| provider.event_handle == Some(event_handle)) {
            self.update_all_cell_status_bars(cx);
        }
    }

    /// `_updateEverything`: every cell of the notebooks in editors asks the providers again.
    pub(super) fn update_all_cell_status_bars(&mut self, cx: &mut Context<Self>) {
        let mut keys: Vec<String> = self.host.notebooks.editors.iter().map(|editor| editor.notebook.clone()).collect();
        keys.dedup();
        for key in keys {
            self.update_notebook_cell_status_bars(&key, cx);
        }
    }

    /// A notebook shown or edited (cells, outputs, metadata): its cells ask the providers again.
    pub(super) fn update_notebook_cell_status_bars(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.notebook_views.status_bars.providers.is_empty() {
            return;
        }
        let Some(model) = self.host.notebooks.model(key) else { return };
        let handles: Vec<u64> = model.cells.iter().map(|cell| cell.handle).collect();
        for handle in handles {
            self.update_cell_status_bar(key, handle, cx);
        }
    }

    /// The typed text of a cell changed: its items are asked again (`onDidChangeContent`).
    pub(super) fn cell_status_bar_content_changed(&mut self, scheme: &str, path: &std::path::Path, cx: &mut Context<Self>) {
        if scheme != notebook::CELL_SCHEME {
            return;
        }
        let found = self.host.notebooks.models.iter().find_map(|model| {
            let cell = model.cells.iter().find(|cell| super::host_requests::uri_parts(&cell.uri).is_ok_and(|(_, cell_path)| cell_path == path))?;
            Some((model.uri_string(), cell.handle))
        });
        if let Some((key, handle)) = found {
            self.update_cell_status_bar(&key, handle, cx);
        }
    }

    /// `CellStatusBarHelper._update`: the providers of the notebook's type answer for the cell's index; the answer
    /// replaces the cell's items and releases the lists it replaced.
    pub(super) fn update_cell_status_bar(&mut self, key: &str, handle: u64, cx: &mut Context<Self>) {
        let Some(model) = self.host.notebooks.model(key) else { return };
        let Some(index) = model.cells.iter().position(|cell| cell.handle == handle) else { return };
        let (uri, view_type) = (model.uri.clone(), model.view_type.clone());
        let handles: Vec<u64> = self.notebook_views.status_bars.providers.iter().filter(|provider| provider.view_type == view_type || provider.view_type == "*").map(|provider| provider.handle).collect();
        let cell_key = (key.to_owned(), handle);
        let cell = self.notebook_views.status_bars.cells.entry(cell_key.clone()).or_default();
        cell.request += 1;
        let request = cell.request;
        let calls: Vec<oneshot::Receiver<Result<Value, Value>>> =
            handles.iter().map(|provider| self.host.call_cancellable("ExtHostNotebook", "$provideNotebookCellStatusBarItems", vec![json!(provider), uri.clone(), json!(index)])).collect();
        cx.spawn(async move |workbench, cx| {
            let mut items = Vec::new();
            let mut caches = Vec::new();
            for call in calls {
                let Ok(Ok(list)) = call.await else { continue };
                items.extend(list["items"].as_array().into_iter().flatten().cloned());
                if !list["cacheId"].is_null() {
                    caches.push(list["cacheId"].clone());
                }
            }
            let _ = workbench.update(cx, |workbench, cx| {
                let cell = workbench.notebook_views.status_bars.cells.entry(cell_key).or_default();
                let released = match cell.request == request {
                    true => {
                        cell.items = items;
                        std::mem::replace(&mut cell.caches, caches)
                    }
                    false => caches,
                };
                for cache in released {
                    workbench.host.send("ExtHostNotebook", "$releaseNotebookCellStatusBarItems", vec![cache]);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// cellStatusPart.ts `updateRenderedItems`: the items by priority, highest first, the left ones after a 10px
    /// margin, the right ones lowest priority first; `onlyShowWhenActive` ones only on the focused cell.
    pub(super) fn cell_status_bar(&self, model: &NotebookModel, index: usize, focused: bool, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let cell = &model.cells[index];
        let key = model.uri_string();
        let mut items = self.builtin_cell_status_items(&key, cell, window);
        // interactiveEditor.ts and replEditor.ts take only the execution state and timer contributions, not the
        // status bar controller with the language picker and the extensions' providers.
        if !self.host.notebooks.interactives.iter().any(|interactive| interactive.notebook == key) {
            items.extend(self.notebook_views.status_bars.items(&key, cell.handle).iter().cloned());
            items.extend(self.cell_language_status_items(cell));
        }
        items.retain(|item| focused || item["onlyShowWhenActive"] != true);
        items.sort_by(|a, b| b["priority"].as_f64().unwrap_or(0.).total_cmp(&a["priority"].as_f64().unwrap_or(0.)));
        let (left, mut right): (Vec<Value>, Vec<Value>) = items.into_iter().partition(|item| item["alignment"] != 2);
        right.reverse();
        let render = |items: Vec<Value>, side: &'static str, cx: &mut Context<Self>| {
            items.into_iter().enumerate().map(|(position, item)| self.cell_status_item(item, cell.handle, index, (side, position), cx)).collect::<Vec<_>>()
        };
        let left = render(left, "left", cx);
        let right = render(right, "right", cx);
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(notebook::layout::STATUS_BAR_HEIGHT))
            .flex()
            .items_center()
            .text_size(px(12.))
            .text_color(self.color("foreground"))
            .child(div().flex_1().flex().overflow_hidden().ml(px(10.)).children(left))
            .child(div().flex().justify_end().overflow_hidden().children(right))
    }

    /// statusBarProviders.ts `CellStatusBarLanguagePickerProvider`: the cell's language, which opens the language
    /// picker, after a warning that searches extensions for a language no extension registers.
    fn cell_language_status_items(&self, cell: &notebook::Cell) -> Vec<Value> {
        let mut items = Vec::new();
        let display = match cell.kind == notebook::MARKUP {
            true => "markdown".to_owned(),
            false if self.languages.id_by_name(&cell.language).is_some() => self.languages.name(&cell.language).to_owned(),
            false => {
                items.push(json!({
                    "text": "$(dialog-warning)",
                    "command": { "id": "workbench.extensions.search", "arguments": [format!("@tag:{}", cell.language)], "title": "Search Extensions" },
                    "tooltip": format!("Unknown cell language. Click to search for '{}' extensions", cell.language),
                    "alignment": 2,
                    "priority": -9007199254740990i64,
                }));
                cell.language.clone()
            }
        };
        items.push(json!({ "text": display, "command": CHANGE_CELL_LANGUAGE, "tooltip": "Select Cell Language Mode", "alignment": 2, "priority": -9007199254740991i64 }));
        items
    }

    /// editActions.ts `ChangeCellLanguageAction`: a `{ start, end }` range with a language sets the language of
    /// the range's first cell; a cell context (a status bar click) or nothing (Cmd+K M) picks the language of that
    /// cell or the focused one.
    pub(super) fn change_cell_language(&mut self, args: &[Value], window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_notebook_editor() else { return };
        let key = editor.notebook.clone();
        let model = self.host.notebooks.model(&key).expect("an editor shows an open notebook");
        let first = args.first().unwrap_or(&Value::Null);
        let index = match (first["start"].as_u64(), first["end"].as_u64()) {
            (Some(start), Some(end)) if start < end && (start as usize) < model.cells.len() => start as usize,
            (Some(_), Some(_)) => return,
            _ => match first["cell"]["handle"].as_u64() {
                Some(handle) => match model.index_of_handle(handle) {
                    Some(index) => index,
                    None => return,
                },
                None => editor.focus,
            },
        };
        let Some(cell) = model.cells.get(index) else { return };
        let handle = cell.handle;
        if let Some(language) = args.get(1).and_then(Value::as_str) {
            return self.set_cell_language(&key, handle, language, window, cx);
        }
        self.show_cell_language_picker(&key, handle, window, cx);
    }

    /// `showLanguagePicker`: Auto Detect, then markdown and the cell's language, then the other languages of the
    /// notebook's kernel (else of every matching kernel, else every registered language) by `(id)`.
    fn show_cell_language_picker(&mut self, key: &str, handle: u64, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.host.notebooks.model(key).expect("an editor shows an open notebook");
        let cell = &model.cells[model.index_of_handle(handle).expect("picked from the notebook")];
        let kernel_languages = |kernel: &notebook::kernels::Kernel| match kernel.supported_languages.is_empty() {
            true => self.languages.ids().to_vec(),
            false => kernel.supported_languages.clone(),
        };
        let (all, selected) = self.host.notebooks.kernels.matching(&model.view_type, key);
        let mut languages = match selected {
            Some(kernel) => kernel_languages(kernel),
            None => all.into_iter().flat_map(kernel_languages).collect(),
        };
        if languages.is_empty() {
            languages = self.languages.ids().to_vec();
        }
        languages.push("markdown".into());
        let mut seen = std::collections::HashSet::new();
        languages.retain(|language| seen.insert(language.clone()));
        let (mut top, mut main) = (Vec::new(), Vec::new());
        for language in languages {
            let current = match cell.kind == notebook::MARKUP {
                true => language == "markdown",
                false => language == cell.language,
            };
            if !self.languages.is_registered(&language) {
                continue;
            }
            let description = match current {
                true => format!("({language}) - Current Language"),
                false => format!("({language})"),
            };
            let item = Item { description: Some(description), language_icon: Some(language.clone()), ..label_item(self.languages.name(&language)) };
            let pick = StatusPick::CellLanguage { notebook: key.to_owned(), handle, language: Some(language.clone()) };
            match language == "markdown" || language == cell.language {
                true => top.push((Entry::Item(item), pick)),
                false => main.push((item, pick)),
            }
        }
        main.sort_by(|(a, _), (b, _)| a.description.cmp(&b.description));
        let mut rows = vec![
            (Entry::Item(label_item("Auto Detect")), StatusPick::CellLanguage { notebook: key.to_owned(), handle, language: None }),
            (Entry::Separator(Some("languages (identifier)".into())), StatusPick::Nothing),
        ];
        rows.extend(top);
        rows.push((Entry::Separator(None), StatusPick::Nothing));
        rows.extend(main.into_iter().map(|(item, pick)| (Entry::Item(item), pick)));
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select Language Mode".into());
        self.show_status_picker(input, rows, window, cx);
    }

    /// A pick of the cell language picker: Auto Detect runs language detection on the cell's text first.
    pub(super) fn accept_cell_language(&mut self, key: &str, handle: u64, language: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(language) = language {
            return self.set_cell_language(key, handle, &language, window, cx);
        }
        let Some(cell) = self.host.notebooks.model(key).and_then(|model| model.index_of_handle(handle).map(|index| model.cells[index].uri.clone())) else { return };
        let Ok((scheme, path)) = super::host_requests::uri_parts(&cell) else { return };
        let detection = self.detect_language(scheme, &path);
        let key = key.to_owned();
        cx.spawn_in(window, async move |workbench, cx| {
            let detection = detection.await;
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                if let Some(language) = workbench.detected_language(detection) {
                    workbench.set_cell_language(&key, handle, &language, window, cx);
                }
            });
        })
        .detach();
    }

    /// `setCellToLanguage`: markdown makes a code cell a markup cell and another language a markup cell a code cell
    /// (`changeCellToKind`: the cell replaced by one of the other kind with its text, outputs and metadata), else
    /// the cell's language changes; one step on the notebook's undo stack.
    fn set_cell_language(&mut self, key: &str, handle: u64, language: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.host.notebooks.model(key) else { return };
        let Some(index) = model.index_of_handle(handle) else { return };
        let cell = &model.cells[index];
        let to_markup = language == "markdown" && cell.language != "markdown";
        let to_code = language != "markdown" && cell.kind == notebook::MARKUP;
        let edit = match (to_markup, to_code) {
            (true, _) if cell.kind == notebook::MARKUP => return,
            (true, _) | (_, true) => {
                let mut data = self.cell_data(cell);
                data["cellKind"] = json!(if to_markup { notebook::MARKUP } else { notebook::CODE });
                data["language"] = json!(language);
                if to_markup {
                    data["mime"] = json!("text/markdown");
                }
                json!({ "editType": 1, "index": index, "count": 1, "cells": [data] })
            }
            _ => json!({ "editType": 4, "index": index, "language": language }),
        };
        let Ok(applied) = self.apply_notebook_edits(key, std::slice::from_ref(&edit), cx) else { return };
        if !applied.undo.is_empty() {
            self.push_cells_undo(key, applied.undo, vec![edit]);
        }
        if to_markup {
            let handle = self.host.notebooks.model(key).expect("still open").cells[index].handle;
            self.edit_markup_cell(handle);
            window.focus(&self.editor_focus, cx);
        }
        cx.notify();
    }

    /// `ExecutionStateCellStatusBarItem` and `TimerCellStatusBarItem` of a code cell.
    fn builtin_cell_status_items(&self, key: &str, cell: &notebook::Cell, window: &mut Window) -> Vec<Value> {
        if cell.kind != notebook::CODE {
            return Vec::new();
        }
        let state = self.host.notebooks.kernels.state(key, cell.handle);
        let internal = &cell.internal_metadata;
        let icon = match state {
            None if internal["lastRunSuccess"] == true => Some(("$(notebook-state-success)", "Success")),
            None if internal["lastRunSuccess"] == false => Some(("$(notebook-state-error)", "Failed")),
            None => None,
            Some(ExecutionState::Pending | ExecutionState::Unconfirmed) => Some(("$(notebook-state-pending)", "Pending")),
            Some(ExecutionState::Executing) => Some(("$(notebook-state-executing~spin)", "Executing")),
        };
        let mut items: Vec<Value> = icon.into_iter().map(|(text, tooltip)| json!({ "text": text, "tooltip": tooltip, "alignment": 1, "priority": 9007199254740991i64 })).collect();
        let start = internal["runStartTime"].as_f64();
        let duration = match state {
            Some(ExecutionState::Executing) => start.map(|start| {
                window.request_animation_frame();
                now_millis() - start + internal["runStartTimeAdjustment"].as_f64().unwrap_or(0.)
            }),
            Some(_) => None,
            None => start.zip(internal["runEndTime"].as_f64()).map(|(start, end)| end - start),
        };
        if let Some(duration) = duration {
            items.push(json!({ "text": format_cell_duration(duration.max(0.) as u64), "alignment": 1, "priority": 9007199254740986i64 }));
        }
        items
    }

    fn cell_status_item(&self, item: Value, handle: u64, index: usize, (side, position): (&'static str, usize), cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let text = item["text"].as_str().unwrap_or_default().replace('\n', " ");
        let foreground = match &item["color"] {
            Value::String(color) => crate::theme::parse_hex(color),
            Value::Object(theme_color) => theme_color["id"].as_str().and_then(|id| self.theme.color(id)),
            _ => None,
        };
        let command = item["command"].clone();
        let mut row = div()
            .id((gpui::SharedString::from(format!("cell-status-{side}-{handle}")), position))
            .h(px(21.))
            .mx(px(3.))
            .px(px(3.))
            .flex()
            .items_center()
            .whitespace_nowrap()
            .overflow_hidden()
            .when_some(foreground, |row, color| row.text_color(color))
            .when_some(item["opacity"].as_str().and_then(|opacity| opacity.parse::<f32>().ok()), |row, opacity| row.opacity(opacity));
        let mut rest = text.as_str();
        while let Some((before, icon, after)) = crate::markdown::split_icon(rest) {
            let color = match icon {
                "notebook-state-success" => self.color("notebookStatusSuccessIcon.foreground"),
                "notebook-state-error" => self.color("notebookStatusErrorIcon.foreground"),
                _ => foreground.unwrap_or(self.color("foreground")),
            };
            if !before.is_empty() {
                row = row.child(before.to_owned());
            }
            row = row.child(self.icon(codicon_of(icon), 14., 14., color));
            rest = after;
        }
        if !rest.is_empty() {
            row = row.child(rest.to_owned());
        }
        if command.is_null() {
            return row;
        }
        // `CellStatusBarItem.executeCommand`: a command without arguments gets the cell's context.
        row.cursor_pointer().hover(|row| row.bg(self.color("toolbar.hoverBackground"))).on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
            let (id, mut args) = match &command {
                Value::String(id) => (id.clone(), Vec::new()),
                command => (command["id"].as_str().unwrap_or_default().to_owned(), command["arguments"].as_array().cloned().unwrap_or_default()),
            };
            if args.is_empty() {
                args.push(json!({ "$mid": 13, "cell": { "handle": handle, "index": index } }));
            }
            cx.stop_propagation();
            let _ = workbench.execute_command(&id, args, window, cx);
        }))
    }
}

/// notebookIcons.ts: the registered notebook icons are codicons by another name.
fn codicon_of(icon: &str) -> &str {
    match icon {
        "notebook-state-success" => "check",
        "notebook-state-error" => "error",
        "notebook-state-pending" => "clock",
        "notebook-state-executing" => "sync",
        icon => icon,
    }
}

/// executionStatusBarItemController.ts `formatCellDuration` without milliseconds.
fn format_cell_duration(duration: u64) -> String {
    let minutes = duration / 1000 / 60;
    let seconds = (duration / 1000) % 60;
    let tenths = (duration % 1000) / 100;
    match minutes {
        0 => format!("{seconds}.{tenths}s"),
        _ => format!("{minutes}m {seconds}.{tenths}s"),
    }
}

fn now_millis() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0., |duration| duration.as_millis() as f64)
}

#[cfg(test)]
mod tests {
    #[test]
    fn durations_read_as_the_timer_shows_them() {
        assert_eq!(super::format_cell_duration(40), "0.0s");
        assert_eq!(super::format_cell_duration(1234), "1.2s");
        assert_eq!(super::format_cell_duration(61500), "1m 1.5s");
    }
}
