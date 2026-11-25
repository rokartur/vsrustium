use serde_json::json;

use super::*;

#[test]
fn index_maps_english_messages_to_the_pack() {
    let index = "vs/a\tinstalled\tInstalled\nvs/b\tinstalled\tInstalled\nvs/c\tquote\tSay \\\"hi\\\"\\n\n";
    let contents = json!({ "vs/b": { "installed": "Installé" }, "vs/c": { "quote": "Dis « salut »" } });
    let translations = translations_from(index, &contents);
    assert_eq!(translations["Installed"], "Installé", "vs/a has no translation, vs/b's counts");
    assert_eq!(translations["Say \"hi\"\n"], "Dis « salut »");
}

#[test]
fn the_build_index_holds_workbench_messages() {
    assert!(INDEX.lines().any(|line| line == "vs/workbench/contrib/localization/common/localizationsActions\tchooseLocale\tSelect Display Language"));
    assert!(INDEX.lines().any(|line| line == "vs/workbench/services/localization/electron-browser/localeService\trestart\t&&Restart"), "an object key with a comment");
}

#[test]
fn unescapes_typescript_literals() {
    assert_eq!(unescape(r"It\'s \u00e9t\u00e9\t\\"), "It's été\t\\");
}

#[test]
fn chinese_locales() {
    assert_eq!(process_zh_locale("zh-hans-cn"), "zh-cn");
    assert_eq!(process_zh_locale("zh-hant-tw"), "zh-tw");
    assert_eq!(process_zh_locale("fr-fr"), "fr-fr");
}

#[test]
fn argv_locale_is_written_and_cleared() {
    let file = std::env::temp_dir().join(format!("vsrustium-argv-locale-{}/argv.json", std::process::id()));
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
    let options = |_: &str| FormattingOptions { tab_size: 4, insert_spaces: false, eol: "\n".into() };
    write_argv_locale(&file, Some("fr"), options).unwrap();
    assert_eq!(user_locale(None, &file), "fr");
    assert_eq!(user_locale(Some("DE"), &file), "de", "--locale wins");
    write_argv_locale(&file, None, options).unwrap();
    assert!(!std::fs::read_to_string(&file).unwrap().contains("locale"));
    std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
}
