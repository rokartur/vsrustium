use std::path::{Path, PathBuf};

use config::workspace::Workspace;
use exthost::scan::scan_extension;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::menus::MenuAction;
use crate::theme::load_theme;
use crate::workbench::Workbench;
use crate::workbench::view_panes::FILE_VIEW;

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
    let opened = workbench.read_with(cx, |workbench, _| workbench.editors.active().map(|editor| editor.document().path.clone()));
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
    let (sub_row, third_row) = (point(px(120.), px(107.)), point(px(120.), px(151.)));

    drop(&["dropped.txt"], sub_row, cx);
    assert_eq!(names(&dir.join("sub")), ["dropped.txt"]);
    assert_eq!(names(&finder), ["a.txt", "dropped.txt", "folder"], "a drop copies");
    let opened = workbench.read_with(cx, |workbench, _| workbench.editors.active().map(|editor| editor.document().path.clone()));
    assert_eq!(opened, Some(dir.join("sub/dropped.txt")), "explorer.autoOpenDroppedFile");

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
        menu.levels[0].actions
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
    assert_eq!(menu_titles(&workbench, cx), ["Reveal in Finder", "Open in Integrated Terminal", "---", "Select for Compare", "---", "Open Timeline", "---", "Cut", "Copy", "---", "Copy Path", "Copy Relative Path", "---", "Rename...", "Delete"]);
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.folder.as_ref().unwrap().selected.clone()), [dir.join("a.txt")]);

    // The pointer focuses the item under it; Down goes on from there and skips the separator; Enter runs it.
    let focused = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.context_menu.as_ref().unwrap().levels[0].focused);
    cx.simulate_mouse_move(point(px(150.), px(148.)), None, Modifiers::none());
    assert_eq!(focused(cx), Some(0));
    cx.simulate_mouse_move(point(px(150.), px(700.)), None, Modifiers::none());
    assert_eq!(focused(cx), None);
    cx.simulate_mouse_move(point(px(150.), px(148.)), None, Modifiers::none());
    cx.simulate_keystrokes("pagedown");
    assert_eq!(focused(cx), Some(14));
    cx.simulate_keystrokes("pageup down down down down down down down down");
    assert_eq!(focused(cx), Some(13));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(workbench.read_with(cx, |workbench, _| workbench.context_menu.is_none() && workbench.explorer.edit.as_ref().is_some_and(|edit| edit.renamed.is_some())));
    cx.simulate_keystrokes("escape");

    // Below the rows: the folder's items; Escape hides the menu and gives the tree its focus back.
    let empty = point(px(120.), px(300.));
    cx.simulate_mouse_move(empty, None, Modifiers::none());
    cx.simulate_mouse_down(empty, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
    // As the reference's (port/fixtures/ui-pair/m12/workspace-folders.json), less the commands the port lacks; imageCarousel.contribution.ts adds Open in Images Preview (navigation, order 25).
    let below = ["New File...", "New Folder...", "Reveal in Finder", "Open in Images Preview", "Open in Integrated Terminal", "---", "Add Folder to Workspace...", "Open Folder Settings", "Remove Folder from Workspace", "---", "Find in Folder...", "---", "Open Timeline", "---", "(Paste)", "---", "Copy Path", "Copy Relative Path"];
    assert_eq!(menu_titles(&workbench, cx), below);
    // The menu takes the focus and `.context-view-block` the pane's hover: the title actions hide.
    assert!(workbench.update_in(cx, |workbench, window, _| !workbench.explorer_title_actions_shown(window)));
    assert!(workbench.update_in(cx, |workbench, window, _| !workbench.pane_hovered(FILE_VIEW, window)));
    cx.simulate_keystrokes("escape");
    assert!(workbench.update_in(cx, |workbench, window, _| workbench.context_menu.is_none() && workbench.explorer.focus.is_focused(window)));
    // A key leaves `:hover` as it is: the header actions stay under the pointer (the reference with a quick pick open).
    assert!(workbench.update_in(cx, |workbench, window, _| workbench.pane_hovered(FILE_VIEW, window)));

    // A click runs the item only 100ms after the menu showed; with Alt, Delete is Delete Permanently.
    // Items are 24px from the border and the 4px padding, the separators 11px.
    cx.simulate_mouse_down(file_row, MouseButton::Right, Modifiers::none());
    let top = workbench.read_with(cx, |workbench, _| f32::from(workbench.context_menu.as_ref().unwrap().levels[0].origin.y));
    let delete = point(px(150.), px(top + 5. + 24. * 9. + 11. * 5. + 12.));
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

/// menuEntryActionViewItem.test.ts 'Alt at open: false, Alt at activation: true': Alt+Enter runs the focused item's
/// alt command, Delete Permanently for Delete.
#[gpui::test]
fn alt_enter_runs_the_alt_command_of_the_focused_item(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("explorer-context-menu-alt-enter", cx);
    let file_row = point(px(120.), px(129.));
    cx.simulate_mouse_down(file_row, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(file_row, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
    cx.simulate_keystrokes("end shift-enter");
    assert!(workbench.read_with(cx, |workbench, _| workbench.context_menu.is_some()), "Shift is no alt key on macOS");
    cx.simulate_keystrokes("alt-enter");
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
    assert_eq!(menu_titles(&workbench, cx), ["Open Text Preview", "Reveal in Finder", "Open in Integrated Terminal", "---", "Select for Compare", "---", "Open Timeline", "---", "Cut", "Copy", "---", "Copy Path", "Copy Relative Path", "---", "Rename...", "Delete"]);
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

#[gpui::test]
fn view_title_and_file_actions_collapse_reveal_and_copy_the_active_file(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("explorer-title-actions", cx);
    std::fs::create_dir_all(dir.join("sub/inner")).unwrap();
    std::fs::write(dir.join("sub/inner/x.txt"), "").unwrap();
    std::fs::write(dir.join("sub/y.txt"), "").unwrap();
    let shows_title_actions = |cx: &mut VisualTestContext| workbench.update_in(cx, |workbench, window, _| workbench.explorer_title_actions_shown(window));
    assert!(shows_title_actions(cx), "the focused pane shows its title actions");

    workbench.update_in(cx, |workbench, window, cx| workbench.select_explorer_item(&dir.join("sub/inner/x.txt"), window, cx));
    assert_eq!(rows(&workbench, cx), ["sub", "inner", "x.txt", "y.txt", "a.txt"]);
    run(&workbench, "workbench.files.action.collapseExplorerFolders", cx);
    assert_eq!(rows(&workbench, cx), ["sub", "a.txt"]);

    let user = std::env::temp_dir().join(format!("code-gpui-title-actions-user-{}", std::process::id()));
    let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), user.clone(), None);
    workbench.update(cx, |workbench, _| {
        *workbench.settings.lock().unwrap() = settings;
        workbench.settings.lock().unwrap().update_value_derived("explorer.autoReveal", Some(&serde_json::json!(false))).unwrap();
    });
    workbench.update_in(cx, |workbench, window, cx| workbench.open_file(dir.join("sub/inner/x.txt"), true, window, cx));
    cx.run_until_parked();
    assert!(!shows_title_actions(cx), "with the focus in the editor and no hover the actions hide");
    assert_eq!(rows(&workbench, cx), ["sub", "a.txt"], "explorer.autoReveal false leaves the tree");
    run(&workbench, "workbench.files.action.showActiveFileInExplorer", cx);
    assert_eq!(focused(&workbench, cx), Some(dir.join("sub/inner/x.txt")));
    assert!(workbench.update_in(cx, |workbench, window, _| workbench.explorer.focus.is_focused(window)), "revealInExplorer focuses the tree");

    workbench.update_in(cx, |workbench, window, cx| {
        workbench.execute_command("revealInExplorer", vec![serde_json::json!({ "scheme": "file", "path": "/outside/file.txt" })], window, cx);
    });
    assert_eq!(focused(&workbench, cx), Some(dir.join("sub/inner/x.txt")), "a resource outside the workspace is not revealed");

    select(&workbench, &dir.join("sub"), cx);
    run(&workbench, "workbench.action.files.copyPathOfActiveFile", cx);
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).unwrap(), dir.join("sub/inner/x.txt").to_string_lossy(), "the active editor's path, not the selection's");
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&user).unwrap();
}

#[gpui::test]
fn keys_move_the_focus_expand_collapse_select_and_open(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("explorer-keys", cx);
    std::fs::write(dir.join("sub/b.txt"), "b").unwrap();
    let selected = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.folder.as_ref().unwrap().selected.clone());

    cx.simulate_keystrokes("down");
    assert_eq!(focused(&workbench, cx), Some(dir.join("sub")), "the first down focuses the first row");
    cx.simulate_keystrokes("right");
    assert_eq!(rows(&workbench, cx), ["sub", "b.txt", "a.txt"]);
    cx.simulate_keystrokes("right");
    assert_eq!(focused(&workbench, cx), Some(dir.join("sub/b.txt")), "right on an expanded folder goes to its first child");
    cx.simulate_keystrokes("left");
    assert_eq!(focused(&workbench, cx), Some(dir.join("sub")), "left on a file goes to its parent");
    cx.simulate_keystrokes("left");
    assert_eq!(rows(&workbench, cx), ["sub", "a.txt"]);
    cx.simulate_keystrokes("space");
    assert_eq!(rows(&workbench, cx), ["sub", "b.txt", "a.txt"], "space toggles a folder");

    cx.simulate_keystrokes("end shift-up");
    assert_eq!(focused(&workbench, cx), Some(dir.join("sub/b.txt")));
    assert_eq!(selected(cx), [dir.join("sub/b.txt")], "shift-up adds the row it moves to");
    cx.simulate_keystrokes("home");
    cx.simulate_keystrokes("cmd-a");
    assert_eq!(selected(cx), [dir.join("sub"), dir.join("sub/b.txt"), dir.join("a.txt")]);
    cx.simulate_keystrokes("escape");
    assert_eq!(selected(cx), Vec::<PathBuf>::new(), "escape clears a multi-selection and keeps the focus");
    assert_eq!(focused(&workbench, cx), Some(dir.join("sub")));

    cx.simulate_keystrokes("end space");
    let active = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().map(|editor| editor.document().path.clone()));
    assert_eq!(active(cx), Some(dir.join("a.txt")), "space opens a file");
    assert!(workbench.update_in(cx, |workbench, window, _| workbench.explorer.focus.is_focused(window)), "space keeps the focus");
    cx.simulate_keystrokes("up cmd-down");
    assert_eq!(active(cx), Some(dir.join("sub/b.txt")), "cmd-down opens a file and passes the focus");
    assert!(workbench.update_in(cx, |workbench, window, _| workbench.editor_focus.is_focused(window)));

    workbench.update_in(cx, |workbench, window, cx| window.focus(&workbench.explorer.focus.clone(), cx));
    cx.simulate_keystrokes("cmd-left");
    assert_eq!(rows(&workbench, cx), ["sub", "a.txt"], "cmd-left collapses every folder");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[gpui::test]
fn page_keys_move_the_focus_by_the_rows_in_view(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("explorer-page-keys", cx);
    for index in 0..150 {
        std::fs::write(dir.join(format!("f{index:03}.txt")), "").unwrap();
    }
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let page = workbench.read_with(cx, |workbench, _| (f32::from(workbench.explorer.scroll.bounds().size.height) / 22.).floor() as usize);
    let focused_index = |cx: &mut VisualTestContext| {
        let path = focused(&workbench, cx).unwrap();
        workbench.read_with(cx, |workbench, _| workbench.folder.as_ref().unwrap().rows().iter().position(|row| row.path == path).unwrap())
    };

    cx.simulate_keystrokes("pagedown");
    assert_eq!(focused_index(cx), page - 1, "the first page down focuses the page's last row");
    cx.simulate_keystrokes("pagedown");
    assert!(focused_index(cx) > page, "the next one scrolls a page");
    let scroll_top = workbench.read_with(cx, |workbench, _| -workbench.explorer.scroll.offset().y);
    assert!(scroll_top > px(0.));
    cx.simulate_keystrokes("end");
    assert_eq!(focused_index(cx), 151);
    cx.simulate_keystrokes("home");
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.explorer.scroll.offset().y), px(0.), "home scrolls the first row in");
    std::fs::remove_dir_all(&dir).unwrap();
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

#[gpui::test]
fn new_file_creates_the_picked_file_next_to_the_last_active_file(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("explorer-new-file", cx);
    workbench.update_in(cx, |workbench, window, cx| workbench.open_file(dir.join("a.txt"), true, window, cx));
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.execute_command("workbench.action.files.newFile", vec![serde_json::json!({ "fileName": "b.md" })], window, cx);
    });
    cx.run_until_parked();
    let created = dir.join("sub/new.md");
    cx.simulate_new_path_selection(|directory| {
        assert_eq!(directory, dir.as_path(), "the dialog opens in the folder of the last active file");
        Some(created.clone())
    });
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&created).unwrap(), "");
    let active = workbench.read_with(cx, |workbench, _| workbench.editors.active().map(|editor| (editor.document().path.clone(), editor.preview)));
    assert_eq!(active, Some((created, false)), "the new file opens pinned");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// editorPlaceholder.ts: binary, too large and missing files open as placeholders whose actions resolve them.
#[gpui::test]
fn files_that_do_not_resolve_open_editor_placeholders_with_their_actions(cx: &mut TestAppContext) {
    use crate::editor::OpenError;
    let (dir, workbench, cx) = explorer("explorer-placeholders", cx);
    let active = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().map(|editor| (editor.document().open_error.clone(), editor.document().language_id.clone(), editor.document().lines.len())).unwrap());

    std::fs::write(dir.join("a.bin"), [0x89, 0, 1, 2, 0, 0, b'\n', b'x']).unwrap();
    workbench.update_in(cx, |workbench, window, cx| workbench.open_file(dir.join("a.bin"), true, window, cx));
    assert_eq!(active(cx), (Some(OpenError::Binary(8)), "plaintext".into(), 1));
    // Open Anyway, then the Open With picker's Text Editor: the file opens as text in the Binary language.
    workbench.update_in(cx, |workbench, window, cx| {
        let uri = workbench.editors.active().unwrap().document().uri();
        drop(workbench.reopen_with(&uri, "default", "default", false, window, cx));
    });
    assert_eq!(active(cx), (None, "code-text-binary".into(), 2));

    workbench.update(cx, |workbench, _| {
        let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), Some(Workspace::folder(dir.clone())));
        workbench.settings = std::sync::Arc::new(std::sync::Mutex::new(settings));
        workbench.update_setting("workbench.editorLargeFileConfirmation", serde_json::json!(1));
    });
    std::fs::write(dir.join("large.txt"), vec![b'a'; 1024 * 1024 + 1]).unwrap();
    workbench.update_in(cx, |workbench, window, cx| workbench.open_file(dir.join("large.txt"), true, window, cx));
    assert_eq!(active(cx).0, Some(OpenError::TooLarge(1024 * 1024 + 1)));
    workbench.update_in(cx, |workbench, window, cx| workbench.run_placeholder_action("Open Anyway", window, cx));
    assert_eq!(active(cx), (None, "plaintext".into(), 1));

    workbench.update_in(cx, |workbench, window, cx| workbench.open_file(dir.join("missing.txt"), true, window, cx));
    assert_eq!(active(cx).0, Some(OpenError::NotFound));
    workbench.update_in(cx, |workbench, window, cx| workbench.run_placeholder_action("Create File", window, cx));
    assert!(dir.join("missing.txt").exists());
    assert_eq!(active(cx).0, None);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn tree_indent_is_clamped_and_defaults_to_8() {
    use serde_json::json;
    let indents: Vec<f32> = [json!(null), json!("12"), json!(12), json!(100), json!(-3)].iter().map(super::tree_indent).collect();
    assert_eq!(indents, [8., 8., 12., 40., 0.]);
}

/// listService.ts: `workbench.list.openMode` and `workbench.tree.expandMode` `doubleClick` need a double click to
/// open a file or expand a folder; `workbench.list.multiSelectModifier` `alt` adds rows with Alt instead of Cmd.
#[gpui::test]
fn list_click_settings_change_what_clicks_do(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("list-click-settings", cx);
    let user = dir.join("../list-click-settings-user");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("settings.json"), r#"{ "workbench.list.openMode": "doubleClick", "workbench.tree.expandMode": "doubleClick", "workbench.list.multiSelectModifier": "alt" }"#).unwrap();
    workbench.update(cx, |workbench, _| {
        workbench.settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), user.clone(), None)));
    });
    let (sub_row, file_row) = (point(px(120.), px(107.)), point(px(120.), px(129.)));
    let state = |cx: &mut VisualTestContext| {
        workbench.read_with(cx, |workbench, _| {
            let tree = workbench.folder.as_ref().unwrap();
            (workbench.editors.active().is_some(), tree.is_expanded(&dir.join("sub")), tree.selected.len())
        })
    };

    cx.simulate_click(file_row, Modifiers::none());
    assert_eq!(state(cx), (false, false, 1), "a single click only selects the file");
    cx.simulate_click(sub_row, Modifiers::none());
    assert_eq!(state(cx), (false, false, 1), "a single click only selects the folder");
    cx.simulate_click(file_row, Modifiers::alt());
    assert_eq!(state(cx).2, 2, "Alt adds the file to the selection");
    cx.simulate_click(sub_row, Modifiers::none());
    cx.simulate_event(gpui::MouseDownEvent { position: sub_row, modifiers: Modifiers::none(), button: MouseButton::Left, click_count: 2, first_mouse: false });
    cx.simulate_event(gpui::MouseUpEvent { position: sub_row, modifiers: Modifiers::none(), button: MouseButton::Left, click_count: 2 });
    assert!(state(cx).1, "a double click expands the folder");
    std::fs::remove_dir_all(user).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

/// Type navigation in the Explorer: a letter focuses the next row starting with it, without selecting it.
#[gpui::test]
fn typing_a_letter_focuses_the_matching_row(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("type-navigation", cx);
    select(&workbench, &dir.join("a.txt"), cx);
    let focused = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.folder.as_ref().unwrap().focused.clone());
    cx.simulate_keystrokes("s");
    assert_eq!(focused(cx), Some(dir.join("sub")));
    cx.simulate_keystrokes("x");
    assert_eq!(focused(cx), Some(dir.join("sub")), "\"sx\" matches no row, the focus stays");
    cx.executor().advance_clock(std::time::Duration::from_millis(800));
    cx.run_until_parked();
    cx.simulate_keystrokes("a");
    assert_eq!(focused(cx), Some(dir.join("a.txt")), "after 800ms a new word starts");
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.folder.as_ref().unwrap().selected.clone()), [dir.join("a.txt")], "the selection stays");
    std::fs::remove_dir_all(dir).unwrap();
}

/// style.css `.hc-black .monaco-list:not(.element-focused):focus:before`: the Explorer focused by its view command
/// has no focused row, so high contrast outlines the list.
#[gpui::test]
fn the_focused_explorer_without_a_focused_row_is_outlined_in_high_contrast(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("explorer-hc-outline", cx);
    workbench.update_in(cx, |workbench, window, cx| {
        let themes = scan_extension(&workbench.app_root.join("extensions/theme-defaults"), true).unwrap();
        workbench.theme = load_theme(&[themes], "Default High Contrast").unwrap();
        drop(workbench.execute_command("workbench.view.explorer", Vec::new(), window, cx));
    });
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, _| {
        let focused_row = workbench.folder.as_ref().unwrap().focused.clone();
        assert_eq!(focused_row, None);
        assert!(workbench.high_contrast_list_outline(workbench.explorer.focus.is_focused(window), focused_row.is_some()).is_some());
    });
    std::fs::remove_dir_all(dir).unwrap();
}

/// searchActionsFind.ts `FindInFolderAction` by its Shift+Alt+F key: the Search view opens with the folder in
/// "files to include".
#[gpui::test]
fn find_in_folder_searches_the_selected_folder(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = explorer("explorer-find-in-folder", cx);
    select(&workbench, &dir.join("sub"), cx);
    cx.simulate_keystrokes("shift-alt-f");
    cx.run_until_parked();
    workbench.read_with(cx, |workbench, _| {
        assert_eq!(workbench.side_bar_view, crate::workbench::search_view::SideBarView::Search);
        assert!(workbench.search.details_visible);
        assert_eq!(workbench.search.value(crate::workbench::search_view::SearchField::Include), "./sub");
    });
    std::fs::remove_dir_all(dir).unwrap();
}
