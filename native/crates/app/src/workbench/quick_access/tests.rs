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
        (input.input_box.value.clone(), input.placeholder.clone())
    });
    assert_eq!(value, ">");
    assert_eq!(placeholder.as_deref(), Some("Type the name of a command to run."));
    let all = workbench.read_with(cx, labels);
    assert!(all.contains(&"Port: Say Hello".to_string()), "{all:?}");
    assert!(all.contains(&"Show All Commands".to_string()), "{all:?}");
    // `viewContainer.workbench.view.explorer.enabled` and `workbench.view.search.active` hold.
    assert!(all.contains(&"View: Show Explorer".to_string()) && all.contains(&"View: Show Search".to_string()), "{all:?}");

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

    cx.simulate_input("color theme");
    let row = workbench.read_with(cx, |workbench, _| {
        let input = workbench.quick_inputs.current().unwrap();
        let item = &input.rows[input.order[0]].item;
        (item.label.clone(), item.keybinding.clone())
    });
    assert_eq!(row, ("Preferences: Color Theme".to_string(), vec![vec!["⌘".to_string(), "K".to_string()], vec!["⌘".to_string(), "T".to_string()]]));

    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input(">zzzz");
    assert_eq!(workbench.read_with(cx, labels), ["No matching commands"]);
    cx.simulate_keystrokes("enter");
    assert!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().is_some()), "no matching commands keeps the picker");
    cx.simulate_keystrokes("escape");
    assert!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().is_none()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn go_to_line_previews_the_line_and_restores_the_view_on_escape(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-goto-line-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.txt");
    std::fs::write(&file, (1..=200).map(|line| format!("line {line}\n")).collect::<String>()).unwrap();
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
        workbench.open_file(file.clone(), true, window, cx);
        workbench
    });
    cx.run_until_parked();
    let editor = |cx: &mut gpui::VisualTestContext| {
        workbench.read_with(cx, |workbench, _| {
            let editor = workbench.editors.active().unwrap();
            let highlight = workbench.quick_access.goto_line.as_ref().and_then(|state| state.highlight(editor.model_id)).map(|(line, _)| line);
            (editor.selection().head.line, editor.selection().head.column, editor.scroll.top > 0., highlight)
        })
    };

    cx.simulate_keystrokes("ctrl-g");
    assert_eq!(workbench.read_with(cx, labels), ["Type a line number to go to (from 1 to 201)."]);
    cx.simulate_input("150");
    assert_eq!(workbench.read_with(cx, labels), ["Press 'Enter' to go to line 150 or enter colon : to add a column number."]);
    assert_eq!(editor(cx), (0, 0, true, Some(149)), "the line is revealed and highlighted, the cursor stays");
    cx.simulate_keystrokes("escape");
    assert_eq!(editor(cx), (0, 0, false, None), "the view state comes back");

    cx.simulate_keystrokes("ctrl-g");
    cx.simulate_input(":5");
    let buttons = workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().unwrap().buttons.len());
    assert_eq!((workbench.read_with(cx, labels), buttons), (vec!["Press 'Enter' to go to line 1 at column 5.".to_string()], 1), "offset mode shows the toggle");
    cx.simulate_keystrokes("backspace backspace");
    cx.simulate_input("150:3");
    cx.simulate_keystrokes("enter");
    assert_eq!(editor(cx), (149, 2, true, None));
    assert!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().is_none()));
    std::fs::remove_dir_all(dir).unwrap();
}

/// helpQuickAccess.ts: `?` lists the providers by prefix, accepting one shows it, typing its prefix too.
#[gpui::test]
fn help_lists_the_providers_and_shows_the_picked_one(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-help-{}", std::process::id()));
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
        workbench
    });
    cx.run_until_parked();
    let picker = |cx: &mut gpui::VisualTestContext| {
        workbench.read_with(cx, |workbench, _| {
            let input = workbench.quick_inputs.current().unwrap();
            let rows = input.order.iter().map(|row| &input.rows[*row]).map(|row| format!("{}|{}|{:?}", row.item.label, row.item.description.clone().unwrap_or_default(), row.item.keybinding)).collect::<Vec<_>>();
            (input.input_box.value.clone(), input.placeholder.clone().unwrap_or_default(), rows)
        })
    };

    cx.simulate_keystrokes("cmd-p");
    cx.simulate_input("?");
    let (value, placeholder, rows) = picker(cx);
    assert_eq!((value.as_str(), placeholder.as_str()), ("?", "Type '?' to get help on the actions you can take from here."));
    assert_eq!(rows, [
        r#"…|Go to File|[["⌘", "P"]]"#,
        r#":|Go to Line/Column|[["⌃", "G"]]"#,
        "::|Go to Offset|[]",
        r#"@|Go to Symbol in Editor|[["⇧", "⌘", "O"]]"#,
        "@:|Go to Symbol in Editor by Category|[]",
        r#"#|Go to Symbol in Workspace|[["⌘", "T"]]"#,
        r#">|Show and Run Commands|[["⇧", "⌘", "P"]]"#,
    ]);
    cx.simulate_keystrokes("down enter");
    assert_eq!(picker(cx).0, ":", "accept shows the picked provider");

    cx.simulate_keystrokes("escape cmd-p");
    cx.simulate_input("?>");
    cx.run_until_parked();
    assert_eq!(picker(cx).0, ">", "a typed prefix shows its provider");
}

/// gotoLineQuickAccess.ts `provideWithTextEditor`: relative line numbers show as "on" while the picker is open.
#[gpui::test]
fn go_to_line_shows_relative_line_numbers_as_on_while_open(cx: &mut TestAppContext) {
    use crate::workbench::editor_view::LineNumbers;

    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-goto-line-relative-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("User")).unwrap();
    std::fs::write(dir.join("User/settings.json"), r#"{ "editor.lineNumbers": "relative" }"#).unwrap();
    let file = dir.join("a.txt");
    std::fs::write(&file, "a\nb\nc\n").unwrap();
    let settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
        workbench.open_file(file.clone(), true, window, cx);
        workbench
    });
    cx.run_until_parked();
    let line_numbers = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.line_numbers(workbench.editors.active().unwrap()));

    assert_eq!(line_numbers(cx), LineNumbers::Relative);
    cx.simulate_keystrokes("ctrl-g");
    assert_eq!(line_numbers(cx), LineNumbers::On);
    cx.simulate_keystrokes("escape");
    assert_eq!(line_numbers(cx), LineNumbers::Relative);
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
