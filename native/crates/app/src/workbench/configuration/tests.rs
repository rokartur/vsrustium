use std::path::Path;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::TestAppContext;
use serde_json::Value;

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::{color_themes, load_theme};
use crate::workbench::Workbench;

#[gpui::test]
fn an_edit_of_settings_json_on_disk_reaches_the_host_the_theme_and_the_models(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = ["theme-defaults", "theme-abyss"].map(|name| scan_extension(&app_root.join("extensions").join(name), true).unwrap());
    let theme = load_theme(&themes, "Dark 2026").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-configuration-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    let user_settings = dir.join("User/settings.json");
    std::fs::write(&user_settings, r#"{ "editor.detectIndentation": false }"#).unwrap();
    let file = dir.join("a.txt");
    std::fs::write(&file, "a\n").unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (sender, messages) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(sender), cx);
        workbench.color_themes = color_themes(&themes).unwrap();
        workbench.open_file(file.clone(), true, window, cx);
        workbench
    });
    cx.run_until_parked();
    let tab_size = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document.indentation.tab_size);
    assert_eq!(tab_size(cx), 4);
    _ = messages.try_iter().count();

    let edited = r#"{ "editor.detectIndentation": false, "editor.tabSize": 2, "workbench.colorTheme": "Abyss" }"#;
    std::fs::write(&user_settings, edited).unwrap();
    workbench.update(cx, |workbench, cx| workbench.reload_changed_settings_files(cx));

    assert_eq!(tab_size(cx), 2);
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.theme.settings_id.clone()), "Abyss");
    let sent: Vec<(&str, Vec<Value>)> = messages.try_iter().map(|message| (message.method, message.args)).collect();
    let methods: Vec<&str> = sent.iter().map(|(method, _)| *method).collect();
    assert_eq!(methods, ["$acceptConfigurationChanged", "$onColorThemeChange", "$acceptEditorPropertiesChanged"]);
    assert_eq!(sent[0].1[1]["keys"], serde_json::json!(["editor.tabSize", "workbench.colorTheme"]));
    assert_eq!(std::fs::read_to_string(&user_settings).unwrap(), edited, "applying the theme writes nothing back");

    workbench.update(cx, |workbench, cx| workbench.reload_changed_settings_files(cx));
    assert_eq!(messages.try_iter().count(), 0, "an unchanged file is no change");

    let looks = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| (workbench.file_icons.is_some(), workbench.theme.color("editor.background")));
    let seti = scan_extension(&app_root.join("extensions/theme-seti"), true).unwrap();
    workbench.update(cx, |workbench, _| workbench.extensions = vec![seti]);
    std::fs::write(&user_settings, r##"{ "workbench.colorTheme": "Abyss", "workbench.iconTheme": "vs-seti", "workbench.colorCustomizations": { "[Abyss]": { "editor.background": "#123456" } } }"##).unwrap();
    workbench.update(cx, |workbench, cx| workbench.reload_changed_settings_files(cx));
    assert_eq!(looks(cx), (true, crate::theme::parse_hex("#123456")));
    std::fs::write(&user_settings, r#"{ "workbench.colorTheme": "Abyss", "workbench.iconTheme": null }"#).unwrap();
    workbench.update(cx, |workbench, cx| workbench.reload_changed_settings_files(cx));
    assert_eq!(looks(cx), (false, crate::theme::parse_hex("#000c18")));
    std::fs::remove_dir_all(dir).unwrap();
}
