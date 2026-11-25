use serde_json::json;

use super::*;

#[test]
fn installed_languages_add_english_and_sort_by_label() {
    let packs = json!({ "fr": { "label": "français", "extensions": [{ "extensionIdentifier": { "id": "MS-CEINTL.vscode-language-pack-fr" } }] } });
    let languages = installed_languages(packs.as_object().unwrap());
    let labels: Vec<&str> = languages.iter().map(|language| language.label.as_str()).collect();
    assert_eq!(labels, ["English", "français"]);
    assert_eq!(languages[1].extension_id.as_deref(), Some("MS-CEINTL.vscode-language-pack-fr"));
}

#[test]
fn quick_pick_items_describe_locale_current_and_installs() {
    let item = LanguageItem { locale: "fr".into(), label: "français".into(), extension_id: Some("p.fr".into()), gallery: None };
    let entries = language_entries(&[english(), item], 3, "Installed");
    let Entry::Separator(Some(separator)) = &entries[0] else { panic!("a separator first") };
    assert_eq!(separator, "Installed");
    let Entry::Item(english) = &entries[1] else { panic!() };
    assert_eq!((english.handle, english.description.as_deref(), english.buttons.len()), (3, Some("(en) (Current)"), 0));
    let Entry::Item(french) = &entries[2] else { panic!() };
    assert_eq!((french.handle, french.description.as_deref(), french.buttons.len()), (4, Some("(fr)"), 1));
    assert_eq!(install_count_label(1_234_567.), "1.2M");
    assert_eq!(install_count_label(34_999.), "34K");
}
