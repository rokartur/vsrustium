use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The SVG for a codicon name. codiconsLibrary.ts registers aliases (`plus`, `gist-new`) under the code point of
/// another icon, and src/icons ships one file per code point; mapping.json lists the names of each code point.
pub fn svg_path(app_root: &Path, name: &str) -> PathBuf {
    static ALIASES: OnceLock<HashMap<String, String>> = OnceLock::new();
    let icons = app_root.join("node_modules/@vscode/codicons/src/icons");
    let aliases = ALIASES.get_or_init(|| read_aliases(&app_root.join("node_modules/@vscode/codicons/src/template/mapping.json"), &icons));
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
    }
}
