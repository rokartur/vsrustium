use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use config::registry::Registry;
use config::service::Settings;
use config::workspace::Workspace;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::explorer::FileTree;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;
use crate::workbench::search_view::{MessagePart, RowKey, SearchField, SideBarView};

fn search<'a>(name: &str, cx: &'a mut TestAppContext) -> (PathBuf, gpui::Entity<Workbench>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::create_dir_all(dir.join("User")).unwrap();
    std::fs::write(dir.join("a.txt"), "alpha beta\nAlpha\n").unwrap();
    std::fs::write(dir.join("sub/b.txt"), "alphabet alpha\n").unwrap();
    let host = HostSync::new(std::sync::mpsc::channel().0);
    let folder = dir.clone();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(FileTree::new(Workspace::folder(folder.clone()))), Languages::new(&[]), settings.clone(), host, cx);
        workbench.skip_dialogs = true;
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), folder.join("keybindings.json"), cx);
        workbench
    });
    cx.run_until_parked();
    (dir, workbench, cx)
}

fn run(workbench: &gpui::Entity<Workbench>, command: &str, args: Vec<Value>, cx: &mut VisualTestContext) {
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.execute_command(command, args, window, cx);
    });
    cx.run_until_parked();
}

fn message(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    workbench.read_with(cx, |workbench, _| {
        let parts = workbench.search.message.clone().unwrap_or_default();
        parts
            .into_iter()
            .map(|part| match part {
                MessagePart::Text(text) => text,
                MessagePart::Link(link) => format!("[{}]", link.label()),
            })
            .collect()
    })
}

fn result_files(workbench: &gpui::Entity<Workbench>, dir: &Path, cx: &mut VisualTestContext) -> Vec<(String, usize)> {
    workbench.read_with(cx, |workbench, _| {
        workbench.search.result.files.iter().map(|file| (file.path.strip_prefix(dir).unwrap().to_string_lossy().into_owned(), file.matches.len())).collect()
    })
}

#[gpui::test]
fn find_in_files_searches_the_folder_with_the_options(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = search("search-find", cx);
    run(&workbench, "workbench.action.findInFiles", vec![json!({ "query": "alpha", "triggerSearch": true })], cx);
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.side_bar_view), SideBarView::Search);
    assert_eq!(result_files(&workbench, &dir, cx), [("a.txt".to_string(), 2), ("sub/b.txt".to_string(), 2)]);
    assert_eq!(message(&workbench, cx), "4 results in 2 files - [Open in editor]");

    run(&workbench, "toggleSearchCaseSensitive", Vec::new(), cx);
    assert_eq!(result_files(&workbench, &dir, cx), [("a.txt".to_string(), 1), ("sub/b.txt".to_string(), 2)]);
    run(&workbench, "toggleSearchWholeWord", Vec::new(), cx);
    assert_eq!(result_files(&workbench, &dir, cx), [("a.txt".to_string(), 1), ("sub/b.txt".to_string(), 1)]);

    run(&workbench, "workbench.action.findInFiles", vec![json!({ "query": "nothing here", "triggerSearch": true, "filesToInclude": "sub" })], cx);
    assert_eq!(message(&workbench, cx), "No results found in 'sub' - [Search again in all files]");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn typing_searches_after_the_debounce_and_an_invalid_regex_does_not(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = search("search-type", cx);
    cx.simulate_keystrokes("cmd-shift-f");
    cx.simulate_input("beta");
    assert!(workbench.read_with(cx, |workbench, _| workbench.search.result.is_empty()), "the search waits for the debounce");
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
    assert_eq!(result_files(&workbench, &dir, cx), [("a.txt".to_string(), 1)]);

    cx.simulate_keystrokes("alt-cmd-r");
    cx.simulate_input("(");
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
    let validation = workbench.read_with(cx, |workbench, _| workbench.search.validation.clone());
    assert!(validation.is_some_and(|message| message.starts_with("Invalid regular expression: /beta(/")));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn replace_all_writes_every_file_and_clears_the_results(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = search("search-replace-all", cx);
    run(&workbench, "workbench.action.findInFiles", vec![json!({ "query": "alpha", "replace": "omega", "isCaseSensitive": true, "matchWholeWord": true, "triggerSearch": true })], cx);
    run(&workbench, "search.action.replaceAll", Vec::new(), cx);
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "omega beta\nAlpha\n");
    assert_eq!(std::fs::read_to_string(dir.join("sub/b.txt")).unwrap(), "alphabet omega\n");
    assert!(workbench.read_with(cx, |workbench, _| workbench.search.result.is_empty()));
    assert_eq!(message(&workbench, cx), "Replaced 2 occurrences across 2 files with 'omega'.");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn replacing_one_match_keeps_the_others_and_dismiss_drops_a_file(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = search("search-replace-one", cx);
    run(&workbench, "workbench.action.findInFiles", vec![json!({ "query": "alpha", "replace": "Omega", "preserveCase": true, "triggerSearch": true })], cx);
    let second = RowKey::Match(dir.join("a.txt"), [1, 0, 1, 5]);
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.search.focused = Some(second.clone());
        window.focus(&workbench.search.focus.clone(), cx);
    });
    cx.simulate_keystrokes("shift-cmd-1");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "alpha beta\nOmega\n");
    assert_eq!(result_files(&workbench, &dir, cx), [("a.txt".to_string(), 1), ("sub/b.txt".to_string(), 2)]);

    workbench.update_in(cx, |workbench, _, _| workbench.search.focused = Some(RowKey::File(dir.join("sub/b.txt"))));
    cx.simulate_keystrokes("cmd-backspace");
    assert_eq!(result_files(&workbench, &dir, cx), [("a.txt".to_string(), 1)]);
    assert_eq!(message(&workbench, cx), "1 result in 1 file - [Open in editor]");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn opening_a_result_selects_the_match_in_the_editor(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = search("search-open", cx);
    run(&workbench, "workbench.action.findInFiles", vec![json!({ "query": "beta", "triggerSearch": true })], cx);
    run(&workbench, "search.action.focusSearchList", Vec::new(), cx);
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    let selection = workbench.read_with(cx, |workbench, _| {
        let editor = workbench.editors.active().unwrap();
        (editor.document.path.clone(), editor.selection().start(), editor.selection().end())
    });
    assert_eq!(selection.0, dir.join("a.txt"));
    assert_eq!((selection.1.line, selection.1.column, selection.2.line, selection.2.column), (0, 6, 0, 10));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn find_in_files_seeds_the_query_from_the_editor_word(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = search("search-seed", cx);
    workbench.update_in(cx, |workbench, window, cx| workbench.open_file(dir.join("a.txt"), true, window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-shift-f");
    let query = workbench.read_with(cx, |workbench, _| workbench.search.value(SearchField::Query).to_string());
    assert_eq!(query, "", "search.seedWithNearestWord is off by default");
    workbench.update(cx, |workbench, _| workbench.settings.lock().unwrap().update_value_derived("search.seedWithNearestWord", Some(&json!(true))).unwrap());
    cx.simulate_keystrokes("cmd-shift-f");
    let query = workbench.read_with(cx, |workbench, _| workbench.search.value(SearchField::Query).to_string());
    assert_eq!(query, "", "nothing is seeded while the focus is in the Search view");
    workbench.update_in(cx, |workbench, window, cx| workbench.open_file(dir.join("a.txt"), true, window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-shift-f");
    let query = workbench.read_with(cx, |workbench, _| workbench.search.value(SearchField::Query).to_string());
    assert_eq!(query, "alpha");
    std::fs::remove_dir_all(dir).unwrap();
}
