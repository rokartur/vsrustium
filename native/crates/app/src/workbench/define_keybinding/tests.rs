use std::path::Path;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::TestAppContext;
use serde_json::{Value, json};

use super::{DEFINE_KEYBINDING_COMMAND, KeybindingMark, key_mark, keybinding_marks};
use crate::host_sync::HostSync;
use crate::keybindings::user_settings_fuzzy_equals;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

/// The US layout's keys for the OEM codes, as `current_raw_mapping` reports them.
fn us_mapping() -> Value {
    let keys = [
        ("Semicolon", ";", ":"),
        ("Equal", "=", "+"),
        ("Comma", ",", "<"),
        ("Minus", "-", "_"),
        ("Period", ".", ">"),
        ("Slash", "/", "?"),
        ("Backquote", "`", "~"),
        ("BracketLeft", "[", "{"),
        ("Backslash", "\\", "|"),
        ("BracketRight", "]", "}"),
        ("Quote", "'", "\""),
    ];
    let mut mapping = serde_json::Map::new();
    for (code, value, with_shift) in keys {
        mapping.insert(code.into(), json!({ "value": value, "withShift": with_shift, "withAltGr": "", "withShiftAltGr": "" }));
    }
    Value::Object(mapping)
}

#[test]
fn user_settings_fuzzy_equals_as_upstream() {
    // keybindingsEditorContribution.test.ts, issue #23335.
    assert!(user_settings_fuzzy_equals("ctrl+a", "CTRL+A"));
    assert!(user_settings_fuzzy_equals("ctrl+shift+a", "shift+ctrl+a"));
    assert!(!user_settings_fuzzy_equals("ctrl+[KeyA]", "ctrl+a"));
    assert!(user_settings_fuzzy_equals("cmd+shift+p", "shift-cmd-p"));
}

#[test]
fn keys_the_layout_cannot_type_are_errors_and_odd_spellings_info() {
    let us = us_mapping();
    assert_eq!(key_mark("cmd+k cmd+;", &us), None);
    assert_eq!(key_mark("shift+cmd+p", &us), None, "the modifier order does not matter");
    assert_eq!(
        key_mark("cmd+nokey", &us),
        Some((true, "You won't be able to produce this key combination under your current keyboard layout.".into()))
    );
    // German: no key types `;` on its own, the semicolon is Shift+Comma.
    let german = json!({ "Comma": { "value": ",", "withShift": ";", "withAltGr": "", "withShiftAltGr": "" } });
    // shortcut (key_mark): upstream marks it info as Shift+Comma; without a scan code mapper it is unmarked.
    assert_eq!(key_mark("cmd+;", &german), None);
    assert_eq!(key_mark("cmd+[", &german).map(|(error, _)| error), Some(true));
    assert_eq!(key_mark("cmd+oem_1", &us).map(|(error, _)| error), Some(false), "abnt_ and oem_ names are always info");
}

#[test]
fn each_key_string_of_the_array_is_marked_where_it_stands() {
    let lines: Vec<String> = [
        "[",
        "\t{ \"key\": \"cmd+nokey\", \"command\": \"a\" },",
        "\t{ \"key\": \"cmd+k\", \"command\": \"b\" },",
        "\t{ \"command\": \"c\", \"key\": \"cmd+oem_1\" }",
        "]",
    ]
    .map(String::from)
    .to_vec();
    let marks = keybinding_marks(&lines, "\n", &us_mapping());
    let found: Vec<_> = marks.iter().map(|KeybindingMark { line, columns, error, .. }| (*line, columns.clone(), *error)).collect();
    assert_eq!(found, vec![(1, 10..21, true), (3, 26..37, false)]);
}

#[gpui::test]
fn define_keybinding_inserts_a_rule_into_the_user_keybindings_json(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-define-keybinding-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    let file = dir.join("User/keybindings.json");
    std::fs::write(&file, "[\n]").unwrap();
    std::fs::write(dir.join("other.json"), "[\n]").unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(
            theme,
            app_root.clone(),
            None,
            Languages::new(&[]),
            settings.clone(),
            HostSync::new(std::sync::mpsc::channel().0),
            cx,
        );
        workbench.set_command_palette_menu(crate::command_palette::menu(&[]));
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), file.clone(), cx);
        workbench
    });
    let resource = crate::editor::uri_string("file", &file);
    workbench.read_with(cx, |workbench, _| {
        let default = workbench.keybindings.defaults.iter().find(|item| item.command == DEFINE_KEYBINDING_COMMAND).unwrap();
        assert_eq!(
            default.when.as_ref().unwrap().serialize(),
            format!("resource == '{resource}'"),
            "the reference's temp profile is not in the condition"
        );
        let palette = workbench.quick_access.menu.iter().find(|item| item.id == DEFINE_KEYBINDING_COMMAND).unwrap();
        assert_eq!(palette.precondition.as_ref().unwrap().serialize(), format!("resource == '{resource}'"));
    });

    workbench.update_in(cx, |workbench, window, cx| workbench.open_file(dir.join("other.json"), true, window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-k cmd-k");
    assert!(!workbench.read_with(cx, |workbench, _| workbench.define_widget_shown(true)), "another file has no Define Keybinding");

    workbench.update_in(cx, |workbench, window, cx| workbench.open_file(file.clone(), true, window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-k cmd-k");
    cx.run_until_parked();
    assert!(
        workbench.read_with(cx, |workbench, _| workbench.define_widget_shown(true)),
        "Cmd+K Cmd+K shows the define widget over keybindings.json"
    );
    cx.simulate_keystrokes("cmd-u enter");
    let text = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().lines.join("\n"));
    // SmartSnippetInserter puts it right after `[`; the snippet's tabs become the file's 4 spaces.
    assert_eq!(text, "[{\n    \"key\": \"cmd+u\",\n    \"command\": \"commandId\",\n    \"when\": \"editorTextFocus\"\n}\n]");
    assert!(!workbench.read_with(cx, |workbench, _| workbench.define_widget_shown(true)));
    let _ = std::fs::remove_dir_all(&dir);
}
