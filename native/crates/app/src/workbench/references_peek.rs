//! referencesController.ts and referencesWidget.ts: the references peek, a zone below the symbol with a preview of the
//! selected reference on the left and the references tree (referencesTree.ts) on the right.

use std::ops::Range;

use gpui::{AnyElement, ClickEvent, Context, Div, FocusHandle, MouseButton, MouseDownEvent, ScrollWheelEvent, Stateful, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::peek_view::{LayoutStorage, PeekKind, PeekLayout, UI_FONT};
use crate::commands::Outcome;
use crate::editor::{LINE_HEIGHT, OpenEditor, Position, TextDocument, char_column, utf16_column};
use crate::references_model::{Location, ReferencesModel};

/// referencesTree.ts `Delegate.getHeight`.
const ROW_HEIGHT: f32 = 23.;
const LAYOUT: LayoutStorage = LayoutStorage { key: "peekViewLayout", height_key: "heightInLines", default_height: 18. };

pub(super) struct ReferencesPeek {
    widget: Option<PeekWidget>,
    tree_focus: FocusHandle,
}

impl ReferencesPeek {
    pub(super) fn new(cx: &mut Context<Workbench>) -> Self {
        ReferencesPeek { widget: None, tree_focus: cx.focus_handle() }
    }
}

struct PeekWidget {
    layout: PeekLayout,
    model: ReferencesModel,
    peek_mode: bool,
    /// The tree's focus and selection; `None` of `Reference` is the file row of the group.
    focused: Option<TreeRow>,
    selected: Option<TreeRow>,
    expanded: Vec<bool>,
    tree_scroll: f32,
    /// `_revealedReference`, shown in the preview, and the preview's scroll top.
    revealed: Option<usize>,
    preview_scroll: f32,
    /// The files of references that no editor has open, read for the preview (`createModelReference`).
    documents: Vec<TextDocument>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum TreeRow {
    File(usize),
    Reference(usize),
}

impl Workbench {
    pub(super) fn register_references_peek_commands(&mut self) {
        for (id, forward) in [("goToNextReference", true), ("goToPreviousReference", false)] {
            self.commands.register(id, move |workbench, _, window, cx| {
                workbench.go_to_next_or_previous_reference(forward, window, cx);
                Outcome::done()
            });
        }
        for (alias, id) in [("goToNextReferenceFromEmbeddedEditor", "goToNextReference"), ("goToPreviousReferenceFromEmbeddedEditor", "goToPreviousReference"), ("closeReferenceSearchEditor", "closeReferenceSearch")] {
            self.commands.register(alias, move |workbench, args, window, cx| workbench.execute_command(id, args, window, cx));
        }
        self.commands.register("closeReferenceSearch", |workbench, _, window, cx| {
            workbench.close_references_peek(true, window, cx);
            Outcome::done()
        });
        // shortcut: the preview is drawn, not an embedded editor, so it takes no focus and the toggle focuses the tree.
        self.commands.register("togglePeekWidgetFocus", |workbench, _, window, cx| {
            if workbench.references_peek.widget.is_some() {
                window.focus(&workbench.references_peek.tree_focus, cx);
            }
            Outcome::done()
        });
        self.commands.register("revealReference", |workbench, _, window, cx| {
            if let Some(reference) = workbench.focused_reference_row() {
                workbench.reveal_peek_reference(reference, cx);
                workbench.peek_reference_selected(reference, Selected::Goto, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("openReferenceToSide", |workbench, _, window, cx| {
            if let Some(reference) = workbench.focused_reference_row() {
                workbench.peek_reference_selected(reference, Selected::Side, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("openReference", |workbench, _, window, cx| {
            if let Some(reference) = workbench.focused_reference_row() {
                workbench.open_peek_reference(reference, false, true, window, cx);
            }
            Outcome::done()
        });
    }

    pub(super) fn references_peek_layout(&self) -> Option<&PeekLayout> {
        self.references_peek.widget.as_ref().map(|widget| &widget.layout)
    }

    pub(super) fn references_peek_layout_mut(&mut self) -> Option<&mut PeekLayout> {
        self.references_peek.widget.as_mut().map(|widget| &mut widget.layout)
    }

    pub(super) fn references_peek_visible(&self) -> bool {
        self.references_peek.widget.is_some()
    }

    /// `referenceSearchVisible`, and the list keys of the tree when it has the focus.
    pub(super) fn references_peek_context(&self, window: &Window, context: &mut std::collections::HashMap<String, Value>) {
        context.insert("referenceSearchVisible".into(), json!(self.references_peek_visible()));
        let Some(widget) = &self.references_peek.widget else { return };
        if !self.references_peek.tree_focus.is_focused(window) {
            return;
        }
        context.insert("listFocus".into(), json!(true));
        let file = match widget.focused {
            Some(TreeRow::File(group)) => Some(widget.expanded[group]),
            _ => None,
        };
        context.insert("treeElementCanCollapse".into(), json!(file == Some(true)));
        context.insert("treeElementCanExpand".into(), json!(file == Some(false)));
    }

    /// `ReferencesController.toggleWidget`: a peek at the position of `range` already open closes; else the widget
    /// opens there with the model, its nearest reference selected and the tree focused.
    pub(super) fn open_references_peek(&mut self, model: ReferencesModel, range: Range<Position>, peek_mode: bool, window: &mut Window, cx: &mut Context<Self>) {
        let position = self.references_peek.widget.as_ref().and_then(|widget| self.peek_zone_position(&widget.layout));
        self.close_references_peek(true, window, cx);
        if position.is_some_and(|position| range.start <= position && position <= range.end) {
            return;
        }
        let layout = self.open_peek_zone(&LAYOUT, range.start);
        self.reveal_peek_range(range.start, range.end);
        let editor = self.editors.active().expect("the peek opens in the active editor");
        let uri = editor.document().uri_string();
        let start = (range.start.line + 1, utf16_column(&editor.document().lines[range.start.line], range.start.column) + 1);
        let nearest = model.nearest_reference(&uri, start);
        let expanded = vec![false; model.groups.len()];
        self.references_peek.widget = Some(PeekWidget {
            layout,
            model,
            peek_mode,
            focused: None,
            selected: None,
            expanded,
            tree_scroll: 0.,
            revealed: None,
            preview_scroll: 0.,
            documents: Vec::new(),
        });
        if let Some(reference) = nearest {
            self.reveal_peek_reference(reference, cx);
            let widget = self.references_peek.widget.as_mut().expect("just opened");
            (widget.selected, widget.focused) = (Some(TreeRow::Reference(reference)), Some(TreeRow::Reference(reference)));
        }
        self.editor_scrolled(cx);
        window.focus(&self.references_peek.tree_focus, cx);
        cx.notify();
    }

    /// `closeWidget`: the layout is stored, the zone removed, and the editor focused.
    pub(super) fn close_references_peek(&mut self, focus_editor: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(widget) = self.references_peek.widget.take() else { return };
        self.close_peek_zone(&widget.layout, Some(&LAYOUT));
        if focus_editor {
            window.focus(&self.editor_focus, cx);
        }
        cx.notify();
    }

    /// The controller's `onDidChangeModel`, run on every render: another model in the editor closes the peek.
    pub(super) fn follow_references_peek_editor(&mut self) {
        let Some(widget) = &self.references_peek.widget else { return };
        if self.editors.active().is_some_and(|editor| editor.model_id == widget.layout.model_id) {
            return;
        }
        let widget = self.references_peek.widget.take().expect("checked above");
        self.close_peek_zone(&widget.layout, None);
    }

    fn focused_reference_row(&self) -> Option<usize> {
        match self.references_peek.widget.as_ref()?.focused? {
            TreeRow::Reference(reference) => Some(reference),
            TreeRow::File(_) => None,
        }
    }

    /// `_revealReference`: the reference's file expanded in the tree and its line centered in the preview.
    fn reveal_peek_reference(&mut self, reference: usize, cx: &mut Context<Self>) {
        let Some(widget) = self.references_peek.widget.as_mut() else { return };
        let location = widget.model.references[reference].location.clone();
        let group = widget.model.references[reference].group;
        widget.expanded[group] = true;
        if widget.revealed != Some(reference) {
            widget.revealed = Some(reference);
            let mut documents = std::mem::take(&mut widget.documents);
            self.read_peek_document(&location.uri, &location.resource, &mut documents);
            self.references_peek.widget.as_mut().expect("checked above").documents = documents;
        }
        let body_height = self.peek_body_height(PeekKind::References);
        let rows = self.peek_rows();
        let line_count = self.preview_document(&location.uri).map_or(1, |document| document.lines.len());
        let line = location.selection().start.0.saturating_sub(1);
        let preview_scroll = self.peek_preview_scroll(line, line_count, body_height);
        let widget = self.references_peek.widget.as_mut().expect("checked above");
        widget.preview_scroll = preview_scroll;
        reveal_row(widget, &rows, TreeRow::Reference(reference), body_height);
        cx.notify();
    }

    fn preview_document(&self, uri: &str) -> Option<super::peek_view::PeekDocument<'_>> {
        self.peek_document(uri, &self.references_peek.widget.as_ref()?.documents)
    }

    /// The tree's rows: one file's references alone (`setInput(groups[0])`), else the files and the expanded ones'
    /// references.
    fn peek_rows(&self) -> Vec<TreeRow> {
        let Some(widget) = &self.references_peek.widget else { return Vec::new() };
        if widget.model.groups.len() == 1 {
            return widget.model.groups[0].children.iter().map(|&reference| TreeRow::Reference(reference)).collect();
        }
        let mut rows = Vec::new();
        for (group, file) in widget.model.groups.iter().enumerate() {
            rows.push(TreeRow::File(group));
            if widget.expanded[group] {
                rows.extend(file.children.iter().map(|&reference| TreeRow::Reference(reference)));
            }
        }
        rows
    }

    /// `onDidSelectReference` as the controller handles it.
    fn peek_reference_selected(&mut self, reference: usize, kind: Selected, window: &mut Window, cx: &mut Context<Self>) {
        let Some(widget) = &self.references_peek.widget else { return };
        match kind {
            Selected::Open => self.open_peek_reference(reference, false, false, window, cx),
            Selected::Side => self.open_peek_reference(reference, true, false, window, cx),
            Selected::Goto if widget.peek_mode => self.goto_peek_reference(reference, true, window, cx),
            Selected::Goto => self.open_peek_reference(reference, false, true, window, cx),
        }
    }

    /// `openReference`: the peek closes (unless to the side) and the reference's range is selected in its editor.
    // shortcut: one editor group, so to the side opens in it; the editor groups row adds the side.
    fn open_peek_reference(&mut self, reference: usize, side_by_side: bool, pinned: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(widget) = &self.references_peek.widget else { return };
        let location = widget.model.references[reference].location.clone();
        if !side_by_side {
            self.close_references_peek(true, window, cx);
        }
        let range = location.selection();
        let selection = json!({ "startLineNumber": range.start.0, "startColumn": range.start.1, "endLineNumber": range.end.0, "endColumn": range.end.1 });
        let _ = self.try_show_text_document(&location.resource, &json!({ "selection": selection, "pinned": pinned }), window, cx);
    }

    /// `_gotoReference`: the editor moves to the reference; in its own file the peek shows again there, in another
    /// the peek opens in that editor with the same model.
    fn goto_peek_reference(&mut self, reference: usize, pinned: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(widget) = self.references_peek.widget.take() else { return };
        let location = widget.model.references[reference].location.clone();
        self.close_peek_zone(&widget.layout, None);
        let range = location.selection();
        let selection = json!({ "startLineNumber": range.start.0, "startColumn": range.start.1, "endLineNumber": range.start.0, "endColumn": range.start.1 });
        if self.try_show_text_document(&location.resource, &json!({ "selection": selection, "pinned": pinned }), window, cx).is_err() {
            return;
        }
        let head = self.editors.active().expect("just shown").selection().head;
        let model = ReferencesModel::new(widget.model.references.iter().map(|reference| reference.location.clone()).collect(), &widget.model.title);
        self.open_references_peek(model, head..head, widget.peek_mode, window, cx);
    }

    /// `goToNextOrPreviousReference`: from the reference nearest the zone to the next or previous one.
    fn go_to_next_or_previous_reference(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(widget) = &self.references_peek.widget else { return };
        let Some(position) = self.peek_zone_position(&widget.layout) else { return };
        let editor = self.editors.active().expect("the peek's editor is active");
        let uri = editor.document().uri_string();
        let position = (position.line + 1, utf16_column(&editor.document().lines[position.line], position.column) + 1);
        let Some(source) = widget.model.nearest_reference(&uri, position) else { return };
        let target = widget.model.next_or_previous(source, forward);
        let editor_focused = self.editor_focus.is_focused(window);
        self.reveal_peek_reference(target, cx);
        let widget = self.references_peek.widget.as_mut().expect("checked above");
        (widget.selected, widget.focused) = (Some(TreeRow::Reference(target)), Some(TreeRow::Reference(target)));
        self.goto_peek_reference(target, false, window, cx);
        if editor_focused {
            window.focus(&self.editor_focus, cx);
        }
    }

    /// A tree row click: a file toggles, a reference shows in the preview; a double click goes to it.
    fn peek_row_clicked(&mut self, row: TreeRow, click_count: usize, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.references_peek.tree_focus, cx);
        let Some(widget) = self.references_peek.widget.as_mut() else { return };
        (widget.focused, widget.selected) = (Some(row), Some(row));
        match row {
            TreeRow::File(group) => widget.expanded[group] = !widget.expanded[group],
            TreeRow::Reference(reference) if click_count >= 2 => self.peek_reference_selected(reference, Selected::Goto, window, cx),
            TreeRow::Reference(reference) => self.reveal_peek_reference(reference, cx),
        }
        cx.notify();
    }

    /// The tree's list commands (listCommands.ts) while it has the focus; false leaves them to the other lists.
    pub(super) fn references_peek_list_command(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.references_peek.widget.is_none() || !self.references_peek.tree_focus.is_focused(window) {
            return false;
        }
        let rows = self.peek_rows();
        let body_height = self.peek_body_height(PeekKind::References);
        let page = ((body_height / ROW_HEIGHT).floor() as isize).max(1);
        let widget = self.references_peek.widget.as_mut().expect("checked above");
        let index = widget.focused.and_then(|focused| rows.iter().position(|row| *row == focused));
        let last = rows.len() as isize - 1;
        let target = match id {
            "list.focusDown" => index.map_or(0, |index| (index as isize + 1).min(last)),
            "list.focusUp" => index.map_or(0, |index| (index as isize - 1).max(0)),
            "list.focusPageDown" => index.map_or(0, |index| (index as isize + page).min(last)),
            "list.focusPageUp" => index.map_or(0, |index| (index as isize - page).max(0)),
            "list.focusFirst" => 0,
            "list.focusLast" => last,
            "list.select" | "list.toggleExpand" | "list.expand" | "list.collapse" => {
                let Some(row) = widget.focused else { return true };
                match (row, id) {
                    (TreeRow::File(group), "list.expand") => widget.expanded[group] = true,
                    (TreeRow::File(group), "list.collapse") => widget.expanded[group] = false,
                    (TreeRow::File(group), _) => widget.expanded[group] = !widget.expanded[group],
                    (TreeRow::Reference(reference), "list.collapse") => {
                        let group = widget.model.references[reference].group;
                        if widget.model.groups.len() > 1 {
                            widget.focused = Some(TreeRow::File(group));
                        }
                    }
                    (TreeRow::Reference(reference), "list.select") => {
                        widget.selected = Some(row);
                        self.reveal_peek_reference(reference, cx);
                    }
                    _ => {}
                }
                cx.notify();
                return true;
            }
            _ => return false,
        };
        if target < 0 {
            return true;
        }
        // `selectionNavigation`: the keys move the selection too, and the navigator shows a reference it lands on.
        let row = rows[target as usize];
        (widget.focused, widget.selected) = (Some(row), Some(row));
        reveal_row(widget, &rows, row, body_height);
        if let TreeRow::Reference(reference) = row {
            self.reveal_peek_reference(reference, cx);
        }
        cx.notify();
        true
    }

    /// The peek over its zone: the head with the revealed reference's file and the model's title and count, and the
    /// preview and the tree, or "No results".
    pub(super) fn references_peek_widget(&self, editor: &OpenEditor, content_left: f32, char_width: f32, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let widget = self.references_peek.widget.as_ref().filter(|widget| widget.layout.model_id == editor.model_id)?;
        let head = self.peek_head(self.references_peek_title(widget, cx), vec![self.peek_action("close", "Close", "closeReferenceSearch", cx).into_any_element()]);
        self.peek_zone(PeekKind::References, editor, content_left, char_width, head, |preview_width, body_height, cx| match widget.model.references.is_empty() {
            true => (self.peek_message("No results".into()), false),
            false => {
                let preview = self.peek_preview(widget, preview_width, body_height, char_width, window, cx);
                (div().size_full().flex().child(preview).child(self.peek_tree(widget, body_height, window, cx)).into_any_element(), true)
            }
        }, cx)
    }

    /// peekView.ts `_fillHead`: the revealed reference's file name, its folder and the model's title and count; a
    /// click on them opens the reference.
    fn references_peek_title(&self, widget: &PeekWidget, cx: &mut Context<Self>) -> Stateful<Div> {
        let (file_name, folder) = match widget.revealed.map(|reference| &widget.model.references[reference].location) {
            Some(location) => {
                let path = location.resource["path"].as_str().unwrap_or_default();
                let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
                // referencesWidget.ts `getUriLabel(dirname(uri))`: not relative, so the whole folder.
                (name.to_string(), self.tildify(std::path::Path::new(folder)))
            }
            None => (String::new(), String::new()),
        };
        let meta = match widget.model.references.is_empty() {
            true => String::new(),
            false => format!("{} ({})", widget.model.title, widget.model.references.len()),
        };
        let title = div().id("references-peek-title").cursor_pointer().on_click(cx.listener(|workbench, event: &ClickEvent, window, cx| {
            let Some(reference) = workbench.focused_reference_row().or_else(|| workbench.references_peek.widget.as_ref()?.revealed) else { return };
            let modifiers = event.modifiers();
            let kind = if modifiers.control || modifiers.platform || modifiers.alt { Selected::Side } else { Selected::Open };
            workbench.peek_reference_selected(reference, kind, window, cx);
        }));
        self.peek_title_parts(title, file_name, folder, meta)
    }

    /// `ILabelService.getUriLabel(dirname, { relative: true })`: the folder relative to its workspace folder.
    pub(super) fn peek_folder_label(&self, folder: &str) -> String {
        for root in self.search_folders() {
            let root = root.to_string_lossy();
            if folder == root {
                return String::new();
            }
            if let Some(relative) = folder.strip_prefix(&*root).and_then(|rest| rest.strip_prefix('/')) {
                return relative.to_string();
            }
        }
        self.tildify(std::path::Path::new(folder))
    }

    /// The preview of the revealed reference's file with the file's references marked (`reference-decoration`).
    fn peek_preview(&self, widget: &PeekWidget, width: f32, height: f32, char_width: f32, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let location = widget.revealed.map(|reference| (reference, &widget.model.references[reference].location));
        let document = location.and_then(|(_, location)| self.preview_document(&location.uri));
        let marks: Vec<(usize, usize, usize, usize)> = match location {
            Some((reference, _)) => widget.model.groups[widget.model.references[reference].group]
                .children
                .iter()
                .map(|&child| widget.model.references[child].location.selection())
                .map(|range| (range.start.0 - 1, range.start.1 - 1, range.end.0 - 1, range.end.1 - 1))
                .collect(),
            None => Vec::new(),
        };
        let cursor_line = location.map_or(0, |(_, location)| location.selection().start.0.saturating_sub(1));
        self.peek_preview_lines("references-peek-preview", document.as_deref(), &marks, cursor_line, widget.preview_scroll, width, height, char_width, window)
            .on_scroll_wheel(cx.listener(move |workbench, event: &ScrollWheelEvent, _, cx| {
                let delta = f32::from(event.delta.pixel_delta(px(LINE_HEIGHT)).y);
                let max = workbench.preview_line_count().map_or(0., |count| (count as f32 * LINE_HEIGHT - height).max(0.));
                let Some(widget) = workbench.references_peek.widget.as_mut() else { return };
                widget.preview_scroll = (widget.preview_scroll - delta).clamp(0., max);
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|workbench, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                if event.click_count == 2
                    && let Some(reference) = workbench.focused_reference_row().or_else(|| workbench.references_peek.widget.as_ref()?.revealed)
                {
                    let modifiers = event.modifiers;
                    let side = modifiers.control || modifiers.platform || modifiers.alt;
                    // `editor.stablePeek` keeps the peek open on a double click in the preview.
                    if side || workbench.setting("editor.stablePeek") != json!(true) {
                        workbench.open_peek_reference(reference, side, false, window, cx);
                    }
                }
            }))
    }

    fn preview_line_count(&self) -> Option<usize> {
        let widget = self.references_peek.widget.as_ref()?;
        let location = &widget.model.references[widget.revealed?].location;
        Some(self.preview_document(&location.uri)?.lines.len())
    }


    /// referencesTree.ts: the files with their folder and count, the references as their line with the range marked.
    fn peek_tree(&self, widget: &PeekWidget, height: f32, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.references_peek.tree_focus.is_focused(window);
        let rows = self.peek_rows();
        let single = widget.model.groups.len() == 1;
        let first = (widget.tree_scroll / ROW_HEIGHT).floor() as usize;
        let last = (((widget.tree_scroll + height) / ROW_HEIGHT).ceil() as usize).min(rows.len());
        let mut elements = Vec::new();
        for (index, &row) in rows.iter().enumerate().take(last).skip(first) {
            elements.push(self.peek_tree_row(widget, row, index, single, focused, cx));
        }
        div()
            .id("references-peek-tree")
            .track_focus(&self.references_peek.tree_focus)
            .flex_1()
            .min_w_0()
            .h(px(height))
            .relative()
            .overflow_hidden()
            .font_family(UI_FONT)
            .text_size(px(13.))
            .line_height(px(ROW_HEIGHT))
            .bg(self.color("peekViewResult.background"))
            .text_color(self.color("peekViewResult.lineForeground"))
            .children(elements)
            .on_scroll_wheel(cx.listener(move |workbench, event: &ScrollWheelEvent, _, cx| {
                let delta = f32::from(event.delta.pixel_delta(px(ROW_HEIGHT)).y);
                let count = workbench.peek_rows().len();
                let Some(widget) = workbench.references_peek.widget.as_mut() else { return };
                widget.tree_scroll = (widget.tree_scroll - delta).clamp(0., (count as f32 * ROW_HEIGHT - height).max(0.));
                cx.stop_propagation();
                cx.notify();
            }))
    }

    fn peek_tree_row(&self, widget: &PeekWidget, row: TreeRow, index: usize, single: bool, tree_focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let is_focused = widget.focused == Some(row);
        let is_selected = widget.selected == Some(row);
        let file_foreground = self.color("peekViewResult.fileForeground");
        let (twistie, depth, content) = match row {
            TreeRow::File(group) => {
                let file = &widget.model.groups[group];
                let path = file.resource["path"].as_str().unwrap_or_default();
                let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
                let count = file.children.len();
                let chevron = if widget.expanded[group] { "chevron-down" } else { "chevron-right" };
                let label = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(file_foreground)
                    .child(div().flex_none().child(name.to_string()))
                    .child(div().ml(px(6.5)).text_size(px(11.7)).opacity(0.7).min_w_0().overflow_hidden().text_ellipsis().child(self.peek_folder_label(folder)));
                let badge = div()
                    .mr(px(12.))
                    .min_w(px(18.))
                    .h(px(18.))
                    .px(px(5.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(11.))
                    .text_size(px(11.))
                    .line_height(px(18.))
                    .bg(self.color("badge.background"))
                    .text_color(self.foreground_or("badge.foreground"))
                    .when_some(self.theme.color("contrastBorder"), |badge, border| badge.border_1().border_color(border))
                    .child(count.to_string());
                (Some(chevron), 1, div().flex_1().min_w_0().flex().items_center().child(label).child(badge))
            }
            TreeRow::Reference(reference) => {
                let location = &widget.model.references[reference].location;
                let label = match self.peek_reference_preview(location) {
                    Some((before, inside, after)) => div()
                        .flex()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(div().flex_none().child(before))
                        .child(div().flex_none().text_color(file_foreground).when_some(self.theme.color("peekViewResult.matchHighlightBackground"), |span, color| span.bg(color)).child(inside))
                        .child(div().min_w_0().overflow_hidden().text_ellipsis().child(after)),
                    None => {
                        let path = location.resource["path"].as_str().unwrap_or_default();
                        let name = path.rsplit('/').next().unwrap_or_default();
                        let range = location.selection();
                        div().child(format!("{name}:{}:{}", range.start.0 + 1, range.start.1 + 1))
                    }
                };
                (None, if single { 1 } else { 2 }, label)
            }
        };
        let twistie = match twistie {
            Some(icon) => self.icon(icon, 16., 16., self.foreground_or("icon.foreground")),
            None => div().size(px(16.)).flex_none(),
        };
        let selection = match tree_focused {
            true => self.theme.color("peekViewResult.selectionBackground"),
            false => self.theme.color("list.inactiveSelectionBackground"),
        };
        let focus_outline = self.theme.color("list.focusOutline").or(self.theme.color("focusBorder")).filter(|_| tree_focused && is_focused);
        let top = index as f32 * ROW_HEIGHT - widget.tree_scroll;
        div()
            .id(("references-peek-row", index))
            .absolute()
            .left_0()
            .right_0()
            .top(px(top))
            .h(px(ROW_HEIGHT))
            .cursor_pointer()
            .flex()
            .items_center()
            .pl(px(8. * (depth - 1) as f32 + 4.))
            .when(!is_selected, |row| row.hover(|row| row.bg(self.color("list.hoverBackground"))))
            .when(is_selected, |row| row.when_some(selection, |row, color| row.bg(color)).when(tree_focused, |row| row.text_color(self.color("peekViewResult.selectionForeground"))))
            .when_some(focus_outline, |row, color| row.child(div().absolute().inset_0().border_1().border_color(color)))
            .child(twistie)
            .child(content.ml(px(6.)))
            .on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| workbench.peek_row_clicked(row, event.click_count(), window, cx)))
            .into_any_element()
    }

    /// `FilePreview.preview`: from the start of the word 8 columns before the range, the range, and the rest of the
    /// line, leading and trailing whitespace dropped.
    fn peek_reference_preview(&self, location: &Location) -> Option<(String, String, String)> {
        let document = self.preview_document(&location.uri)?;
        let range = location.selection();
        let line = document.lines.get(range.start.0 - 1)?;
        let start = char_column(line, range.start.1 - 1);
        let end_line = document.lines.get(range.end.0 - 1)?;
        let end = char_column(end_line, range.end.1 - 1);
        let chars: Vec<char> = line.chars().collect();
        // `getWordUntilPosition` at the column 8 before: the word that column is in, from its start.
        let mut from = start.saturating_sub(8);
        while from > 0 && (chars[from - 1].is_alphanumeric() || chars[from - 1] == '_') {
            from -= 1;
        }
        let before: String = chars[from..start].iter().collect::<String>().trim_start().to_string();
        let inside = match range.start.0 == range.end.0 {
            true => chars[start..end.max(start)].iter().collect(),
            false => document.lines[range.start.0 - 1..range.end.0].join("\n").chars().skip(start).collect::<String>(),
        };
        let after: String = end_line.chars().skip(end).collect::<String>().trim_end().to_string();
        Some((before, inside, after))
    }
}

#[derive(Clone, Copy)]
enum Selected {
    Open,
    Side,
    Goto,
}

/// `tree.reveal`: the row scrolled into the tree's view.
fn reveal_row(widget: &mut PeekWidget, rows: &[TreeRow], row: TreeRow, height: f32) {
    let Some(index) = rows.iter().position(|candidate| *candidate == row) else { return };
    let top = index as f32 * ROW_HEIGHT;
    if top < widget.tree_scroll {
        widget.tree_scroll = top;
    } else if top + ROW_HEIGHT > widget.tree_scroll + height {
        widget.tree_scroll = top + ROW_HEIGHT - height;
    }
}

#[cfg(test)]
mod tests;
