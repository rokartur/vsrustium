//! The cell status bar items: contributed ones (mainThreadNotebook.ts `$registerNotebookCellStatusBarItemProvider`,
//! contributedStatusBarItemController.ts) and the built-in execution state and timer
//! (executionStatusBarItemController.ts), drawn left and right of a cell's status bar (cellStatusPart.ts).

use std::collections::HashMap;

use futures::channel::oneshot;
use gpui::{ClickEvent, Context, Div, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use crate::notebook::kernels::ExecutionState;
use crate::notebook::{self, NotebookModel};

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
        items.extend(self.notebook_views.status_bars.items(&key, cell.handle).iter().cloned());
        // statusBarProviders.ts `CellStatusBarLanguagePickerProvider`. shortcut: without its command, the port has no
        // notebook.cell.changeLanguage picker yet; add it with the language picker for cells.
        items.push(json!({ "text": self.languages.name(&cell.language), "alignment": 2, "priority": -9007199254740991i64 }));
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
