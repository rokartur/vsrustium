//! The editor's viewport: scroll position, scroll size and reveal, as viewLayout.ts, scrollable.ts and
//! viewLines.ts compute them for one line height and no wrapping. Pixels are the reference's CSS pixels.

use std::ops::Range;

use super::{OpenEditor, Position};
#[cfg(test)]
use super::Selection;

pub const LINE_HEIGHT: f32 = 18.;
/// `editor.scrollbar.verticalScrollbarSize` and `horizontalScrollbarSize`.
pub const VERTICAL_SCROLLBAR_WIDTH: f32 = 14.;
pub const HORIZONTAL_SCROLLBAR_HEIGHT: f32 = 12.;
/// `editor.scrollBeyondLastColumn`, in typical halfwidth characters.
const SCROLL_BEYOND_LAST_COLUMN: f32 = 4.;
/// `editor.stickyScroll.maxLineCount`: with sticky scroll on, a reveal keeps this many lines above the cursor.
const STICKY_SCROLL_MAX_LINE_COUNT: f32 = 5.;
/// scrollableElement.ts `SCROLL_WHEEL_SENSITIVITY`, and `editor.fastScrollSensitivity` for Alt.
const SCROLL_WHEEL_SENSITIVITY: f32 = 50.;
const FAST_SCROLL_SENSITIVITY: f32 = 5.;
/// viewLines.ts `HORIZONTAL_EXTRA_PX` and `editor.revealHorizontalRightPadding`.
const HORIZONTAL_EXTRA_PX: f32 = 30.;
const REVEAL_HORIZONTAL_RIGHT_PADDING: f32 = 15.;

/// The size of the scrollable text area: right of the gutter, scrollbars included.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
    /// `typicalHalfwidthCharacterWidth` of the editor font.
    pub char_width: f32,
}

impl Viewport {
    /// `CursorConfiguration.pageSize`: the lines a page move goes, two less than fit.
    pub fn page_size(&self) -> usize {
        ((self.height / LINE_HEIGHT).floor() as usize).saturating_sub(2).max(1)
    }
}

/// The `VerticalRevealType`s that `TextEditorRevealType` maps onto (mainThreadEditor.ts `revealRange`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VerticalReveal {
    Simple,
    Center,
    CenterIfOutsideViewport,
    Top,
}

impl OpenEditor {
    /// `_getContentHeight` with `scrollBeyondLastLine`, at least the viewport.
    pub fn scroll_height(&self, viewport: Viewport) -> f32 {
        let lines = self.document.lines.len() as f32 * LINE_HEIGHT;
        viewport.height.max(lines + (viewport.height - LINE_HEIGHT).max(0.))
    }

    /// `_computeContentWidth` without wrapping: the widest line drawn so far, 4 columns and the vertical scrollbar.
    pub fn scroll_width(&self, viewport: Viewport) -> f32 {
        viewport.width.max(self.max_line_width + SCROLL_BEYOND_LAST_COLUMN * viewport.char_width + VERTICAL_SCROLLBAR_WIDTH)
    }

    /// `ViewLines._ensureMaxLineWidth`: the content width only grows with the lines that were drawn.
    pub fn saw_line_width(&mut self, width: f32) {
        self.max_line_width = self.max_line_width.max(width);
    }

    /// Sets the scroll position, clamped like `Scrollable.validateScrollPosition`. Returns whether it moved.
    pub fn set_scroll(&mut self, top: f32, left: f32, viewport: Viewport) -> bool {
        let top = top.min(self.scroll_height(viewport) - viewport.height).max(0.);
        let left = left.min(self.scroll_width(viewport) - viewport.width).max(0.);
        let moved = (top, left) != (self.scroll.top, self.scroll.left);
        self.scroll.top = top;
        self.scroll.left = left;
        moved
    }

    /// scrollableElement.ts `_onMouseWheel` for `StandardWheelEvent` deltas (one per wheel notch, positive
    /// towards the start): only the predominant axis scrolls, 50px a notch, 5 times that with Alt. Returns whether it moved.
    pub fn scroll_by_wheel(&mut self, mut delta_x: f32, mut delta_y: f32, fast: bool, viewport: Viewport) -> bool {
        if delta_y.abs() >= delta_x.abs() {
            delta_x = 0.;
        } else {
            delta_y = 0.;
        }
        if fast {
            delta_x *= FAST_SCROLL_SENSITIVITY;
            delta_y *= FAST_SCROLL_SENSITIVITY;
        }
        // -0.3 becomes -1 and 0.3 becomes 1, or slow scrolling would never move.
        let step = |delta: f32| {
            let pixels = SCROLL_WHEEL_SENSITIVITY * delta;
            if pixels < 0. { pixels.floor() } else { pixels.ceil() }
        };
        self.set_scroll(self.scroll.top - step(delta_y), self.scroll.left - step(delta_x), viewport)
    }

    /// The lines at least partly in the viewport (`getLinesViewportData` start and end).
    pub fn visible_lines(&self, viewport: Viewport) -> Range<usize> {
        let last = self.document.lines.len() - 1;
        let start = ((self.scroll.top / LINE_HEIGHT).floor() as usize).min(last);
        let bottom = self.scroll.top + viewport.height;
        let end = (((bottom / LINE_HEIGHT).ceil() as usize).max(start + 1)).min(last + 1);
        start..end
    }

    /// The lines wholly in the viewport (`getCompletelyVisibleViewRange`, what `TextEditor.visibleRanges` reports);
    /// a partly shown first or last line counts only when it is the only line.
    pub fn completely_visible_lines(&self, viewport: Viewport) -> Range<usize> {
        let lines = self.visible_lines(viewport);
        let (mut start, mut end) = (lines.start, lines.end - 1);
        if start < end && (start as f32 * LINE_HEIGHT) < self.scroll.top {
            start += 1;
        }
        if start < end && ((end + 1) as f32 * LINE_HEIGHT) > self.scroll.top + viewport.height {
            end -= 1;
        }
        start..end + 1
    }

    /// The vertical scrollbar: the viewport height is its track, the horizontal scrollbar does not shorten it.
    pub fn vertical_scrollbar(&self, viewport: Viewport) -> Scrollbar {
        Scrollbar::new(0., viewport.height, self.scroll_height(viewport), self.scroll.top)
    }

    /// The horizontal scrollbar: its track ends where the vertical one starts.
    pub fn horizontal_scrollbar(&self, viewport: Viewport) -> Scrollbar {
        Scrollbar::new(VERTICAL_SCROLLBAR_WIDTH, viewport.width, self.scroll_width(viewport), self.scroll.left)
    }

    /// `revealAll` after a command or a click (cursor.ts): one cursor reveals its head; several reveal the
    /// lines of all their selections, unless they do not fit, then their columns when every selection is
    /// on one line. A mouse source skips the sticky scroll padding.
    pub fn reveal_cursors(&mut self, source: &str, viewport: Viewport) {
        let single = self.selections.len() == 1;
        let ranges: Vec<(Position, Position)> = match single {
            true => vec![(self.selection().head, self.selection().head)],
            false => self.selections.iter().map(|selection| (selection.start(), selection.end())).collect(),
        };
        let first = ranges.iter().map(|range| range.0.line).min().expect("an editor has a cursor");
        let last = ranges.iter().map(|range| range.1.line).max().expect("an editor has a cursor");
        let box_lines = (first as f32 * LINE_HEIGHT, (last + 1) as f32 * LINE_HEIGHT);
        let Some(top) = scroll_top_to_reveal(self.scroll.top, viewport.height, box_lines, VerticalReveal::Simple, source == "mouse", single) else {
            return;
        };
        self.set_scroll(top, self.scroll.left, viewport);
        if ranges.iter().all(|(start, end)| start.line == end.line) {
            self.reveal_columns(&ranges, !single, viewport);
        }
    }

    /// `revealRange` from the API (`TextEditor.revealRange`, source "api"): a range over several lines scrolls
    /// back to the left edge.
    pub fn reveal_range(&mut self, start: Position, end: Position, reveal: VerticalReveal, viewport: Viewport) {
        let box_lines = (start.line as f32 * LINE_HEIGHT, (end.line + 1) as f32 * LINE_HEIGHT);
        let Some(top) = scroll_top_to_reveal(self.scroll.top, viewport.height, box_lines, reveal, false, true) else {
            return;
        };
        self.set_scroll(top, self.scroll.left, viewport);
        if start.line != end.line {
            self.set_scroll(self.scroll.top, 0., viewport);
            return;
        }
        self.reveal_columns(&[(start, end)], false, viewport);
    }

    /// `revealTopMostCursor`/`revealBottomMostCursor`: one cursor position, vertically and horizontally.
    pub fn reveal_position(&mut self, position: Position, source: &str, viewport: Viewport) {
        let box_lines = (position.line as f32 * LINE_HEIGHT, (position.line + 1) as f32 * LINE_HEIGHT);
        if let Some(top) = scroll_top_to_reveal(self.scroll.top, viewport.height, box_lines, VerticalReveal::Simple, source == "mouse", true) {
            self.set_scroll(top, self.scroll.left, viewport);
        }
        self.reveal_columns(&[(position, position)], false, viewport);
    }

    /// `_computeScrollLeftToReveal` over one-line ranges: 30px of room before the box and 15px after it.
    fn reveal_columns(&mut self, ranges: &[(Position, Position)], selections: bool, viewport: Viewport) {
        let tab_size = self.document.indentation.tab_size;
        let x = |position: Position| (super::visible_column(&self.document.lines[position.line], position.column, tab_size) as f32 * viewport.char_width).round();
        let box_start = ranges.iter().map(|range| x(range.0)).fold(f32::MAX, f32::min);
        let box_end = ranges.iter().map(|range| x(range.1)).fold(0., f32::max);
        let box_start = (box_start - HORIZONTAL_EXTRA_PX).max(0.);
        let box_end = box_end + REVEAL_HORIZONTAL_RIGHT_PADDING;
        if selections && box_end - box_start > viewport.width {
            return;
        }
        let viewport_end = self.scroll.left + viewport.width - VERTICAL_SCROLLBAR_WIDTH;
        let left = minimum_scrolling(self.scroll.left, viewport_end, box_start, box_end, false);
        self.set_scroll(self.scroll.top, left, viewport);
    }
}

/// scrollbarState.ts for a scrollbar without arrows: the slider's size and offset along the track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scrollbar {
    pub track: f32,
    pub needed: bool,
    pub slider_size: f32,
    pub slider_position: f32,
    ratio: f32,
}

/// scrollbarState.ts `MINIMUM_SLIDER_SIZE`.
const MINIMUM_SLIDER_SIZE: f32 = 20.;

impl Scrollbar {
    /// `_computeValues` with no arrows.
    fn new(opposite_size: f32, visible: f32, scroll_size: f32, position: f32) -> Self {
        let track = (visible - opposite_size).max(0.);
        if !(scroll_size > 0. && scroll_size > visible) {
            return Scrollbar { track: track.round(), needed: false, slider_size: track.round(), slider_position: 0., ratio: 0. };
        }
        let slider_size = MINIMUM_SLIDER_SIZE.max((visible * track / scroll_size).floor()).round();
        let ratio = (track - slider_size) / (scroll_size - visible);
        Scrollbar { track: track.round(), needed: true, slider_size, slider_position: (position * ratio).round(), ratio }
    }

    /// `getDesiredScrollPositionFromOffset`: a press on the track centers the slider on the pointer.
    pub fn position_at(&self, offset: f32) -> f32 {
        if !self.needed {
            return 0.;
        }
        ((offset - self.slider_size / 2.) / self.ratio).round()
    }

    /// `getDesiredScrollPositionFromDelta`: the slider dragged by `delta` from where this state had it.
    pub fn position_dragged(&self, delta: f32) -> f32 {
        if !self.needed {
            return 0.;
        }
        ((self.slider_position + delta) / self.ratio).round()
    }
}

/// viewLines.ts `_computeScrollTopToRevealRange` (not minimal, `cursorSurroundingLines` 0) for a box of whole lines;
/// `None` keeps the scroll. A box of selections taller than the viewport does not scroll.
fn scroll_top_to_reveal(viewport_top: f32, viewport_height: f32, (box_start, box_end): (f32, f32), reveal: VerticalReveal, ignore_scroll_off: bool, single_range: bool) -> Option<f32> {
    let viewport_end = viewport_top + viewport_height;
    let (mut padding_top, mut padding_bottom) = (LINE_HEIGHT, 0.);
    if !ignore_scroll_off {
        let context = (viewport_height / LINE_HEIGHT / 2.).min(STICKY_SCROLL_MAX_LINE_COUNT);
        padding_top = context * LINE_HEIGHT;
        padding_bottom = (context - 1.).max(0.) * LINE_HEIGHT;
    }
    // Reveal one line more when the last line would be under the horizontal scrollbar.
    if reveal == VerticalReveal::Simple {
        padding_bottom += LINE_HEIGHT;
    }
    let (box_start, box_end) = (box_start - padding_top, box_end + padding_bottom);
    let inside = viewport_top <= box_start && box_end <= viewport_end;
    if box_end - box_start > viewport_height {
        return single_range.then_some(box_start);
    }
    Some(match reveal {
        VerticalReveal::CenterIfOutsideViewport if inside => viewport_top,
        VerticalReveal::Center | VerticalReveal::CenterIfOutsideViewport => ((box_start + box_end) / 2. - viewport_height / 2.).max(0.),
        VerticalReveal::Simple | VerticalReveal::Top => minimum_scrolling(viewport_top, viewport_end, box_start, box_end, reveal == VerticalReveal::Top),
    })
}

/// viewLines.ts `_computeMinimumScrolling`, with its integer truncation.
fn minimum_scrolling(viewport_start: f32, viewport_end: f32, box_start: f32, box_end: f32, at_start: bool) -> f32 {
    let (viewport_start, viewport_end, box_start, box_end) = (viewport_start.trunc(), viewport_end.trunc(), box_start.trunc(), box_end.trunc());
    let viewport_length = viewport_end - viewport_start;
    if box_end - box_start >= viewport_length || at_start || box_start < viewport_start {
        return box_start;
    }
    if box_end > viewport_end {
        return (box_end - viewport_length).max(0.);
    }
    viewport_start
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::edit::tests::editor;

    fn editor_with(text: &str) -> OpenEditor {
        editor(text, "\n")
    }

    const VIEWPORT: Viewport = Viewport { width: 400., height: 360., char_width: 7.2 };

    #[test]
    fn scrolls_past_the_last_line_by_one_viewport_less_one_line() {
        let mut editor = editor_with(&"x\n".repeat(99));
        assert_eq!(editor.scroll_height(VIEWPORT), 100. * 18. + 342.);
        editor.set_scroll(1e9, 0., VIEWPORT);
        assert_eq!(editor.scroll.top, 100. * 18. + 342. - 360.);
        assert_eq!(editor.visible_lines(VIEWPORT), 99..100);
        editor.set_scroll(27., 0., VIEWPORT);
        assert_eq!(editor.visible_lines(VIEWPORT), 1..22);
    }

    #[test]
    fn wheel_scrolls_the_predominant_axis_50px_a_notch() {
        let mut editor = editor_with(&format!("{}\n", "y".repeat(200)).repeat(99));
        editor.saw_line_width(200. * 7.2);
        assert!(editor.scroll_by_wheel(0.2, -1., false, VIEWPORT));
        assert_eq!((editor.scroll.top, editor.scroll.left), (50., 0.));
        editor.scroll_by_wheel(0., -0.1, true, VIEWPORT);
        assert_eq!(editor.scroll.top, 75.);
        editor.scroll_by_wheel(-0.25, 0., false, VIEWPORT);
        assert_eq!(editor.scroll.left, 13.);
        editor.scroll_by_wheel(0., 100., false, VIEWPORT);
        assert_eq!(editor.scroll.top, 0.);
    }

    #[test]
    fn completely_visible_lines_drop_the_partly_shown_ends() {
        let mut editor = editor_with(&"x\n".repeat(99));
        assert_eq!(editor.completely_visible_lines(VIEWPORT), 0..20);
        editor.set_scroll(9., 0., VIEWPORT);
        assert_eq!(editor.visible_lines(VIEWPORT), 0..21);
        assert_eq!(editor.completely_visible_lines(VIEWPORT), 1..20);
    }

    #[test]
    fn scrollbar_slider_follows_scrollbar_state() {
        let mut editor = editor_with(&"x\n".repeat(99));
        // scrollHeight 1800 + 342; slider floor(360 * 360 / 2142) = 60.
        let bar = editor.vertical_scrollbar(VIEWPORT);
        assert_eq!((bar.needed, bar.track, bar.slider_size, bar.slider_position), (true, 360., 60., 0.));
        assert_eq!(bar.position_at(360.), 1960.);
        assert_eq!(bar.position_dragged(150.), 891.);
        editor.set_scroll(891., 0., VIEWPORT);
        assert_eq!(editor.vertical_scrollbar(VIEWPORT).slider_position, 150.);
        // A short line needs no horizontal scrollbar.
        assert!(!editor.horizontal_scrollbar(VIEWPORT).needed);
        // scrollBeyondLastLine makes even a 4 line file scroll.
        let tiny = editor_with(&"x\n".repeat(3));
        assert_eq!(tiny.vertical_scrollbar(VIEWPORT).slider_size, 313.);
    }

    #[test]
    fn keyboard_reveal_keeps_the_sticky_scroll_lines_and_one_more_below() {
        let mut editor = editor_with(&"x\n".repeat(99));
        // 20 lines fit: the context is min(20 / 2, 5) = 5 lines above, and 4 + 1 below.
        editor.selections = vec![Selection::caret(Position::new(20, 0))];
        editor.reveal_cursors("keyboard", VIEWPORT);
        assert_eq!(editor.scroll.top, (21. + 5.) * 18. - 360.);
        editor.selections = vec![Selection::caret(Position::new(10, 0))];
        editor.reveal_cursors("keyboard", VIEWPORT);
        assert_eq!(editor.scroll.top, 5. * 18.);
        // Visible with its padding: no scroll.
        editor.selections = vec![Selection::caret(Position::new(12, 0))];
        editor.reveal_cursors("keyboard", VIEWPORT);
        assert_eq!(editor.scroll.top, 5. * 18.);
        // A click reveals one line above and one below.
        editor.selections = vec![Selection::caret(Position::new(30, 0))];
        editor.reveal_cursors("mouse", VIEWPORT);
        assert_eq!(editor.scroll.top, 32. * 18. - 360.);
    }

    #[test]
    fn api_reveal_centers_or_tops_the_range() {
        // Centered with its padding: 5 lines above, 4 below.
        let mut editor = editor_with(&"x\n".repeat(99));
        editor.reveal_range(Position::new(50, 0), Position::new(50, 0), VerticalReveal::Center, VIEWPORT);
        assert_eq!(editor.scroll.top, 50. * 18. - 180.);
        editor.reveal_range(Position::new(52, 0), Position::new(52, 0), VerticalReveal::CenterIfOutsideViewport, VIEWPORT);
        assert_eq!(editor.scroll.top, 50. * 18. - 180.);
        editor.reveal_range(Position::new(30, 0), Position::new(30, 0), VerticalReveal::Top, VIEWPORT);
        assert_eq!(editor.scroll.top, 25. * 18.);
        // Cursors further apart than the viewport keep the scroll.
        editor.selections = vec![Selection::caret(Position::new(0, 0)), Selection::caret(Position::new(60, 0))];
        editor.reveal_cursors("keyboard", VIEWPORT);
        assert_eq!(editor.scroll.top, 25. * 18.);
    }

    #[test]
    fn horizontal_reveal_leaves_30px_before_and_15px_after_the_cursor() {
        let mut editor = editor_with(&"y".repeat(200));
        editor.saw_line_width(200. * 7.2);
        editor.selections = vec![Selection::caret(Position::new(0, 100))];
        editor.reveal_cursors("keyboard", VIEWPORT);
        assert_eq!(editor.scroll.left, 720. + 15. - (400. - 14.));
        editor.selections = vec![Selection::caret(Position::new(0, 10))];
        editor.reveal_cursors("keyboard", VIEWPORT);
        assert_eq!(editor.scroll.left, 72. - 30.);
    }
}
