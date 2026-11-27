//! The minimap (minimap.ts `Minimap` and `InnerMinimap`, minimap.css) left of the vertical scrollbar, or at the text
//! area's left edge with `editor.minimap.side: left`: the lines canvas, the decorations canvas over it, the slider and
//! the shadow.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    Bounds, Context, DispatchPhase, Font, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, RenderImage, Rgba, SharedString, Stateful,
    Task, TextRun, Window, canvas, div, fill, font, point, prelude::*, px, size,
};
use image::{Frame, RgbaImage};
use serde_json::json;

use super::Workbench;
use crate::editor::minimap::{
    CharRenderer, LinesColors, MINIMAP_GUTTER_WIDTH, MinimapLayout, MinimapLayoutInfo, MinimapLayoutInput, MinimapOptions, MinimapRange, MinimapSampling, MinimapSide, MinimapSize,
    SliderOptions, compute_minimap_layout, contained_minimap_line_count, decoration_fills, render_lines,
};
use crate::editor::folding::FoldDecoration;
use crate::editor::section_headers::{FindOptions, MARK_SECTION_HEADER_REGEX, SectionHeader, find_section_headers, fit_section_header};
use crate::editor::{OpenEditor, Position, VERTICAL_SCROLLBAR_WIDTH, Viewport};
use crate::theme::relative_luminance;

/// minimap.css `.minimap-slider` opacity transition, and `_hideSliderAfterScrollScheduler`.
const SLIDER_FADE: Duration = Duration::from_millis(100);
const SLIDER_SCROLLING_TIMEOUT: Duration = Duration::from_millis(500);
/// `_hideDelayedScheduler` of `autohide: scroll`.
const MINIMAP_HIDE_DELAY: Duration = Duration::from_millis(500);
/// minimap.css `.minimap-shadow-visible`: a 6px strip left of the minimap with `--vscode-shadow-md`,
/// `0 0 6px rgba(0, 0, 0, 0.08)`, a blur of sigma 3px.
const SHADOW_STRIP_WIDTH: f32 = 6.;
const SHADOW_ALPHA: f32 = 0.08;
const SHADOW_SIGMA: f32 = 3.;

#[derive(Default)]
pub(super) struct Minimap {
    /// The minimap's layout in the text area and the window's pixel ratio, from the last layout.
    pub info: Option<(MinimapLayoutInfo, f64)>,
    /// `_lastRenderData.renderedLayout` with the model it showed: the next layout keeps the lines from jumping back.
    rendered: Cell<Option<(u32, MinimapLayout)>>,
    image: RefCell<Option<Arc<RenderImage>>>,
    /// Replaced images, dropped from the sprite atlas at the next paint.
    stale: Rc<RefCell<Vec<Arc<RenderImage>>>>,
    /// Window y of the minimap, recorded at paint, to turn a press into a minimap row.
    top: Rc<Cell<f32>>,
    mouse_over: bool,
    /// `slider-scrolling`: shown for 500ms after the scroll top changes.
    scrolling: Option<Task<()>>,
    scroll_top: f32,
    drag: Option<MinimapDrag>,
    slider: Fade,
    /// `autohide`: the minimap's own opacity, shown while hovered (`mouseover`) or `active` (`scroll`).
    autohide: Fade,
    /// `active` of `autohide: scroll`: shown after a scroll until 500ms pass without the pointer over it.
    active: Option<Task<()>>,
    /// The section headers of a model version and the find options they were found with.
    section_headers: RefCell<Option<(SectionHeaderKey, Rc<Vec<SectionHeader>>)>>,
}

/// A section header in the minimap's lines: its view line (1-based), label and separator line.
struct SectionLabel {
    view_line: usize,
    text: String,
    has_separator_line: bool,
}

/// `_startSliderDragging`: the pointer's window y at the press, the layout then and the editor's scroll top.
#[derive(Clone, Copy)]
struct MinimapDrag {
    pointer: f32,
    layout: MinimapLayout,
    scroll_top: f32,
}

/// An opacity transition between hidden (0) and shown (1).
#[derive(Default)]
struct Fade {
    shown: bool,
    /// When `shown` last flipped, and the opacity the transition started from.
    changed: Option<(Instant, f32)>,
}

impl Fade {
    fn set(&mut self, shown: bool, now: Instant, transition: Transition) {
        if self.shown != shown {
            let from = self.opacity(now, transition);
            self.shown = shown;
            self.changed = Some((now, from));
        }
    }

    fn opacity(&self, now: Instant, (duration, curve): Transition) -> f32 {
        let target = if self.shown { 1. } else { 0. };
        let Some((at, from)) = self.changed else { return target };
        let progress = (now.duration_since(at).as_secs_f32() / duration.as_secs_f32()).min(1.);
        from + (target - from) * curve(progress)
    }

    fn in_transition(&self, now: Instant, transition: Transition) -> bool {
        self.opacity(now, transition) != if self.shown { 1. } else { 0. }
    }
}

type Transition = (Duration, fn(f32) -> f32);
/// minimap.css: `transition: opacity 100ms linear` on the slider, `transition: opacity 0.5s` (ease) on the minimap.
const SLIDER_TRANSITION: Transition = (SLIDER_FADE, |progress| progress);
const AUTOHIDE_TRANSITION: Transition = (Duration::from_millis(500), super::editor_view::ease);

impl Minimap {
    fn update_slider(&mut self, now: Instant) {
        let shown = self.mouse_over || self.scrolling.is_some() || self.drag.is_some();
        self.slider.set(shown, now, SLIDER_TRANSITION);
    }

    pub(super) fn in_transition(&self, now: Instant) -> bool {
        self.slider.in_transition(now, SLIDER_TRANSITION) || self.autohide.in_transition(now, AUTOHIDE_TRANSITION)
    }
}

/// The `editor.minimap.*` settings of the view, validated as `EditorMinimap.validate`.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct MinimapSettings {
    pub enabled: bool,
    pub layout: MinimapOptions,
    pub slider_always: bool,
    pub autohide: Autohide,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Autohide {
    None,
    Mouseover,
    Scroll,
}

/// `EditorLayoutInfoComputer` for the minimap: the text area's outer size, the gutter's width, the editor font's
/// metrics and the view's line count.
#[allow(clippy::too_many_arguments)]
pub(super) fn minimap_info(outer_width: f32, outer_height: f32, content_left: f32, char_width: f32, line_height: f32, view_line_count: usize, pixel_ratio: f32, options: MinimapOptions) -> MinimapLayoutInfo {
    compute_minimap_layout(&layout_input(outer_width, outer_height, content_left, char_width, line_height, view_line_count, pixel_ratio, options))
}

#[allow(clippy::too_many_arguments)]
fn layout_input(outer_width: f32, outer_height: f32, content_left: f32, char_width: f32, line_height: f32, view_line_count: usize, pixel_ratio: f32, options: MinimapOptions) -> MinimapLayoutInput {
    MinimapLayoutInput {
        outer_width: outer_width as f64,
        outer_height: outer_height as f64,
        line_height: line_height as f64,
        typical_halfwidth_character_width: char_width as f64,
        pixel_ratio: pixel_ratio as f64,
        scroll_beyond_last_line: true,
        padding_top: 0.,
        padding_bottom: 0.,
        vertical_scrollbar_width: VERTICAL_SCROLLBAR_WIDTH as f64,
        view_line_count,
        remaining_width: (outer_width - content_left) as f64,
        options,
    }
}

/// `MinimapSamplingState.compute` for the editor's view lines; only the heights count, the widths are 0.
fn minimap_sampling(editor: &OpenEditor, viewport: Viewport, pixel_ratio: f64, options: MinimapOptions) -> MinimapSampling {
    let input = layout_input(0., viewport.height, 0., viewport.char_width, viewport.line_height, editor.view_line_count(), pixel_ratio as f32, options);
    MinimapSampling::new(editor.view_line_count(), contained_minimap_line_count(&input).minimap_line_count as usize)
}

/// The 1-based view line a 1-based minimap line shows.
fn sampled_view_line(sampling: Option<&MinimapSampling>, minimap_line: usize) -> usize {
    sampling.map_or(minimap_line, |sampling| sampling.lines[minimap_line - 1])
}

impl Workbench {
    /// `InnerMinimap.render`: the slider's layout, the lines and decorations of the lines it covers.
    pub(super) fn minimap(&self, editor: &OpenEditor, cx: &mut Context<Self>) -> Option<Stateful<gpui::Div>> {
        let (info, pixel_ratio) = self.minimap.info?;
        if info.canvas_inner_width == 0. || info.canvas_inner_height == 0. {
            return None;
        }
        let viewport = self.editors.viewport;
        let settings = self.minimap_settings();
        let sampling = info.is_sampling.then(|| Rc::new(minimap_sampling(editor, viewport, pixel_ratio, settings.layout)));
        let layout = self.minimap_layout(editor, info, pixel_ratio, viewport, sampling.as_deref());
        let image = self.minimap_image(editor, &info, &layout, sampling.as_deref());
        let fills = self.minimap_decorations(editor, &info, &layout, sampling.as_deref());
        let label_options = self.section_header_options();
        let labels = self.minimap_section_labels(editor, &info, pixel_ratio, &layout, &label_options, sampling.as_deref());
        let background = self.minimap_background();
        let label_background = Rgba { a: 0.7, ..background };
        let label_foreground = Rgba { a: 1., ..self.theme.color("editor.foreground").unwrap_or_else(|| Rgba::try_from(self.theme.tokens.color_map()[1].as_str()).expect("the token color map holds hex colors")) };
        let now = Instant::now();
        // minimap.css: only `.slider-mouseover` hides the slider.
        let opacity = if settings.slider_always { 1. } else { self.minimap.slider.opacity(now, SLIDER_TRANSITION) };
        let minimap_opacity = if settings.autohide == Autohide::None { 1. } else { self.minimap.autohide.opacity(now, AUTOHIDE_TRANSITION) };
        // `.minimap-autohide-scroll` takes pointer events only while `active`.
        let takes_pointer = settings.autohide != Autohide::Scroll || self.minimap.active.is_some();
        let dragging = self.minimap.drag.is_some();
        let slider_color = |key: &str| self.color(key);
        let (slider, slider_hover, slider_active) = (slider_color("minimapSlider.background"), slider_color("minimapSlider.hoverBackground"), slider_color("minimapSlider.activeBackground"));
        let shadow = editor.scroll.left + viewport.width < editor.scroll_width(viewport);
        let stale = self.minimap.stale.clone();
        let top = self.minimap.top.clone();
        let drag = self.minimap.drag;
        let workbench = cx.entity();
        let lines = canvas(
            move |bounds, _, _| top.set(f32::from(bounds.top())),
            move |bounds, _, window, _| {
                for image in stale.borrow_mut().drain(..) {
                    window.drop_image(image).expect("dropping a sprite cannot fail");
                }
                window.paint_image(bounds, bounds, Default::default(), image, 0, false).expect("the minimap image fits the atlas");
                let device = |value: f64| px((value / pixel_ratio) as f32);
                for decoration in &fills {
                    let origin = bounds.origin + point(device(decoration.x), device(decoration.y));
                    window.paint_quad(fill(Bounds::new(origin, size(device(decoration.width), device(decoration.height))), decoration.color));
                }
                paint_section_labels(&labels, &label_options, &info, pixel_ratio, &layout, label_background, label_foreground, bounds.origin, window);
                let Some(drag) = drag else { return };
                let workbench_moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        workbench_moved.update(cx, |workbench, cx| workbench.drag_minimap_slider(drag, f32::from(event.position.y), cx));
                    }
                });
                let workbench = workbench.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                        workbench.update(cx, |workbench, cx| {
                            workbench.minimap.drag = None;
                            workbench.minimap.update_slider(Instant::now());
                            cx.notify();
                        });
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .w(px(info.canvas_outer_width as f32))
        .h(px(info.canvas_outer_height as f32));
        let slider_fill = div()
            .size_full()
            .bg(if dragging { slider_active } else { slider })
            .when(!dragging, |fill| fill.hover(move |style| style.bg(slider_hover)));
        let slider = div()
            .id("minimap-slider")
            .absolute()
            .left_0()
            .top(px(layout.slider_top as f32))
            .w_full()
            .h(px(layout.slider_height as f32))
            .opacity(opacity)
            .child(slider_fill)
            .when(takes_pointer, |slider| {
                slider.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |workbench, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        workbench.start_minimap_drag(f32::from(event.position.y), layout, cx);
                    }),
                )
            });
        Some(
            div()
                .id("minimap")
                .absolute()
                .top_0()
                .left(px(info.left as f32))
                .w(px(info.width as f32))
                .h(px(viewport.height))
                // _applyLayout paints an opaque minimap background behind the canvas too.
                .when(background.a == 1., |minimap| minimap.bg(background))
                .opacity(minimap_opacity)
                .when(shadow, |minimap| minimap.child(self.minimap_shadow().absolute().top_0().h_full().left(px(-SHADOW_STRIP_WIDTH - 3. * SHADOW_SIGMA)).w(px(3. * SHADOW_SIGMA))))
                .child(lines)
                .when(layout.slider_needed, |minimap| minimap.child(slider))
                .when(takes_pointer, |minimap| {
                    minimap
                        .cursor_default()
                        .block_mouse_except_scroll()
                        .on_hover(cx.listener(|workbench, hovered: &bool, _, cx| {
                            workbench.minimap.mouse_over = *hovered;
                            workbench.minimap_mouse_over_changed(cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |workbench, event: &MouseDownEvent, _, cx| workbench.press_minimap(f32::from(event.position.y), info, pixel_ratio, layout, sampling.as_deref(), cx)),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|workbench, event: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                workbench.show_scrollbar_context_menu(event.position, window, cx);
                            }),
                        )
                }),
        )
    }

    /// `MinimapLayout.create` from the last frame's layout of the same editor.
    fn minimap_layout(&self, editor: &OpenEditor, info: MinimapLayoutInfo, pixel_ratio: f64, viewport: Viewport, sampling: Option<&MinimapSampling>) -> MinimapLayout {
        let options = SliderOptions {
            pixel_ratio,
            minimap_line_height: info.line_height,
            canvas_inner_height: info.canvas_inner_height,
            line_height: viewport.line_height as f64,
            height_is_editor_height: info.height_is_editor_height,
            minimap_height: viewport.height as f64,
            padding_top: 0.,
            padding_bottom: 0.,
            scroll_beyond_last_line: true,
        };
        let previous = self.minimap.rendered.get().filter(|(model_id, _)| *model_id == editor.model_id).map(|(_, layout)| layout);
        let (mut first_line, mut line_count) = (editor.visible_lines(viewport).start + 1, editor.view_line_count());
        if let Some(sampling) = sampling {
            (first_line, line_count) = (sampling.minimap_line(first_line), sampling.lines.len());
        }
        let layout = MinimapLayout::create(&options, first_line, viewport.height as f64, line_count, editor.view_line_count(), editor.scroll.top as f64, editor.scroll_height(viewport) as f64, previous.as_ref());
        self.minimap.rendered.set(Some((editor.model_id, layout)));
        layout
    }

    /// `renderLines`, kept as the last image while its pixels do not change, so the atlas holds one minimap.
    fn minimap_image(&self, editor: &OpenEditor, info: &MinimapLayoutInfo, layout: &MinimapLayout, sampling: Option<&MinimapSampling>) -> Arc<RenderImage> {
        let color_map = self.theme.tokens.color_map();
        let default_background = Rgba::try_from(color_map[2].as_str()).expect("the token color map holds hex colors");
        let colors = LinesColors {
            background: bytes(self.minimap_background()),
            default_background: [default_background.r, default_background.g, default_background.b].map(byte),
            foreground_alpha: self.theme.color("minimap.foregroundOpacity").map_or(255, |color| byte(color.a)),
            use_lighter_font: relative_luminance(default_background) >= 0.5,
        };
        let document = &editor.document();
        let tab_size = document.indentation.tab_size;
        let renderer = CharRenderer::create(info.scale);
        let mut image = render_lines(info, layout, &renderer, &colors, tab_size, |minimap_line| {
            let view_line = sampled_view_line(sampling, minimap_line);
            let index = editor.model_line(view_line - 1);
            editor.view_line_minimap_data(view_line - 1, document.tokens.minimap_runs(&self.theme, &document.lines, index))
        });
        // RenderImage holds BGRA.
        for pixel in image.data.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let mut current = self.minimap.image.borrow_mut();
        if let Some(previous) = current.as_ref().filter(|previous| previous.as_bytes(0) == Some(image.data.as_slice())) {
            return previous.clone();
        }
        let buffer = RgbaImage::from_raw(image.width as u32, image.height as u32, image.data).expect("the buffer holds width * height pixels");
        let next = Arc::new(RenderImage::new([Frame::new(buffer)]));
        if let Some(previous) = current.replace(next.clone()) {
            self.minimap.stale.borrow_mut().push(previous);
        }
        next
    }

    /// `renderDecorations`: the selections in `minimap.selectionHighlight`, then the word highlights in
    /// `minimap.selectionOccurrenceHighlight`, the collapsed folds' whole lines in `editor.foldBackground` and the find
    /// matches in `minimap.findMatchHighlight`.
    fn minimap_decorations(&self, editor: &OpenEditor, info: &MinimapLayoutInfo, layout: &MinimapLayout, sampling: Option<&MinimapSampling>) -> Vec<crate::editor::minimap::Fill> {
        // convertModelPositionToViewPosition: a hidden position shows at the end of its fold line.
        let view = |line: usize, column: usize| {
            if editor.folding.is_hidden(line) {
                let view_line = editor.view_line(line);
                let fold_line = editor.segment(view_line).line;
                let (view_line, column) = editor.view_column(Position::new(fold_line, editor.document().lines[fold_line].chars().count()));
                return (view_line + 1, column);
            }
            let (view_line, column) = editor.view_column(Position::new(line, column));
            (view_line + 1, column)
        };
        let cursor_selections = if editor.cursor_changed { editor.selections.as_slice() } else { &[] };
        let mut selections: Vec<MinimapRange> = cursor_selections.iter().map(|selection| MinimapRange { start: view(selection.start().line, selection.start().column), end: view(selection.end().line, selection.end().column) }).collect();
        selections.sort_by_key(|range| range.start);
        let mut decorations = Vec::new();
        if let Some(color) = self.theme.color("minimap.selectionOccurrenceHighlight") {
            for (line, columns) in &editor.highlights {
                decorations.push((MinimapRange { start: view(*line, columns.start), end: view(*line, columns.end) }, color));
            }
        }
        if let Some(color) = self.theme.color("editor.foldBackground") {
            for (range, decoration) in editor.folding.regions.ranges().iter().zip(editor.folding.decorations()) {
                if decoration == FoldDecoration::Collapsed {
                    let line = range.start - 1;
                    decorations.push((MinimapRange { start: view(line, 0), end: view(line, editor.document().lines[line].chars().count()) }, color));
                }
            }
        }
        decorations.sort_by_key(|(range, _)| range.start);
        // Find matches after the z-index 0 decorations above, the current match last (findDecorations.ts zIndex 10, 13).
        if let Some(color) = self.theme.color("minimap.findMatchHighlight") {
            for mark in self.find_marks(editor, self.editors.viewport.height).into_iter().filter(|mark| mark.in_minimap) {
                decorations.push((MinimapRange { start: view(mark.range.start.line, mark.range.start.column), end: view(mark.range.end.line, mark.range.end.column) }, color));
            }
        }
        // markerDecorationsService.ts: inline minimap colors at z-index 10, 20 and 30.
        for (start, end, color) in self.marker_minimap_ranges(editor) {
            decorations.push((MinimapRange { start: view(start.line, start.column), end: view(end.line, end.column) }, color));
        }
        // `_getMinimapDecorationsInViewport` and `getSelections` with sampling: the ranges move to minimap lines.
        if let Some(sampling) = sampling {
            for range in &mut selections {
                (range.start.0, range.end.0) = sampling.decoration_lines(range.start.0, range.end.0);
            }
            for (range, _) in &mut decorations {
                (range.start.0, range.end.0) = (sampling.minimap_line(range.start.0), sampling.minimap_line(range.end.0));
            }
        }
        let tab_size = editor.document().indentation.tab_size;
        let mut fills = decoration_fills(info, layout, self.theme.color("minimap.selectionHighlight"), &selections, &decorations, tab_size, |minimap_line| editor.view_line_text(sampled_view_line(sampling, minimap_line) - 1));
        // minimap.ts `_renderDecorationsHighlights` for `MinimapPosition.Gutter` (the quick diff's): 2px at x 2.
        for (start, end, color) in self.quick_diff_minimap_lines(editor) {
            let (mut first, mut last) = (view(start, 0).0, view(end, 0).0);
            if let Some(sampling) = sampling {
                (first, last) = sampling.decoration_lines(first, last);
            }
            let Some((first, last)) = layout.intersect_with_viewport(first, last) else { continue };
            for line in first..=last {
                fills.push(crate::editor::minimap::Fill { x: 2., y: layout.y_for_line_number(line, info.line_height), width: 2., height: info.line_height, color });
            }
        }
        fills
    }

    /// sectionHeaders.ts `SectionHeaderDetector` and `getSectionHeaderDecorationsInViewport`: the headers from a font
    /// size above the first line to the last.
    // shortcut: found again on the UI thread for each new model version, where upstream asks the editor worker 250ms
    // after the last edit and moves the old headers with the text meanwhile; upgrade when large files lag on typing.
    #[allow(clippy::too_many_arguments)]
    fn minimap_section_labels(&self, editor: &OpenEditor, info: &MinimapLayoutInfo, pixel_ratio: f64, layout: &MinimapLayout, options: &SectionHeaderOptions, sampling: Option<&MinimapSampling>) -> Vec<SectionLabel> {
        let key = (editor.model_id, editor.text_model().version_id, options.show_region, options.mark_regex.clone());
        let mut cached = self.minimap.section_headers.borrow_mut();
        let headers = match cached.as_ref().filter(|(cached_key, _)| *cached_key == key) {
            Some((_, headers)) => headers.clone(),
            None => {
                let headers = Rc::new(section_headers(editor, options));
                *cached = Some((key, headers.clone()));
                headers
            }
        };
        let header_lines = options.font_size * pixel_ratio / info.line_height;
        let first_line = (layout.start_line_number as f64 - header_lines).max(1.).floor() as usize;
        let mut labels: Vec<SectionLabel> = headers
            .iter()
            .map(|header| {
                let view_line = editor.view_line(header.start.line) + 1;
                let view_line = sampling.map_or(view_line, |sampling| sampling.minimap_line(view_line));
                SectionLabel { view_line, text: header.text.clone(), has_separator_line: header.has_separator_line }
            })
            .filter(|label| (first_line..=layout.end_line_number).contains(&label.view_line))
            .collect();
        labels.sort_by_key(|label| label.view_line);
        labels
    }

    /// `MinimapOptions._getMinimapBackground`: `minimap.background`, else the token color map's default background.
    fn minimap_background(&self) -> Rgba {
        self.theme.color("minimap.background").unwrap_or_else(|| Rgba::try_from(self.theme.tokens.color_map()[2].as_str()).expect("the token color map holds hex colors"))
    }

    /// The shadow left of the 6px strip: the strip's blurred box, outside the box itself.
    fn minimap_shadow(&self) -> gpui::Canvas<()> {
        canvas(
            |_, _, _| {},
            |bounds, _, window, _| {
                let scale = window.scale_factor();
                let columns = (f32::from(bounds.size.width) * scale) as usize;
                let erfc = |x: f32| super::editor_view::erfc(x / (SHADOW_SIGMA * std::f32::consts::SQRT_2));
                for column in 0..columns {
                    // Distance from the strip's left edge, leftwards.
                    let distance = f32::from(bounds.size.width) - (column as f32 + 0.5) / scale;
                    let alpha = SHADOW_ALPHA * 0.5 * (erfc(distance) - erfc(distance + SHADOW_STRIP_WIDTH));
                    let origin = bounds.origin + point(px(column as f32 / scale), px(0.));
                    window.paint_quad(fill(Bounds::new(origin, size(px(1. / scale), bounds.size.height)), Rgba { r: 0., g: 0., b: 0., a: alpha }));
                }
            },
        )
    }

    /// A press off the slider: `revealLineNumber` of the line under the pointer.
    #[allow(clippy::too_many_arguments)]
    fn press_minimap(&mut self, pointer: f32, info: MinimapLayoutInfo, pixel_ratio: f64, layout: MinimapLayout, sampling: Option<&MinimapSampling>, cx: &mut Context<Self>) {
        let offset = (pointer - self.minimap.top.get()) as f64;
        let line_index = (offset * pixel_ratio / info.line_height).floor() as usize;
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("the minimap shows only with an editor open");
        let line_count = sampling.map_or(editor.view_line_count(), |sampling| sampling.lines.len());
        let line = (line_index + layout.start_line_number).saturating_sub(layout.top_padding_line_count).min(line_count).max(1);
        editor.reveal_view_line_centered(sampled_view_line(sampling, line) - 1, viewport);
        self.editor_scrolled(cx);
    }

    /// The pointer entered or left the minimap: the slider, and with `autohide: mouseover` the minimap, fade.
    fn minimap_mouse_over_changed(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        self.minimap.update_slider(now);
        if self.minimap_settings().autohide == Autohide::Mouseover {
            self.minimap.autohide.set(self.minimap.mouse_over, now, AUTOHIDE_TRANSITION);
        }
        cx.notify();
    }

    fn start_minimap_drag(&mut self, pointer: f32, layout: MinimapLayout, cx: &mut Context<Self>) {
        let editor = self.editors.active().expect("the minimap shows only with an editor open");
        self.minimap.drag = Some(MinimapDrag { pointer, layout, scroll_top: editor.scroll.top });
        self.minimap.update_slider(Instant::now());
        cx.notify();
    }

    /// `handlePointerMove`: the slider follows the pointer; back where it was pressed, the editor's scroll is back too.
    fn drag_minimap_slider(&mut self, drag: MinimapDrag, pointer: f32, cx: &mut Context<Self>) {
        let delta = pointer - drag.pointer;
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("the minimap shows only with an editor open");
        let top = if delta == 0. { drag.scroll_top } else { drag.layout.desired_scroll_top_from_delta(delta as f64) as f32 };
        if editor.set_scroll(top, editor.scroll.left, viewport) {
            self.editor_scrolled(cx);
        }
    }

    /// `onScrollChanged` with `showSlider: mouseover`: a new scroll top shows the slider for 500ms.
    pub(super) fn minimap_scrolled(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        if editor.scroll.top == self.minimap.scroll_top {
            return;
        }
        self.minimap.scroll_top = editor.scroll.top;
        if self.minimap_settings().autohide == Autohide::Scroll {
            self.minimap.autohide.set(true, Instant::now(), AUTOHIDE_TRANSITION);
            self.minimap.active = Some(cx.spawn(async move |workbench, cx| {
                // `_hideSoon`: 500ms, again while the pointer is over the minimap.
                loop {
                    cx.background_executor().timer(MINIMAP_HIDE_DELAY).await;
                    let hidden = workbench.update(cx, |workbench, cx| {
                        if workbench.minimap.mouse_over {
                            return false;
                        }
                        workbench.minimap.active = None;
                        workbench.minimap.autohide.set(false, Instant::now(), AUTOHIDE_TRANSITION);
                        cx.notify();
                        true
                    });
                    if !matches!(hidden, Ok(false)) {
                        return;
                    }
                }
            }));
        }
        self.minimap.scrolling = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(SLIDER_SCROLLING_TIMEOUT).await;
            workbench
                .update(cx, |workbench, cx| {
                    workbench.minimap.scrolling = None;
                    workbench.minimap.update_slider(Instant::now());
                    cx.notify();
                })
                .ok();
        }));
        self.minimap.update_slider(Instant::now());
    }
}

type SectionHeaderKey = (u32, u32, bool, Option<String>);

/// The `editor.minimap` section header options as editorOptions.ts `EditorMinimap.validate` reads them.
pub(super) struct SectionHeaderOptions {
    show_region: bool,
    /// `markSectionHeaderRegex` when `showMarkSectionHeaders` is on; one that does not compile falls back to the default.
    mark_regex: Option<String>,
    font_size: f64,
    letter_spacing: f64,
}

impl Workbench {
    /// `EditorMinimap.validate`: an unknown value falls back to the default; `scale` and `maxColumn` are clamped.
    pub(super) fn minimap_settings(&self) -> MinimapSettings {
        let string = |key: &str| self.setting(&format!("editor.minimap.{key}")).as_str().map(str::to_owned);
        let number = |key: &str, default: f64, min: f64, max: f64| self.setting(&format!("editor.minimap.{key}")).as_f64().map_or(default, |value| value.floor().clamp(min, max));
        let defaults = MinimapOptions::default();
        let layout = MinimapOptions {
            size: match string("size").as_deref() {
                Some("fill") => MinimapSize::Fill,
                Some("fit") => MinimapSize::Fit,
                _ => MinimapSize::Proportional,
            },
            side: if string("side").as_deref() == Some("left") { MinimapSide::Left } else { MinimapSide::Right },
            render_characters: self.setting("editor.minimap.renderCharacters").as_bool().unwrap_or(defaults.render_characters),
            max_column: number("maxColumn", defaults.max_column, 1., 10000.),
            scale: number("scale", defaults.scale, 1., 3.),
        };
        MinimapSettings {
            enabled: self.setting("editor.minimap.enabled").as_bool().unwrap_or(true),
            layout,
            slider_always: string("showSlider").as_deref() == Some("always"),
            autohide: match string("autohide").as_deref() {
                Some("mouseover") => Autohide::Mouseover,
                Some("scroll") => Autohide::Scroll,
                _ => Autohide::None,
            },
        }
    }

    pub(super) fn section_header_options(&self) -> SectionHeaderOptions {
        let float = |key: &str, default: f64, min: f64, max: f64| self.setting(key).as_f64().unwrap_or(default).clamp(min, max);
        let mark_regex = self.setting("editor.minimap.markSectionHeaderRegex");
        let mark_regex = mark_regex.as_str().filter(|pattern| onig::Regex::new(pattern).is_ok()).unwrap_or(MARK_SECTION_HEADER_REGEX);
        SectionHeaderOptions {
            show_region: self.setting("editor.minimap.showRegionSectionHeaders") != json!(false),
            mark_regex: (self.setting("editor.minimap.showMarkSectionHeaders") != json!(false)).then(|| mark_regex.to_string()),
            font_size: float("editor.minimap.sectionHeaderFontSize", 9., 4., 32.),
            letter_spacing: float("editor.minimap.sectionHeaderLetterSpacing", 1., 0., 5.),
        }
    }
}

/// `findSectionHeaders` then `updateDecorations`: a mark header stays only when it starts in a comment token.
fn section_headers(editor: &OpenEditor, options: &SectionHeaderOptions) -> Vec<SectionHeader> {
    let document = &editor.document();
    let options = FindOptions {
        region_start: document.folding_markers.as_deref().map(|markers| &markers.start_with_flags).filter(|_| options.show_region),
        mark_regex: options.mark_regex.as_deref(),
        block_comment: document.block_comment.as_ref().map(|(start, end)| (start.as_str(), end.as_str())),
    };
    let mut headers = find_section_headers(&document.lines, &options);
    headers.retain(|header| {
        let text = &document.lines[header.start.line];
        let offset = text.char_indices().nth(header.start.column).map_or(text.len(), |(offset, _)| offset);
        !header.should_be_in_comments || document.tokens.token_type_at(&document.lines, header.start.line, offset) == textmate::StandardTokenType::Comment
    });
    headers
}

/// `_renderSectionHeaders`: each label on a 70% minimap background, in the `500` system font at `sectionHeaderFontSize`
/// with `sectionHeaderLetterSpacing` canvas pixels between letters; labels of nearby lines stack below one another.
/// Canvas pixels are `pixel_ratio` per point.
fn paint_section_labels(labels: &[SectionLabel], options: &SectionHeaderOptions, info: &MinimapLayoutInfo, pixel_ratio: f64, layout: &MinimapLayout, background: Rgba, foreground: Rgba, origin: Point<Pixels>, window: &mut Window) {
    let font_size = options.font_size * pixel_ratio;
    let background_height = font_size * 1.5;
    let label_width = info.canvas_inner_width;
    let point_at = |x: f64, y: f64| origin + point(px((x / pixel_ratio) as f32), px((y / pixel_ratio) as f32));
    let text_system = window.text_system().clone();
    let label_font = Font { weight: FontWeight::MEDIUM, ..font(".SystemUIFont") };
    let font_id = text_system.resolve_font(&label_font);
    let run = |len: usize| TextRun { len, font: label_font.clone(), color: foreground.into(), background_color: None, underline: None, strikethrough: None };
    let shape = |text: &str| text_system.shape_line(SharedString::from(text.to_string()), px(font_size as f32), &[run(text.len())], None);
    // Glyph advances, not CoreText line positions: Chrome's canvas skips the font's `trak` tracking.
    let advance = |text: &str| text.chars().map(|ch| f64::from(f32::from(text_system.advance(font_id, px(font_size as f32), ch).map_or_else(|_| shape(&ch.to_string()).width, |size| size.width))) + options.letter_spacing).sum::<f64>();
    let measure = |text: &str| advance(text);
    let mut next_label_y = f64::NEG_INFINITY;
    for label in labels {
        let text = fit_section_header(&label.text, label_width - MINIMAP_GUTTER_WIDTH, measure);
        let line_y = layout.y_for_line_number(label.view_line, info.line_height);
        let background_y = if text.is_empty() { line_y } else { line_y.max(next_label_y) };
        if !text.is_empty() {
            next_label_y = background_y + background_height;
            window.paint_quad(fill(Bounds::from_corners(point_at(0., background_y), point_at(label_width, background_y + background_height)), background));
            let shaped = shape(&text);
            let baseline = point_at(MINIMAP_GUTTER_WIDTH, background_y + font_size);
            for shaped_run in &shaped.runs {
                for glyph in &shaped_run.glyphs {
                    let x = advance(&text[..glyph.index]);
                    let glyph_origin = baseline + point(px((x / pixel_ratio) as f32), glyph.position.y / pixel_ratio as f32);
                    window.paint_glyph(glyph_origin, shaped_run.font_id, glyph.id, px(options.font_size as f32), foreground.into()).expect("the system font paints its glyphs");
                }
            }
        }
        // A 0.4 canvas pixel line through `backgroundFillY + 2`.
        if label.has_separator_line {
            let y = background_y + 2. - 0.2;
            window.paint_quad(fill(Bounds::from_corners(point_at(0., y), point_at(label_width, y + 0.4)), foreground));
        }
    }
}

fn byte(channel: f32) -> u8 {
    (channel * 255.).round() as u8
}

fn bytes(color: Rgba) -> [u8; 4] {
    [color.r, color.g, color.b, color.a].map(byte)
}

