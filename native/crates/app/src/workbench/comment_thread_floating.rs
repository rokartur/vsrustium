//! commentThreadFloatingWidget.ts and commentThreadConnector.ts: with `comments.thread.experimental.style` `floating`
//! an expanded thread shows in a window over the editor instead of a zone. The window is placed by
//! comment_thread_floating_layout.rs, moves with its code as the editor scrolls, is moved by dragging its title,
//! resized from its edges, put back by a double click on its title, and a line ties it to the outlined code.
//! Coordinates: x from the left of the code area (`contentLeft`), y from the editor's top or the document's top.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{AnyElement, Bounds, BoxShadow, ContentMask, Context, CursorStyle, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Rgba, ScrollHandle, ScrollWheelEvent, Window, canvas, div, point, prelude::*, px, rgba, size};
use serde_json::Value;

use super::Workbench;
use super::comment_node::{REACTIONS_ROW, comment_text};
use super::comment_thread_floating_layout::{
    BesideLines, ContentHeights, ContentWidths, EDGE_MARGIN, Layout, MIN_HEIGHT, Obstacle, OutlineCorner, PlacementRequest, Point, Rect, Size, SizeModel, SizeRange, Span, centered_top, clamp, connector_points,
    default_floating_widget_width, floating_widget_placement, is_in_the_way_of, placement_beside_lines, reveal_scroll_top, rows_outline, width_beside_code,
};
use super::comment_thread_widget::{BODY_LINE, COMMENT_CHROME, EDIT_FORM, Place, REPLY_BUTTON_FORM, REPLY_EDITOR_FORM, ThreadWidget};
use super::peek_view::UI_FONT;
use crate::editor::injected_text::InjectionOwner;
use crate::editor::{OpenEditor, VERTICAL_SCROLLBAR_WIDTH, Viewport, visible_column};
use crate::menus::{MenuAction, MenuId, menu_actions};

#[cfg(test)]
mod tests;

/// `--vscode-cornerRadius-large`, the widget's corners, and `--vscode-cornerRadius-small`, the outline's.
const CORNER_RADIUS: f32 = 8.;
const OUTLINE_CORNER_RADIUS: f32 = 4.;
/// `--vscode-strokeThickness`: the frame, the outline and the connector.
const STROKE: f32 = 1.;
/// Space between the commented code and its outline.
const OUTLINE_PADDING: f32 = 2.;
/// How far the outline may reach into the gutter, so code at the first column keeps its outline.
const CONNECTOR_GUTTER_BLEED: f32 = OUTLINE_PADDING + 2.;
/// sash.ts `size` of the ResizableHTMLElement's sashes, centered on the edges.
const SASH_SIZE: f32 = 4.;
/// commentsController.ts `_floatingWidgetLayouts`, an `LRUCache` of 500.
const LAYOUT_CACHE_SIZE: usize = 500;
/// commentReply.ts: the "Reply..." button's 1px border and 12px padding each side, and `.comment-form` 20px margins.
const REPLY_BUTTON_CHROME: f32 = 2. + 24.;
const FORM_MARGINS: f32 = 40.;
/// The head's 20px title margin, 2px actions padding and each 22px action with its 4px margin.
const HEAD_CHROME: f32 = 20. + 2.;
const HEAD_ACTION: f32 = 22. + 4.;
/// `.review-comment`: 20px padding, the details' 20px padding and 16px on the right.
const COMMENT_CHROME_WIDTH: f32 = 20. + 20. + 16.;

/// One floating widget: the `ReviewFloatingWidget` state that lives while its thread is expanded.
pub(super) struct Floating {
    layout: Option<Layout>,
    size: SizeModel,
    /// `_resizable.size`.
    width: f32,
    height: f32,
    /// The left as last rendered (`_renderedLeft`) and the document top.
    left: f32,
    top: f32,
    comments_height: f32,
    /// Placement still to do: Some(true) where the user last put it, Some(false) placed automatically (double click).
    pending_placement: Option<bool>,
    pending_reveal: bool,
    comments_scroll: ScrollHandle,
    /// The comments' scroll offset the last wheel event saw: a changed one means the comments took the wheel.
    comments_offset: Rc<Cell<f32>>,
}

impl Floating {
    pub(super) fn new(reveal: bool) -> Floating {
        Floating {
            layout: None,
            size: SizeModel::default(),
            width: 0.,
            height: 0.,
            left: 0.,
            top: 0.,
            comments_height: 0.,
            pending_placement: Some(true),
            pending_reveal: reveal,
            comments_scroll: ScrollHandle::new(),
            comments_offset: Rc::new(Cell::new(0.)),
        }
    }
}

/// The floating widgets' shared state: the layouts the controller keeps per thread, the widget in front, the gesture.
#[derive(Default)]
pub(super) struct FloatingWidgets {
    layouts: Vec<(String, Layout)>,
    front: Option<WidgetKey>,
    gesture: Option<Gesture>,
}

type WidgetKey = (u64, u64, u32);

/// A title drag or an edge resize in progress, in window pixels.
struct Gesture {
    key: WidgetKey,
    start: (f32, f32),
    kind: GestureKind,
}

enum GestureKind {
    Drag { layout: Layout, top: f32, left: f32, head_height: f32 },
    Resize(ResizeSession),
}

/// `IResizeSession`; `edges` is the sash that was grabbed: north, east, south, west.
struct ResizeSession {
    layout: Layout,
    top: f32,
    left: f32,
    width: f32,
    height: f32,
    width_range: SizeRange,
    edges: [bool; 4],
}

/// What the thread's content needs, measured once per frame: its widths and the parts of its height.
struct Content {
    widths: ContentWidths,
    fixed: f32,
    /// Each comment's height without its body, and the widths of its body's lines.
    comments: Vec<(f32, Vec<f32>)>,
    is_editing: bool,
    is_new: bool,
}

impl Content {
    /// `measureHeights` at a widget width: body lines wrap at the comment's text width.
    fn heights(&self, width: f32) -> ContentHeights {
        let text_width = (width - 2. * STROKE - COMMENT_CHROME_WIDTH).max(1.);
        let mut comments = 0.;
        for (chrome, lines) in &self.comments {
            let rows: f32 = lines.iter().map(|line| (line / text_width).ceil().max(1.)).sum();
            comments += chrome + rows * BODY_LINE;
        }
        ContentHeights { fixed: self.fixed, comments }
    }
}

/// `CommentedCodeGeometry`: where the commented code shows in an editor, its lines' text measured from their columns.
struct CodeGeometry<'a> {
    editor: &'a OpenEditor,
    viewport: Viewport,
    range: Option<&'a Value>,
}

/// The commented lines, 0-based: a range ending at a further line's start doesn't cover that line.
struct CodeLines {
    start: usize,
    end: usize,
    start_column: usize,
    end_column: usize,
    empty: bool,
}

impl CodeGeometry<'_> {
    fn lines(&self) -> Option<CodeLines> {
        let range = self.range?;
        let last = self.editor.document().lines.len().saturating_sub(1);
        let number = |key: &str| (range[key].as_u64().unwrap_or(1) as usize).max(1) - 1;
        let (start, end) = (number("startLineNumber").min(last), number("endLineNumber").min(last));
        let (start_column, end_column) = (number("startColumn"), number("endColumn"));
        let empty = start == end && start_column == end_column;
        let end_line = if end > start && end_column == 0 { end - 1 } else { end };
        Some(CodeLines { start, end: end_line, start_column, end_column, empty })
    }

    fn line_top(&self, line: usize) -> f32 {
        self.editor.line_top(self.editor.view_line(line), self.viewport)
    }

    /// `getBottomForLineNumber`: the line's top and all its wrapped rows.
    fn line_bottom(&self, line: usize) -> f32 {
        let first = self.editor.view_line(line);
        let next = if line + 1 < self.editor.document().lines.len() { self.editor.view_line(line + 1) } else { self.editor.view_line_count() };
        self.line_top(line) + next.saturating_sub(first).max(1) as f32 * self.viewport.line_height
    }

    fn is_hidden(&self, line: usize) -> bool {
        self.editor.model_line(self.editor.view_line(line)) != line
    }

    /// `getVerticalBounds`; file comments sit at the document's top.
    fn vertical_bounds(&self) -> Span {
        match self.lines() {
            Some(lines) => Span { top: self.line_top(lines.start), bottom: self.line_bottom(lines.end) },
            None => Span { top: 0., bottom: 0. },
        }
    }

    /// `getOffsetForColumn`: past the injected text before the column (inlay hints, color boxes); `usize::MAX` is the
    /// line's end with the injections after its last char.
    fn offset(&self, line: usize, column: usize) -> f32 {
        let layout = self.editor.line_layout(line);
        let x = if column == usize::MAX { layout.width() } else { layout.x(column) };
        x * self.viewport.char_width
    }

    /// `getLineRects`: the commented lines in view, in editor coordinates, top to bottom without gaps; code running out
    /// of view reaches past the editor's edge. With none in view, the code's estimated area as one line.
    fn line_rects(&self) -> Option<Vec<Rect>> {
        let code = self.lines()?;
        let scroll = self.editor.scroll;
        let min_width = self.viewport.char_width;
        let view_bottom = scroll.top + self.viewport.height;
        let mut lines = Vec::new();
        let (mut above, mut below) = (false, false);
        for line in code.start..=code.end {
            let (top, bottom) = (self.line_top(line), self.line_bottom(line));
            if self.is_hidden(line) {
                continue;
            }
            if bottom <= scroll.top {
                above = true;
                continue;
            }
            if top >= view_bottom {
                below = true;
                continue;
            }
            // An empty range comments on its whole line.
            let start_column = if !code.empty && line == code.start { code.start_column } else { 0 };
            let end_column = if !code.empty && line == code.end { code.end_column } else { usize::MAX };
            let start = self.offset(line, start_column);
            let mut end = self.offset(line, end_column);
            if bottom - top > self.viewport.line_height {
                // The line wraps, so its text reaches the viewport's right.
                end = end.max(self.viewport.width - VERTICAL_SCROLLBAR_WIDTH);
            }
            lines.push(Rect { top: top - scroll.top, left: start - scroll.left, width: (end - start).max(min_width), height: bottom - top });
        }
        if lines.is_empty() {
            let vertical = self.vertical_bounds();
            let width = self.offset(code.start, usize::MAX).max(min_width);
            return Some(vec![Rect { top: vertical.top - scroll.top, left: -scroll.left, width, height: vertical.bottom - vertical.top }]);
        }
        // Close the gaps that view zones leave between lines.
        for i in 0..lines.len() - 1 {
            lines[i].height = lines[i + 1].top - lines[i].top;
        }
        let first = lines[0];
        if first.top > -EDGE_MARGIN && above {
            lines[0] = Rect { top: -EDGE_MARGIN, height: first.top + first.height + EDGE_MARGIN, ..first };
        }
        let last_index = lines.len() - 1;
        let last = lines[last_index];
        let viewport_bottom = self.viewport.height + EDGE_MARGIN;
        if last.top + last.height < viewport_bottom && below {
            lines[last_index].height = viewport_bottom - last.top;
        }
        Some(lines)
    }

    /// `getBoundingRect` in editor coordinates.
    fn bounding_rect(&self) -> Option<Rect> {
        let lines = self.line_rects()?;
        let vertical = self.vertical_bounds();
        let left = lines.iter().map(|line| line.left).fold(f32::INFINITY, f32::min);
        let right = lines.iter().map(|line| line.left + line.width).fold(f32::NEG_INFINITY, f32::max);
        Some(Rect { top: vertical.top - self.editor.scroll.top, left, width: right - left, height: vertical.bottom - vertical.top })
    }

    /// `estimateLineRects`: where the view lines between two document offsets end, from their columns; a wrapped row
    /// reaches the code area's right.
    fn estimate_line_rects(&self, top: f32, bottom: f32) -> Vec<Rect> {
        let left = -self.editor.scroll.left;
        let count = self.editor.view_line_count();
        let mut lines = Vec::new();
        let mut view_line = self.editor.view_line_at(top.max(0.), self.viewport);
        while view_line < count {
            let line_top = self.editor.line_top(view_line, self.viewport);
            if line_top >= bottom {
                break;
            }
            let model_line = self.editor.model_line(view_line);
            let wrapped = self.line_bottom(model_line) - self.line_top(model_line) > self.viewport.line_height;
            let width = if wrapped { self.viewport.width } else { self.estimated_columns(model_line) * self.viewport.char_width };
            lines.push(Rect { top: line_top, left, width, height: self.viewport.line_height });
            view_line += 1;
        }
        lines
    }

    /// `visibleColumnFromColumn` of the view line's content, which holds injected text as chars: a color box's
    /// injection is one no-break space (colorDetector.ts); the others are taken as wide as they draw.
    fn estimated_columns(&self, line: usize) -> f32 {
        let document = self.editor.document();
        let text = &document.lines[line];
        let injected: f32 = self.editor.injected_text_on(line).iter().map(|injection| if injection.owner == InjectionOwner::Color { 1. } else { injection.columns }).sum();
        visible_column(text, text.chars().count(), document.indentation.tab_size) as f32 + injected
    }
}

/// The editor's measures a placement pass needs, in content coordinates.
#[derive(Clone, Copy)]
struct EditorFrame {
    /// `_getHorizontalBounds().right`; the left is 0.
    right: f32,
    viewport: Viewport,
    scroll_top: f32,
    scrollable_height: f32,
}

impl EditorFrame {
    fn editor_size(&self) -> Size {
        Size { width: self.right.max(0.), height: self.viewport.height }
    }
}

/// A floating widget with what a placement pass knows of its thread.
struct Item<'a> {
    key: WidgetKey,
    floating: &'a mut Floating,
    geometry: CodeGeometry<'a>,
    content: Content,
}

impl Item<'_> {
    fn code_span(&self) -> Span {
        self.geometry.vertical_bounds()
    }

    fn document_top(&self, layout: &Layout, height: f32, frame: EditorFrame) -> f32 {
        self.floating.size.top(layout.offset_top, self.code_span(), height, frame.scrollable_height)
    }

    /// `getDocumentRect`.
    fn rect(&self, frame: EditorFrame) -> Option<Obstacle> {
        let layout = self.floating.layout.as_ref()?;
        let (width, height) = (self.floating.width, self.floating.height);
        let rect = Rect { top: self.document_top(layout, height, frame), left: self.floating.left, width, height };
        Some(Obstacle { rect, code_top: self.code_span().top })
    }

    fn is_chosen_by_user(&self) -> bool {
        self.floating.layout.is_some_and(|layout| layout.is_chosen_by_user)
    }

    /// `_setWidth`: the content is laid out for the width at render.
    fn set_width(&mut self, width: f32, range: SizeRange) -> f32 {
        self.floating.width = clamp(width, range.min, range.max);
        self.floating.width
    }

    fn set_height(&mut self, height: f32, content: ContentHeights) -> f32 {
        let range = self.floating.size.height_range(content);
        self.floating.height = clamp(height, range.min, range.max);
        self.floating.comments_height = self.floating.size.comments_height(content, self.floating.height);
        self.floating.height
    }

    fn lay_out_at_width(&mut self, width: f32, range: SizeRange) -> Size {
        let width = self.set_width(width, range);
        Size { width, height: self.floating.size.height(self.content.heights(width), None) }
    }

    /// `_applySize`.
    fn apply_size(&mut self, frame: EditorFrame) {
        let Some(layout) = self.floating.layout else { return };
        let range = self.floating.size.width_range(self.content.widths);
        let width = self.set_width(layout.width, range);
        let heights = self.content.heights(width);
        let growth_changed = self.floating.size.update_growth(heights);
        let height = self.set_height(self.floating.size.height(heights, layout.height), heights);
        if growth_changed {
            let code = self.code_span();
            self.floating.size.update_upward_shift(layout.offset_top, code, height, frame.scroll_top, frame.viewport.height);
        }
    }

    /// `_render` without drawing: where the widget is now.
    fn update_position(&mut self, frame: EditorFrame) {
        let Some(layout) = self.floating.layout else { return };
        let width = self.floating.width;
        self.floating.left = clamp(layout.left, 0., (frame.right - width).max(0.));
        self.floating.top = self.document_top(&layout, self.floating.height, frame);
    }

    /// `_computeDefaultLayout`.
    fn default_layout(&mut self, obstacles: Vec<Obstacle>, frame: EditorFrame) -> Layout {
        self.floating.size.reset_growth();
        self.floating.size.capture_editor_size(frame.editor_size());
        let widths = self.content.widths;
        let width_range = self.floating.size.width_range(widths);
        let preferred_width = default_floating_widget_width(frame.viewport.width, frame.right - 2. * EDGE_MARGIN);
        let default_width = self.floating.size.default_width(widths, preferred_width, self.content.is_editing);

        let code = self.geometry.bounding_rect().map(|code| Rect { top: code.top + frame.scroll_top, ..code });
        let area = PlacementRequest { min_left: EDGE_MARGIN, max_right: frame.right, width: 0., height: 0., code, obstacles, scrollable_height: frame.scrollable_height, beside_code: false, lines_right: None };
        let mut default_size = None;
        let mut placement = match code {
            Some(code) if !self.content.is_new => self.place_beside_lines(&area, code, default_width, &mut default_size, width_range),
            _ => None,
        };
        if placement.is_none() {
            let size = *default_size.get_or_insert_with(|| self.lay_out_at_width(default_width, width_range));
            let request = PlacementRequest { width: size.width, height: size.height, ..area.clone() };
            placement = match code {
                Some(code) if self.content.is_new => self.place_beside_code(&request, code, width_range),
                _ => None,
            };
            placement = placement.or_else(|| {
                let placed = floating_widget_placement(&request);
                Some((placed.top, placed.left, placed.centered, request.width))
            });
        }
        let (top, left, centered, width) = placement.expect("placed above");
        let offset_top = (!centered).then(|| top - self.code_span().top);
        Layout { offset_top, left, width, height: None, is_chosen_by_user: false }
    }

    /// `_placeBesideCode`: a new comment right after its code, level with it, when there is room.
    fn place_beside_code(&mut self, request: &PlacementRequest, code: Rect, width_range: SizeRange) -> Option<(f32, f32, bool, f32)> {
        let obstacles: Vec<Rect> = request.obstacles.iter().map(|obstacle| obstacle.rect).collect();
        let width = width_beside_code(code, request.height, request.width, width_range.min, request.max_right, &obstacles)?;
        let size = self.lay_out_at_width(width, width_range);
        let placed = floating_widget_placement(&PlacementRequest { width: size.width, height: size.height, beside_code: true, ..request.clone() });
        Some((placed.top, placed.left, placed.centered, size.width))
    }

    /// `_placeBesideLines`: an existing comment right after the lines next to its code, as wide as its content
    /// fills, when there is room; laid out once unless its height decides its width.
    fn place_beside_lines(&mut self, area: &PlacementRequest, code: Rect, default_width: f32, default_size: &mut Option<Size>, width_range: SizeRange) -> Option<(f32, f32, bool, f32)> {
        let max_height = self.floating.size.max_auto_height();
        let natural_width = self.content.widths.natural;
        let centered = |height: f32| centered_top(code.top, code.top + code.height, height);
        let nearby_lines = self.geometry.estimate_line_rects(centered(max_height), centered(max_height) + max_height);
        let placement = |lines: &[Rect], top: f32, height: f32| placement_beside_lines(lines, top, top + height, code.left + code.width, area.max_right, default_width, natural_width);
        let centered_placement = |height: f32| placement(&nearby_lines, centered(height), height);

        let shortest = centered_placement(MIN_HEIGHT)?;
        let tallest = centered_placement(max_height);
        let fits_tallest = |tallest: BesideLines| {
            let request = PlacementRequest { width: tallest.width, height: max_height, lines_right: Some(tallest.lines_right), ..area.clone() };
            tallest.lines_right == shortest.lines_right && tallest.width == shortest.width && floating_widget_placement(&request).centered
        };
        let (beside, size) = match tallest.filter(|&tallest| fits_tallest(tallest)) {
            Some(tallest) => (Some(tallest), Some(self.lay_out_at_width(tallest.width, width_range))),
            None => {
                let measured = *default_size.get_or_insert_with(|| self.lay_out_at_width(default_width, width_range));
                let beside = (measured.height <= max_height).then(|| centered_placement(measured.height)).flatten();
                let size = beside.map(|beside| if beside.width == measured.width { measured } else { self.lay_out_at_width(beside.width, width_range) });
                (beside, size)
            }
        };
        // A widget whose title, reply and actions alone are taller than it sizes itself to is next to unchecked lines.
        let (beside, size) = (beside?, size?);
        if size.height > max_height {
            return None;
        }
        let placed = floating_widget_placement(&PlacementRequest { width: size.width, height: size.height, lines_right: Some(beside.lines_right), ..area.clone() });
        if !placed.centered {
            // Other widgets moved it away from its code, next to other lines.
            let lines = self.geometry.estimate_line_rects(placed.top, placed.top + size.height);
            let moved = placement(&lines, placed.top, size.height)?;
            if moved.lines_right > beside.lines_right || moved.width < size.width {
                return None;
            }
        }
        Some((placed.top, placed.left, placed.centered, size.width))
    }
}

/// `_placeAutomatically` for `items[index]`, around the widgets the user put somewhere or for code not after its own.
fn place_automatically(items: &mut [Item], index: usize, frame: EditorFrame, layouts: &mut Vec<(String, Layout)>, resource: &str) {
    let code_top = items[index].code_span().top;
    let mut obstacles = Vec::new();
    for (other, item) in items.iter().enumerate() {
        if other == index {
            continue;
        }
        if let Some(rect) = item.rect(frame).filter(|rect| item.is_chosen_by_user() || rect.code_top <= code_top) {
            obstacles.push(rect);
        }
    }
    let item = &mut items[index];
    let layout = item.default_layout(obstacles, frame);
    item.floating.layout = Some(layout);
    store_layout(layouts, layout_key(resource, item.key), layout);
    item.apply_size(frame);
    item.update_position(frame);
}

/// `_placeAmongOtherWidgets`: places the widget, then again the automatically placed widgets it is now in the way of,
/// in the order of their code, so the widgets stay in that order.
fn place_among_other_widgets(items: &mut [Item], index: usize, chosen: Option<Layout>, frame: EditorFrame, layouts: &mut Vec<(String, Layout)>, resource: &str) {
    match chosen {
        Some(layout) => {
            items[index].floating.layout = Some(layout);
            items[index].apply_size(frame);
            items[index].update_position(frame);
        }
        None => place_automatically(items, index, frame, layouts, resource),
    }
    let Some(placed) = items[index].rect(frame) else { return };
    let mut to_move: Vec<(usize, Obstacle)> = Vec::new();
    for (other, item) in items.iter().enumerate() {
        if other == index || item.is_chosen_by_user() {
            continue;
        }
        if let Some(rect) = item.rect(frame).filter(|rect| chosen.is_some() || rect.code_top > placed.code_top) {
            to_move.push((other, rect));
        }
    }
    to_move.sort_by(|a, b| a.1.code_top.total_cmp(&b.1.code_top));
    let mut moved = vec![placed];
    for (other, rect) in to_move {
        if moved.iter().any(|&moved_rect| is_in_the_way_of(rect, moved_rect)) {
            place_automatically(items, other, frame, layouts, resource);
            moved.extend(items[other].rect(frame));
        }
    }
}

fn layout_key(resource: &str, (controller, thread, _): WidgetKey) -> String {
    format!("{resource}\n{controller}\n{thread}")
}

/// `LRUCache.set`: the newest entry last, the oldest dropped past the limit.
fn store_layout(layouts: &mut Vec<(String, Layout)>, key: String, layout: Layout) {
    layouts.retain(|(existing, _)| *existing != key);
    layouts.push((key, layout));
    if layouts.len() > LAYOUT_CACHE_SIZE {
        layouts.remove(0);
    }
}

impl Workbench {
    pub(super) fn floating_comment_threads(&self) -> bool {
        self.setting("comments.thread.experimental.style").as_str() == Some("floating")
    }

    /// `_render` and the pending `_show` placements of the active editor's floating widgets, in the order of their
    /// code (`_sortThreadsForDisplay`). Runs before every render: the content is measured with the window's text system.
    pub(super) fn lay_out_floating_comment_widgets(&mut self, window: &Window) {
        let Some(editor) = self.editors.active() else { return };
        let model_id = editor.model_id;
        let mut contents = Vec::new();
        for widget in &self.comments.widgets {
            if widget.model_id == model_id && matches!(widget.place, Place::Floating(_)) {
                contents.push(self.floating_content(widget.controller, widget.thread, window));
            }
        }
        if contents.is_empty() {
            return;
        }
        let viewport = self.editors.viewport;
        let frame = EditorFrame { right: viewport.width - VERTICAL_SCROLLBAR_WIDTH, viewport, scroll_top: editor.scroll.top, scrollable_height: editor.scroll_height(viewport).max(viewport.height) };
        let resource = editor.document().uri()["path"].as_str().unwrap_or_default().to_owned();
        // `_onContentSizeChanged` leaves a widget being resized alone.
        let resizing = self.comments.floating.gesture.as_ref().filter(|gesture| matches!(gesture.kind, GestureKind::Resize(_))).map(|gesture| gesture.key);
        let controllers = &self.comments.controllers;
        let layouts = &mut self.comments.floating.layouts;
        let mut items = Vec::new();
        for (widget, content) in self.comments.widgets.iter_mut().filter(|widget| widget.model_id == model_id).filter_map(|widget| match &mut widget.place {
            Place::Floating(floating) => Some((widget.controller, widget.thread, floating)),
            Place::Zone(_) => None,
        }).zip(contents) {
            let (controller, thread, floating) = widget;
            let Some(content) = content else { continue };
            let range = controllers.iter().find(|candidate| candidate.handle == controller).and_then(|owner| owner.threads.iter().find(|candidate| candidate.handle == thread)).and_then(|thread| thread.range.as_ref());
            items.push(Item { key: (controller, thread, model_id), floating, geometry: CodeGeometry { editor, viewport, range }, content });
        }
        items.sort_by(|a, b| a.code_span().top.total_cmp(&b.code_span().top));
        for item in &mut items {
            // Resizing the editor leaves a shown widget's size alone, unless the editor had no size when it showed.
            let editor_size = item.floating.size.editor_size();
            if editor_size.width <= 0. || editor_size.height <= 0. {
                item.floating.size.capture_editor_size(frame.editor_size());
            }
            if item.floating.layout.is_some() && resizing != Some(item.key) {
                item.apply_size(frame);
            }
            item.update_position(frame);
        }
        let mut reveal = None;
        for index in 0..items.len() {
            let Some(use_cache) = items[index].floating.pending_placement.take() else { continue };
            // `_show`: a widget the user moved or resized goes back there; the others are placed among the others.
            let cached = layouts.iter().find(|(key, _)| *key == layout_key(&resource, items[index].key)).map(|(_, layout)| *layout);
            let chosen = if use_cache { items[index].floating.layout.or(cached).filter(|layout| layout.is_chosen_by_user) } else { None };
            items[index].floating.size.reset_growth();
            items[index].floating.size.capture_editor_size(frame.editor_size());
            place_among_other_widgets(&mut items, index, chosen, frame, layouts, &resource);
            if std::mem::take(&mut items[index].floating.pending_reveal) {
                let item = &items[index];
                let widget = Span { top: item.floating.top, bottom: item.floating.top + item.floating.height };
                reveal = reveal_scroll_top(item.code_span(), widget, frame.scroll_top, viewport.height);
            }
        }
        drop(items);
        if let Some(top) = reveal {
            // `_revealInEditor`.
            let editor = self.editors.active_mut().expect("laid out for the active editor");
            let left = editor.scroll.left;
            editor.set_scroll(top, left, viewport);
        }
    }

    /// `measureWidths` and `measureHeights` of the thread's content, from the text widths of its parts.
    fn floating_content(&self, controller: u64, thread_handle: u64, window: &Window) -> Option<Content> {
        let thread = self.comment_thread(controller, thread_handle)?;
        let reaction_handler = self.comments.controllers.iter().find(|candidate| candidate.handle == controller).is_some_and(|owner| owner.reaction_handler);
        let reply = self.comments.reply.as_ref().filter(|reply| reply.controller == controller && reply.thread == thread_handle);
        let replying = reply.is_some_and(|reply| reply.comment.is_none());
        let edited = reply.and_then(|reply| reply.comment);
        let can_reply = thread.can_reply != false;
        let context = self.comment_thread_context(controller, thread, window);
        let ui = |text: &str, weight: gpui::FontWeight| ui_text_width(text, UI_FONT, weight, window);

        let title_actions = menu_actions(&self.menus, MenuId::CommentThreadTitle, &context).into_iter().filter(|action| matches!(action, MenuAction::Command { enabled: true, .. })).count();
        let head_minimum = HEAD_CHROME + (title_actions + 1) as f32 * HEAD_ACTION;
        let label = thread.label.clone().unwrap_or_else(|| if thread.comments.is_empty() { "Start discussion".into() } else { String::new() });
        let head_natural = head_minimum + ui(&label, gpui::FontWeight::NORMAL);
        let reply_minimum = match (can_reply, replying) {
            (false, _) => 0.,
            (true, false) => {
                let prompt = if thread.comments.is_empty() { "Type a new comment" } else { "Reply..." };
                FORM_MARGINS + ui_text_width(prompt, "Menlo", gpui::FontWeight::MEDIUM, window) + REPLY_BUTTON_CHROME
            }
            (true, true) => {
                let buttons = menu_actions(&self.menus, MenuId::CommentThreadActions, &context).into_iter().filter_map(|action| match action {
                    MenuAction::Command { title, .. } => Some(ui(&title, gpui::FontWeight::NORMAL) + 16. + 5.),
                    _ => None,
                });
                FORM_MARGINS + buttons.fold(0., f32::max)
            }
        };

        let mut body_natural: f32 = 0.;
        let mut comments = Vec::new();
        for comment in &thread.comments {
            let author = ui(comment["userName"].as_str().unwrap_or_default(), gpui::FontWeight::BOLD);
            let label = comment["label"].as_str().map_or(0., |label| ui(label, gpui::FontWeight::NORMAL) + 14.);
            let text = comment_text(comment);
            let lines: Vec<f32> = text.lines().filter(|line| !line.trim().is_empty()).map(|line| ui(line, gpui::FontWeight::NORMAL)).collect();
            let lines = if lines.is_empty() { vec![0.] } else { lines };
            body_natural = body_natural.max(COMMENT_CHROME_WIDTH + lines.iter().copied().fold(author + label, f32::max));
            let editing = edited.is_some() && comment["uniqueIdInThread"].as_u64() == edited;
            let reactions = reaction_handler || comment["commentReactions"].as_array().into_iter().flatten().any(|reaction| reaction["count"].as_u64().unwrap_or_default() > 0);
            let chrome = COMMENT_CHROME + if reactions { REACTIONS_ROW } else { 0. };
            match editing {
                true => comments.push((chrome + EDIT_FORM, Vec::new())),
                false => comments.push((chrome, lines)),
            }
        }
        let frame = 2. * STROKE;
        let head = (self.editors.viewport.line_height * 1.2).ceil();
        let form = match (can_reply, replying) {
            (false, _) => 0.,
            (true, false) => REPLY_BUTTON_FORM,
            (true, true) => REPLY_EDITOR_FORM,
        };
        let widths = ContentWidths { minimum: frame + head_minimum.max(reply_minimum), natural: frame + head_natural.max(reply_minimum).max(body_natural) };
        let is_new = thread.comments.is_empty();
        let is_editing = replying || edited.is_some() || (can_reply && is_new);
        Some(Content { widths, fixed: frame + head + STROKE + form, comments, is_editing, is_new })
    }

    /// `reveal` of a floating widget: scrolled into view with its code at the next layout.
    pub(super) fn reveal_floating_comment_widget(&mut self, controller: u64, thread: u64) {
        for widget in self.comments.widgets.iter_mut().filter(|widget| widget.controller == controller && widget.thread == thread) {
            if let Place::Floating(floating) = &mut widget.place {
                floating.pending_reveal = true;
            }
        }
    }

    fn floating_widget_mut(&mut self, key: WidgetKey) -> Option<&mut Floating> {
        let widget = self.comments.widgets.iter_mut().find(|widget| (widget.controller, widget.thread, widget.model_id) == key)?;
        match &mut widget.place {
            Place::Floating(floating) => Some(floating),
            Place::Zone(_) => None,
        }
    }

    fn floating_code_span(&self, key: WidgetKey) -> Span {
        let editor = self.editors.active().expect("floating widgets show in the active editor");
        let range = self.comment_thread(key.0, key.1).and_then(|thread| thread.range.as_ref());
        CodeGeometry { editor, viewport: self.editors.viewport, range }.vertical_bounds()
    }

    /// `_onHeaderPointerDown`: dragging the title moves the widget; a double click puts it back (`_onHeaderDoubleClick`).
    fn start_floating_drag(&mut self, key: WidgetKey, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let line_height = self.editors.viewport.line_height;
        let Some(floating) = self.floating_widget_mut(key) else { return };
        if event.click_count == 2 {
            floating.pending_placement = Some(false);
            cx.notify();
            return;
        }
        let Some(layout) = floating.layout else { return };
        let (top, left) = (floating.top, floating.left);
        let head_height = (line_height * 1.2).ceil();
        let start = (f32::from(event.position.x), f32::from(event.position.y));
        self.comments.floating.gesture = Some(Gesture { key, start, kind: GestureKind::Drag { layout, top, left, head_height } });
        cx.notify();
    }

    /// `_onWillResize`.
    fn start_floating_resize(&mut self, key: WidgetKey, edges: [bool; 4], event: &MouseDownEvent, window: &Window, cx: &mut Context<Self>) {
        let Some(content) = self.floating_content(key.0, key.1, window) else { return };
        let viewport = self.editors.viewport;
        let editor_size = Size { width: (viewport.width - VERTICAL_SCROLLBAR_WIDTH).max(0.), height: viewport.height };
        let Some(floating) = self.floating_widget_mut(key) else { return };
        let Some(layout) = floating.layout else { return };
        floating.size.grow_editor_size(editor_size);
        let width_range = floating.size.width_range(content.widths);
        let session = ResizeSession { layout, top: floating.top, left: floating.left, width: floating.width, height: floating.height, width_range, edges };
        let start = (f32::from(event.position.x), f32::from(event.position.y));
        self.comments.floating.gesture = Some(Gesture { key, start, kind: GestureKind::Resize(session) });
        cx.notify();
    }

    fn move_floating_gesture(&mut self, position: gpui::Point<gpui::Pixels>, done: bool, window: &Window, cx: &mut Context<Self>) {
        let Some(gesture) = self.comments.floating.gesture.take() else { return };
        let (dx, dy) = (f32::from(position.x) - gesture.start.0, f32::from(position.y) - gesture.start.1);
        let key = gesture.key;
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active() else { return };
        let scroll_top = editor.scroll.top;
        let scrollable_height = editor.scroll_height(viewport).max(viewport.height);
        let right = viewport.width - VERTICAL_SCROLLBAR_WIDTH;
        let code = self.floating_code_span(key);
        let content = self.floating_content(key.0, key.1, window);
        let resource = editor.document().uri()["path"].as_str().unwrap_or_default().to_owned();
        let Some(floating) = self.floating_widget_mut(key) else { return };
        match &gesture.kind {
            GestureKind::Drag { layout, top, left, head_height } => {
                // The title stays in the editor, so the widget can always be dragged back.
                let max_top = (viewport.height - head_height).max(0.);
                let new_top = clamp(top - scroll_top + dy, 0., max_top) + scroll_top;
                let new_left = clamp(left + dx, 0., (right - floating.width).max(0.));
                floating.size.clear_upward_shift();
                floating.layout = Some(Layout { left: new_left, offset_top: Some(new_top - code.top), is_chosen_by_user: true, ..*layout });
            }
            GestureKind::Resize(session) => {
                let Some(content) = content else { return };
                let [north, east, south, west] = session.edges;
                let vertical = north || south;
                // Kept within the editor horizontally.
                let max_width_on_side = if west { session.left + session.width } else { right - session.left };
                let range = SizeRange { min: session.width_range.min, max: session.width_range.min.max(session.width_range.max.min(max_width_on_side)) };
                let requested_width = session.width + if east { dx } else if west { -dx } else { 0. };
                let width = clamp(requested_width, range.min, range.max);
                floating.width = width;
                let heights = content.heights(width);
                floating.size.update_growth(heights);
                let mut requested_height = if vertical { session.height + if south { dy } else { -dy } } else { floating.size.height(heights, session.layout.height) };
                if north {
                    // The top edge doesn't go above the document's start.
                    requested_height = requested_height.min(session.top + session.height);
                } else if vertical {
                    requested_height = requested_height.min(scrollable_height - EDGE_MARGIN - session.top);
                }
                let height_range = floating.size.height_range(heights);
                floating.height = clamp(requested_height, height_range.min, height_range.max);
                floating.comments_height = floating.size.comments_height(heights, floating.height);
                // Resizing vertically pins the other edge: the widget stops being centered on its code.
                let mut offset_top = session.layout.offset_top;
                if vertical {
                    let top = if north { (session.top + session.height - floating.height).max(0.) } else { session.top };
                    offset_top = Some(top - code.top);
                    floating.size.clear_upward_shift();
                }
                let left = if west { session.left + session.width - width } else { session.left };
                let height = if vertical { Some(floating.height) } else { session.layout.height };
                floating.layout = Some(Layout { offset_top, left, width, height, is_chosen_by_user: true });
                if done && vertical {
                    // The chosen height includes the reply as it is now.
                    floating.size.reset_growth();
                }
            }
        }
        let layout = floating.layout;
        if done {
            if let Some(layout) = layout {
                store_layout(&mut self.comments.floating.layouts, layout_key(&resource, key), layout);
            }
        } else {
            self.comments.floating.gesture = Some(gesture);
        }
        cx.notify();
    }

    /// The floating widgets of `editor` and their connectors, the one in front last.
    pub(super) fn floating_comment_widgets(&self, editor: &OpenEditor, content_left: f32, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut widgets: Vec<&ThreadWidget> = self.comments.widgets.iter().filter(|widget| widget.model_id == editor.model_id && matches!(&widget.place, Place::Floating(floating) if floating.layout.is_some())).collect();
        let front = self.comments.floating.front;
        widgets.sort_by_key(|widget| front == Some((widget.controller, widget.thread, widget.model_id)));
        let mut elements: Vec<AnyElement> = widgets.iter().filter_map(|widget| self.floating_connector(widget, editor, content_left, window)).collect();
        elements.extend(widgets.iter().filter_map(|widget| self.floating_comment_widget(widget, editor, content_left, window, cx)));
        if self.comments.floating.gesture.is_some() {
            elements.push(self.floating_gesture_listener(cx).into_any_element());
        }
        elements
    }

    /// `CommentThreadConnector.render`: the outline around the code and the line from it to the widget's top left
    /// corner, halfway around its curve; subtle until focus is in the widget, full in high contrast.
    fn floating_connector(&self, widget: &ThreadWidget, editor: &OpenEditor, content_left: f32, window: &Window) -> Option<AnyElement> {
        let Place::Floating(floating) = &widget.place else { return None };
        let thread = self.comment_thread(widget.controller, widget.thread)?;
        let viewport = self.editors.viewport;
        let geometry = CodeGeometry { editor, viewport, range: thread.range.as_ref() };
        let code_lines = geometry.line_rects()?;
        let origin_x = -CONNECTOR_GUTTER_BLEED;
        let width = viewport.width - VERTICAL_SCROLLBAR_WIDTH - origin_x;
        let rows: Vec<Rect> = code_lines.iter().map(|line| Rect { top: line.top, left: line.left - OUTLINE_PADDING - origin_x, width: line.width + 2. * OUTLINE_PADDING, height: line.height }).collect();
        let outline = rows_outline(&rows, OUTLINE_CORNER_RADIUS);
        let corner_inset = CORNER_RADIUS * (1. - std::f32::consts::FRAC_1_SQRT_2);
        let anchor = Point { x: floating.left + corner_inset - origin_x, y: floating.top - editor.scroll.top + corner_inset };
        let line = connector_points(&rows, anchor);
        let active = self.theme.kind.is_high_contrast() || self.comments.reply.as_ref().is_some_and(|reply| reply.controller == widget.controller && reply.thread == widget.thread) && self.comments.reply_focus.is_focused(window);
        let color = self.comment_thread_state_color(thread);
        let (stroke, fill) = if active { (color, color) } else { (Rgba { a: color.a * 0.5, ..color }, Rgba { a: color.a * 0.5, ..color }) };
        let element = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let at = |p: Point| point(bounds.origin.x + px(p.x), bounds.origin.y + px(p.y));
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    let mut path = PathBuilder::stroke(px(STROKE));
                    for (i, corner) in outline.iter().enumerate() {
                        let (start, end) = match *corner {
                            OutlineCorner::Sharp(p) => (p, None),
                            OutlineCorner::Rounded { start, corner, end } => (start, Some((corner, end))),
                        };
                        if i == 0 { path.move_to(at(start)) } else { path.line_to(at(start)) }
                        if let Some((corner, end)) = end {
                            path.curve_to(at(end), at(corner));
                        }
                    }
                    path.close();
                    if let Ok(path) = path.build() {
                        window.paint_path(path, stroke);
                    }
                    let Some((from, to)) = line else { return };
                    let mut connector = PathBuilder::stroke(px(STROKE));
                    connector.move_to(at(from));
                    connector.line_to(at(to));
                    if let Ok(connector) = connector.build() {
                        window.paint_path(connector, stroke);
                    }
                    let anchor = Bounds { origin: at(Point { x: from.x - 2., y: from.y - 2. }), size: size(px(4.), px(4.)) };
                    window.paint_quad(gpui::fill(anchor, fill).corner_radii(px(2.)));
                });
            },
        )
        .absolute()
        .left(px(content_left + origin_x))
        .top_0()
        .w(px(width))
        .h(px(viewport.height));
        Some(element.into_any_element())
    }

    fn floating_comment_widget(&self, widget: &ThreadWidget, editor: &OpenEditor, content_left: f32, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Place::Floating(floating) = &widget.place else { return None };
        let thread = self.comment_thread(widget.controller, widget.thread)?;
        let key = (widget.controller, widget.thread, widget.model_id);
        let state_color = self.comment_thread_state_color(thread);
        let head_height = (self.editors.viewport.line_height * 1.2).ceil();
        let dragging = self.comments.floating.gesture.as_ref().is_some_and(|gesture| gesture.key == key && matches!(gesture.kind, GestureKind::Drag { .. }));
        let head = div()
            .id("comment-thread-floating-head")
            .debug_selector(|| "comment-thread-floating-head".into())
            .flex_none()
            .rounded_t(px(CORNER_RADIUS - STROKE))
            .overflow_hidden()
            .cursor(if dragging { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, _, cx| workbench.start_floating_drag(key, event, cx)))
            .child(self.comment_thread_head(widget, thread, head_height, state_color, true, window, cx));
        let comments = div()
            .id("comment-thread-floating-comments")
            .h(px(floating.comments_height))
            .flex_none()
            .overflow_y_scroll()
            .track_scroll(&floating.comments_scroll)
            .children((0..thread.comments.len()).map(|index| self.comment_node(widget, thread, index, window, cx)));
        let body = div()
            .id("comment-thread-floating-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .border_t(px(STROKE))
            .border_color(state_color)
            .child(comments)
            .when(thread.can_reply != false, |body| body.child(self.comment_reply_form(widget, thread, window, cx)));
        let (comments_scroll, comments_offset) = (floating.comments_scroll.clone(), floating.comments_offset.clone());
        let element = div()
            .id(gpui::SharedString::from(format!("comment-thread-floating-{}-{}", widget.controller, widget.thread)))
            .absolute()
            .left(px(content_left + floating.left))
            .top(px(floating.top - editor.scroll.top))
            .w(px(floating.width))
            .h(px(floating.height))
            .flex()
            .flex_col()
            .border(px(STROKE))
            .border_color(state_color)
            .rounded(px(CORNER_RADIUS))
            .bg(self.color("peekViewResult.background"))
            // style.css `--vscode-shadow-lg`.
            .shadow(vec![BoxShadow { color: rgba(0x00000024).into(), offset: point(px(0.), px(0.)), blur_radius: px(12.), spread_radius: px(0.), inset: false }])
            .cursor(if dragging { CursorStyle::ClosedHand } else { CursorStyle::Arrow })
            .occlude()
            .font_family(UI_FONT)
            .text_size(px(13.))
            .text_color(self.foreground_or("foreground"))
            .capture_any_mouse_down(cx.listener(move |workbench, _, _, cx| {
                workbench.comments.floating.front = Some(key);
                cx.notify();
            }))
            // `_onMouseWheel`: a wheel the comments don't take scrolls the editor; the widget moves with the code.
            .on_scroll_wheel(cx.listener(move |workbench, event: &ScrollWheelEvent, window, cx| {
                let offset = f32::from(comments_scroll.offset().y);
                if comments_offset.replace(offset) != offset {
                    return;
                }
                workbench.editor_wheel(event, window, cx);
            }))
            .child(head)
            .child(body)
            .children(self.floating_sashes(key, floating, cx));
        Some(element.into_any_element())
    }

    /// The ResizableHTMLElement's sashes on the four edges, the corners resizing both ways.
    fn floating_sashes(&self, key: WidgetKey, floating: &Floating, cx: &mut Context<Self>) -> Vec<AnyElement> {
        // Absolute children sit inside the frame.
        let (width, height) = (floating.width - 2. * STROKE, floating.height - 2. * STROKE);
        let half = SASH_SIZE / 2.;
        let (right, bottom) = (width - half, height - half);
        // [north, east, south, west] and [top, left, width, height]; the corners resize both ways.
        let sashes: [([bool; 4], [f32; 4], CursorStyle); 8] = [
            ([true, false, false, false], [-half, half, width - SASH_SIZE, SASH_SIZE], CursorStyle::ResizeUpDown),
            ([false, true, false, false], [half, right, SASH_SIZE, height - SASH_SIZE], CursorStyle::ResizeLeftRight),
            ([false, false, true, false], [bottom, half, width - SASH_SIZE, SASH_SIZE], CursorStyle::ResizeUpDown),
            ([false, false, false, true], [half, -half, SASH_SIZE, height - SASH_SIZE], CursorStyle::ResizeLeftRight),
            ([true, false, false, true], [-half, -half, SASH_SIZE, SASH_SIZE], CursorStyle::ResizeUpLeftDownRight),
            ([true, true, false, false], [-half, right, SASH_SIZE, SASH_SIZE], CursorStyle::ResizeUpRightDownLeft),
            ([false, true, true, false], [bottom, right, SASH_SIZE, SASH_SIZE], CursorStyle::ResizeUpLeftDownRight),
            ([false, false, true, true], [bottom, -half, SASH_SIZE, SASH_SIZE], CursorStyle::ResizeUpRightDownLeft),
        ];
        let mut elements = Vec::new();
        for (index, (edges, [top, left, w, h], cursor)) in sashes.into_iter().enumerate() {
            let sash = div()
                .id(("comment-thread-floating-sash", index))
                .absolute()
                .top(px(top))
                .left(px(left))
                .w(px(w))
                .h(px(h))
                .cursor(cursor)
                .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    workbench.start_floating_resize(key, edges, event, window, cx);
                }));
            elements.push(sash.into_any_element());
        }
        elements
    }

    /// Follows the pointer anywhere in the window while a floating widget drags or resizes.
    fn floating_gesture_listener(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let workbench = cx.entity();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, _, window, cx| {
                    moved.update(cx, |workbench, cx| workbench.move_floating_gesture(event.position, false, window, cx));
                });
                let released = workbench.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, _, window, cx| {
                    released.update(cx, |workbench, cx| workbench.move_floating_gesture(event.position, true, window, cx));
                });
            },
        )
        .size_0()
    }
}

fn ui_text_width(text: &str, family: &'static str, weight: gpui::FontWeight, window: &Window) -> f32 {
    if text.is_empty() {
        return 0.;
    }
    let font = gpui::Font { weight, ..gpui::font(family) };
    let run = gpui::TextRun { len: text.len(), font, color: gpui::black(), background_color: None, underline: None, strikethrough: None };
    f32::from(window.text_system().shape_line(text.to_string().into(), px(13.), &[run], None).width)
}
