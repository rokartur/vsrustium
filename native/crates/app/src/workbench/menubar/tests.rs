use std::path::Path;

use exthost::scan::scan_extension;
use gpui::TestAppContext;

use super::*;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;

fn workbench<'a>(file: Option<&Path>, cx: &'a mut TestAppContext) -> (gpui::Entity<Workbench>, &'a mut gpui::VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let keybindings = std::env::temp_dir().join(format!("code-gpui-menubar-{}.json", std::process::id()));
    // A fresh writable User folder, so commands that toggle a setting can write it.
    let user = std::env::temp_dir().join(format!("code-gpui-menubar-user-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&user);
    std::fs::create_dir_all(&user).unwrap();
    let settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), user, None)));
    cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), keybindings, cx);
        if let Some(file) = file {
            workbench.open_file(file.to_path_buf(), true, window, cx);
        }
        workbench
    })
}

fn command<'a>(menubar: &'a Menubar, menu: &str, id: &str) -> &'a MenuAction {
    let (_, actions) = menubar.menus.iter().find(|(title, _)| title == menu).unwrap();
    actions.iter().find(|action| matches!(action, MenuAction::Command { id: found, .. } if found == id)).unwrap()
}

fn enabled(action: &MenuAction) -> bool {
    matches!(action, MenuAction::Command { enabled: true, .. })
}

#[gpui::test]
fn a_menu_click_runs_its_command_with_from_menu_in_the_active_window(cx: &mut TestAppContext) {
    let (workbench, cx) = workbench(None, cx);
    let runs = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    workbench.update(cx, |workbench, _| {
        let runs = runs.clone();
        workbench.commands.register("port.menu", move |_, args, _, _| {
            runs.borrow_mut().push(args);
            crate::commands::Outcome::Ready(Ok(serde_json::Value::Null))
        });
    });
    cx.update(|window, cx| install(window.window_handle().downcast().unwrap(), cx));
    cx.dispatch_action(RunMenuCommand { id: "port.menu".into() });
    cx.run_until_parked();
    assert_eq!(*runs.borrow(), [vec![json!({ "from": "menu" })]]);
}

#[gpui::test]
fn menus_follow_the_reference_order_and_enablement(cx: &mut TestAppContext) {
    let (workbench, cx) = workbench(None, cx);
    let menubar = cx.update(|window, cx| workbench.read(cx).menubar(window));
    let titles: Vec<&str> = menubar.menus.iter().map(|(title, _)| title.as_str()).collect();
    assert_eq!(titles, ["File", "Edit", "Selection", "View", "Go", "Help", "Preferences"]);
    // The empty window of the reference capture: Save, Save All and Revert File are disabled.
    for id in ["workbench.action.files.save", "saveAll", "workbench.action.files.revert"] {
        assert!(!enabled(command(&menubar, "File", id)), "{id}");
    }
    assert!(enabled(command(&menubar, "Edit", "undo")));
    let (_, edit) = menubar.menus.iter().find(|(title, _)| title == "Edit").unwrap();
    assert_eq!(edit.iter().filter(|action| **action == MenuAction::Separator).count(), 4);
    assert!(enabled(command(&menubar, "Edit", "editor.action.commentLine")));
}

#[gpui::test]
fn an_open_file_enables_save_and_revert(cx: &mut TestAppContext) {
    let file = std::env::temp_dir().join(format!("code-gpui-menubar-{}.txt", std::process::id()));
    std::fs::write(&file, "a").unwrap();
    let (workbench, cx) = workbench(Some(&file), cx);
    let menubar = cx.update(|window, cx| workbench.read(cx).menubar(window));
    assert!(enabled(command(&menubar, "File", "workbench.action.files.save")));
    assert!(enabled(command(&menubar, "File", "workbench.action.files.revert")));
    assert!(!enabled(command(&menubar, "File", "saveAll")));
    std::fs::remove_file(&file).unwrap();
}

#[gpui::test]
fn the_selection_menu_names_the_multi_cursor_modifier_it_switches_to(cx: &mut TestAppContext) {
    let (workbench, cx) = workbench(None, cx);
    let title = |cx: &mut gpui::VisualTestContext| {
        let menubar = cx.update(|window, cx| workbench.read(cx).menubar(window));
        let MenuAction::Command { title, .. } = command(&menubar, "Selection", "workbench.action.toggleMultiCursorModifier") else { unreachable!() };
        title.clone()
    };
    assert_eq!(title(cx), "Switch to Cmd+Click for Multi-Cursor");
    cx.update(|window, cx| workbench.update(cx, |workbench, cx| workbench.execute_command("workbench.action.toggleMultiCursorModifier", Vec::new(), window, cx)));
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.setting("editor.multiCursorModifier")), json!("ctrlCmd"));
    assert_eq!(title(cx), "Switch to Alt+Click for Multi-Cursor");
}

#[gpui::test]
fn native_menus_show_keybindings_as_key_equivalents_or_in_the_label(cx: &mut TestAppContext) {
    let (workbench, cx) = workbench(None, cx);
    let menubar = cx.update(|window, cx| workbench.read(cx).menubar(window));
    let (menus, key_equivalents) = native_menus(&menubar);
    let names: Vec<&str> = menus.iter().map(|menu| menu.name.as_ref()).collect();
    assert_eq!(names, ["VSRustium", "File", "Edit", "Selection", "View", "Go", "Window", "Help"]);
    let MenuItem::Submenu(preferences) = &menus[0].items[0] else { panic!("Preferences first") };
    let themes = preferences.items.iter().find_map(|item| match item {
        MenuItem::Submenu(menu) if menu.name.as_ref() == "Themes" => Some(menu),
        _ => None,
    });
    let themes = themes.expect("Themes");
    assert!(preferences.items.iter().any(|item| matches!(item, MenuItem::Action { name, .. } if name.as_ref() == "Keyboard Shortcuts [⌘K ⌘S]")));
    let MenuItem::Action { name, .. } = &themes.items[0] else { panic!("Color Theme") };
    assert_eq!(name.as_ref(), "Color Theme [⌘K ⌘T]");
    let MenuItem::Action { name, .. } = &menus[4].items[0] else { panic!("Command Palette") };
    assert_eq!(name.as_ref(), "Command Palette…");
    let keystrokes: Vec<String> = key_equivalents.iter().map(|binding| binding.keystrokes().iter().map(|keystroke| keystroke.unparse()).collect()).collect();
    for keystroke in ["cmd-q", "cmd-s", "alt-cmd-s", "cmd-z", "cmd-shift-z", "cmd-a", "cmd-shift-p", "cmd-p"] {
        assert!(keystrokes.iter().any(|found| found == keystroke), "{keystroke} in {keystrokes:?}");
    }
}

#[gpui::test]
fn open_recent_lists_folders_then_files_before_more(cx: &mut TestAppContext) {
    let (workbench, cx) = workbench(None, cx);
    let menubar = cx.update(|window, cx| {
        workbench.update(cx, |workbench, _| {
            let mut storage = workbench.shared_storage.lock().unwrap();
            let entries = vec![
                crate::recently_opened::entry(crate::recently_opened::Kind::Folder, Path::new("/tmp/a&b"), None),
                crate::recently_opened::entry(crate::recently_opened::Kind::File, Path::new("/tmp/one.txt"), None),
            ];
            crate::recently_opened::add(&mut storage, entries);
            drop(storage);
            workbench.recently_opened = crate::recently_opened::load(&mut workbench.shared_storage.lock().unwrap());
        });
        workbench.read(cx).menubar(window)
    });
    assert_eq!(menubar.recent.iter().map(|(label, _)| label.as_str()).collect::<Vec<_>>(), ["/tmp/a&b", "/tmp/one.txt"]);
    assert_eq!(menubar.recent_workspaces, 1);
    let (menus, _) = native_menus(&menubar);
    let MenuItem::Submenu(recent) = menus[1].items.iter().find(|item| matches!(item, MenuItem::Submenu(menu) if menu.name.as_ref() == "Open Recent")).unwrap() else { unreachable!() };
    let names: Vec<String> = recent.items.iter().map(|item| match item {
        MenuItem::Action { name, .. } => name.to_string(),
        MenuItem::Separator => "-".into(),
        _ => "?".into(),
    }).collect();
    assert_eq!(names[..5], ["/tmp/a&b", "-", "/tmp/one.txt", "-", "More…"]);
}

#[test]
fn truncate_middle_keeps_both_ends() {
    assert_eq!(truncate_middle("abcdefghij", 5), "ab…ij");
    assert_eq!(truncate_middle("abc", 5), "abc");
}

/// The menu bar against 1.147.0's `IMenubarData` (`port/tools/ref-menubar`, menus and keybinding labels kept in
/// `reference-empty.json` and `reference-file.json`): every reference item whose command the port registers, in the
/// reference's order, groups, labels and enablement, nothing more. Open Recent's entries depend on the history.
fn assert_menubar_matches_the_reference(reference: &str, folder: Option<&Path>, file: Option<&Path>, cx: &mut TestAppContext) {
    let reference: Value = serde_json::from_str(reference).unwrap();
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let user = std::env::temp_dir().join(format!("code-gpui-menubar-reference-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&user);
    std::fs::create_dir_all(&user).unwrap();
    let settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), user.clone(), None)));
    let workspace = folder.map(|folder| config::workspace::Workspace::folder(folder.to_path_buf()));
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), workspace, Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), user.join("keybindings.json"), cx);
        if let Some(file) = file {
            workbench.open_file(file.to_path_buf(), true, window, cx);
        }
        workbench
    });
    cx.run_until_parked();
    let (menubar, registered) = cx.update(|window, cx| {
        let workbench = workbench.read(cx);
        let menubar = workbench.menubar(window);
        let ids = reference_ids(&reference["menus"]);
        (menubar, ids.into_iter().filter(|id| workbench.commands.get(id).is_some()).collect::<Vec<_>>())
    });
    let mut port = Vec::new();
    let mut expected = Vec::new();
    for (title, actions) in &menubar.menus {
        port.push(format!("{title}:"));
        port_lines(actions, 1, &mut port);
        expected.push(format!("{title}:"));
        reference_lines(&reference["menus"][title]["items"], 1, &registered, &mut expected);
    }
    assert_eq!(port.join("\n"), expected.join("\n"));
    let labels = |id: &str| reference["keybindings"][id].as_str().map(str::to_string);
    for (id, chords) in &menubar.keybindings {
        // menubarControl.ts: one chord as its Electron accelerator, a chord sequence as its label.
        let label = match chords.as_slice() {
            [chord] => chord.user_settings_label().split('+').map(|part| part[..1].to_uppercase() + &part[1..]).collect::<Vec<_>>().join("+"),
            _ => crate::keybindings::label(chords),
        };
        assert_eq!(Some(label), labels(id), "{id}");
    }
    std::fs::remove_dir_all(user).unwrap();
}

fn reference_ids(menus: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    let mut stack: Vec<&Value> = menus.as_object().unwrap().values().map(|menu| &menu["items"]).collect();
    while let Some(items) = stack.pop() {
        for item in items.as_array().unwrap() {
            ids.push(item["id"].as_str().unwrap().to_string());
            if let Some(submenu) = item.get("submenu") {
                stack.push(&submenu["items"]);
            }
        }
    }
    ids
}

fn label(title: &str) -> String {
    title.replace("&&", "")
}

fn port_lines(actions: &[MenuAction], depth: usize, lines: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    for action in actions {
        match action {
            MenuAction::Command { id, title, enabled, checked, .. } => lines.push(format!("{indent}{} [{id}]{}{}", label(title), if *enabled { "" } else { " disabled" }, if *checked { " checked" } else { "" })),
            MenuAction::Submenu { title, actions, .. } => {
                lines.push(format!("{indent}{} >", label(title)));
                port_lines(actions, depth + 1, lines);
            }
            MenuAction::Separator => lines.push(format!("{indent}---")),
        }
    }
}

/// The reference items the port can show; separators collapse as `getFlatContextMenuActions` leaves them.
fn reference_lines(items: &Value, depth: usize, registered: &[String], lines: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    let start = lines.len();
    for item in items.as_array().into_iter().flatten() {
        let id = item["id"].as_str().unwrap();
        if id == "vscode.menubar.separator" {
            if lines.len() > start && !lines.last().unwrap().ends_with("---") {
                lines.push(format!("{indent}---"));
            }
        } else if let Some(submenu) = item.get("submenu") {
            let mut children = Vec::new();
            reference_lines(&submenu["items"], depth + 1, registered, &mut children);
            if !children.is_empty() {
                lines.push(format!("{indent}{} >", label(item["label"].as_str().unwrap())));
                lines.extend(children);
            }
        } else if !id.starts_with("openRecent") && registered.iter().any(|known| known == id) {
            let disabled = if item["enabled"] == json!(false) { " disabled" } else { "" };
            let checked = if item["checked"] == json!(true) { " checked" } else { "" };
            lines.push(format!("{indent}{} [{id}]{disabled}{checked}", label(item["label"].as_str().unwrap())));
        }
    }
    if lines.len() > start && lines.last().unwrap().ends_with("---") {
        lines.pop();
    }
}

#[gpui::test]
fn the_empty_window_menu_bar_matches_the_reference(cx: &mut TestAppContext) {
    assert_menubar_matches_the_reference(include_str!("reference-empty.json"), None, None, cx);
}

#[gpui::test]
fn the_folder_and_file_menu_bar_matches_the_reference(cx: &mut TestAppContext) {
    let folder = std::env::temp_dir().join(format!("code-gpui-menubar-folder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("notes.txt"), "notes\n").unwrap();
    assert_menubar_matches_the_reference(include_str!("reference-file.json"), Some(&folder), Some(&folder.join("notes.txt")), cx);
    std::fs::remove_dir_all(folder).unwrap();
}
