//! referencesController.ts and referencesWidget.ts: the references peek, a zone below the symbol with a preview of the
//! selected reference on the left and the references tree (referencesTree.ts) on the right.

use std::ops::Range;

use gpui::{AnyElement, ClickEvent, Context, Div, Stateful, FocusHandle, MouseButton, MouseDownEvent, PathBuilder, ScrollWheelEvent, StyledText, Task, Window, canvas, div, point, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::{LINE_HEIGHT, OpenEditor, Position, Selection, TextDocument, VERTICAL_SCROLLBAR_WIDTH, VerticalReveal, char_column, expand_tabs, utf16_column, visible_column};
use crate::references_model::{Location, ReferencesModel};

/// zoneWidget.ts: the arrow is a third of a line high; peekView.ts: the head is 1.2 lines.
const ARROW_HEIGHT: f32 = 6.;
const HEAD_HEIGHT: f32 = 22.;
/// referencesTree.ts `Delegate.getHeight`.
const ROW_HEIGHT: f32 = 23.;
const LAYOUT_KEY: &str = "peekViewLayout";
const UI_FONT: &str = ".SystemUIFont";

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
    model_id: u32,
    zone_id: u32,
    model: ReferencesModel,
    peek_mode: bool,
    height_in_lines: f32,
    ratio: f32,
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
    /// The sash being dragged: which one, the pointer where the drag started, the size then and the widget's width.
    sash_drag: Option<(PeekSash, f32, f32, f32)>,
    /// sash.ts: the hover border shows 300ms after the pointer enters a sash.
    hovered_sash: Option<PeekSash>,
    sash_hover: Option<Task<()>>,
}

/// zoneWidget.ts `_resizeSash` below the zone and the referencesWidget.ts split view sash between preview and tree.
#[derive(Clone, Copy, PartialEq, Debug)]
enum PeekSash {
    Zone,
    Split,
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
        let position = self.references_peek.widget.as_ref().and_then(|widget| self.peek_position(widget));
        self.close_references_peek(true, window, cx);
        if position.is_some_and(|position| range.start <= position && position <= range.end) {
            return;
        }
        let layout: Value = self.profile_storage.lock().unwrap().get(LAYOUT_KEY).and_then(|raw| serde_json::from_str(raw).ok()).unwrap_or_default();
        let ratio = layout["ratio"].as_f64().filter(|ratio| *ratio != 0.).unwrap_or(0.7) as f32;
        let stored_height = layout["heightInLines"].as_f64().filter(|height| *height != 0.).unwrap_or(18.) as f32;
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("the peek opens in the active editor");
        // zoneWidget.ts `_getMaximumHeightInLines`.
        let height_in_lines = stored_height.min(12f32.max(viewport.height / viewport.line_height * 0.8));
        let zone_id = editor.add_view_zone(Some(range.start), height_in_lines * viewport.line_height);
        // `_showImpl`: the selection becomes the range, its lines and the zone are revealed.
        editor.set_selections(vec![Selection { anchor: range.start, head: range.end }]);
        let last = editor.document.lines.len() - 1;
        match range.end.line == last {
            true => editor.reveal_range(Position::new(last, 0), Position::new(last, 0), VerticalReveal::NearTop, viewport),
            false => editor.reveal_range(Position::new(range.start.line, 0), Position::new(range.end.line + 1, 0), VerticalReveal::Simple, viewport),
        }
        self.host.send_selection(editor, "api");
        let model_id = editor.model_id;
        let uri = editor.document.uri_string();
        let start = (range.start.line + 1, utf16_column(&editor.document.lines[range.start.line], range.start.column) + 1);
        let nearest = model.nearest_reference(&uri, start);
        let expanded = vec![false; model.groups.len()];
        self.references_peek.widget = Some(PeekWidget {
            model_id,
            zone_id,
            model,
            peek_mode,
            height_in_lines,
            ratio,
            focused: None,
            selected: None,
            expanded,
            tree_scroll: 0.,
            revealed: None,
            preview_scroll: 0.,
            documents: Vec::new(),
            sash_drag: None,
            hovered_sash: None,
            sash_hover: None,
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
        let layout = json!({ "ratio": widget.ratio, "heightInLines": widget.height_in_lines });
        self.profile_storage.lock().unwrap().set(LAYOUT_KEY, layout.to_string());
        if let Some(editor) = self.editors.editors.iter_mut().chain(&mut self.editors.models).find(|editor| editor.model_id == widget.model_id) {
            editor.remove_view_zone(widget.zone_id);
        }
        if focus_editor {
            window.focus(&self.editor_focus, cx);
        }
        cx.notify();
    }

    /// The controller's `onDidChangeModel`, run on every render: another model in the editor closes the peek.
    pub(super) fn follow_references_peek_editor(&mut self) {
        let Some(widget) = &self.references_peek.widget else { return };
        if self.editors.active().is_some_and(|editor| editor.model_id == widget.model_id) {
            return;
        }
        let widget = self.references_peek.widget.take().expect("checked above");
        if let Some(editor) = self.editors.editors.iter_mut().chain(&mut self.editors.models).find(|editor| editor.model_id == widget.model_id) {
            editor.remove_view_zone(widget.zone_id);
        }
    }

    /// The zone's position (`ZoneWidget.position`).
    fn peek_position(&self, widget: &PeekWidget) -> Option<Position> {
        let editor = self.editors.editors.iter().find(|editor| editor.model_id == widget.model_id)?;
        editor.view_zone(widget.zone_id)?.anchor
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
            if self.editors.editors.iter().chain(&self.editors.models).all(|editor| editor.document.uri_string() != location.uri)
                && widget.documents.iter().all(|document| document.uri_string() != location.uri)
                && let Ok((_, path)) = super::host_requests::uri_parts(&location.resource)
                && let Ok(document) = TextDocument::read(&path, &self.languages, &self.settings.lock().unwrap())
            {
                widget.documents.push(document);
            }
        }
        let viewport = self.editors.viewport;
        let body_height = self.peek_body_height();
        let rows = self.peek_rows();
        let line_count = self.preview_document(&location.uri).map_or(1, |document| document.lines.len());
        let widget = self.references_peek.widget.as_mut().expect("checked above");
        let line = location.selection().start.0.saturating_sub(1);
        // `revealRangeInCenter` without scrolling past the last line.
        let max = (line_count as f32 * viewport.line_height - body_height).max(0.);
        widget.preview_scroll = (line as f32 * viewport.line_height + viewport.line_height / 2. - body_height / 2.).clamp(0., max);
        reveal_row(widget, &rows, TreeRow::Reference(reference), body_height);
        cx.notify();
    }

    fn preview_document(&self, uri: &str) -> Option<&TextDocument> {
        let widget = self.references_peek.widget.as_ref()?;
        let open = self.editors.editors.iter().chain(&self.editors.models).map(|editor| &editor.document);
        open.chain(&widget.documents).find(|document| document.uri_string() == uri)
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

    fn peek_body_height(&self) -> f32 {
        let Some(widget) = &self.references_peek.widget else { return 0. };
        let viewport = self.editors.viewport;
        // peekView.ts `_doLayout`: the container less the head and the body's 1px top border.
        widget.height_in_lines * viewport.line_height - 2. * ARROW_HEIGHT - (HEAD_HEIGHT + 1.)
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
        if let Some(editor) = self.editors.editors.iter_mut().chain(&mut self.editors.models).find(|editor| editor.model_id == widget.model_id) {
            editor.remove_view_zone(widget.zone_id);
        }
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
        let Some(position) = self.peek_position(widget) else { return };
        let editor = self.editors.active().expect("the peek's editor is active");
        let uri = editor.document.uri_string();
        let position = (position.line + 1, utf16_column(&editor.document.lines[position.line], position.column) + 1);
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
        let body_height = self.peek_body_height();
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

    /// The peek over its zone: the arrow, then the frame with the head and the body.
    pub(super) fn references_peek_widget(&self, editor: &OpenEditor, content_left: f32, char_width: f32, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let widget = self.references_peek.widget.as_ref().filter(|widget| widget.model_id == editor.model_id)?;
        let viewport = self.editors.viewport;
        let zone = editor.view_zone(widget.zone_id)?;
        let top = editor.view_zone_top(widget.zone_id, viewport)? - editor.scroll.top;
        let width = content_left + viewport.width - VERTICAL_SCROLLBAR_WIDTH;
        let frame = self.color("peekView.border");
        // zoneWidget.ts `show`: the arrow is not pretty at column 1 and moves to column 2.
        let anchor = zone.anchor?;
        let line = &editor.document.lines[anchor.line];
        let arrow_column = anchor.column.max(1).min(line.chars().count().max(1));
        let arrow_x = content_left + visible_column(line, arrow_column, editor.document.indentation.tab_size) as f32 * char_width - editor.scroll.left;
        let arrow = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let (x, y) = (bounds.origin.x, bounds.origin.y);
                let mut path = PathBuilder::fill();
                path.move_to(point(x, y + px(ARROW_HEIGHT)));
                path.line_to(point(x + px(2. * ARROW_HEIGHT), y + px(ARROW_HEIGHT)));
                path.line_to(point(x + px(ARROW_HEIGHT), y));
                path.close();
                if let Ok(path) = path.build() {
                    window.paint_path(path, frame);
                }
            },
        )
        .absolute()
        .top_0()
        .left(px(arrow_x - ARROW_HEIGHT))
        .w(px(2. * ARROW_HEIGHT))
        .h(px(ARROW_HEIGHT));
        let container_height = zone.height - 2. * ARROW_HEIGHT;
        let body_height = container_height - HEAD_HEIGHT - 1.;
        let body = match widget.model.references.is_empty() {
            true => div().size_full().pt(px(39.)).flex().justify_center().text_size(px(13.)).font_family(UI_FONT).child("No results"),
            false => {
                let preview_width = ((width * widget.ratio).max(200.)).min(width - 100.);
                div()
                    .size_full()
                    .relative()
                    .flex()
                    .child(self.peek_preview(widget, preview_width, body_height, char_width, window, cx))
                    .child(self.peek_tree(widget, body_height, window, cx))
                    .child(self.peek_sash(widget, PeekSash::Split, [preview_width - 2., 0., 4., body_height], (preview_width, width), cx))
            }
        };
        let container = div()
            .absolute()
            .top(px(ARROW_HEIGHT))
            .left_0()
            .right_0()
            .h(px(container_height + 2.))
            .overflow_hidden()
            .border_t_1()
            .border_b_1()
            .border_color(frame)
            .child(self.peek_head(widget, cx))
            .child(div().h(px(body_height + 1.)).border_t_1().border_color(frame).child(body));
        let widget = div()
            .id("references-peek")
            .absolute()
            .left_0()
            .top(px(top))
            .w(px(width))
            .h(px(zone.height))
            .cursor_default()
            .occlude()
            .child(container)
            .child(arrow)
            // zoneWidget.ts `getHorizontalSashTop`: half the arrows' height above the zone's end.
            .child(self.peek_sash(widget, PeekSash::Zone, [0., zone.height - ARROW_HEIGHT - 2., width, 4.], (widget.height_in_lines, width), cx))
            .children(widget.sash_drag.is_some().then(|| self.peek_sash_drag(cx)));
        Some(widget.into_any_element())
    }

    /// A `.monaco-sash` (sash.css): 4px, the mac resize cursor, `sash.hoverBorder` after the hover delay and while dragged.
    fn peek_sash(&self, widget: &PeekWidget, sash: PeekSash, [x, y, w, h]: [f32; 4], (size, width): (f32, f32), cx: &mut Context<Self>) -> AnyElement {
        let active = widget.hovered_sash == Some(sash) || widget.sash_drag.is_some_and(|(dragged, ..)| dragged == sash);
        let hover_border = self.theme.color("sash.hoverBorder").unwrap_or(self.color("focusBorder"));
        let cursor = match sash {
            PeekSash::Zone => gpui::CursorStyle::ResizeRow,
            PeekSash::Split => gpui::CursorStyle::ResizeColumn,
        };
        div()
            .id(match sash {
                PeekSash::Zone => "references-peek-zone-sash",
                PeekSash::Split => "references-peek-split-sash",
            })
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(w))
            .h(px(h))
            .cursor(cursor)
            .when(active, |element| element.bg(hover_border))
            .on_hover(cx.listener(move |workbench, hovered: &bool, window, cx| {
                let Some(widget) = workbench.references_peek.widget.as_mut() else { return };
                widget.hovered_sash = None;
                widget.sash_hover = hovered.then(|| {
                    cx.spawn_in(window, async move |workbench, cx| {
                        cx.background_executor().timer(super::part_grid::SASH_HOVER_DELAY).await;
                        let _ = workbench.update(cx, |workbench, cx| {
                            if let Some(widget) = workbench.references_peek.widget.as_mut() {
                                widget.hovered_sash = Some(sash);
                                cx.notify();
                            }
                        });
                    })
                });
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |workbench, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let Some(widget) = workbench.references_peek.widget.as_mut() else { return };
                    let pointer = match sash {
                        PeekSash::Zone => f32::from(event.position.y),
                        PeekSash::Split => f32::from(event.position.x),
                    };
                    widget.sash_drag = Some((sash, pointer, size, width));
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// Follows the pointer anywhere in the window while a peek sash drags.
    fn peek_sash_drag(&self, cx: &mut Context<Self>) -> AnyElement {
        let workbench = cx.entity();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = workbench.clone();
                window.on_mouse_event(move |event: &gpui::MouseMoveEvent, _, _, cx| {
                    moved.update(cx, |workbench, cx| workbench.drag_peek_sash(f32::from(event.position.x), f32::from(event.position.y), cx));
                });
                let released = workbench.clone();
                window.on_mouse_event(move |_: &gpui::MouseUpEvent, _, _, cx| {
                    released.update(cx, |workbench, cx| {
                        if let Some(widget) = workbench.references_peek.widget.as_mut() {
                            widget.sash_drag = None;
                            cx.notify();
                        }
                    });
                });
            },
        )
        .size_0()
        .into_any_element()
    }

    /// zoneWidget.ts `_initSash` `onDidChange`: whole lines between 5 and 35 exclusive; the split view keeps the
    /// preview at least 200px and the tree at least 100px wide, and the ratio follows (`onDidSashChange`).
    fn drag_peek_sash(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let line_height = self.editors.viewport.line_height;
        let Some(widget) = self.references_peek.widget.as_mut() else { return };
        let Some((sash, start, size, width)) = widget.sash_drag else { return };
        match sash {
            PeekSash::Zone => {
                let delta = ((y - start) / line_height).trunc();
                let height_in_lines = size + delta;
                if height_in_lines <= 5. || height_in_lines >= 35. || height_in_lines == widget.height_in_lines {
                    return;
                }
                widget.height_in_lines = height_in_lines;
                let (model_id, zone_id) = (widget.model_id, widget.zone_id);
                let editor = self.editors.editors.iter_mut().find(|editor| editor.model_id == model_id).expect("the peek's editor is open");
                editor.view_zones.iter_mut().find(|zone| zone.id == zone_id).expect("the peek's zone").height = height_in_lines * line_height;
            }
            PeekSash::Split => {
                let preview_width = (size + x - start).max(200.).min(width - 100.);
                widget.ratio = preview_width / width;
            }
        }
        cx.notify();
    }

    /// peekView.ts `_fillHead`: the revealed reference's file name, its folder and the model's title and count; a
    /// click on them opens the reference.
    fn peek_head(&self, widget: &PeekWidget, cx: &mut Context<Self>) -> Div {
        let (file_name, folder) = match widget.revealed.map(|reference| &widget.model.references[reference].location) {
            Some(location) => {
                let path = location.resource["path"].as_str().unwrap_or_default();
                let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
                (name.to_string(), self.peek_folder_label(folder))
            }
            None => (String::new(), String::new()),
        };
        let meta = match widget.model.references.is_empty() {
            true => String::new(),
            false => format!("{} ({})", widget.model.title, widget.model.references.len()),
        };
        let description = self.color("peekViewTitleDescription.foreground");
        let title = div()
            .id("references-peek-title")
            .ml(px(20.))
            .min_w_0()
            .overflow_hidden()
            .flex()
            .items_baseline()
            .text_size(px(13.))
            .cursor_pointer()
            .child(div().whitespace_nowrap().text_color(self.color("peekViewTitleLabel.foreground")).child(file_name))
            .when(!folder.is_empty(), |title| title.child(div().whitespace_nowrap().ml(px(0.5 * 0.9 * 13.)).text_size(px(0.9 * 13.)).text_color(description).child(folder)))
            .when(!meta.is_empty(), |title| title.child(div().whitespace_nowrap().text_color(description).child(format!("\u{2009}-\u{2009}{meta}"))))
            .on_click(cx.listener(|workbench, event: &ClickEvent, window, cx| {
                let Some(reference) = workbench.focused_reference_row().or_else(|| workbench.references_peek.widget.as_ref()?.revealed) else { return };
                let modifiers = event.modifiers();
                let kind = if modifiers.control || modifiers.platform || modifiers.alt { Selected::Side } else { Selected::Open };
                workbench.peek_reference_selected(reference, kind, window, cx);
            }));
        let actions = div().flex_1().flex().justify_end().items_center().pr(px(2.)).child(self.peek_action("close", "Close", "closeReferenceSearch", cx));
        div()
            .h(px(HEAD_HEIGHT))
            .line_height(px(HEAD_HEIGHT))
            .flex()
            .justify_between()
            .flex_nowrap()
            .font_family(UI_FONT)
            .bg(self.color("peekViewTitle.background"))
            .child(title)
            .child(actions)
    }

    /// `ILabelService.getUriLabel(dirname, { relative: true })`: the folder relative to its workspace folder.
    fn peek_folder_label(&self, folder: &str) -> String {
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

    /// The preview of the revealed reference's file: its lines with their syntax colors, the line numbers in the
    /// gutter and the file's references marked (`reference-decoration`).
    // shortcut: drawn lines, not an embedded editor; it scrolls with the wheel but cannot be edited or focused and has
    // no indent guides, folding or scrollbars. A second editor view replaces it once the editor groups row lands.
    fn peek_preview(&self, widget: &PeekWidget, width: f32, height: f32, char_width: f32, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let background = self.color("peekViewEditor.background");
        let preview = div().id("references-peek-preview").w(px(width)).h(px(height)).flex_none().overflow_hidden().relative().bg(background);
        let Some(reference) = widget.revealed else { return preview };
        let location = &widget.model.references[reference].location;
        let Some(document) = self.preview_document(&location.uri) else { return preview };
        let viewport = self.editors.viewport;
        let line_height = viewport.line_height;
        let lines = &document.lines;
        let tab_size = document.indentation.tab_size;
        let content_left = self.preview_content_left(lines.len(), window);
        let first = (widget.preview_scroll / line_height).floor() as usize;
        let last = (((widget.preview_scroll + height) / line_height).ceil() as usize).min(lines.len());
        let cursor_line = location.selection().start.0.saturating_sub(1);
        let group = &widget.model.groups[widget.model.references[reference].group];
        let decorations: Vec<(usize, usize, usize, usize)> = group
            .children
            .iter()
            .map(|&child| widget.model.references[child].location.selection())
            .map(|range| (range.start.0 - 1, range.start.1 - 1, range.end.0 - 1, range.end.1 - 1))
            .collect();
        let match_background = self.theme.color("peekViewEditor.matchHighlightBackground");
        let match_border = self.theme.color("peekViewEditor.matchHighlightBorder");
        let line_border = self.color("editor.lineHighlightBorder");
        let mut rows = Vec::new();
        for index in first..last {
            let text = &lines[index];
            let mut content = div().absolute().top_0().h_full().left(px(content_left)).right_0();
            for &(start_line, start, end_line, end) in decorations.iter().filter(|range| range.0 <= index && index <= range.2) {
                let from = if start_line == index { char_column(text, start) } else { 0 };
                let to = if end_line == index { char_column(text, end) } else { text.chars().count() };
                let left = visible_column(text, from, tab_size) as f32 * char_width;
                let right = visible_column(text, to, tab_size) as f32 * char_width;
                content = content.child(
                    div()
                        .absolute()
                        .top_0()
                        .h_full()
                        .left(px(left))
                        .w(px(right - left))
                        .when_some(match_background, |mark, color| mark.bg(color))
                        .when_some(match_border, |mark, color| mark.border_2().border_color(color)),
                );
            }
            let highlights = document.tokens.highlights(&self.theme, &self.languages, lines, index, tab_size);
            content = content.child(div().absolute().top_0().left_0().whitespace_nowrap().child(StyledText::new(expand_tabs(text, tab_size)).with_highlights(highlights)));
            let number_color = self.color(if index == cursor_line { "editorLineNumber.activeForeground" } else { "editorLineNumber.foreground" });
            let number = div().absolute().top_0().left_0().w(px(content_left - 26. + line_height)).flex().justify_end().text_color(number_color).child((index + 1).to_string());
            rows.push(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(index as f32 * line_height - widget.preview_scroll))
                    .h(px(line_height))
                    .when(index == cursor_line, |row| row.child(div().absolute().top_0().bottom_0().left(px(content_left)).right_0().border_2().border_color(line_border)))
                    .child(number)
                    .child(content),
            );
        }
        preview
            .font_family(super::editor_view::EDITOR_FONT)
            .text_size(px(super::editor_view::EDITOR_FONT_SIZE * self.editor_zoom.multiplier()))
            .line_height(px(line_height))
            .text_color(self.color("editor.foreground"))
            .child(div().absolute().top_0().left_0().h_full().w(px(content_left - 26. + line_height + 26.)).bg(self.color("peekViewEditorGutter.background")))
            .children(rows)
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

    /// The embedded editor's gutter takes the editor's options: the glyph margin, the line numbers and the folding.
    fn preview_content_left(&self, line_count: usize, window: &Window) -> f32 {
        let metrics = super::editor_view::EditorMetrics::measure(window, line_count, super::editor_view::LineNumbers::On, self.editor_zoom.multiplier());
        metrics.content_left()
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
