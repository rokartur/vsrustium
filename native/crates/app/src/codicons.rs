use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// codicons.ts: the icons the workbench registers as another codicon's glyph.
const DERIVED: [(&str, &str); 18] = [
    ("dialog-error", "error"),
    ("dialog-warning", "warning"),
    ("dialog-info", "info"),
    ("dialog-close", "close"),
    ("tree-item-expanded", "chevron-down"),
    ("tree-filter-on-type-on", "list-filter"),
    ("tree-filter-on-type-off", "list-selection"),
    ("tree-filter-clear", "close"),
    ("tree-item-loading", "loading"),
    ("menu-selection", "check"),
    ("menu-submenu", "chevron-right"),
    ("menubar-more", "more"),
    ("scrollbar-button-left", "triangle-left"),
    ("scrollbar-button-right", "triangle-right"),
    ("scrollbar-button-up", "triangle-up"),
    ("scrollbar-button-down", "triangle-down"),
    ("toolbar-more", "more"),
    ("quick-input-back", "arrow-left"),
];

/// The SVG for a codicon name. codiconsLibrary.ts registers aliases (`plus`, `gist-new`) under the code point of
/// another icon, and src/icons ships one file per code point; mapping.json lists the names of each code point.
pub fn svg_path(app_root: &Path, name: &str) -> PathBuf {
    static ALIASES: OnceLock<HashMap<String, String>> = OnceLock::new();
    let icons = app_root.join("node_modules/@vscode/codicons/src/icons");
    let aliases = ALIASES.get_or_init(|| read_aliases(&app_root.join("node_modules/@vscode/codicons/src/template/mapping.json"), &icons));
    let name = DERIVED.iter().find(|(derived, _)| *derived == name).map_or(name, |(_, glyph)| glyph);
    icons.join(format!("{}.svg", aliases.get(name).map_or(name, String::as_str)))
}

fn read_aliases(mapping: &Path, icons: &Path) -> HashMap<String, String> {
    let Ok(text) = std::fs::read_to_string(mapping) else { return HashMap::new() };
    let groups: HashMap<String, Vec<String>> = serde_json::from_str(&text).expect("codicons mapping.json is a map of code points to names");
    let mut aliases = HashMap::new();
    for names in groups.into_values() {
        let Some(file) = names.iter().find(|name| icons.join(format!("{name}.svg")).exists()) else { continue };
        for name in &names {
            if name != file {
                aliases.insert(name.clone(), file.clone());
            }
        }
    }
    aliases
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_alias_resolves_to_the_file_of_its_code_point() {
        let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        assert!(super::svg_path(&app_root, "plus").ends_with("add.svg"));
        assert!(super::svg_path(&app_root, "gear").ends_with("gear.svg"));
        assert!(super::svg_path(&app_root, "toolbar-more").ends_with("ellipsis.svg"));
    }
}
