use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};

use super::LOCAL_HISTORY_SCHEME;
use crate::editor::uri_string;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::local_history;
use crate::theme::load_theme;
use crate::workbench::Workbench;
use crate::workbench::readonly::Readonly;

/// A folder holding `notes.txt`, opened in a workbench, and the file's path.
fn open<'a>(cx: &'a mut TestAppContext, name: &str) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext, PathBuf) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-local-history-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("notes.txt");
    std::fs::write(&file, "saved").unwrap();
    let workspace = config::workspace::Workspace::folder(dir.clone());
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(workspace.clone()))));
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.watch_editor_focus(window, cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("User/keybindings.json"), cx);
        workbench.open_file(file.clone(), true, window, cx);
        workbench
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (workbench, cx, file)
}

fn run(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, command: &str) {
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command(command, vec![], window, cx));
    cx.run_until_parked();
}

fn history_home(file: &Path) -> PathBuf {
    file.parent().unwrap().join("User/History")
}

#[gpui::test]
fn create_entry_then_find_it_and_open_its_contents_read_only(cx: &mut TestAppContext) {
    let (workbench, cx, file) = open(cx, "find");
    run(&workbench, cx, "workbench.action.localHistory.create");
    cx.simulate_input("Before refactor");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let folder = local_history::entries_folder(&history_home(&file), &uri_string("file", &file));
    let entries = local_history::read_entries(&folder, &file).unwrap();
    assert_eq!(entries.iter().map(|entry| entry.source.as_deref()).collect::<Vec<_>>(), [Some("Before refactor")]);

    std::fs::write(&file, "changed").unwrap();
    run(&workbench, cx, "workbench.action.localHistory.restoreViaPicker");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let label = format!("notes.txt (Before refactor \u{2022} {})", local_history::date_label(entries[0].timestamp));
    workbench.read_with(cx, |workbench, _| {
        let editor = workbench.editors.active().unwrap();
        assert_eq!(editor.document.scheme, LOCAL_HISTORY_SCHEME);
        assert_eq!(editor.document.lines, ["saved"]);
        assert_eq!(workbench.editor_name(editor), label);
        assert_eq!(workbench.readonly(&editor.document.scheme, &editor.document.path), Some(Readonly::Provider));
    });
    std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
}
