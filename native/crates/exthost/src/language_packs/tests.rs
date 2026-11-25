use super::*;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vsrustium-language-packs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn language_pack(dir: &Path, language: &str) -> ScannedExtension {
    let location = dir.join(format!("ms-ceintl.vscode-language-pack-{language}"));
    std::fs::create_dir_all(location.join("translations/extensions")).unwrap();
    std::fs::write(location.join("translations/main.i18n.json"), r#"{"contents":{"vs/a":{"k":"Bonjour"}}}"#).unwrap();
    std::fs::write(location.join("translations/extensions/vscode.git.i18n.json"), r#"{"contents":{"package":{"displayName":"Git (fr)"}}}"#).unwrap();
    let description = json!({
        "version": "1.0.0",
        "contributes": { "localizations": [{
            "languageId": language, "languageName": "French", "localizedLanguageName": "français",
            "translations": [{ "id": "vscode", "path": "./translations/main.i18n.json" }, { "id": "vscode.git", "path": "./translations/extensions/vscode.git.i18n.json" }],
        }] },
    });
    ScannedExtension { id: format!("MS-CEINTL.vscode-language-pack-{language}"), location, description }
}

#[test]
fn packs_from_localizations_resolve_the_locale_and_its_prefix() {
    let dir = temp_dir("resolve");
    let packs = update_language_packs(&dir, &[language_pack(&dir, "fr")]).unwrap();
    assert_eq!(packs["fr"]["label"], "français");
    assert_eq!(packs["fr"]["extensions"][0]["extensionIdentifier"]["id"], "MS-CEINTL.vscode-language-pack-fr");
    assert_eq!(packs["fr"]["hash"].as_str().unwrap().len(), 32);
    assert_eq!(read_language_packs(&dir), packs, "written to languagepacks.json");
    assert_eq!(resolve_nls(&dir, "fr-ca").language, "fr", "a region falls back to its language");
    assert_eq!(resolve_nls(&dir, "de").language, "en", "no pack for the locale");
    assert_eq!(resolve_nls(&dir, "en-gb").language, "en");
    assert_eq!(built_in_bundle_path(&dir, "vscode.git", "fr").unwrap(), dir.join("ms-ceintl.vscode-language-pack-fr/./translations/extensions/vscode.git.i18n.json"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn invalid_localizations_are_skipped() {
    let mut extension = language_pack(&temp_dir("invalid"), "fr");
    extension.description["contributes"]["localizations"][0]["translations"] = json!([]);
    assert!(language_packs_from(&[extension]).is_empty());
}

#[test]
fn manifest_messages_from_the_pack_or_the_locale_file() {
    let dir = temp_dir("messages");
    let pack = language_pack(&dir, "fr");
    let packs = language_packs_from(&[pack]);
    let nls = Nls { user_locale: "fr".into(), language: "fr".into(), translations: packs["fr"]["translations"].as_object().unwrap().clone() };
    let git = dir.join("git");
    std::fs::create_dir_all(&git).unwrap();
    std::fs::write(git.join("package.nls.json"), r#"{"displayName":"Git","description":"Git SCM"}"#).unwrap();
    let messages = localized_messages(&git, "vscode.git", &nls).unwrap();
    assert_eq!((messages["displayName"].as_str(), messages["description"].as_str()), (Some("Git (fr)"), Some("Git SCM")));

    let user = dir.join("user");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("package.nls.json"), r#"{"title":"Run"}"#).unwrap();
    std::fs::write(user.join("package.nls.zh-cn.json"), r#"{"title":"运行"}"#).unwrap();
    let chinese = Nls { user_locale: "zh-cn".into(), language: "zh-cn".into(), translations: Map::new() };
    assert_eq!(localized_messages(&user, "pub.user", &chinese).unwrap()["title"], "运行");
    assert_eq!(localized_messages(&user, "pub.user", &Nls::english("en")).unwrap()["title"], "Run");
    std::fs::remove_dir_all(&dir).unwrap();
}
