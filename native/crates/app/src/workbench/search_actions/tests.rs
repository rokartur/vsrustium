use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use config::registry::Registry;
use config::service::Settings;
use config::workspace::Workspace;
use exthost::scan::scan_extension;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use serde_json::{Value, json};

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::menus::MenuAction;
use crate::theme::load_theme;
use crate::workbench::Workbench;
use crate::workbench::search_view::{MessagePart, RowKey, SearchField, SideBarView, keep_before_ellipsis};

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
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(Workspace::folder(folder.clone())), Languages::new(&[]), settings.clone(), host, cx);
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

fn menu_titles(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    workbench.read_with(cx, |workbench, _| {
        let menu = workbench.context_menu.as_ref().expect("a context menu shows");
        menu.actions
            .iter()
            .map(|action| match action {
                MenuAction::Command { title, .. } => title.clone(),
                _ => "---".into(),
            })
            .collect()
    })
}

#[gpui::test]
fn the_context_menu_focuses_the_row_and_its_items_change_the_inputs(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = search("search-context-menu", cx);
    run(&workbench, "workbench.action.findInFiles", vec![json!({ "query": "alpha", "triggerSearch": true })], cx);
    let file = RowKey::File(dir.join("sub/b.txt"));
    workbench.update_in(cx, |workbench, window, cx| workbench.show_search_context_menu(&file, point(px(100.), px(200.)), window, cx));
    cx.run_until_parked();
    let titles = ["Dismiss", "Exclude File Type from Search", "Include File Type from Search", "---", "Copy", "Copy Path", "Copy All", "---", "Reveal in Explorer View"];
    assert_eq!(menu_titles(&workbench, cx), titles);
    assert_eq!(workbench.read_with(cx, |workbench, _| (workbench.search.focused.clone(), workbench.search.selected.clone())), (Some(file), None));

    run(&workbench, "search.action.copyAll", Vec::new(), cx);
    let copied = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
    assert_eq!(copied.lines().filter(|line| line.contains(": ")).count(), 4);
    run(&workbench, "search.action.excludeFileTypeFromSearch", Vec::new(), cx);
    run(&workbench, "search.action.excludeFileTypeFromSearch", Vec::new(), cx);
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.search.value(SearchField::Exclude).to_string()), "*.txt");
    assert!(workbench.read_with(cx, |workbench, _| workbench.search.result.is_empty() && workbench.search.details_visible));

    run(&workbench, "workbench.action.findInFiles", vec![json!({ "query": "alpha", "filesToExclude": "", "triggerSearch": true })], cx);
    workbench.update_in(cx, |workbench, _, _| workbench.search.focused = Some(RowKey::File(dir.join("sub/b.txt"))));
    run(&workbench, "search.action.restrictSearchToFolder", Vec::new(), cx);
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.search.value(SearchField::Include).to_string()), "./sub");
    workbench.update_in(cx, |workbench, _, _| workbench.search.focused = Some(RowKey::File(dir.join("sub/b.txt"))));
    run(&workbench, "search.action.revealInSideBar", Vec::new(), cx);
    assert_eq!(workbench.read_with(cx, |workbench, _| (workbench.side_bar_view, workbench.folder.as_ref().unwrap().selected.clone())), (SideBarView::Explorer, vec![dir.join("sub/b.txt")]));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn up_and_down_walk_the_search_history_saved_for_the_workspace(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = search("search-history", cx);
    for query in ["alpha", "beta"] {
        run(&workbench, "workbench.action.findInFiles", vec![json!({ "query": query, "triggerSearch": true })], cx);
        cx.executor().advance_clock(std::time::Duration::from_secs(3));
        cx.run_until_parked();
    }
    let stored = workbench.read_with(cx, |workbench, _| workbench.workspace_storage.lock().unwrap().get("workbench.search.history").map(str::to_owned));
    assert_eq!(stored.as_deref(), Some(r#"{"search":["alpha","beta"]}"#));
    let query = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.search.value(SearchField::Query).to_string());
    cx.simulate_keystrokes("up");
    assert_eq!(query(cx), "alpha");
    cx.simulate_keystrokes("up");
    assert_eq!(query(cx), "alpha", "the first element stays");
    cx.simulate_keystrokes("down");
    assert_eq!(query(cx), "beta");
    cx.simulate_keystrokes("down");
    assert_eq!(query(cx), "");
    run(&workbench, "search.action.clearHistory", Vec::new(), cx);
    assert!(workbench.read_with(cx, |workbench, _| workbench.workspace_storage.lock().unwrap().get("workbench.search.history").is_none()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn a_right_click_on_a_hovered_row_opens_the_menu_without_selecting_it(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = search("search-right-click", cx);
    run(&workbench, "workbench.action.findInFiles", vec![json!({ "query": "alpha", "triggerSearch": true })], cx);
    // The row under the mouse shows its actions in place of its badge; this redraw used to panic in paint.
    let row = point(px(150.), px(175.));
    cx.simulate_mouse_move(row, None, Modifiers::none());
    cx.simulate_mouse_down(row, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(row, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
    let (menu, focused, selected) = workbench.read_with(cx, |workbench, _| (workbench.context_menu.is_some(), workbench.search.focused.clone(), workbench.search.selected.clone()));
    assert!(menu && focused.is_some() && selected.is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

fn active_editor(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> (String, String, String) {
    workbench.read_with(cx, |workbench, _| {
        let editor = workbench.editors.active().unwrap();
        (editor.document.scheme.clone(), workbench.editor_name(editor), editor.document.lines.join("\n"))
    })
}

#[gpui::test]
fn open_in_editor_serializes_the_results_into_a_search_editor(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = search("search-open-in-editor", cx);
    run(&workbench, "workbench.action.findInFiles", vec![json!({ "query": "alphabet", "triggerSearch": true })], cx);
    workbench.update(cx, |workbench, _| workbench.settings.lock().unwrap().update_value_derived("search.searchEditor.defaultNumberOfContextLines", Some(&json!(0))).unwrap());
    run(&workbench, "search.action.openInEditor", Vec::new(), cx);
    let (scheme, name, text) = active_editor(&workbench, cx);
    assert_eq!((scheme.as_str(), name.as_str()), ("search-editor", "Search: alphabet"));
    assert_eq!(text, "1 result - 1 file\n\nsub/b.txt:\n  1: alphabet alpha\n");
    let ranges = workbench.read_with(cx, |workbench, _| workbench.search_editors.values().next().unwrap().match_ranges.clone());
    assert_eq!(ranges.len(), 1);
    assert!(workbench.read_with(cx, |workbench, _| !workbench.editors.active().unwrap().is_dirty()));
}

#[gpui::test]
fn new_search_editor_runs_its_query_with_context_lines(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = search("search-new-editor", cx);
    run(&workbench, "search.action.openNewEditor", vec![json!({ "query": "Alpha", "isCaseSensitive": true, "contextLines": 1, "triggerSearch": true })], cx);
    let (_, name, text) = active_editor(&workbench, cx);
    assert_eq!(name, "Search: Alpha");
    assert_eq!(text, "1 result - 1 file\n\na.txt:\n  1  alpha beta\n  2: Alpha\n");

    run(&workbench, "workbench.action.closeActiveEditor", Vec::new(), cx);
    assert!(workbench.read_with(cx, |workbench, _| workbench.search_editors.is_empty()));
}

#[gpui::test]
fn the_command_palette_opens_a_new_search_editor(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = search("search-palette-editor", cx);
    cx.simulate_keystrokes("cmd-shift-p");
    cx.simulate_input("New Search Editor");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let (scheme, name, _) = active_editor(&workbench, cx);
    assert_eq!((scheme.as_str(), name.as_str()), ("search-editor", "Search"));
    let context_lines = workbench.read_with(cx, |workbench, _| workbench.search_editors.values().next().unwrap().config.context_lines);
    assert_eq!(context_lines, 1, "search.searchEditor.defaultNumberOfContextLines");

    workbench.update(cx, |workbench, _| workbench.settings.lock().unwrap().update_value_derived("search.searchEditor.reusePriorSearchConfiguration", Some(&json!(true))).unwrap());
    run(&workbench, "search.action.openNewEditor", vec![json!({ "query": "alpha", "isRegexp": true, "contextLines": 0, "triggerSearch": true })], cx);
    run(&workbench, "search.action.openNewEditor", Vec::new(), cx);
    let config = workbench.read_with(cx, |workbench, _| workbench.search_editors[&PathBuf::from("/3")].config.clone());
    assert_eq!((config.query.as_str(), config.is_regexp, config.context_lines), ("", true, 1));
}

#[gpui::test]
fn search_editor_keys_select_matches_delete_file_results_and_focus_the_query(cx: &mut TestAppContext) {
    let (_dir, workbench, cx) = search("search-editor-keys", cx);
    run(&workbench, "search.action.openNewEditor", vec![json!({ "query": "alpha", "contextLines": 0, "triggerSearch": true })], cx);
    workbench.update_in(cx, |workbench, window, cx| window.focus(&workbench.editor_focus, cx));
    cx.simulate_keystrokes("cmd-shift-l");
    let selections = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().selections.len());
    assert_eq!(selections, 4);

    let (_, _, text) = active_editor(&workbench, cx);
    let line = text.lines().position(|line| line == "sub/b.txt:").unwrap();
    workbench.update(cx, |workbench, _| workbench.editors.active_mut().unwrap().set_selections(vec![crate::editor::Selection::caret(crate::editor::Position::new(line + 1, 3))]));
    cx.simulate_keystrokes("cmd-shift-backspace");
    let (_, _, text) = active_editor(&workbench, cx);
    assert_eq!(text, "4 results - 2 files\n\na.txt:\n  1: alpha beta\n  2: Alpha\n\n");

    cx.simulate_keystrokes("escape");
    let focused = workbench.update_in(cx, |workbench, window, _| workbench.focused_input(window));
    assert!(matches!(focused, Some(crate::workbench::input_box_view::InputId::SearchEditor(SearchField::Query))));
}

#[test]
fn a_preview_keeps_its_space_and_punctuation_before_the_ellipsis() {
    assert_eq!(keep_before_ellipsis(" pixel check."), " \u{200B}pixel \u{200B}check.\u{200B}");
}
