//! Color themes as `ColorThemeData` resolves them: the theme file's `include` chain merged, then the
//! color registry default for every key the theme leaves out (src/vs/platform/theme/common/colorUtils.ts).

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, bail};
use exthost::scan::ScannedExtension;
use gpui::Rgba;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ThemeKind {
    Dark,
    Light,
    HighContrastDark,
}

pub struct Theme {
    pub kind: ThemeKind,
    colors: HashMap<String, Rgba>,
}

/// Finds the theme whose settings id (`id`, else `label`, as in `ColorThemeData.fromExtensionTheme`) is `settings_id`.
pub fn load_theme(extensions: &[ScannedExtension], settings_id: &str) -> anyhow::Result<Theme> {
    for extension in extensions {
        let Some(themes) = extension.description["contributes"]["themes"].as_array() else { continue };
        for theme in themes {
            let id = theme["id"].as_str().or(theme["label"].as_str());
            if id != Some(settings_id) {
                continue;
            }
            let kind = match theme["uiTheme"].as_str().unwrap_or("vs-dark") {
                "vs" => ThemeKind::Light,
                "vs-dark" => ThemeKind::Dark,
                "hc-black" => ThemeKind::HighContrastDark,
                // shortcut: no hcLight registry defaults yet, add the hcLight column to registry_default when a light HC theme is needed.
                other => bail!("theme {settings_id}: ui theme {other} is not ported"),
            };
            let path = theme["path"].as_str().context("theme without path")?;
            let mut colors = HashMap::new();
            read_colors(&extension.location.join(path), &mut colors)?;
            return Ok(Theme { kind, colors });
        }
    }
    bail!("no extension contributes the color theme {settings_id}")
}

/// Merges the included file first, so the including theme wins, as `_loadColorTheme` does.
fn read_colors(path: &Path, colors: &mut HashMap<String, Rgba>) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let theme: Value = serde_json::from_str(&strip_jsonc(&text)).with_context(|| format!("parsing {}", path.display()))?;
    if let Some(include) = theme["include"].as_str() {
        read_colors(&path.parent().unwrap().join(include), colors)?;
    }
    if let Some(entries) = theme["colors"].as_object() {
        for (key, value) in entries {
            // A null or malformed value leaves the key to the registry default, as Color.fromHex does upstream.
            if let Some(color) = value.as_str().and_then(parse_hex) {
                colors.insert(key.clone(), color);
            }
        }
    }
    Ok(())
}

impl Theme {
    /// The theme's color, else the registry default; `None` where upstream resolves to no color.
    pub fn color(&self, key: &str) -> Option<Rgba> {
        if let Some(color) = self.colors.get(key) {
            return Some(*color);
        }
        self.evaluate(&registry_default(key, self.kind))
    }

    fn evaluate(&self, value: &ColorValue) -> Option<Rgba> {
        match value {
            ColorValue::None => None,
            ColorValue::Hex(hex) => Some(parse_hex(hex).expect("registry hex")),
            ColorValue::Key(key) => self.color(key),
            ColorValue::Transparent(inner, factor) => self.evaluate(inner).map(|color| Rgba { a: color.a * factor, ..color }),
            ColorValue::Opaque(inner, background) => {
                let (color, background) = (self.evaluate(inner)?, self.evaluate(background)?);
                // Color.makeOpaque rounds each channel to a byte.
                let blend = |c: f32, b: f32| ((b + color.a * (c - b)) * 255.).round() / 255.;
                Some(Rgba { r: blend(color.r, background.r), g: blend(color.g, background.g), b: blend(color.b, background.b), a: 1. })
            }
        }
    }
}

enum ColorValue {
    None,
    Hex(&'static str),
    Key(&'static str),
    Transparent(&'static ColorValue, f32),
    Opaque(&'static ColorValue, &'static ColorValue),
}

/// Registry defaults of the keys the workbench shell paints, copied from their `registerColor` calls
/// (theme.ts, baseColors.ts, editorColors.ts). Keys not listed resolve to no color.
fn registry_default(key: &str, kind: ThemeKind) -> ColorValue {
    use ColorValue::*;
    let (dark, light, hc) = match key {
        "foreground" => (Hex("#CCCCCC"), Hex("#616161"), Hex("#FFFFFF")),
        "contrastBorder" => (None, None, Hex("#6FC3DF")),
        "disabledForeground" => (Hex("#CCCCCC80"), Hex("#61616180"), Hex("#A5A5A5")),
        "icon.foreground" => (Hex("#C5C5C5"), Hex("#424242"), Hex("#FFFFFF")),
        "editor.background" => (Hex("#1E1E1E"), Hex("#ffffff"), Hex("#000000")),
        "sideBar.background" => (Hex("#252526"), Hex("#F3F3F3"), Hex("#000000")),
        "window.activeBorder" => (None, None, Key("contrastBorder")),
        "titleBar.activeBackground" => (Hex("#3C3C3C"), Hex("#DDDDDD"), Hex("#000000")),
        "titleBar.activeForeground" => (Hex("#CCCCCC"), Hex("#333333"), Hex("#FFFFFF")),
        "commandCenter.foreground" => (Key("titleBar.activeForeground"), Key("titleBar.activeForeground"), Key("titleBar.activeForeground")),
        "commandCenter.background" => (Hex("#FFFFFF0D"), Hex("#0000000D"), None),
        "commandCenter.border" => (
            Transparent(&Key("titleBar.activeForeground"), 0.2),
            Transparent(&Key("titleBar.activeForeground"), 0.2),
            Key("contrastBorder"),
        ),
        "activityBar.background" => (Hex("#333333"), Hex("#2C2C2C"), Hex("#000000")),
        "activityBar.foreground" => (Hex("#FFFFFF"), Hex("#FFFFFF"), Hex("#FFFFFF")),
        "activityBar.inactiveForeground" => (
            Transparent(&Key("activityBar.foreground"), 0.4),
            Transparent(&Key("activityBar.foreground"), 0.4),
            Hex("#FFFFFF"),
        ),
        "surface.background" => (Key("sideBar.background"), Key("editor.background"), Key("sideBar.background")),
        "surface.border" => (
            Opaque(&Transparent(&Key("foreground"), 0.15), &Key("surface.background")),
            Opaque(&Transparent(&Key("foreground"), 0.15), &Key("surface.background")),
            Key("contrastBorder"),
        ),
        "statusBar.foreground" => (Hex("#FFFFFF"), Hex("#FFFFFF"), Hex("#FFFFFF")),
        "statusBar.noFolderForeground" => (Key("statusBar.foreground"), Key("statusBar.foreground"), Key("statusBar.foreground")),
        "statusBar.noFolderBackground" => (Hex("#68217A"), Hex("#68217A"), None),
        _ => (None, None, None),
    };
    match kind {
        ThemeKind::Dark => dark,
        ThemeKind::Light => light,
        ThemeKind::HighContrastDark => hc,
    }
}

/// `#RGB`, `#RGBA`, `#RRGGBB` or `#RRGGBBAA`, as `Color.Format.CSS.parseHex` accepts.
fn parse_hex(text: &str) -> Option<Rgba> {
    let digits = text.strip_prefix('#')?;
    let expanded: String = match digits.len() {
        3 | 4 => digits.chars().flat_map(|c| [c, c]).collect(),
        6 | 8 => digits.to_string(),
        _ => return Option::None,
    };
    let byte = |i: usize| u8::from_str_radix(expanded.get(i..i + 2)?, 16).ok().map(|b| b as f32 / 255.);
    let a = if expanded.len() == 8 { byte(6)? } else { 1. };
    Some(Rgba { r: byte(0)?, g: byte(2)?, b: byte(4)?, a })
}

/// Theme files are JSON with comments and trailing commas; drops both outside strings.
fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                out.push(c);
                while let Some(c) = chars.next() {
                    out.push(c);
                    match c {
                        '\\' => out.extend(chars.next()),
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                while chars.next_if(|&c| c != '\n').is_some() {}
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = ' ';
                for c in chars.by_ref() {
                    if previous == '*' && c == '/' {
                        break;
                    }
                    previous = c;
                }
            }
            '}' | ']' => {
                let trimmed = out.trim_end().len();
                if out[..trimmed].ends_with(',') {
                    out.truncate(trimmed - 1);
                }
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use exthost::scan::scan_extension;

    fn hex(color: Rgba) -> String {
        let byte = |v: f32| (v * 255.).round() as u8;
        format!("#{:02x}{:02x}{:02x}{:02x}", byte(color.r), byte(color.g), byte(color.b), byte(color.a))
    }

    fn builtin_theme(id: &str) -> Theme {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../extensions/theme-defaults");
        load_theme(&[scan_extension(&folder, true).unwrap()], id).unwrap()
    }

    #[test]
    fn strips_comments_and_trailing_commas_outside_strings() {
        let text = "{ // c\n \"a\": \"//x,}\", /* b */ \"c\": [1, 2,], }";
        let value: Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
        assert_eq!(value, serde_json::json!({ "a": "//x,}", "c": [1, 2] }));
    }

    // Expected values: the reference workbench's --vscode-* variables per theme (port/results/pixels.md method).
    #[test]
    fn resolves_like_the_reference_workbench() {
        let cases = [
            ("Dark Modern", "surface.border", "#252526ff"),
            ("Dark Modern", "commandCenter.background", "#ffffff0d"),
            ("Dark Modern", "commandCenter.border", "#cccccc33"),
            ("Dark Modern", "statusBar.noFolderBackground", "#1f1f1fff"),
            ("Dark Modern", "activityBar.inactiveForeground", "#868686ff"),
            ("Light Modern", "surface.border", "#e5e5e5ff"),
            ("Light Modern", "commandCenter.border", "#1e1e1e33"),
            ("Light Modern", "editor.background", "#ffffffff"),
            ("Default High Contrast", "window.activeBorder", "#6fc3dfff"),
            ("Default High Contrast", "commandCenter.border", "#6fc3dfff"),
            ("Default High Contrast", "statusBar.noFolderForeground", "#ffffffff"),
        ];
        for (theme, key, expected) in cases {
            assert_eq!(builtin_theme(theme).color(key).map(hex).as_deref(), Some(expected), "{theme} {key}");
        }
        assert_eq!(builtin_theme("Default High Contrast").color("commandCenter.background"), None);
        assert_eq!(builtin_theme("Default High Contrast").color("statusBar.noFolderBackground"), None);
    }
}
