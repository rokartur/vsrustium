//! cellDnd.ts: a cell dragged by its left focus indicator column and dropped above or below another cell (the half
//! of the cell under the pointer), its selection range moving with it; Alt copies (`isCopy` on macOS).

use gpui::{Context, DragMoveEvent, Render, SharedString, Window, div, prelude::*, px};
use serde_json::json;

use super::super::Workbench;
use super::CellRow;
use crate::notebook::layout;

/// The dragged cell, drawn under the pointer as its first line (`CodeCellDragImageRenderer`).
#[derive(Clone)]
pub(in crate::workbench) struct CellDrag {
    tab: u32,
    handle: u64,
    label: SharedString,
    background: gpui::Rgba,
    foreground: gpui::Rgba,
}

impl Render for CellDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().px(px(16.)).py(px(12.)).bg(self.background).text_color(self.foreground).text_size(px(13.)).opacity(0.8).child(self.label.clone())
    }
}

impl Workbench {
    /// The left focus indicator column of a cell (`registerDragHandle`): pressed and moved, it drags the cell.
    pub(super) fn cell_drag_handle(&self, tab: u32, row: &CellRow, first_line: String) -> Option<gpui::Stateful<gpui::Div>> {
        if self.setting("notebook.dragAndDropEnabled") == false {
            return None;
        }
        let drag = CellDrag { tab, handle: row.handle, label: first_line.into(), background: self.color("editor.background"), foreground: self.foreground_or("editor.foreground") };
        Some(
            div()
                .id(("notebook-cell-drag-handle", row.handle as usize))
                .debug_selector({
                    let handle = row.handle;
                    move || format!("notebook-cell-drag-handle-{handle}")
                })
                .absolute()
                .left_0()
                .top_0()
                .w(px(layout::CODE_CELL_LEFT_MARGIN))
                .h(px(row.layout.height - layout::BOTTOM_TOOLBAR_GAP))
                .cursor_grab()
                .on_drag(drag.clone(), |drag, _, _, cx| cx.new(|_| drag.clone())),
        )
    }

    /// `onCellDragover`: the insertion indicator above or below the cell under the pointer, none over the dragged cell.
    pub(super) fn cell_drag_move(&mut self, tab: u32, row_handle: u64, event: &DragMoveEvent<CellDrag>, cx: &mut Context<Self>) {
        let drag = event.drag(cx);
        if drag.tab != tab {
            return;
        }
        let dragged = drag.handle;
        let view = self.notebook_views.views.entry(tab).or_default();
        let over = event.bounds.contains(&event.event.position);
        let target = match over && dragged != row_handle {
            true => Some((dragged, row_handle, event.event.position.y > event.bounds.center().y)),
            false => None,
        };
        let mine = view.drop_target.is_some_and(|(_, handle, _)| handle == row_handle);
        if over || mine {
            if view.drop_target != target {
                view.drop_target = target;
                cx.notify();
            }
        }
    }

    /// `onCellDrop` and `performCellDropEdits`: the dragged cell, or the selection range holding it, moves (or with Alt
    /// is copied) above or below the cell it was dropped on.
    pub(super) fn drop_cell(&mut self, tab: u32, over_handle: u64, copy: bool, cx: &mut Context<Self>) {
        let Some(view) = self.notebook_views.views.get_mut(&tab) else { return };
        let Some((dragged_handle, target, below)) = view.drop_target.take().filter(|(_, handle, _)| *handle == over_handle) else { return };
        cx.notify();
        let Some(editor) = self.host.notebooks.editor(tab) else { return };
        let key = editor.notebook.clone();
        let Some(model) = self.host.notebooks.model(&key) else { return };
        let (Some(dragged), Some(over)) = (model.index_of_handle(dragged_handle), model.index_of_handle(target)) else { return };
        // getNextVisibleCellIndex: below a collapsed header is after its section.
        let rows = self.notebook_rows(tab, model);
        let mut to = over;
        if below {
            to = rows.iter().find(|row| row.index > over && !row.hidden).map_or(model.cells.len(), |row| row.index);
        }
        // shortcut: the one selection range holding the dragged cell moves, not several ranges; upgrade with multi-range selections.
        let (start, end) = editor.selections.iter().copied().find(|(start, end)| *start <= dragged && dragged < *end).unwrap_or((dragged, dragged + 1));
        if copy {
            let cells: Vec<_> = model.cells[start..end].iter().map(|cell| self.cell_data(cell)).collect();
            let focus = if to <= start { to + dragged - start } else { dragged + to - start };
            self.edit_notebook_cells(&key, vec![json!({ "editType": 1, "index": to, "count": 0, "cells": cells })], Some(focus), cx);
            return;
        }
        if (start..=end).contains(&to) {
            return;
        }
        let length = end - start;
        let new_index = if end <= to { to - length } else { to };
        self.edit_notebook_cells(&key, vec![json!({ "editType": 6, "index": start, "length": length, "newIdx": new_index })], Some(new_index + dragged - start), cx);
    }

    /// The insertion indicator (`cell-list-insertion-indicator`): 2px across the list, half the bottom toolbar gap
    /// below the cell's top or bottom.
    pub(super) fn cell_drop_indicator(&self, tab: u32, rows: &[CellRow], scroll_top: f32) -> Option<gpui::Div> {
        let (_, handle, below) = self.notebook_views.views.get(&tab)?.drop_target?;
        let row = rows.iter().find(|row| row.handle == handle)?;
        let top = if below { row.layout.top + row.layout.height } else { row.layout.top };
        Some(div().absolute().left_0().right_0().top(px(top - scroll_top + layout::BOTTOM_TOOLBAR_GAP / 2.)).h(px(2.)).bg(self.color("notebook.cellInsertionIndicator")))
    }
}
