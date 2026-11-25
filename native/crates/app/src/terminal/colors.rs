//! terminalColorRegistry.ts defaults and xterm.js color handling: the 256-color palette (`DEFAULT_ANSI_COLORS`)
//! and `rgba.ensureContrastRatio` (common/Color.ts) behind `terminal.integrated.minimumContrastRatio`.

use gpui::Rgba;

use crate::theme::{Theme, ThemeKind};

/// The 16 `terminal.ansi*` keys in palette order, with their (dark, light, hcDark, hcLight) defaults.
const ANSI_COLORS: [(&str, [&str; 4]); 16] = [
    ("terminal.ansiBlack", ["#000000", "#000000", "#000000", "#292929"]),
    ("terminal.ansiRed", ["#cd3131", "#cd3131", "#cd0000", "#cd3131"]),
    ("terminal.ansiGreen", ["#0DBC79", "#107C10", "#00cd00", "#136C13"]),
    ("terminal.ansiYellow", ["#e5e510", "#949800", "#cdcd00", "#949800"]),
    ("terminal.ansiBlue", ["#2472c8", "#0451a5", "#0000ee", "#0451a5"]),
    ("terminal.ansiMagenta", ["#bc3fbc", "#bc05bc", "#cd00cd", "#bc05bc"]),
    ("terminal.ansiCyan", ["#11a8cd", "#0598bc", "#00cdcd", "#0598bc"]),
    ("terminal.ansiWhite", ["#e5e5e5", "#555555", "#e5e5e5", "#555555"]),
    ("terminal.ansiBrightBlack", ["#666666", "#666666", "#7f7f7f", "#666666"]),
    ("terminal.ansiBrightRed", ["#f14c4c", "#f14c4c", "#ff0000", "#cd3131"]),
    ("terminal.ansiBrightGreen", ["#23d18b", "#14CE14", "#00ff00", "#00bc00"]),
    ("terminal.ansiBrightYellow", ["#f5f543", "#b5ba00", "#ffff00", "#b5ba00"]),
    ("terminal.ansiBrightBlue", ["#3b8eea", "#3b8eea", "#5c5cff", "#0451a5"]),
    ("terminal.ansiBrightMagenta", ["#d670d6", "#d670d6", "#ff00ff", "#bc05bc"]),
    ("terminal.ansiBrightCyan", ["#29b8db", "#29b8db", "#00ffff", "#0598bc"]),
    ("terminal.ansiBrightWhite", ["#e5e5e5", "#a5a5a5", "#ffffff", "#a5a5a5"]),
];

/// The colors `XtermTerminal.getXtermTheme` hands xterm.js.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalColors {
    pub palette: [Rgba; 256],
    pub foreground: Rgba,
    pub background: Rgba,
    pub cursor: Rgba,
    pub cursor_accent: Rgba,
    pub selection: Rgba,
    pub inactive_selection: Rgba,
    pub selection_foreground: Option<Rgba>,
    pub border: Option<Rgba>,
}

impl TerminalColors {
    /// The panel location: the background falls back to `panel.background`.
    pub fn new(theme: &Theme) -> Self {
        let variant = match theme.kind {
            ThemeKind::Dark => 0,
            ThemeKind::Light => 1,
            ThemeKind::HighContrastDark => 2,
            ThemeKind::HighContrastLight => 3,
        };
        let mut palette = [Rgba::default(); 256];
        for (index, (key, defaults)) in ANSI_COLORS.iter().enumerate() {
            palette[index] = theme.color(key).unwrap_or_else(|| hex(defaults[variant]));
        }
        // xterm.js DEFAULT_ANSI_COLORS: a 6x6x6 cube, then 24 grays.
        let steps = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
        for index in 0..216 {
            palette[16 + index] = rgb(steps[index / 36], steps[index / 6 % 6], steps[index % 6]);
        }
        for index in 0..24 {
            let level = 8 + index as u8 * 10;
            palette[232 + index] = rgb(level, level, level);
        }
        let foreground = theme.color("terminal.foreground").unwrap_or_else(|| hex(["#CCCCCC", "#333333", "#FFFFFF", "#292929"][variant]));
        let background = theme.color("terminal.background").or_else(|| theme.color("panel.background")).or_else(|| theme.color("editor.background")).expect("editor.background has a registry default");
        let selection = theme.color("terminal.selectionBackground").or_else(|| theme.color("editor.selectionBackground")).unwrap_or(Rgba { r: 0.149, g: 0.31, b: 0.47, a: 1. });
        let inactive_alpha = if theme.kind == ThemeKind::HighContrastDark { 0.7 } else { 0.5 };
        TerminalColors {
            palette,
            foreground,
            background,
            cursor: theme.color("terminalCursor.foreground").unwrap_or(foreground),
            cursor_accent: theme.color("terminalCursor.background").unwrap_or(background),
            selection,
            inactive_selection: theme.color("terminal.inactiveSelectionBackground").unwrap_or(Rgba { a: selection.a * inactive_alpha, ..selection }),
            selection_foreground: theme.color("terminal.selectionForeground").or(match theme.kind {
                ThemeKind::HighContrastDark => Some(hex("#000000")),
                ThemeKind::HighContrastLight => Some(hex("#ffffff")),
                _ => None,
            }),
            border: theme.color("terminal.border").or_else(|| theme.color("panel.border")),
        }
    }
}

fn hex(text: &str) -> Rgba {
    let value = u32::from_str_radix(&text[1..], 16).expect("registry hex");
    rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    Rgba { r: r as f32 / 255., g: g as f32 / 255., b: b as f32 / 255., a: 1. }
}

fn bytes(color: Rgba) -> [u8; 3] {
    [color.r, color.g, color.b].map(|channel| (channel * 255.).round() as u8)
}

fn relative_luminance([r, g, b]: [u8; 3]) -> f64 {
    let linear = |channel: u8| {
        let value = channel as f64 / 255.;
        if value <= 0.03928 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
    };
    linear(r) * 0.2126 + linear(g) * 0.7152 + linear(b) * 0.0722
}

fn contrast_ratio(a: f64, b: f64) -> f64 {
    if a < b { (b + 0.05) / (a + 0.05) } else { (a + 0.05) / (b + 0.05) }
}

/// `rgba.ensureContrastRatio`: the foreground moved toward black or white, 10% a step, until it meets `ratio`
/// against the background; `None` when it already does.
pub fn ensure_contrast_ratio(background: Rgba, foreground: Rgba, ratio: f64) -> Option<Rgba> {
    let (bg, fg) = (bytes(background), bytes(foreground));
    let (bg_l, fg_l) = (relative_luminance(bg), relative_luminance(fg));
    if contrast_ratio(bg_l, fg_l) >= ratio {
        return None;
    }
    let reduce = |mut fg: [u8; 3]| {
        while contrast_ratio(relative_luminance(fg), bg_l) < ratio && fg != [0, 0, 0] {
            fg = fg.map(|channel| channel - (channel as f64 * 0.1).ceil() as u8);
        }
        fg
    };
    let increase = |mut fg: [u8; 3]| {
        while contrast_ratio(relative_luminance(fg), bg_l) < ratio && fg != [255, 255, 255] {
            fg = fg.map(|channel| 255.min(channel as u16 + ((255 - channel) as f64 * 0.1).ceil() as u16) as u8);
        }
        fg
    };
    let (first, second): (&dyn Fn([u8; 3]) -> [u8; 3], &dyn Fn([u8; 3]) -> [u8; 3]) = if fg_l < bg_l { (&reduce, &increase) } else { (&increase, &reduce) };
    let mut result = first(fg);
    let result_ratio = contrast_ratio(bg_l, relative_luminance(result));
    if result_ratio < ratio {
        let other = second(fg);
        if contrast_ratio(bg_l, relative_luminance(other)) >= result_ratio {
            result = other;
        }
    }
    Some(rgb(result[0], result[1], result[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_moves_the_foreground_away_from_the_background() {
        assert_eq!(ensure_contrast_ratio(hex("#1e1e1e"), hex("#cccccc"), 4.5), None);
        let adjusted = ensure_contrast_ratio(hex("#1e1e1e"), hex("#2472c8"), 4.5).unwrap();
        assert_eq!(bytes(adjusted), [78, 142, 211]);
        let darker = ensure_contrast_ratio(hex("#ffffff"), hex("#e5e510"), 4.5).unwrap();
        assert!(relative_luminance(bytes(darker)) < relative_luminance(bytes(hex("#e5e510"))));
    }
}
