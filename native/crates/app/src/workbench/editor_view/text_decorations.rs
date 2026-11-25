//! The extension decorations of a text editor (`createTextEditorDecorationType` and `setDecorations`), drawn as
//! decorationRenderOptions.ts turns them into CSS: background, outline and border boxes per line (the whole line with
//! `isWholeLine`), text color, font style and opacity over the range, the `before` and `after` content text, and
//! overview ruler marks.

use std::ops::Range;

use gpui::{Div, FontStyle, FontWeight, HighlightStyle, Rgba, div, prelude::*, px};

use super::Workbench;
use crate::editor::decoration_types::{DecorationColor, DecorationStyle, TextDecoration};
use crate::editor::{OpenEditor, expand_tabs};
use crate::theme::parse_hex;

impl Workbench {
    /// The style of one decoration: its type's options with the decoration's own `renderOptions` over them.
    fn decoration_style(&self, decoration: &TextDecoration) -> Option<DecorationStyle> {
        let options = self.editors.decoration_types.get(&decoration.key)?;
        let dark = !self.theme.kind.is_light();
        let mut style = DecorationStyle::from_options(options, dark);
        if let Some(own) = &decoration.render_options {
            let own = DecorationStyle::from_options(own, dark);
            style.before = own.before.or(style.before);
            style.after = own.after.or(style.after);
        }
        Some(style)
    }

    fn decoration_color(&self, color: &Option<DecorationColor>) -> Option<Rgba> {
        match color.as_ref()? {
            DecorationColor::Theme(id) => self.theme.color(id),
            DecorationColor::Css(css) => css_color(css),
        }
    }

    /// The boxes and content text of the decorations on model line `line`, `x` giving a column's left edge.
    pub(super) fn text_decoration_boxes(&self, editor: &OpenEditor, line: usize, scroll_left: f32, x: impl Fn(usize) -> f32) -> Vec<Div> {
        let line_length = editor.document.lines[line].chars().count();
        let mut boxes = Vec::new();
        for decoration in editor.text_decorations.iter().filter(|decoration| decoration.start.line <= line && line <= decoration.end.line) {
            let Some(style) = self.decoration_style(decoration) else { continue };
            let from = if line == decoration.start.line { decoration.start.column } else { 0 };
            let to = if line == decoration.end.line { decoration.end.column } else { line_length };
            // A whole-line decoration ending at column 0 of its last line does not reach that line (Range of lines).
            if style.is_whole_line && line == decoration.end.line && to == 0 && decoration.end.line > decoration.start.line {
                continue;
            }
            let mut area = match style.is_whole_line {
                true => div().absolute().inset_0().left(px(scroll_left)),
                false => div().absolute().top_0().h_full().left(px(x(from))).w(px(x(to) - x(from))),
            };
            if let Some(background) = self.decoration_color(&style.background_color) {
                area = area.bg(background);
            }
            // shortcut: GPUI draws borders inside the box, where CSS outlines draw outside it; widths round to 1px.
            if let Some(outline) = self.decoration_color(&style.outline_color).or_else(|| self.decoration_color(&style.border_color)) {
                area = area.border_1().border_color(outline);
            }
            boxes.push(area);
            for (attachment, column) in [(&style.before, from), (&style.after, to)] {
                let Some(attachment) = attachment.as_ref().filter(|attachment| !attachment.content_text.is_empty()) else { continue };
                if (column == from && line != decoration.start.line) || (column == to && line != decoration.end.line) {
                    continue;
                }
                // shortcut: content text is drawn over the line after its column; upstream's inline span moves the text behind it.
                let mut text = div().absolute().top_0().left(px(x(column))).whitespace_nowrap().child(attachment.content_text.clone());
                if let Some(color) = self.decoration_color(&attachment.color) {
                    text = text.text_color(color);
                }
                if let Some(background) = self.decoration_color(&attachment.background_color) {
                    text = text.bg(background);
                }
                if attachment.font_style.as_deref() == Some("italic") {
                    text = text.italic();
                }
                if attachment.font_weight.as_deref().is_some_and(|weight| weight == "bold" || weight.parse::<u16>().is_ok_and(|weight| weight >= 600)) {
                    text = text.font_weight(FontWeight::BOLD);
                }
                boxes.push(text);
            }
        }
        boxes
    }

    /// The text styles of the decorations on model line `line`, as byte ranges of its tab-expanded text.
    pub(super) fn text_decoration_styles(&self, editor: &OpenEditor, line: usize, tab_size: usize) -> Vec<(Range<usize>, HighlightStyle)> {
        let text = &editor.document.lines[line];
        let byte = |column: usize| expand_tabs(&text.chars().take(column).collect::<String>(), tab_size).len();
        let mut styles = Vec::new();
        for decoration in editor.text_decorations.iter().filter(|decoration| decoration.start.line <= line && line <= decoration.end.line) {
            let Some(style) = self.decoration_style(decoration) else { continue };
            let highlight = HighlightStyle {
                color: self.decoration_color(&style.color).map(Into::into),
                font_style: (style.font_style.as_deref() == Some("italic")).then_some(FontStyle::Italic),
                font_weight: style.font_weight.as_deref().filter(|weight| *weight == "bold").map(|_| FontWeight::BOLD),
                fade_out: style.opacity.map(|opacity| 1. - opacity),
                ..Default::default()
            };
            if highlight == HighlightStyle::default() {
                continue;
            }
            let (from, to) = match style.is_whole_line {
                true => (0, text.chars().count()),
                false => (if line == decoration.start.line { decoration.start.column } else { 0 }, if line == decoration.end.line { decoration.end.column } else { text.chars().count() }),
            };
            if from < to {
                styles.push((byte(from)..byte(to), highlight));
            }
        }
        styles.sort_by_key(|(range, _)| range.start);
        styles
    }

    /// `overviewRulerColor` marks: first and last line, color and `OverviewRulerLane`.
    pub(super) fn text_decoration_ruler_marks(&self, editor: &OpenEditor) -> Vec<(usize, usize, Rgba, u64)> {
        let mut marks = Vec::new();
        for decoration in &editor.text_decorations {
            let Some(style) = self.decoration_style(decoration) else { continue };
            if let Some(color) = self.decoration_color(&style.overview_ruler_color) {
                marks.push((decoration.start.line, decoration.end.line, color, style.overview_ruler_lane));
            }
        }
        marks
    }
}

/// A CSS color: hex, `rgb()`/`rgba()`, or `transparent`.
fn css_color(css: &str) -> Option<Rgba> {
    let css = css.trim();
    if css == "transparent" {
        return Some(Rgba { r: 0., g: 0., b: 0., a: 0. });
    }
    if let Some(hex) = parse_hex(css) {
        return Some(hex);
    }
    let inner = css.strip_prefix("rgba(").or_else(|| css.strip_prefix("rgb("))?.strip_suffix(')')?;
    let parts: Vec<f32> = inner.split(',').map(|part| part.trim().parse::<f32>()).collect::<Result<_, _>>().ok()?;
    match parts[..] {
        [r, g, b] => Some(Rgba { r: r / 255., g: g / 255., b: b / 255., a: 1. }),
        [r, g, b, a] => Some(Rgba { r: r / 255., g: g / 255., b: b / 255., a }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::css_color;

    #[test]
    fn css_colors_parse() {
        assert_eq!(css_color("rgba(255, 0, 0, 0.5)").unwrap().a, 0.5);
        assert_eq!(css_color("#00ff00").unwrap().g, 1.);
        assert_eq!(css_color("transparent").unwrap().a, 0.);
        assert!(css_color("red").is_none());
    }
}
