use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use exthost::scan::{ScannedExtension, scan_extension};
use gpui::TestAppContext;
use serde_json::json;

use crate::command_palette;
use crate::commands::Outcome;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::quick_input::Entry;
use crate::theme::load_theme;
use crate::workbench::Workbench;
use storage::Storage;

fn labels(workbench: &Workbench, _: &gpui::App) -> Vec<String> {
    let input = workbench.quick_inputs.current().expect("a visible quick input");
    input.order.iter().map(|row| &input.rows[*row]).map(|row| format!("{}{}", row.separator.as_ref().map(|label| format!("--{label}--")).unwrap_or_default(), row.item.label)).collect()
}

#[gpui::test]
fn command_palette_runs_an_extension_command_and_remembers_it(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-palette-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let extension = ScannedExtension {
        id: "port.hello".into(),
        location: dir.clone(),
        description: json!({ "contributes": { "commands": [{ "command": "port.hello", "title": "Say Hello", "category": "Port" }] } }),
    };
    let runs = Rc::new(Cell::new(0));
    let storage = Arc::new(Mutex::new(Storage::in_memory()));
    storage.lock().unwrap().set("commandPalette.mru.counter", "41".into());
    let profile = storage.clone();
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
        workbench.quick_access.menu = command_palette::menu(&[extension]);
        workbench.restore_profile_state(profile);
        let runs = runs.clone();
        workbench.commands.register("port.hello", move |_, _, _, _| {
            runs.set(runs.get() + 1);
            Outcome::done()
        });
        workbench
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("shift-cmd-p");
    let (value, placeholder) = workbench.read_with(cx, |workbench, _| {
        let input = workbench.quick_inputs.current().unwrap();
        (input.value.clone(), input.placeholder.clone())
    });
    assert_eq!(value, ">");
    assert_eq!(placeholder.as_deref(), Some("Type the name of a command to run."));
    let all = workbench.read_with(cx, labels);
    assert!(all.contains(&"Port: Say Hello".to_string()), "{all:?}");
    assert!(all.contains(&"Show All Commands".to_string()), "{all:?}");

    cx.simulate_input("say hel");
    assert_eq!(workbench.read_with(cx, labels), ["Port: Say Hello"]);
    let highlights = workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().unwrap().rows[0].label_highlights.clone());
    assert_eq!(highlights, [6..13]);

    cx.simulate_keystrokes("enter");
    assert_eq!(runs.get(), 1);
    let stored = |key: &str| storage.lock().unwrap().get(key).map(str::to_owned);
    assert_eq!(stored("commandPalette.mru.cache").as_deref(), Some(r#"{"usesLRU":true,"entries":[{"key":"port.hello","value":41}]}"#));
    assert_eq!(stored("commandPalette.mru.counter").as_deref(), Some("42"));
    assert!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().is_none()));

    cx.simulate_keystrokes("shift-cmd-p");
    let reopened = workbench.read_with(cx, labels);
    assert_eq!(reopened[0], "--recently used--Port: Say Hello");
    assert!(reopened[1].starts_with("--other commands--"), "{reopened:?}");

    cx.simulate_input("zzzz");
    assert_eq!(workbench.read_with(cx, labels), ["No matching commands"]);
    cx.simulate_keystrokes("enter");
    assert!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().is_some()), "no matching commands keeps the picker");
    cx.simulate_keystrokes("escape");
    assert!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().is_none()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn removing_a_recently_used_pick_keeps_the_others() {
    let mut input = crate::quick_input::QuickInput::new(1, crate::quick_input::Kind::QuickPick);
    input.show();
    let item = |handle: i64, label: &str| Entry::Item(crate::quick_input::Item { handle, label: label.into(), pickable: true, ..Default::default() });
    input.set_items(vec![Entry::Separator(Some("recently used".into())), item(0, "a"), item(1, "b"), Entry::Separator(Some("other commands".into())), item(2, "c")]);
    input.set_active_items(&[1]);
    input.remove_item(0);
    assert_eq!(input.order.iter().map(|row| input.rows[*row].item.label.as_str()).collect::<Vec<_>>(), ["b", "c"]);
    assert_eq!(input.active_handles(), [1]);
}
