//! Color themes as `ColorThemeData` resolves them: the theme file's `include` chain merged, then the
//! color registry default for every key the theme leaves out (src/vs/platform/theme/common/colorUtils.ts).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use exthost::scan::ScannedExtension;
use gpui::Rgba;
use serde_json::Value;

use crate::semantic_tokens::{SemanticTheme, SemanticTokenRule, TokenClassificationRegistry, read_semantic_token_customizations, read_theme_file_semantics, semantic_highlighting};

mod debug_colors;
mod welcome_colors;

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
    /// `workbench.colorCustomizations` for this theme; `None` is `"default"`, the registry default over the theme's color.
    custom_colors: HashMap<String, Option<Rgba>>,
    /// The theme file's `tokenColors`, before `editor.tokenColorCustomizations` is appended.
    token_rules: Vec<Value>,
    /// The TextMate theme of the token colors (`ColorThemeData.tokenColors`).
    pub tokens: textmate::Theme,
    /// The theme file's `semanticTokenColors` and `semanticHighlighting`, before the settings' customizations.
    semantic_rules: Vec<SemanticTokenRule>,
    file_semantic_highlighting: bool,
    /// `ColorThemeData.getTokenStyleMetadata` with `editor.semanticTokenColorCustomizations` applied.
    pub semantic: SemanticTheme,
    /// The `ColorThemeData.semanticHighlighting` getter.
    pub semantic_highlighting: bool,
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
        let (mut semantic_rules, mut file_semantic_highlighting) = (Vec::new(), false);
        read_colors(&self.path, &mut colors, &mut token_colors, &mut semantic_rules, &mut file_semantic_highlighting)?;
        let semantic = SemanticTheme::new(TokenClassificationRegistry::new(), self.kind, semantic_rules.clone(), &[]);
        let mut theme = Theme {
            kind: self.kind,
            settings_id: self.settings_id.clone(),
            colors,
            custom_colors: HashMap::new(),
            token_rules: token_colors,
            tokens: textmate::Theme::new(&[]),
            semantic_rules,
            file_semantic_highlighting,
            semantic,
            semantic_highlighting: file_semantic_highlighting,
        };
        theme.tokens = crate::syntax::token_theme(&theme, &theme.token_rules);
        theme.set_semantic_customizations(&Value::Null, &Value::Null);
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
fn read_colors(path: &Path, colors: &mut HashMap<String, Rgba>, token_colors: &mut Vec<Value>, semantic_rules: &mut Vec<SemanticTokenRule>, semantic_highlighting: &mut bool) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let theme: Value = serde_json::from_str(&strip_jsonc(&text)).with_context(|| format!("parsing {}", path.display()))?;
    if let Some(include) = theme["include"].as_str() {
        read_colors(&path.parent().unwrap().join(include), colors, token_colors, semantic_rules, semantic_highlighting)?;
    }
    read_theme_file_semantics(&theme, semantic_rules, semantic_highlighting);
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
        match self.custom_colors.get(key) {
            Some(Some(color)) => return Some(*color),
            Some(None) => {}
            None => {
                if let Some(color) = self.colors.get(key) {
                    return Some(*color);
                }
            }
        }
        self.evaluate(&registry_default(key, self.kind))
    }

    /// `ColorThemeData.defines`: the theme file or a customization sets the color, `"default"` unsets it.
    pub fn defines(&self, key: &str) -> bool {
        match self.custom_colors.get(key) {
            Some(color) => color.is_some(),
            None => self.colors.contains_key(key),
        }
    }

    /// `ColorThemeData.setCustomColors` and `setCustomTokenColors`: the settings' values, then those of every
    /// `[theme id]` block matching this theme, with the token rules appended after the theme's own.
    pub fn set_customizations(&mut self, colors: &Value, token_colors: &Value) {
        self.custom_colors.clear();
        for colors in [Some(colors.clone()), self.theme_specific(colors)].into_iter().flatten() {
            for (key, value) in colors.as_object().into_iter().flatten() {
                match value.as_str() {
                    Some("default") => self.custom_colors.insert(key.clone(), None),
                    // Color.fromHex: an unparsable value is red.
                    Some(hex) => self.custom_colors.insert(key.clone(), Some(parse_hex(hex).unwrap_or(Rgba { r: 1., g: 0., b: 0., a: 1. }))),
                    None => continue,
                };
            }
        }
        let mut rules = self.token_rules.clone();
        for customizations in [Some(token_colors.clone()), self.theme_specific(token_colors)].into_iter().flatten() {
            custom_token_rules(&customizations, &mut rules);
        }
        self.tokens = crate::syntax::token_theme(self, &rules);
    }

    /// `setCustomSemanticTokenColors`: `editor.semanticTokenColorCustomizations` after the theme's rules, over the
    /// customized token colors; call after `set_customizations`.
    pub fn set_semantic_customizations(&mut self, token_colors: &Value, semantic_colors: &Value) {
        let mut rules = self.semantic_rules.clone();
        let theme_specific = self.theme_specific(semantic_colors);
        let enabled = read_semantic_token_customizations(semantic_colors, theme_specific.as_ref(), &mut rules);
        let theme_specific_deprecated = self.theme_specific(token_colors).and_then(|colors| colors["semanticHighlighting"].as_bool());
        let deprecated = theme_specific_deprecated.or(token_colors["semanticHighlighting"].as_bool());
        self.semantic_highlighting = semantic_highlighting(enabled, deprecated, self.file_semantic_highlighting);
        self.semantic = SemanticTheme::new(TokenClassificationRegistry::new(), self.kind, rules, self.tokens.rules());
    }

    /// `getThemeSpecificColors`: the merged values of the `[id]` keys (`[a][b]`, `*` wildcards at either end) matching
    /// this theme; arrays of several blocks concatenate.
    fn theme_specific(&self, colors: &Value) -> Option<Value> {
        let mut merged: Option<serde_json::Map<String, Value>> = None;
        for (key, scoped) in colors.as_object().into_iter().flatten() {
            if !(key.starts_with('[') && key.ends_with(']')) || !scoped.is_object() {
                continue;
            }
            // themeScopeRegex `/\[(.+?)\]/g`
            for theme_id in key[1..key.len() - 1].split("][").filter(|id| !id.is_empty()) {
                if !self.is_theme_scope_match(theme_id) {
                    continue;
                }
                let merged = merged.get_or_insert_default();
                for (subkey, value) in scoped.as_object().unwrap() {
                    match (merged.get_mut(subkey), value) {
                        (Some(Value::Array(original)), Value::Array(more)) => original.extend(more.iter().cloned()),
                        (_, Value::Null | Value::Bool(false)) => {}
                        _ => _ = merged.insert(subkey.clone(), value.clone()),
                    }
                }
            }
        }
        merged.map(Value::Object)
    }

    fn is_theme_scope_match(&self, theme_id: &str) -> bool {
        let id = &self.settings_id;
        let (starts, ends) = (theme_id.starts_with('*'), theme_id.ends_with('*'));
        theme_id == id
            || (starts && ends && theme_id.len() >= 2 && id.contains(&theme_id[1..theme_id.len() - 1]))
            || (ends && id.starts_with(&theme_id[..theme_id.len() - 1]))
            || (starts && id.ends_with(&theme_id[1..]))
    }

    fn evaluate(&self, value: &ColorValue) -> Option<Rgba> {
        match value {
            ColorValue::None => None,
            ColorValue::Hex(hex) => Some(parse_hex(hex).expect("registry hex")),
            ColorValue::Key(key) => self.color(key),
            ColorValue::Transparent(inner, factor) => self.evaluate(inner).map(|color| Rgba { a: color.a * factor, ..color }),
            ColorValue::Opacity(inner, opacity) => self.evaluate(inner).map(|color| Rgba { a: *opacity, ..color }),
            ColorValue::Darken(inner, factor) => self.evaluate(inner).map(|color| darken(color, *factor)),
            ColorValue::Lighten(inner, factor) => self.evaluate(inner).map(|color| lighten(color, *factor)),
            ColorValue::Opaque(inner, background) => {
                let (color, background) = (self.evaluate(inner)?, self.evaluate(background)?);
                // Color.makeOpaque: RGBA's `_clampUint8` truncates each blended byte (`| 0`).
                // The alpha is RGBA's, rounded to 3 decimals; the math is JS doubles.
                let alpha = (color.a as f64 * 1000.).round() / 1000.;
                let blend = |c: f32, b: f32| {
                    let (c, b) = ((c as f64 * 255.).round(), (b as f64 * 255.).round());
                    ((b - alpha * (b - c)).floor() / 255.) as f32
                };
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

/// `addCustomTokenColors`: the groups (`comments`, `strings`, ...) as their scopes first, so `textMateRules` override them.
fn custom_token_rules(customizations: &Value, rules: &mut Vec<Value>) {
    const GROUPS: [(&str, &[&str]); 7] = [
        ("comments", &["comment", "punctuation.definition.comment"]),
        ("strings", &["string", "meta.embedded.assembly"]),
        ("keywords", &["keyword - keyword.operator", "keyword.control", "storage", "storage.type"]),
        ("numbers", &["constant.numeric"]),
        ("types", &["entity.name.type", "entity.name.class", "support.type", "support.class"]),
        ("functions", &["entity.name.function", "support.function"]),
        ("variables", &["variable", "entity.name.variable"]),
    ];
    for (group, scopes) in GROUPS {
        let settings = match &customizations[group] {
            Value::String(color) if !color.is_empty() => serde_json::json!({ "foreground": color }),
            Value::Object(settings) => Value::Object(settings.clone()),
            _ => continue,
        };
        for scope in scopes {
            rules.push(serde_json::json!({ "scope": scope, "settings": settings }));
        }
    }
    rules.extend(customizations["textMateRules"].as_array().into_iter().flatten().cloned());
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

/// `Color.lighten`: the HSLA lightness plus `factor` of itself.
fn lighten(color: Rgba, factor: f32) -> Rgba {
    let (h, s, l, a) = hsla(color);
    from_hsla(h, s, round3(l + l * factor as f64), a)
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
pub(crate) fn from_hsla(h: f64, s: f64, l: f64, a: f64) -> Rgba {
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

#[derive(Clone, Copy)]
pub(crate) enum ColorValue {
    None,
    Hex(&'static str),
    Key(&'static str),
    Transparent(&'static ColorValue, f32),
    /// `withOpacity`: the color with this alpha.
    Opacity(&'static ColorValue, f32),
    Darken(&'static ColorValue, f32),
    Lighten(&'static ColorValue, f32),
    Opaque(&'static ColorValue, &'static ColorValue),
    /// The color, the background, the factor and the transparency.
    LessProminent(&'static ColorValue, &'static ColorValue, f32, f32),
}

const MINIMAP_FIND_MATCH: ColorValue = ColorValue::Opacity(&ColorValue::Key("editorOverviewRuler.findMatchForeground"), 0.8);
const MINIMAP_SLIDER: ColorValue = ColorValue::Transparent(&ColorValue::Key("scrollbarSlider.background"), 0.5);
const MINIMAP_SLIDER_HOVER: ColorValue = ColorValue::Transparent(&ColorValue::Key("scrollbarSlider.hoverBackground"), 0.5);
const MINIMAP_SLIDER_ACTIVE: ColorValue = ColorValue::Transparent(&ColorValue::Key("scrollbarSlider.activeBackground"), 0.5);
const MINIMAP_OCCURRENCE: ColorValue = ColorValue::Key("editor.selectionHighlightBackground");
const MINIMAP_SELECTION: ColorValue = ColorValue::Key("editor.selectionBackground");

/// Registry defaults of the keys the workbench shell paints, copied from their `registerColor` calls
/// (theme.ts, baseColors.ts, editorColors.ts, miscColors.ts, minimapColors.ts). Keys not listed resolve to no color.
fn registry_default(key: &str, kind: ThemeKind) -> ColorValue {
    use ColorValue::*;
    if let Some([dark, light, hc, hc_light]) = crate::symbol_icons::color_default(key) {
        return match kind {
            ThemeKind::Dark => dark,
            ThemeKind::Light => light,
            ThemeKind::HighContrastDark => hc,
            ThemeKind::HighContrastLight => hc_light,
        };
    }
    if let Some([dark, light, hc, hc_light]) = debug_colors::debug_default(key).or_else(|| welcome_colors::welcome_default(key)) {
        return match kind {
            ThemeKind::Dark => dark,
            ThemeKind::Light => light,
            ThemeKind::HighContrastDark => hc,
            ThemeKind::HighContrastLight => hc_light,
        };
    }
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
        "panel.background" => (Key("editor.background"), Key("editor.background"), Key("editor.background")),
        "panelTitle.activeForeground" => (Hex("#E7E7E7"), Hex("#424242"), Hex("#FFFFFF")),
        "panelTitle.inactiveForeground" => (Transparent(&Key("panelTitle.activeForeground"), 0.6), Transparent(&Key("panelTitle.activeForeground"), 0.75), Hex("#FFFFFF")),
        "activityBarTop.foreground" => (Hex("#E7E7E7"), Hex("#424242"), Hex("#FFFFFF")),
        "activityBarTop.inactiveForeground" => (Transparent(&Key("activityBarTop.foreground"), 0.6), Transparent(&Key("activityBarTop.foreground"), 0.75), Hex("#FFFFFF")),
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
        "modernSash.gripForeground" => (Transparent(&Key("foreground"), 0.4), Transparent(&Key("foreground"), 0.4), Key("foreground")),
        "surface.border" => (
            Opaque(&Transparent(&Key("foreground"), 0.15), &Key("surface.background")),
            Opaque(&Transparent(&Key("foreground"), 0.15), &Key("surface.background")),
            Key("contrastBorder"),
        ),
        "statusBar.foreground" => (Hex("#FFFFFF"), Hex("#FFFFFF"), Hex("#FFFFFF")),
        "statusBar.noFolderForeground" => (Key("statusBar.foreground"), Key("statusBar.foreground"), Key("statusBar.foreground")),
        "statusBarItem.prominentForeground" => (Key("statusBar.foreground"), Key("statusBar.foreground"), Key("statusBar.foreground")),
        "statusBarItem.prominentBackground" => (Hex("#00000080"), Hex("#00000080"), Hex("#00000080")),
        "statusBar.noFolderBackground" => (Hex("#68217A"), Hex("#68217A"), None),
        "statusBar.background" => (Hex("#007ACC"), Hex("#007ACC"), None),
        "statusBar.focusBorder" => (Key("statusBar.foreground"), Key("statusBar.foreground"), None),
        "focusBorder" => (Hex("#007FD4"), Hex("#0090F1"), Hex("#F38518")),
        "contrastActiveBorder" => (None, None, Key("focusBorder")),
        "sideBar.border" => (None, None, Key("contrastBorder")),
        "sideBarTitle.foreground" => (Key("sideBar.foreground"), Key("sideBar.foreground"), Key("sideBar.foreground")),
        "sideBarTitle.border" => (None, None, Key("sideBar.border")),
        "sideBarSectionHeader.background" => (Hex("#80808033"), Hex("#80808033"), None),
        "sideBarSectionHeader.foreground" => (Key("sideBar.foreground"), Key("sideBar.foreground"), Key("sideBar.foreground")),
        "sideBarSectionHeader.border" => (Key("contrastBorder"), Key("contrastBorder"), Key("contrastBorder")),
        "editorGroup.dropBackground" => (Transparent(&Hex("#53595D"), 0.5), Transparent(&Hex("#2677CB"), 0.18), None),
        "sideBar.dropBackground" => (Key("editorGroup.dropBackground"), Key("editorGroup.dropBackground"), Key("editorGroup.dropBackground")),
        "list.inactiveSelectionBackground" => (Hex("#37373D"), Hex("#E4E6F1"), None),
        "modernTab.activeBackground" => (Key("list.inactiveSelectionBackground"), Key("list.inactiveSelectionBackground"), Key("list.inactiveSelectionBackground")),
        // oneOf(list.inactiveSelectionForeground, foreground), and the first has no default.
        "modernTab.activeForeground" => (Key("foreground"), Key("foreground"), Key("foreground")),
        "modernEditorTab.activeForeground" => (Key("modernTab.activeForeground"), Key("modernTab.activeForeground"), Key("modernTab.activeForeground")),
        "modernActivityBarItem.activeBackground" => (Key("modernTab.activeBackground"), Key("modernTab.activeBackground"), Key("modernTab.activeBackground")),
        "modernActivityBar.background" => (Key("activityBar.background"), Key("activityBar.background"), Key("activityBar.background")),
        "modernActivityBar.inactiveBackground" => (Key("modernActivityBar.background"), Key("modernActivityBar.background"), Key("modernActivityBar.background")),
        "modernActivityBar.border" => (Key("surface.border"), Key("surface.border"), Key("surface.border")),
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
        "editor.inactiveLineHighlightBackground" => (Key("editor.lineHighlightBackground"), Key("editor.lineHighlightBackground"), Key("editor.lineHighlightBackground")),
        "editorCursor.foreground" => (Hex("#AEAFAD"), Hex("#000000"), Hex("#FFFFFF")),
        "editorMultiCursor.primary.foreground" => (Key("editorCursor.foreground"), Key("editorCursor.foreground"), Key("editorCursor.foreground")),
        "editorMultiCursor.secondary.foreground" => (Key("editorCursor.foreground"), Key("editorCursor.foreground"), Key("editorCursor.foreground")),
        "editor.wordHighlightBackground" => (Hex("#575757B8"), Hex("#57575740"), None),
        "editor.wordHighlightTextBackground" => (Key("editor.wordHighlightBackground"), Key("editor.wordHighlightBackground"), Key("editor.wordHighlightBackground")),
        "editor.wordHighlightBorder" => (None, None, Key("contrastActiveBorder")),
        "editor.wordHighlightStrongBackground" => (Hex("#004972B8"), Hex("#0e639c40"), None),
        "editor.wordHighlightStrongBorder" => (None, None, Key("contrastActiveBorder")),
        "editorOverviewRuler.wordHighlightForeground" => (Hex("#A0A0A0CC"), Hex("#A0A0A0CC"), Hex("#A0A0A0CC")),
        "editorOverviewRuler.wordHighlightStrongForeground" => (Hex("#C0A0C0CC"), Hex("#C0A0C0CC"), Hex("#C0A0C0CC")),
        "editor.selectionHighlightBorder" => (None, None, Key("contrastActiveBorder")),
        "editor.wordHighlightTextBorder" => (Key("editor.wordHighlightBorder"), Key("editor.wordHighlightBorder"), Key("editor.wordHighlightBorder")),
        "editor.findMatchBackground" => (Hex("#515C6A"), Hex("#A8AC94"), None),
        "editor.findMatchHighlightBackground" => (Hex("#EA5C0055"), Hex("#EA5C0055"), None),
        "editor.findRangeHighlightBackground" => (Hex("#3a3d4166"), Hex("#b4b4b44d"), None),
        "editor.findMatchBorder" | "editor.findMatchHighlightBorder" | "editor.rangeHighlightBorder" => (None, None, Key("contrastActiveBorder")),
        "editor.findRangeHighlightBorder" => (None, None, Transparent(&Key("contrastActiveBorder"), 0.4)),
        "editor.rangeHighlightBackground" => (Hex("#ffffff0b"), Hex("#fdff0033"), None),
        "editorWhitespace.foreground" => (Hex("#e3e4e229"), Hex("#33333333"), Hex("#e3e4e229")),
        "editorIndentGuide.background" => (Key("editorWhitespace.foreground"), Key("editorWhitespace.foreground"), Key("editorWhitespace.foreground")),
        "editorWordWrapIndicator.foreground" => (Key("editorWhitespace.foreground"), Key("editorWhitespace.foreground"), Key("editorWhitespace.foreground")),
        "editorIndentGuide.background1" => (Key("editorIndentGuide.background"), Key("editorIndentGuide.background"), Key("editorIndentGuide.background")),
        "editorOverviewRuler.border" => (Hex("#7f7f7f4d"), Hex("#7f7f7f4d"), Hex("#7f7f7f4d")),
        "editorOverviewRuler.selectionHighlightForeground" => (Hex("#A0A0A0CC"), Hex("#A0A0A0CC"), Hex("#A0A0A0CC")),
        "editorOverviewRuler.findMatchForeground" => (Hex("#d186167e"), Hex("#d186167e"), Hex("#AB5A00")),
        "editorOverviewRuler.rangeHighlightForeground" => (Hex("#007acc99"), Hex("#007acc99"), Hex("#007acc99")),
        "editorOverviewRuler.errorForeground" => (Hex("#ff1212b3"), Hex("#ff1212b3"), Hex("#ff3232")),
        "editorOverviewRuler.infoForeground" => (Key("editorInfo.foreground"), Key("editorInfo.foreground"), Key("editorInfo.border")),
        "editorInfo.foreground" => (Hex("#59a4f9"), Hex("#0063d3"), Hex("#59a4f9")),
        "editorInfo.border" => (None, None, Hex("#59a4f9cc")),
        "editorError.foreground" => (Hex("#F14C4C"), Hex("#E51400"), Hex("#F48771")),
        "editorError.border" => (None, None, Hex("#E47777cc")),
        "editorWarning.border" => (None, None, Hex("#FFCC00cc")),
        "editorHint.foreground" => (Hex("#eeeeeeb3"), Hex("#6c6c6c"), None),
        "editorHint.border" => (None, None, Hex("#eeeeeecc")),
        "editorUnnecessaryCode.border" => (None, None, Hex("#ffffffcc")),
        "editorUnnecessaryCode.opacity" => (Hex("#000a"), Hex("#0007"), None),
        "editorOverviewRuler.warningForeground" => (Key("editorWarning.foreground"), Key("editorWarning.foreground"), Key("editorWarning.border")),
        "minimap.errorHighlight" => (Hex("#ff1212b3"), Hex("#ff1212b3"), Hex("#ff3232")),
        "minimap.warningHighlight" => (Key("editorWarning.foreground"), Key("editorWarning.foreground"), Key("editorWarning.border")),
        "minimap.infoHighlight" => (Key("editorInfo.foreground"), Key("editorInfo.foreground"), Key("editorInfo.border")),
        "problemsErrorIcon.foreground" => (Key("editorError.foreground"), Key("editorError.foreground"), Key("editorError.foreground")),
        "problemsWarningIcon.foreground" => (Key("editorWarning.foreground"), Key("editorWarning.foreground"), Key("editorWarning.foreground")),
        "problemsInfoIcon.foreground" => (Key("editorInfo.foreground"), Key("editorInfo.foreground"), Key("editorInfo.foreground")),
        "editorMarkerNavigationError.background" => (Key("editorError.foreground"), Key("editorError.foreground"), Key("contrastBorder")),
        "editorMarkerNavigationError.headerBackground" => (Transparent(&Key("editorMarkerNavigationError.background"), 0.1), Transparent(&Key("editorMarkerNavigationError.background"), 0.1), None),
        "editorMarkerNavigationWarning.background" => (Key("editorWarning.foreground"), Key("editorWarning.foreground"), Key("contrastBorder")),
        "editorMarkerNavigationWarning.headerBackground" => (Transparent(&Key("editorMarkerNavigationWarning.background"), 0.1), Transparent(&Key("editorMarkerNavigationWarning.background"), 0.1), Hex("#0C141F")),
        "editorMarkerNavigationInfo.background" => (Key("editorInfo.foreground"), Key("editorInfo.foreground"), Key("contrastBorder")),
        "editorMarkerNavigationInfo.headerBackground" => (Transparent(&Key("editorMarkerNavigationInfo.background"), 0.1), Transparent(&Key("editorMarkerNavigationInfo.background"), 0.1), None),
        "editorMarkerNavigation.background" => (Key("editor.background"), Key("editor.background"), Key("editor.background")),
        "peekViewTitleLabel.foreground" => (Hex("#FFFFFF"), Hex("#000000"), Hex("#FFFFFF")),
        "peekViewTitleDescription.foreground" => (Hex("#ccccccb3"), Hex("#616161"), Hex("#FFFFFF99")),
        "peekViewTitle.background" | "peekViewResult.background" => (Hex("#252526"), Hex("#F3F3F3"), Hex("#000000")),
        "peekView.border" => (Key("editorInfo.foreground"), Key("editorInfo.foreground"), Key("contrastBorder")),
        "peekViewResult.lineForeground" => (Hex("#bbbbbb"), Hex("#646465"), Hex("#FFFFFF")),
        "peekViewResult.fileForeground" => (Hex("#FFFFFF"), Hex("#1E1E1E"), Hex("#FFFFFF")),
        "peekViewResult.selectionBackground" => (Hex("#3399ff33"), Hex("#3399ff33"), None),
        "peekViewResult.selectionForeground" => (Hex("#FFFFFF"), Hex("#6C6C6C"), Hex("#FFFFFF")),
        "peekViewEditor.background" => (Hex("#001F33"), Hex("#F2F8FC"), Hex("#000000")),
        "peekViewEditorGutter.background" => (Key("peekViewEditor.background"), Key("peekViewEditor.background"), Key("peekViewEditor.background")),
        "peekViewResult.matchHighlightBackground" => (Hex("#ea5c004d"), Hex("#ea5c004d"), None),
        "peekViewEditor.matchHighlightBackground" => (Hex("#ff8f0099"), Hex("#f5d802de"), None),
        "peekViewEditor.matchHighlightBorder" | "editor.symbolHighlightBorder" => (None, None, Key("contrastActiveBorder")),
        "editor.symbolHighlightBackground" => (Key("editor.findMatchHighlightBackground"), Key("editor.findMatchHighlightBackground"), None),
        "editorLink.activeForeground" => (Hex("#4E94CE"), Hex("#0000FF"), Hex("#00FFFF")),
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
        "editorGutter.modifiedBackground" => (Hex("#1B81A8"), Hex("#2090D3"), Hex("#1B81A8")),
        "editorGutter.addedBackground" => (Hex("#487E02"), Hex("#48985D"), Hex("#487E02")),
        "editorGutter.deletedBackground" => (Key("editorError.foreground"), Key("editorError.foreground"), Key("editorError.foreground")),
        // scm/common/quickDiff.ts and editorColors.ts `editorGutter.background`.
        "editorGutter.background" => (Key("editor.background"), Key("editor.background"), Key("editor.background")),
        "editorGutter.modifiedSecondaryBackground" => (Darken(&Key("editorGutter.modifiedBackground"), 0.5), Lighten(&Key("editorGutter.modifiedBackground"), 0.7), Hex("#1B81A8")),
        "editorGutter.addedSecondaryBackground" => (Darken(&Key("editorGutter.addedBackground"), 0.5), Lighten(&Key("editorGutter.addedBackground"), 0.7), Hex("#487E02")),
        "editorGutter.deletedSecondaryBackground" => (Darken(&Key("editorGutter.deletedBackground"), 0.4), Lighten(&Key("editorGutter.deletedBackground"), 0.3), Hex("#F48771")),
        "minimapGutter.modifiedBackground" => (Key("editorGutter.modifiedBackground"), Key("editorGutter.modifiedBackground"), Key("editorGutter.modifiedBackground")),
        "minimapGutter.addedBackground" => (Key("editorGutter.addedBackground"), Key("editorGutter.addedBackground"), Key("editorGutter.addedBackground")),
        "minimapGutter.deletedBackground" => (Key("editorGutter.deletedBackground"), Key("editorGutter.deletedBackground"), Key("editorGutter.deletedBackground")),
        "editorOverviewRuler.modifiedForeground" => (Transparent(&Key("editorGutter.modifiedBackground"), 0.6), Transparent(&Key("editorGutter.modifiedBackground"), 0.6), Transparent(&Key("editorGutter.modifiedBackground"), 0.6)),
        "editorOverviewRuler.addedForeground" => (Transparent(&Key("editorGutter.addedBackground"), 0.6), Transparent(&Key("editorGutter.addedBackground"), 0.6), Transparent(&Key("editorGutter.addedBackground"), 0.6)),
        "editorOverviewRuler.deletedForeground" => (Transparent(&Key("editorGutter.deletedBackground"), 0.6), Transparent(&Key("editorGutter.deletedBackground"), 0.6), Transparent(&Key("editorGutter.deletedBackground"), 0.6)),
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
        "diffEditor.removedLineBackground" => (Hex("#ff000033"), Hex("#ff000033"), None),
        "diffEditor.insertedTextBorder" => (None, None, Hex("#33ff2eff")),
        "diffEditor.removedTextBorder" => (None, None, Hex("#FF008F")),
        "diffEditor.border" => (None, None, Key("contrastBorder")),
        "diffEditor.diagonalFill" => (Hex("#cccccc33"), Hex("#22222233"), None),
        "diffEditor.unchangedRegionBackground" => (Key("sideBar.background"), Key("sideBar.background"), Key("sideBar.background")),
        "diffEditor.unchangedRegionForeground" => (Key("foreground"), Key("foreground"), Key("foreground")),
        "diffEditor.unchangedRegionShadow" => (Hex("#000000"), Hex("#737373BF"), Hex("#000000")),
        "diffEditor.unchangedCodeBackground" => (Hex("#74747429"), Hex("#b8b8b829"), None),
        // mergeEditor/browser/view/colors.ts.
        "mergeEditor.change.background" => (Hex("#9bb95533"), Hex("#9bb95533"), Hex("#9bb95533")),
        "mergeEditor.change.word.background" => (Hex("#9ccc2c33"), Hex("#9ccc2c66"), Hex("#9ccc2c33")),
        "mergeEditor.conflict.unhandledUnfocused.border" => (Hex("#ffa6007a"), Hex("#ffa600FF"), Hex("#ffa6007a")),
        "mergeEditor.conflict.unhandledFocused.border" => (Hex("#ffa600"), Hex("#ffa600"), Hex("#ffa600")),
        "mergeEditor.conflict.handledUnfocused.border" => (Hex("#86868649"), Hex("#86868649"), Hex("#86868649")),
        "mergeEditor.conflict.handledFocused.border" => (Hex("#c1c1c1cc"), Hex("#c1c1c1cc"), Hex("#c1c1c1cc")),
        // multiDiffEditor/colors.ts.
        "multiDiffEditor.headerBackground" => (Hex("#262626"), Key("tab.inactiveBackground"), Key("tab.inactiveBackground")),
        "multiDiffEditor.background" => (Key("editor.background"), Key("editor.background"), Key("editor.background")),
        "multiDiffEditor.border" => (Key("sideBarSectionHeader.border"), Hex("#cccccc"), Key("sideBarSectionHeader.border")),
        // editorColors.ts merge conflict colors: headers at 0.5 (common 0.4), contents at 0.4 of their header.
        "merge.currentHeaderBackground" => (Transparent(&Hex("#40C8AE"), 0.5), Transparent(&Hex("#40C8AE"), 0.5), None),
        "merge.incomingHeaderBackground" => (Transparent(&Hex("#40A6FF"), 0.5), Transparent(&Hex("#40A6FF"), 0.5), None),
        "merge.commonHeaderBackground" => (Transparent(&Hex("#606060"), 0.4), Transparent(&Hex("#606060"), 0.4), None),
        "merge.currentContentBackground" => (Transparent(&Key("merge.currentHeaderBackground"), 0.4), Transparent(&Key("merge.currentHeaderBackground"), 0.4), Transparent(&Key("merge.currentHeaderBackground"), 0.4)),
        "merge.incomingContentBackground" => (Transparent(&Key("merge.incomingHeaderBackground"), 0.4), Transparent(&Key("merge.incomingHeaderBackground"), 0.4), Transparent(&Key("merge.incomingHeaderBackground"), 0.4)),
        "merge.commonContentBackground" => (Transparent(&Key("merge.commonHeaderBackground"), 0.4), Transparent(&Key("merge.commonHeaderBackground"), 0.4), Transparent(&Key("merge.commonHeaderBackground"), 0.4)),
        "merge.border" => (None, None, Hex("#C3DF6F")),
        "editorOverviewRuler.currentContentForeground" => (Key("merge.currentHeaderBackground"), Key("merge.currentHeaderBackground"), Key("merge.border")),
        "editorOverviewRuler.incomingContentForeground" => (Key("merge.incomingHeaderBackground"), Key("merge.incomingHeaderBackground"), Key("merge.border")),
        "editorOverviewRuler.commonContentForeground" => (Key("merge.commonHeaderBackground"), Key("merge.commonHeaderBackground"), Key("merge.border")),
        // symbolIcons.ts; the kinds not listed take `foreground`.
        "symbolIcon.classForeground" | "symbolIcon.enumeratorForeground" | "symbolIcon.eventForeground" => (Hex("#EE9D28"), Hex("#D67E00"), Hex("#EE9D28")),
        "symbolIcon.constructorForeground" | "symbolIcon.functionForeground" | "symbolIcon.methodForeground" => (Hex("#B180D7"), Hex("#652D90"), Hex("#B180D7")),
        "symbolIcon.enumeratorMemberForeground" | "symbolIcon.fieldForeground" | "symbolIcon.interfaceForeground" | "symbolIcon.variableForeground" => (Hex("#75BEFF"), Hex("#007ACC"), Hex("#75BEFF")),
        symbol if symbol.starts_with("symbolIcon.") => (Key("foreground"), Key("foreground"), Key("foreground")),
        "editorGutter.itemGlyphForeground" => (Key("editor.foreground"), Key("editor.foreground"), Hex("#000000")),
        "editorGutter.itemBackground" => (
            Opaque(&Key("list.inactiveSelectionBackground"), &Key("editor.background")),
            Darken(&Opaque(&Key("list.inactiveSelectionBackground"), &Key("editor.background")), 0.05),
            Hex("#FFFFFF"),
        ),
        "textLink.foreground" => (Hex("#3794FF"), Hex("#006AB1"), Hex("#21A6FF")),
        "textLink.activeForeground" => (Hex("#3794FF"), Hex("#006AB1"), Hex("#21A6FF")),
        "search.resultsInfoForeground" => (Transparent(&Key("foreground"), 0.65), Key("foreground"), Key("foreground")),
        "inputOption.activeBorder" => (Hex("#007ACC"), Hex("#007ACC"), Key("contrastBorder")),
        "inputOption.hoverBackground" => (Hex("#5a5d5e80"), Hex("#b8b8b850"), None),
        "inputOption.activeBackground" => (Transparent(&Key("focusBorder"), 0.4), Transparent(&Key("focusBorder"), 0.2), Hex("#0000")),
        "inputOption.activeForeground" => (Hex("#FFFFFF"), Hex("#000000"), Key("foreground")),
        // workbench/common/theme.ts, for the notification toasts and center.
        "notificationCenter.border" | "notificationToast.border" => (Key("widget.border"), Key("widget.border"), Key("contrastBorder")),
        "notifications.foreground" => (Key("editorWidget.foreground"), Key("editorWidget.foreground"), Key("editorWidget.foreground")),
        "notifications.background" => (Key("editorWidget.background"), Key("editorWidget.background"), Key("editorWidget.background")),
        "notificationLink.foreground" => (Key("textLink.foreground"), Key("textLink.foreground"), Key("textLink.foreground")),
        "notificationCenterHeader.background" => (Lighten(&Key("notifications.background"), 0.3), Darken(&Key("notifications.background"), 0.05), Key("notifications.background")),
        "notifications.border" => (Key("notificationCenterHeader.background"), Key("notificationCenterHeader.background"), Key("notificationCenterHeader.background")),
        "notificationsErrorIcon.foreground" => (Key("editorError.foreground"), Key("editorError.foreground"), Key("editorError.foreground")),
        "notificationsWarningIcon.foreground" => (Key("editorWarning.foreground"), Key("editorWarning.foreground"), Key("editorWarning.foreground")),
        "notificationsInfoIcon.foreground" => (Key("editorInfo.foreground"), Key("editorInfo.foreground"), Key("editorInfo.foreground")),
        "charts.blue" => (Key("editorInfo.foreground"), Key("editorInfo.foreground"), Key("editorInfo.foreground")),
        "charts.purple" => (Hex("#B180D7"), Hex("#652D90"), Hex("#B180D7")),
        "scmGraph.historyItemRefColor" => (Key("charts.blue"), Key("charts.blue"), Key("charts.blue")),
        "scmGraph.historyItemRemoteRefColor" => (Key("charts.purple"), Key("charts.purple"), Key("charts.purple")),
        "scmGraph.historyItemBaseRefColor" => (Hex("#EA5C00"), Hex("#EA5C00"), Hex("#EA5C00")),
        "scmGraph.historyItemHoverDefaultLabelBackground" => (Key("badge.background"), Key("badge.background"), Key("badge.background")),
        "scmGraph.historyItemHoverLabelForeground" => (Key("panel.background"), Key("panel.background"), Key("panel.background")),
        "scmGraph.foreground1" => (Hex("#FFB000"), Hex("#FFB000"), Hex("#FFB000")),
        "scmGraph.foreground2" => (Hex("#DC267F"), Hex("#DC267F"), Hex("#DC267F")),
        "scmGraph.foreground3" => (Hex("#994F00"), Hex("#994F00"), Hex("#994F00")),
        "scmGraph.foreground4" => (Hex("#40B0A6"), Hex("#40B0A6"), Hex("#40B0A6")),
        "scmGraph.foreground5" => (Hex("#B66DFF"), Hex("#B66DFF"), Hex("#B66DFF")),
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
        "editorGroup.dropBackground" => Transparent(&Hex("#0F4A85"), 0.5),
        "sideBar.dropBackground" => Key("editorGroup.dropBackground"),
        "disabledForeground" => Hex("#7F7F7F"),
        "icon.foreground" => Hex("#292929"),
        "editorGutter.foldingControlForeground" => Key("icon.foreground"),
        "editorGutter.modifiedBackground" => Hex("#2090D3"),
        "diffEditor.insertedTextBorder" => Hex("#374E06"),
        "diffEditor.removedTextBorder" => Hex("#AD0707"),
        "diffEditor.border" => Key("contrastBorder"),
        "diffEditor.unchangedRegionBackground" => Key("sideBar.background"),
        "diffEditor.unchangedRegionForeground" => Key("foreground"),
        "diffEditor.unchangedRegionShadow" => Hex("#737373BF"),
        "mergeEditor.change.background" => Hex("#9bb95533"),
        "mergeEditor.change.word.background" => Hex("#9ccc2c66"),
        "mergeEditor.conflict.unhandledUnfocused.border" => Hex("#ffa6007a"),
        "mergeEditor.conflict.unhandledFocused.border" => Hex("#ffa600"),
        "mergeEditor.conflict.handledUnfocused.border" => Hex("#86868649"),
        "mergeEditor.conflict.handledFocused.border" => Hex("#c1c1c1cc"),
        "multiDiffEditor.headerBackground" => Key("tab.inactiveBackground"),
        "multiDiffEditor.background" => Key("editor.background"),
        "multiDiffEditor.border" => Hex("#cccccc"),
        "merge.currentContentBackground" => Transparent(&Key("merge.currentHeaderBackground"), 0.4),
        "merge.incomingContentBackground" => Transparent(&Key("merge.incomingHeaderBackground"), 0.4),
        "merge.commonContentBackground" => Transparent(&Key("merge.commonHeaderBackground"), 0.4),
        "merge.border" => Hex("#007ACC"),
        "editorOverviewRuler.currentContentForeground" | "editorOverviewRuler.incomingContentForeground" | "editorOverviewRuler.commonContentForeground" => Key("merge.border"),
        "symbolIcon.classForeground" | "symbolIcon.enumeratorForeground" | "symbolIcon.eventForeground" => Hex("#D67E00"),
        "symbolIcon.constructorForeground" | "symbolIcon.functionForeground" | "symbolIcon.methodForeground" => Hex("#652D90"),
        "symbolIcon.enumeratorMemberForeground" | "symbolIcon.fieldForeground" | "symbolIcon.interfaceForeground" | "symbolIcon.variableForeground" => Hex("#007ACC"),
        symbol if symbol.starts_with("symbolIcon.") => Key("foreground"),
        "editorGutter.itemGlyphForeground" => Hex("#FFFFFF"),
        "editorGutter.itemBackground" => Hex("#000000"),
        "editorGutter.addedBackground" => Hex("#48985D"),
        "editorGutter.deletedBackground" => Key("editorError.foreground"),
        "editorGutter.background" => Key("editor.background"),
        "editorGutter.modifiedSecondaryBackground" => Hex("#2090D3"),
        "editorGutter.addedSecondaryBackground" => Hex("#48985D"),
        "editorGutter.deletedSecondaryBackground" => Hex("#B5200D"),
        "minimapGutter.modifiedBackground" => Key("editorGutter.modifiedBackground"),
        "minimapGutter.addedBackground" => Key("editorGutter.addedBackground"),
        "minimapGutter.deletedBackground" => Key("editorGutter.deletedBackground"),
        "editorOverviewRuler.modifiedForeground" => Transparent(&Key("editorGutter.modifiedBackground"), 0.6),
        "editorOverviewRuler.addedForeground" => Transparent(&Key("editorGutter.addedBackground"), 0.6),
        "editorOverviewRuler.deletedForeground" => Transparent(&Key("editorGutter.deletedBackground"), 0.6),
        "editorStickyScroll.background" => Key("editor.background"),
        "editorStickyScrollGutter.background" => Key("editor.background"),
        "editorStickyScrollHover.background" => Transparent(&Hex("#0F4A85"), 0.1),
        "editorStickyScroll.border" => Key("contrastBorder"),
        "editorStickyScroll.shadow" => Key("scrollbar.shadow"),
        "editor.background" => Hex("#ffffff"),
        "sideBar.background" => Hex("#FFFFFF"),
        "panel.background" => Key("editor.background"),
        "panelTitle.activeForeground" => Key("editor.foreground"),
        "panelTitle.inactiveForeground" => Key("editor.foreground"),
        "activityBarTop.foreground" => Key("editor.foreground"),
        "activityBarTop.inactiveForeground" => Key("editor.foreground"),
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
        "modernSash.gripForeground" => Key("foreground"),
        "statusBar.foreground" => Key("editor.foreground"),
        "statusBar.focusBorder" => Key("statusBar.foreground"),
        "statusBarItem.prominentForeground" => Key("statusBar.foreground"),
        "statusBarItem.prominentBackground" => Hex("#00000080"),
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
        "editorError.foreground" => Hex("#B5200D"),
        "editorError.border" => Hex("#B5200D"),
        "editorWarning.border" => Hex("#FFCC00cc"),
        "editorHint.border" => Hex("#292929"),
        "editorUnnecessaryCode.border" => Key("contrastBorder"),
        "editorOverviewRuler.warningForeground" => Key("editorWarning.border"),
        "minimap.errorHighlight" => Hex("#B5200D"),
        "minimap.warningHighlight" => Key("editorWarning.border"),
        "minimap.infoHighlight" => Key("editorInfo.border"),
        "editorMarkerNavigationWarning.headerBackground" => Transparent(&Key("editorMarkerNavigationWarning.background"), 0.2),
        "peekViewTitleLabel.foreground" => Key("editor.foreground"),
        "peekViewTitleDescription.foreground" => Hex("#292929"),
        "peekViewTitle.background" | "peekViewResult.background" | "peekViewEditor.background" => Hex("#FFFFFF"),
        "peekView.border" => Key("contrastBorder"),
        "peekViewResult.lineForeground" | "peekViewResult.fileForeground" | "peekViewResult.selectionForeground" => Key("editor.foreground"),
        "peekViewEditorGutter.background" => Key("peekViewEditor.background"),
        "peekViewEditor.matchHighlightBorder" | "editor.symbolHighlightBorder" => Key("contrastActiveBorder"),
        "editorLink.activeForeground" => Hex("#292929"),
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
        "modernActivityBar.background" => Key("activityBar.background"),
        "modernActivityBar.inactiveBackground" => Key("modernActivityBar.background"),
        "modernActivityBar.border" => Key("surface.border"),
        "modernActivityBarItem.activeForeground" => Key("modernTab.activeForeground"),
        "editor.foreground" => Key("foreground"),
        "editorLineNumber.foreground" => Hex("#292929"),
        "editorActiveLineNumber.foreground" => Key("contrastActiveBorder"),
        "editorLineNumber.activeForeground" => Key("editorActiveLineNumber.foreground"),
        "editor.lineHighlightBorder" => Key("contrastBorder"),
        "editor.inactiveLineHighlightBackground" => Key("editor.lineHighlightBackground"),
        "editorCursor.foreground" => Hex("#0F4A85"),
        "editorMultiCursor.primary.foreground" => Key("editorCursor.foreground"),
        "editorMultiCursor.secondary.foreground" => Key("editorCursor.foreground"),
        "editor.wordHighlightBorder" | "editor.selectionHighlightBorder" | "editor.wordHighlightStrongBorder" => Key("contrastActiveBorder"),
        "editorOverviewRuler.wordHighlightForeground" => Hex("#A0A0A0CC"),
        "editorOverviewRuler.wordHighlightStrongForeground" => Hex("#C0A0C0CC"),
        "editor.wordHighlightTextBorder" => Key("editor.wordHighlightBorder"),
        "editor.findMatchBorder" | "editor.findMatchHighlightBorder" | "editor.rangeHighlightBorder" => Key("contrastActiveBorder"),
        "editor.findRangeHighlightBorder" => Transparent(&Key("contrastActiveBorder"), 0.4),
        "editorWhitespace.foreground" => Hex("#CCCCCC"),
        "editorIndentGuide.background" => Key("editorWhitespace.foreground"),
        "editorWordWrapIndicator.foreground" => Key("editorWhitespace.foreground"),
        "editorIndentGuide.background1" => Key("editorIndentGuide.background"),
        "editorOverviewRuler.border" => Hex("#666666"),
        "editorOverviewRuler.selectionHighlightForeground" => Hex("#A0A0A0CC"),
        "editorOverviewRuler.findMatchForeground" => Hex("#AB5A00"),
        "minimap.findMatchHighlight" => Key("editor.findMatchHighlightBorder"),
        "searchEditor.findMatchBackground" => Key("editor.findMatchHighlightBackground"),
        "terminal.findMatchBackground" => Key("editor.findMatchBackground"),
        "terminal.findMatchHighlightBackground" => Key("editor.findMatchHighlightBackground"),
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
        "notificationCenter.border" | "notificationToast.border" => Key("contrastBorder"),
        "notifications.foreground" => Key("editorWidget.foreground"),
        "notifications.background" | "notificationCenterHeader.background" => Key("editorWidget.background"),
        "notificationLink.foreground" => Key("textLink.foreground"),
        "notifications.border" => Key("notificationCenterHeader.background"),
        "notificationsErrorIcon.foreground" => Key("editorError.foreground"),
        "charts.blue" => Key("editorInfo.foreground"),
        "charts.purple" => Hex("#652D90"),
        "scmGraph.historyItemRefColor" => Key("charts.blue"),
        "scmGraph.historyItemRemoteRefColor" => Key("charts.purple"),
        "scmGraph.historyItemBaseRefColor" => Hex("#EA5C00"),
        "scmGraph.historyItemHoverDefaultLabelBackground" => Key("badge.background"),
        "scmGraph.historyItemHoverLabelForeground" => Key("panel.background"),
        "scmGraph.foreground1" => Hex("#FFB000"),
        "scmGraph.foreground2" => Hex("#DC267F"),
        "scmGraph.foreground3" => Hex("#994F00"),
        "scmGraph.foreground4" => Hex("#40B0A6"),
        "scmGraph.foreground5" => Hex("#B66DFF"),
        "notificationsWarningIcon.foreground" => Key("editorWarning.foreground"),
        "notificationsInfoIcon.foreground" => Key("editorInfo.foreground"),
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
        "radio.activeForeground" => (Key("inputOption.activeForeground"), Key("inputOption.activeForeground"), Key("inputOption.activeForeground")),
        "radio.activeBackground" => (Key("inputOption.activeBackground"), Key("inputOption.activeBackground"), Key("inputOption.activeBackground")),
        "radio.activeBorder" => (Key("inputOption.activeBorder"), Key("inputOption.activeBorder"), Key("inputOption.activeBorder")),
        "radio.inactiveBorder" => (Transparent(&Key("radio.activeForeground"), 0.2), Transparent(&Key("radio.activeForeground"), 0.2), Transparent(&Key("radio.activeForeground"), 0.4)),
        "editorWidget.border" => (Transparent(&Key("editorWidget.foreground"), 0.2), Transparent(&Key("editorWidget.foreground"), 0.2), Key("contrastBorder")),
        "errorForeground" => (Hex("#F48771"), Hex("#A1260D"), Hex("#F48771")),
        "list.errorForeground" => (Hex("#F88070"), Hex("#B01011"), None),
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
        "button.hoverBackground" => (Lighten(&Key("button.background"), 0.2), Darken(&Key("button.background"), 0.2), Key("button.background")),
        "button.separator" => (Transparent(&Key("button.foreground"), 0.4), Transparent(&Key("button.foreground"), 0.4), Key("contrastBorder")),
        "button.secondaryForeground" => (Key("foreground"), Key("foreground"), Hex("#FFFFFF")),
        "button.secondaryBackground" => (Key("list.hoverBackground"), Key("list.hoverBackground"), None),
        "button.secondaryBorder" => (Transparent(&Key("foreground"), 0.15), Transparent(&Key("foreground"), 0.15), Key("contrastBorder")),
        "button.secondaryHoverBackground" => (Lighten(&Key("list.hoverBackground"), 0.2), Lighten(&Key("list.hoverBackground"), 0.2), None),
        "extensionButton.background" => (Key("button.secondaryBackground"), Key("button.secondaryBackground"), None),
        "extensionButton.foreground" => (Key("button.secondaryForeground"), Key("button.secondaryForeground"), None),
        "extensionButton.hoverBackground" => (Key("button.secondaryHoverBackground"), Key("button.secondaryHoverBackground"), None),
        "extensionButton.border" => (Key("button.secondaryBorder"), Key("button.secondaryBorder"), Key("button.secondaryBorder")),
        "extensionButton.separator" => (Key("button.separator"), Key("button.separator"), Key("button.separator")),
        "extensionButton.prominentBackground" => (Key("button.background"), Key("button.background"), None),
        "extensionButton.prominentForeground" => (Key("button.foreground"), Key("button.foreground"), None),
        "extensionButton.prominentHoverBackground" => (Key("button.hoverBackground"), Key("button.hoverBackground"), None),
        "extensionIcon.starForeground" => (Hex("#FF8E00"), Hex("#DF6100"), Hex("#FF8E00")),
        "extensionIcon.preReleaseForeground" => (Hex("#1d9271"), Hex("#1d9271"), Hex("#1d9271")),
        "extensionIcon.verifiedForeground" => (Key("textLink.foreground"), Key("textLink.foreground"), Key("textLink.foreground")),
        "activityBarBadge.background" => (Hex("#007ACC"), Hex("#007ACC"), Hex("#000000")),
        "activityBarBadge.foreground" => (Hex("#FFFFFF"), Hex("#FFFFFF"), Hex("#FFFFFF")),
        "profileBadge.background" => (Hex("#4D4D4D"), Hex("#C4C4C4"), Hex("#FFFFFF")),
        "profileBadge.foreground" => (Hex("#FFFFFF"), Hex("#333333"), Hex("#000000")),
        "panel.border" => (Hex("#80808059"), Hex("#80808059"), Key("contrastBorder")),
        "panelSection.border" => (Key("panel.border"), Key("panel.border"), Key("panel.border")),
        "panelTitle.activeForeground" => (Hex("#E7E7E7"), Hex("#424242"), Hex("#FFFFFF")),
        "panelTitle.inactiveForeground" => (Transparent(&Key("panelTitle.activeForeground"), 0.6), Transparent(&Key("panelTitle.activeForeground"), 0.75), Hex("#FFFFFF")),
        "panelTitle.activeBorder" => (Key("panelTitle.activeForeground"), Key("panelTitle.activeForeground"), Key("contrastBorder")),
        "checkbox.background" => (Key("dropdown.background"), Key("dropdown.background"), Key("dropdown.background")),
        "checkbox.foreground" => (Key("dropdown.foreground"), Key("dropdown.foreground"), Key("dropdown.foreground")),
        "checkbox.border" => (Key("dropdown.border"), Key("dropdown.border"), Key("dropdown.border")),
        "toolbar.hoverBackground" => (Hex("#5a5d5e50"), Hex("#b8b8b850"), None),
        "dropdown.background" => (Hex("#3C3C3C"), Hex("#FFFFFF"), Hex("#000000")),
        "dropdown.foreground" => (Hex("#F0F0F0"), Key("foreground"), Hex("#FFFFFF")),
        "dropdown.border" => (Key("dropdown.background"), Hex("#CECECE"), Key("contrastBorder")),
        // menuColors.ts and editorWidget.border, the menu's fallback border (defaultStyles.ts defaultMenuStyles).
        "menu.border" => (None, None, Key("contrastBorder")),
        "menu.foreground" => (Key("dropdown.foreground"), Key("dropdown.foreground"), Key("dropdown.foreground")),
        "menu.background" => (Key("dropdown.background"), Key("dropdown.background"), Key("dropdown.background")),
        "menu.selectionBorder" => (None, None, Key("contrastActiveBorder")),
        "menu.separatorBackground" => (Transparent(&Key("foreground"), 0.2), Transparent(&Key("foreground"), 0.2), Key("contrastBorder")),
        // editorColors.ts, for the workbench hover (hover.css).
        "editorHoverWidget.background" => (Key("editorWidget.background"), Key("editorWidget.background"), Key("editorWidget.background")),
        "editorHoverWidget.foreground" => (Key("editorWidget.foreground"), Key("editorWidget.foreground"), Key("editorWidget.foreground")),
        "editorHoverWidget.border" => (Key("editorWidget.border"), Key("editorWidget.border"), Key("editorWidget.border")),
        "editorHoverWidget.highlightForeground" => (Key("list.highlightForeground"), Key("list.highlightForeground"), Key("list.highlightForeground")),
        "editorLightBulb.foreground" => (Hex("#FFCC00"), Hex("#DDB100"), Hex("#FFCC00")),
        "editorCodeLens.foreground" => (Hex("#999999"), Hex("#919191"), Hex("#999999")),
        "editorLink.activeForeground" => (Hex("#4E94CE"), Hex("#0000FF"), Hex("#00FFFF")),
        "editorInlayHint.foreground" => (Hex("#969696"), Hex("#969696"), Hex("#FFFFFF")),
        "commentsView.resolvedIcon" => (Key("disabledForeground"), Key("disabledForeground"), Key("contrastBorder")),
        "commentsView.unresolvedIcon" => (Key("list.focusOutline"), Key("list.focusOutline"), Key("contrastBorder")),
        "editorGutter.commentRangeForeground" => (Opaque(&Key("list.inactiveSelectionBackground"), &Key("editor.background")), Darken(&Opaque(&Key("list.inactiveSelectionBackground"), &Key("editor.background")), 0.05), Hex("#FFFFFF")),
        "editorGutter.commentGlyphForeground" => (Key("editor.foreground"), Key("editor.foreground"), Hex("#000000")),
        "editorGutter.commentUnresolvedGlyphForeground" | "editorGutter.commentDraftGlyphForeground" => (Key("editorGutter.commentGlyphForeground"), Key("editorGutter.commentGlyphForeground"), Key("editorGutter.commentGlyphForeground")),
        "editorGhostText.foreground" => (Hex("#ffffff56"), Hex("#0007"), None),
        "editorGhostText.border" => (None, None, Transparent(&Hex("#FFFFFF"), 0.8)),
        "editorInlayHint.background" => (Transparent(&Key("badge.background"), 0.1), Transparent(&Key("badge.background"), 0.1), Transparent(&Hex("#FFFFFF"), 0.1)),
        "editorInlayHint.typeForeground" | "editorInlayHint.parameterForeground" => (Key("editorInlayHint.foreground"), Key("editorInlayHint.foreground"), Key("editorInlayHint.foreground")),
        "editorInlayHint.typeBackground" | "editorInlayHint.parameterBackground" => (Key("editorInlayHint.background"), Key("editorInlayHint.background"), Key("editorInlayHint.background")),
        "editor.linkedEditingBackground" => (Transparent(&Hex("#FF0000"), 0.3), Transparent(&Hex("#FF0000"), 0.3), Transparent(&Hex("#FF0000"), 0.3)),
        "editorLightBulbAutoFix.foreground" => (Hex("#75BEFF"), Hex("#007ACC"), Hex("#75BEFF")),
        "editorLightBulbAi.foreground" => (Key("editorLightBulb.foreground"), Key("editorLightBulb.foreground"), Key("editorLightBulb.foreground")),
        // `lighten(editorHoverBackground, 0.2)` is a darken by -0.2: both scale the lightness by the factor.
        "editorHoverWidget.statusBarBackground" => (Darken(&Key("editorHoverWidget.background"), -0.2), Darken(&Key("editorHoverWidget.background"), 0.05), Key("editorWidget.background")),
        "editor.hoverHighlightBackground" => (Hex("#264f7840"), Hex("#ADD6FF26"), Hex("#ADD6FF26")),
        "textCodeBlock.background" => (Hex("#0a0a0a66"), Hex("#dcdcdc66"), Hex("#000000")),
        "textLink.foreground" => (Hex("#3794FF"), Hex("#006AB1"), Hex("#21A6FF")),
        "textLink.activeForeground" => (Hex("#3794FF"), Hex("#006AB1"), Hex("#21A6FF")),
        // listColors.ts, editorColors.ts and keybindingsEditor.ts, for the Keyboard Shortcuts editor.
        "list.focusOutline" => (Key("focusBorder"), Key("focusBorder"), Key("contrastActiveBorder")),
        "widget.shadow" => (Hex("#0000005C"), Hex("#00000029"), None),
        // listColors.ts, for the tree find widget and its matches.
        "listFilterWidget.background" => (Key("editorWidget.background"), Key("editorWidget.background"), Key("editorWidget.background")),
        "listFilterWidget.shadow" => (Key("widget.shadow"), Key("widget.shadow"), Key("widget.shadow")),
        "list.filterMatchBackground" => (Key("editor.findMatchHighlightBackground"), Key("editor.findMatchHighlightBackground"), None),
        "list.filterMatchBorder" => (Key("editor.findMatchHighlightBorder"), Key("editor.findMatchHighlightBorder"), Key("contrastBorder")),
        "tree.tableOddRowsBackground" => (Transparent(&Key("foreground"), 0.04), Transparent(&Key("foreground"), 0.04), None),
        "tree.tableColumnsBorder" => (Hex("#CCCCCC20"), Hex("#61616120"), None),
        "tree.indentGuidesStroke" => (Hex("#585858"), Hex("#A9A9A9"), Hex("#A9A9A9")),
        "tree.inactiveIndentGuidesStroke" => {
            (Transparent(&Key("tree.indentGuidesStroke"), 0.4), Transparent(&Key("tree.indentGuidesStroke"), 0.4), Transparent(&Key("tree.indentGuidesStroke"), 0.4))
        }
        "keybindingTable.headerBackground" | "keybindingTable.rowsBackground" => {
            (Key("tree.tableOddRowsBackground"), Key("tree.tableOddRowsBackground"), Key("tree.tableOddRowsBackground"))
        }
        // settingsEditorColorRegistry.ts and baseColors.ts, for the Settings editor.
        "settings.headerForeground" => (Hex("#e7e7e7"), Hex("#444444"), Hex("#ffffff")),
        "settings.modifiedItemIndicator" => (Hex("#0C7D9D"), Hex("#66AFE0"), Hex("#00497A")),
        "settings.settingsHeaderHoverForeground" => (Transparent(&Key("settings.headerForeground"), 0.7), Transparent(&Key("settings.headerForeground"), 0.7), Transparent(&Key("settings.headerForeground"), 0.7)),
        "settings.headerBorder" | "settings.sashBorder" => (Key("panel.border"), Key("panel.border"), Key("panel.border")),
        "modernTab.hoverBackground" => (Key("list.hoverBackground"), Key("list.hoverBackground"), Key("list.hoverBackground")),
        "modernTab.hoverForeground" => (Key("list.hoverForeground"), Key("list.hoverForeground"), Key("list.hoverForeground")),
        "settings.focusedRowBackground" => (Transparent(&Key("list.hoverBackground"), 0.6), Transparent(&Key("list.hoverBackground"), 0.6), None),
        "settings.rowHoverBackground" => (Transparent(&Key("list.hoverBackground"), 0.3), Transparent(&Key("list.hoverBackground"), 0.3), None),
        "settings.focusedRowBorder" => (Key("focusBorder"), Key("focusBorder"), Key("focusBorder")),
        "settings.checkboxBackground" => (Key("checkbox.background"), Key("checkbox.background"), Key("checkbox.background")),
        "settings.checkboxForeground" => (Key("checkbox.foreground"), Key("checkbox.foreground"), Key("checkbox.foreground")),
        "settings.checkboxBorder" => (Key("checkbox.border"), Key("checkbox.border"), Key("checkbox.border")),
        "settings.dropdownBackground" => (Key("select.background"), Key("select.background"), Key("select.background")),
        "settings.dropdownForeground" => (Key("select.foreground"), Key("select.foreground"), Key("select.foreground")),
        "settings.dropdownBorder" => (Key("select.border"), Key("select.border"), Key("select.border")),
        "settings.textInputBackground" | "settings.numberInputBackground" => (Key("input.background"), Key("input.background"), Key("input.background")),
        "settings.textInputForeground" | "settings.numberInputForeground" => (Key("input.foreground"), Key("input.foreground"), Key("input.foreground")),
        "settings.textInputBorder" | "settings.numberInputBorder" => (Key("input.border"), Key("input.border"), Key("input.border")),
        "textPreformat.foreground" => (Hex("#D7BA7D"), Hex("#A31515"), Hex("#FFFFFF")),
        "textPreformat.background" => (Hex("#FFFFFF1A"), Hex("#0000001A"), None),
        "textPreformat.border" => (None, None, Key("contrastBorder")),
        "editorSuggestWidget.background" => (Key("editorWidget.background"), Key("editorWidget.background"), Key("editorWidget.background")),
        "editorSuggestWidget.border" => (Key("editorWidget.border"), Key("editorWidget.border"), Key("editorWidget.border")),
        "editorSuggestWidget.foreground" => (Key("editor.foreground"), Key("editor.foreground"), Key("editor.foreground")),
        "editorSuggestWidget.selectedForeground" => (Key("quickInputList.focusForeground"), Key("quickInputList.focusForeground"), Key("editorSuggestWidget.background")),
        "editorSuggestWidget.selectedBackground" => (Key("quickInputList.focusBackground"), Key("quickInputList.focusBackground"), Key("editorSuggestWidget.foreground")),
        "editorSuggestWidget.highlightForeground" => (Key("list.highlightForeground"), Key("list.highlightForeground"), Key("list.highlightForeground")),
        "editorSuggestWidget.focusHighlightForeground" => (Key("list.focusHighlightForeground"), Key("list.focusHighlightForeground"), Key("editorSuggestWidget.selectedForeground")),
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
        "radio.activeForeground" => Key("inputOption.activeForeground"),
        "radio.activeBackground" => Key("inputOption.activeBackground"),
        "radio.activeBorder" => Key("inputOption.activeBorder"),
        "radio.inactiveBorder" => Transparent(&Key("radio.activeForeground"), 0.2),
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
        "button.hoverBackground" => Key("button.background"),
        "button.separator" => Key("contrastBorder"),
        "button.secondaryForeground" => Key("foreground"),
        "button.secondaryBackground" => Hex("#FFFFFF"),
        "button.secondaryBorder" | "extensionButton.border" => Key("contrastBorder"),
        "extensionButton.separator" => Key("button.separator"),
        "extensionIcon.starForeground" | "extensionIcon.preReleaseForeground" | "extensionIcon.verifiedForeground" => Key("textLink.foreground"),
        "activityBarBadge.background" => Hex("#0F4A85"),
        "activityBarBadge.foreground" => Hex("#FFFFFF"),
        "profileBadge.background" => Hex("#000000"),
        "profileBadge.foreground" => Hex("#FFFFFF"),
        "panel.border" | "panelSection.border" => Key("contrastBorder"),
        "panelTitle.activeForeground" | "panelTitle.inactiveForeground" => Key("editor.foreground"),
        "panelTitle.activeBorder" => Hex("#B5200D"),
        "checkbox.background" => Key("dropdown.background"),
        "checkbox.foreground" => Key("dropdown.foreground"),
        "checkbox.border" => Key("dropdown.border"),
        "dropdown.background" => Hex("#FFFFFF"),
        "dropdown.foreground" => Key("foreground"),
        "dropdown.border" => Key("contrastBorder"),
        "menu.border" | "menu.separatorBackground" => Key("contrastBorder"),
        "menu.foreground" => Key("dropdown.foreground"),
        "menu.background" => Key("dropdown.background"),
        "menu.selectionBorder" => Key("contrastActiveBorder"),
        "editorHoverWidget.background" => Key("editorWidget.background"),
        "editorHoverWidget.foreground" => Key("editorWidget.foreground"),
        "editorHoverWidget.border" => Key("editorWidget.border"),
        "editorHoverWidget.highlightForeground" => Key("list.highlightForeground"),
        "editorLightBulb.foreground" | "editorLightBulbAutoFix.foreground" => Hex("#007ACC"),
        "editorCodeLens.foreground" | "editorLink.activeForeground" => Hex("#292929"),
        "editorInlayHint.foreground" => Hex("#000000"),
        "commentsView.resolvedIcon" | "commentsView.unresolvedIcon" => Key("contrastBorder"),
        "editorGutter.commentRangeForeground" => Hex("#000000"),
        "editorGutter.commentGlyphForeground" => Hex("#FFFFFF"),
        "editorGutter.commentUnresolvedGlyphForeground" | "editorGutter.commentDraftGlyphForeground" => Key("editorGutter.commentGlyphForeground"),
        "editorGhostText.border" => Transparent(&Hex("#292929"), 0.8),
        "editorInlayHint.background" => Transparent(&Key("badge.background"), 0.1),
        "editorInlayHint.typeForeground" | "editorInlayHint.parameterForeground" => Key("editorInlayHint.foreground"),
        "editorInlayHint.typeBackground" | "editorInlayHint.parameterBackground" => Key("editorInlayHint.background"),
        "editor.linkedEditingBackground" => Hex("#FFFFFF"),
        "editorLightBulbAi.foreground" => Key("editorLightBulb.foreground"),
        "textCodeBlock.background" => Hex("#F2F2F2"),
        "textLink.foreground" | "textLink.activeForeground" => Hex("#0F4A85"),
        "list.focusOutline" => Key("contrastActiveBorder"),
        "listFilterWidget.background" => Key("editorWidget.background"),
        "listFilterWidget.shadow" => Key("widget.shadow"),
        "list.filterMatchBorder" => Key("contrastActiveBorder"),
        "tree.indentGuidesStroke" => Hex("#A5A5A5"),
        "tree.inactiveIndentGuidesStroke" => Transparent(&Key("tree.indentGuidesStroke"), 0.4),
        "keybindingTable.headerBackground" | "keybindingTable.rowsBackground" => Key("tree.tableOddRowsBackground"),
        "settings.headerForeground" => Hex("#292929"),
        "settings.modifiedItemIndicator" => Hex("#66AFE0"),
        "settings.settingsHeaderHoverForeground" => Transparent(&Key("settings.headerForeground"), 0.7),
        "settings.headerBorder" | "settings.sashBorder" => Key("panel.border"),
        "modernTab.hoverBackground" => Key("list.hoverBackground"),
        "modernTab.hoverForeground" => Key("list.hoverForeground"),
        "editorSuggestWidget.background" => Key("editorWidget.background"),
        "editorSuggestWidget.border" => Key("editorWidget.border"),
        "editorSuggestWidget.foreground" => Key("editor.foreground"),
        "editorSuggestWidget.selectedForeground" => Key("editorSuggestWidget.background"),
        "editorSuggestWidget.selectedBackground" => Key("editorSuggestWidget.foreground"),
        "editorSuggestWidget.highlightForeground" => Key("list.highlightForeground"),
        "editorSuggestWidget.focusHighlightForeground" => Key("editorSuggestWidget.selectedForeground"),
        "settings.focusedRowBorder" => Key("focusBorder"),
        "settings.checkboxBackground" => Key("checkbox.background"),
        "settings.checkboxForeground" => Key("checkbox.foreground"),
        "settings.checkboxBorder" => Key("checkbox.border"),
        "settings.dropdownBackground" => Key("select.background"),
        "settings.dropdownForeground" => Key("select.foreground"),
        "settings.dropdownBorder" => Key("select.border"),
        "settings.textInputBackground" | "settings.numberInputBackground" => Key("input.background"),
        "settings.textInputForeground" | "settings.numberInputForeground" => Key("input.foreground"),
        "settings.textInputBorder" | "settings.numberInputBorder" => Key("input.border"),
        "textPreformat.foreground" => Hex("#FFFFFF"),
        "textPreformat.background" => Hex("#09345f"),
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
            ("Dark Modern", "minimap.findMatchHighlight", "#d18616cc"),
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

    // colorThemeData.test.ts has no customization cases; expected values follow setCustomColors,
    // setCustomTokenColors and isThemeScopeMatch.
    #[test]
    fn customizations_override_the_theme_and_its_scoped_blocks_match_by_id() {
        let mut theme = builtin_theme("Dark Modern");
        let colors = serde_json::json!({
            "editor.background": "#102030",
            "statusBar.background": "default",
            "[Dark*]": { "editor.background": "#405060" },
            "[Light Modern][*Modern]": { "tab.activeForeground": "#abc" },
            "[Abyss]": { "editor.background": "#000000" },
        });
        let token_colors = serde_json::json!({
            "comments": "#111111",
            "textMateRules": [{ "scope": "comment.line", "settings": { "foreground": "#222222" } }],
            "[*Dark Modern]": { "strings": { "foreground": "#333333", "fontStyle": "italic" } },
        });
        theme.set_customizations(&colors, &token_colors);
        assert_eq!(theme.color("editor.background").map(hex).as_deref(), Some("#405060ff"));
        assert_eq!(theme.color("tab.activeForeground").map(hex).as_deref(), Some("#aabbccff"));
        // "default": the registry's statusBar.background, not Dark Modern's #181818.
        assert_eq!(theme.color("statusBar.background").map(hex).as_deref(), Some("#007accff"));
        let color = |theme: &Theme, names: &[&str]| theme.tokens.color_map()[theme.tokens.style(&textmate::ScopeStack::from_names(names)).foreground].clone();
        assert_eq!(color(&theme, &["source.ts", "comment.block"]), "#111111");
        assert_eq!(color(&theme, &["source.ts", "comment.line"]), "#222222");
        assert_eq!(color(&theme, &["source.ts", "string.quoted"]), "#333333");
        // The default rule follows the customized editor colors.
        assert_eq!(theme.tokens.color_map()[theme.tokens.defaults().background], "#405060");
        theme.set_customizations(&Value::Null, &Value::Null);
        assert_eq!(theme.color("editor.background").map(hex).as_deref(), Some("#1f1f1fff"));
        assert_ne!(color(&theme, &["source.ts", "comment.block"]), "#111111");
    }

    /// `Color.toString`: `#rrggbb` when opaque, else `rgba(r, g, b, a)`.
    fn parse_css_color(text: &str) -> Option<Rgba> {
        let Some(channels) = text.strip_prefix("rgba(").and_then(|rest| rest.strip_suffix(')')) else { return parse_hex(text) };
        let values: Vec<f32> = channels.split(',').map(|value| value.trim().parse().ok()).collect::<Option<_>>()?;
        let [r, g, b, a] = values[..] else { return None };
        Some(Rgba { r: r / 255., g: g / 255., b: b / 255., a })
    }

    // port/fixtures/theme-colors: the reference workbench's --vscode-* variables per theme (`port/tools/ref-dom colors:<key>`).
    // Each color the port resolves must equal the reference's; ids the port leaves unresolved are not painted by it.
    #[test]
    fn resolved_colors_match_the_reference_workbench_variables() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let extensions = exthost::scan::scan_builtin_extensions(&root.join("extensions")).unwrap();
        let themes = [
            ("dark", "Dark Modern"), ("light", "Light Modern"), ("hc", "Default High Contrast"), ("hclight", "Default High Contrast Light"),
            ("abyss", "Abyss"), ("kimbie", "Kimbie Dark"), ("monokai", "Monokai"), ("dimmed", "Monokai Dimmed"), ("quiet", "Quiet Light"),
            ("red", "Red"), ("sdark", "Solarized Dark"), ("slight", "Solarized Light"), ("tnb", "Tomorrow Night Blue"),
        ];
        let mut mismatches = Vec::new();
        for (key, id) in themes {
            let fixture = std::fs::read_to_string(root.join(format!("port/fixtures/theme-colors/{key}.json"))).unwrap();
            let reference: serde_json::Map<String, Value> = serde_json::from_str(&fixture).unwrap();
            let theme = load_theme(&extensions, id).unwrap();
            for color_id in include_str!("webview/color_ids.txt").lines() {
                let Some(color) = theme.color(color_id) else { continue };
                let variable = format!("--vscode-{}", color_id.replace('.', "-"));
                let expected = reference.get(&variable).and_then(Value::as_str).and_then(parse_css_color);
                // rgba() prints the alpha with two decimals, so it may be one step of 255 off.
                let same = expected.is_some_and(|expected| hex(expected)[..7] == hex(color)[..7] && (expected.a - color.a).abs() <= 1.5 / 255.);
                if !same {
                    let expected = expected.map(hex);
                    mismatches.push(format!("{id} {color_id}: port {} reference {expected:?}", hex(color)));
                }
            }
        }
        assert!(mismatches.is_empty(), "{} mismatches:\n{}", mismatches.len(), mismatches.join("\n"));
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
