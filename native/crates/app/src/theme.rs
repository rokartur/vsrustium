//! Color themes as `ColorThemeData` resolves them: the theme file's `include` chain merged, then the
//! color registry default for every key the theme leaves out (src/vs/platform/theme/common/colorUtils.ts).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

    /// `isDark` negated: the light variant of an icon path applies.
    pub fn is_light(self) -> bool {
        matches!(self, ThemeKind::Light | ThemeKind::HighContrastLight)
    }
}

pub struct Theme {
    pub kind: ThemeKind,
    /// `IWorkbenchColorTheme.settingsId`, the `workbench.colorTheme` value.
    pub settings_id: String,
    colors: HashMap<String, Rgba>,
    /// The TextMate theme of the token colors (`ColorThemeData.tokenColors`).
    pub tokens: textmate::Theme,
}

/// A contributed color theme before its file is read (`ColorThemeData.fromExtensionTheme`).
#[derive(Clone, Debug)]
pub struct ColorThemeEntry {
    pub settings_id: String,
    pub label: String,
    pub description: Option<String>,
    pub kind: ThemeKind,
    pub extension_id: String,
    path: PathBuf,
}

impl ColorThemeEntry {
    pub fn load(&self) -> anyhow::Result<Theme> {
        let (mut colors, mut token_colors) = (HashMap::new(), Vec::new());
        read_colors(&self.path, &mut colors, &mut token_colors)?;
        let mut theme = Theme { kind: self.kind, settings_id: self.settings_id.clone(), colors, tokens: textmate::Theme::new(&[]) };
        theme.tokens = crate::syntax::token_theme(&theme, &token_colors);
        Ok(theme)
    }
}

/// `contributes.themes` of every extension (`ThemeRegistry.onExtensionsRead`); a contribution without a path is skipped.
pub fn color_themes(extensions: &[ScannedExtension]) -> anyhow::Result<Vec<ColorThemeEntry>> {
    let mut entries = Vec::new();
    for extension in extensions {
        let Some(themes) = extension.description["contributes"]["themes"].as_array() else { continue };
        for theme in themes {
            let Some(path) = theme["path"].as_str() else { continue };
            let label = theme["label"].as_str().map_or_else(|| file_name(path), str::to_owned);
            let settings_id = theme["id"].as_str().map_or_else(|| label.clone(), str::to_owned);
            let kind = match theme["uiTheme"].as_str().unwrap_or("vs-dark") {
                "vs" => ThemeKind::Light,
                "vs-dark" => ThemeKind::Dark,
                "hc-black" => ThemeKind::HighContrastDark,
                "hc-light" => ThemeKind::HighContrastLight,
                other => bail!("theme {settings_id}: unknown ui theme {other}"),
            };
            let description = theme["description"].as_str().map(str::to_owned);
            entries.push(ColorThemeEntry { settings_id, label, description, kind, extension_id: extension.id.clone(), path: extension.location.join(path) });
        }
    }
    Ok(entries)
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(|| path.to_owned(), |name| name.to_string_lossy().into_owned())
}

/// Finds the theme whose settings id (`id`, else `label`, as in `ColorThemeData.fromExtensionTheme`) is `settings_id`.
pub fn load_theme(extensions: &[ScannedExtension], settings_id: &str) -> anyhow::Result<Theme> {
    let themes = color_themes(extensions)?;
    let entry = themes.iter().find(|entry| entry.settings_id == settings_id).with_context(|| format!("no extension contributes the color theme {settings_id}"))?;
    entry.load()
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
            ColorValue::Darken(inner, factor) => self.evaluate(inner).map(|color| darken(color, *factor)),
            ColorValue::Opaque(inner, background) => {
                let (color, background) = (self.evaluate(inner)?, self.evaluate(background)?);
                // Color.makeOpaque rounds each channel to a byte.
                let blend = |c: f32, b: f32| ((b + color.a * (c - b)) * 255.).round() / 255.;
                Some(Rgba { r: blend(color.r, background.r), g: blend(color.g, background.g), b: blend(color.b, background.b), a: 1. })
            }
            ColorValue::LessProminent(inner, background, factor, transparency) => {
                let color = self.evaluate(inner)?;
                let Some(background) = self.evaluate(background) else {
                    return Some(Rgba { a: color.a * factor * transparency, ..color });
                };
                Some(less_prominent(color, background, *factor, *transparency))
            }
        }
    }
}

/// `Color.getRelativeLuminance`, rounded to 4 decimals as upstream does.
pub fn relative_luminance(color: Rgba) -> f64 {
    let component = |value: f32| {
        let c = (value as f64 * 255.).round() / 255.;
        if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    let luminance = 0.2126 * component(color.r) + 0.7152 * component(color.g) + 0.0722 * component(color.b);
    (luminance * 10_000.).round() / 10_000.
}

/// colorUtils.ts `lessProminent`: `Color.getLighterColor` or `getDarkerColor` relative to the background, then
/// `transparent`, through `Color`'s HSLA with its rounding (hue to an integer, the rest to 3 decimals).
fn less_prominent(color: Rgba, background: Rgba, factor: f32, transparency: f32) -> Rgba {
    let (h, s, l, a) = hsla(color);
    let (lum, background_lum) = (relative_luminance(color), relative_luminance(background));
    let l = match lum < background_lum {
        true => round3(l + l * factor as f64 * (background_lum - lum) / background_lum),
        false => round3(l - l * factor as f64 * (lum - background_lum) / lum),
    };
    from_hsla(h, s, l, round3(a * transparency as f64))
}

/// `Color.darken`: the HSLA lightness less `factor` of itself.
fn darken(color: Rgba, factor: f32) -> Rgba {
    let (h, s, l, a) = hsla(color);
    from_hsla(h, s, round3(l - l * factor as f64), a)
}

fn round3(value: f64) -> f64 {
    (value.clamp(0., 1.) * 1000.).round() / 1000.
}

/// `HSLA.fromRGBA`: hue in whole degrees, the rest to 3 decimals.
fn hsla(color: Rgba) -> (f64, f64, f64, f64) {
    let (r, g, b) = ((color.r as f64 * 255.).round() / 255., (color.g as f64 * 255.).round() / 255., (color.b as f64 * 255.).round() / 255.);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (min + max) / 2.;
    let chroma = max - min;
    let (mut h, mut s) = (0., 0.);
    if chroma > 0. {
        s = (if l <= 0.5 { chroma / (2. * l) } else { chroma / (2. - 2. * l) }).min(1.);
        h = match max {
            _ if max == r => (g - b) / chroma + if g < b { 6. } else { 0. },
            _ if max == g => (b - r) / chroma + 2.,
            _ => (r - g) / chroma + 4.,
        };
        h = (h * 60.).round();
    }
    (h, round3(s), round3(l), round3(color.a as f64))
}

/// `HSLA.toRGBA`, each channel rounded to a byte.
fn from_hsla(h: f64, s: f64, l: f64, a: f64) -> Rgba {
    let hue = |p: f64, q: f64, mut t: f64| {
        if t < 0. {
            t += 1.;
        }
        if t > 1. {
            t -= 1.;
        }
        match t {
            _ if t < 1. / 6. => p + (q - p) * 6. * t,
            _ if t < 1. / 2. => q,
            _ if t < 2. / 3. => p + (q - p) * (2. / 3. - t) * 6.,
            _ => p,
        }
    };
    let h = h.clamp(0., 360.).trunc() / 360.;
    let (r, g, b) = match s == 0. {
        true => (l, l, l),
        false => {
            let q = if l < 0.5 { l * (1. + s) } else { l + s - l * s };
            let p = 2. * l - q;
            (hue(p, q, h + 1. / 3.), hue(p, q, h), hue(p, q, h - 1. / 3.))
        }
    };
    let byte = |value: f64| (value * 255.).round() as f32 / 255.;
    Rgba { r: byte(r), g: byte(g), b: byte(b), a: a as f32 }
}

enum ColorValue {
    None,
    Hex(&'static str),
    Key(&'static str),
    Transparent(&'static ColorValue, f32),
    Darken(&'static ColorValue, f32),
    Opaque(&'static ColorValue, &'static ColorValue),
    /// The color, the background, the factor and the transparency.
    LessProminent(&'static ColorValue, &'static ColorValue, f32, f32),
}

const MINIMAP_FIND_MATCH: ColorValue = ColorValue::Transparent(&ColorValue::Key("editorOverviewRuler.findMatchForeground"), 0.8);
const MINIMAP_SLIDER: ColorValue = ColorValue::Transparent(&ColorValue::Key("scrollbarSlider.background"), 0.5);
const MINIMAP_SLIDER_HOVER: ColorValue = ColorValue::Transparent(&ColorValue::Key("scrollbarSlider.hoverBackground"), 0.5);
const MINIMAP_SLIDER_ACTIVE: ColorValue = ColorValue::Transparent(&ColorValue::Key("scrollbarSlider.activeBackground"), 0.5);
const MINIMAP_OCCURRENCE: ColorValue = ColorValue::Key("editor.selectionHighlightBackground");
const MINIMAP_SELECTION: ColorValue = ColorValue::Key("editor.selectionBackground");

/// Registry defaults of the keys the workbench shell paints, copied from their `registerColor` calls
/// (theme.ts, baseColors.ts, editorColors.ts, miscColors.ts, minimapColors.ts). Keys not listed resolve to no color.
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
        "modernEditorTab.activeForeground" => (Key("modernTab.activeForeground"), Key("modernTab.activeForeground"), Key("modernTab.activeForeground")),
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
        "editorMultiCursor.primary.foreground" => (Key("editorCursor.foreground"), Key("editorCursor.foreground"), Key("editorCursor.foreground")),
        "editorMultiCursor.secondary.foreground" => (Key("editorCursor.foreground"), Key("editorCursor.foreground"), Key("editorCursor.foreground")),
        "editor.wordHighlightBackground" => (Hex("#575757B8"), Hex("#57575740"), None),
        "editor.wordHighlightTextBackground" => (Key("editor.wordHighlightBackground"), Key("editor.wordHighlightBackground"), Key("editor.wordHighlightBackground")),
        "editor.wordHighlightBorder" => (None, None, Key("contrastActiveBorder")),
        "editor.wordHighlightTextBorder" => (Key("editor.wordHighlightBorder"), Key("editor.wordHighlightBorder"), Key("editor.wordHighlightBorder")),
        "editor.findMatchBackground" => (Hex("#515C6A"), Hex("#A8AC94"), None),
        "editor.findMatchHighlightBackground" => (Hex("#EA5C0055"), Hex("#EA5C0055"), None),
        "editor.findRangeHighlightBackground" => (Hex("#3a3d4166"), Hex("#b4b4b44d"), None),
        "editor.findMatchBorder" | "editor.findMatchHighlightBorder" | "editor.rangeHighlightBorder" => (None, None, Key("contrastActiveBorder")),
        "editor.findRangeHighlightBorder" => (None, None, Transparent(&Key("contrastActiveBorder"), 0.4)),
        "editor.rangeHighlightBackground" => (Hex("#ffffff0b"), Hex("#fdff0033"), None),
        "editorWhitespace.foreground" => (Hex("#e3e4e229"), Hex("#33333333"), Hex("#e3e4e229")),
        "editorIndentGuide.background" => (Key("editorWhitespace.foreground"), Key("editorWhitespace.foreground"), Key("editorWhitespace.foreground")),
        "editorIndentGuide.background1" => (Key("editorIndentGuide.background"), Key("editorIndentGuide.background"), Key("editorIndentGuide.background")),
        "editorOverviewRuler.border" => (Hex("#7f7f7f4d"), Hex("#7f7f7f4d"), Hex("#7f7f7f4d")),
        "editorOverviewRuler.selectionHighlightForeground" => (Hex("#A0A0A0CC"), Hex("#A0A0A0CC"), Hex("#A0A0A0CC")),
        "editorOverviewRuler.findMatchForeground" => (Hex("#d186167e"), Hex("#d186167e"), Hex("#AB5A00")),
        "editorOverviewRuler.rangeHighlightForeground" => (Hex("#007acc99"), Hex("#007acc99"), Hex("#007acc99")),
        "editorOverviewRuler.errorForeground" => (Hex("#ff1212b3"), Hex("#ff1212b3"), Hex("#ff3232")),
        "editorOverviewRuler.infoForeground" => (Key("editorInfo.foreground"), Key("editorInfo.foreground"), Key("editorInfo.border")),
        "editorInfo.foreground" => (Hex("#59a4f9"), Hex("#0063d3"), Hex("#59a4f9")),
        "editorInfo.border" => (None, None, Hex("#59a4f9cc")),
        "minimap.findMatchHighlight" => (MINIMAP_FIND_MATCH, MINIMAP_FIND_MATCH, Key("editor.findMatchHighlightBorder")),
        "searchEditor.findMatchBackground" => (
            Transparent(&Key("editor.findMatchHighlightBackground"), 0.66),
            Transparent(&Key("editor.findMatchHighlightBackground"), 0.66),
            Key("editor.findMatchHighlightBackground"),
        ),
        "searchEditor.findMatchBorder" => (
            Transparent(&Key("editor.findMatchHighlightBorder"), 0.66),
            Transparent(&Key("editor.findMatchHighlightBorder"), 0.66),
            Key("editor.findMatchHighlightBorder"),
        ),
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
        "tab.activeForeground" => (Hex("#FFFFFF"), Hex("#333333"), Hex("#FFFFFF")),
        "tab.inactiveForeground" => (Transparent(&Key("tab.activeForeground"), 0.5), Transparent(&Key("tab.activeForeground"), 0.7), Hex("#FFFFFF")),
        "editorBracketMatch.background" => (Hex("#0064001A"), Hex("#0064001A"), Hex("#0064001A")),
        "editorBracketMatch.border" => (Hex("#888"), Hex("#B9B9B9"), Key("contrastBorder")),
        "editorWarning.foreground" => (Hex("#CCA700"), Hex("#BF8803"), Hex("#FFD370")),
        "editorUnicodeHighlight.border" => (Key("editorWarning.foreground"), Key("editorWarning.foreground"), Key("editorWarning.foreground")),
        "editorUnicodeHighlight.background" => (Key("editorWarning.background"), Key("editorWarning.background"), Key("editorWarning.background")),
        "banner.background" => (Key("list.activeSelectionBackground"), Darken(&Key("list.activeSelectionBackground"), 0.3), Key("list.activeSelectionBackground")),
        "banner.foreground" => (Key("list.activeSelectionForeground"), Key("list.activeSelectionForeground"), Key("list.activeSelectionForeground")),
        "banner.iconForeground" => (Key("editorInfo.foreground"), Key("editorInfo.foreground"), Key("editorInfo.foreground")),
        "editorOverviewRuler.bracketMatchForeground" => (Hex("#A0A0A0"), Hex("#A0A0A0"), Hex("#A0A0A0")),
        "editorBracketHighlight.foreground1" => (Hex("#FFD700"), Hex("#0431FAFF"), Hex("#FFD700")),
        "editorBracketHighlight.foreground2" => (Hex("#DA70D6"), Hex("#319331FF"), Hex("#DA70D6")),
        "editorBracketHighlight.foreground3" => (Hex("#179FFF"), Hex("#7B3814FF"), Hex("#87CEFA")),
        "editorBracketHighlight.unexpectedBracket.foreground" => (Hex("#FF1212CC"), Hex("#FF1212CC"), Hex("#FF3232")),
        "editor.foldBackground" => (Transparent(&Key("editor.selectionBackground"), 0.3), Transparent(&Key("editor.selectionBackground"), 0.3), None),
        "editor.foldPlaceholderForeground" => (Hex("#808080"), Hex("#808080"), None),
        "editorGutter.foldingControlForeground" => (Key("icon.foreground"), Key("icon.foreground"), Key("icon.foreground")),
        "editorStickyScroll.background" => (Key("editor.background"), Key("editor.background"), Key("editor.background")),
        "editorStickyScrollGutter.background" => (Key("editor.background"), Key("editor.background"), Key("editor.background")),
        "editorStickyScrollHover.background" => (Hex("#2A2D2E"), Hex("#F0F0F0"), None),
        "editorStickyScroll.border" => (None, None, Key("contrastBorder")),
        "editorStickyScroll.shadow" => (Key("scrollbar.shadow"), Key("scrollbar.shadow"), Key("scrollbar.shadow")),
        "scrollbar.shadow" => (Hex("#000000"), Hex("#DDDDDD"), None),
        "editor.selectionHighlightBackground" => (
            LessProminent(&Key("editor.selectionBackground"), &Key("editor.background"), 0.3, 0.6),
            LessProminent(&Key("editor.selectionBackground"), &Key("editor.background"), 0.3, 0.6),
            None,
        ),
        "scrollbarSlider.background" => (Transparent(&Hex("#797979"), 0.4), Transparent(&Hex("#646464"), 0.4), Transparent(&Key("contrastBorder"), 0.6)),
        "scrollbarSlider.hoverBackground" => (Transparent(&Hex("#646464"), 0.7), Transparent(&Hex("#646464"), 0.7), Transparent(&Key("contrastBorder"), 0.8)),
        "scrollbarSlider.activeBackground" => (Transparent(&Hex("#BFBFBF"), 0.4), Transparent(&Hex("#000000"), 0.6), Key("contrastBorder")),
        "minimapSlider.background" => (MINIMAP_SLIDER, MINIMAP_SLIDER, MINIMAP_SLIDER),
        "minimapSlider.hoverBackground" => (MINIMAP_SLIDER_HOVER, MINIMAP_SLIDER_HOVER, MINIMAP_SLIDER_HOVER),
        "minimapSlider.activeBackground" => (MINIMAP_SLIDER_ACTIVE, MINIMAP_SLIDER_ACTIVE, MINIMAP_SLIDER_ACTIVE),
        "minimap.selectionOccurrenceHighlight" => (MINIMAP_OCCURRENCE, MINIMAP_OCCURRENCE, MINIMAP_OCCURRENCE),
        "minimap.selectionHighlight" => (MINIMAP_SELECTION, MINIMAP_SELECTION, MINIMAP_SELECTION),
        "minimap.foregroundOpacity" => (Hex("#000f"), Hex("#000f"), Hex("#000f")),
        // The search view's keys (searchview.css, inputColors.ts, baseColors.ts).
        "diffEditor.insertedTextBackground" => (Hex("#9ccc2c33"), Hex("#9ccc2c40"), None),
        "diffEditor.removedTextBackground" => (Hex("#ff000033"), Hex("#ff000033"), None),
        "diffEditor.insertedLineBackground" => (Hex("#9bb95533"), Hex("#9bb95533"), None),
        "textLink.foreground" => (Hex("#3794FF"), Hex("#006AB1"), Hex("#21A6FF")),
        "textLink.activeForeground" => (Hex("#3794FF"), Hex("#006AB1"), Hex("#21A6FF")),
        "search.resultsInfoForeground" => (Transparent(&Key("foreground"), 0.65), Key("foreground"), Key("foreground")),
        "inputOption.activeBorder" => (Hex("#007ACC"), Hex("#007ACC"), Key("contrastBorder")),
        "inputOption.hoverBackground" => (Hex("#5a5d5e80"), Hex("#b8b8b850"), None),
        "inputOption.activeBackground" => (Transparent(&Key("focusBorder"), 0.4), Transparent(&Key("focusBorder"), 0.2), Hex("#0000")),
        "inputOption.activeForeground" => (Hex("#FFFFFF"), Hex("#000000"), Key("foreground")),
        _ => quick_input_default(key),
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
        "editorGutter.foldingControlForeground" => Key("icon.foreground"),
        "editorStickyScroll.background" => Key("editor.background"),
        "editorStickyScrollGutter.background" => Key("editor.background"),
        "editorStickyScrollHover.background" => Transparent(&Hex("#0F4A85"), 0.1),
        "editorStickyScroll.border" => Key("contrastBorder"),
        "editorStickyScroll.shadow" => Key("scrollbar.shadow"),
        "editor.background" => Hex("#ffffff"),
        "sideBar.background" => Hex("#FFFFFF"),
        "window.activeBorder" => Key("contrastBorder"),
        "titleBar.activeBackground" => Hex("#FFFFFF"),
        "titleBar.activeForeground" => Hex("#292929"),
        "commandCenter.foreground" => Key("titleBar.activeForeground"),
        "commandCenter.border" => Key("contrastBorder"),
        "editor.selectionBackground" => Hex("#0F4A85"),
        "editor.selectionForeground" => Hex("#FFFFFF"),
        "editor.inactiveSelectionBackground" => Transparent(&Key("editor.selectionBackground"), 0.5),
        "scrollbarSlider.background" => Transparent(&Key("contrastBorder"), 0.4),
        "scrollbarSlider.hoverBackground" => Transparent(&Key("contrastBorder"), 0.8),
        "scrollbarSlider.activeBackground" => Key("contrastBorder"),
        "minimapSlider.background" => MINIMAP_SLIDER,
        "minimapSlider.hoverBackground" => MINIMAP_SLIDER_HOVER,
        "minimapSlider.activeBackground" => MINIMAP_SLIDER_ACTIVE,
        "minimap.selectionOccurrenceHighlight" => MINIMAP_OCCURRENCE,
        "minimap.selectionHighlight" => MINIMAP_SELECTION,
        "minimap.foregroundOpacity" => Hex("#000f"),
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
        "editorWarning.foreground" => Hex("#895503"),
        "editorInfo.foreground" => Hex("#0063d3"),
        "editorUnicodeHighlight.border" => Key("editorWarning.foreground"),
        "editorUnicodeHighlight.background" => Key("editorWarning.background"),
        "banner.background" => Key("list.activeSelectionBackground"),
        "banner.foreground" => Key("list.activeSelectionForeground"),
        "banner.iconForeground" => Key("editorInfo.foreground"),
        "editorOverviewRuler.bracketMatchForeground" => Hex("#A0A0A0"),
        "editorOverviewRuler.errorForeground" => Hex("#B5200D"),
        "editorOverviewRuler.infoForeground" => Key("editorInfo.border"),
        "editorInfo.border" => Hex("#292929"),
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
        "modernEditorTab.activeForeground" => Key("modernTab.activeForeground"),
        "modernActivityBarItem.activeBackground" => Key("modernTab.activeBackground"),
        "modernActivityBarItem.activeForeground" => Key("modernTab.activeForeground"),
        "editor.foreground" => Key("foreground"),
        "editorLineNumber.foreground" => Hex("#292929"),
        "editorActiveLineNumber.foreground" => Key("contrastActiveBorder"),
        "editorLineNumber.activeForeground" => Key("editorActiveLineNumber.foreground"),
        "editor.lineHighlightBorder" => Key("contrastBorder"),
        "editorCursor.foreground" => Hex("#0F4A85"),
        "editorMultiCursor.primary.foreground" => Key("editorCursor.foreground"),
        "editorMultiCursor.secondary.foreground" => Key("editorCursor.foreground"),
        "editor.wordHighlightBorder" => Key("contrastActiveBorder"),
        "editor.wordHighlightTextBorder" => Key("editor.wordHighlightBorder"),
        "editor.findMatchBorder" | "editor.findMatchHighlightBorder" | "editor.rangeHighlightBorder" => Key("contrastActiveBorder"),
        "editor.findRangeHighlightBorder" => Transparent(&Key("contrastActiveBorder"), 0.4),
        "editorWhitespace.foreground" => Hex("#CCCCCC"),
        "editorIndentGuide.background" => Key("editorWhitespace.foreground"),
        "editorIndentGuide.background1" => Key("editorIndentGuide.background"),
        "editorOverviewRuler.border" => Hex("#666666"),
        "editorOverviewRuler.selectionHighlightForeground" => Hex("#A0A0A0CC"),
        "editorOverviewRuler.findMatchForeground" => Hex("#AB5A00"),
        "minimap.findMatchHighlight" => Key("editor.findMatchHighlightBorder"),
        "searchEditor.findMatchBackground" => Key("editor.findMatchHighlightBackground"),
        "searchEditor.findMatchBorder" => Key("editor.findMatchHighlightBorder"),
        "searchEditor.textInputBorder" => Key("input.border"),
        "editorOverviewRuler.wordHighlightTextForeground" => Key("editorOverviewRuler.selectionHighlightForeground"),
        "breadcrumb.foreground" => Transparent(&Key("foreground"), 0.8),
        "breadcrumb.background" => Key("editor.background"),
        "tab.border" => Key("contrastBorder"),
        "tab.activeForeground" | "tab.inactiveForeground" => Hex("#292929"),
        "textLink.foreground" => Hex("#0F4A85"),
        "textLink.activeForeground" => Hex("#0F4A85"),
        "search.resultsInfoForeground" => Key("foreground"),
        "inputOption.activeBorder" => Key("contrastBorder"),
        "inputOption.activeBackground" => Hex("#0000"),
        "inputOption.activeForeground" => Key("foreground"),
        _ => quick_input_default_hc_light(key),
    }
}

/// The keys the quick input paints (quickpickColors.ts, inputColors.ts, listColors.ts, miscColors.ts),
/// as `registry_default` columns: dark, light, high contrast.
fn quick_input_default(key: &str) -> (ColorValue, ColorValue, ColorValue) {
    use ColorValue::*;
    match key {
        "editorWidget.background" => (Hex("#252526"), Hex("#F3F3F3"), Hex("#0C141F")),
        "editorWidget.foreground" => (Key("foreground"), Key("foreground"), Key("foreground")),
        "quickInput.background" => (Key("editorWidget.background"), Key("editorWidget.background"), Key("editorWidget.background")),
        "quickInput.foreground" => (Key("editorWidget.foreground"), Key("editorWidget.foreground"), Key("editorWidget.foreground")),
        "quickInputTitle.background" => (Hex("#FFFFFF1B"), Hex("#0000000F"), Hex("#000000")),
        "widget.border" => (None, None, Key("contrastBorder")),
        "input.background" => (Hex("#3C3C3C"), Hex("#FFFFFF"), Hex("#000000")),
        "input.foreground" => (Key("foreground"), Key("foreground"), Key("foreground")),
        "input.border" => (None, None, Key("contrastBorder")),
        "input.placeholderForeground" => (Transparent(&Key("foreground"), 0.5), Transparent(&Key("foreground"), 0.5), Transparent(&Key("foreground"), 0.7)),
        "inputOption.activeBorder" => (Hex("#007ACC"), Hex("#007ACC"), Key("contrastBorder")),
        "inputOption.hoverBackground" => (Hex("#5a5d5e80"), Hex("#b8b8b850"), None),
        "inputOption.activeBackground" => (Transparent(&Key("focusBorder"), 0.4), Transparent(&Key("focusBorder"), 0.2), Hex("#00000000")),
        "inputOption.activeForeground" => (Hex("#FFFFFF"), Hex("#000000"), Key("foreground")),
        "editorWidget.border" => (Transparent(&Key("editorWidget.foreground"), 0.2), Transparent(&Key("editorWidget.foreground"), 0.2), Key("contrastBorder")),
        "errorForeground" => (Hex("#F48771"), Hex("#A1260D"), Hex("#F48771")),
        "inputValidation.infoBackground" => (Hex("#063B49"), Hex("#D6ECF2"), Hex("#000000")),
        "inputValidation.infoBorder" => (Hex("#007acc"), Hex("#007acc"), Key("contrastBorder")),
        "inputValidation.warningBackground" => (Hex("#352A05"), Hex("#F6F5D2"), Hex("#000000")),
        "inputValidation.warningBorder" => (Hex("#B89500"), Hex("#B89500"), Key("contrastBorder")),
        "inputValidation.errorBackground" => (Hex("#5A1D1D"), Hex("#F2DEDE"), Hex("#000000")),
        "inputValidation.errorBorder" => (Hex("#BE1100"), Hex("#BE1100"), Key("contrastBorder")),
        "pickerGroup.foreground" => (Hex("#3794FF"), Hex("#0066BF"), Hex("#FFFFFF")),
        "pickerGroup.border" => (Hex("#3F3F46"), Hex("#CCCEDB"), Hex("#FFFFFF")),
        "descriptionForeground" => (Transparent(&Key("foreground"), 0.7), Hex("#717171"), Transparent(&Key("foreground"), 0.7)),
        "list.activeSelectionBackground" => (Hex("#04395E"), Hex("#0060C0"), None),
        "list.activeSelectionForeground" => (Hex("#FFFFFF"), Hex("#FFFFFF"), None),
        "list.hoverBackground" => (Hex("#2A2D2E"), Hex("#F0F0F0"), Hex("#FFFFFF1A")),
        "list.highlightForeground" => (Hex("#2AAAFF"), Hex("#0066BF"), Key("focusBorder")),
        // light: ifDefinedThenElse(list.activeSelectionBackground, ...), and that key always resolves there.
        "list.focusHighlightForeground" => (Key("list.highlightForeground"), Key("list.highlightForeground"), Key("list.highlightForeground")),
        // oneOf(quickInput.list.focusBackground, list.activeSelectionBackground), and the first has no default.
        "quickInputList.focusBackground" => (Key("list.activeSelectionBackground"), Key("list.activeSelectionBackground"), None),
        "quickInputList.focusForeground" => (Key("list.activeSelectionForeground"), Key("list.activeSelectionForeground"), Key("list.activeSelectionForeground")),
        "quickInputList.focusHighlightForeground" => (
            Key("list.focusHighlightForeground"),
            Key("list.focusHighlightForeground"),
            Key("list.focusHighlightForeground"),
        ),
        "keybindingLabel.background" => (Hex("#8080802B"), Hex("#DDDDDD66"), Hex("#0000")),
        "keybindingLabel.foreground" => (Hex("#CCCCCC"), Hex("#555555"), Hex("#FFFFFF")),
        "keybindingLabel.border" => (Hex("#33333399"), Hex("#CCCCCC66"), Hex("#6FC3DF")),
        "keybindingLabel.bottomBorder" => (Hex("#44444499"), Hex("#BBBBBB66"), Hex("#6FC3DF")),
        "progressBar.background" => (Hex("#0E70C0"), Hex("#0E70C0"), Key("contrastBorder")),
        "badge.background" => (Hex("#4D4D4D"), Hex("#C4C4C4"), Hex("#000000")),
        "badge.foreground" => (Hex("#FFFFFF"), Hex("#333"), Hex("#FFFFFF")),
        "button.background" => (Hex("#0E639C"), Hex("#007ACC"), Hex("#000000")),
        "button.foreground" => (Hex("#FFFFFF"), Hex("#FFFFFF"), Hex("#FFFFFF")),
        "button.border" => (Key("contrastBorder"), Key("contrastBorder"), Key("contrastBorder")),
        "checkbox.background" => (Key("select.background"), Key("select.background"), Key("select.background")),
        "checkbox.foreground" => (Key("select.foreground"), Key("select.foreground"), Key("select.foreground")),
        "checkbox.border" => (Key("select.border"), Key("select.border"), Key("select.border")),
        "select.background" => (Hex("#3C3C3C"), Hex("#FFFFFF"), Hex("#000000")),
        "select.foreground" => (Hex("#F0F0F0"), Key("foreground"), Hex("#FFFFFF")),
        "select.border" => (Hex("#3C3C3C"), Hex("#CECECE"), Key("contrastBorder")),
        "toolbar.hoverBackground" => (Hex("#5a5d5e50"), Hex("#b8b8b850"), None),
        // menuColors.ts and editorWidget.border, the menu's fallback border (defaultStyles.ts defaultMenuStyles).
        "menu.border" => (None, None, Key("contrastBorder")),
        "menu.foreground" => (Key("select.foreground"), Key("select.foreground"), Key("select.foreground")),
        "menu.background" => (Key("select.background"), Key("select.background"), Key("select.background")),
        "menu.selectionBorder" => (None, None, Key("contrastActiveBorder")),
        "menu.separatorBackground" => (Transparent(&Key("foreground"), 0.2), Transparent(&Key("foreground"), 0.2), Key("contrastBorder")),
        // editorColors.ts, for the workbench hover (hover.css).
        "editorHoverWidget.background" => (Key("editorWidget.background"), Key("editorWidget.background"), Key("editorWidget.background")),
        "editorHoverWidget.foreground" => (Key("editorWidget.foreground"), Key("editorWidget.foreground"), Key("editorWidget.foreground")),
        "editorHoverWidget.border" => (Key("editorWidget.border"), Key("editorWidget.border"), Key("editorWidget.border")),
        "textLink.foreground" => (Hex("#3794FF"), Hex("#006AB1"), Hex("#21A6FF")),
        "textLink.activeForeground" => (Hex("#3794FF"), Hex("#006AB1"), Hex("#21A6FF")),
        // listColors.ts, editorColors.ts and keybindingsEditor.ts, for the Keyboard Shortcuts editor.
        "list.focusOutline" => (Key("focusBorder"), Key("focusBorder"), Key("contrastActiveBorder")),
        "widget.shadow" => (Hex("#0000005C"), Hex("#00000029"), None),
        "tree.tableOddRowsBackground" => (Transparent(&Key("foreground"), 0.04), Transparent(&Key("foreground"), 0.04), None),
        "tree.tableColumnsBorder" => (Hex("#CCCCCC20"), Hex("#61616120"), None),
        "keybindingTable.headerBackground" | "keybindingTable.rowsBackground" => {
            (Key("tree.tableOddRowsBackground"), Key("tree.tableOddRowsBackground"), Key("tree.tableOddRowsBackground"))
        }
        _ => (None, None, None),
    }
}

/// The `hcLight` column of the keys `quick_input_default` lists.
fn quick_input_default_hc_light(key: &str) -> ColorValue {
    use ColorValue::*;
    match key {
        "editorWidget.background" => Hex("#FFFFFF"),
        "editorWidget.foreground" => Key("foreground"),
        "quickInput.background" => Key("editorWidget.background"),
        "quickInput.foreground" => Key("editorWidget.foreground"),
        "quickInputTitle.background" => Hex("#FFFFFF"),
        "widget.border" => Key("contrastBorder"),
        "input.background" => Hex("#FFFFFF"),
        "input.foreground" => Key("foreground"),
        "input.border" => Key("contrastBorder"),
        "input.placeholderForeground" => Transparent(&Key("foreground"), 0.7),
        "inputOption.activeBorder" | "editorWidget.border" => Key("contrastBorder"),
        "inputOption.activeBackground" => Hex("#00000000"),
        "inputOption.activeForeground" => Key("foreground"),
        "errorForeground" => Hex("#B5200D"),
        "inputValidation.infoBackground" | "inputValidation.warningBackground" | "inputValidation.errorBackground" => Hex("#FFFFFF"),
        "inputValidation.infoBorder" | "inputValidation.warningBorder" | "inputValidation.errorBorder" => Key("contrastBorder"),
        "inputValidation.infoForeground" | "inputValidation.warningForeground" | "inputValidation.errorForeground" => Key("foreground"),
        "pickerGroup.foreground" | "pickerGroup.border" => Hex("#0F4A85"),
        "descriptionForeground" => Transparent(&Key("foreground"), 0.7),
        "list.activeSelectionBackground" => Hex("#0F4A851A"),
        "list.hoverBackground" => Hex("#0F4A851A"),
        "list.highlightForeground" => Key("focusBorder"),
        "list.focusHighlightForeground" => Key("list.highlightForeground"),
        "quickInputList.focusHighlightForeground" => Key("list.focusHighlightForeground"),
        "keybindingLabel.background" => Hex("#0000"),
        "keybindingLabel.foreground" => Key("foreground"),
        "keybindingLabel.border" => Key("contrastBorder"),
        "keybindingLabel.bottomBorder" => Key("foreground"),
        "progressBar.background" => Key("contrastBorder"),
        "badge.background" => Hex("#0F4A85"),
        "badge.foreground" => Hex("#FFFFFF"),
        "button.background" => Hex("#0F4A85"),
        "button.foreground" => Hex("#FFFFFF"),
        "button.border" => Key("contrastBorder"),
        "checkbox.background" => Key("select.background"),
        "checkbox.foreground" => Key("select.foreground"),
        "checkbox.border" => Key("select.border"),
        "select.background" => Hex("#FFFFFF"),
        "select.foreground" => Key("foreground"),
        "select.border" => Key("contrastBorder"),
        "menu.border" | "menu.separatorBackground" => Key("contrastBorder"),
        "menu.foreground" => Key("select.foreground"),
        "menu.background" => Key("select.background"),
        "menu.selectionBorder" => Key("contrastActiveBorder"),
        "editorHoverWidget.background" => Key("editorWidget.background"),
        "editorHoverWidget.foreground" => Key("editorWidget.foreground"),
        "editorHoverWidget.border" => Key("editorWidget.border"),
        "textLink.foreground" | "textLink.activeForeground" => Hex("#0F4A85"),
        "list.focusOutline" => Key("contrastActiveBorder"),
        "keybindingTable.headerBackground" | "keybindingTable.rowsBackground" => Key("tree.tableOddRowsBackground"),
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

    // Expected values: upstream color.ts `lessProminent` and `darken` run under Bun on the same inputs.
    #[test]
    fn less_prominent_matches_upstream_color_math() {
        let color = |text| parse_hex(text).unwrap();
        assert_eq!(hex(less_prominent(color("#84613daa"), color("#221a0f"), 0.3, 0.6)), "#60462c66");
        assert_eq!(hex(less_prominent(color("#C9D0D9"), color("#F5F5F5"), 0.3, 0.6)), "#e0e4ea99");
        assert_eq!(hex(darken(color("#0060C0"), 0.3)), "#004386ff");
        assert_eq!(hex(darken(color("#3794ff80"), 0.3)), "#0065d980");
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
            ("Dark Modern", "editor.findMatchBackground", "#9e6a03ff"),
            ("Dark Modern", "editor.findMatchHighlightBackground", "#ea5c0055"),
            ("Dark Modern", "editor.rangeHighlightBackground", "#ffffff0b"),
            ("Default High Contrast", "editor.findMatchBorder", "#f38518ff"),
            ("Default High Contrast", "editor.rangeHighlightBorder", "#f38518ff"),
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
            ("Dark Modern", "editorOverviewRuler.findMatchForeground", "#d186167e"),
            ("Dark Modern", "editorOverviewRuler.errorForeground", "#ff1212b3"),
            ("Dark Modern", "editorOverviewRuler.infoForeground", "#59a4f9ff"),
            ("Default High Contrast", "editorOverviewRuler.infoForeground", "#59a4f9cc"),
            ("Dark Modern", "minimap.findMatchHighlight", "#d1861665"),
            ("Default High Contrast", "editorOverviewRuler.findMatchForeground", "#ab5a00ff"),
            ("Default High Contrast", "minimap.findMatchHighlight", "#f38518ff"),
            ("Dark Modern", "modernEditorTab.activeForeground", "#ccccccff"),
            ("Dark Modern", "tab.activeForeground", "#ffffffff"),
            ("Default High Contrast", "tab.inactiveForeground", "#ffffffff"),
            ("Default High Contrast Light", "tab.activeForeground", "#292929ff"),
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
