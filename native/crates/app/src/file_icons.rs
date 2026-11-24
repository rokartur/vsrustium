//! The file icon theme, as fileIconThemeData.ts turns it into CSS: each file gets the icon definition of the most
//! specific matching association (file name, then extensions, then language, then the default file icon).
//! Only font icon themes are drawn; the glyph outlines are read from the theme's woff font and drawn as SVG.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use anyhow::Context as _;
use exthost::scan::ScannedExtension;
use gpui::{Div, Rgba, div, prelude::*, px, svg};
use serde_json::Value;

use crate::theme::{ThemeKind, parse_hex};

/// `workbench.iconTheme` default.
pub const DEFAULT_ICON_THEME: &str = "vs-seti";

pub struct FileIconTheme {
    base: Associations,
    light: Associations,
    high_contrast: Associations,
    definitions: HashMap<String, IconDefinition>,
    /// The first font's `size`, relative to the label's font size (150% when unset).
    font_scale: f32,
}

#[derive(Default)]
struct Associations {
    file: Option<String>,
    file_extensions: HashMap<String, String>,
    file_names: HashMap<String, String>,
    language_ids: HashMap<String, String>,
}

struct IconDefinition {
    /// The glyph outline as an SVG path in font units (y up), with its units per em.
    outline: Option<(String, f32)>,
    color: Option<Rgba>,
}

impl FileIconTheme {
    pub fn load(extensions: &[ScannedExtension], id: &str) -> anyhow::Result<Self> {
        let (extension, path) = extensions
            .iter()
            .find_map(|extension| {
                let themes = extension.description["contributes"]["iconThemes"].as_array()?;
                let theme = themes.iter().find(|theme| theme["id"] == id)?;
                Some((extension, theme["path"].as_str()?))
            })
            .with_context(|| format!("no icon theme {id}"))?;
        let document_path = extension.location.join(path);
        let document: Value = serde_json::from_slice(&std::fs::read(&document_path)?)?;
        let folder = document_path.parent().expect("a theme file has a folder");
        let font = &document["fonts"][0];
        let face_data = match font["src"][0]["path"].as_str() {
            Some(font_path) => woff_to_sfnt(&std::fs::read(folder.join(font_path))?)?,
            None => Vec::new(),
        };
        let face = ttf_parser::Face::parse(&face_data, 0).ok();
        let font_scale = font["size"].as_str().and_then(|size| size.strip_suffix('%')).and_then(|size| size.parse::<f32>().ok()).unwrap_or(150.) / 100.;
        let mut definitions = HashMap::new();
        for (name, definition) in document["iconDefinitions"].as_object().into_iter().flatten() {
            let character = definition["fontCharacter"].as_str().and_then(font_character);
            let outline = character.zip(face.as_ref()).and_then(|(character, face)| glyph_outline(face, character));
            let color = definition["fontColor"].as_str().and_then(parse_hex);
            definitions.insert(name.clone(), IconDefinition { outline, color });
        }
        Ok(FileIconTheme {
            base: Associations::read(&document),
            light: Associations::read(&document["light"]),
            high_contrast: Associations::read(&document["highContrast"]),
            definitions,
            font_scale,
        })
    }

    /// The icon of a file in a label box 16px wide and `line_height` tall (iconlabel.css `::before`), the glyph
    /// sitting on the baseline of a line of `font_size` text. `None` when the theme has no glyph for the file.
    pub fn file_icon(&self, path: &Path, language_id: &str, kind: ThemeKind, font_size: f32, line_height: f32, foreground: Rgba) -> Option<Div> {
        let name = path.file_name()?.to_string_lossy().to_lowercase();
        let definition = self.definitions.get(self.definition_id(&name, language_id, kind)?)?;
        let (outline, units_per_em) = definition.outline.as_ref()?;
        let glyph_size = font_size * self.font_scale;
        // Chrome rounds the font's ascent (1em for icon fonts, no descent) to whole pixels and centers it in the line.
        let ascent = glyph_size.round();
        let baseline = ((line_height - ascent) / 2.).floor() + ascent;
        let scale = glyph_size / units_per_em;
        // The glyph may be wider than the 16px box; CSS lets it overflow.
        let width = 24.;
        let svg_text = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{line_height}" viewBox="0 0 {width} {line_height}"><path transform="translate(0 {baseline}) scale({scale} -{scale})" d="{outline}"/></svg>"#
        );
        let glyph = svg().data(svg_text.as_bytes()).absolute().top_0().left_0().w(px(width)).h(px(line_height)).text_color(definition.color.unwrap_or(foreground));
        Some(div().relative().w(px(16.)).h(px(line_height)).flex_none().child(glyph))
    }

    /// The highest CSS specificity wins; on a tie the rule written later, so the light and high contrast sections
    /// (collected after the base one) and within a section file names over extensions over languages.
    fn definition_id(&self, name: &str, language_id: &str, kind: ThemeKind) -> Option<&str> {
        let sections = match kind {
            ThemeKind::Dark => vec![(&self.base, 0)],
            ThemeKind::Light => vec![(&self.base, 0), (&self.light, 1)],
            ThemeKind::HighContrastDark | ThemeKind::HighContrastLight => vec![(&self.base, 0), (&self.high_contrast, 1)],
        };
        let mut candidates: Vec<(usize, &String)> = Vec::new();
        for (section, qualifier) in sections {
            // `.show-file-icons` (+ `.vs` / `.hc-black`) then the classes each selector lists.
            if let Some(id) = &section.file {
                candidates.push((2 + qualifier, id));
            }
            let language_id = if language_id == "jsonc" && !section.language_ids.contains_key("jsonc") { "json" } else { language_id };
            if let Some(id) = section.language_ids.get(language_id) {
                candidates.push((3 + qualifier, id));
            }
            for (dot, _) in name.match_indices('.') {
                let extension = &name[dot + 1..];
                if let Some(id) = section.file_extensions.get(extension) {
                    candidates.push((4 + extension.matches('.').count() + qualifier, id));
                }
            }
            if let Some(id) = section.file_names.get(name) {
                candidates.push((5 + name.matches('.').count() + qualifier, id));
            }
        }
        let mut best: Option<(usize, &String)> = None;
        for (specificity, id) in candidates {
            if self.definitions.contains_key(id) && best.is_none_or(|(best_specificity, _)| specificity >= best_specificity) {
                best = Some((specificity, id));
            }
        }
        best.map(|(_, id)| id.as_str())
    }
}

impl Associations {
    /// shortcut: keys with a parent folder (`dir/name`, handleParentFolder) never match; Seti has none.
    fn read(section: &Value) -> Self {
        let map = |key: &str| -> HashMap<String, String> {
            section[key].as_object().into_iter().flatten().filter_map(|(name, id)| Some((name.to_lowercase(), id.as_str()?.to_string()))).collect()
        };
        Associations {
            file: section["file"].as_str().map(str::to_string),
            file_extensions: map("fileExtensions"),
            file_names: map("fileNames"),
            language_ids: section["languageIds"].as_object().into_iter().flatten().filter_map(|(id, icon)| Some((id.clone(), icon.as_str()?.to_string()))).collect(),
        }
    }
}

/// `"\\E023"` (a CSS escape) or the character itself.
fn font_character(text: &str) -> Option<char> {
    match text.strip_prefix('\\') {
        Some(hex) => char::from_u32(u32::from_str_radix(hex, 16).ok()?),
        None => text.chars().next(),
    }
}

fn glyph_outline(face: &ttf_parser::Face, character: char) -> Option<(String, f32)> {
    struct SvgPath(String);
    impl ttf_parser::OutlineBuilder for SvgPath {
        fn move_to(&mut self, x: f32, y: f32) {
            self.0 += &format!("M{x} {y}");
        }
        fn line_to(&mut self, x: f32, y: f32) {
            self.0 += &format!("L{x} {y}");
        }
        fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
            self.0 += &format!("Q{x1} {y1} {x} {y}");
        }
        fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
            self.0 += &format!("C{x1} {y1} {x2} {y2} {x} {y}");
        }
        fn close(&mut self) {
            self.0 += "Z";
        }
    }
    let mut path = SvgPath(String::new());
    face.outline_glyph(face.glyph_index(character)?, &mut path)?;
    Some((path.0, f32::from(face.units_per_em())))
}

/// WOFF 1.0: the sfnt tables, each zlib-compressed when that made it smaller. Rebuilds the plain sfnt.
fn woff_to_sfnt(woff: &[u8]) -> anyhow::Result<Vec<u8>> {
    let bytes = |offset: usize, length: usize| woff.get(offset..offset + length).context("truncated woff");
    let u32_at = |offset: usize| Ok::<_, anyhow::Error>(u32::from_be_bytes(bytes(offset, 4)?.try_into()?));
    anyhow::ensure!(bytes(0, 4)? == b"wOFF", "not a woff 1.0 font");
    let table_count = u16::from_be_bytes(bytes(12, 2)?.try_into()?) as usize;
    let mut tables = Vec::with_capacity(table_count);
    for index in 0..table_count {
        let record = 44 + index * 20;
        let (offset, compressed, original) = (u32_at(record + 4)? as usize, u32_at(record + 8)? as usize, u32_at(record + 12)? as usize);
        let stored = bytes(offset, compressed)?;
        let data = if compressed < original {
            let mut data = Vec::with_capacity(original);
            flate2::read::ZlibDecoder::new(stored).read_to_end(&mut data)?;
            data
        } else {
            stored.to_vec()
        };
        tables.push((bytes(record, 4)?, data));
    }
    let mut sfnt = Vec::new();
    sfnt.extend_from_slice(bytes(4, 4)?);
    sfnt.extend_from_slice(&(table_count as u16).to_be_bytes());
    // searchRange, entrySelector and rangeShift: ttf-parser does not read them.
    sfnt.extend_from_slice(&[0; 6]);
    let mut offset = 12 + 16 * table_count;
    for (tag, data) in &tables {
        sfnt.extend_from_slice(tag);
        sfnt.extend_from_slice(&0u32.to_be_bytes());
        sfnt.extend_from_slice(&(offset as u32).to_be_bytes());
        sfnt.extend_from_slice(&(data.len() as u32).to_be_bytes());
        offset += data.len().next_multiple_of(4);
    }
    for (_, data) in &tables {
        sfnt.extend_from_slice(data);
        sfnt.resize(sfnt.len().next_multiple_of(4), 0);
    }
    Ok(sfnt)
}

#[cfg(test)]
mod tests {
    use exthost::scan::scan_extension;

    use super::*;

    fn seti() -> FileIconTheme {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let seti = scan_extension(&app_root.join("extensions/theme-seti"), true).unwrap();
        FileIconTheme::load(&[seti], DEFAULT_ICON_THEME).unwrap()
    }

    #[test]
    fn picks_the_definition_the_reference_css_picks() {
        let theme = seti();
        // Class lists and colors from the reference explorer (port/fixtures/workspace, Dark Modern and Light Modern).
        assert_eq!(theme.definition_id("notes.txt", "plaintext", ThemeKind::Dark), Some("_default"));
        assert_eq!(theme.definition_id("package.json", "json", ThemeKind::Dark), Some("_json"));
        assert_eq!(theme.definition_id("readme.md", "markdown", ThemeKind::Dark), Some("_info"));
        assert_eq!(theme.definition_id("readme.md", "markdown", ThemeKind::Light), Some("_info_light"));
        assert_eq!(theme.definition_id("main.ts", "typescript", ThemeKind::Dark), Some("_typescript"));
        assert_eq!(theme.definition_id("main.spec.ts", "typescript", ThemeKind::Dark), Some("_typescript_1"));
        assert_eq!(theme.definition_id("tsconfig.json", "jsonc", ThemeKind::Dark), Some("_tsconfig"));
        let color = |id: &str| theme.definitions[id].color.map(|color| [color.r, color.g, color.b].map(|c| (c * 255.).round() as u8));
        assert_eq!(color("_default"), Some([212, 215, 214]));
        assert_eq!(color("_json"), Some([203, 203, 65]));
        assert_eq!(color("_info_light"), Some([73, 139, 167]));
        assert!(theme.definitions["_default"].outline.is_some());
    }
}
