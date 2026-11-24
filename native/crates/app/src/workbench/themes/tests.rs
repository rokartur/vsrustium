use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::TestAppContext;
use serde_json::{Value, json};

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::{color_themes, load_theme};
use crate::workbench::Workbench;

fn rows(workbench: &Workbench, _: &gpui::App) -> Vec<String> {
    let input = workbench.quick_inputs.current().expect("a visible quick input");
    let mut rows = Vec::new();
    for row in input.order.iter().map(|row| &input.rows[*row]) {
        let separator = row.separator.as_ref().map(|label| format!("--{label}--")).unwrap_or_default();
        let description = row.item.description.as_ref().map(|description| format!(" ({description})")).unwrap_or_default();
        rows.push(format!("{separator}{}{description}", row.item.label));
    }
    rows
}

#[gpui::test]
fn the_picker_previews_the_focused_theme_writes_the_accepted_one_and_escape_restores(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let mut extensions = Vec::new();
    for entry in std::fs::read_dir(app_root.join("extensions")).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap().to_string_lossy().starts_with("theme-") {
            extensions.push(scan_extension(&path, true).unwrap());
        }
    }
    let theme = load_theme(&extensions, "Dark 2026").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-themes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (sender, messages) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings.clone(), HostSync::new(sender), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("User/keybindings.json"), cx);
        workbench.color_themes = color_themes(&extensions).unwrap();
        workbench
    });
    cx.run_until_parked();
    let theme_id = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.theme.settings_id.clone());
    let sent = || messages.try_iter().filter(|message| message.method != "$activateByEvent").map(|message| (message.method, message.args)).collect::<Vec<(&str, Vec<Value>)>>();
    let user_settings = || std::fs::read_to_string(dir.join("User/settings.json")).ok();

    cx.simulate_keystrokes("cmd-k cmd-t");
    assert_eq!(
        workbench.read_with(cx, rows),
        [
            "--light themes--Light 2026 (Default Light)",
            "Light (Visual Studio) (Visual Studio Light)",
            "Light Modern",
            "Light+",
            "Quiet Light",
            "Solarized Light",
            "--dark themes--Dark 2026 (Default Dark)",
            "Abyss",
            "Dark (Visual Studio) (Visual Studio Dark)",
            "Dark Modern",
            "Dark+",
            "Kimbie Dark",
            "Monokai",
            "Monokai Dimmed",
            "Red",
            "Solarized Dark",
            "Tomorrow Night Blue",
            "--high contrast themes--Dark High Contrast (Default High Contrast)",
            "Light High Contrast (Default High Contrast Light)",
        ]
    );
    let (placeholder, active) = workbench.read_with(cx, |workbench, _| {
        let input = workbench.quick_inputs.current().unwrap();
        (input.placeholder.clone(), input.active_handles())
    });
    assert_eq!(placeholder.as_deref(), Some("Select Color Theme (detect system color mode disabled)"));
    assert_eq!(active, [6], "the current theme is focused");

    cx.simulate_keystrokes("down");
    cx.executor().advance_clock(Duration::from_millis(199));
    cx.run_until_parked();
    assert_eq!(theme_id(cx), "Dark 2026", "the preview waits 200ms");
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(theme_id(cx), "Abyss");
    assert_eq!(sent(), [("$onColorThemeChange", vec![json!("dark")])]);
    assert_eq!(user_settings(), None, "a preview writes nothing");

    cx.simulate_keystrokes("escape");
    assert_eq!(theme_id(cx), "Dark 2026");
    assert_eq!(sent(), [("$onColorThemeChange", vec![json!("dark")])]);
    assert_eq!(user_settings(), None, "restoring the current theme writes nothing");

    cx.simulate_keystrokes("cmd-k cmd-t");
    cx.simulate_input("quiet");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(theme_id(cx), "Quiet Light");
    assert!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().is_none()));
    let written: Value = serde_json::from_str(&user_settings().unwrap()).unwrap();
    assert_eq!(written, json!({ "workbench.colorTheme": "Quiet Light" }));
    let methods: Vec<&str> = sent().into_iter().map(|(method, _)| method).collect();
    assert_eq!(methods, ["$onColorThemeChange", "$acceptConfigurationChanged"]);

    // Picking the default again removes the user value (`updateValue` without a target).
    cx.simulate_keystrokes("cmd-k cmd-t");
    cx.simulate_input("dark 2026");
    cx.simulate_keystrokes("enter");
    assert_eq!(theme_id(cx), "Dark 2026");
    assert_eq!(serde_json::from_str::<Value>(&user_settings().unwrap()).unwrap(), json!({}));
}
