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
    HighContrastLight,
}

impl ThemeKind {
    /// `isHighContrast` (platform/theme/common/theme.ts): the `.hc-black` and `.hc-light` CSS rules.
    pub fn is_high_contrast(self) -> bool {
        matches!(self, ThemeKind::HighContrastDark | ThemeKind::HighContrastLight)
    }
}

pub struct Theme {
    pub kind: ThemeKind,
    colors: HashMap<String, Rgba>,
    /// The TextMate theme of the token colors (`ColorThemeData.tokenColors`).
    pub tokens: textmate::Theme,
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
                "hc-light" => ThemeKind::HighContrastLight,
                other => bail!("theme {settings_id}: unknown ui theme {other}"),
            };
            let path = theme["path"].as_str().context("theme without path")?;
            let (mut colors, mut token_colors) = (HashMap::new(), Vec::new());
            read_colors(&extension.location.join(path), &mut colors, &mut token_colors)?;
            let mut theme = Theme { kind, colors, tokens: textmate::Theme::new(&[]) };
            theme.tokens = crate::syntax::token_theme(&theme, &token_colors);
            return Ok(theme);
        }
    }
    bail!("no extension contributes the color theme {settings_id}")
}

/// Merges the included file first, so the including theme wins, as `_loadColorTheme` does.
/// shortcut: `tokenColors` as a path to a .tmTheme file is not read; no built-in theme uses one, port it with theme extensions from the Marketplace.
fn read_colors(path: &Path, colors: &mut HashMap<String, Rgba>, token_colors: &mut Vec<Value>) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let theme: Value = serde_json::from_str(&strip_jsonc(&text)).with_context(|| format!("parsing {}", path.display()))?;
    if let Some(include) = theme["include"].as_str() {
        read_colors(&path.parent().unwrap().join(include), colors, token_colors)?;
    }
    token_colors.extend(theme["tokenColors"].as_array().into_iter().flatten().cloned());
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
    if kind == ThemeKind::HighContrastLight {
        return hc_light_default(key);
    }
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
        "statusBar.background" => (Hex("#007ACC"), Hex("#007ACC"), None),
        "focusBorder" => (Hex("#007FD4"), Hex("#0090F1"), Hex("#F38518")),
        "contrastActiveBorder" => (None, None, Key("focusBorder")),
        "sideBar.border" => (None, None, Key("contrastBorder")),
        "sideBarTitle.foreground" => (Key("sideBar.foreground"), Key("sideBar.foreground"), Key("sideBar.foreground")),
        "sideBarTitle.border" => (None, None, Key("sideBar.border")),
        "sideBarSectionHeader.background" => (Hex("#80808033"), Hex("#80808033"), None),
        "sideBarSectionHeader.foreground" => (Key("sideBar.foreground"), Key("sideBar.foreground"), Key("sideBar.foreground")),
        "sideBarSectionHeader.border" => (Key("contrastBorder"), Key("contrastBorder"), Key("contrastBorder")),
        "list.inactiveSelectionBackground" => (Hex("#37373D"), Hex("#E4E6F1"), None),
        "modernTab.activeBackground" => (Key("list.inactiveSelectionBackground"), Key("list.inactiveSelectionBackground"), Key("list.inactiveSelectionBackground")),
        // oneOf(list.inactiveSelectionForeground, foreground), and the first has no default.
        "modernTab.activeForeground" => (Key("foreground"), Key("foreground"), Key("foreground")),
        "modernActivityBarItem.activeBackground" => (Key("modernTab.activeBackground"), Key("modernTab.activeBackground"), Key("modernTab.activeBackground")),
        "modernActivityBarItem.activeForeground" => (Key("modernTab.activeForeground"), Key("modernTab.activeForeground"), Key("modernTab.activeForeground")),
        "editor.foreground" => (Hex("#BBBBBB"), Hex("#333333"), Hex("#FFFFFF")),
        "editorLineNumber.foreground" => (Hex("#858585"), Hex("#237893"), Hex("#FFFFFF")),
        "editorActiveLineNumber.foreground" => (Hex("#c6c6c6"), Hex("#0B216F"), Key("contrastActiveBorder")),
        "editorLineNumber.activeForeground" => (
            Key("editorActiveLineNumber.foreground"),
            Key("editorActiveLineNumber.foreground"),
            Key("editorActiveLineNumber.foreground"),
        ),
        "editor.selectionBackground" => (Hex("#264F78"), Hex("#ADD6FF"), Hex("#f3f518")),
        "editor.selectionForeground" => (None, None, Hex("#000000")),
        "editor.inactiveSelectionBackground" => (
            Transparent(&Key("editor.selectionBackground"), 0.5),
            Transparent(&Key("editor.selectionBackground"), 0.5),
            Transparent(&Key("editor.selectionBackground"), 0.7),
        ),
        "editor.lineHighlightBorder" => (Hex("#282828"), Hex("#eeeeee"), Hex("#f38518")),
        "editorCursor.foreground" => (Hex("#AEAFAD"), Hex("#000000"), Hex("#FFFFFF")),
        "editor.wordHighlightBackground" => (Hex("#575757B8"), Hex("#57575740"), None),
        "editor.wordHighlightTextBackground" => (Key("editor.wordHighlightBackground"), Key("editor.wordHighlightBackground"), Key("editor.wordHighlightBackground")),
        "editor.wordHighlightBorder" => (None, None, Key("contrastActiveBorder")),
        "editor.wordHighlightTextBorder" => (Key("editor.wordHighlightBorder"), Key("editor.wordHighlightBorder"), Key("editor.wordHighlightBorder")),
        "editorWhitespace.foreground" => (Hex("#e3e4e229"), Hex("#33333333"), Hex("#e3e4e229")),
        "editorIndentGuide.background" => (Key("editorWhitespace.foreground"), Key("editorWhitespace.foreground"), Key("editorWhitespace.foreground")),
        "editorIndentGuide.background1" => (Key("editorIndentGuide.background"), Key("editorIndentGuide.background"), Key("editorIndentGuide.background")),
        "editorOverviewRuler.border" => (Hex("#7f7f7f4d"), Hex("#7f7f7f4d"), Hex("#7f7f7f4d")),
        "editorOverviewRuler.selectionHighlightForeground" => (Hex("#A0A0A0CC"), Hex("#A0A0A0CC"), Hex("#A0A0A0CC")),
        "editorOverviewRuler.wordHighlightTextForeground" => (
            Key("editorOverviewRuler.selectionHighlightForeground"),
            Key("editorOverviewRuler.selectionHighlightForeground"),
            Key("editorOverviewRuler.selectionHighlightForeground"),
        ),
        "breadcrumb.foreground" => (Transparent(&Key("foreground"), 0.8), Transparent(&Key("foreground"), 0.8), Transparent(&Key("foreground"), 0.8)),
        "breadcrumb.background" => (Key("editor.background"), Key("editor.background"), Key("editor.background")),
        "editorGroupHeader.tabsBackground" => (Hex("#252526"), Hex("#F3F3F3"), None),
        "editorGroupHeader.connectedTabsBackground" => (
            Key("editorGroupHeader.tabsBackground"),
            Key("editorGroupHeader.tabsBackground"),
            Key("editorGroupHeader.tabsBackground"),
        ),
        "tab.border" => (Hex("#252526"), Hex("#F3F3F3"), Key("contrastBorder")),
        "editorBracketMatch.background" => (Hex("#0064001A"), Hex("#0064001A"), Hex("#0064001A")),
        "editorBracketMatch.border" => (Hex("#888"), Hex("#B9B9B9"), Key("contrastBorder")),
        "editorOverviewRuler.bracketMatchForeground" => (Hex("#A0A0A0"), Hex("#A0A0A0"), Hex("#A0A0A0")),
        "editorBracketHighlight.foreground1" => (Hex("#FFD700"), Hex("#0431FAFF"), Hex("#FFD700")),
        "editorBracketHighlight.foreground2" => (Hex("#DA70D6"), Hex("#319331FF"), Hex("#DA70D6")),
        "editorBracketHighlight.foreground3" => (Hex("#179FFF"), Hex("#7B3814FF"), Hex("#87CEFA")),
        "editorBracketHighlight.unexpectedBracket.foreground" => (Hex("#FF1212CC"), Hex("#FF1212CC"), Hex("#FF3232")),
        _ => (None, None, None),
    };
    match kind {
        ThemeKind::Dark => dark,
        ThemeKind::Light => light,
        ThemeKind::HighContrastDark => hc,
        ThemeKind::HighContrastLight => unreachable!(),
    }
}

/// The `hcLight` column of the keys `registry_default` lists; a key missing here resolves to no color.
fn hc_light_default(key: &str) -> ColorValue {
    use ColorValue::*;
    match key {
        "foreground" => Hex("#292929"),
        "contrastBorder" => Hex("#0F4A85"),
        "disabledForeground" => Hex("#7F7F7F"),
        "icon.foreground" => Hex("#292929"),
        "editor.background" => Hex("#ffffff"),
        "sideBar.background" => Hex("#FFFFFF"),
        "window.activeBorder" => Key("contrastBorder"),
        "titleBar.activeBackground" => Hex("#FFFFFF"),
        "titleBar.activeForeground" => Hex("#292929"),
        "commandCenter.foreground" => Key("titleBar.activeForeground"),
        "commandCenter.border" => Key("contrastBorder"),
        "activityBar.background" => Hex("#FFFFFF"),
        "activityBar.foreground" => Key("editor.foreground"),
        "activityBar.inactiveForeground" => Key("editor.foreground"),
        "surface.background" => Key("sideBar.background"),
        "surface.border" => Key("contrastBorder"),
        "statusBar.foreground" => Key("editor.foreground"),
        "statusBar.noFolderForeground" => Key("statusBar.foreground"),
        "focusBorder" => Hex("#006BBD"),
        "editorBracketMatch.background" => Hex("#0000"),
        "editorBracketMatch.border" => Key("contrastBorder"),
        "editorOverviewRuler.bracketMatchForeground" => Hex("#A0A0A0"),
        "editorBracketHighlight.foreground1" => Hex("#0431FAFF"),
        "editorBracketHighlight.foreground2" => Hex("#319331FF"),
        "editorBracketHighlight.foreground3" => Hex("#7B3814FF"),
        "editorBracketHighlight.unexpectedBracket.foreground" => Hex("#B5200D"),
        "contrastActiveBorder" => Key("focusBorder"),
        "sideBar.border" => Key("contrastBorder"),
        "sideBarTitle.foreground" => Key("sideBar.foreground"),
        "sideBarTitle.border" => Key("sideBar.border"),
        "sideBarSectionHeader.foreground" => Key("sideBar.foreground"),
        "sideBarSectionHeader.border" => Key("contrastBorder"),
        "list.inactiveSelectionBackground" => Hex("#0F4A851A"),
        "modernTab.activeBackground" => Key("list.inactiveSelectionBackground"),
        "modernTab.activeForeground" => Key("foreground"),
        "modernActivityBarItem.activeBackground" => Key("modernTab.activeBackground"),
        "modernActivityBarItem.activeForeground" => Key("modernTab.activeForeground"),
        "editor.foreground" => Key("foreground"),
        "editorLineNumber.foreground" => Hex("#292929"),
        "editorActiveLineNumber.foreground" => Key("contrastActiveBorder"),
        "editorLineNumber.activeForeground" => Key("editorActiveLineNumber.foreground"),
        "editor.lineHighlightBorder" => Key("contrastBorder"),
        "editorCursor.foreground" => Hex("#0F4A85"),
        "editor.wordHighlightBorder" => Key("contrastActiveBorder"),
        "editor.wordHighlightTextBorder" => Key("editor.wordHighlightBorder"),
        "editorWhitespace.foreground" => Hex("#CCCCCC"),
        "editorIndentGuide.background" => Key("editorWhitespace.foreground"),
        "editorIndentGuide.background1" => Key("editorIndentGuide.background"),
        "editorOverviewRuler.border" => Hex("#666666"),
        "editorOverviewRuler.selectionHighlightForeground" => Hex("#A0A0A0CC"),
        "editorOverviewRuler.wordHighlightTextForeground" => Key("editorOverviewRuler.selectionHighlightForeground"),
        "breadcrumb.foreground" => Transparent(&Key("foreground"), 0.8),
        "breadcrumb.background" => Key("editor.background"),
        "tab.border" => Key("contrastBorder"),
        _ => None,
    }
}

/// `#RGB`, `#RGBA`, `#RRGGBB` or `#RRGGBBAA`, as `Color.Format.CSS.parseHex` accepts.
pub fn parse_hex(text: &str) -> Option<Rgba> {
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
pub fn strip_jsonc(text: &str) -> String {
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
            ("Dark Modern", "modernActivityBarItem.activeBackground", "#ffffff22"),
            ("Dark Modern", "modernActivityBarItem.activeForeground", "#ccccccff"),
            ("Light Modern", "sideBarSectionHeader.border", "#e5e5e5ff"),
            ("Default High Contrast", "contrastActiveBorder", "#f38518ff"),
            ("Default High Contrast", "sideBarSectionHeader.border", "#6fc3dfff"),
            ("Default High Contrast", "sideBarTitle.border", "#6fc3dfff"),
            ("Dark Modern", "editorLineNumber.activeForeground", "#ccccccff"),
            ("Dark Modern", "editor.lineHighlightBorder", "#282828ff"),
            ("Dark Modern", "editor.wordHighlightTextBackground", "#575757b8"),
            ("Dark Modern", "editorOverviewRuler.wordHighlightTextForeground", "#a0a0a0cc"),
            ("Dark Modern", "breadcrumb.foreground", "#cccccccc"),
            ("Dark Modern", "editorGroupHeader.connectedTabsBackground", "#2b2b2bff"),
            ("Light Modern", "editor.wordHighlightTextBackground", "#57575740"),
            ("Light Modern", "editorLineNumber.activeForeground", "#171184ff"),
            ("Light Modern", "editorCursor.foreground", "#000000ff"),
            ("Default High Contrast", "editorLineNumber.activeForeground", "#f38518ff"),
            ("Default High Contrast", "editor.wordHighlightTextBorder", "#f38518ff"),
            ("Default High Contrast", "editorOverviewRuler.border", "#7f7f7f4d"),
            ("Default High Contrast", "editorLineNumber.foreground", "#ffffffff"),
            ("Default High Contrast", "breadcrumb.foreground", "#ffffffcc"),
        ];
        for (theme, key, expected) in cases {
            assert_eq!(builtin_theme(theme).color(key).map(hex).as_deref(), Some(expected), "{theme} {key}");
        }
        assert_eq!(builtin_theme("Default High Contrast").color("commandCenter.background"), None);
        assert_eq!(builtin_theme("Default High Contrast").color("statusBar.noFolderBackground"), None);
        assert_eq!(builtin_theme("Default High Contrast").color("statusBar.background"), None);
        assert_eq!(builtin_theme("Default High Contrast").color("modernActivityBarItem.activeBackground"), None);
        assert_eq!(builtin_theme("Default High Contrast").color("editor.wordHighlightTextBackground"), None);
        assert_eq!(builtin_theme("Default High Contrast").color("editorGroupHeader.connectedTabsBackground"), None);
    }

    #[test]
    fn every_builtin_theme_loads_with_token_colors() {
        let extensions = exthost::scan::scan_builtin_extensions(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../extensions")).unwrap();
        let mut loaded = 0;
        for extension in &extensions {
            for contribution in extension.description["contributes"]["themes"].as_array().into_iter().flatten() {
                let id = contribution["id"].as_str().or(contribution["label"].as_str()).unwrap();
                let theme = load_theme(&extensions, id).unwrap_or_else(|error| panic!("{id}: {error:#}"));
                // Index 0 is unused, 1 and 2 are the editor colors of the default rule.
                assert!(theme.tokens.color_map().len() > 3, "{id} has no token colors");
                loaded += 1;
            }
        }
        assert_eq!(loaded, 19);
        assert_eq!(builtin_theme("Default High Contrast Light").kind, ThemeKind::HighContrastLight);
    }
}
