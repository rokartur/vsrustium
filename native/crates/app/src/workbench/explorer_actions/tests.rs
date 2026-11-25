use std::path::{Path, PathBuf};

use config::workspace::Workspace;
use exthost::scan::scan_extension;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::menus::MenuAction;
use crate::theme::load_theme;
use crate::workbench::Workbench;

fn explorer<'a>(name: &str, cx: &'a mut TestAppContext) -> (PathBuf, gpui::Entity<Workbench>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    // A dropped receiver: the file work runs on Disk, without participants.
    let host = HostSync::new(std::sync::mpsc::channel().0);
    let folder = dir.clone();
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(Workspace::folder(folder.clone())), Languages::new(&[]), crate::default_settings(), host, cx);
        workbench.skip_dialogs = true;
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), folder.join("keybindings.json"), cx);
        window.focus(&workbench.explorer.focus.clone(), cx);
        workbench
    });
    cx.run_until_parked();
    (dir, workbench, cx)
}

fn select(workbench: &gpui::Entity<Workbench>, path: &Path, cx: &mut VisualTestContext) {
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.folder.as_mut().unwrap().set_selection(path);
        window.focus(&workbench.explorer.focus.clone(), cx);
    });
    cx.run_until_parked();
}

fn run(workbench: &gpui::Entity<Workbench>, command: &str, cx: &mut VisualTestContext) {
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.execute_command(command, Vec::new(), window, cx);
    });
    cx.run_until_parked();
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names.retain(|name| name != "keybindings.json");
    names
}

#[gpui::test]
fn new_file_takes_a_validated_name_and_opens(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("explorer-new", cx);
    select(&workbench, &dir.join("a.txt"), cx);

    run(&workbench, "explorer.newFile", cx);
    cx.simulate_input("a.txt");
    let message = workbench.read_with(cx, |workbench, _| workbench.explorer_edit_message());
    assert_eq!(message.unwrap().0, "A file or folder **a.txt** already exists at this location. Please choose a different name.");
    cx.simulate_keystrokes("enter");
    assert!(workbench.read_with(cx, |workbench, _| workbench.explorer.edit.is_some()), "an invalid name keeps the editor");

    cx.simulate_keystrokes("backspace backspace backspace backspace backspace");
    cx.simulate_input("b.txt");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(names(&dir), ["a.txt", "b.txt", "sub"]);
    let opened = workbench.read_with(cx, |workbench, _| workbench.editors.active().map(|editor| editor.document.path.clone()));
    assert_eq!(opened, Some(dir.join("b.txt")));

    run(&workbench, "explorer.newFolder", cx);
    cx.simulate_keystrokes("escape");
    assert_eq!(names(&dir), ["a.txt", "b.txt", "sub"], "escape creates nothing");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[gpui::test]
fn dragging_a_file_onto_a_folder_moves_it_and_alt_copies(cx: &mut TestAppContext) {
    let (dir, _workbench, cx) = explorer("explorer-dnd", cx);
    // Rows are 22px from y 96 under the section header: sub, then a.txt.
    let (sub_row, file_row) = (point(px(120.), px(107.)), point(px(120.), px(129.)));
    let drag = |from, to, modifiers: Modifiers, cx: &mut VisualTestContext| {
        cx.simulate_mouse_down(from, MouseButton::Left, modifiers);
        cx.simulate_mouse_move(point(px(120.), px(118.)), MouseButton::Left, modifiers);
        cx.simulate_mouse_move(to, MouseButton::Left, modifiers);
        cx.simulate_mouse_up(to, MouseButton::Left, modifiers);
        cx.run_until_parked();
    };

    drag(file_row, sub_row, Modifiers::alt(), cx);
    assert_eq!(names(&dir), ["a.txt", "sub"]);
    assert_eq!(names(&dir.join("sub")), ["a.txt"], "alt copies");

    // The copy opened sub/a.txt and revealing it expanded sub, so a.txt is the third row now.
    drag(point(px(120.), px(151.)), sub_row, Modifiers::none(), cx);
    assert_eq!(names(&dir), ["sub"]);
    assert_eq!(names(&dir.join("sub")), ["a.txt"], "the moved file replaces the copy");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[gpui::test]
fn files_dropped_from_finder_copy_into_the_folder_under_the_pointer(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("explorer-import", cx);
    let finder = dir.with_extension("finder");
    std::fs::create_dir_all(finder.join("folder")).unwrap();
    std::fs::write(finder.join("dropped.txt"), "dropped").unwrap();
    std::fs::write(finder.join("a.txt"), "from finder").unwrap();
    let drop = |paths: &[&str], at, cx: &mut VisualTestContext| {
        let paths = gpui::ExternalPaths(paths.iter().map(|name| finder.join(name)).collect());
        cx.simulate_event(gpui::FileDropEvent::Entered { position: at, paths });
        cx.simulate_event(gpui::FileDropEvent::Pending { position: at });
        cx.simulate_event(gpui::FileDropEvent::Submit { position: at });
        cx.run_until_parked();
    };
    // Rows are 22px from y 96 under the section header: sub, then a.txt; the empty area follows.
    let (sub_row, third_row, empty_area) = (point(px(120.), px(107.)), point(px(120.), px(151.)), point(px(120.), px(300.)));

    drop(&["dropped.txt"], sub_row, cx);
    assert_eq!(names(&dir.join("sub")), ["dropped.txt"]);
    assert_eq!(names(&finder), ["a.txt", "dropped.txt", "folder"], "a drop copies");
    let opened = workbench.read_with(cx, |workbench, _| workbench.editors.active().map(|editor| editor.document.path.clone()));
    assert_eq!(opened, Some(dir.join("sub/dropped.txt")), "explorer.autoOpenDroppedFile");

    // A folder dropped into a workspace folder asks first; the skipped dialog picks Add, which the port lacks.
    drop(&["folder"], empty_area, cx);
    assert!(!dir.join("folder").exists());

    // A name the folder has asks to replace it; the skipped dialog confirms. Revealing sub/dropped.txt expanded sub,
    // so a.txt is the third row.
    drop(&["a.txt"], third_row, cx);
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "from finder");
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&finder).unwrap();
}

#[gpui::test]
fn rename_copy_paste_cut_delete_and_undo(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("explorer-ops", cx);
    select(&workbench, &dir.join("a.txt"), cx);

    // Enter renames with the name before the extension selected; Cmd+A in the name editor selects all of it.
    // The window losing focus (Cmd+Tab away and back) keeps the name editor.
    cx.update(|window, _| window.activate_window());
    cx.simulate_keystrokes("enter");
    cx.deactivate_window();
    cx.run_until_parked();
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("c.txt");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(names(&dir), ["c.txt", "sub"]);
    // The renamed row keeps its selection (explorerService.ts renames the model element in place).
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.folder.as_ref().unwrap().selected.clone()), [dir.join("c.txt")]);

    cx.simulate_keystrokes("cmd-c cmd-v");
    cx.run_until_parked();
    assert_eq!(names(&dir), ["c copy.txt", "c.txt", "sub"]);
    select(&workbench, &dir.join("c.txt"), cx);
    cx.simulate_keystrokes("cmd-v");
    cx.run_until_parked();
    assert_eq!(names(&dir), ["c copy 2.txt", "c copy.txt", "c.txt", "sub"]);

    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(names(&dir), ["c copy.txt", "c.txt", "sub"]);
    cx.simulate_keystrokes("shift-cmd-z");
    cx.run_until_parked();
    assert_eq!(names(&dir), ["c copy 2.txt", "c copy.txt", "c.txt", "sub"]);

    select(&workbench, &dir.join("c copy 2.txt"), cx);
    cx.simulate_keystrokes("cmd-x");
    assert!(workbench.read_with(cx, |workbench, _| workbench.folder.as_ref().unwrap().is_cut(&dir.join("c copy 2.txt"))));
    select(&workbench, &dir.join("sub"), cx);
    cx.simulate_keystrokes("cmd-v");
    cx.run_until_parked();
    assert_eq!(names(&dir.join("sub")), ["c copy 2.txt"]);

    select(&workbench, &dir.join("c copy.txt"), cx);
    cx.simulate_keystrokes("alt-cmd-backspace");
    cx.run_until_parked();
    assert_eq!(names(&dir), ["c.txt", "sub"]);
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(names(&dir), ["c copy.txt", "c.txt", "sub"], "undo restores the deleted file");
    std::fs::remove_dir_all(&dir).unwrap();
}

fn menu_titles(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    workbench.read_with(cx, |workbench, _| {
        let menu = workbench.context_menu.as_ref().expect("a context menu shows");
        menu.actions
            .iter()
            .map(|action| match action {
                MenuAction::Command { title, enabled: true, .. } => title.clone(),
                MenuAction::Command { title, .. } => format!("({title})"),
                _ => "---".into(),
            })
            .collect()
    })
}

#[gpui::test]
fn the_context_menu_selects_the_row_and_runs_its_items_with_keys_and_clicks(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("explorer-context-menu", cx);
    let file_row = point(px(120.), px(129.));
    cx.simulate_mouse_down(file_row, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(file_row, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(menu_titles(&workbench, cx), ["Reveal in Finder", "---", "Cut", "Copy", "---", "Copy Path", "Copy Relative Path", "---", "Rename...", "Delete"]);
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.folder.as_ref().unwrap().selected.clone()), [dir.join("a.txt")]);

    // The pointer focuses the item under it; Down goes on from there and skips the separator; Enter runs it.
    let focused = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.context_menu.as_ref().unwrap().focused);
    cx.simulate_mouse_move(point(px(150.), px(148.)), None, Modifiers::none());
    assert_eq!(focused(cx), Some(0));
    cx.simulate_mouse_move(point(px(150.), px(400.)), None, Modifiers::none());
    assert_eq!(focused(cx), None);
    cx.simulate_mouse_move(point(px(150.), px(148.)), None, Modifiers::none());
    cx.simulate_keystrokes("pagedown");
    assert_eq!(focused(cx), Some(9));
    cx.simulate_keystrokes("pageup down down down down down");
    assert_eq!(focused(cx), Some(8));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(workbench.read_with(cx, |workbench, _| workbench.context_menu.is_none() && workbench.explorer.edit.as_ref().is_some_and(|edit| edit.renamed.is_some())));
    cx.simulate_keystrokes("escape");

    // Below the rows: the folder's items; Escape hides the menu and gives the tree its focus back.
    let empty = point(px(120.), px(300.));
    cx.simulate_mouse_down(empty, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(menu_titles(&workbench, cx), ["New File...", "New Folder...", "---", "(Paste)"]);
    cx.simulate_keystrokes("escape");
    assert!(workbench.update_in(cx, |workbench, window, _| workbench.context_menu.is_none() && workbench.explorer.focus.is_focused(window)));

    // A click runs the item only 100ms after the menu showed; with Alt, Delete is Delete Permanently.
    // Items are 24px from the border and the 4px padding, the separators 11px.
    cx.simulate_mouse_down(file_row, MouseButton::Right, Modifiers::none());
    let top = workbench.read_with(cx, |workbench, _| f32::from(workbench.context_menu.as_ref().unwrap().origin.y));
    let delete = point(px(150.), px(top + 5. + 24. * 6. + 11. * 3. + 12.));
    cx.simulate_mouse_down(delete, MouseButton::Left, Modifiers::alt());
    cx.simulate_mouse_up(delete, MouseButton::Left, Modifiers::alt());
    assert!(workbench.read_with(cx, |workbench, _| workbench.context_menu.is_some()), "too early");
    cx.executor().advance_clock(std::time::Duration::from_millis(100));
    cx.simulate_mouse_down(delete, MouseButton::Left, Modifiers::alt());
    cx.simulate_mouse_up(delete, MouseButton::Left, Modifiers::alt());
    cx.run_until_parked();
    assert_eq!(names(&dir), ["sub"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// fileCommands.ts `copyPathCommandHandler` and `copyRelativePathCommandHandler` by their default keybindings,
/// one line per selected item, then with `explorer.copyRelativePathSeparator`.
#[gpui::test]
fn copy_path_copies_the_selected_items_paths(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("explorer-copy-path", cx);
    let clipboard = |cx: &mut VisualTestContext| cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
    select(&workbench, &dir.join("sub"), cx);
    cx.simulate_keystrokes("alt-cmd-c");
    assert_eq!(clipboard(cx), dir.join("sub").to_string_lossy());
    cx.simulate_keystrokes("shift-alt-cmd-c");
    assert_eq!(clipboard(cx), "sub");

    workbench.update(cx, |workbench, _| workbench.folder.as_mut().unwrap().click_select(&dir.join("a.txt"), true, false));
    cx.simulate_keystrokes("shift-alt-cmd-c");
    assert_eq!(clipboard(cx), "sub\na.txt");
    let user = std::env::temp_dir().join(format!("code-gpui-copy-path-user-{}", std::process::id()));
    let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), user.clone(), None);
    workbench.update(cx, |workbench, _| {
        *workbench.settings.lock().unwrap() = settings;
        workbench.settings.lock().unwrap().update_value_derived("explorer.copyRelativePathSeparator", Some(&serde_json::json!("\\"))).unwrap();
    });
    std::fs::create_dir_all(dir.join("sub/deep")).unwrap();
    workbench.update_in(cx, |workbench, window, cx| workbench.select_explorer_item(&dir.join("sub/deep"), window, cx));
    cx.simulate_keystrokes("shift-alt-cmd-c");
    assert_eq!(clipboard(cx), "sub\\deep");
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&user).unwrap();
}

/// `contributes.menus` `explorer/context` items show by the right-clicked row's resource keys.
#[gpui::test]
fn extension_items_follow_the_right_clicked_rows_resource(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("explorer-extension-menu", cx);
    let extension = exthost::scan::ScannedExtension {
        id: "test.text".into(),
        location: dir.clone(),
        description: serde_json::json!({ "contributes": {
            "commands": [{ "command": "text.preview", "title": "Open Text Preview" }],
            "menus": { "explorer/context": [{ "command": "text.preview", "group": "navigation", "when": "resourceExtname == .txt && resourceFilename == a.txt" }] }
        } }),
    };
    workbench.update(cx, |workbench, _| workbench.menus.extend(crate::menus::extension_menus(&[extension])));
    let file_row = point(px(120.), px(129.));
    cx.simulate_mouse_down(file_row, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(menu_titles(&workbench, cx), ["Open Text Preview", "Reveal in Finder", "---", "Cut", "Copy", "---", "Copy Path", "Copy Relative Path", "---", "Rename...", "Delete"]);
    // markdown.showPreview opens only a `vscode.Uri`, which the host revives from `$mid` 1.
    let resource = workbench.read_with(cx, |workbench, _| workbench.context_menu.as_ref().unwrap().args[0].clone());
    assert_eq!(resource["$mid"], 1);
    cx.simulate_keystrokes("escape");
    let folder_row = point(px(120.), px(107.));
    cx.simulate_mouse_down(folder_row, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
    assert!(!menu_titles(&workbench, cx).contains(&"Open Text Preview".to_string()));
    std::fs::remove_dir_all(&dir).unwrap();
}
#[gpui::test]
fn closing_the_window_stores_the_tree_view_state_for_the_next_open(cx: &mut TestAppContext) {
    let (dir, workbench, window_cx) = explorer("explorer-view-state", cx);
    std::fs::write(dir.join("sub/b.txt"), "b").unwrap();
    let storage = workbench.update(window_cx, |workbench, _| {
        workbench.folder.as_mut().unwrap().reveal(&dir.join("sub/b.txt"));
        workbench.workspace_storage.clone()
    });
    drop(workbench);
    window_cx.update(|window, _| window.remove_window());
    window_cx.run_until_parked();
    assert!(storage.lock().unwrap().get("workbench.explorer.treeViewState").is_some(), "stored when the workbench is released");

    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let theme = load_theme(&[scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap()], "Dark Modern").unwrap();
    let folder = dir.clone();
    let (reopened, window_cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(Workspace::folder(folder)), Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.restore_workspace_state(storage);
        workbench
    });
    let restored = reopened.read_with(window_cx, |workbench, _| {
        let tree = workbench.folder.as_ref().unwrap();
        (tree.is_expanded(&dir.join("sub")), tree.focused.clone())
    });
    assert_eq!(restored, (true, Some(dir.join("sub/b.txt"))));
    std::fs::remove_dir_all(&dir).unwrap();
}
#[gpui::test]
fn opening_a_file_reveals_it_unless_auto_reveal_is_off_or_excludes_it(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("explorer-auto-reveal", cx);
    std::fs::create_dir_all(dir.join("node_modules/lib")).unwrap();
    std::fs::write(dir.join("node_modules/lib/x.js"), "").unwrap();
    // More rows than the 1080px test window shows.
    for index in 0..150 {
        std::fs::write(dir.join(format!("sub/f{index:03}.txt")), "").unwrap();
    }
    let user = std::env::temp_dir().join(format!("code-gpui-auto-reveal-user-{}", std::process::id()));
    let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), user.clone(), None);
    workbench.update(cx, |workbench, _| *workbench.settings.lock().unwrap() = settings);
    let open = |path: PathBuf, cx: &mut VisualTestContext| {
        workbench.update_in(cx, |workbench, window, cx| workbench.open_file(path, true, window, cx));
        cx.run_until_parked();
    };

    open(dir.join("sub/f149.txt"), cx);
    assert_eq!(focused(&workbench, cx), Some(dir.join("sub/f149.txt")));
    let scroll_top = workbench.read_with(cx, |workbench, _| -workbench.explorer.scroll.offset().y);
    assert!(scroll_top > px(0.), "the row below the view scrolls in");

    workbench.update(cx, |workbench, _| workbench.settings.lock().unwrap().update_value_derived("explorer.autoReveal", Some(&serde_json::json!(false))).unwrap());
    open(dir.join("a.txt"), cx);
    assert_eq!(focused(&workbench, cx), Some(dir.join("sub/f149.txt")), "explorer.autoReveal false keeps the selection");

    workbench.update(cx, |workbench, _| workbench.settings.lock().unwrap().update_value_derived("explorer.autoReveal", Some(&serde_json::json!(true))).unwrap());
    open(dir.join("node_modules/lib/x.js"), cx);
    assert_eq!(focused(&workbench, cx), Some(dir.join("sub/f149.txt")), "the default explorer.autoRevealExclude has **/node_modules");
    assert!(!rows(&workbench, cx).contains(&"x.js".to_string()));
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&user).unwrap();
}


fn rows(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    workbench.read_with(cx, |workbench, _| workbench.folder.as_ref().unwrap().rows().into_iter().map(|row| row.name).collect())
}

fn focused(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Option<PathBuf> {
    workbench.read_with(cx, |workbench, _| workbench.folder.as_ref().unwrap().focused.clone())
}

/// Past `AsyncFindController`'s 250ms throttle, with the file search done.
fn settle_find(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(std::time::Duration::from_millis(300));
    cx.run_until_parked();
}

#[gpui::test]
fn find_filters_the_tree_by_a_file_search_and_restores_it_on_close(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("explorer-find", cx);
    std::fs::write(dir.join("sub/needle.rs"), "").unwrap();
    std::fs::write(dir.join("other.md"), "").unwrap();
    select(&workbench, &dir.join("a.txt"), cx);
    cx.simulate_keystrokes("alt-cmd-f");
    assert!(workbench.update_in(cx, |workbench, window, _| workbench.explorer.find.is_open() && workbench.explorer.find.focus.is_focused(window)));

    // Highlight mode keeps every row; no row's name matches, so the focus stays.
    cx.simulate_input("needle");
    settle_find(cx);
    assert_eq!(rows(&workbench, cx), ["sub", "a.txt", "other.md"]);
    assert_eq!(focused(&workbench, cx), Some(dir.join("a.txt")));

    run(&workbench, "list.toggleFindMode", cx);
    settle_find(cx);
    assert_eq!(rows(&workbench, cx), ["sub", "needle.rs"], "the filter shows the matches and their folders, expanded");
    assert_eq!(focused(&workbench, cx), Some(dir.join("sub/needle.rs")));

    cx.simulate_keystrokes("escape");
    settle_find(cx);
    assert!(workbench.update_in(cx, |workbench, window, _| !workbench.explorer.find.is_open() && workbench.explorer.focus.is_focused(window)));
    assert_eq!(rows(&workbench, cx), ["sub", "a.txt", "other.md"], "the session's view state comes back");
    assert_eq!(focused(&workbench, cx), Some(dir.join("a.txt")));

    cx.simulate_keystrokes("alt-cmd-f");
    settle_find(cx);
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.explorer.find.input.value.clone()), "needle", "the widget reopens with the last pattern");
    assert_eq!(rows(&workbench, cx), ["sub", "needle.rs"]);
    std::fs::remove_dir_all(&dir).unwrap();
}
