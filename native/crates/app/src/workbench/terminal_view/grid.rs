//! The xterm.js WebGL renderer's output for one terminal: the cell grid with its backgrounds, selection, text,
//! decorations and cursor, and the overlay scrollbar of xterm's viewport.

use std::collections::HashMap;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Side};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use gpui::{BoxShadow, Corners, Bounds, Font, FontStyle, FontWeight, Hsla, Pixels, Rgba, SharedString, StrikethroughStyle, TextRun, UnderlineStyle, Window, fill, font, outline, point, px, size};

use crate::terminal::TerminalInstance;
use crate::terminal::sticky_scroll::StickyScroll;
use crate::terminal::colors::{TerminalColors, ensure_contrast_ratio};

/// xtermTerminal.ts `TerminalScrollbarWidth` under `workbench.experimental.modernUI`, else the classic width.
pub const SCROLLBAR_WIDTH_MODERN: f32 = 10.;
pub const SCROLLBAR_WIDTH: f32 = 14.;
/// terminal.css `.xterm` padding-left: the gutter left of the grid.
pub const GUTTER: f32 = 20.;

/// The font as `TerminalConfigurationService.getFont` resolves it and the cell `WebglRenderer._updateDimensions`
/// derives from xterm's `TextMetricsMeasureStrategy` ('W' advance, font bounding box height).
#[derive(Clone, Debug, PartialEq)]
pub struct CellMetrics {
    pub family: SharedString,
    pub font_size: f32,
    pub bold_weight: FontWeight,
    pub normal_weight: FontWeight,
    /// The measured advance and height, unrounded: `getXtermScaledDimensions` divides by these.
    pub char_width: f32,
    pub char_height: f32,
    pub line_height: f32,
    pub letter_spacing: f32,
    /// The rendered cell in px: whole device pixels.
    pub cell_width: f32,
    pub cell_height: f32,
    pub descent: f32,
    pub scale: f32,
}

impl CellMetrics {
    pub fn measure(window: &Window, family: SharedString, font_size: f32, line_height: f32, letter_spacing: f32, normal_weight: FontWeight, bold_weight: FontWeight) -> Self {
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font(family.clone()));
        let char_width = f32::from(text_system.advance(font_id, px(font_size), 'W').map(|advance| advance.width).unwrap_or(px(font_size * 0.6)));
        let ascent = f32::from(text_system.ascent(font_id, px(font_size)));
        let descent = f32::from(text_system.descent(font_id, px(font_size))).abs();
        let char_height = ascent + descent;
        let scale = window.scale_factor();
        let device_char_width = (char_width * scale).floor();
        let device_char_height = (char_height * scale).ceil();
        let device_cell_height = (device_char_height * line_height).floor();
        let device_cell_width = device_char_width + letter_spacing.round();
        CellMetrics {
            family,
            font_size,
            bold_weight,
            normal_weight,
            char_width,
            char_height,
            line_height,
            letter_spacing,
            cell_width: device_cell_width / scale,
            cell_height: device_cell_height / scale,
            descent,
            scale,
        }
    }

    /// `getXtermScaledDimensions`: the columns and rows that fit `width` by `height` px.
    pub fn grid_size(&self, width: f32, height: f32) -> (u16, u16) {
        let scaled_char_width = self.char_width * self.scale + self.letter_spacing;
        let cols = ((width * self.scale) / scaled_char_width).floor().max(1.);
        let scaled_line_height = ((self.char_height * self.scale).ceil() * self.line_height).floor();
        let rows = ((height * self.scale) / scaled_line_height).floor().max(1.);
        (cols.min(u16::MAX as f32) as u16, rows.min(u16::MAX as f32) as u16)
    }

    fn font(&self, bold: bool, italic: bool) -> Font {
        Font {
            weight: if bold { self.bold_weight } else { self.normal_weight },
            style: if italic { FontStyle::Italic } else { FontStyle::Normal },
            ..font(self.family.clone())
        }
    }
}

/// What paints besides the cells: the settings and state xterm's renderer reads.
pub struct Look<'a> {
    pub colors: &'a TerminalColors,
    pub metrics: &'a CellMetrics,
    pub focused: bool,
    /// The blinking cursor's hidden phase.
    pub cursor_blink_off: bool,
    /// `terminal.integrated.cursorStyle` and `cursorStyleInactive`.
    pub cursor_inactive_style: &'a str,
    pub cursor_width: f32,
    pub minimum_contrast_ratio: f64,
    pub draw_bold_text_in_bright_colors: bool,
    pub scrollbar_width: f32,
    /// The scrollbar shows while the pointer is over the terminal (`ScrollbarVisibility.Auto`).
    pub hovered: bool,
    pub scrollbar_slider: Rgba,
    /// The find widget's matches, the selected one, and their `terminal.findMatch*Background` colors.
    pub find_matches: &'a [alacritty_terminal::term::search::Match],
    pub find_current: Option<usize>,
    pub find_match: Rgba,
    pub find_highlight: Rgba,
}

/// The grid's top left: bottom aligned in the pane (terminal.css `.xterm { bottom: 0 }`), right of the gutter.
pub fn grid_origin(bounds: Bounds<Pixels>, rows: usize, metrics: &CellMetrics) -> gpui::Point<Pixels> {
    let height = rows as f32 * metrics.cell_height;
    point(bounds.origin.x + px(GUTTER), (bounds.bottom() - px(height)).max(bounds.origin.y))
}

pub fn paint(bounds: Bounds<Pixels>, instance: &TerminalInstance, look: &Look, window: &mut Window) {
    let metrics = look.metrics;
    let term = &instance.term;
    let (cols, rows) = (term.columns(), term.screen_lines());
    let origin = grid_origin(bounds, rows, metrics);
    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let colors = look.colors;
    let resolve = |color: Color, bold: bool| resolve_color(color, bold, content.colors, look);
    let cursor = content.cursor;
    let cursor_row = cursor.point.line.0 + offset;
    let cursor_shown = cursor.shape != CursorShape::Hidden && !(look.focused && look.cursor_blink_off);
    let block_cursor = look.focused && cursor_shown && cursor.shape == CursorShape::Block;
    let mut contrast_cache: HashMap<([u32; 4], [u32; 4]), Option<Rgba>> = HashMap::new();
    let mut rows_text: Vec<RowText> = (0..rows).map(|_| RowText::default()).collect();
    let cell = |column: usize, row: usize| Bounds::new(origin + point(px(column as f32 * metrics.cell_width), px(row as f32 * metrics.cell_height)), size(px(metrics.cell_width), px(metrics.cell_height)));

    let (top, bottom) = (-offset, rows as i32 - 1 - offset);
    let visible_matches: Vec<(usize, &alacritty_terminal::term::search::Match)> = look.find_matches.iter().enumerate().filter(|(_, found)| found.end().line.0 >= top && found.start().line.0 <= bottom).collect();
    for indexed in content.display_iter {
        let row = (indexed.point.line.0 + offset) as usize;
        let column = indexed.point.column.0;
        if row >= rows || column >= cols {
            continue;
        }
        let flags = indexed.cell.flags;
        if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let bold = flags.contains(Flags::BOLD);
        let mut fg = resolve(indexed.cell.fg, bold);
        let mut bg = match indexed.cell.bg {
            Color::Named(NamedColor::Background) => None,
            color => Some(resolve(color, false)),
        };
        if flags.contains(Flags::INVERSE) {
            let background = bg.unwrap_or(colors.background);
            bg = Some(fg);
            fg = background;
        }
        if flags.contains(Flags::DIM) {
            fg.a *= 0.5;
        }
        let width = if flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
        let is_cursor = indexed.point == cursor.point && cursor.shape != CursorShape::Hidden;
        let selected = content.selection.is_some_and(|selection| selection.contains(indexed.point));
        if selected && let Some(selection_foreground) = colors.selection_foreground {
            fg = selection_foreground;
        }
        if is_cursor && block_cursor {
            bg = Some(colors.cursor);
            fg = colors.cursor_accent;
        } else if look.minimum_contrast_ratio > 1. {
            let background = bg.unwrap_or(colors.background);
            let key = (key(background), key(fg));
            let adjusted = *contrast_cache.entry(key).or_insert_with(|| ensure_contrast_ratio(background, Rgba { a: 1., ..fg }, look.minimum_contrast_ratio));
            if let Some(adjusted) = adjusted {
                fg = Rgba { a: fg.a, ..adjusted };
            }
        }
        let cell_bounds = Bounds::new(cell(column, row).origin, size(px(metrics.cell_width * width as f32), px(metrics.cell_height)));
        if let Some(bg) = bg {
            window.paint_quad(fill(cell_bounds, bg));
        }
        if let Some((index, _)) = visible_matches.iter().find(|(_, found)| found.contains(&indexed.point)) {
            window.paint_quad(fill(cell_bounds, if look.find_current == Some(*index) { look.find_match } else { look.find_highlight }));
        }
        if selected && !(is_cursor && block_cursor) {
            let selection = if look.focused { colors.selection } else { colors.inactive_selection };
            window.paint_quad(fill(cell_bounds, selection));
        }
        push_cell_text(indexed.cell, column, fg, cell_bounds, &mut rows_text[row], &resolve, metrics, window);
    }

    let baseline = |row: usize| {
        let y = f32::from(origin.y) + row as f32 * metrics.cell_height + metrics.cell_height - metrics.descent;
        px((y * metrics.scale).round() / metrics.scale)
    };
    for (row, line) in rows_text.into_iter().enumerate() {
        paint_row_text(line, origin.x, baseline(row), metrics, window);
    }

    if cursor_shown && (0..rows as i32).contains(&cursor_row) {
        let bounds = cell(cursor.point.column.0.min(cols - 1), cursor_row as usize);
        let thickness = px(look.cursor_width.max(1.));
        let shape = if look.focused { Some(cursor.shape) } else { inactive_shape(look.cursor_inactive_style) };
        match shape {
            Some(CursorShape::Beam) => window.paint_quad(fill(Bounds::new(bounds.origin, size(thickness, bounds.size.height)), colors.cursor)),
            Some(CursorShape::Underline) => window.paint_quad(fill(Bounds::new(point(bounds.origin.x, bounds.bottom() - thickness), size(bounds.size.width, thickness)), colors.cursor)),
            Some(CursorShape::HollowBlock) => window.paint_quad(outline(bounds, colors.cursor, gpui::BorderStyle::Solid)),
            Some(CursorShape::Block) if !look.focused => window.paint_quad(fill(bounds, colors.cursor)),
            _ => {}
        }
    }

    let history = term.history_size();
    if look.hovered && history > 0 {
        paint_scrollbar(bounds, rows, history, content.display_offset, look, window);
    }
}

/// terminalStickyScroll.css `.terminal-sticky-scroll`: its colors, the hover one while the pointer is over it.
pub struct StickyLook {
    pub background: Rgba,
    pub hover: Option<Rgba>,
    pub border: Option<Rgba>,
    pub shadow: Option<Rgba>,
}

/// The overlay: left of the gutter to the scrollbar, its top `shift` rows above the grid's.
pub fn sticky_scroll_bounds(bounds: Bounds<Pixels>, rows: usize, sticky: &StickyScroll, metrics: &CellMetrics, scrollbar_width: f32) -> Bounds<Pixels> {
    let top = grid_origin(bounds, rows, metrics).y - px(sticky.shift as f32 * metrics.cell_height);
    Bounds::new(point(bounds.origin.x, top), size(bounds.size.width - px(scrollbar_width), px(sticky.count as f32 * metrics.cell_height)))
}

/// The sticky overlay's own xterm: the buffer lines `sticky` names, without cursor or selection, ` …` after the
/// last when truncated, a `0 3px 2px -2px` shadow and a bottom border.
pub fn paint_sticky_scroll(bounds: Bounds<Pixels>, instance: &TerminalInstance, sticky: &StickyScroll, sticky_look: &StickyLook, look: &Look, window: &mut Window) {
    let metrics = look.metrics;
    let term = &instance.term;
    let overlay = sticky_scroll_bounds(bounds, term.screen_lines(), sticky, metrics, look.scrollbar_width);
    if let Some(shadow) = sticky_look.shadow {
        let shadow = BoxShadow { color: Hsla::from(shadow), offset: point(px(0.), px(3.)), blur_radius: px(2.), spread_radius: px(-2.), inset: false };
        window.paint_drop_shadows(overlay, Corners::default(), &[shadow]);
    }
    let background = sticky_look.hover.unwrap_or(sticky_look.background);
    window.paint_quad(fill(overlay, background));
    if let Some(border) = sticky_look.border {
        window.paint_quad(fill(Bounds::new(point(overlay.origin.x, overlay.bottom()), size(overlay.size.width, px(1.))), border));
    }

    let origin = point(overlay.origin.x + px(GUTTER), overlay.origin.y);
    let resolve = |color: Color, bold: bool| resolve_color(color, bold, term.colors(), look);
    let mut contrast_cache: HashMap<([u32; 4], [u32; 4]), Option<Rgba>> = HashMap::new();
    for row in 0..sticky.count {
        let line = sticky.start + row as i64;
        let y = origin.y + px(row as f32 * metrics.cell_height);
        let mut text = RowText::default();
        let mut last_column = None;
        for column in 0..term.columns() {
            let cell = &term.grid()[Line((line - term.history_size() as i64) as i32)][Column(column)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let bold = cell.flags.contains(Flags::BOLD);
            let mut fg = resolve(cell.fg, bold);
            let mut bg = match cell.bg {
                Color::Named(NamedColor::Background) => None,
                color => Some(resolve(color, false)),
            };
            if cell.flags.contains(Flags::INVERSE) {
                let cell_background = bg.unwrap_or(background);
                bg = Some(fg);
                fg = cell_background;
            }
            if cell.flags.contains(Flags::DIM) {
                fg.a *= 0.5;
            }
            if look.minimum_contrast_ratio > 1. {
                let cell_background = bg.unwrap_or(background);
                let adjusted = *contrast_cache.entry((key(cell_background), key(fg))).or_insert_with(|| ensure_contrast_ratio(cell_background, Rgba { a: 1., ..fg }, look.minimum_contrast_ratio));
                if let Some(adjusted) = adjusted {
                    fg = Rgba { a: fg.a, ..adjusted };
                }
            }
            let width = if cell.flags.contains(Flags::WIDE_CHAR) { 2. } else { 1. };
            let cell_bounds = Bounds::new(point(origin.x + px(column as f32 * metrics.cell_width), y), size(px(metrics.cell_width * width), px(metrics.cell_height)));
            if let Some(bg) = bg {
                window.paint_quad(fill(cell_bounds, bg));
            }
            if cell.c != ' ' && cell.c != '\0' {
                last_column = Some(column);
            }
            push_cell_text(cell, column, fg, cell_bounds, &mut text, &resolve, metrics, window);
        }
        if sticky.truncated && row + 1 == sticky.count {
            let column = last_column.map_or(0, |column| column + 2);
            if column < term.columns() {
                let start = text.text.len();
                text.text.push('…');
                text.cells.push(CellText { start, column, fg: look.colors.foreground, bold: false, italic: false });
            }
        }
        let baseline = f32::from(y) + metrics.cell_height - metrics.descent;
        paint_row_text(text, origin.x, px((baseline * metrics.scale).round() / metrics.scale), metrics, window);
    }
}

/// The cell's text joins its row's, shaped and painted later; underline and strikethrough paint now.
#[allow(clippy::too_many_arguments)]
fn push_cell_text(cell: &Cell, column: usize, fg: Rgba, cell_bounds: Bounds<Pixels>, line: &mut RowText, resolve: &dyn Fn(Color, bool) -> Rgba, metrics: &CellMetrics, window: &mut Window) {
    let flags = cell.flags;
    if cell.c == ' ' || cell.c == '\0' || flags.contains(Flags::HIDDEN) {
        return;
    }
    let start = line.text.len();
    line.text.push(cell.c);
    for &zero_width in cell.zerowidth().into_iter().flatten() {
        line.text.push(zero_width);
    }
    line.cells.push(CellText { start, column, fg, bold: flags.contains(Flags::BOLD), italic: flags.contains(Flags::ITALIC) });
    let decoration_width = cell_bounds.size.width;
    if flags.intersects(Flags::ALL_UNDERLINES) {
        let underline_color = cell.underline_color().map_or(fg, |color| resolve(color, false));
        let style = UnderlineStyle { thickness: px(1.), color: Some(Hsla::from(underline_color)), wavy: flags.contains(Flags::UNDERCURL) };
        window.paint_underline(point(cell_bounds.origin.x, cell_bounds.bottom() - px(metrics.descent.max(2.)) + px(1.)), decoration_width, &style);
    }
    if flags.contains(Flags::STRIKEOUT) {
        let style = StrikethroughStyle { thickness: px(1.), color: Some(Hsla::from(fg)) };
        window.paint_strikethrough(point(cell_bounds.origin.x, cell_bounds.origin.y + px((metrics.cell_height / 2.).round())), decoration_width, &style);
    }
}

/// xterm's color lookup: an OSC 4/10/11 override, else the theme palette, bold colors brightened when
/// `drawBoldTextInBrightColors` is on.
fn resolve_color(color: Color, bold: bool, overrides: &Colors, look: &Look) -> Rgba {
    let colors = look.colors;
    let index = match color {
        Color::Spec(rgb) => return rgba(rgb.r, rgb.g, rgb.b),
        Color::Indexed(index) if bold && look.draw_bold_text_in_bright_colors && index < 8 => index as usize + 8,
        Color::Indexed(index) => index as usize,
        Color::Named(named) if bold && look.draw_bold_text_in_bright_colors && (named as usize) < 8 => named as usize + 8,
        Color::Named(named) => named as usize,
    };
    if let Some(rgb) = overrides[index] {
        return rgba(rgb.r, rgb.g, rgb.b);
    }
    match index {
        0..256 => colors.palette[index],
        index if index == NamedColor::Background as usize => colors.background,
        index if index == NamedColor::Cursor as usize => colors.cursor,
        index if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&index) => {
            let color = colors.palette[index - NamedColor::DimBlack as usize];
            Rgba { a: color.a * 0.5, ..color }
        }
        _ => colors.foreground,
    }
}

/// `cursorStyleInactive`: outline, block, line, underline or none while the terminal is not focused.
fn inactive_shape(style: &str) -> Option<CursorShape> {
    match style {
        "outline" => Some(CursorShape::HollowBlock),
        "block" => Some(CursorShape::Block),
        "line" => Some(CursorShape::Beam),
        "underline" => Some(CursorShape::Underline),
        _ => None,
    }
}

/// The monaco scrollbar xterm.js 6 uses for its viewport: the slider over the right edge, sized to the rows shown.
fn paint_scrollbar(bounds: Bounds<Pixels>, rows: usize, history: usize, display_offset: usize, look: &Look, window: &mut Window) {
    let track = f32::from(bounds.size.height);
    let total = (rows + history) as f32;
    let slider_height = (track * rows as f32 / total).max(20.).min(track);
    let first_row = (history - display_offset) as f32;
    let top = (track - slider_height) * first_row / history as f32;
    let slider = Bounds::new(point(bounds.right() - px(look.scrollbar_width), bounds.origin.y + px(top)), size(px(look.scrollbar_width), px(slider_height)));
    window.paint_quad(fill(slider, look.scrollbar_slider));
}

#[derive(Default)]
struct RowText {
    text: String,
    cells: Vec<CellText>,
}

struct CellText {
    /// The byte offset of the cell's text in its row.
    start: usize,
    column: usize,
    fg: Rgba,
    bold: bool,
    italic: bool,
}

/// Shapes a row's text in one pass and paints each glyph at its cell, as xterm draws every cell on the grid
/// whatever the font's advance.
fn paint_row_text(line: RowText, left: Pixels, baseline: Pixels, metrics: &CellMetrics, window: &mut Window) {
    if line.cells.is_empty() {
        return;
    }
    let mut runs: Vec<TextRun> = Vec::new();
    let mut run_style = None;
    for (index, cell) in line.cells.iter().enumerate() {
        let end = line.cells.get(index + 1).map_or(line.text.len(), |next| next.start);
        let style = (cell.bold, cell.italic);
        match runs.last_mut() {
            Some(run) if run_style == Some(style) => run.len += end - cell.start,
            _ => {
                runs.push(TextRun { len: end - cell.start, font: metrics.font(cell.bold, cell.italic), color: Hsla::default(), background_color: None, underline: None, strikethrough: None });
                run_style = Some(style);
            }
        }
    }
    let font_size = px(metrics.font_size);
    let shaped = window.text_system().shape_line(SharedString::from(line.text), font_size, &runs, None);
    let mut cell_index = 0;
    let mut cluster_x = px(0.);
    for run in &shaped.runs {
        for glyph in &run.glyphs {
            while cell_index + 1 < line.cells.len() && line.cells[cell_index + 1].start <= glyph.index {
                cell_index += 1;
                cluster_x = glyph.position.x;
            }
            if cell_index == 0 && glyph.index == 0 {
                cluster_x = px(0.).min(glyph.position.x);
            }
            let cell = &line.cells[cell_index];
            let x = left + px(cell.column as f32 * metrics.cell_width) + (glyph.position.x - cluster_x);
            let origin = point(x, baseline);
            let result = if glyph.is_emoji {
                window.paint_emoji(origin, run.font_id, glyph.id, font_size)
            } else {
                window.paint_glyph(origin, run.font_id, glyph.id, font_size, Hsla::from(cell.fg))
            };
            if let Err(error) = result {
                eprintln!("[terminal] painting a glyph: {error}");
            }
        }
    }
}

fn rgba(r: u8, g: u8, b: u8) -> Rgba {
    Rgba { r: r as f32 / 255., g: g as f32 / 255., b: b as f32 / 255., a: 1. }
}

fn key(color: Rgba) -> [u32; 4] {
    [color.r, color.g, color.b, color.a].map(f32::to_bits)
}

/// The viewport cell under a position, with the side of the cell it is on (xterm's `getCoords`).
pub fn cell_at(bounds: Bounds<Pixels>, position: gpui::Point<Pixels>, instance: &TerminalInstance, metrics: &CellMetrics) -> (i32, usize, Side) {
    let term = &instance.term;
    let origin = grid_origin(bounds, term.screen_lines(), metrics);
    let x = f32::from(position.x - origin.x).max(0.);
    let y = f32::from(position.y - origin.y);
    let column = ((x / metrics.cell_width) as usize).min(term.columns() - 1);
    let side = if x - column as f32 * metrics.cell_width < metrics.cell_width / 2. { Side::Left } else { Side::Right };
    let row = (y / metrics.cell_height).floor() as i32;
    (row, column, side)
}
