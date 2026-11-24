//! The editor part with open files: the connected tab strip (connectedEditorTabs.css), the breadcrumbs,
//! and the code editor's view parts (margin, current line, word highlights, indent guides, text, cursor,
//! decorationsOverviewRuler.ts). Sizes are the reference's CSS pixels with editor.fontSize 12 and lineHeight 18.

use std::ops::Range;

use std::time::{Duration, Instant};

use gpui::{
    Axis, Bounds, Context, DispatchPhase, Div, ElementInputHandler, Hitbox, HitboxBehavior, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    Rgba, ScrollDelta, ScrollWheelEvent, Stateful, StyledText, Task, Window, canvas, div, fill, font, point, prelude::*, px, size,
};

use super::Workbench;
use crate::brackets::BracketRange;
use crate::editor::{
    HORIZONTAL_SCROLLBAR_HEIGHT, LINE_HEIGHT, OpenEditor, Position, Reveal, Scrollbar, Selection, VERTICAL_SCROLLBAR_WIDTH, Viewport, column_at_visible, commands,
    expand_tabs, visible_column, word_at,
};
use crate::theme::ThemeKind;

const EDITOR_FONT: &str = "Menlo";
const EDITOR_FONT_SIZE: f32 = 12.;
/// The glyph margin, then lineDecorationsWidth 10 plus 16 for the folding controls (editorOptions.ts layout).
const GLYPH_MARGIN_WIDTH: f32 = 18.;
const DECORATIONS_WIDTH: f32 = 26.;
/// The workbench text editor sets lineNumbersMinChars to 3 (textEditor.ts).
const LINE_NUMBERS_MIN_CHARS: usize = 3;
const OVERVIEW_RULER_WIDTH: f32 = 14.;
const TAB_STRIP_HEIGHT: f32 = 32.;
const BREADCRUMBS_HEIGHT: f32 = 22.;
/// scrollableElement.ts `HIDE_TIMEOUT`; scrollbars.css fades in over 100ms and out over 800ms.
const SCROLLBAR_HIDE_TIMEOUT: Duration = Duration::from_millis(500);
const SCROLLBAR_FADE_IN: Duration = Duration::from_millis(100);
const SCROLLBAR_FADE_OUT: Duration = Duration::from_millis(800);
const SCROLL_SHADOW_HEIGHT: f32 = 6.;

impl Workbench {
    pub(super) fn text_editor_part(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let editor = self.editors.active().expect("text editor part is drawn only with an editor open");
        div()
            .flex_1()
            .relative()
            .flex()
            .flex_col()
            .border_1()
            .rounded(px(8.))
            .bg(self.color("editor.background"))
            .border_color(self.color("surface.border"))
            .child(self.tab_strip(cx))
            .child(self.breadcrumbs(editor))
            .child(self.text_area(editor, window, cx))
            // The active tab reaches into the part's top border, which Chrome still draws over it.
            .child(div().absolute().inset(px(-1.)).border_1().rounded(px(8.)).border_color(self.color("surface.border")))
    }

    /// One strip behind every tab; the active tab is an editor-colored fill whose bottom shoulders curve
    /// into the strip, so the active tab and the editor below read as one surface.
    fn tab_strip(&self, cx: &mut Context<Self>) -> Div {
        let active = self.editors.active_index().expect("tab strip is drawn only with an editor open");
        let icon_color = self.color("icon.foreground");
        let actions = div()
            .ml_auto()
            // Centered above the strip's bottom border, as the tabs are.
            .h(px(TAB_STRIP_HEIGHT - 1.))
            .px(px(4.))
            .flex()
            .items_center()
            .gap(px(4.))
            .child(self.icon("split-horizontal", 22., 16., icon_color))
            .child(self.icon("ellipsis", 22., 16., icon_color));
        // The line under the strip (tabs-border-bottom) runs below the shoulders; the active fill covers it.
        let bottom_line = div().absolute().bottom_0().left_0().right_0().h(px(1.)).bg(self.connected_tab_border());
        div()
            .relative()
            .h(px(TAB_STRIP_HEIGHT))
            .flex_none()
            .flex()
            .bg(self.tab_strip_background())
            .child(bottom_line)
            .children((0..self.editors.editors.len()).map(|index| self.tab(index, index == active, cx)))
            .child(actions)
    }

    /// `--modern-ui-connected-tab-strip-background` (connectedEditorTabs.ts): the connected tabs background,
    /// else the tabs background, else the editor's.
    fn tab_strip_background(&self) -> Rgba {
        ["editorGroupHeader.connectedTabsBackground", "editorGroupHeader.tabsBackground", "editor.background"]
            .into_iter()
            .find_map(|key| self.theme.color(key))
            .expect("editor.background has a registry default")
    }

    /// `--modern-ui-connected-tab-border`: the header's tabs border; in high contrast the focus border of the active group.
    fn connected_tab_border(&self) -> Rgba {
        match self.theme.kind {
            ThemeKind::HighContrastDark | ThemeKind::HighContrastLight => self.color("focusBorder"),
            _ => ["editorGroupHeader.tabsBorder", "tab.border", "editor.background"]
                .into_iter()
                .find_map(|key| self.theme.color(key))
                .expect("editor.background has a registry default"),
        }
    }

    /// A tab sized to its label (tab.sizing fit): 8px before the label, 30.5px after it for the close action.
    fn tab(&self, index: usize, active: bool, cx: &mut Context<Self>) -> Div {
        let editor = &self.editors.editors[index];
        let foreground = if active { self.foreground_or("modernTab.activeForeground") } else { self.foreground_or("tab.inactiveForeground") };
        let label = div()
            .mt(px(3.5))
            .h(px(24.))
            .line_height(px(24.))
            .whitespace_nowrap()
            .when(editor.preview, |label| label.italic())
            .child(editor.document.name());
        // A dirty tab shows a filled circle where the close action goes, also when inactive (tabstitlecontrol.css).
        let close_icon = if editor.is_dirty() { "circle-filled" } else { "close-small" };
        let close = self.icon(close_icon, 20., 16., foreground).id(("close", index)).rounded(px(4.)).on_click(cx.listener(move |workbench, _, _, cx| {
            cx.stop_propagation();
            workbench.editors.close(index);
            workbench.editors_changed(cx);
        }));
        let tab = div()
            .id(("tab", index))
            .relative()
            .h_full()
            .flex_none()
            .flex()
            .pl(px(8.))
            .pr(px(30.5))
            .text_color(foreground)
            .child(label)
            .on_click(cx.listener(move |workbench, event: &gpui::ClickEvent, _, cx| {
                workbench.editors.activate(index);
                if event.click_count() == 2 {
                    workbench.editors.pin(index);
                }
                workbench.editors_changed(cx);
            }))
            .when(active || editor.is_dirty(), |tab| tab.child(div().absolute().top(px(5.5)).right(px(6.5)).child(close)));
        if !active {
            return div().h_full().flex_none().child(tab).mr(px(8.));
        }
        // The fill starts in the part's top border row; its caps and shoulders are cornerRadius-medium plus the stroke.
        let surface = self.color("editor.background");
        let border = self.connected_tab_border();
        let radius = px(7.);
        let shoulder = |piece: Div| piece.absolute().bottom_0().size(radius).bg(surface);
        let strip_corner = || div().size_full().bg(self.tab_strip_background()).border_b_1().border_color(border);
        div()
            .h_full()
            .flex_none()
            .relative()
            .child(div().absolute().top(px(-1.)).left_0().right_0().h(px(TAB_STRIP_HEIGHT + 1.)).bg(surface).border_t_1().border_r_1().rounded_t(radius).border_color(border))
            .when_some(self.theme.color("tab.activeBorder"), |tab, color| tab.child(div().absolute().bottom_0().left_0().right_0().h(px(1.)).bg(color)))
            // The first tab meets the part's own rounded corner, so it has no left shoulder.
            .when(index > 0, |tab| tab.child(shoulder(div().left(-radius)).child(strip_corner().border_r_1().rounded_br(radius))))
            .child(shoulder(div().left_full()).child(strip_corner().border_l_1().rounded_bl(radius)))
            .child(tab)
    }

    /// The file's path below the workspace folder, one item per segment (breadcrumbs.filePath relative).
    fn breadcrumbs(&self, editor: &OpenEditor) -> Div {
        let path = &editor.document.path;
        let relative = self.folder.as_ref().and_then(|tree| path.strip_prefix(&tree.root).ok()).unwrap_or(path);
        let segments: Vec<String> = relative.iter().map(|segment| segment.to_string_lossy().into_owned()).collect();
        let foreground = self.color("breadcrumb.foreground");
        let last = segments.len() - 1;
        let items = segments.into_iter().enumerate().map(|(index, segment)| {
            // shortcut: no file icon theme, so the 16px icon slot stays empty; draw the icon once Seti is ported.
            div()
                .flex()
                .items_center()
                .pl(px(16.))
                .pr(px(8.))
                .whitespace_nowrap()
                .child(segment)
                .when(index < last, |item| item.child(self.icon("chevron-right", 16., 16., foreground).ml(px(8.))))
        });
        // editorGroupView.ts: the title's bottom border, drawn over the breadcrumbs' last row.
        let title_border = ["editorGroupHeader.border", "contrastBorder"].into_iter().find_map(|key| self.theme.color(key));
        div()
            .relative()
            .h(px(BREADCRUMBS_HEIGHT))
            .flex_none()
            .flex()
            .bg(self.color("breadcrumb.background"))
            .text_color(foreground)
            .children(items)
            .when_some(title_border, |crumbs, color| crumbs.child(div().absolute().bottom_0().left_0().right_0().h(px(1.)).bg(color)))
    }

    fn text_area(&self, editor: &OpenEditor, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let metrics = EditorMetrics::measure(window, editor.document.lines.len());
        let area_left = self.text_area_left.clone();
        let focus = self.editor_focus.clone();
        let workbench = cx.entity();
        let laid_out = cx.entity();
        let viewport = self.editors.viewport;
        let lines = editor.visible_lines(viewport);
        let lines_top = lines.start as f32 * LINE_HEIGHT - editor.scroll.top;
        let rows = self.editor_lines(editor, lines, metrics, window, cx);
        if self.scrollbars.in_transition(Instant::now()) {
            window.request_animation_frame();
        }
        div()
            .id("text-area")
            .flex_1()
            .relative()
            .overflow_hidden()
            .font_family(EDITOR_FONT)
            .text_size(px(EDITOR_FONT_SIZE))
            .line_height(px(LINE_HEIGHT))
            .track_focus(&self.editor_focus)
            .on_key_down(cx.listener(Self::editor_key_down))
            .on_scroll_wheel(cx.listener(Self::editor_wheel))
            .on_hover(cx.listener(|workbench, hovered: &bool, _, cx| {
                workbench.scrollbars.mouse_over = *hovered;
                match hovered {
                    true => workbench.reveal_scrollbars(cx),
                    false => workbench.hide_scrollbars(cx),
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|workbench, _, _, _| workbench.selecting_text = false))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|workbench, _, _, _| workbench.selecting_text = false))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        area_left.set(f32::from(bounds.left()));
                        let viewport = Viewport { width: f32::from(bounds.size.width) - metrics.content_left(), height: f32::from(bounds.size.height), char_width: metrics.char_width };
                        laid_out.update(cx, |workbench, cx| workbench.editor_laid_out(viewport, cx));
                    },
                    move |bounds, _, window, cx| window.handle_input(&focus, ElementInputHandler::new(bounds, workbench), cx),
                )
                .absolute()
                .inset_0(),
            )
            .child(div().absolute().left_0().right_0().top(px(lines_top)).children(rows))
            .child(self.overview_ruler(editor).absolute().top_0().right_0().h_full())
            .child(self.scrollbars_canvas(editor, metrics, cx).absolute().inset_0())
            .when(editor.scroll.top > 0., |area| area.child(self.scroll_shadow().absolute().top_0().left_0().right(px(VERTICAL_SCROLLBAR_WIDTH)).h(px(SCROLL_SHADOW_HEIGHT))))
    }

    /// The text area got its size: the group's viewport follows it, which can clamp the scroll position and
    /// change the visible lines; the lines drawn widen the scrollable content (`ViewLines._ensureMaxLineWidth`).
    fn editor_laid_out(&mut self, viewport: Viewport, cx: &mut Context<Self>) {
        let resized = self.editors.viewport != viewport;
        self.editors.viewport = viewport;
        let editor = self.editors.active_mut().expect("laid out only with an editor open");
        let scroll_width = editor.scroll_width(viewport);
        for line in editor.visible_lines(viewport) {
            let text = &editor.document.lines[line];
            editor.saw_line_width(visible_column(text, text.chars().count(), editor.document.indentation.tab_size) as f32 * viewport.char_width);
        }
        let moved = editor.set_scroll(editor.scroll.top, editor.scroll.left, viewport);
        let widened = editor.scroll_width(viewport) != scroll_width;
        if resized || moved {
            self.host.send_visible_ranges(&self.editors);
        }
        if resized || moved || widened {
            cx.notify();
        }
    }

    /// The wheel scrolls the editor in `StandardWheelEvent` units: 40 browser pixels or one line per notch.
    fn editor_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (x, y) = match event.delta {
            ScrollDelta::Pixels(delta) => (f32::from(delta.x) / 40., f32::from(delta.y) / 40.),
            ScrollDelta::Lines(delta) => (delta.x, delta.y),
        };
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("the text area scrolls only with an editor open");
        if editor.scroll_by_wheel(x, y, event.modifiers.alt, viewport) {
            cx.stop_propagation();
            self.editor_scrolled(cx);
        }
    }

    /// scrollableElement.ts `_onDidScroll`: the scrollbars show, and the host hears of new visible lines.
    pub(super) fn editor_scrolled(&mut self, cx: &mut Context<Self>) {
        self.host.send_visible_ranges(&self.editors);
        self.reveal_scrollbars(cx);
        cx.notify();
    }

    /// `_reveal`: the scrollbars fade in, and fade out 500ms later unless the pointer is over the editor.
    fn reveal_scrollbars(&mut self, cx: &mut Context<Self>) {
        self.scrollbars.set_shown(true, Instant::now());
        self.scrollbars.hide_timer = None;
        if !self.scrollbars.mouse_over && self.scrollbars.drag.is_none() {
            self.scrollbars.hide_timer = Some(cx.spawn(async move |workbench, cx| {
                cx.background_executor().timer(SCROLLBAR_HIDE_TIMEOUT).await;
                workbench.update(cx, |workbench, cx| workbench.hide_scrollbars(cx)).ok();
            }));
        }
        cx.notify();
    }

    /// `_hide`: kept while the pointer is over the editor or drags a slider.
    fn hide_scrollbars(&mut self, cx: &mut Context<Self>) {
        if !self.scrollbars.mouse_over && self.scrollbars.drag.is_none() {
            self.scrollbars.set_shown(false, Instant::now());
            cx.notify();
        }
    }

    /// The sliders over the overview ruler and along the bottom (scrollbars.css, abstractScrollbar.ts): a press
    /// on a track centers the slider on the pointer, then the slider follows the pointer until release.
    fn scrollbars_canvas(&self, editor: &OpenEditor, metrics: EditorMetrics, cx: &mut Context<Self>) -> gpui::Canvas<Vec<(Hitbox, Bounds<Pixels>, Axis)>> {
        let viewport = self.editors.viewport;
        let bars = [(Axis::Vertical, editor.vertical_scrollbar(viewport)), (Axis::Horizontal, editor.horizontal_scrollbar(viewport))];
        let opacity = self.scrollbars.opacity(Instant::now());
        let shown = self.scrollbars.shown;
        let drag = self.scrollbars.drag;
        let faded = |key: &str| {
            let color = self.color(key);
            Rgba { a: color.a * opacity, ..color }
        };
        let colors = [faded("scrollbarSlider.background"), faded("scrollbarSlider.hoverBackground"), faded("scrollbarSlider.activeBackground")];
        let workbench = cx.entity();
        canvas(
            move |bounds, window, _| {
                let mut tracks = Vec::new();
                for (axis, bar) in bars {
                    let track = match axis {
                        Axis::Vertical => Bounds::new(point(bounds.right() - px(VERTICAL_SCROLLBAR_WIDTH), bounds.top()), size(px(VERTICAL_SCROLLBAR_WIDTH), px(bar.track))),
                        Axis::Horizontal => Bounds::new(point(bounds.left() + px(metrics.content_left()), bounds.bottom() - px(HORIZONTAL_SCROLLBAR_HEIGHT)), size(px(bar.track), px(HORIZONTAL_SCROLLBAR_HEIGHT))),
                    };
                    // An invisible scrollbar takes no pointer events (.invisible { pointer-events: none }).
                    if bar.needed && shown {
                        tracks.push((window.insert_hitbox(track, HitboxBehavior::BlockMouse), track, axis));
                    }
                }
                tracks
            },
            move |_, tracks, window, _| {
                let mut sliders = Vec::new();
                for (axis, bar) in bars {
                    let Some((_, track, _)) = tracks.iter().find(|(_, _, track_axis)| *track_axis == axis) else { continue };
                    let slider = slider_bounds(*track, axis, bar);
                    let color = match drag {
                        Some(drag) if drag.axis == axis => colors[2],
                        _ if slider.contains(&window.mouse_position()) => colors[1],
                        _ => colors[0],
                    };
                    window.paint_quad(fill(slider, color));
                    sliders.push(slider);
                }
                let hovered_slider = move |window: &Window| sliders.iter().any(|slider| slider.contains(&window.mouse_position()));
                let painted_hover = hovered_slider(window);
                let workbench_moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    if let Some(drag) = drag {
                        let along = if drag.axis == Axis::Vertical { event.position.y } else { event.position.x };
                        workbench_moved.update(cx, |workbench, cx| workbench.drag_slider(drag, f32::from(along), cx));
                    } else if hovered_slider(window) != painted_hover {
                        window.refresh();
                    }
                });
                let workbench_released = workbench.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left && drag.is_some() {
                        workbench_released.update(cx, |workbench, cx| {
                            workbench.scrollbars.drag = None;
                            workbench.hide_scrollbars(cx);
                        });
                    }
                });
                let workbench = workbench.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                        return;
                    }
                    let Some((_, track, axis)) = tracks.iter().find(|(hitbox, _, _)| hitbox.is_hovered(window)) else { return };
                    let (track, axis) = (*track, *axis);
                    let along = |position: Point<Pixels>| f32::from(if axis == Axis::Vertical { position.y - track.top() } else { position.x - track.left() });
                    let offset = along(event.position);
                    cx.stop_propagation();
                    workbench.update(cx, |workbench, cx| workbench.press_scrollbar(axis, offset, f32::from(if axis == Axis::Vertical { event.position.y } else { event.position.x }), cx));
                });
            },
        )
    }

    /// A press on a track: outside the slider it jumps to center the slider there, then a drag starts.
    fn press_scrollbar(&mut self, axis: Axis, offset: f32, pointer: f32, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("scrollbars show only with an editor open");
        let bar = scrollbar(editor, axis, viewport);
        let on_slider = (bar.slider_position..bar.slider_position + bar.slider_size).contains(&offset);
        if !on_slider {
            set_axis_scroll(editor, axis, bar.position_at(offset), viewport);
        }
        let state = scrollbar(editor, axis, viewport);
        self.scrollbars.drag = Some(SliderDrag { axis, pointer, state });
        self.editor_scrolled(cx);
    }

    fn drag_slider(&mut self, drag: SliderDrag, pointer: f32, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("scrollbars show only with an editor open");
        if set_axis_scroll(editor, drag.axis, drag.state.position_dragged(pointer - drag.pointer), viewport) {
            self.editor_scrolled(cx);
        }
    }

    /// scrollDecoration.css: `box-shadow: var(--vscode-scrollbar-shadow) 0 6px 6px -6px inset` over a 6px strip,
    /// which leaves the blurred top edge only: alpha falls off as a Gaussian with sigma 3px, half strength at the edge.
    fn scroll_shadow(&self) -> gpui::Canvas<()> {
        let shadow = self.theme.color("scrollbar.shadow");
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let Some(shadow) = shadow else { return };
                let scale = window.scale_factor();
                let rows = (SCROLL_SHADOW_HEIGHT * scale) as usize;
                for row in 0..rows {
                    let y = (row as f32 + 0.5) / scale;
                    let alpha = shadow.a * 0.5 * erfc(y / (3. * std::f32::consts::SQRT_2));
                    let origin = bounds.origin + point(px(0.), px(row as f32 / scale));
                    window.paint_quad(fill(Bounds::new(origin, size(bounds.size.width, px(1. / scale))), Rgba { a: alpha, ..shadow }));
                }
            },
        )
    }

    fn editor_lines(&self, editor: &OpenEditor, range: Range<usize>, metrics: EditorMetrics, window: &mut Window, cx: &mut Context<Self>) -> Vec<Div> {
        let focused = self.editor_focus.is_focused(window);
        let scroll_left = editor.scroll.left;
        let selections = &editor.selections;
        let tab_size = editor.document.indentation.tab_size;
        let primary_line = editor.selection().head.line;
        // currentLineHighlight.ts: every cursor's line, drawn only while all selections are empty.
        let all_empty = selections.iter().all(|selection| selection.is_empty());
        let bracket_matches = self.bracket_matches(editor);
        let bracket_box = (self.theme.color("editorBracketMatch.background"), self.theme.color("editorBracketMatch.border"));
        let colors = LineColors {
            number: self.color("editorLineNumber.foreground"),
            active_number: self.color("editorLineNumber.activeForeground"),
            text: self.color("editor.foreground"),
            line_border: self.color("editor.lineHighlightBorder"),
            word: self.theme.color("editor.wordHighlightTextBackground"),
            word_border: self.theme.color("editor.wordHighlightTextBorder"),
            guide: self.color("editorIndentGuide.background1"),
            cursor: self.color("editorCursor.foreground"),
            primary_cursor: self.color("editorMultiCursor.primary.foreground"),
            secondary_cursor: self.color("editorMultiCursor.secondary.foreground"),
            selection: self.color(if focused { "editor.selectionBackground" } else { "editor.inactiveSelectionBackground" }),
        };
        // currentLineHighlight.css: high contrast draws the exact border 1px wide.
        let line_border_width = if self.theme.kind.is_high_contrast() { 1. } else { 2. };
        let char_width = metrics.char_width;
        range
            .map(|index| {
                let text = &editor.document.lines[index];
                // lineNumbers.ts marks only the primary cursor's line active.
                let number = div()
                    .w(px(metrics.line_numbers_width))
                    .flex_none()
                    .text_right()
                    .text_color(if index == primary_line { colors.active_number } else { colors.number })
                    .child((index + 1).to_string());
                let mut current_line = div().absolute().inset_0();
                if all_empty && selections.iter().any(|selection| selection.head.line == index) {
                    current_line = current_line.border(px(line_border_width)).border_color(colors.line_border);
                }
                // The text scrolls left under the fixed margin and current line border.
                let mut content = div().absolute().top_0().bottom_0().right_0().left(px(-scroll_left));
                for selection in selections {
                    let Some((start, end, line_break)) = selected_columns(*selection, index, text) else { continue };
                    // A selected line break is one typicalHalfwidthCharacterWidth wide (viewLines.ts linesVisibleRangesForRange).
                    // shortcut: square corners, where roundedSelection rounds the outer ones by 3px.
                    let left = visible_column(text, start, tab_size) as f32 * char_width;
                    let right = visible_column(text, end, tab_size) as f32 * char_width + if line_break { char_width } else { 0. };
                    content = content.child(div().absolute().top_0().h_full().left(px(left)).w(px(right - left)).bg(colors.selection));
                }
                for level in 0..editor.guides[index] {
                    let x = (level * editor.document.indentation.tab_size) as f32 * char_width;
                    content = content.child(div().absolute().top_0().left(px(x)).w(px(1.)).h_full().bg(colors.guide));
                }
                for (_, columns) in editor.highlights.iter().filter(|(line, _)| *line == index) {
                    let start = visible_column(text, columns.start, tab_size) as f32 * char_width;
                    let end = visible_column(text, columns.end, tab_size) as f32 * char_width;
                    let mut highlight = div().absolute().top_0().h_full().left(px(start)).w(px(end - start));
                    if let Some(background) = colors.word {
                        highlight = highlight.bg(background);
                    }
                    // shortcut: GPUI has no dotted border, so the high contrast word highlight is dashed.
                    if let Some(border) = colors.word_border {
                        highlight = highlight.border_1().border_dashed().border_color(border);
                    }
                    content = content.child(highlight);
                }
                // bracketMatching.css .bracket-match: background and a 1px border inside the bracket's box.
                for (_, bytes) in bracket_matches.iter().flat_map(|(pair, _)| pair).filter(|(line, _)| *line == index) {
                    let start = visible_column(text, text[..bytes.start].chars().count(), tab_size) as f32 * char_width;
                    let end = visible_column(text, text[..bytes.end].chars().count(), tab_size) as f32 * char_width;
                    let mut bracket = div().absolute().top_0().h_full().left(px(start)).w(px(end - start));
                    if let Some(background) = bracket_box.0 {
                        bracket = bracket.bg(background);
                    }
                    if let Some(border) = bracket_box.1 {
                        bracket = bracket.border_1().border_color(border);
                    }
                    content = content.child(bracket);
                }
                let highlights = editor.document.tokens.highlights(&self.theme, &self.languages, &editor.document.lines, index, tab_size);
                let text_runs = StyledText::new(expand_tabs(text, tab_size)).with_highlights(highlights);
                content = content.child(div().absolute().top_0().left_0().whitespace_nowrap().child(text_runs));
                for (cursor_index, selection) in selections.iter().enumerate().filter(|(_, selection)| focused && selection.head.line == index) {
                    // shortcut: the cursor does not blink (editor.cursorBlinking "blink" hides it every 500ms).
                    let color = match (selections.len(), cursor_index) {
                        (1, _) => colors.cursor,
                        (_, 0) => colors.primary_cursor,
                        _ => colors.secondary_cursor,
                    };
                    let x = visible_column(text, selection.head.column, tab_size) as f32 * char_width;
                    content = content.child(div().absolute().top_0().left(px(x)).w(px(2.)).h_full().bg(color));
                }
                let visible_at = move |workbench: &Workbench, x: gpui::Pixels| (f32::from(x) - workbench.text_area_left.get() - metrics.content_left() + scroll_left) / char_width;
                let content = div()
                    .relative()
                    .flex_1()
                    .h_full()
                    .overflow_hidden()
                    .text_color(colors.text)
                    .child(current_line)
                    .child(content)
                    .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                        window.focus(&workbench.editor_focus, cx);
                        workbench.selecting_text = true;
                        let modifiers = event.modifiers;
                        workbench.click_text(index, visible_at(workbench, event.position.x), modifiers, modifiers.shift, event.click_count, cx);
                    }))
                    .on_mouse_down(MouseButton::Middle, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                        window.focus(&workbench.editor_focus, cx);
                        workbench.selecting_text = true;
                        workbench.column_select_text(index, visible_at(workbench, event.position.x), event.modifiers.shift, cx);
                    }))
                    .on_mouse_move(cx.listener(move |workbench, event: &MouseMoveEvent, _, cx| {
                        match event.pressed_button {
                            Some(MouseButton::Left) if workbench.selecting_text => workbench.click_text(index, visible_at(workbench, event.position.x), event.modifiers, true, 1, cx),
                            Some(MouseButton::Middle) if workbench.selecting_text => workbench.column_select_text(index, visible_at(workbench, event.position.x), true, cx),
                            _ => {}
                        }
                    }));
                div()
                    .w_full()
                    .h(px(LINE_HEIGHT))
                    .flex()
                    .child(div().w(px(GLYPH_MARGIN_WIDTH)).flex_none())
                    .child(number)
                    .child(div().w(px(DECORATIONS_WIDTH)).flex_none())
                    .child(content)
            })
            .collect()
    }

    /// bracketMatching.ts `_recomputeBrackets`: the brackets matched at each empty cursor, none past 100 selections.
    fn bracket_matches(&self, editor: &OpenEditor) -> Vec<([BracketRange; 2], bool)> {
        if editor.selections.len() > 100 {
            return Vec::new();
        }
        let lines = &editor.document.lines;
        let mut matches = Vec::new();
        for selection in editor.selections.iter().filter(|selection| selection.is_empty()) {
            matches.extend(editor.document.tokens.bracket_match(&self.theme, &self.languages, lines, (selection.head.line, selection.head.column)));
        }
        matches
    }

    /// viewController.ts `dispatchMouse` on the text: a click places the cursor, Shift or a drag extends the
    /// selection, a double click selects the word and a triple click the line. With Alt (`multiCursorModifier`)
    /// a click adds or removes a cursor, a drag extends the cursor added last, a double or triple click
    /// selects its word or line, and with Shift too it selects a column.
    /// shortcut: a drag selects only while the pointer is over the lines, and by chars, not by the words or lines it started with.
    fn click_text(&mut self, line: usize, visible: f32, modifiers: gpui::Modifiers, select: bool, click_count: usize, cx: &mut Context<Self>) {
        let editor = self.editors.active_mut().expect("text is clickable only with an editor open");
        let text = &editor.document.lines[line];
        let column = column_at_visible(text, visible, editor.document.indentation.tab_size);
        let position = Position::new(line, column);
        let before = editor.selections.clone();
        if modifiers.alt && click_count < 4 {
            if modifiers.platform || modifiers.control {
                return;
            }
            // Only the column selection reveals; the other `_lastCursor*` commands and `createCursor` do not.
            let reveal = match click_count {
                2 => {
                    editor.last_cursor_select_word(position);
                    Reveal::Nothing
                }
                3 => {
                    editor.last_cursor_select_line(line);
                    Reveal::Nothing
                }
                _ if modifiers.shift => editor.column_select_to_pointer(line, mouse_column(visible), true),
                _ if select => {
                    editor.last_cursor_select_to(position);
                    Reveal::Nothing
                }
                _ => {
                    editor.toggle_cursor(position);
                    Reveal::Nothing
                }
            };
            self.cursors_changed(before, reveal, "mouse", cx);
            return;
        }
        match click_count {
            2 => {
                let word = word_at(text, column).unwrap_or(column..column);
                editor.move_to(Position::new(line, word.start), false);
                editor.move_to(Position::new(line, word.end), true);
            }
            3.. => {
                let end = if line + 1 < editor.document.lines.len() { Position::new(line + 1, 0) } else { Position::new(line, text.chars().count()) };
                editor.move_to(Position::new(line, 0), false);
                editor.move_to(end, true);
            }
            _ => editor.move_to(Position::new(line, column), select),
        }
        let moved = editor.selections != before;
        self.editor_edited(None, moved, "mouse", cx);
    }

    /// The middle button selects a column (`editor.scrollOnMiddleClick` off): from the pointer, or with
    /// Shift or while dragging from where the last column selection started.
    fn column_select_text(&mut self, line: usize, visible: f32, extend: bool, cx: &mut Context<Self>) {
        let editor = self.editors.active_mut().expect("text is clickable only with an editor open");
        let before = editor.selections.clone();
        let reveal = editor.column_select_to_pointer(line, mouse_column(visible), extend);
        self.cursors_changed(before, reveal, "mouse", cx);
    }

    /// Runs an editor command; false when its precondition does not hold.
    pub(super) fn run_editor_command(&mut self, command: &str, source: &str, cx: &mut Context<Self>) -> bool {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("editor commands run on the active editor");
        let before = editor.selections.clone();
        let Some(reveal) = commands::run(editor, command, viewport) else {
            return false;
        };
        self.cursors_changed(before, reveal, source, cx);
        true
    }

    /// After a command that moves cursors only: scrolls to what it reveals and tells the host.
    fn cursors_changed(&mut self, before: Vec<Selection>, reveal: Reveal, source: &str, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("cursors change in the active editor");
        let scroll = editor.scroll;
        match reveal {
            Reveal::Nothing => {}
            Reveal::All => editor.reveal_cursors(source, viewport),
            Reveal::Position(position) => editor.reveal_position(position, source, viewport),
        }
        let (moved, scrolled) = (editor.selections != before, editor.scroll != scroll);
        if moved {
            self.host.send_selection(self.editors.active().expect("still open"), source);
        }
        if scrolled {
            self.editor_scrolled(cx);
        }
        cx.notify();
    }

    /// decorationsOverviewRuler.ts `_render`, in device pixels: word highlights, then matched brackets in the center lane, the cursor
    /// across all lanes, then the 1px border on the left and top edges.
    fn overview_ruler(&self, editor: &OpenEditor) -> gpui::Canvas<()> {
        let line_count = editor.document.lines.len();
        let highlight_lines: Vec<usize> = editor.highlights.iter().map(|(line, _)| *line).collect();
        let highlight = self.color("editorOverviewRuler.wordHighlightTextForeground");
        let mut bracket_lines = Vec::new();
        for ([open, close], in_ruler) in self.bracket_matches(editor) {
            if in_ruler {
                bracket_lines.extend([open.0, close.0]);
            }
        }
        let bracket_color = self.color("editorOverviewRuler.bracketMatchForeground");
        // decorationsOverviewRuler.ts onCursorStateChanged: the cursor colors, each at 70% opacity.
        let ruler_color = |key: &str| self.theme.color(key).map(|color| Rgba { a: color.a * 0.7, ..color });
        let mut cursors = Vec::new();
        for (index, selection) in editor.selections.iter().enumerate() {
            let key = match (editor.selections.len(), index) {
                (1, _) => "editorCursor.foreground",
                (_, 0) => "editorMultiCursor.primary.foreground",
                _ => "editorMultiCursor.secondary.foreground",
            };
            cursors.push((selection.head.line, ruler_color(key)));
        }
        cursors.sort_by_key(|(line, _)| *line);
        bracket_lines.sort();
        let border = self.theme.color("editorOverviewRuler.border");
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let scale = window.scale_factor();
                let device = |value: f32| px(value / scale);
                let origin = bounds.origin;
                let paint = |window: &mut Window, x: f32, y: f32, width: f32, height: f32, color: Rgba| {
                    window.paint_quad(fill(Bounds::new(origin + point(device(x), device(y)), size(device(width), device(height))), color));
                };
                let canvas_width = (OVERVIEW_RULER_WIDTH * scale).floor();
                let canvas_height = (f32::from(bounds.size.height) * scale).floor();
                let viewport_height = f32::from(bounds.size.height);
                // scrollBeyondLastLine: the last line can scroll to the top of the viewport.
                let scroll_height = line_count as f32 * LINE_HEIGHT + viewport_height - LINE_HEIGHT;
                let ratio = canvas_height / scroll_height;
                let lanes = RulerLanes::new(canvas_width);
                let min_height = (6. * scale).floor();
                for (lines, color) in [(&highlight_lines, highlight), (&bracket_lines, bracket_color)] {
                    let mut previous: Option<(f32, f32)> = None;
                    for line in lines {
                        let mut y1 = (*line as f32 * LINE_HEIGHT * ratio).floor();
                        let mut y2 = ((*line as f32 * LINE_HEIGHT + LINE_HEIGHT) * ratio).floor();
                        if y2 - y1 < min_height {
                            let half = (min_height / 2.).floor();
                            let center = ((y1 + y2) / 2.).floor().clamp(half, canvas_height - half);
                            (y1, y2) = (center - half, center + half);
                        }
                        previous = match previous {
                            Some((start, end)) if y1 <= end + 1. => Some((start, end.max(y2))),
                            Some((start, end)) => {
                                paint(window, lanes.center_x, start, lanes.center_width, end - start, color);
                                Some((y1, y2))
                            }
                            None => Some((y1, y2)),
                        };
                    }
                    if let Some((start, end)) = previous {
                        paint(window, lanes.center_x, start, lanes.center_width, end - start, color);
                    }
                }
                // Cursors sorted by line; touching marks of one color paint as one.
                let height = (2. * scale).floor();
                let half = (height / 2.).floor();
                let mut previous: Option<(f32, f32, Rgba)> = None;
                for (line, color) in cursors.iter().filter_map(|(line, color)| Some((*line, (*color)?))) {
                    let y1 = (line as f32 * LINE_HEIGHT * ratio).floor().clamp(half, canvas_height - half) - half;
                    let y2 = y1 + height;
                    previous = match previous {
                        Some((start, end, previous_color)) if y1 <= end + 1. && previous_color == color => Some((start, end.max(y2), color)),
                        Some((start, end, previous_color)) => {
                            paint(window, lanes.full_x, start, lanes.full_width, end - start, previous_color);
                            Some((y1, y2, color))
                        }
                        None => Some((y1, y2, color)),
                    };
                }
                if let Some((start, end, color)) = previous {
                    paint(window, lanes.full_x, start, lanes.full_width, end - start, color);
                }
                // A 1px canvas stroke along x = 0 and y = 0 covers half of the first device column and row.
                if let Some(border) = border {
                    let half_covered = Rgba { a: border.a * 0.5, ..border };
                    paint(window, 0., 0., 1., canvas_height, half_covered);
                    paint(window, 1., 0., canvas_width - 1., 1., half_covered);
                }
            },
        )
        .w(px(OVERVIEW_RULER_WIDTH))
    }

    /// The selection, indentation, encoding, end of line and language items (editorStatus.ts), right to left
    /// before the notifications bell.
    pub(super) fn editor_status_items(&self, foreground: Rgba) -> Option<Div> {
        let editor = self.editors.active()?;
        let document = &editor.document;
        let indentation = match document.indentation.insert_spaces {
            true => format!("Spaces: {}", document.indentation.tab_size),
            false => format!("Tab Size: {}", document.indentation.tab_size),
        };
        let encoding = if document.encoding == "utf8bom" { "UTF-8 with BOM" } else { "UTF-8" };
        let eol = if document.eol == "\r\n" { "CRLF" } else { "LF" };
        let labels = [selection_label(editor), indentation, encoding.into(), eol.into(), self.languages.name(&document.language_id).to_string()];
        let items = labels.into_iter().map(|label| div().h_full().mx(px(3.)).px(px(5.)).flex().items_center().whitespace_nowrap().child(label));
        Some(div().h_full().flex().text_color(foreground).children(items))
    }
}

#[derive(Clone, Copy)]
struct EditorMetrics {
    char_width: f32,
    line_numbers_width: f32,
}

impl EditorMetrics {
    fn content_left(&self) -> f32 {
        GLYPH_MARGIN_WIDTH + self.line_numbers_width + DECORATIONS_WIDTH
    }

    /// `typicalHalfwidthCharacterWidth` and `maxDigitWidth` of the editor font; the line numbers are as wide
    /// as the longest number, at least lineNumbersMinChars digits, rounded (editorOptions.ts).
    fn measure(window: &Window, line_count: usize) -> Self {
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font(EDITOR_FONT));
        let font_size = px(EDITOR_FONT_SIZE);
        let advance = |c: char| f32::from(text_system.advance(font_id, font_size, c).expect("Menlo has ASCII glyphs").width);
        let max_digit_width = ('0'..='9').map(advance).fold(0., f32::max);
        let digits = line_count.to_string().len().max(LINE_NUMBERS_MIN_CHARS);
        EditorMetrics { char_width: advance('x'), line_numbers_width: (digits as f32 * max_digit_width).round() }
    }
}

struct LineColors {
    number: Rgba,
    active_number: Rgba,
    text: Rgba,
    line_border: Rgba,
    word: Option<Rgba>,
    word_border: Option<Rgba>,
    guide: Rgba,
    cursor: Rgba,
    primary_cursor: Rgba,
    secondary_cursor: Rgba,
    selection: Rgba,
}

/// editorStatus.ts `getSelectionLabel`: the primary cursor's line and visible column for one cursor, the
/// count for several, and the chars selected (`getCharacterCountInRange`: code points, a line break as the EOL's length).
fn selection_label(editor: &OpenEditor) -> String {
    let document = &editor.document;
    let mut selected = 0;
    for selection in &editor.selections {
        let (start, end) = (selection.start(), selection.end());
        for line in start.line..=end.line {
            let first = if line == start.line { start.column } else { 0 };
            let last = if line == end.line { end.column } else { document.lines[line].chars().count() };
            selected += last - first;
        }
        selected += document.eol.len() * (end.line - start.line);
    }
    if let [selection] = editor.selections.as_slice() {
        let Position { line, column } = selection.head;
        let visible = visible_column(&document.lines[line], column, document.indentation.tab_size);
        return match selected {
            0 => format!("Ln {}, Col {}", line + 1, visible + 1),
            _ => format!("Ln {}, Col {} ({selected} selected)", line + 1, visible + 1),
        };
    }
    match selected {
        0 => format!("{} selections", editor.selections.len()),
        _ => format!("{} selections ({selected} characters selected)", editor.selections.len()),
    }
}

/// The selected chars of a line and whether its line break is selected too.
fn selected_columns(selection: Selection, line: usize, text: &str) -> Option<(usize, usize, bool)> {
    let (start, end) = (selection.start(), selection.end());
    if selection.is_empty() || line < start.line || line > end.line {
        return None;
    }
    let first = if line == start.line { start.column } else { 0 };
    let last = if line == end.line { end.column } else { text.chars().count() };
    Some((first, last, line < end.line))
}

/// mouseTarget.ts `mouseColumn` less one: the visible column nearest the pointer, past the line end too.
fn mouse_column(visible: f32) -> usize {
    visible.round().max(0.) as usize
}

/// `_initLanes` with three lanes: one device pixel for the border, then three thirds, the center taking the rest.
struct RulerLanes {
    center_x: f32,
    center_width: f32,
    full_x: f32,
    full_width: f32,
}

impl RulerLanes {
    fn new(canvas_width: f32) -> Self {
        let remaining = canvas_width - 1.;
        let side = (remaining / 3.).floor();
        RulerLanes { center_x: 1. + side, center_width: remaining - 2. * side, full_x: 1., full_width: remaining }
    }
}



/// scrollbarVisibilityController.ts for both scrollbars, with the CSS opacity transition between its states.
#[derive(Default)]
pub(super) struct Scrollbars {
    pub mouse_over: bool,
    shown: bool,
    /// When `shown` last flipped, and the opacity the transition started from.
    changed: Option<(Instant, f32)>,
    hide_timer: Option<Task<()>>,
    drag: Option<SliderDrag>,
}

impl Scrollbars {
    fn set_shown(&mut self, shown: bool, now: Instant) {
        if self.shown != shown {
            let from = self.opacity(now);
            self.shown = shown;
            self.changed = Some((now, from));
        }
    }

    /// Linear from where the last change started: 100ms to visible, 800ms to invisible.
    fn opacity(&self, now: Instant) -> f32 {
        let target = if self.shown { 1. } else { 0. };
        let Some((at, from)) = self.changed else { return target };
        let duration = if self.shown { SCROLLBAR_FADE_IN } else { SCROLLBAR_FADE_OUT };
        let progress = (now.duration_since(at).as_secs_f32() / duration.as_secs_f32()).min(1.);
        from + (target - from) * progress
    }

    fn in_transition(&self, now: Instant) -> bool {
        self.opacity(now) != if self.shown { 1. } else { 0. }
    }
}

#[derive(Clone, Copy)]
struct SliderDrag {
    axis: Axis,
    /// The pointer's window coordinate along the axis at the press, and the scrollbar then.
    pointer: f32,
    state: Scrollbar,
}

fn scrollbar(editor: &OpenEditor, axis: Axis, viewport: Viewport) -> Scrollbar {
    match axis {
        Axis::Vertical => editor.vertical_scrollbar(viewport),
        Axis::Horizontal => editor.horizontal_scrollbar(viewport),
    }
}

fn set_axis_scroll(editor: &mut OpenEditor, axis: Axis, position: f32, viewport: Viewport) -> bool {
    match axis {
        Axis::Vertical => editor.set_scroll(position, editor.scroll.left, viewport),
        Axis::Horizontal => editor.set_scroll(editor.scroll.top, position, viewport),
    }
}

/// The slider spans the scrollbar's thickness (`verticalSliderSize` and `horizontalSliderSize` default to it).
fn slider_bounds(track: Bounds<Pixels>, axis: Axis, bar: Scrollbar) -> Bounds<Pixels> {
    match axis {
        Axis::Vertical => Bounds::new(track.origin + point(px(0.), px(bar.slider_position)), size(track.size.width, px(bar.slider_size))),
        Axis::Horizontal => Bounds::new(track.origin + point(px(bar.slider_position), px(0.)), size(px(bar.slider_size), track.size.height)),
    }
}

/// The complementary error function (Abramowitz and Stegun 7.1.26, error below 1.5e-7).
fn erfc(x: f32) -> f32 {
    let t = 1. / (1. + 0.327_591_1 * x);
    let poly = t * (0.254_829_6 + t * (-0.284_496_74 + t * (1.421_413_7 + t * (-1.453_152_1 + t * 1.061_405_4))));
    poly * (-x * x).exp()
}
