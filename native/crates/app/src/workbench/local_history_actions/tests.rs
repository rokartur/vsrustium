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
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
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

/// Creates one entry named `source` through Create Entry and returns the file's entries.
fn create_entry(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, file: &Path, source: &str) -> Vec<local_history::Entry> {
    run(workbench, cx, "workbench.action.localHistory.create");
    cx.simulate_input(source);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let folder = local_history::entries_folder(&history_home(file), &uri_string("file", file));
    local_history::read_entries(&folder, file).unwrap()
}

#[gpui::test]
fn find_entry_to_restore_compares_the_entry_with_the_file(cx: &mut TestAppContext) {
    let (workbench, cx, file) = open(cx, "find");
    let entries = create_entry(&workbench, cx, &file, "Before refactor");
    assert_eq!(entries.iter().map(|entry| entry.source.as_deref()).collect::<Vec<_>>(), [Some("Before refactor")]);

    std::fs::write(&file, "changed").unwrap();
    run(&workbench, cx, "workbench.action.localHistory.restoreViaPicker");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let date = local_history::date_label(entries[0].timestamp);
    workbench.read_with(cx, |workbench, _| {
        let diff = workbench.editors.active_diff().expect("a diff editor");
        assert_eq!(diff.label.as_deref(), Some(&*format!("notes.txt (Before refactor \u{2022} {date}) \u{2194} notes.txt")));
        assert_eq!(diff.original.scheme, LOCAL_HISTORY_SCHEME);
        assert_eq!(diff.modified.scheme, "file");
        let original = workbench.editors.model(LOCAL_HISTORY_SCHEME, &diff.original.path).unwrap();
        assert_eq!(original.document().lines, ["saved"]);
        assert_eq!(original.document().name(), "notes.txt");
        assert_eq!(workbench.readonly(LOCAL_HISTORY_SCHEME, &diff.original.path), Some(Readonly::Provider));
    });
    std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
}

#[gpui::test]
fn the_timeline_lists_the_entries_and_their_commands_run(cx: &mut TestAppContext) {
    let (workbench, cx, file) = open(cx, "timeline");
    create_entry(&workbench, cx, &file, "First");
    std::fs::write(&file, "second").unwrap();
    let entries = create_entry(&workbench, cx, &file, "Second");
    let uri = crate::editor::uri_value("file", &file);
    let timeline = workbench.read_with(cx, |workbench, _| workbench.local_history_timeline(&uri));
    let labels: Vec<_> = timeline.items.iter().map(|item| (item.label.as_str(), item.context_value.as_deref(), item.handle.as_str())).collect();
    // timelineService.ts sorts newest first.
    assert_eq!(labels, [("Second", Some("localHistory:item"), &*entries[1].id), ("First", Some("localHistory:item"), &*entries[0].id)]);
    assert_eq!(timeline.items[0].command.as_ref().unwrap()["id"], "_workbench.diff");
    assert_eq!(timeline.items[0].theme_icon, Some(("circle-outline".into(), None)));

    let item = |handle: &str| serde_json::json!({ "handle": handle, "source": "timeline.localHistory", "uri": uri });
    let second = item(&entries[1].id);
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.localHistory.compareWithPrevious", vec![second.clone()], window, cx)));
    cx.run_until_parked();
    workbench.read_with(cx, |workbench, _| {
        let diff = workbench.editors.active_diff().expect("a diff editor");
        let text = |path| workbench.editors.model(LOCAL_HISTORY_SCHEME, path).unwrap().document().lines.clone();
        assert_eq!((text(&diff.original.path), text(&diff.modified.path)), (vec!["saved".to_string()], vec!["second".to_string()]));
    });

    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.localHistory.rename", vec![second.clone()], window, cx)));
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("Renamed");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let folder = local_history::entries_folder(&history_home(&file), &uri_string("file", &file));
    let sources = || local_history::read_entries(&folder, &file).unwrap().into_iter().map(|entry| entry.source.unwrap_or_default()).collect::<Vec<_>>();
    assert_eq!(sources(), ["First", "Renamed"]);
    std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
}
