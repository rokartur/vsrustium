//! commentThreadFloatingWidgetLayout.ts: where a floating comment thread widget goes and how large it is, and the
//! outline and connector that tie it to its code. Pure geometry, in the editor's CSS pixels.

#[cfg(test)]
mod tests;

/// Room kept between an automatically placed widget and the edges of the editor or other widgets.
pub(super) const EDGE_MARGIN: f32 = 8.;
/// Room kept between the commented code and a widget that is placed next to it.
const CODE_GAP: f32 = 24.;
pub(super) const MIN_WIDTH: f32 = 240.;
pub(super) const MIN_HEIGHT: f32 = 64.;
/// How much of the comments stays visible when the widget is as short as it can be.
const MIN_COMMENTS_HEIGHT: f32 = 40.;
const DEFAULT_MIN_WIDTH: f32 = 320.;
const DEFAULT_MAX_WIDTH: f32 = 560.;
const DEFAULT_WIDTH_RATIO: f32 = 0.4;
/// How many lines next to a widget placed after them may run under it, so a couple of long lines don't keep it from its code.
const COVERABLE_LINES: usize = 2;
/// The widest a widget placed after the ends of the lines next to it is shown, so its comments stay readable.
const MAX_WIDTH_BESIDE_LINES: f32 = 640.;
/// Portion of the editor height that a widget sizing itself to its content may take up.
const AUTO_HEIGHT_RATIO: f32 = 0.6;

/// numbers.ts `clamp`: `max` wins over `min`, where `f32::clamp` panics.
pub(super) fn clamp(value: f32, min: f32, max: f32) -> f32 {
    value.max(min).min(max)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Rect {
    pub top: f32,
    pub left: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Size {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SizeRange {
    pub min: f32,
    pub max: f32,
}

/// `ICommentThreadFloatingWidgetLayout`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Layout {
    /// Distance from the top of the first commented line to the widget's top; None centers it on the code.
    pub offset_top: Option<f32>,
    /// Distance from the editor's left edge to the widget's left edge.
    pub left: f32,
    pub width: f32,
    /// The height the user chose; None sizes the widget to its content.
    pub height: Option<f32>,
    /// The user moved or resized the widget: others are placed around it instead of keeping code order.
    pub is_chosen_by_user: bool,
}

/// `getConnectorPoints`: from the outline point closest to `anchor` to the anchor, None when the anchor is inside.
pub(super) fn connector_points(code_lines: &[Rect], anchor: Point) -> Option<(Point, Point)> {
    let mut from = None;
    let mut from_distance = f32::INFINITY;
    for line in code_lines {
        let point = Point { x: clamp(anchor.x, line.left, line.left + line.width), y: clamp(anchor.y, line.top, line.top + line.height) };
        let distance = (anchor.x - point.x).hypot(anchor.y - point.y);
        if distance < from_distance {
            from = Some(point);
            from_distance = distance;
        }
    }
    if from_distance == 0. {
        return None;
    }
    from.map(|from| (from, anchor))
}

fn round_coordinate(value: f32) -> f32 {
    (value * 100.).round() / 100.
}

/// One corner of an outline: a sharp point, or a quadratic curve from `start` around `corner` to `end`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum OutlineCorner {
    Sharp(Point),
    Rounded { start: Point, corner: Point, end: Point },
}

/// `getRowsOutlinePath` as its corners: the outline of rows stacked top to bottom with rounded corners, empty for no rows.
pub(super) fn rows_outline(rows: &[Rect], corner_radius: f32) -> Vec<OutlineCorner> {
    // Down the right side of the rows, then back up their left side.
    let mut corners = Vec::new();
    for row in rows {
        corners.push(Point { x: row.left + row.width, y: row.top });
        corners.push(Point { x: row.left + row.width, y: row.top + row.height });
    }
    for row in rows.iter().rev() {
        corners.push(Point { x: row.left, y: row.top + row.height });
        corners.push(Point { x: row.left, y: row.top });
    }
    let mut corners: Vec<Point> = corners.into_iter().map(|point| Point { x: round_coordinate(point.x), y: round_coordinate(point.y) }).collect();

    // Rows of the same width give repeated points and points in the middle of straight edges.
    let mut removed = true;
    while removed && corners.len() > 2 {
        removed = false;
        let mut i = 0;
        while i < corners.len() && corners.len() > 2 {
            let previous = corners[(i + corners.len() - 1) % corners.len()];
            let current = corners[i];
            let next = corners[(i + 1) % corners.len()];
            if current == next || (previous.x == current.x && current.x == next.x) || (previous.y == current.y && current.y == next.y) {
                corners.remove(i);
                removed = true;
            } else {
                i += 1;
            }
        }
    }

    let mut outline = Vec::new();
    for i in 0..corners.len() {
        let previous = corners[(i + corners.len() - 1) % corners.len()];
        let current = corners[i];
        let next = corners[(i + 1) % corners.len()];
        let radius = corner_radius.min((current.x - previous.x).hypot(current.y - previous.y) / 2.).min((next.x - current.x).hypot(next.y - current.y) / 2.);
        if radius <= 0. {
            outline.push(OutlineCorner::Sharp(current));
            continue;
        }
        let towards = |target: Point| {
            let length = (target.x - current.x).hypot(target.y - current.y);
            Point { x: round_coordinate(current.x + (target.x - current.x) / length * radius), y: round_coordinate(current.y + (target.y - current.y) / length * radius) }
        };
        outline.push(OutlineCorner::Rounded { start: towards(previous), corner: current, end: towards(next) });
    }
    outline
}

/// `getCenteredTop`: centered on the code, not above the document's start.
pub(super) fn centered_top(code_top: f32, code_bottom: f32, height: f32) -> f32 {
    ((code_top + code_bottom - height) / 2.).max(0.)
}

/// `getDefaultFloatingWidgetWidth`: the width a widget starts with in an editor whose code area is this wide.
pub(super) fn default_floating_widget_width(editor_content_width: f32, available_width: f32) -> f32 {
    clamp((editor_content_width * DEFAULT_WIDTH_RATIO).round(), DEFAULT_MIN_WIDTH, DEFAULT_MAX_WIDTH).min(available_width)
}

/// `getWidthBesideCode`: the width of a widget right after the end of the code, level with its top (a new comment),
/// or None without room for a reasonably wide widget or the width its content needs.
pub(super) fn width_beside_code(code: Rect, height: f32, preferred_width: f32, min_width: f32, max_right: f32, obstacles: &[Rect]) -> Option<f32> {
    let left = code.left + code.width + CODE_GAP;
    let mut right = max_right - EDGE_MARGIN;
    for obstacle in obstacles {
        if obstacle.left + obstacle.width > left && obstacle.top < code.top + height && code.top < obstacle.top + obstacle.height {
            right = right.min(obstacle.left - EDGE_MARGIN);
        }
    }
    let available_width = (right - left).floor();
    let needed_width = min_width.max(preferred_width.min(DEFAULT_MIN_WIDTH));
    (available_width >= needed_width).then(|| preferred_width.min(available_width))
}

/// `IBesideLinesPlacement`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct BesideLines {
    /// Where the lines the widget goes after end.
    pub lines_right: f32,
    pub width: f32,
}

/// `getPlacementBesideLines`: after the lines between `top` and `bottom` that leave room for the default width, as
/// wide as the content fills up to a readable width; it may hide a couple of lines at its default width, else None.
pub(super) fn placement_beside_lines(lines: &[Rect], top: f32, bottom: f32, code_right: f32, max_right: f32, default_width: f32, natural_width: f32) -> Option<BesideLines> {
    let max_lines_right = max_right - EDGE_MARGIN - CODE_GAP - default_width;
    let mut lines_right = code_right;
    let mut hidden_lines = 0;
    for line in lines {
        if line.top < bottom && top < line.top + line.height {
            let right = line.left + line.width;
            if right > max_lines_right {
                hidden_lines += 1;
            } else {
                lines_right = lines_right.max(right);
            }
        }
    }
    if hidden_lines > COVERABLE_LINES || lines_right > max_lines_right {
        return None;
    }
    let available_width = (max_right - EDGE_MARGIN - (lines_right + CODE_GAP)).floor();
    let width = if hidden_lines > 0 { default_width } else { default_width.max(natural_width.min(MAX_WIDTH_BESIDE_LINES).min(available_width)) };
    Some(BesideLines { lines_right, width })
}

/// A vertical extent in document coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Span {
    pub top: f32,
    pub bottom: f32,
}

/// `getRevealScrollTop`: the scroll position that shows the code and the widget, None when no scroll is needed; when
/// they don't fit together the widget shows whole with as much of the code's start as fits.
pub(super) fn reveal_scroll_top(code: Span, widget: Span, scroll_top: f32, viewport_height: f32) -> Option<f32> {
    let top = code.top.min(widget.top);
    let bottom = code.bottom.max(widget.bottom);
    if bottom - top <= viewport_height {
        let in_view = top >= scroll_top && bottom <= scroll_top + viewport_height;
        return (!in_view).then(|| ((top + bottom - viewport_height) / 2.).max(0.));
    }
    if widget.top >= scroll_top && widget.bottom <= scroll_top + viewport_height {
        return None;
    }
    // A widget taller than the viewport shows from its top.
    Some(clamp(code.top - EDGE_MARGIN, widget.bottom + EDGE_MARGIN - viewport_height, widget.top - EDGE_MARGIN).max(0.))
}

fn rects_intersect(a: Rect, b: Rect) -> bool {
    a.left < b.left + b.width && b.left < a.left + a.width && a.top < b.top + b.height && b.top < a.top + a.height
}

/// `IFloatingWidgetObstacle`: the area another widget takes up and where its code starts, both in document coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Obstacle {
    pub rect: Rect,
    pub code_top: f32,
}

/// `areOutOfOrder`: of two widgets one above the other, the one for later code is above.
fn are_out_of_order(a: Obstacle, b: Obstacle) -> bool {
    let (ra, rb) = (a.rect, b.rect);
    if ra.left >= rb.left + rb.width || rb.left >= ra.left + ra.width {
        return false;
    }
    if a.code_top < b.code_top { rb.top + rb.height <= ra.top } else { b.code_top < a.code_top && ra.top + ra.height <= rb.top }
}

/// `isInTheWayOf`: covers the other widget, or sits above or below it out of code order.
pub(super) fn is_in_the_way_of(widget: Obstacle, other: Obstacle) -> bool {
    rects_intersect(widget.rect, other.rect) || are_out_of_order(widget, other)
}

/// `IFloatingWidgetPlacementRequest`.
#[derive(Clone, Debug)]
pub(super) struct PlacementRequest {
    /// The leftmost position the widget may take.
    pub min_left: f32,
    /// The rightmost position its right edge may take.
    pub max_right: f32,
    pub width: f32,
    pub height: f32,
    /// The commented code in document coordinates, None for a comment on the whole file.
    pub code: Option<Rect>,
    pub obstacles: Vec<Obstacle>,
    /// The height the editor can scroll through, which keeps the widget in.
    pub scrollable_height: f32,
    /// Right after the end of the code, level with its top, when there is room (a new comment).
    pub beside_code: bool,
    /// Where the lines next to the code end: right after them, centered on the code, when there is room.
    pub lines_right: Option<f32>,
}

/// `IFloatingWidgetPlacement`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Placement {
    pub top: f32,
    pub left: f32,
    /// Vertically centered on the code.
    pub centered: bool,
}

/// `computeFloatingWidgetPlacement`: at the editor's right edge centered on the code; beside the code or the lines
/// next to it when asked and there is room; below the code when it leaves no room at the right; then the free position
/// closest to that, keeping the widgets in the order of their code unless there is no room on its own side.
pub(super) fn floating_widget_placement(request: &PlacementRequest) -> Placement {
    let PlacementRequest { min_left, width, height, code, .. } = *request;
    let max_left = min_left.max(request.max_right - EDGE_MARGIN - width);
    let mut left = max_left;
    let mut preferred_top = code.map_or(0., |code| centered_top(code.top, code.top + code.height, height));
    let mut centered = true;
    let code_right = code.map_or(0., |code| code.left + code.width);
    if let Some(code) = code {
        if request.beside_code && code_right + CODE_GAP <= max_left {
            left = min_left.max(code_right + CODE_GAP);
            preferred_top = code.top;
            centered = false;
        } else if code_right + CODE_GAP > max_left {
            preferred_top = code.top + code.height + EDGE_MARGIN;
            centered = false;
        } else if let Some(lines_right) = request.lines_right {
            left = clamp(code_right.max(lines_right) + CODE_GAP, min_left, max_left);
        }
    }

    // The closest free position is the preferred one, an edge of the scrollable area, or just next to an area to keep clear of.
    let max_top = (request.scrollable_height - EDGE_MARGIN - height).max(0.);
    let mut areas: Vec<Rect> = request.obstacles.iter().map(|obstacle| obstacle.rect).collect();
    areas.extend(code);
    let mut candidates = vec![preferred_top, 0., max_top];
    for area in &areas {
        candidates.push(area.top + area.height + EDGE_MARGIN);
        candidates.push(area.top - EDGE_MARGIN - height);
    }
    let closest_free_top = |is_allowed: &dyn Fn(f32) -> bool| {
        let mut closest: Option<f32> = None;
        for &candidate in &candidates {
            let candidate_top = clamp(candidate, 0., max_top);
            if closest.is_none_or(|closest| (candidate_top - preferred_top).abs() < (closest - preferred_top).abs())
                && is_allowed(candidate_top)
                && !areas.iter().any(|&area| rects_intersect(Rect { top: candidate_top, left, width, height }, area))
            {
                closest = Some(candidate_top);
            }
        }
        closest
    };

    // Out of order with a widget above or below: on the side of it that its code isn't on.
    let code_top = code.map_or(0., |code| code.top);
    let is_out_of_order = |top: f32, obstacle: Obstacle| are_out_of_order(Obstacle { rect: Rect { top, left, width, height }, code_top }, obstacle);
    // Widgets already out of order with the preferred position (moved by the user, or for earlier code that runs long
    // enough to be centered below this one) don't hold this one back.
    let unmoved_top = clamp(preferred_top, 0., max_top);
    let ordered: Vec<Obstacle> = request.obstacles.iter().copied().filter(|&obstacle| !is_out_of_order(unmoved_top, obstacle)).collect();
    // With no free position the widget stays as close to the preferred one as it can.
    let top = closest_free_top(&|top| !ordered.iter().any(|&obstacle| is_out_of_order(top, obstacle))).or_else(|| closest_free_top(&|_| true)).unwrap_or(unmoved_top);
    Placement { top, left, centered: centered && top == preferred_top }
}

/// `IFloatingWidgetContentWidths`: the widths the content needs, whatever the widget's width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ContentWidths {
    /// Shows the title actions, the reply and the additional actions in full.
    pub minimum: f32,
    /// Nothing wraps or is cut off.
    pub natural: f32,
}

/// `IFloatingWidgetContentHeights`: the content's heights at the widget's current width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ContentHeights {
    /// Everything but the comments: the frame, title, reply and additional actions.
    pub fixed: f32,
    pub comments: f32,
}

/// `FloatingWidgetSizeModel`: sizes a widget to its content, the size the user chose and the editor's size, captured
/// when the widget shows, resets or is resized, so resizing the editor leaves a shown widget alone. When the reply below
/// the comments grows the widget grows down by as much; past the editor's bottom it moves up instead.
#[derive(Clone, Debug, Default)]
pub(super) struct SizeModel {
    editor_size: Size,
    /// The fixed content height when the size and placement were chosen.
    fixed_height_base: Option<f32>,
    growth: f32,
    upward_shift: f32,
}

impl SizeModel {
    pub fn editor_size(&self) -> Size {
        self.editor_size
    }

    #[cfg(test)]
    pub fn upward_shift(&self) -> f32 {
        self.upward_shift
    }

    pub fn capture_editor_size(&mut self, editor_size: Size) {
        self.editor_size = editor_size;
    }

    /// Takes the editor's current size without first shrinking to an editor that got smaller since the capture.
    pub fn grow_editor_size(&mut self, editor_size: Size) {
        self.editor_size = Size { width: self.editor_size.width.max(editor_size.width), height: self.editor_size.height.max(editor_size.height) };
    }

    /// The reply and additional actions as next measured are the ones the size and placement are for.
    pub fn reset_growth(&mut self) {
        self.fixed_height_base = None;
        self.growth = 0.;
        self.upward_shift = 0.;
    }

    /// The user moved the widget, which takes in the upward shift.
    pub fn clear_upward_shift(&mut self) {
        self.upward_shift = 0.;
    }

    /// Measures how much the reply and additional actions grew; returns whether that changed.
    pub fn update_growth(&mut self, content: ContentHeights) -> bool {
        let base = *self.fixed_height_base.get_or_insert(content.fixed);
        let growth = content.fixed - base;
        let changed = growth != self.growth;
        self.growth = growth;
        changed
    }

    /// Wide enough for the title actions, reply and additional actions, no wider than the editor.
    pub fn width_range(&self, content: ContentWidths) -> SizeRange {
        let editor_width = self.editor_size.width;
        SizeRange { min: MIN_WIDTH.max(content.minimum).min(editor_width), max: editor_width }
    }

    /// The preferred width, but no wider than the content fills unless a reply or comment is being written.
    pub fn default_width(&self, content: ContentWidths, preferred_width: f32, is_editing: bool) -> f32 {
        let range = self.width_range(content);
        clamp(if is_editing { preferred_width } else { preferred_width.min(content.natural) }, range.min, range.max)
    }

    /// Tall enough for the title, reply, actions and some comments; no taller than the content or the editor.
    pub fn height_range(&self, content: ContentHeights) -> SizeRange {
        let max = (content.fixed + content.comments).min(MIN_HEIGHT.max(self.editor_size.height - 2. * EDGE_MARGIN));
        SizeRange { min: (content.fixed + content.comments.min(MIN_COMMENTS_HEIGHT)).min(max), max }
    }

    /// The tallest a widget sizes itself to without a chosen height, before its reply grows.
    pub fn max_auto_height(&self) -> f32 {
        MIN_HEIGHT.max((self.editor_size.height * AUTO_HEIGHT_RATIO).floor())
    }

    /// The chosen height, or the content's; the reply's growth adds to it rather than taking from the comments.
    pub fn height(&self, content: ContentHeights, chosen_height: Option<f32>) -> f32 {
        let range = self.height_range(content);
        let height = match chosen_height {
            Some(chosen) => chosen + self.growth,
            None => range.max.min(self.max_auto_height() + self.growth),
        };
        clamp(height, range.min, range.max)
    }

    /// The comments get what the rest leaves; a widget too short for the rest scrolls whole and still shows some comments.
    pub fn comments_height(&self, content: ContentHeights, height: f32) -> f32 {
        content.comments.min(MIN_COMMENTS_HEIGHT).max(height - content.fixed)
    }

    /// The widget's document top: a centered one stays centered as before its reply grew, so it grows down; kept
    /// within the scrollable area.
    pub fn top(&self, offset_top: Option<f32>, code: Span, height: f32, scrollable_height: f32) -> f32 {
        clamp(self.unshifted_top(offset_top, code, height) - self.upward_shift, 0., (scrollable_height - EDGE_MARGIN - height).max(0.))
    }

    /// Moves the widget up as far as its grown reply needs to show above the viewport's bottom, without moving its top
    /// out of view; it doesn't move back down while the reply stays grown.
    pub fn update_upward_shift(&mut self, offset_top: Option<f32>, code: Span, height: f32, scroll_top: f32, viewport_height: f32) {
        if self.growth <= 0. {
            self.upward_shift = 0.;
            return;
        }
        let top = self.unshifted_top(offset_top, code, height) - scroll_top;
        if top >= viewport_height {
            // Out of view, so its reply isn't being shown.
            return;
        }
        let overflow = top + height - (viewport_height - EDGE_MARGIN);
        let room_above = (top - EDGE_MARGIN).max(0.);
        self.upward_shift = self.upward_shift.max(clamp(overflow, 0., room_above));
    }

    fn unshifted_top(&self, offset_top: Option<f32>, code: Span, height: f32) -> f32 {
        match offset_top {
            Some(offset) => code.top + offset,
            None => centered_top(code.top, code.bottom, height - self.growth),
        }
    }
}
