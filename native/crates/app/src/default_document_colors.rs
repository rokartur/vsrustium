//! defaultDocumentColorsComputer.ts: the colors the editor worker finds in any text (`#rgb`, `#rgba`, `#rrggbb`,
//! `#rrggbbaa` at a line start or after a quote or whitespace; `rgb()`, `rgba()`, `hsl()`, `hsla()` with comma or
//! CSS Level 4 syntax), and defaultDocumentColorProvider.ts's presentations of a color.

use crate::editor::Position;

/// One found color: its range on one line (char columns) and its channels from 0 to 1.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorInformation {
    pub start: Position,
    pub end: Position,
    pub color: [f64; 4],
}

const INITIAL_VALIDATION: &str = r#"\b(rgb|rgba|hsl|hsla)(\([0-9\s,.\%\/]*\))|^(#)([A-Fa-f0-9]{3})\b|^(#)([A-Fa-f0-9]{4})\b|^(#)([A-Fa-f0-9]{6})\b|^(#)([A-Fa-f0-9]{8})\b|(?<=['"\s])(#)([A-Fa-f0-9]{3})\b|(?<=['"\s])(#)([A-Fa-f0-9]{4})\b|(?<=['"\s])(#)([A-Fa-f0-9]{6})\b|(?<=['"\s])(#)([A-Fa-f0-9]{8})\b"#;
const RGB: &str = r"^\(\s*(25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9][0-9]|[0-9])\s*[\s,]\s*(25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9][0-9]|[0-9])\s*[\s,]\s*(25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9][0-9]|[0-9])\s*\)$";
const RGBA: &str = r"^\(\s*(25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9][0-9]|[0-9])\s*[\s,]\s*(25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9][0-9]|[0-9])\s*[\s,]\s*(25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9][0-9]|[0-9])\s*(?:[\s,]|[\s]*\/)\s*(0[.][0-9]+|[.][0-9]+|[01][.]|[01])\s*\)$";
const HSL: &str = r"^\(\s*((?:360(?:\.0+)?|(?:36[0]|3[0-5][0-9]|[12][0-9][0-9]|[1-9]?[0-9])(?:\.\d+)?))\s*[\s,]\s*(100(?:\.0+)?|\d{1,2}[.]\d*|\d{1,2})%\s*[\s,]\s*(100(?:\.0+)?|\d{1,2}[.]\d*|\d{1,2})%\s*\)$";
const HSLA: &str = r"^\(\s*((?:360(?:\.0+)?|(?:36[0]|3[0-5][0-9]|[12][0-9][0-9]|[1-9]?[0-9])(?:\.\d+)?))\s*[\s,]\s*(100(?:\.0+)?|\d{1,2}[.]\d*|\d{1,2})%\s*[\s,]\s*(100(?:\.0+)?|\d{1,2}[.]\d*|\d{1,2})%\s*(?:[\s,]|[\s]*\/)\s*(0[.][0-9]+|[.][0-9]+|[01][.]0*|[01])\s*\)$";

/// `computeDefaultDocumentColors` over the model's lines joined by `\n` (the regexes run on the whole text).
pub fn compute_default_document_colors(lines: &[String]) -> Vec<ColorInformation> {
    let text = lines.join("\n");
    let initial = onig::Regex::new(INITIAL_VALIDATION).expect("the upstream regex compiles");
    let mut result = Vec::new();
    for captures in initial.captures_iter(&text) {
        // `initialMatch.filter(captureGroup => captureGroup !== undefined)`: the match, the scheme, the parameters.
        let groups: Vec<&str> = (0..captures.len()).filter_map(|index| captures.at(index)).collect();
        let (Some(&scheme), Some(&parameters)) = (groups.get(1), groups.get(2)) else { continue };
        let (start_byte, end_byte) = captures.pos(0).expect("group 0 is the match");
        let start = position_at(&text, start_byte);
        let end = Position::new(start.line, start.column + text[start_byte..end_byte].chars().count());
        let color = match scheme {
            "rgb" => rgb(parameters, RGB, false),
            "rgba" => rgb(parameters, RGBA, true),
            "hsl" => hsl(parameters, HSL, false),
            "hsla" => hsl(parameters, HSLA, true),
            _ => crate::theme::parse_hex(&format!("#{parameters}")).map(|color| [round_byte(color.r) / 255., round_byte(color.g) / 255., round_byte(color.b) / 255., color.a as f64]),
        };
        if let Some(color) = color {
            result.push(ColorInformation { start, end, color });
        }
    }
    result
}

fn round_byte(channel: f32) -> f64 {
    (channel as f64 * 255.).round()
}

fn position_at(text: &str, byte: usize) -> Position {
    let before = &text[..byte];
    let line = before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    Position::new(line, before[line_start..].chars().count())
}

/// The numbers among the parameter regex's groups (`_parseCaptureGroups`: the whole match is not a number).
fn parameter_values(parameters: &str, pattern: &str) -> Option<Vec<f64>> {
    let regex = onig::Regex::new(pattern).expect("the upstream regex compiles");
    let mut matches = regex.captures_iter(parameters);
    let captures = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some((0..captures.len()).filter_map(|index| captures.at(index)).filter_map(|group| group.trim().parse::<f64>().ok().or_else(|| group.parse::<f64>().ok())).collect())
}

fn rgb(parameters: &str, pattern: &str, alpha: bool) -> Option<[f64; 4]> {
    let values = parameter_values(parameters, pattern)?;
    Some([values[0] / 255., values[1] / 255., values[2] / 255., if alpha { values[3] } else { 1. }])
}

fn hsl(parameters: &str, pattern: &str, alpha: bool) -> Option<[f64; 4]> {
    let values = parameter_values(parameters, pattern)?;
    let color = crate::theme::from_hsla(values[0], values[1] / 100., values[2] / 100., if alpha { values[3] } else { 1. });
    Some([round_byte(color.r) / 255., round_byte(color.g) / 255., round_byte(color.b) / 255., color.a as f64])
}

/// `DefaultDocumentColorProvider.provideColorPresentations`: `Color.Format.CSS` rgb, hsl and hex labels.
pub fn default_color_presentations(color: [f64; 4]) -> Vec<String> {
    vec![format_rgb(color), format_hsl(color), format_hex_a(color)]
}

fn bytes(color: [f64; 4]) -> (u8, u8, u8) {
    let byte = |value: f64| (value * 255.).round().clamp(0., 255.) as u8;
    (byte(color[0]), byte(color[1]), byte(color[2]))
}

/// `Color.Format.CSS.formatRGB`, or `formatRGBA` when not opaque (alpha to 2 decimals).
fn format_rgb(color: [f64; 4]) -> String {
    let (r, g, b) = bytes(color);
    match color[3] == 1. {
        true => format!("rgb({r}, {g}, {b})"),
        false => format!("rgba({r}, {g}, {b}, {})", trim_number(color[3], 2)),
    }
}

/// `Color.Format.CSS.formatHSL`, or `formatHSLA`: hue whole, saturation and lightness whole percents.
fn format_hsl(color: [f64; 4]) -> String {
    let (h, s, l) = to_hsl(color);
    let (s, l) = ((s * 100.).round(), (l * 100.).round());
    match color[3] == 1. {
        true => format!("hsl({h}, {s}%, {l}%)"),
        false => format!("hsla({h}, {s}%, {l}%, {:.2})", (color[3] * 1000.).round() / 1000.),
    }
}

/// `Color.Format.CSS.formatHexA(color, true)`: `#rrggbb` when opaque, else `#rrggbbaa`.
fn format_hex_a(color: [f64; 4]) -> String {
    let (r, g, b) = bytes(color);
    match color[3] == 1. {
        true => format!("#{r:02x}{g:02x}{b:02x}"),
        false => format!("#{r:02x}{g:02x}{b:02x}{:02x}", (color[3] * 255.).round() as u8),
    }
}

/// `HSLA.fromRGBA` with its rounding: hue whole, saturation and lightness to 3 decimals.
fn to_hsl(color: [f64; 4]) -> (f64, f64, f64) {
    let (r, g, b) = bytes(color);
    let (r, g, b) = (r as f64 / 255., g as f64 / 255., b as f64 / 255.);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (min + max) / 2.;
    let chroma = max - min;
    if chroma == 0. {
        return (0., 0., (l * 1000.).round() / 1000.);
    }
    let s = (if l <= 0.5 { chroma / (2. * l) } else { chroma / (2. - 2. * l) }).min(1.);
    let h = match max {
        _ if max == r => (g - b) / chroma + if g < b { 6. } else { 0. },
        _ if max == g => (b - r) / chroma + 2.,
        _ => (r - g) / chroma + 4.,
    };
    ((h * 60.).round(), (s * 1000.).round() / 1000., (l * 1000.).round() / 1000.)
}

/// `+value.toFixed(digits)`: trailing zeros dropped.
fn trim_number(value: f64, digits: usize) -> String {
    let fixed = format!("{value:.digits$}");
    fixed.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests;
