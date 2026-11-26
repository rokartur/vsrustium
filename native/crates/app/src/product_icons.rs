//! The product icon theme (productIconThemeData.ts): an extension's `productIconThemes` contribution maps icon ids
//! to glyphs of its icon fonts. GPUI's text system cannot load icon fonts (no 'm' glyph), so each glyph is read with
//! ttf-parser and drawn as an SVG path, sized and placed as the `.codicon` glyph of `font-size` and `line-height: 1`.

use std::collections::HashMap;

use anyhow::Context as _;
use exthost::scan::ScannedExtension;
use serde_json::Value;

use crate::file_icons::{font_character, glyph_outline, woff_to_sfnt};

/// `ThemeSettingDefaults.PRODUCT_ICON_THEME`: the codicons, no contributed theme.
pub const DEFAULT_PRODUCT_ICON_THEME: &str = "Default";

/// A `contributes.productIconThemes` entry: its settings id, label and description.
pub struct ProductIconThemeEntry {
    pub settings_id: String,
    pub label: String,
    pub description: Option<String>,
}

/// `contributes.productIconThemes` of every extension; an entry without an id or a path is skipped.
pub fn product_icon_themes(extensions: &[ScannedExtension]) -> Vec<ProductIconThemeEntry> {
    let mut entries = Vec::new();
    for extension in extensions {
        for theme in extension.description["contributes"]["productIconThemes"].as_array().into_iter().flatten() {
            let (Some(id), Some(path)) = (theme["id"].as_str(), theme["path"].as_str()) else { continue };
            let label = theme["label"].as_str().map_or_else(|| path.rsplit('/').next().unwrap_or(path).to_string(), str::to_string);
            entries.push(ProductIconThemeEntry { settings_id: id.to_string(), label, description: theme["description"].as_str().map(str::to_owned) });
        }
    }
    entries
}

struct Glyph {
    outline: String,
    units_per_em: f32,
    ascender: f32,
    descender: f32,
    advance: f32,
}

pub struct ProductIconTheme {
    /// The theme's id in `contributes.productIconThemes`, its `workbench.productIconTheme` value.
    pub settings_id: String,
    glyphs: HashMap<String, Glyph>,
}

impl ProductIconTheme {
    /// `_loadProductIconThemeDocument`: the fonts with a valid id and source, then each icon definition whose font
    /// (its `fontId`, else the first font) loaded and has the glyph. Invalid parts are skipped as upstream warns.
    pub fn load(extensions: &[ScannedExtension], settings_id: &str) -> anyhow::Result<Self> {
        let (extension, path) = extensions
            .iter()
            .find_map(|extension| {
                let themes = extension.description["contributes"]["productIconThemes"].as_array()?;
                let theme = themes.iter().find(|theme| theme["id"] == settings_id)?;
                Some((extension, theme["path"].as_str()?))
            })
            .with_context(|| format!("no product icon theme {settings_id}"))?;
        let document_path = extension.location.join(path);
        let document: Value = serde_json::from_str(&crate::theme::strip_jsonc(&std::fs::read_to_string(&document_path)?))?;
        let folder = document_path.parent().expect("a theme file has a folder");
        let fonts = document["fonts"].as_array().context("product icon theme without fonts")?;
        let primary_font_id = fonts.first().and_then(|font| font["id"].as_str()).context("product icon theme without fonts")?;
        let mut faces: HashMap<&str, Vec<u8>> = HashMap::new();
        for font in fonts {
            let Some(id) = font["id"].as_str().filter(|id| is_font_id(id)) else { continue };
            // The first source the font loads from, as the browser's @font-face falls back through `src`.
            let data = font["src"].as_array().into_iter().flatten().find_map(|source| {
                let data = std::fs::read(folder.join(source["path"].as_str()?)).ok()?;
                match source["format"].as_str()? {
                    "woff" => woff_to_sfnt(&data).ok(),
                    "truetype" | "opentype" => Some(data),
                    // shortcut: woff2 needs a Brotli and glyf transform decoder the port does not ship; such a source is skipped.
                    _ => None,
                }
            });
            if let Some(data) = data {
                faces.insert(id, data);
            }
        }
        let faces: HashMap<&str, ttf_parser::Face> = faces.iter().filter_map(|(id, data)| Some((*id, ttf_parser::Face::parse(data, 0).ok()?))).collect();
        let mut glyphs = HashMap::new();
        for (icon_id, definition) in document["iconDefinitions"].as_object().into_iter().flatten() {
            let Some(character) = definition["fontCharacter"].as_str().and_then(font_character) else { continue };
            let font_id = definition["fontId"].as_str().unwrap_or(primary_font_id);
            let Some(face) = faces.get(font_id) else { continue };
            let Some((outline, units_per_em)) = glyph_outline(face, character) else { continue };
            let advance = face.glyph_index(character).and_then(|glyph| face.glyph_hor_advance(glyph)).map_or(units_per_em, f32::from);
            glyphs.insert(icon_id.clone(), Glyph { outline, units_per_em, ascender: f32::from(face.ascender()), descender: f32::from(face.descender()), advance });
        }
        Ok(ProductIconTheme { settings_id: settings_id.to_string(), glyphs })
    }

    /// The SVG of icon `id` in a `size` square, as the glyph sits in a `.codicon` box of `font-size: size` and
    /// `line-height: 1`: centered on its advance, the baseline where the half-leading puts it. `None` when the theme
    /// does not define the icon.
    pub fn svg(&self, id: &str, size: f32) -> Option<String> {
        let glyph = self.glyphs.get(id)?;
        let scale = size / glyph.units_per_em;
        let x = (size - glyph.advance * scale) / 2.;
        let baseline = (size - (glyph.ascender - glyph.descender) * scale) / 2. + glyph.ascender * scale;
        Some(format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" viewBox="0 0 {size} {size}"><path transform="translate({x} {baseline}) scale({scale} -{scale})" d="{}"/></svg>"#,
            glyph.outline
        ))
    }
}

/// `restoreProductIconTheme`: the theme `workbench.productIconTheme` names, `None` for Default or a theme that is not
/// installed or does not load.
pub fn settings_product_icons(extensions: &[ScannedExtension], settings: &config::service::Settings) -> Option<ProductIconTheme> {
    let id = settings.get_value(Some("workbench.productIconTheme"), &Default::default())?;
    let id = id.as_str().filter(|id| *id != DEFAULT_PRODUCT_ICON_THEME)?;
    match ProductIconTheme::load(extensions, id) {
        Ok(theme) => Some(theme),
        Err(error) => {
            eprintln!("loading product icon theme {id}: {error:#}");
            None
        }
    }
}

/// iconRegistry.ts `fontIdRegex`.
fn is_font_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixture_theme_draws_its_glyphs_for_codicon_ids() {
        let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let fixture = exthost::scan::scan_extension(&app_root.join("port/fixtures/extensions/product-icons"), false).unwrap();
        let extensions = [fixture];
        assert_eq!(product_icon_themes(&extensions).iter().map(|theme| theme.label.as_str()).collect::<Vec<_>>(), ["Fixture Icons"]);
        let theme = ProductIconTheme::load(&extensions, "fixture-icons").unwrap();
        let files = theme.svg("files", 16.).unwrap();
        assert!(files.starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16""#));
        assert!(files.contains(r#"d="M"#));
        assert_ne!(files, theme.svg("search", 16.).unwrap());
        assert!(theme.svg("gear", 16.).is_none());
    }
}
