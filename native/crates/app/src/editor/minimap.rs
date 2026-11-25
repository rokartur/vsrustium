//! The minimap: its layout (editorOptions.ts `_computeMinimapLayout`), its slider (minimap.ts `MinimapLayout`) and
//! its text, drawn a few pixels per character (minimapCharRenderer.ts with the minimapPreBaked.ts tables). Sizes are
//! CSS pixels; "canvas" sizes are device pixels.

use gpui::Rgba;

/// editorOptions.ts `MINIMAP_GUTTER_WIDTH`, in CSS pixels in the layout and canvas pixels when drawing.
pub const MINIMAP_GUTTER_WIDTH: f64 = 8.;
/// minimapCharSheet.ts `Constants`.
const START_CH_CODE: u32 = 32;
const END_CH_CODE: u32 = 126;
const CHAR_COUNT: u32 = END_CH_CODE - START_CH_CODE + 2;
const BASE_CHAR_HEIGHT: usize = 2;
const BASE_CHAR_WIDTH: usize = 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MinimapSize {
    Proportional,
    Fill,
    Fit,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MinimapSide {
    // Only the tests lay it out until `editor.minimap.side` is read.
    #[cfg_attr(not(test), allow(dead_code))]
    Left,
    Right,
}

/// The `editor.minimap.*` settings the layout reads, with their defaults.
// shortcut: the editor reads no settings yet, so only the defaults draw; the other sizes and sides are laid out
// but not drawn, and `fill`/`fit` sampling of long files is not ported. Upgrade with the settings milestone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinimapOptions {
    pub size: MinimapSize,
    pub side: MinimapSide,
    pub render_characters: bool,
    pub max_column: f64,
    pub scale: f64,
}

impl Default for MinimapOptions {
    fn default() -> Self {
        MinimapOptions { size: MinimapSize::Proportional, side: MinimapSide::Right, render_characters: true, max_column: 120., scale: 1. }
    }
}

pub struct MinimapLayoutInput {
    pub outer_width: f64,
    pub outer_height: f64,
    pub line_height: f64,
    pub typical_halfwidth_character_width: f64,
    pub pixel_ratio: f64,
    pub scroll_beyond_last_line: bool,
    pub padding_top: f64,
    pub padding_bottom: f64,
    pub vertical_scrollbar_width: f64,
    pub view_line_count: usize,
    /// The editor width right of the gutter.
    pub remaining_width: f64,
    pub options: MinimapOptions,
}

/// `EditorMinimapLayoutInfo` of an enabled minimap; `render_characters` is `RenderMinimap.Text`, else `Blocks`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinimapLayoutInfo {
    pub render_characters: bool,
    pub left: f64,
    pub width: f64,
    pub height_is_editor_height: bool,
    pub is_sampling: bool,
    pub scale: usize,
    pub line_height: f64,
    pub canvas_inner_width: f64,
    pub canvas_inner_height: f64,
    pub canvas_outer_width: f64,
    pub canvas_outer_height: f64,
}

/// `_computeMinimapScale`: at a pixel ratio of 2 and above the scale doubles, so it does not jump near 2.
fn minimap_scale(configured: f64, pixel_ratio: f64) -> f64 {
    if pixel_ratio >= 2. {
        return (configured * 2.).round();
    }
    configured.max((configured * pixel_ratio + 0.125).floor())
}

/// `_computeMinimapLayout` without viewport wrapping (the editor does not wrap).
pub fn compute_minimap_layout(input: &MinimapLayoutInput) -> MinimapLayoutInfo {
    let options = input.options;
    let pixel_ratio = input.pixel_ratio;
    let mut scale = minimap_scale(options.scale, pixel_ratio);
    let base_char_height = if options.render_characters { 2. } else { 3. };
    let mut canvas_inner_height = (pixel_ratio * input.outer_height).floor();
    let canvas_outer_height = canvas_inner_height / pixel_ratio;
    let mut height_is_editor_height = false;
    let mut is_sampling = false;
    let mut line_height = base_char_height * scale;
    let mut char_width = scale / pixel_ratio;
    let mut width_multiplier: f64 = 1.;
    if options.size != MinimapSize::Proportional {
        // computeContainedMinimapLineCount
        let view_line_count = input.view_line_count as f64;
        let typical_viewport_line_count = input.outer_height / input.line_height;
        let extra_lines_before = (input.padding_top / input.line_height).floor();
        let mut extra_lines_beyond = (input.padding_bottom / input.line_height).floor();
        if input.scroll_beyond_last_line {
            extra_lines_beyond = extra_lines_beyond.max(typical_viewport_line_count - 1.);
        }
        let desired_ratio = (extra_lines_before + view_line_count + extra_lines_beyond) / (pixel_ratio * input.outer_height);
        let minimap_line_count = (view_line_count / desired_ratio).floor();
        if view_line_count / minimap_line_count > 1. {
            height_is_editor_height = true;
            is_sampling = true;
            scale = 1.;
            line_height = 1.;
            char_width = scale / pixel_ratio;
        } else {
            let max_scale = scale + 1.;
            let fit_becomes_fill = options.size == MinimapSize::Fit
                && ((extra_lines_before + view_line_count + extra_lines_beyond) * line_height).ceil() > canvas_inner_height;
            if options.size == MinimapSize::Fill || fit_becomes_fill {
                height_is_editor_height = true;
                let configured_scale = scale;
                line_height = (input.line_height * pixel_ratio).min((1. / desired_ratio).floor().max(1.));
                scale = max_scale.min((line_height / base_char_height).floor().max(1.));
                if scale > configured_scale {
                    width_multiplier = 2f64.min(scale / configured_scale);
                }
                char_width = scale / pixel_ratio / width_multiplier;
                canvas_inner_height = (typical_viewport_line_count.max(extra_lines_before + view_line_count + extra_lines_beyond) * line_height).ceil();
            }
        }
    }
    let max_width = (options.max_column * char_width).floor();
    let fitting_width = (((input.remaining_width - input.vertical_scrollbar_width - 2.) * char_width) / (input.typical_halfwidth_character_width + char_width)).floor();
    let width = max_width.min(fitting_width.max(0.) + MINIMAP_GUTTER_WIDTH);
    let canvas_inner_width = (pixel_ratio * width).floor();
    let canvas_outer_width = canvas_inner_width / pixel_ratio;
    let left = match options.side {
        MinimapSide::Left => 0.,
        MinimapSide::Right => input.outer_width - width - input.vertical_scrollbar_width,
    };
    MinimapLayoutInfo {
        render_characters: options.render_characters,
        left,
        width,
        height_is_editor_height,
        is_sampling,
        scale: scale as usize,
        line_height,
        canvas_inner_width: (canvas_inner_width * width_multiplier).floor(),
        canvas_inner_height,
        canvas_outer_width,
        canvas_outer_height,
    }
}

/// The options `MinimapLayout.create` reads.
pub struct SliderOptions {
    pub pixel_ratio: f64,
    pub minimap_line_height: f64,
    pub canvas_inner_height: f64,
    pub line_height: f64,
    pub height_is_editor_height: bool,
    pub minimap_height: f64,
    pub padding_top: f64,
    pub padding_bottom: f64,
    pub scroll_beyond_last_line: bool,
}

/// minimap.ts `MinimapLayout`: the slider and the lines drawn, 1-based, for a scroll position. Lines all have
/// `line_height` and there are no view zones, so the zone-free scroll position is the scroll position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinimapLayout {
    pub scroll_top: f64,
    pub scroll_height: f64,
    pub slider_needed: bool,
    computed_slider_ratio: f64,
    pub slider_top: f64,
    pub slider_height: f64,
    pub top_padding_line_count: usize,
    pub start_line_number: usize,
    pub end_line_number: usize,
}

impl MinimapLayout {
    pub fn desired_scroll_top_from_delta(&self, delta: f64) -> f64 {
        (self.scroll_top + delta / self.computed_slider_ratio).round()
    }

    pub fn intersect_with_viewport(&self, start_line_number: usize, end_line_number: usize) -> Option<(usize, usize)> {
        let start = self.start_line_number.max(start_line_number);
        let end = self.end_line_number.min(end_line_number);
        (start <= end).then_some((start, end))
    }

    pub fn y_for_line_number(&self, line_number: usize, minimap_line_height: f64) -> f64 {
        (line_number as f64 - self.start_line_number as f64 + self.top_padding_line_count as f64) * minimap_line_height
    }

    /// `MinimapLayout.create` for a viewport whose first, partly shown line is `viewport_start_line_number`.
    pub fn create(options: &SliderOptions, viewport_start_line_number: usize, viewport_height: f64, line_count: usize, scroll_top: f64, scroll_height: f64, previous: Option<&MinimapLayout>) -> MinimapLayout {
        let pixel_ratio = options.pixel_ratio;
        let minimap_line_height = options.minimap_line_height;
        let minimap_lines_fitting = (options.canvas_inner_height / minimap_line_height).floor();
        let line_height = options.line_height;
        let lines = line_count as f64;
        if options.height_is_editor_height {
            let mut logical_scroll_height = lines * line_height + options.padding_top + options.padding_bottom;
            if options.scroll_beyond_last_line {
                logical_scroll_height += (viewport_height - line_height - options.padding_bottom).max(0.);
            }
            let slider_height = (viewport_height * viewport_height / logical_scroll_height).floor().max(1.);
            let max_slider_top = (options.minimap_height - slider_height).max(0.);
            let computed_slider_ratio = max_slider_top / (scroll_height - viewport_height);
            let top_padding_line_count = (options.padding_top / line_height).floor() as usize;
            return MinimapLayout {
                scroll_top,
                scroll_height,
                slider_needed: max_slider_top > 0.,
                computed_slider_ratio,
                slider_top: scroll_top * computed_slider_ratio,
                slider_height,
                top_padding_line_count,
                start_line_number: 1,
                end_line_number: line_count.min(minimap_lines_fitting as usize),
            };
        }
        let expected_viewport_line_count = viewport_height / line_height;
        let slider_height = (expected_viewport_line_count * minimap_line_height / pixel_ratio).floor();
        let extra_lines_at_the_top = (options.padding_top / line_height).floor();
        let mut extra_lines_at_the_bottom = (options.padding_bottom / line_height).floor();
        if options.scroll_beyond_last_line {
            extra_lines_at_the_bottom = extra_lines_at_the_bottom.max(expected_viewport_line_count - 1.);
        }
        let mut max_slider_top = if extra_lines_at_the_bottom > 0. {
            (extra_lines_at_the_top + lines + extra_lines_at_the_bottom - expected_viewport_line_count - 1.) * minimap_line_height / pixel_ratio
        } else {
            ((extra_lines_at_the_top + lines) * minimap_line_height / pixel_ratio - slider_height).max(0.)
        };
        max_slider_top = max_slider_top.min(options.minimap_height - slider_height);
        let computed_slider_ratio = max_slider_top / (scroll_height - viewport_height);
        let slider_top = scroll_top * computed_slider_ratio;
        if minimap_lines_fitting >= extra_lines_at_the_top + lines + extra_lines_at_the_bottom {
            return MinimapLayout {
                scroll_top,
                scroll_height,
                slider_needed: max_slider_top > 0.,
                computed_slider_ratio,
                slider_top,
                slider_height,
                top_padding_line_count: extra_lines_at_the_top as usize,
                start_line_number: 1,
                end_line_number: line_count,
            };
        }
        let considering_start_line_number = match viewport_start_line_number > 1 {
            true => viewport_start_line_number as f64 + extra_lines_at_the_top,
            false => (scroll_top / line_height).max(1.),
        };
        let mut start_line_number = (considering_start_line_number - slider_top * pixel_ratio / minimap_line_height).floor().max(1.);
        let mut top_padding_line_count;
        if start_line_number < extra_lines_at_the_top {
            top_padding_line_count = extra_lines_at_the_top - start_line_number + 1.;
            start_line_number = 1.;
        } else {
            top_padding_line_count = 0.;
            start_line_number = (start_line_number - extra_lines_at_the_top).max(1.);
        }
        // Keep the drawn lines from jumping back while the scroll position only moves one way.
        if let Some(previous) = previous.filter(|previous| previous.scroll_height == scroll_height) {
            if previous.scroll_top > scroll_top {
                start_line_number = start_line_number.min(previous.start_line_number as f64);
                top_padding_line_count = top_padding_line_count.max(previous.top_padding_line_count as f64);
            }
            if previous.scroll_top < scroll_top {
                start_line_number = start_line_number.max(previous.start_line_number as f64);
                top_padding_line_count = top_padding_line_count.min(previous.top_padding_line_count as f64);
            }
        }
        let end_line_number = lines.min(start_line_number - top_padding_line_count + minimap_lines_fitting - 1.);
        let viewport_start_top = (viewport_start_line_number as f64 - 1.) * line_height + options.padding_top;
        let partial_line = (scroll_top - viewport_start_top).max(0.).min(line_height) / line_height;
        let slider_top_aligned = if scroll_top >= options.padding_top {
            (viewport_start_line_number as f64 - start_line_number + top_padding_line_count + partial_line) * minimap_line_height / pixel_ratio
        } else {
            (scroll_top / options.padding_top) * (top_padding_line_count + partial_line) * minimap_line_height / pixel_ratio
        };
        MinimapLayout {
            scroll_top,
            scroll_height,
            slider_needed: true,
            computed_slider_ratio,
            slider_top: slider_top_aligned,
            slider_height,
            top_padding_line_count: top_padding_line_count as usize,
            start_line_number: start_line_number as usize,
            end_line_number: end_line_number as usize,
        }
    }
}

/// An RGBA pixel buffer, `ImageData` of the minimap canvas.
pub struct MinimapImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl MinimapImage {
    /// `MinimapBuffers`: a buffer filled with the background.
    pub fn new(width: usize, height: usize, background: [u8; 4]) -> Self {
        MinimapImage { width, height, data: background.repeat(width * height) }
    }
}

/// `Uint8ClampedArray` assignment: clamped, rounded half to even.
fn clamped(value: f64) -> u8 {
    value.clamp(0., 255.).round_ties_even() as u8
}

/// minimapCharSheet.ts `getCharIndex`: outside ASCII, small scales reuse an ASCII shape, larger ones a box.
fn char_index(code: u32, font_scale: usize) -> usize {
    let code = code as i64 - START_CH_CODE as i64;
    if code < 0 || code > CHAR_COUNT as i64 {
        if font_scale <= 2 {
            return (code + CHAR_COUNT as i64).rem_euclid(CHAR_COUNT as i64) as usize;
        }
        return CHAR_COUNT as usize - 1;
    }
    code as usize
}

/// strings.ts `isFullWidthCharacter`: the ranges the minimap draws two columns wide.
pub fn is_full_width_character(code: u32) -> bool {
    (0x2E80..=0xD7AF).contains(&code) || (0xF900..=0xFAFF).contains(&code) || (0xFF01..=0xFF5E).contains(&code) || (0xFFE0..=0xFFE6).contains(&code)
}

/// minimapCharRenderer.ts `MinimapCharRenderer`: alpha masks of the ASCII characters, softened for dark and light backgrounds.
pub struct CharRenderer {
    scale: usize,
    normal: Vec<u8>,
    light: Vec<u8>,
}

impl CharRenderer {
    /// `MinimapCharRendererFactory.create` for the scales with a prebaked table (minimapPreBaked.ts), the ones the
    /// default `editor.minimap.scale` gives at any pixel ratio.
    pub fn prebaked(scale: usize) -> Self {
        let hex = match scale {
            1 => PREBAKED_SCALE_1,
            2 => PREBAKED_SCALE_2,
            _ => unimplemented!("minimap scale {scale} samples the font, which needs the minimap scale setting"),
        };
        let data = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("prebaked hex")).collect();
        CharRenderer::new(data, scale)
    }

    fn new(data: Vec<u8>, scale: usize) -> Self {
        let soften = |ratio: f64| data.iter().map(|&value| (value as f64 * ratio).clamp(0., 255.) as u8).collect();
        CharRenderer { scale, normal: soften(12. / 15.), light: soften(50. / 60.) }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_char(&self, target: &mut MinimapImage, dx: usize, dy: usize, code: u32, color: [u8; 3], foreground_alpha: u8, background: [u8; 3], background_alpha: u8, font_scale: usize, use_lighter_font: bool, force_1px_height: bool) {
        let char_width = BASE_CHAR_WIDTH * self.scale;
        let char_height = BASE_CHAR_HEIGHT * self.scale;
        let render_height = if force_1px_height { 1 } else { char_height };
        if dx + char_width > target.width || dy + render_height > target.height {
            return;
        }
        let char_data = if use_lighter_font { &self.light } else { &self.normal };
        let mut source = char_index(code, font_scale) * char_width * char_height;
        let dest_alpha = foreground_alpha.max(background_alpha);
        for y in 0..render_height {
            let mut column = ((dy + y) * target.width + dx) * 4;
            for _ in 0..char_width {
                let c = (char_data[source] as f64 / 255.) * (foreground_alpha as f64 / 255.);
                source += 1;
                for channel in 0..3 {
                    target.data[column + channel] = clamped(background[channel] as f64 + (color[channel] as f64 - background[channel] as f64) * c);
                }
                target.data[column + 3] = dest_alpha;
                column += 4;
            }
        }
    }

    /// `blockRenderChar`, for `editor.minimap.renderCharacters` off: a half-strength block.
    #[allow(clippy::too_many_arguments)]
    pub fn block_render_char(&self, target: &mut MinimapImage, dx: usize, dy: usize, color: [u8; 3], foreground_alpha: u8, background: [u8; 3], background_alpha: u8, force_1px_height: bool) {
        let char_width = BASE_CHAR_WIDTH * self.scale;
        let char_height = BASE_CHAR_HEIGHT * self.scale;
        let render_height = if force_1px_height { 1 } else { char_height };
        if dx + char_width > target.width || dy + render_height > target.height {
            return;
        }
        let c = 0.5 * (foreground_alpha as f64 / 255.);
        let pixel: Vec<u8> = (0..3).map(|channel| clamped(background[channel] as f64 + (color[channel] as f64 - background[channel] as f64) * c)).collect();
        let dest_alpha = foreground_alpha.max(background_alpha);
        for y in 0..render_height {
            let mut column = ((dy + y) * target.width + dx) * 4;
            for _ in 0..char_width {
                target.data[column..column + 3].copy_from_slice(&pixel);
                target.data[column + 3] = dest_alpha;
                column += 4;
            }
        }
    }
}

/// What `InnerMinimap._renderLine` reads besides the line.
pub struct LineStyle<'a> {
    pub renderer: &'a CharRenderer,
    pub render_characters: bool,
    pub background: [u8; 3],
    pub background_alpha: u8,
    pub foreground_alpha: u8,
    pub use_lighter_font: bool,
    pub font_scale: usize,
    pub char_width: usize,
    pub minimap_line_height: f64,
    pub tab_size: usize,
}

/// `InnerMinimap._renderLine`: the characters of `text` from the gutter on, each in the color of the run its byte
/// offset falls in (`colors`: run end offsets with their colors), cut at the canvas edge.
pub fn render_line(target: &mut MinimapImage, style: &LineStyle, dy: usize, text: &str, colors: &[(usize, [u8; 3])]) {
    let char_width = style.char_width;
    let max_dx = target.width.saturating_sub(char_width);
    let force_1px_height = style.minimap_line_height == 1.;
    let mut dx = MINIMAP_GUTTER_WIDTH as usize;
    let mut run = 0;
    // Columns count UTF-16 code units, as the reference's char codes do.
    let mut column = 0;
    let mut tabs_char_delta = 0;
    for (offset, c) in text.char_indices() {
        while run + 1 < colors.len() && colors[run].0 <= offset {
            run += 1;
        }
        let color = colors[run].1;
        for unit in c.encode_utf16(&mut [0; 2]).iter().map(|&unit| unit as u32) {
            if dx > max_dx {
                return;
            }
            if c == '\t' {
                let insert_spaces_count = style.tab_size - (column + tabs_char_delta) % style.tab_size;
                tabs_char_delta += insert_spaces_count - 1;
                dx += insert_spaces_count * char_width;
            } else if c == ' ' {
                dx += char_width;
            } else {
                let count = if is_full_width_character(unit) { 2 } else { 1 };
                for _ in 0..count {
                    match style.render_characters {
                        true => style.renderer.render_char(target, dx, dy, unit, color, style.foreground_alpha, style.background, style.background_alpha, style.font_scale, style.use_lighter_font, force_1px_height),
                        false => style.renderer.block_render_char(target, dx, dy, color, style.foreground_alpha, style.background, style.background_alpha, force_1px_height),
                    }
                    dx += char_width;
                    if dx > max_dx {
                        return;
                    }
                }
            }
            column += 1;
        }
    }
}

/// `InnerMinimap.getXOffsetForPosition`: the canvas x of `column` (0-based, in chars) of `text`.
pub fn x_offset_for_column(text: &str, column: usize, tab_size: usize, char_width: f64, canvas_inner_width: f64) -> f64 {
    let mut x = MINIMAP_GUTTER_WIDTH;
    if column == 0 {
        return x;
    }
    if column as f64 * char_width >= canvas_inner_width {
        return canvas_inner_width;
    }
    for c in text.chars().take(column) {
        x += match c {
            '\t' => tab_size as f64 * char_width,
            _ if is_full_width_character(c as u32) => 2. * char_width,
            _ => char_width,
        };
        if x >= canvas_inner_width {
            return canvas_inner_width;
        }
    }
    if column > text.chars().count() {
        return canvas_inner_width;
    }
    x
}

/// What `MinimapOptions` reads from the theme for the text canvas.
pub struct LinesColors {
    /// `minimap.background`, else the editor background.
    pub background: [u8; 4],
    /// The editor background (`ColorId.DefaultBackground`).
    pub default_background: [u8; 3],
    /// The alpha of `minimap.foregroundOpacity`.
    pub foreground_alpha: u8,
    /// `MinimapTokensColorTracker.backgroundIsLight`: the editor background's relative luminance is 0.5 or more.
    pub use_lighter_font: bool,
}

/// `InnerMinimap.renderLines` drawn whole, without reusing the rows of the last frame: the lines of `layout`, 1-based
/// view lines, below `top_padding_line_count` empty rows. `line` gives a line's text and its color runs.
pub fn render_lines<'a>(info: &MinimapLayoutInfo, layout: &MinimapLayout, renderer: &CharRenderer, colors: &LinesColors, tab_size: usize, line: impl Fn(usize) -> (std::borrow::Cow<'a, str>, Vec<(usize, [u8; 3])>)) -> MinimapImage {
    let mut image = MinimapImage::new(info.canvas_inner_width as usize, info.canvas_inner_height as usize, colors.background);
    let base_char_height = if info.render_characters { BASE_CHAR_HEIGHT } else { BASE_CHAR_HEIGHT + 1 };
    let render_line_height = (base_char_height * info.scale) as f64;
    let inner_line_padding = if info.line_height > render_line_height { ((info.line_height - render_line_height) / 2.).floor() as usize } else { 0 };
    let alpha = colors.background[3] as f64 / 255.;
    let render_background = [0, 1, 2].map(|channel| {
        let default = colors.default_background[channel] as f64;
        ((colors.background[channel] as f64 - default) * alpha + default).round() as u8
    });
    let style = LineStyle {
        renderer,
        render_characters: info.render_characters,
        background: render_background,
        background_alpha: colors.background[3],
        foreground_alpha: colors.foreground_alpha,
        use_lighter_font: colors.use_lighter_font,
        font_scale: info.scale,
        char_width: BASE_CHAR_WIDTH * info.scale,
        minimap_line_height: info.line_height,
        tab_size,
    };
    let line_height = info.line_height as usize;
    let mut dy = layout.top_padding_line_count * line_height;
    for line_number in layout.start_line_number..=layout.end_line_number {
        let (text, runs) = line(line_number);
        render_line(&mut image, &style, dy + inner_line_padding, &text, &runs);
        dy += line_height;
    }
    image
}

/// A range of view lines (1-based) and char columns (0-based), start before end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinimapRange {
    pub start: (usize, usize),
    pub end: (usize, usize),
}

/// A `fillRect` on the decorations canvas, in canvas pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fill {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub color: Rgba,
}

/// `InnerMinimap.renderDecorations` for `MinimapPosition.Inline` decorations: one half-transparent line highlight
/// per line, the selection's first, then the selections and decorations over their columns. `selections` sort by
/// start, `decorations` by z-index; `line` gives a view line's text.
#[allow(clippy::too_many_arguments)]
pub fn decoration_fills<'a>(info: &MinimapLayoutInfo, layout: &MinimapLayout, selection_color: Option<Rgba>, selections: &[MinimapRange], decorations: &[(MinimapRange, Rgba)], tab_size: usize, line: impl Fn(usize) -> std::borrow::Cow<'a, str>) -> Vec<Fill> {
    let selection_color = selection_color.filter(|color| color.a > 0.);
    let line_height = info.line_height;
    let canvas_width = info.canvas_inner_width;
    let half = |color: Rgba| Rgba { a: color.a * 0.5, ..color };
    let mut fills = Vec::new();
    let mut highlighted = Vec::new();
    // `_renderSelectionLineHighlights`: from the first line's top to the last line's top, merged when they touch.
    if let Some(color) = selection_color {
        let (mut y1, mut y2) = (0., 0.);
        for selection in selections {
            let Some((start, end)) = layout.intersect_with_viewport(selection.start.0, selection.end.0) else { continue };
            highlighted.extend(start..=end);
            let (yy1, yy2) = (layout.y_for_line_number(start, line_height), layout.y_for_line_number(end, line_height));
            if y2 >= yy1 {
                y2 = yy2;
                continue;
            }
            if y2 > y1 {
                fills.push(Fill { x: MINIMAP_GUTTER_WIDTH, y: y1, width: canvas_width, height: y2 - y1, color: half(color) });
            }
            (y1, y2) = (yy1, yy2);
        }
        if y2 > y1 {
            fills.push(Fill { x: MINIMAP_GUTTER_WIDTH, y: y1, width: canvas_width, height: y2 - y1, color: half(color) });
        }
    }
    // `_renderDecorationsLineHighlights`: backwards, so the decoration with the higher z-index colors the line.
    for (range, color) in decorations.iter().rev().filter(|(_, color)| color.a > 0.) {
        let Some((start, end)) = layout.intersect_with_viewport(range.start.0, range.end.0) else { continue };
        for line_number in start..=end {
            if highlighted.contains(&line_number) {
                continue;
            }
            highlighted.push(line_number);
            fills.push(Fill { x: MINIMAP_GUTTER_WIDTH, y: layout.y_for_line_number(line_number, line_height), width: canvas_width, height: line_height, color: half(*color) });
        }
    }
    let on_line = |fills: &mut Vec<Fill>, range: &MinimapRange, color: Rgba, line_number: usize| {
        let y = layout.y_for_line_number(line_number, line_height);
        if y + line_height < 0. || y > info.canvas_inner_height {
            return;
        }
        let text = line(line_number);
        let start_column = if range.start.0 == line_number { range.start.1 } else { 0 };
        let end_column = if range.end.0 == line_number { range.end.1 } else { text.chars().count() };
        let char_width = (BASE_CHAR_WIDTH * info.scale) as f64;
        let x1 = x_offset_for_column(&text, start_column, tab_size, char_width, canvas_width);
        let x2 = x_offset_for_column(&text, end_column, tab_size, char_width, canvas_width);
        fills.push(Fill { x: x1, y, width: x2 - x1, height: line_height, color });
    };
    let colored = selections.iter().filter_map(|range| selection_color.map(|color| (range, color))).chain(decorations.iter().filter(|(_, color)| color.a > 0.).map(|(range, color)| (range, *color)));
    for (range, color) in colored {
        let Some((start, end)) = layout.intersect_with_viewport(range.start.0, range.end.0) else { continue };
        for line_number in start..=end {
            on_line(&mut fills, range, color, line_number);
        }
    }
    fills
}

const PREBAKED_SCALE_1: &str = "0000511D6300CF609C709645A78432005642574171487021003C451900274D35D762755E8B629C5BA856AF57BA649530C167D1512A272A3F6038604460398526BCA2A968DB6F8957C768BE5FBE2FB467CF5D8D5B795DC7625B5DFF50DE64C466DB2FC47CD860A65E9A2EB96CB54CE06DA763AB2EA26860524D3763536601005116008177A8705E53AB738E6A982F88BAA35B5F5B626D9C636B449B737E5B7B678598869A662F6B5B8542706C704C80736A607578685B70594A49715A4522E792";
const PREBAKED_SCALE_2: &str = "000000000000000055394F383D2800008B8B1F210002000081B1CBCBCC820000847AAF6B9AAF2119BE08B8881AD60000A44FD07DCCF107015338130C00000000385972265F390B406E2437634B4B48031B12B8A0847000001E15B29A402F0000000000004B33460B00007A752C2A0000000000004D3900000084394B82013400ABA5CFC7AD9C0302A45A3E5A98AB000089A43382D97900008BA54AA087A70A0248A6A7AE6DBE0000BF6F94987EA40A01A06DCFA7A7A9030496C32F77891D0000A99FB1A0AFA80603B29AB9CA75930D010C0948354D3900000C0948354F37460D0028BE673D8400000000AF9D7B6E00002B007AA8933400007AA642675C2700007984CFB9C3985B768772A8A6B7B20000CAAECAAFC4B700009F94A6009F840009D09F9BA4CA9C0000CC8FC76DC87F0000C991C472A2000000A894A48CA7B501079BA2C9C69BA20000B19A5D3FA89000005CA6009DA2960901B0A7F0669FB200009D009E00B7890000DAD0F5D092820000D294D4C48BD10000B5A7A4A3B1A50402CAB6CBA6A2000000B5A7A4A3B1A8044FCDADD19D9CB00000B7778F7B8AAE0803C9AB5D3F5D3F00009EA09EA0BAB006039EA0989A8C7900009B9EF4D6B7C00000A9A7816CACA80000ABAC84705D3F000096DA635CDC8C00006F486F266F263D4784006124097B00374F6D2D6D2D6D4A3A95872322000000030000000000008D8939130000000000002E22A5C9CBC70600AB25C0B5C9B400061A2DB04CA67001082AA6BEBEBFC606002321DACBC19E03087AA08B6768380000282FBAC0B8CA7A88AD25BBA5A29900004C396C5894A6000040485A6E356E9442A32CD17EADA70000B4237923628600003E2DE9C1D7B500002F25BBA5A2990000231DB6AFB4A804023025C0B5CAB588062B2CBDBEC0C706882435A75CA20000002326BD6A82A908048B4B9A5A668000002423A09CB4BB060025259C9D8A7900001C1FCAB2C7C700002A2A9387ABA200002626A4A47D6E9D14333163A0C87500004B6F9C2D643A257049364936493647358A34438355497F1A0000A24C1D590000D38DFFBDD4CD3126";

#[cfg(test)]
mod tests {
    use super::*;

    /// editorLayoutProvider.test.ts `minimapInput`, without line numbers, glyph margin or scrollbars.
    fn layout(view_line_count: usize, pixel_ratio: f64, side: MinimapSide, size: MinimapSize, scale: f64) -> MinimapLayoutInfo {
        compute_minimap_layout(&MinimapLayoutInput {
            outer_width: 1000.,
            outer_height: 800.,
            line_height: 16.,
            typical_halfwidth_character_width: 10.,
            pixel_ratio,
            scroll_beyond_last_line: false,
            padding_top: 0.,
            padding_bottom: 0.,
            vertical_scrollbar_width: 0.,
            view_line_count,
            remaining_width: 990.,
            options: MinimapOptions { size, side, render_characters: true, max_column: 150., scale },
        })
    }

    fn info(left: f64, width: f64, height_is_editor_height: bool, is_sampling: bool, scale: usize, line_height: f64, canvas_inner: (f64, f64)) -> MinimapLayoutInfo {
        MinimapLayoutInfo {
            render_characters: true,
            left,
            width,
            height_is_editor_height,
            is_sampling,
            scale,
            line_height,
            canvas_inner_width: canvas_inner.0,
            canvas_inner_height: canvas_inner.1,
            canvas_outer_width: width,
            canvas_outer_height: 800.,
        }
    }

    #[test]
    fn editor_layout_provider_minimap_cases() {
        use MinimapSide::*;
        use MinimapSize::*;
        assert_eq!(layout(9, 1., Right, Proportional, 1.), info(903., 97., false, false, 1, 2., (97., 800.)), "9 - render minimap");
        assert_eq!(layout(9, 2., Right, Proportional, 1.), info(903., 97., false, false, 2, 4., (194., 1600.)), "9 - pixelRatio = 2");
        assert_eq!(layout(9, 4., Right, Proportional, 1.), info(945., 55., false, false, 2, 4., (220., 3200.)), "9 - pixelRatio = 4");
        assert_eq!(layout(9, 4., Left, Proportional, 1.), info(0., 55., false, false, 2, 4., (220., 3200.)), "10 - minimap to left");
        assert_eq!(layout(120, 2., Right, Fill, 1.), info(903., 97., true, false, 3, 13., (291., 1560.)), "11 - cover without sampling");
        assert_eq!(layout(2500, 2., Right, Fill, 1.), info(945., 55., true, true, 1, 1., (110., 1600.)), "12 - cover with sampling");
        assert_eq!(layout(120, 2., Right, Fit, 1.), info(903., 97., false, false, 2, 4., (194., 1600.)), "13 - contain without sampling");
        assert_eq!(layout(2500, 2., Right, Fit, 1.), info(945., 55., true, true, 1, 1., (110., 1600.)), "14 - contain with sampling");
    }

    #[test]
    fn issue_236999_minimap_scale_does_not_jump_when_the_pixel_ratio_crosses_2() {
        let pixel_ratios = [1., 1.25, 1.5, 1.75, 1.96, 1.9999998, 2., 2.5];
        let actual: Vec<Vec<usize>> = [1., 2., 3.].iter().map(|&scale| pixel_ratios.iter().map(|&ratio| layout(9, ratio, MinimapSide::Right, MinimapSize::Proportional, scale).scale).collect()).collect();
        assert_eq!(actual, [[1, 1, 1, 1, 2, 2, 2, 2], [2, 2, 3, 3, 4, 4, 4, 4], [3, 3, 4, 5, 6, 6, 6, 6]]);
    }

    /// minimapLayout.test.ts `scrollDown` over the document without view zones (the editor has none).
    #[test]
    fn slider_moves_only_forward_when_scrolling_down() {
        let (line_count, line_height, viewport_height, minimap_line_height) = (1000, 20., 400., 2.);
        let options = SliderOptions { pixel_ratio: 1., minimap_line_height, canvas_inner_height: 400., line_height, height_is_editor_height: false, minimap_height: 400., padding_top: 0., padding_bottom: 0., scroll_beyond_last_line: false };
        let scroll_height = line_count as f64 * line_height;
        let (mut previous, mut previous_slider_line, mut backward_moves, mut min_slider_top, mut max_slider_bottom) = (None::<MinimapLayout>, 0., 0, f64::MAX, 0.);
        let mut scroll_top = 0.;
        while scroll_top <= scroll_height - viewport_height {
            let start_line = (scroll_top / line_height).floor() as usize + 1;
            let layout = MinimapLayout::create(&options, start_line, viewport_height, line_count, scroll_top, scroll_height, previous.as_ref());
            let slider_line = layout.start_line_number as f64 - layout.top_padding_line_count as f64 + layout.slider_top / minimap_line_height;
            if previous.is_some() && slider_line < previous_slider_line - 1e-9 {
                backward_moves += 1;
            }
            min_slider_top = min_slider_top.min(layout.slider_top);
            max_slider_bottom = f64::max(max_slider_bottom, (layout.slider_top + layout.slider_height).round());
            previous = Some(layout);
            previous_slider_line = slider_line;
            scroll_top += 10.;
        }
        assert_eq!((backward_moves, min_slider_top, max_slider_bottom), (0, 0., 401.));
    }

    /// minimapCharRenderer.test.ts draws a sampled `d`; the port draws the prebaked tables, so this checks the
    /// prebaked `d` the same way: its alpha mask blended from the background to the color.
    #[test]
    fn letter_d_from_the_prebaked_tables() {
        for scale in [1, 2] {
            let renderer = CharRenderer::prebaked(scale);
            assert_eq!(renderer.normal.len(), CHAR_COUNT as usize * scale * scale * BASE_CHAR_HEIGHT);
            let mut image = MinimapImage::new(scale, 2 * scale, [0, 0, 0, 255]);
            renderer.render_char(&mut image, 0, 0, 'd' as u32, [255, 255, 255], 255, [0, 0, 0], 255, scale, false, false);
            let offset = ('d' as usize - 32) * scale * scale * 2;
            let expected: Vec<u8> = renderer.normal[offset..offset + 2 * scale * scale].iter().flat_map(|&a| [a, a, a, 255]).collect();
            assert_eq!(image.data, expected);
        }
    }

    #[test]
    fn char_index_maps_unknown_characters() {
        assert_eq!(char_index('a' as u32, 1), 65);
        assert_eq!(char_index(0x4E00, 1), (0x4E00 - 32 + 96) % 96);
        assert_eq!(char_index(0x4E00, 3), 95);
        assert_eq!(char_index(9, 1), 73);
    }

    #[test]
    fn render_line_skips_whitespace_and_expands_tabs() {
        let renderer = CharRenderer::prebaked(1);
        let style = LineStyle { renderer: &renderer, render_characters: false, background: [0, 0, 0], background_alpha: 255, foreground_alpha: 255, use_lighter_font: false, font_scale: 1, char_width: 1, minimap_line_height: 2., tab_size: 4 };
        let mut image = MinimapImage::new(20, 2, [0, 0, 0, 255]);
        render_line(&mut image, &style, 0, "\ta b", &[(4, [200, 100, 50])]);
        let painted: Vec<usize> = (0..20).filter(|x| image.data[x * 4] != 0).collect();
        assert_eq!(painted, [12, 14]);
        assert_eq!(&image.data[48..52], &[100, 50, 25, 255]);
    }
}
