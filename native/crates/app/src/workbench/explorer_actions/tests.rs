use std::path::{Path, PathBuf};

use config::workspace::Workspace;
use exthost::scan::scan_extension;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};

use crate::explorer::FileTree;
use crate::host_sync::HostSync;
use crate::languages::Languages;
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
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(FileTree::new(Workspace::folder(folder.clone()))), Languages::new(&[]), crate::default_settings(), host, cx);
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

    select(&workbench, &dir.join("c.txt"), cx);
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
