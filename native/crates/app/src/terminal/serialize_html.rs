//! @xterm/addon-serialize `HTMLSerializeHandler`: buffer cells as the HTML a copy puts beside the text, one
//! `<div><span>` per row and a styled `<span>` wherever the cell attributes change.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use gpui::Rgba;

use super::TerminalInstance;

impl TerminalInstance {
    /// `serializeAsHTML({ range })` from buffer line `start.0` column `start.1` to line `end.0` column `end.1`
    /// (exclusive), with the theme's ANSI colors, `fontFamily` and `fontSize`.
    pub fn serialize_html(&self, start: (i64, usize), end: (i64, usize), palette: &[Rgba; 256], font_family: &str, font_size: f32) -> String {
        let history = self.term.history_size() as i64;
        let columns = self.term.columns();
        let mut html = format!("<html><body><!--StartFragment--><pre><div style='color: #000000; background-color: #ffffff; font-family: {font_family}; font-size: {font_size}px;'>");
        let mut old = Cell::default();
        for line in start.0..=end.0 {
            let screen_line = line - history;
            if screen_line < -history || screen_line >= self.term.screen_lines() as i64 {
                continue;
            }
            let row = &self.term.grid()[Line(screen_line as i32)];
            let first = if line == start.0 { start.1 } else { 0 };
            let last = if line == end.0 { end.1.min(columns) } else { columns };
            html.push_str("<div><span>");
            for column in first..last {
                let cell = &row[Column(column)];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                if (cell.fg, cell.bg, cell.flags, cell.underline_color()) != (old.fg, old.bg, old.flags, old.underline_color()) {
                    let style = style(cell, palette);
                    match style.is_empty() {
                        true => html.push_str("</span><span>"),
                        false => html.push_str(&format!("</span><span style='{}'>", style.join(" "))),
                    }
                    old = cell.clone();
                }
                match cell.c {
                    ' ' | '\0' => html.push(' '),
                    c => {
                        escape(c, &mut html);
                        for c in cell.zerowidth().into_iter().flatten() {
                            escape(*c, &mut html);
                        }
                    }
                }
            }
            html.push_str("</span></div>");
        }
        html.push_str("</div></pre><!--EndFragment--></body></html>");
        html
    }
}

/// `_diffStyle`: the declarations of a cell whose attributes differ from the cell before it.
fn style(cell: &Cell, palette: &[Rgba; 256]) -> Vec<String> {
    let mut content = Vec::new();
    if let Some(color) = hex(cell.fg, palette) {
        content.push(format!("color: {color};"));
    }
    if let Some(color) = hex(cell.bg, palette) {
        content.push(format!("background-color: {color};"));
    }
    let flags = cell.flags;
    if flags.contains(Flags::INVERSE) {
        content.push("color: #000000; background-color: #BFBFBF;".into());
    }
    if flags.contains(Flags::BOLD) {
        content.push("font-weight: bold;".into());
    }
    let underline = if flags.contains(Flags::DOUBLE_UNDERLINE) {
        Some("underline double")
    } else if flags.contains(Flags::UNDERCURL) {
        Some("underline wavy")
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        Some("underline dotted")
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        Some("underline dashed")
    } else if flags.contains(Flags::UNDERLINE) {
        Some("underline")
    } else {
        None
    };
    let decorations: Vec<&str> = underline.into_iter().chain(flags.contains(Flags::STRIKEOUT).then_some("line-through")).collect();
    if !decorations.is_empty() {
        content.push(format!("text-decoration: {};", decorations.join(" ")));
    }
    if underline.is_some()
        && let Some(color) = cell.underline_color().and_then(|color| hex(color, palette))
    {
        content.push(format!("text-decoration-color: {color};"));
    }
    if flags.contains(Flags::HIDDEN) {
        content.push("visibility: hidden;".into());
    }
    if flags.contains(Flags::ITALIC) {
        content.push("font-style: italic;".into());
    }
    if flags.contains(Flags::DIM) {
        content.push("opacity: 0.5;".into());
    }
    content
}

/// `_getHexColor`: an RGB or palette color; the default foreground and background have none.
fn hex(color: Color, palette: &[Rgba; 256]) -> Option<String> {
    let (r, g, b) = match color {
        Color::Spec(rgb) => (rgb.r, rgb.g, rgb.b),
        Color::Indexed(index) => to_bytes(palette[index as usize]),
        Color::Named(named) if (named as usize) < 16 => to_bytes(palette[named as usize]),
        Color::Named(named) if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&(named as usize)) => to_bytes(palette[named as usize - NamedColor::DimBlack as usize]),
        Color::Named(_) => return None,
    };
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

fn to_bytes(color: Rgba) -> (u8, u8, u8) {
    ((color.r * 255.).round() as u8, (color.g * 255.).round() as u8, (color.b * 255.).round() as u8)
}

/// `escapeHTMLChar`.
fn escape(c: char, html: &mut String) {
    match c {
        '<' => html.push_str("&lt;"),
        '>' => html.push_str("&gt;"),
        '&' => html.push_str("&amp;"),
        '"' => html.push_str("&quot;"),
        '\'' => html.push_str("&#39;"),
        c => html.push(c),
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::term::Config;

    use super::super::{ShellLaunchConfig, TerminalInstance, colors};

    #[test]
    fn rows_become_spans_with_the_changed_styles() {
        let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let colors = colors::TerminalColors::new(&crate::theme::load_theme(&[themes], "Dark Modern").unwrap());
        let launch = ShellLaunchConfig { extension_pty: true, cwd: std::env::temp_dir(), ..Default::default() };
        let (mut instance, _) = TerminalInstance::new(1, launch, 8, 4, Config { scrolling_history: 100, ..Config::default() });
        instance.process(b"a<\x1b[1;38;2;255;0;0mb\x1b[0m\r\nok", &colors);
        let html = instance.serialize_html((0, 0), (1, 2), &colors.palette, "Menlo", 12.);
        assert_eq!(
            html,
            "<html><body><!--StartFragment--><pre><div style='color: #000000; background-color: #ffffff; font-family: Menlo; font-size: 12px;'>\
             <div><span>a&lt;</span><span style='color: #ff0000; font-weight: bold;'>b</span><span>     </span></div>\
             <div><span>ok</span></div></div></pre><!--EndFragment--></body></html>"
        );
    }
}
