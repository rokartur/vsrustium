use serde_json::json;

use super::*;

/// port/fixtures/settings-import: a home folder with VS Code, Cursor and VSCodium user data.
fn fixture_home() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../port/fixtures/settings-import")
}

fn source(name: &str) -> Source {
    sources(&fixture_home()).into_iter().find(|source| source.name == name).unwrap()
}

#[test]
fn finds_the_three_editors() {
    let names: Vec<&str> = sources(&fixture_home()).iter().map(|source| source.name).collect();
    assert_eq!(names, ["VS Code", "Cursor", "VSCodium"]);
    assert!(sources(Path::new("/nonexistent")).is_empty());
}

#[test]
fn reads_vs_code_without_the_removed_features() {
    let import = read(&source("VS Code"));
    let keys: Vec<&str> = import.settings.keys().map(String::as_str).collect();
    assert_eq!(keys, ["editor.fontSize", "workbench.colorTheme", "workbench.iconTheme", "[markdown]"]);
    assert_eq!(import.settings["[markdown]"], json!({ "editor.wordWrap": "on" }));
    assert_eq!(import.keybindings, [json!({ "key": "cmd+k cmd+t", "command": "workbench.action.selectTheme" })]);
    assert_eq!(import.snippets.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["rust.json"]);
    assert_eq!(import.extensions, ["rust-lang.rust-analyzer"]);
}

#[test]
fn reads_cursor_and_vscodium() {
    let cursor = read(&source("Cursor"));
    assert_eq!(cursor.settings.keys().collect::<Vec<_>>(), ["editor.tabSize", "workbench.colorTheme"]);
    assert_eq!(cursor.extensions, ["esbenp.prettier-vscode"]);
    let vscodium = read(&source("VSCodium"));
    assert_eq!(vscodium.settings["workbench.colorTheme"], "Quiet Light");
    assert_eq!(vscodium.snippets.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["global.code-snippets"]);
    // No extensions.json: the folder names give the ids.
    assert_eq!(vscodium.extensions, ["vscodevim.vim"]);
}

#[test]
fn writes_into_the_profile_keeping_what_it_has() {
    let root = std::env::temp_dir().join(format!("app-settings-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("snippets")).unwrap();
    std::fs::write(root.join("settings.json"), "{\n\t// mine\n\t\"editor.fontSize\": 12,\n\t\"files.trimTrailingWhitespace\": true\n}").unwrap();
    std::fs::write(root.join("keybindings.json"), "[\n\t{ \"key\": \"cmd+k cmd+t\", \"command\": \"workbench.action.selectTheme\" }\n]").unwrap();
    let configuration = config::service::Settings::load(config::registry::Registry::core(), root.join("User"), None).configuration;
    let target = Target { settings: &root.join("settings.json"), keybindings: &root.join("keybindings.json"), snippets_home: &root.join("snippets") };

    let written = write(&read(&source("VS Code")), &target, &configuration).unwrap();
    assert_eq!(written, Written { settings: 4, keybindings: 0, snippets: 1 });
    let written = write(&read(&source("Cursor")), &target, &configuration).unwrap();
    assert_eq!(written, Written { settings: 2, keybindings: 1, snippets: 0 });

    let settings = std::fs::read_to_string(root.join("settings.json")).unwrap();
    assert!(settings.contains("// mine"));
    let settings = json::parse(&settings, &mut Vec::new(), parse_options()).unwrap();
    assert_eq!(settings["editor.fontSize"], 15);
    assert_eq!(settings["files.trimTrailingWhitespace"], true);
    assert_eq!(settings["workbench.colorTheme"], "Monokai");
    assert_eq!(settings["editor.tabSize"], 2);
    let keybindings = json::parse(&std::fs::read_to_string(root.join("keybindings.json")).unwrap(), &mut Vec::new(), parse_options()).unwrap();
    assert_eq!(keybindings.as_array().unwrap().len(), 2);
    assert!(root.join("snippets/rust.json").is_file());
    std::fs::remove_dir_all(&root).unwrap();
}
