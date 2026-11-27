//! The file icon theme, as fileIconThemeData.ts turns it into CSS: each file gets the icon definition of the most
//! specific matching association (file name, then extensions, then language, then the default file icon).
//! Font glyph outlines are read from the theme's woff font and drawn as SVG; image icons are drawn from their file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use exthost::scan::ScannedExtension;
use gpui::{AnyElement, Div, Rgba, div, img, prelude::*, px, svg};
use serde_json::Value;

use crate::theme::{ThemeKind, parse_hex};

/// `workbench.iconTheme` default.
pub const DEFAULT_ICON_THEME: &str = "vs-seti";

/// A `contributes.iconThemes` entry (`FileIconThemeData.fromExtensionTheme`): its id, label and description.
pub struct FileIconThemeEntry {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

/// `contributes.iconThemes` of every extension; an entry without an id or a path is skipped.
pub fn file_icon_themes(extensions: &[ScannedExtension]) -> Vec<FileIconThemeEntry> {
    let mut entries = Vec::new();
    for extension in extensions {
        for theme in extension.description["contributes"]["iconThemes"].as_array().into_iter().flatten() {
            let (Some(id), Some(_)) = (theme["id"].as_str(), theme["path"].as_str()) else { continue };
            let label = theme["label"].as_str().unwrap_or(id).to_string();
            entries.push(FileIconThemeEntry { id: id.to_string(), label, description: theme["description"].as_str().map(str::to_owned) });
        }
    }
    entries
}

pub struct FileIconTheme {
    /// The theme's id, its `workbench.iconTheme` value.
    pub id: String,
    base: Associations,
    light: Associations,
    high_contrast: Associations,
    definitions: HashMap<String, IconDefinition>,
    /// The first font's `size`, relative to the label's font size (150% when unset).
    font_scale: f32,
    /// `usesCurrentColor`: image icons are masks filled with the label color.
    uses_current_color: bool,
}

#[derive(Default)]
struct Associations {
    file: Option<String>,
    folder: Option<String>,
    folder_expanded: Option<String>,
    root_folder: Option<String>,
    root_folder_expanded: Option<String>,
    folder_names: HashMap<String, String>,
    folder_names_expanded: HashMap<String, String>,
    root_folder_names: HashMap<String, String>,
    root_folder_names_expanded: HashMap<String, String>,
    file_extensions: HashMap<String, String>,
    file_names: HashMap<String, String>,
    language_ids: HashMap<String, String>,
}

struct IconDefinition {
    /// The glyph outline as an SVG path in font units (y up), with its units per em.
    outline: Option<(String, f32)>,
    /// `iconPath`, resolved against the theme file's folder.
    image: Option<PathBuf>,
    color: Option<Rgba>,
}

/// The folder a row shows, as getIconClasses.ts names it.
pub struct FolderIcon<'a> {
    pub name: &'a str,
    pub is_root: bool,
    pub expanded: bool,
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
        let document: Value = serde_json::from_str(&crate::theme::strip_jsonc(&std::fs::read_to_string(&document_path)?))?;
        let folder = document_path.parent().expect("a theme file has a folder");
        let font = &document["fonts"][0];
        let face_data = match font["src"][0]["path"].as_str() {
            Some(font_path) => wuff::decompress_woff1(&std::fs::read(folder.join(font_path))?)?,
            None => Vec::new(),
        };
        let face = ttf_parser::Face::parse(&face_data, 0).ok();
        let font_scale = font["size"].as_str().and_then(|size| size.strip_suffix('%')).and_then(|size| size.parse::<f32>().ok()).unwrap_or(150.) / 100.;
        let mut definitions = HashMap::new();
        for (name, definition) in document["iconDefinitions"].as_object().into_iter().flatten() {
            let character = definition["fontCharacter"].as_str().and_then(font_character);
            let outline = character.zip(face.as_ref()).and_then(|(character, face)| glyph_outline(face, character));
            let image = definition["iconPath"].as_str().map(|image| folder.join(image));
            let color = definition["fontColor"].as_str().and_then(parse_hex);
            definitions.insert(name.clone(), IconDefinition { outline, image, color });
        }
        Ok(FileIconTheme {
            id: id.to_string(),
            base: Associations::read(&document),
            light: Associations::read(&document["light"]),
            high_contrast: Associations::read(&document["highContrast"]),
            definitions,
            font_scale,
            uses_current_color: document["usesCurrentColor"] == true,
        })
    }

    /// The icon of a file in a label box 16px wide and `line_height` tall (iconlabel.css `::before`), the glyph
    /// sitting on the baseline of a line of `font_size` text. `None` when the theme has no glyph for the file.
    pub fn file_icon(&self, path: &Path, language_id: &str, kind: ThemeKind, font_size: f32, line_height: f32, foreground: Rgba) -> Option<Div> {
        let name = path.file_name()?.to_string_lossy().to_lowercase();
        self.icon(self.definition_id(&name, language_id, kind)?, font_size, line_height, foreground)
    }

    /// fileIconThemeData.ts `hasFolderIcons`: the explorer then draws an icon after each folder's twistie and
    /// stops aligning file icons with twisties.
    pub fn has_folder_icons(&self) -> bool {
        [&self.base, &self.light, &self.high_contrast].into_iter().any(Associations::has_folder_icons)
    }

    pub fn folder_icon(&self, folder: FolderIcon, kind: ThemeKind, font_size: f32, line_height: f32, foreground: Rgba) -> Option<Div> {
        self.icon(self.folder_definition_id(folder, kind)?, font_size, line_height, foreground)
    }

    /// `getIconClassesForLanguageId`: the icon of the language alone, no file name or extension.
    pub fn language_icon(&self, language_id: &str, kind: ThemeKind, font_size: f32, line_height: f32, foreground: Rgba) -> Option<Div> {
        self.icon(self.definition_id("", language_id, kind)?, font_size, line_height, foreground)
    }

    fn icon(&self, id: &str, font_size: f32, line_height: f32, foreground: Rgba) -> Option<Div> {
        let definition = &self.definitions[id];
        if let Some(image) = &definition.image {
            return Some(div().relative().w(px(16.)).h(px(line_height)).flex_none().child(self.image_icon(image, line_height, foreground)));
        }
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

    /// iconlabel.css `::before`: a 16px image at the left, centered in the line.
    fn image_icon(&self, image: &Path, line_height: f32, foreground: Rgba) -> AnyElement {
        let top = px((line_height - 16.) / 2.);
        match self.uses_current_color {
            true => svg().external_path(image.to_string_lossy().into_owned()).absolute().top(top).left_0().size(px(16.)).text_color(foreground).into_any_element(),
            false => img(image.to_path_buf()).absolute().top(top).left_0().size(px(16.)).into_any_element(),
        }
    }

    /// Specificity as in `definition_id`; `expanded` is `.monaco-tl-twistie.collapsible:not(.collapsed) + .monaco-tl-contents`.
    fn folder_definition_id(&self, folder: FolderIcon, kind: ThemeKind) -> Option<&str> {
        let name = folder.name.to_lowercase();
        let mut candidates: Vec<(usize, &String)> = Vec::new();
        for (section, qualifier) in self.sections(kind) {
            let (plain, expanded, plain_named, expanded_named) = match folder.is_root {
                true => (
                    section.root_folder.as_ref().or(section.folder.as_ref()),
                    section.root_folder_expanded.as_ref().or(section.folder_expanded.as_ref()),
                    section.root_folder_names.get(&name),
                    section.root_folder_names_expanded.get(&name),
                ),
                false => (section.folder.as_ref(), section.folder_expanded.as_ref(), section.folder_names.get(&name), section.folder_names_expanded.get(&name)),
            };
            candidates.extend(plain.map(|id| (2 + qualifier, id)));
            candidates.extend(plain_named.map(|id| (3 + qualifier, id)));
            if folder.expanded {
                candidates.extend(expanded.map(|id| (6 + qualifier, id)));
                candidates.extend(expanded_named.map(|id| (7 + qualifier, id)));
            }
        }
        self.most_specific(candidates)
    }

    /// The highest CSS specificity wins; on a tie the rule written later, so the light and high contrast sections
    /// (collected after the base one) and within a section file names over extensions over languages.
    fn definition_id(&self, name: &str, language_id: &str, kind: ThemeKind) -> Option<&str> {
        let mut candidates: Vec<(usize, &String)> = Vec::new();
        for (section, qualifier) in self.sections(kind) {
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
        self.most_specific(candidates)
    }

    fn sections(&self, kind: ThemeKind) -> Vec<(&Associations, usize)> {
        match kind {
            ThemeKind::Dark => vec![(&self.base, 0)],
            ThemeKind::Light => vec![(&self.base, 0), (&self.light, 1)],
            ThemeKind::HighContrastDark | ThemeKind::HighContrastLight => vec![(&self.base, 0), (&self.high_contrast, 1)],
        }
    }

    fn most_specific<'a>(&self, candidates: Vec<(usize, &'a String)>) -> Option<&'a str> {
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
    fn has_folder_icons(&self) -> bool {
        let names = [&self.folder_names, &self.folder_names_expanded, &self.root_folder_names, &self.root_folder_names_expanded];
        [&self.folder, &self.folder_expanded, &self.root_folder, &self.root_folder_expanded].iter().any(|id| id.is_some()) || names.iter().any(|names| !names.is_empty())
    }

    /// shortcut: keys with a parent folder (`dir/name`, handleParentFolder) never match; Seti has none.
    fn read(section: &Value) -> Self {
        let map = |key: &str| -> HashMap<String, String> {
            section[key].as_object().into_iter().flatten().filter_map(|(name, id)| Some((name.to_lowercase(), id.as_str()?.to_string()))).collect()
        };
        Associations {
            file: section["file"].as_str().map(str::to_string),
            folder: section["folder"].as_str().map(str::to_string),
            folder_expanded: section["folderExpanded"].as_str().map(str::to_string),
            root_folder: section["rootFolder"].as_str().map(str::to_string),
            root_folder_expanded: section["rootFolderExpanded"].as_str().map(str::to_string),
            folder_names: map("folderNames"),
            folder_names_expanded: map("folderNamesExpanded"),
            root_folder_names: map("rootFolderNames"),
            root_folder_names_expanded: map("rootFolderNamesExpanded"),
            file_extensions: map("fileExtensions"),
            file_names: map("fileNames"),
            language_ids: section["languageIds"].as_object().into_iter().flatten().filter_map(|(id, icon)| Some((id.clone(), icon.as_str()?.to_string()))).collect(),
        }
    }
}

/// `"\\E023"` (a CSS escape) or the character itself.
pub(crate) fn font_character(text: &str) -> Option<char> {
    match text.strip_prefix('\\') {
        Some(hex) => char::from_u32(u32::from_str_radix(hex, 16).ok()?),
        None => text.chars().next(),
    }
}

pub(crate) fn glyph_outline(face: &ttf_parser::Face, character: char) -> Option<(String, f32)> {
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

#[cfg(test)]
mod tests {
    use exthost::scan::scan_extension;

    use super::*;

    fn seti() -> FileIconTheme {
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
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
        assert!(!theme.has_folder_icons());
    }

    #[test]
    fn image_themes_give_folders_their_icons() {
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
        let modern = scan_extension(&app_root.join("extensions/theme-modern-icons"), true).unwrap();
        let theme = FileIconTheme::load(&[modern], "vscode-modern-icons").unwrap();
        assert!(theme.has_folder_icons() && theme.uses_current_color);
        let folder = |name, is_root, expanded| theme.folder_definition_id(FolderIcon { name, is_root, expanded }, ThemeKind::Dark);
        assert_eq!(folder("src", false, false), Some("_folder"));
        assert_eq!(folder("src", false, true), Some("_folderOpen"));
        // rootFolder falls back to folder (fileIconThemeData.ts `associations.rootFolder || associations.folder`).
        assert_eq!(folder("workspace", true, true), Some("_folderOpen"));
        assert_eq!(theme.definition_id("main.ts", "typescript", ThemeKind::Dark), Some("_typescript"));
        assert!(theme.definitions["_folder"].image.as_ref().is_some_and(|image| image.ends_with("images/folder.svg") && image.exists()));
    }
}
