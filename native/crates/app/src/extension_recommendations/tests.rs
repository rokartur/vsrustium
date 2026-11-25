use super::*;

fn temp_folder(name: &str) -> std::path::PathBuf {
    let folder = std::env::temp_dir().join(format!("vsrustium-recommendations-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

fn write(folder: &Path, dir: &str, text: &str) {
    std::fs::create_dir_all(folder.join(dir)).unwrap();
    std::fs::write(folder.join(dir).join("extensions.json"), text).unwrap();
}

#[test]
fn vsrustium_wins_per_key_over_vscode() {
    let folder = temp_folder("per-key");
    write(&folder, ".vscode", r#"{ // comment
        "recommendations": ["a.vscode"], "unwantedRecommendations": ["b.vscode"], }"#);
    write(&folder, ".vsrustium", r#"{ "recommendations": ["a.vsrustium"] }"#);
    let config = read_folder_config(&folder);
    assert_eq!(config.recommendations, Some(vec!["a.vsrustium".into()]));
    assert_eq!(config.unwanted_recommendations, Some(vec!["b.vscode".into()]), "a key only .vscode has stays");
    std::fs::remove_dir_all(&folder).unwrap();
}

#[test]
fn missing_files_give_no_recommendations() {
    let folder = temp_folder("missing");
    assert_eq!(read_folder_config(&folder), ExtensionsConfig::default());
    std::fs::remove_dir_all(&folder).unwrap();
}

#[test]
fn workspace_file_extensions_key() {
    let folder = temp_folder("workspace-file");
    let file = folder.join("a.code-workspace");
    std::fs::write(&file, r#"{ "folders": [], "extensions": { "recommendations": ["pub.ws", 3] } }"#).unwrap();
    assert_eq!(read_workspace_file_config(&file).unwrap().recommendations, Some(vec!["pub.ws".into()]), "non string entries are dropped");
    std::fs::remove_dir_all(&folder).unwrap();
}

#[test]
fn bad_ids_are_dropped_with_a_warning_and_unwanted_ones_filtered() {
    let configs = [
        ExtensionsConfig { recommendations: Some(vec!["Pub.One".into(), "bad id".into(), "pub.two".into()]), unwanted_recommendations: Some(vec!["pub.two".into()]) },
        ExtensionsConfig { recommendations: Some(vec!["pub.one".into(), "pub.three".into()]), unwanted_recommendations: None },
    ];
    let workspace = workspace_recommendations(&configs);
    assert_eq!(workspace.warning.as_deref(), Some("The 1 extension(s) below, in workspace recommendations have issues:\nbad id (bad format) Expected: <provider>.<name>\n"));
    assert_eq!(allowed_recommendations(&workspace, &[]), ["pub.one", "pub.three"]);
    assert_eq!(allowed_recommendations(&workspace, &parse_ignored(Some(r#"["PUB.THREE"]"#))), ["pub.one"]);
}

#[test]
fn prompt_names_one_extension_or_its_publishers() {
    let one = |name: &str, publisher: &str| (name.to_owned(), publisher.to_owned());
    assert_eq!(prompt_message(&[one("Rust", "rust-lang")]), "Do you want to install the recommended 'Rust' extension from rust-lang for this repository?");
    assert_eq!(prompt_message(&[one("A", "p"), one("B", "p")]), "Do you want to install the recommended extensions from p for this repository?");
    assert_eq!(prompt_message(&[one("A", "p"), one("B", "q")]), "Do you want to install the recommended extensions from p and q for this repository?");
    assert_eq!(prompt_message(&[one("A", "p"), one("B", "q"), one("C", "r")]), "Do you want to install the recommended extensions from p, q and others for this repository?");
}
