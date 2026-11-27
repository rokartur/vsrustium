//! cellToolbars.ts `CellTitleToolbarPart`: a cell's toolbar from `MenuId.NotebookCellTitle` (its `inline*` groups as
//! buttons, the rest in the `...` dropdown) and `MenuId.NotebookCellDelete`, the notebook toolbar's `...`
//! (`MenuId.NotebookToolbar` secondary items), and the commands of those menus (cellCommands.ts,
//! notebookClipboard.ts, insertCellActions.ts, executeActions.ts, sectionActions.ts, layoutActions.ts).

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{BoxShadow, ClickEvent, Context, Div, MouseButton, Window, canvas, div, point, prelude::*, px, rgba};
use serde_json::{Value, json};

use super::super::Workbench;
use super::super::hover::{HoverContent, HoverOptions};
use super::CellRow;
use crate::commands::Outcome;
use crate::menus::{CommandIcon, MenuAction, MenuId, menu_group_actions};
use crate::notebook::{self, CODE, MARKUP, NotebookModel, layout, outline};
use crate::theme::ThemeKind;

/// notebookBrowser.ts `KERNEL_EXTENSIONS`: the extension that brings kernels for a notebook type.
const KERNEL_EXTENSIONS: [(&str, &str); 1] = [("jupyter-notebook", "ms-toolsai.jupyter")];

fn is_inline(group: &str) -> bool {
    group.starts_with("inline")
}

/// The title and delete menus of the cell toolbar: interactiveEditor.ts and replEditor.ts use their own.
fn cell_toolbar_menus(context: &HashMap<String, Value>) -> (MenuId, MenuId) {
    match context.get("notebookEditable") == Some(&json!(true)) {
        true => (MenuId::NotebookCellTitle, MenuId::NotebookCellDelete),
        false => (MenuId::InteractiveCellTitle, MenuId::InteractiveCellDelete),
    }
}

/// notebookOptions.ts `computeCellToolbarLocation`: `right`, `left` or `hidden`, from the setting's value or its entry
/// for the notebook type, else its `default` entry.
pub(in crate::workbench) fn cell_toolbar_location(setting: &Value, view_type: &str) -> &'static str {
    let value = match setting {
        Value::Object(per_type) => per_type.get(view_type).or(per_type.get("default")),
        value => Some(value),
    };
    match value.and_then(Value::as_str) {
        Some("left") => "left",
        Some("hidden") => "hidden",
        _ => "right",
    }
}

/// layoutActions.ts `ToggleCellToolbarPositionAction.togglePosition`: the setting value with the notebook type's
/// position swapped between `left` and `right`.
pub(in crate::workbench) fn toggled_cell_toolbar_position(setting: &Value, view_type: &str) -> Value {
    let swap = |position: &str| if position == "right" { "left" } else { "right" };
    match setting {
        Value::String(position) if ["left", "right", "hidden"].contains(&position.as_str()) => json!({ "default": position, view_type: swap(position) }),
        Value::Object(per_type) => {
            let current = per_type.get(view_type).or(per_type.get("default")).and_then(Value::as_str).unwrap_or("right");
            let mut toggled = per_type.clone();
            toggled.insert(view_type.to_owned(), json!(swap(current)));
            Value::Object(toggled)
        }
        _ => json!({ "default": "right", view_type: "left" }),
    }
}

impl Workbench {
    /// notebookEditorWidgetContextKeys.ts `_updateKernelContext` and `_updateForInstalledExtension`. The port has no
    /// kernel source actions (`notebook/kernelSource`), so their count is 0.
    pub(in crate::workbench) fn notebook_kernel_context(&self, model: &NotebookModel, context: &mut HashMap<String, Value>) {
        let (all, _) = self.host.notebooks.kernels.matching(&model.view_type, &model.uri_string());
        context.insert("notebookKernelCount".into(), json!(all.len()));
        context.insert("notebookKernelSourceCount".into(), json!(0));
        let extension = KERNEL_EXTENSIONS.iter().find(|(view_type, _)| *view_type == model.view_type).map(|(_, id)| *id);
        let missing = extension.is_some_and(|id| !self.extensions.iter().any(|extension| extension.id.eq_ignore_ascii_case(id)));
        context.insert("notebookMissingKernelExtension".into(), json!(missing));
    }

    /// The cell toolbar's scoped context keys (cellContextKeys.ts): the row's cell, not the focused one. The toolbar
    /// sits in the notebook editor, so its menus see the editor focused.
    fn cell_toolbar_context(&self, model: &NotebookModel, row: &CellRow, window: &Window) -> HashMap<String, Value> {
        let mut context = self.key_context(window);
        let cell = &model.cells[row.index];
        let editable = !self.host.notebooks.interactives.iter().any(|interactive| interactive.notebook == model.uri_string());
        context.insert("notebookEditorFocused".into(), json!(true));
        context.insert("notebookEditable".into(), json!(editable));
        context.insert("notebookCellEditable".into(), json!(editable));
        context.insert("notebookCellType".into(), json!(if cell.kind == MARKUP { "markup" } else { "code" }));
        context.insert("notebookCellMarkdownEditMode".into(), json!(cell.kind == MARKUP && row.editing));
        context.insert("notebookCellHasOutputs".into(), json!(!cell.outputs.is_empty()));
        context.insert("notebookCellInputIsCollapsed".into(), json!(false));
        let line_numbers = match self.notebook_views.cell_line_numbers.get(&cell_key(&cell.uri)) {
            Some(true) => "on",
            Some(false) => "off",
            None => "inherit",
        };
        context.insert("notebookCellLineNumbers".into(), json!(line_numbers));
        self.notebook_kernel_context(model, &mut context);
        context
    }

    /// The toolbar over the cell's top edge: its title menu's buttons, `...` when the menu has more, then Delete
    /// (`updateActions`); None without actions (`cell-has-toolbar-actions`) or with the toolbar hidden.
    pub(in crate::workbench) fn cell_toolbar(&self, tab: u32, model: &NotebookModel, row: &CellRow, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let location = cell_toolbar_location(&self.setting("notebook.cellToolbarLocation"), &model.view_type);
        if location == "hidden" {
            return None;
        }
        let (primary, more, delete) = self.cell_toolbar_actions(model, row, window);
        let more = !more.is_empty();
        if primary.is_empty() && delete.is_empty() && !more {
            return None;
        }
        let args = vec![json!({ "start": row.index, "end": row.index + 1, "ui": true }), model.uri.clone()];
        // notebook.css: `--vscode-shadow-sm`, none in high contrast (`no-shadows`).
        let high_contrast = matches!(self.theme.kind, ThemeKind::HighContrastDark | ThemeKind::HighContrastLight);
        let shadow = if high_contrast { Vec::new() } else { vec![BoxShadow { color: rgba(0x00000014).into(), offset: point(px(0.), px(0.)), blur_radius: px(2.), spread_radius: px(0.), inset: false }] };
        let mut toolbar = div()
            .absolute()
            // notebookCellTitleToolbar.css: 26px high plus the border, 14px over the cell's top edge.
            .top(px(-14.))
            .h(px(28.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .border_1()
            .border_color(self.color("notebook.cellToolbarSeparator"))
            .bg(self.color("editorWidget.background"))
            .shadow(shadow);
        // notebookEditorWidget.ts `cell-title-toolbar-left` and `-right`.
        toolbar = match location {
            "left" => toolbar.left(px(layout::CODE_CELL_LEFT_MARGIN + layout::CELL_RUN_GUTTER + 16.)),
            _ => toolbar.right(px(layout::CELL_RIGHT_MARGIN + 26.)),
        };
        for action in &primary {
            toolbar = toolbar.children(self.cell_toolbar_button(row.handle, action, &args, cx));
        }
        if more {
            toolbar = toolbar.child(self.cell_toolbar_more(tab, row.handle, model, cx));
        }
        for action in &delete {
            toolbar = toolbar.children(self.cell_toolbar_button(row.handle, action, &args, cx));
        }
        Some(toolbar)
    }

    /// The toolbar's buttons, its `...` dropdown and its delete buttons for the row's cell (`getCellToolbarActions`).
    pub(super) fn cell_toolbar_actions(&self, model: &NotebookModel, row: &CellRow, window: &Window) -> (Vec<MenuAction>, Vec<MenuAction>, Vec<MenuAction>) {
        let context = self.cell_toolbar_context(model, row, window);
        let (title_menu, delete_menu) = cell_toolbar_menus(&context);
        let primary = menu_group_actions(&self.menus, title_menu, &context, is_inline);
        let more = menu_group_actions(&self.menus, title_menu, &context, |group| !is_inline(group));
        let delete = menu_group_actions(&self.menus, delete_menu, &context, is_inline);
        (primary, more, delete)
    }

    /// One `.action-item.menu-entry`: 24px square, 1px 2px margins, its command's icon.
    fn cell_toolbar_button(&self, handle: u64, action: &MenuAction, args: &[Value], cx: &mut Context<Self>) -> Option<gpui::Stateful<Div>> {
        let MenuAction::Command { id, title, enabled, .. } = action else { return None };
        let icon = match self.command_icons.get(id) {
            Some(CommandIcon::Codicon(name)) => self.icon(name, 24., 16., self.color("icon.foreground")),
            _ => div().px(px(4.)).child(title.clone()),
        };
        let (command, args) = (id.clone(), args.to_vec());
        let button = icon
            .id(gpui::SharedString::from(format!("notebook-cell-toolbar-{handle}-{id}")))
            .mx(px(2.))
            .my(px(1.))
            .rounded(px(5.))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .child(self.hover_target(gpui::SharedString::from(format!("notebook-cell-toolbar-hover-{handle}-{id}")), HoverContent::Text(title.clone()), HoverOptions::mouse()));
        if !enabled {
            return Some(button.opacity(0.4));
        }
        Some(button.cursor_pointer().hover(|button| button.bg(self.color("toolbar.hoverBackground"))).on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
            cx.stop_propagation();
            drop(workbench.execute_command(&command, args.clone(), window, cx));
        })))
    }

    /// The toolbar's `...` (`ToggleMenuAction`): the title menu's other groups as a dropdown.
    fn cell_toolbar_more(&self, tab: u32, handle: u64, model: &NotebookModel, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let bounds = Rc::new(Cell::new(gpui::Bounds::default()));
        let measured = bounds.clone();
        let notebook = model.uri_string();
        let open = self.context_menu.is_some() && self.notebook_views.views.get(&tab).is_some_and(|view| view.toolbar_menu == Some(handle));
        // A plain `.action-item` (not `.menu-entry`): 22px wide without margins, as measured in the reference.
        self.icon("toolbar-more", 22., 16., self.color("icon.foreground"))
            .id(("notebook-cell-toolbar-more", handle as usize))
            .debug_selector(move || format!("notebook-cell-toolbar-more-{handle}"))
            .rounded(px(5.))
            .when(open, |button| button.bg(self.color("toolbar.activeBackground")))
            .cursor_pointer()
            .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(canvas(move |element_bounds, _, _| measured.set(element_bounds), |_, _, _, _| {}).absolute().size_full())
            .child(self.hover_target(("notebook-cell-toolbar-more-hover", handle as usize), HoverContent::Text("More Actions...".into()), HoverOptions::mouse()))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                let Some(model) = workbench.host.notebooks.model(&notebook) else { return };
                let Some(row) = workbench.notebook_rows(tab, model).into_iter().find(|row| row.handle == handle) else { return };
                let context = workbench.cell_toolbar_context(model, &row, window);
                let actions = menu_group_actions(&workbench.menus, cell_toolbar_menus(&context).0, &context, |group| !is_inline(group));
                let args = vec![json!({ "start": row.index, "end": row.index + 1, "ui": true }), model.uri.clone()];
                // The pointer leaves the preview for the menu, which lies over the webview upstream (`mouseLeaveMarkupCell`);
                // here the hidden webview never reports it.
                if let Some(view) = workbench.notebook_views.views.get_mut(&tab) {
                    view.toolbar_menu = Some(handle);
                    view.hovered = None;
                }
                workbench.show_context_menu_actions(actions, &context, bounds.get(), args, window, cx);
            }))
    }

    /// The notebook toolbar's `...` (notebookEditorToolbar.ts secondary actions), shown when the menu has items.
    pub(in crate::workbench) fn notebook_toolbar_more(&self, window: &Window, cx: &mut Context<Self>) -> Option<gpui::Stateful<Div>> {
        let context = self.notebook_toolbar_context(window);
        if menu_group_actions(&self.menus, MenuId::NotebookToolbar, &context, |group| !group.starts_with("navigation")).is_empty() {
            return None;
        }
        let bounds = Rc::new(Cell::new(gpui::Bounds::default()));
        let measured = bounds.clone();
        let button = self
            .icon("toolbar-more", 22., 16., self.color("icon.foreground"))
            .id("notebook-toolbar-more")
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(canvas(move |element_bounds, _, _| measured.set(element_bounds), |_, _, _, _| {}).absolute().size_full())
            .child(self.hover_target("notebook-toolbar-more-hover", HoverContent::Text("More Actions...".into()), HoverOptions::mouse()))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                let context = workbench.notebook_toolbar_context(window);
                let actions = menu_group_actions(&workbench.menus, MenuId::NotebookToolbar, &context, |group| !group.starts_with("navigation"));
                workbench.show_context_menu_actions(actions, &context, bounds.get(), Vec::new(), window, cx);
            }));
        Some(button)
    }

    /// The notebook toolbar's context: the editor focused, as the toolbar is in it.
    fn notebook_toolbar_context(&self, window: &Window) -> HashMap<String, Value> {
        let mut context = self.key_context(window);
        context.insert("notebookEditorFocused".into(), json!(true));
        context
    }

    /// Whether the active notebook editor's focused cell shows a code editor: a code cell or a markdown cell in edit
    /// mode, not a preview.
    pub(in crate::workbench) fn active_cell_has_editor(&self) -> bool {
        let Some(editor) = self.active_notebook_editor() else { return false };
        let Some(cell) = self.host.notebooks.model(&editor.notebook).and_then(|model| model.cells.get(editor.focus)) else { return false };
        cell.kind == CODE || self.notebook_views.views.get(&editor.tab_model_id).is_some_and(|view| view.editing.contains(&cell.handle))
    }

    /// Whether the cell editor of `cell_uri` (`TextDocument::uri_string`) shows line numbers: its own `lineNumbers`,
    /// else `notebook.lineNumbers`.
    pub(in crate::workbench) fn cell_line_numbers(&self, cell_uri: &str) -> bool {
        match self.notebook_views.cell_line_numbers.get(cell_uri) {
            Some(on) => *on,
            None => self.setting("notebook.lineNumbers") == "on",
        }
    }

    pub(in crate::workbench) fn register_notebook_cell_commands(&mut self) {
        self.register_notebook_fold_commands();
        self.commands.register("notebook.cell.delete", |workbench, args, window, cx| {
            let Some((key, cell)) = workbench.cell_from_args(&args) else { return Outcome::done() };
            let Some(tab) = workbench.host.notebooks.editors.iter().find(|editor| editor.notebook == key).map(|editor| editor.tab_model_id) else { return Outcome::done() };
            workbench.delete_notebook_cell(tab, cell, window, cx);
            Outcome::done()
        });
        self.commands.register("notebook.section.runCells", |workbench, args, _, cx| {
            let Some((key, handle)) = workbench.cell_from_args(&args) else { return Outcome::done() };
            let model = workbench.host.notebooks.model(&key).expect("found above");
            let index = model.index_of_handle(handle).expect("found above");
            let ranges = workbench.notebook_folding_ranges(model);
            let Some((start, end)) = outline::section_of(&ranges, index) else { return Outcome::done() };
            let handles = model.cells[start..=end].iter().filter(|cell| cell.kind == CODE).map(|cell| cell.handle).collect();
            workbench.execute_notebook_cells(&key, handles, cx)
        });
        for (id, above) in [("notebook.cell.executeCellsAbove", true), ("notebook.cell.executeCellAndBelow", false)] {
            self.commands.register(id, move |workbench, args, window, cx| {
                let Some((key, handle)) = workbench.cell_from_args(&args) else { return Outcome::done() };
                let model = workbench.host.notebooks.model(&key).expect("found above");
                let index = model.index_of_handle(handle).expect("found above");
                let range = if above { 0..index } else { index..model.cells.len() };
                let handles = model.cells[range].iter().filter(|cell| cell.kind == CODE).map(|cell| cell.handle).collect();
                if let Some(tab) = workbench.host.notebooks.editors.iter().find(|editor| editor.notebook == key).map(|editor| editor.tab_model_id) {
                    workbench.focus_notebook_cell(tab, handle, false, window, cx);
                }
                workbench.execute_notebook_cells(&key, handles, cx)
            });
        }
        self.commands.register("notebook.cell.split", |workbench, args, window, cx| {
            workbench.split_notebook_cell(&args, window, cx);
            Outcome::done()
        });
        for (id, cut) in [("notebook.cell.copy", false), ("notebook.cell.cut", true)] {
            self.commands.register(id, move |workbench, args, _, cx| {
                workbench.copy_notebook_cell(&args, cut, cx);
                Outcome::done()
            });
        }
        self.commands.register("notebook.cell.paste", |workbench, args, _, cx| {
            workbench.paste_notebook_cells(&args, cx);
            Outcome::done()
        });
        for (id, kind) in [("notebook.cell.insertCodeCellAbove", CODE), ("notebook.cell.insertMarkdownCellAbove", MARKUP)] {
            self.commands.register(id, move |workbench, args, window, cx| workbench.insert_toolbar_cell(&args, kind, true, window, cx));
        }
        for (id, kind) in [("notebook.cell.changeToCode", CODE), ("notebook.cell.changeToMarkdown", MARKUP)] {
            self.commands.register(id, move |workbench, args, window, cx| {
                workbench.change_notebook_cell_kind(&args, kind, window, cx);
                Outcome::done()
            });
        }
        for (id, above) in [("notebook.cell.joinAbove", true), ("notebook.cell.joinBelow", false)] {
            self.commands.register(id, move |workbench, args, _, cx| {
                workbench.join_notebook_cells(&args, above, cx);
                Outcome::done()
            });
        }
        self.commands.register("notebook.cell.joinSelected", |workbench, _, _, cx| {
            workbench.join_selected_notebook_cells(cx);
            Outcome::done()
        });
        for (id, up) in [("notebook.cell.moveUp", true), ("notebook.cell.moveDown", false)] {
            self.commands.register(id, move |workbench, args, _, cx| {
                workbench.move_notebook_cell(&args, up, cx);
                Outcome::done()
            });
        }
        self.commands.register("notebook.cell.toggleLineNumbers", |workbench, args, _, cx| {
            let Some((key, handle)) = workbench.cell_from_args(&args) else { return Outcome::done() };
            let model = workbench.host.notebooks.model(&key).expect("found above");
            let uri = cell_key(&model.cells[model.index_of_handle(handle).expect("found above")].uri);
            let on = workbench.cell_line_numbers(&uri);
            workbench.notebook_views.cell_line_numbers.insert(uri, !on);
            cx.notify();
            Outcome::done()
        });
        self.commands.register("notebook.toggleCellToolbarPosition", |workbench, args, _, cx| {
            // From the toolbar only (`context.ui`), as upstream.
            if args.first().is_none_or(|context| context["ui"] != true) {
                return Outcome::done();
            }
            let Some((key, _)) = workbench.cell_from_args(&args) else { return Outcome::done() };
            let view_type = workbench.host.notebooks.model(&key).expect("found above").view_type.clone();
            let toggled = toggled_cell_toolbar_position(&workbench.setting("notebook.cellToolbarLocation"), &view_type);
            workbench.update_setting("notebook.cellToolbarLocation", toggled);
            cx.notify();
            Outcome::done()
        });
        // layoutActions.ts: forwards its arguments, which from the editor title have no `ui`, so it changes nothing.
        self.commands.register("notebook.toggleCellToolbarPositionFromEditorTitle", |workbench, args, window, cx| workbench.execute_command("notebook.toggleCellToolbarPosition", args, window, cx));
        self.commands.register("workbench.notebook.layout.configure", |workbench, _, window, cx| workbench.execute_command("workbench.notebook.layout.configure.editorTitle", Vec::new(), window, cx));
    }

    /// The notebook and the handle of the cell a cell command runs on: the toolbar's or API's range start, else the
    /// focused cell (`getContextFromActiveEditor`).
    fn cell_from_args(&self, args: &[Value]) -> Option<(String, u64)> {
        let (key, indices) = self.cells_from_args(args)?;
        let model = self.host.notebooks.model(&key)?;
        Some((key, model.cells.get(*indices.first()?)?.handle))
    }

    /// The notebook editor tab and the cell handle of `cell_from_args`.
    pub(super) fn cell_target(&self, args: &[Value]) -> Option<(u32, u64)> {
        let (key, handle) = self.cell_from_args(args)?;
        let tab = self.host.notebooks.editors.iter().find(|editor| editor.notebook == key)?.tab_model_id;
        Some((tab, handle))
    }

    /// foldingModel.ts `recompute` over the notebook's cells.
    pub(in crate::workbench) fn notebook_folding_ranges(&self, model: &NotebookModel) -> Vec<(usize, usize)> {
        let texts: Vec<String> = model.cells.iter().map(|cell| if cell.kind == MARKUP { self.cell_text(&cell.uri) } else { String::new() }).collect();
        let sources: Vec<outline::CellSource> = model.cells.iter().zip(&texts).map(|(cell, text)| outline::CellSource { markup: cell.kind == MARKUP, text, executing: false }).collect();
        outline::folding_ranges(&sources)
    }

    /// Replaces cells with one undo step and moves the editor's focus to `focus`.
    pub(super) fn edit_notebook_cells(&mut self, key: &str, edits: Vec<Value>, focus: Option<usize>, cx: &mut Context<Self>) -> bool {
        let Ok(applied) = self.apply_notebook_edits(key, &edits, cx) else { return false };
        if !applied.undo.is_empty() {
            self.push_cells_undo(key, applied.undo, edits);
        }
        if let Some(focus) = focus
            && let Some(editor) = self.host.notebooks.editors.iter_mut().find(|editor| editor.notebook == key)
        {
            editor.focus = focus;
            editor.selections = vec![(focus, focus + 1)];
        }
        self.editors_changed(cx);
        true
    }

    /// cellCommands.ts `SplitCellAction`: the cell's text cut at each cursor (at its start when the cell editor does
    /// not have the focus), the first part kept in the cell, the others new cells below.
    fn split_notebook_cell(&mut self, args: &[Value], window: &Window, cx: &mut Context<Self>) {
        let Some((key, handle)) = self.cell_from_args(args) else { return };
        let model = self.host.notebooks.model(&key).expect("found above");
        let index = model.index_of_handle(handle).expect("found above");
        let cell = &model.cells[index];
        let Some(editor) = self.cell_model(&cell.uri) else { return };
        let lines = editor.document().lines.clone();
        let cell_editor = self.editors.active().filter(|active| self.editor_focus.is_focused(window) && super::uri_parts(&cell.uri).is_ok_and(|(scheme, path)| active.document().is(scheme, &path)));
        let mut points: Vec<(usize, usize)> = match cell_editor {
            Some(active) => active.selections.iter().map(|selection| (selection.start().line, selection.start().column)).collect(),
            None => vec![(0, 0)],
        };
        points.sort();
        let parts = split_at(&lines, &points);
        let (kind, language, mime) = (cell.kind, cell.language.clone(), cell.mime.clone());
        let cell_uri = cell.uri.clone();
        let last = lines.len() - 1;
        let whole = json!({ "startLineNumber": 1, "startColumn": 1, "endLineNumber": last + 1, "endColumn": lines[last].chars().count() + 1 });
        let edit = json!([{ "resource": cell_uri, "textEdit": { "range": whole, "text": parts[0] } }]);
        if self.apply_workspace_text_edits(&edit, &Value::Null, crate::editor::next_undo_stamp(), cx).is_err() {
            return;
        }
        let cells: Vec<Value> = parts[1..].iter().map(|text| json!({ "cellKind": kind, "language": language, "mime": mime, "source": text, "outputs": [], "metadata": {} })).collect();
        self.edit_notebook_cells(&key, vec![json!({ "editType": 1, "index": index + 1, "count": 0, "cells": cells })], None, cx);
    }

    /// notebookClipboard.ts `runCopyCells` and `runCutCells` for one cell: its text on the clipboard, the cell kept
    /// for Paste Cell; Cut deletes it.
    fn copy_notebook_cell(&mut self, args: &[Value], cut: bool, cx: &mut Context<Self>) {
        let Some((key, handle)) = self.cell_from_args(args) else { return };
        let model = self.host.notebooks.model(&key).expect("found above");
        let index = model.index_of_handle(handle).expect("found above");
        let cell = &model.cells[index];
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(self.cell_text(&cell.uri)));
        self.notebook_views.cell_clipboard = Some((vec![self.cell_data(cell)], !cut));
        if !cut {
            return;
        }
        let focus = self.host.notebooks.editors.iter().find(|editor| editor.notebook == key).map_or(0, |editor| editor.focus);
        let length = model.cells.len();
        let focus = if focus > index || (focus == index && focus + 1 == length) { focus.saturating_sub(1) } else { focus };
        self.edit_notebook_cells(&key, vec![json!({ "editType": 1, "index": index, "count": 1, "cells": [] })], Some(focus), cx);
    }

    /// notebookClipboard.ts `runPasteCells`: the copied cells below the cell (at the top of an empty notebook), the
    /// first of them focused.
    fn paste_notebook_cells(&mut self, args: &[Value], cx: &mut Context<Self>) {
        let Some((cells, _)) = self.notebook_views.cell_clipboard.clone() else { return };
        let (key, index) = match self.cell_from_args(args) {
            Some((key, handle)) => {
                let index = self.host.notebooks.model(&key).expect("found above").index_of_handle(handle).expect("found above");
                (key, index + 1)
            }
            None => match self.active_notebook_editor() {
                Some(editor) if self.host.notebooks.model(&editor.notebook).is_some_and(|model| model.cells.is_empty()) => (editor.notebook.clone(), 0),
                _ => return,
            },
        };
        let count = cells.len();
        if self.edit_notebook_cells(&key, vec![json!({ "editType": 1, "index": index, "count": 0, "cells": cells })], Some(index), cx)
            && let Some(editor) = self.host.notebooks.editors.iter_mut().find(|editor| editor.notebook == key)
        {
            editor.selections = vec![(index, index + count)];
        }
    }

    /// insertCellActions.ts `insertNewCell` above or below the command's cell: a code cell gets the language of the
    /// code cell above it, else of the first code cell; a markdown cell starts in edit mode.
    pub(in crate::workbench) fn insert_toolbar_cell(&mut self, args: &[Value], kind: u64, above: bool, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        if let Some((key, handle)) = self.cell_from_args(args) {
            self.insert_notebook_cell(&key, handle, kind, above, window, cx);
        }
        Outcome::done()
    }

    fn insert_notebook_cell(&mut self, key: &str, handle: u64, kind: u64, above: bool, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.host.notebooks.model(key).expect("an open notebook");
        let index = model.index_of_handle(handle).expect("a cell of the notebook") + usize::from(!above);
        let language = match kind {
            CODE => model.cells[..index].iter().rev().chain(&model.cells).find(|cell| cell.kind == CODE).map_or("plaintext".to_owned(), |cell| cell.language.clone()),
            _ => "markdown".to_owned(),
        };
        let cell = json!({ "cellKind": kind, "language": language, "source": "", "outputs": [], "metadata": {} });
        if !self.edit_notebook_cells(key, vec![json!({ "editType": 1, "index": index, "count": 0, "cells": [cell] })], Some(index), cx) {
            return;
        }
        let inserted = self.host.notebooks.model(key).expect("still open").cells[index].handle;
        if kind != CODE {
            self.edit_markup_cell(inserted);
        }
        window.focus(&self.editor_focus, cx);
    }

    /// cellOperations.ts `changeCellToKind` from the toolbar: the cell replaced by one of `kind` with its text,
    /// outputs and metadata, a code cell in the kernel's first language (else plaintext).
    fn change_notebook_cell_kind(&mut self, args: &[Value], kind: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, handle)) = self.cell_from_args(args) else { return };
        let model = self.host.notebooks.model(&key).expect("found above");
        let index = model.index_of_handle(handle).expect("found above");
        let cell = &model.cells[index];
        if cell.kind == kind {
            return;
        }
        let mut data = self.cell_data(cell);
        let editing = self.host.notebooks.editors.iter().find(|editor| editor.notebook == key).and_then(|editor| self.notebook_views.views.get(&editor.tab_model_id)).is_some_and(|view| view.editing.contains(&handle));
        data["cellKind"] = json!(kind);
        data["language"] = match kind {
            MARKUP => json!("markdown"),
            _ => json!(self.host.notebooks.kernels.resolve(&model.view_type, &key).and_then(|kernel| kernel.supported_languages.first().cloned()).unwrap_or("plaintext".into())),
        };
        if kind == MARKUP {
            data["mime"] = json!("text/markdown");
        }
        self.edit_notebook_cells(&key, vec![json!({ "editType": 1, "index": index, "count": 1, "cells": [data] })], Some(index), cx);
        let new_handle = self.host.notebooks.model(&key).expect("still open").cells[index].handle;
        if kind == MARKUP || editing {
            self.edit_markup_cell(new_handle);
        }
        if let Some(tab) = self.host.notebooks.editors.iter().find(|editor| editor.notebook == key).map(|editor| editor.tab_model_id) {
            self.focus_notebook_cell(tab, new_handle, editing, window, cx);
        }
    }

    /// cellOperations.ts `joinNotebookCells` from the toolbar: the cell's text appended to the cell above (or the
    /// cell below's to it) on a new line, and the joined cell removed; the remaining cell is focused in edit mode.
    fn join_notebook_cells(&mut self, args: &[Value], above: bool, cx: &mut Context<Self>) {
        let Some((key, handle)) = self.cell_from_args(args) else { return };
        let model = self.host.notebooks.model(&key).expect("found above");
        let index = model.index_of_handle(handle).expect("found above");
        if (above && index == 0) || (!above && index + 1 == model.cells.len()) {
            return;
        }
        let (target, joined) = if above { (index - 1, index) } else { (index, index + 1) };
        let text = format!("\n{}", self.cell_text(&model.cells[joined].uri));
        let (target_uri, target_handle) = (model.cells[target].uri.clone(), model.cells[target].handle);
        let Some(editor) = self.cell_model(&target_uri) else { return };
        let lines = editor.document().lines.clone();
        let end = json!({ "startLineNumber": lines.len(), "startColumn": lines[lines.len() - 1].chars().count() + 1, "endLineNumber": lines.len(), "endColumn": lines[lines.len() - 1].chars().count() + 1 });
        let edit = json!([{ "resource": target_uri, "textEdit": { "range": end, "text": text } }]);
        if self.apply_workspace_text_edits(&edit, &Value::Null, crate::editor::next_undo_stamp(), cx).is_err() {
            return;
        }
        self.edit_notebook_cells(&key, vec![json!({ "editType": 1, "index": joined, "count": 1, "cells": [] })], Some(target), cx);
        self.edit_markup_cell(target_handle);
    }

    /// cellOperations.ts `joinSelectedCells`: the selected cells' texts in the first, one per line, the others
    /// removed; cells of different kinds are not joined.
    fn join_selected_notebook_cells(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.active_notebook_editor() else { return };
        let key = editor.notebook.clone();
        let model = self.host.notebooks.model(&key).expect("an editor shows an open notebook");
        let indices: Vec<usize> = editor.selections.iter().flat_map(|(start, end)| *start..*end).filter(|index| *index < model.cells.len()).collect();
        if indices.len() <= 1 {
            return;
        }
        let cells: Vec<&notebook::Cell> = indices.iter().map(|index| &model.cells[*index]).collect();
        if cells.iter().any(|cell| cell.kind != cells[0].kind) {
            self.notify_warning("Cannot join cells of different kinds");
            return;
        }
        let mut data = self.cell_data(cells[0]);
        data["source"] = json!(cells.iter().map(|cell| self.cell_text(&cell.uri)).collect::<Vec<_>>().join("\n"));
        let selections = editor.selections.clone();
        let (first_start, first_end) = selections[0];
        let mut edits = vec![json!({ "editType": 1, "index": first_start, "count": first_end - first_start, "cells": [data] })];
        for (start, end) in &selections[1..] {
            edits.push(json!({ "editType": 1, "index": start, "count": end - start, "cells": [] }));
        }
        self.edit_notebook_cells(&key, edits, Some(first_start), cx);
    }

    /// cellOperations.ts `moveCellRange` of one cell.
    fn move_notebook_cell(&mut self, args: &[Value], up: bool, cx: &mut Context<Self>) {
        let Some((key, handle)) = self.cell_from_args(args) else { return };
        let model = self.host.notebooks.model(&key).expect("found above");
        let index = model.index_of_handle(handle).expect("found above");
        let target = match up {
            true if index > 0 => index - 1,
            false if index + 1 < model.cells.len() => index + 1,
            _ => return,
        };
        self.edit_notebook_cells(&key, vec![json!({ "editType": 6, "index": index, "length": 1, "newIdx": target })], Some(target), cx);
    }
}

/// The key of a cell in `cell_line_numbers`: its text model's URI string, as `TextDocument::uri_string` makes it.
fn cell_key(cell_uri: &Value) -> String {
    super::uri_parts(cell_uri).map(|(scheme, path)| crate::editor::uri_string(scheme, &path)).unwrap_or_default()
}

/// cellOperations.ts `computeCellLinesContents`: the text between the start, each split point (one at a line's
/// end moves to the next line's start) and the end; `points` are sorted (line, column) pairs.
fn split_at(lines: &[String], points: &[(usize, usize)]) -> Vec<String> {
    let last = lines.len() - 1;
    let mut boundaries = vec![(0, 0)];
    for &(line, column) in points {
        let length = lines[line].chars().count();
        let point = if column == length && column != 0 && line < last { (line + 1, 0) } else { (line, column) };
        if boundaries.len() == 1 || boundaries.last() != Some(&point) {
            boundaries.push(point);
        }
    }
    boundaries.push((last, lines[last].chars().count()));
    let offset = |(line, column): (usize, usize)| lines[..line].iter().map(|line| line.chars().count() + 1).sum::<usize>() + column;
    let text: Vec<char> = lines.join("\n").chars().collect();
    boundaries.windows(2).map(|pair| text[offset(pair[0])..offset(pair[1])].iter().collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_points_cut_the_text() {
        let lines = ["for x in y:".to_owned(), "    print(x)".to_owned(), String::new()];
        assert_eq!(split_at(&lines, &[(0, 0)]), ["", "for x in y:\n    print(x)\n"]);
        // A point at a line's end starts the next part on the next line.
        assert_eq!(split_at(&lines, &[(0, 11)]), ["for x in y:\n", "    print(x)\n"]);
        assert_eq!(split_at(&lines, &[(1, 4)]), ["for x in y:\n    ", "print(x)\n"]);
    }

    #[test]
    fn the_toolbar_position_toggles_per_notebook_type() {
        assert_eq!(cell_toolbar_location(&json!({ "default": "right" }), "jupyter-notebook"), "right");
        let toggled = toggled_cell_toolbar_position(&json!({ "default": "right" }), "jupyter-notebook");
        assert_eq!(toggled, json!({ "default": "right", "jupyter-notebook": "left" }));
        assert_eq!(cell_toolbar_location(&toggled, "jupyter-notebook"), "left");
        assert_eq!(toggled_cell_toolbar_position(&json!("hidden"), "a"), json!({ "default": "hidden", "a": "right" }));
        assert_eq!(cell_toolbar_location(&json!("hidden"), "a"), "hidden");
    }
}
