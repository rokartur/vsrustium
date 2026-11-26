//! notebookOutline.ts in the Outline view: `NotebookOutlinePaneProvider` rows of the active notebook editor (markdown
//! headers by default, `notebook.outline.*`), the active cell's entry selected, a click reveals its cell at the top.

use gpui::{AnyElement, ClickEvent, Context, Window, div, prelude::*, px};

use super::Workbench;
use super::notebook_editor::CellReveal;
use crate::notebook::outline::{self, Entry};

/// `NotebookOutlineVirtualDelegate.getHeight`.
const ROW_HEIGHT: f32 = 22.;

/// A shown entry: its index in the outline, its depth and whether it has shown children.
struct Row {
    entry: usize,
    depth: usize,
    has_children: bool,
}

impl Workbench {
    /// The Outline view's body while a notebook editor is active.
    pub(super) fn notebook_outline_pane_body(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let editor = self.active_notebook_editor()?;
        let tab = editor.tab_model_id;
        let model = self.host.notebooks.model(&editor.notebook)?;
        let entries = self.notebook_outline(model);
        let (headers_only, code_cells) = (self.setting("notebook.outline.showMarkdownHeadersOnly") != false, self.setting("notebook.outline.showCodeCells") == true);
        let rows = self.notebook_outline_rows(tab, &entries, headers_only, code_cells);
        let selected = self.outline_pane.view_state.follow_cursor.then(|| active_shown_entry(&entries, editor.focus, headers_only, code_cells)).flatten();
        let focused = self.outline_pane.focus.is_focused(window);
        let (selection, selection_foreground) = match focused {
            true => (self.color("list.activeSelectionBackground"), self.theme.color("list.activeSelectionForeground")),
            false => (self.color("list.inactiveSelectionBackground"), self.theme.color("list.inactiveSelectionForeground")),
        };
        let indent = self.tree_indent();
        let mut elements = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let entry = &entries[row.entry];
            let is_selected = selected == Some(row.entry);
            let icon_color = match focused && is_selected {
                true => self.list_selection_icon_color(),
                false => self.color("icon.foreground"),
            };
            let id = row.entry.to_string();
            let twistie = match row.has_children {
                true => {
                    let chevron = if self.outline_row_expanded(tab, &id) { "chevron-down" } else { "chevron-right" };
                    let toggled = id.clone();
                    self.icon(chevron, 16., 16., self.foreground_or("icon.foreground")).id(("notebook-outline-twistie", index)).cursor_pointer().on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        let expanded = workbench.outline_row_expanded(tab, &toggled);
                        workbench.set_outline_row_expanded(tab, &toggled, !expanded);
                        cx.notify();
                    }))
                }
                false => div().id(("notebook-outline-twistie", index)).size(px(16.)).flex_none(),
            };
            let icon = (self.setting("outline.icons") != false).then(|| self.outline_entry_icon(model, entry, icon_color, ROW_HEIGHT)).flatten();
            let cell = entry.cell;
            elements.push(
                div()
                    .id(("notebook-outline-row", index))
                    .h(px(ROW_HEIGHT))
                    .flex_none()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .mx(px(4.))
                    .rounded(px(4.))
                    .pl(px(11. + indent * row.depth as f32))
                    .when(!is_selected, |row| row.hover(|row| row.bg(self.color("list.hoverBackground"))))
                    .when(is_selected, |row| row.bg(selection).when_some(selection_foreground, |row, color| row.text_color(color)))
                    .child(twistie)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .children(icon.map(|icon| icon.mr(px(6.))))
                            .child(entry.label.replace('\n', "\u{23ce}")),
                    )
                    .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                        window.focus(&workbench.outline_pane.focus, cx);
                        workbench.reveal_notebook_outline_entry(tab, cell, cx);
                    }))
                    .into_any_element(),
            );
        }
        Some(div().id("notebook-outline-tree").track_focus(&self.outline_pane.focus).size_full().overflow_y_scroll().line_height(px(ROW_HEIGHT)).children(elements).into_any_element())
    }

    /// `getChildren` from the top, each shown entry followed by its shown children when expanded.
    fn notebook_outline_rows(&self, tab: u32, entries: &[Entry], headers_only: bool, code_cells: bool) -> Vec<Row> {
        let shown = |index: usize| !outline::filtered(&entries[index], headers_only, code_cells);
        let mut rows = Vec::new();
        push_rows(&mut rows, entries, None, 0, &shown, &|id: &str| self.outline_row_expanded(tab, id));
        rows
    }

    /// `NotebookCellOutline.reveal`: the entry's cell is focused and at the top of the list.
    fn reveal_notebook_outline_entry(&mut self, tab: u32, cell: usize, cx: &mut Context<Self>) {
        let Some(editor) = self.host.notebooks.editors.iter_mut().find(|editor| editor.tab_model_id == tab) else { return };
        editor.focus = cell;
        editor.selections = vec![(cell, cell + 1)];
        self.reveal_notebook_cells(tab, cell, cell + 1, CellReveal::Top);
        self.editors_changed(cx);
        cx.notify();
    }
}

fn push_rows(rows: &mut Vec<Row>, entries: &[Entry], parent: Option<usize>, depth: usize, shown: &impl Fn(usize) -> bool, expanded: &impl Fn(&str) -> bool) {
    for index in (0..entries.len()).filter(|index| entries[*index].parent == parent && shown(*index)) {
        let has_children = entries.iter().enumerate().any(|(child, entry)| entry.parent == Some(index) && shown(child));
        rows.push(Row { entry: index, depth, has_children });
        if has_children && expanded(&index.to_string()) {
            push_rows(rows, entries, Some(index), depth + 1, shown, expanded);
        }
    }
}

/// `NotebookOutlinePaneProvider.getActiveEntry`: the focused cell's entry, else its nearest shown parent.
fn active_shown_entry(entries: &[Entry], focus: usize, headers_only: bool, code_cells: bool) -> Option<usize> {
    let mut candidate = outline::active(entries, focus);
    while let Some(index) = candidate {
        if !outline::filtered(&entries[index], headers_only, code_cells) {
            return Some(index);
        }
        candidate = entries[index].parent;
    }
    None
}
