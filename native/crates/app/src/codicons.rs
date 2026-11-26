use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};

/// The SVG for a codicon name. codiconsLibrary.ts registers aliases (`plus`, `gist-new`) under the code point of
/// another icon, and src/icons ships one file per code point; mapping.json lists the names of each code point.
pub fn svg_path(app_root: &Path, name: &str) -> PathBuf {
    static ALIASES: OnceLock<HashMap<String, String>> = OnceLock::new();
    let icons = app_root.join("node_modules/@vscode/codicons/src/icons");
    let aliases = ALIASES.get_or_init(|| read_aliases(&app_root.join("node_modules/@vscode/codicons/src/template/mapping.json"), &icons));
    icons.join(format!("{}.svg", aliases.get(name).map_or(name, String::as_str)))
}

/// iconRegistry.ts `_resolveIconDefinition`'s walk: `id`, then each registered default after it, ending at a codicon
/// with a font character. The defaults come from port/tools/gen-icon-registry.ts.
pub fn icon_ids(id: &str) -> Vec<&str> {
    static DEFAULTS: LazyLock<HashMap<String, String>> = LazyLock::new(|| serde_json::from_str(include_str!("icon_registry.json")).expect("icon_registry.json is a map of ids"));
    let mut ids = vec![id];
    while let Some(default) = DEFAULTS.get(*ids.last().expect("starts with id")) {
        ids.push(default);
    }
    ids
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
    // codicons.ts registers 'git-fetch' again at 0xec1d, repo-fetch's glyph, over codiconsLibrary.ts's 0xecb2.
    aliases.insert("git-fetch".into(), "repo-fetch".into());
    aliases
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_alias_resolves_to_the_file_of_its_code_point() {
        let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        assert!(super::svg_path(&app_root, "plus").ends_with("add.svg"));
        assert!(super::svg_path(&app_root, "gear").ends_with("gear.svg"));
        assert!(super::svg_path(&app_root, "git-fetch").ends_with("repo-fetch.svg"));
    }

    #[test]
    fn a_registered_icon_resolves_through_its_defaults() {
        assert_eq!(super::icon_ids("explorer-view-icon"), ["explorer-view-icon", "files"]);
        assert_eq!(super::icon_ids("folding-manual-collapsed"), ["folding-manual-collapsed", "folding-collapsed", "chevron-right"]);
        assert_eq!(super::icon_ids("gear"), ["gear"]);
    }
}
