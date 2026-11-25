use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Point, TestAppContext, VisualTestContext, point, px};
use serde_json::{Value, json};

use super::reveal_path_items;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::menus::MenuAction;
use crate::theme::load_theme;
use crate::workbench::Workbench;

#[test]
fn the_title_menu_reveals_each_folder_of_the_file_from_the_file_up() {
    let items = reveal_path_items(Path::new("/Users/me/ws/notes.txt"));
    let expected = [("/Users", "/"), ("/Users/me", "Users"), ("/Users/me/ws", "me"), ("/Users/me/ws/notes.txt", "ws"), ("/Users/me/ws/notes.txt", "notes.txt")];
    assert_eq!(items, expected.map(|(path, label)| (PathBuf::from(path), label.to_string())));
}

fn menu_titles(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    workbench.read_with(cx, |workbench, _| {
        let Some(menu) = workbench.context_menu.as_ref() else { return Vec::new() };
        menu.levels[0].actions
            .iter()
            .map(|action| match action {
                MenuAction::Command { title, checked: true, .. } => format!("{title} [x]"),
                MenuAction::Command { title, .. } => title.clone(),
                _ => "---".into(),
            })
            .collect()
    })
}

fn right_click(position: Point<Pixels>, cx: &mut VisualTestContext) {
    cx.simulate_event(MouseDownEvent { button: MouseButton::Right, position, modifiers: Modifiers::default(), click_count: 1, first_mouse: false });
    cx.simulate_event(MouseUpEvent { button: MouseButton::Right, position, modifiers: Modifiers::default(), click_count: 1 });
    cx.run_until_parked();
}

#[gpui::test]
fn the_title_bar_menus_toggle_the_command_center_and_the_layout_controls(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-title-bar-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    let file = dir.join("notes.txt");
    std::fs::write(&file, "hello").unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings.clone(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("User/keybindings.json"), cx);
        workbench.open_file(file.clone(), true, window, cx);
        workbench
    });
    cx.run_until_parked();
    let width = workbench.update_in(cx, |_, window, _| f32::from(window.viewport_size().width));
    // Between the title and the layout controls, which take the last 112px.
    let bar = point(px(width - 150.), px(15.));
    let title = point(px(width / 2.), px(17.));
    let user_settings = || serde_json::from_str::<Value>(&std::fs::read_to_string(dir.join("User/settings.json")).unwrap()).unwrap();

    right_click(bar, cx);
    assert_eq!(menu_titles(&workbench, cx), ["Command Center [x]", "Navigation Controls [x]", "Layout Controls [x]"]);
    cx.simulate_keystrokes("escape");
    // No-drag parts (layout controls, command center, quick input): the test window panics on a window move.
    cx.simulate_click(point(px(width - 20.), px(17.)), Modifiers::default());
    assert!(workbench.update_in(cx, |workbench, window, _| workbench.editor_focus.is_focused(window)), "focus goes back to the editor");

    // A click on the command center shows Quick Open with the help picks above the history.
    cx.simulate_click(title, Modifiers::default());
    let picker = workbench.read_with(cx, |workbench, _| {
        let input = workbench.quick_inputs.current().expect("Quick Open");
        let rows = input.order.iter().map(|row| &input.rows[*row]);
        (input.input_box.value.clone(), rows.map(|row| format!("{}|{}|{:?}", row.item.label, row.item.description.clone().unwrap_or_default(), row.item.keybinding)).collect::<Vec<_>>())
    });
    assert_eq!(picker, (String::new(), vec![r#"Go to File||[["⌘", "P"]]"#.to_string(), r#"Show and Run Commands|>|[["⇧", "⌘", "P"]]"#.into(), r#"Go to Symbol in Editor|@|[["⇧", "⌘", "O"]]"#.into(), "More|?|[]".into(), format!("notes.txt|{}|[]", dir.display())]));
    cx.simulate_click(point(px(width / 2.), px(20.)), Modifiers::default());
    assert!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().is_some()), "a press in the quick input over the bar keeps it");
    cx.simulate_keystrokes("down enter");
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().unwrap().input_box.value.clone()), ">");
    cx.simulate_keystrokes("escape");

    // The command center's own toolbar menu: its Configure Keybinding, then the bar's toggles.
    right_click(title, cx);
    assert_eq!(menu_titles(&workbench, cx), ["Configure Keybinding", "---", "Command Center [x]", "Navigation Controls [x]", "Layout Controls [x]"]);
    cx.simulate_keystrokes("escape");

    // Cmd+click over the title shows the title's menu, which leads with the file's path, from the file up to `/`.
    let cmd = Modifiers { platform: true, ..Modifiers::default() };
    cx.simulate_click(title, cmd);
    assert!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().is_none()), "the title's menu, not Quick Open");
    let titles = menu_titles(&workbench, cx);
    let path_items = titles.iter().position(|title| title == "---").unwrap();
    assert_eq!(titles[0], "notes.txt");
    assert_eq!(titles[path_items - 1], "/");
    let opened = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().path.clone());
    assert_eq!(path_items, opened.components().count());
    assert_eq!(titles[path_items + 1..], ["Command Center [x]", "Navigation Controls [x]", "Layout Controls [x]"]);
    cx.simulate_keystrokes("escape");

    // Off, the command center leaves the window title, and Navigation Controls leave the menu with it.
    workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("toggle.window.commandCenter", Vec::new(), window, cx));
    assert_eq!(user_settings(), json!({ "window.commandCenter": false }));
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.window_title()), "notes.txt");
    right_click(title, cx);
    let titles = menu_titles(&workbench, cx);
    assert_eq!(titles[0], "notes.txt");
    assert_eq!(titles[path_items + 1..], ["Command Center", "Layout Controls [x]"]);
    cx.simulate_keystrokes("escape");
    right_click(bar, cx);
    assert_eq!(menu_titles(&workbench, cx), ["Command Center", "Layout Controls [x]"]);
    cx.simulate_keystrokes("down down enter");
    assert_eq!(user_settings(), json!({ "window.commandCenter": false, "workbench.layoutControl.enabled": false }));

    // Back to the default value, the key leaves settings.json.
    right_click(bar, cx);
    assert_eq!(menu_titles(&workbench, cx), ["Command Center", "Layout Controls"]);
    cx.simulate_keystrokes("down enter");
    assert_eq!(user_settings(), json!({ "workbench.layoutControl.enabled": false }));
    std::fs::remove_dir_all(dir).unwrap();
}

// The test window's `start_window_move` is `unimplemented!()`, so reaching the drag region panics.
#[gpui::test]
#[should_panic(expected = "not implemented")]
fn a_press_on_the_empty_title_bar_moves_the_window(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), std::env::temp_dir().join("code-gpui-title-bar-drag/User"), None)));
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx));
    cx.run_until_parked();
    let width = workbench.update_in(cx, |_, window, _| f32::from(window.viewport_size().width));
    cx.simulate_event(MouseDownEvent { button: MouseButton::Left, position: point(px(width - 150.), px(15.)), modifiers: Modifiers::default(), click_count: 1, first_mouse: false });
}
