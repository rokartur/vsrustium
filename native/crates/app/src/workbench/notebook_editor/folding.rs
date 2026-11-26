//! foldingController.ts, markupCell.ts `layoutFoldingIndicator` and foldedCellHint.ts: a markdown header's fold
//! chevron, the hint under a collapsed section, and Fold Cell / Unfold Cell.

use gpui::{ClickEvent, Context, Div, MouseButton, div, prelude::*, px};
use serde_json::Value;

use super::super::Workbench;
use super::CellRow;
use crate::commands::Outcome;
use super::super::editor_view::EDITOR_FONT;
use crate::notebook::{CODE, NotebookModel, layout, outline};

impl Workbench {
    /// `notebook-folding-indicator`: centered in the markdown gutter, the expanded chevron only on the hovered or
    /// focused row unless `notebook.showFoldingControls` is `always`, none with `never`.
    pub(super) fn fold_indicator(&self, tab: u32, row: &CellRow, focused: bool, hovered: bool, cx: &mut Context<Self>) -> Option<gpui::Stateful<Div>> {
        let (collapsed, _) = row.fold?;
        let show = self.setting("notebook.showFoldingControls");
        if show == "never" || (!collapsed && show != "always" && !focused && !hovered) {
            return None;
        }
        let (handle, index) = (row.handle, row.index);
        let icon = if collapsed { "chevron-right" } else { "chevron-down" };
        Some(
            self.icon(icon, 20., 16., self.color("icon.foreground"))
                .id(("notebook-fold", handle as usize))
                .absolute()
                .left(px((layout::MARKDOWN_CELL_GUTTER - 20.) / 2. + layout::MARKDOWN_CELL_LEFT_MARGIN))
                // `.webview-backed-markdown-cell` moves it up from 10px.
                .top(px(if row.editing { 10. } else { 8. }))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                    workbench.set_notebook_fold(tab, index, !collapsed);
                    workbench.focus_notebook_cell(tab, handle, false, window, cx);
                })),
        )
    }

    /// foldedCellHint.ts: under a collapsed header's preview, Run (when the section has code cells), `N cells hidden`
    /// and the `...` that expands it.
    pub(super) fn folded_hint(&self, tab: u32, model: &NotebookModel, row: &CellRow, cx: &mut Context<Self>) -> Option<Div> {
        let (true, length) = row.fold? else { return None };
        if row.editing {
            return None;
        }
        let index = row.index;
        let has_code = model.cells[index..=index + length].iter().any(|cell| cell.kind == CODE);
        let label = if length == 1 { "1 cell hidden".to_owned() } else { format!("{length} cells hidden") };
        let font_size = match self.setting("notebook.output.fontSize").as_f64() {
            Some(size) if size > 0. => size,
            _ => self.setting("editor.fontSize").as_f64().unwrap_or(12.),
        };
        let button = |icon: &str, id: &'static str| {
            self.icon(icon, 20., 16., self.color("icon.foreground"))
                .id((id, row.handle as usize))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|button| button.bg(self.color("editorStickyScrollHover.background")))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };
        let uri = model.uri.clone();
        let run = has_code.then(|| {
            button("play", "notebook-folded-run").mr(px(4.)).on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                drop(workbench.execute_command("notebook.cell.execute", vec![serde_json::json!({ "start": index, "end": index + length + 1 }), uri.clone()], window, cx));
            }))
        });
        let expand = button("more", "notebook-folded-expand").ml(px(4.)).on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
            workbench.set_notebook_fold(tab, index, false);
            cx.notify();
        }));
        Some(
            div()
                .absolute()
                .left(px(layout::MARKDOWN_CELL_GUTTER + layout::MARKDOWN_CELL_LEFT_MARGIN + 8.))
                .top(px(row.layout.output_top))
                .h(px(layout::FOLD_HINT_HEIGHT))
                .flex()
                .items_center()
                .children(run)
                .child(div().font_family(EDITOR_FONT).text_size(px(font_size as f32)).italic().opacity(0.7).text_color(self.color("foreground")).child(label))
                .child(expand),
        )
    }

    /// `setFoldingStateUp` one level: the innermost section holding cell `index` collapses or expands; a focused cell
    /// it hides gives the focus to the nearest visible cell above (`getNearestVisibleCellIndexUpwards`).
    pub(super) fn set_notebook_fold(&mut self, tab: u32, index: usize, collapse: bool) {
        let Some(editor) = self.host.notebooks.editor(tab) else { return };
        let Some(model) = self.host.notebooks.model(&editor.notebook) else { return };
        let Some((start, _)) = outline::section_of(&self.notebook_folding_ranges(model), index) else { return };
        let header = model.cells[start].handle;
        let view = self.notebook_views.views.entry(tab).or_default();
        match collapse {
            true => view.collapsed.insert(header),
            false => view.collapsed.remove(&header),
        };
        let model = self.host.notebooks.model(&self.host.notebooks.editor(tab).expect("found above").notebook).expect("found above");
        let rows = self.notebook_rows(tab, model);
        let editor = self.host.notebooks.editors.iter_mut().find(|editor| editor.tab_model_id == tab).expect("found above");
        let mut focus = editor.focus;
        while focus > 0 && rows.get(focus).is_some_and(|row| row.hidden) {
            focus -= 1;
        }
        if focus != editor.focus {
            editor.focus = focus;
            editor.selections = vec![(focus, focus + 1)];
        }
    }

    pub(super) fn register_notebook_fold_commands(&mut self) {
        for (id, collapse) in [("notebook.fold", true), ("notebook.unfold", false)] {
            self.commands.register(id, move |workbench, args, _, cx| {
                workbench.fold_command(args.first(), collapse);
                cx.notify();
                Outcome::done()
            });
        }
    }

    /// `NotebookFoldingAction` and `NotebookUnfoldingAction`: `{ index, direction }` or the active cell; a code cell
    /// folds nothing downwards. shortcut: `levels` above 1 fold one level, no caller passes more.
    fn fold_command(&mut self, args: Option<&Value>, collapse: bool) {
        let Some(editor) = self.active_notebook_editor() else { return };
        let (tab, focus) = (editor.tab_model_id, editor.focus);
        let index = args.and_then(|args| args["index"].as_u64()).map_or(focus, |index| index as usize);
        let up = args.is_some_and(|args| args["direction"] == "up");
        let Some(model) = self.host.notebooks.model(&editor.notebook) else { return };
        if model.cells.get(index).is_none_or(|cell| cell.kind == CODE && !up) {
            return;
        }
        self.set_notebook_fold(tab, index, collapse);
    }
}
