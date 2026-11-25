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
    cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
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
    assert_eq!(edit.iter().filter(|action| **action == MenuAction::Separator).count(), 2);
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
fn native_menus_show_keybindings_as_key_equivalents_or_in_the_label(cx: &mut TestAppContext) {
    let (workbench, cx) = workbench(None, cx);
    let menubar = cx.update(|window, cx| workbench.read(cx).menubar(window));
    let (menus, key_equivalents) = native_menus(&menubar);
    let names: Vec<&str> = menus.iter().map(|menu| menu.name.as_ref()).collect();
    assert_eq!(names, ["Code - OSS", "File", "Edit", "Selection", "View", "Go", "Window", "Help"]);
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
