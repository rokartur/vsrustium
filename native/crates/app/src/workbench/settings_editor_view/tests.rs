use serde_json::json;

use super::*;
use std::path::Path;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;

fn settings_editor_window(cx: &mut TestAppContext) -> (gpui::Entity<Workbench>, &mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-settings-editor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("User/keybindings.json"), cx);
        workbench
    });
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.openSettings2", Vec::new(), window, cx)));
    cx.run_until_parked();
    (workbench, cx)
}

#[gpui::test]
fn typed_at_opens_the_search_suggestions_and_enter_inserts_the_selected_one(cx: &mut TestAppContext) {
    let (workbench, cx) = settings_editor_window(cx);
    let search = |workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.settings_editor.search.value.clone());
    let suggest_open = |workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.settings_editor.suggest.is_some());
    cx.simulate_input("font ");
    assert!(!suggest_open(&workbench, cx));
    cx.simulate_input("@");
    assert_eq!(search(&workbench, cx), "font @");
    assert!(suggest_open(&workbench, cx));
    cx.simulate_keystrokes("down enter");
    assert_eq!(search(&workbench, cx), "font @feature:chat ");
    assert!(!suggest_open(&workbench, cx), "accepting does not reopen the widget");

    for char in ["@", "x", "y", "z"] {
        cx.simulate_input(char);
    }
    assert!(!suggest_open(&workbench, cx), "no suggestion matches the word");
    cx.simulate_keystrokes("backspace backspace backspace");
    assert!(!suggest_open(&workbench, cx), "deleting back to the trigger does not reopen it");
    cx.simulate_input(":");
    assert!(suggest_open(&workbench, cx));
    cx.simulate_keystrokes("escape");
    assert!(!suggest_open(&workbench, cx));
    assert_eq!(search(&workbench, cx), "font @feature:chat @:", "escape closes the widget before it clears the search");
}

#[test]
fn filter_toggles_add_and_remove_their_word() {
    assert_eq!(apply_filter_action("", "modifiedSettingsSearch"), "@modified");
    assert_eq!(apply_filter_action("editor", "modifiedSettingsSearch"), "editor @modified");
    assert_eq!(apply_filter_action("editor @modified", "modifiedSettingsSearch"), "editor");
    assert_eq!(apply_filter_action("editor ", "extSettingsSearch"), "editor @ext:");
}

#[test]
fn stable_preview_and_experimental_exclude_each_other() {
    assert_eq!(apply_filter_action("font @tag:preview", "stableSettingsSearch"), "font @stable");
    assert_eq!(apply_filter_action("@stable", "experimentalSettingsSearch"), "@tag:experimental");
}

#[test]
fn filter_menu_checks_the_toggles_in_the_query() {
    let checked: Vec<String> = filter_menu_actions("@modified @tag:advanced")
        .into_iter()
        .filter_map(|action| match action {
            MenuAction::Command { id, checked: true, .. } => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(checked, ["modifiedSettingsSearch", "advancedSettingsSearch"]);
}

#[test]
fn typed_values_are_validated_by_the_schema_before_they_parse() {
    let integer = json!({ "type": "integer", "minimum": 1 });
    assert_eq!(parse_typed_value(&integer, ValueType::Integer, " 12 "), Ok(json!(12)));
    assert_eq!(parse_typed_value(&integer, ValueType::Integer, "1.5"), Err("Value must be an integer.".to_string()));
    assert_eq!(parse_typed_value(&integer, ValueType::Integer, "0"), Err("Value must be greater than or equal to 1.".to_string()));
    assert_eq!(parse_typed_value(&json!({ "type": "number" }), ValueType::Number, "1.5"), Ok(json!(1.5)));
    assert_eq!(parse_typed_value(&json!({ "type": ["number", "null"] }), ValueType::NullableNumber, ""), Ok(json!(null)));
    assert_eq!(parse_typed_value(&json!({ "type": "string" }), ValueType::String, " a "), Ok(json!(" a ")));
    assert_eq!(parse_typed_value(&json!({ "type": "string", "maxLength": 1 }), ValueType::String, "ab"), Err("Value must be 1 or fewer characters long.".to_string()));
}

#[test]
fn setting_as_json_indents_by_two_spaces() {
    assert_eq!(setting_as_json("editor.fontSize", &json!(14)), "\"editor.fontSize\": 14");
    assert_eq!(setting_as_json("files.exclude", &json!({ "**/.git": true })), "\"files.exclude\": {\n  \"**/.git\": true\n}");
}

#[gpui::test]
fn dragging_the_toc_sash_resizes_the_toc_and_stores_its_width(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, point, px};
    let (workbench, cx) = settings_editor_window(cx);
    let stored = |workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext| {
        workbench.read_with(cx, |workbench, _| workbench.profile_storage.lock().unwrap().get(SPLIT_VIEW_WIDTH_KEY).map(|raw| raw.to_string()))
    };
    let toc_width = |workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.settings_editor.split.as_ref().unwrap().sizes()[0]);
    assert_eq!(toc_width(&workbench, cx), TOC_RESET_WIDTH);
    assert_eq!(stored(&workbench, cx), None);

    let sash = cx.debug_bounds("settings-sash").unwrap().center();
    cx.simulate_mouse_down(sash, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(sash + point(px(60.), px(0.)), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(sash + point(px(60.), px(0.)), MouseButton::Left, Modifiers::none());
    assert_eq!(toc_width(&workbench, cx), TOC_RESET_WIDTH + 60.);
    assert_eq!(stored(&workbench, cx).as_deref(), Some("260"));

    let sash = cx.debug_bounds("settings-sash").unwrap().center();
    cx.simulate_event(MouseDownEvent { button: MouseButton::Left, position: sash, modifiers: Modifiers::none(), click_count: 2, first_mouse: false });
    cx.simulate_event(MouseUpEvent { button: MouseButton::Left, position: sash, modifiers: Modifiers::none(), click_count: 2 });
    assert_eq!(toc_width(&workbench, cx), TOC_RESET_WIDTH, "a double click resets the TOC width");
    assert_eq!(stored(&workbench, cx).as_deref(), Some("200"));

    workbench.update_in(cx, |workbench, window, cx| {
        workbench.profile_storage.lock().unwrap().set(SPLIT_VIEW_WIDTH_KEY, "150".to_string());
        workbench.close_settings_editor(window, cx);
        drop(workbench.execute_command("workbench.action.openSettings2", Vec::new(), window, cx));
    });
    cx.run_until_parked();
    assert_eq!(toc_width(&workbench, cx), 150., "reopening starts at the stored width");
}
