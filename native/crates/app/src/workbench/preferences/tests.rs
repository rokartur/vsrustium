use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::editor::Position;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::{color_themes, load_theme};
use crate::workbench::Workbench;

fn workbench<'a>(cx: &'a mut TestAppContext, name: &str) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext, PathBuf) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let scan = |name: &str| scan_extension(&app_root.join("extensions").join(name), true).unwrap();
    let themes = [scan("theme-defaults")];
    let theme = load_theme(&themes, "Dark 2026").unwrap();
    let languages = Languages::new(&[scan("json")]);
    let dir = std::env::temp_dir().join(format!("code-gpui-preferences-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::core(), dir.join("User"), None)));
    let (sender, _messages) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, languages, settings, HostSync::new(sender), cx);
        workbench.color_themes = color_themes(&themes).unwrap();
        workbench
    });
    (workbench, cx, dir)
}

fn run(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, command: &str, args: Vec<Value>) {
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command(command, args, window, cx));
    cx.run_until_parked();
}

fn active_text_and_cursor(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> (String, Position) {
    workbench.read_with(cx, |workbench, _| {
        let editor = workbench.editors.active().unwrap();
        (editor.document.lines.join("\n"), editor.selections[0].head)
    })
}

#[gpui::test]
fn open_settings_json_creates_the_file_and_reveals_a_language_block_for_editing(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "edit");
    let settings_file = dir.join("User/settings.json");

    run(&workbench, cx, "workbench.action.openSettingsJson", vec![json!({ "revealSetting": { "key": "[json]", "edit": true } })]);

    assert_eq!(std::fs::read_to_string(&settings_file).unwrap(), "{\n}", "emptyEditableSettingsContent on disk");
    let (text, cursor) = active_text_and_cursor(&workbench, cx);
    assert_eq!(text, "{\n    \"[json]\": {\n        \n    }\n}");
    assert_eq!(cursor, Position::new(2, 8));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn a_setting_query_reveals_its_key_or_its_value(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "query");
    std::fs::create_dir_all(dir.join("User")).unwrap();
    std::fs::write(dir.join("User/settings.json"), "{\n\t\"editor.tabSize\": 2,\n}").unwrap();

    run(&workbench, cx, "workbench.action.openSettingsJson", vec![json!({ "query": "@id:editor.tabSize" })]);
    assert_eq!(active_text_and_cursor(&workbench, cx).1, Position::new(1, 1));

    run(&workbench, cx, "workbench.action.openSettingsJson", vec![json!({ "revealSetting": { "key": "editor.tabSize", "edit": true } })]);
    assert_eq!(active_text_and_cursor(&workbench, cx), ("{\n\t\"editor.tabSize\": 2,\n}".to_string(), Position::new(1, 20)));

    run(&workbench, cx, "workbench.action.openSettingsJson", vec![json!({ "revealSetting": { "key": "editor.unknownSetting", "edit": true } })]);
    assert_eq!(active_text_and_cursor(&workbench, cx).0, "{\n\t\"editor.tabSize\": 2,\n}", "an unregistered key is not added");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn the_raw_default_settings_open_read_only(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "raw");

    run(&workbench, cx, "workbench.action.openRawDefaultSettings", vec![]);

    let (uri, first_line, language, readonly) = workbench.read_with(cx, |workbench, _| {
        let editor = workbench.editors.active().unwrap();
        let document = &editor.document;
        (document.uri_string(), document.lines[0].clone(), document.language_id.clone(), workbench.active_editor_readonly())
    });
    assert_eq!((uri.as_str(), first_line.as_str(), language.as_str()), ("vscode://defaultsettings/defaultSettings.jsonc", "{", "jsonc"));
    assert_eq!(readonly, Some(super::super::readonly::Readonly::Provider));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn the_default_keybindings_file_lists_the_defaults_then_the_unbound_commands(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "keybindings");
    workbench.update(cx, |workbench, cx| workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("User/keybindings.json"), cx));

    run(&workbench, cx, "workbench.action.openDefaultKeybindingsFile", vec![]);

    let (name, lines) = workbench.read_with(cx, |workbench, _| {
        let editor = workbench.editors.active().unwrap();
        (workbench.editor_name(editor), editor.document.lines.clone())
    });
    assert_eq!(name, "Default Keybindings");
    assert_eq!(lines[..2], ["// Override key bindings by placing them into your key bindings file.", "["]);
    assert!(lines.contains(&"{ \"key\": \"cmd+,\",                 \"command\": \"workbench.action.openSettings\" },".to_string()));
    let when = lines.iter().position(|line| line.trim_start().starts_with("\"when\": ")).unwrap();
    assert!(lines[when].starts_with("                                     \"when\": \"") && lines[when - 1].ends_with(","));
    let unbound = lines.iter().position(|line| line == "// Here are other available commands: ").unwrap();
    assert_eq!(lines[unbound - 3..unbound], ["]", "", ""]);
    assert!(lines[unbound + 1..].iter().all(|line| line.starts_with("// - ")));
    assert!(lines.contains(&"// - workbench.action.openRawDefaultSettings".to_string()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn workspace_settings_need_a_folder(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "workspace");
    run(&workbench, cx, "workbench.action.openWorkspaceSettingsFile", vec![]);
    assert!(workbench.read_with(cx, |workbench, _| workbench.editors.active().is_none()));
    std::fs::remove_dir_all(dir).unwrap();
}
