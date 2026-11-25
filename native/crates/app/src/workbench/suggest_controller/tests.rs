use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};

use super::*;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;

#[test]
fn quick_suggestions_read_bools_and_kinds() {
    assert!(quick_suggestions_all_off(&json!(false)));
    assert!(quick_suggestions_all_on(&json!(true)));
    let default = json!({ "other": "on", "comments": "off", "strings": "off" });
    assert!(!quick_suggestions_all_off(&default) && !quick_suggestions_all_on(&default));
    assert_eq!(quick_suggestions_value(&default["strings"]), "off");
}

#[test]
fn line_words_split_at_separators() {
    assert_eq!(line_words("foo.bar(baz, 12)"), ["foo", "bar", "baz", "12"]);
}

/// A folder with `a.txt` holding `text`, open and focused at its end.
fn open<'a>(cx: &'a mut TestAppContext, text: &str) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    static OPENED: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("code-gpui-suggest-{}-{}", std::process::id(), OPENED.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), text).unwrap();
    let workspace = config::workspace::Workspace::folder(dir.clone());
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(workspace.clone()))));
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.watch_editor_focus(window, cx);
        workbench.open_file(dir.join("a.txt"), true, window, cx);
        window.focus(&workbench.editor_focus, cx);
        workbench
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    run(&workbench, cx, "cursorBottom");
    (workbench, cx)
}

fn run(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, command: &str) {
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command(command, vec![], window, cx));
    cx.run_until_parked();
}

fn type_text(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, text: &str) {
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("type", vec![json!({ "text": text })], window, cx));
    cx.executor().advance_clock(Duration::from_millis(20));
    cx.run_until_parked();
}

fn shown(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    workbench.read_with(cx, |workbench, _| match workbench.suggest_widget_visible() {
        true => workbench.suggest.session.as_ref().and_then(|session| session.model.as_ref()).map(|model| model.filtered().iter().map(|item| item.label.clone()).collect()).unwrap_or_default(),
        false => Vec::new(),
    })
}

fn text(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document.lines.join("\n"))
}

/// Typing a word start shows the other words of the document (the word based provider) and filters as it goes.
#[gpui::test]
fn typing_shows_the_document_words_and_filters_them(cx: &mut TestAppContext) {
    let (workbench, cx) = open(cx, "foo fob bar\n");
    type_text(&workbench, cx, "f");
    assert_eq!(shown(&workbench, cx), ["fob", "foo"]);
    type_text(&workbench, cx, "oo");
    assert_eq!(shown(&workbench, cx), ["foo"]);
    run(&workbench, cx, "hideSuggestWidget");
    assert!(shown(&workbench, cx).is_empty());
}

/// selectNextSuggestion moves the focus; acceptSelectedSuggestion replaces the typed word with the item.
#[gpui::test]
fn accepting_replaces_the_typed_prefix(cx: &mut TestAppContext) {
    let (workbench, cx) = open(cx, "foo fob bar\n");
    type_text(&workbench, cx, "f");
    run(&workbench, cx, "selectNextSuggestion");
    run(&workbench, cx, "acceptSelectedSuggestion");
    assert_eq!(text(&workbench, cx), "foo fob bar\nfoo");
    assert!(shown(&workbench, cx).is_empty());
}

fn insert_snippet(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, template: &str) {
    workbench.update(cx, |workbench, cx| {
        let head = workbench.editors.active().unwrap().selection().head;
        let snippets = [(head, head, template, false)];
        workbench.insert_snippets(SnippetInsert { snippets: &snippets, clipboard: None, undo_stop_before: true, undo_stop_after: true, reason: "snippet" }, cx);
    });
    cx.run_until_parked();
}

fn in_snippet(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> bool {
    workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().snippet_session.is_some())
}

/// snippetController2.test.ts: typing replaces the placeholder, Tab goes to the next one, the final tab stop ends
/// the session.
#[gpui::test]
fn tab_moves_through_the_placeholders_and_the_final_one_ends_the_session(cx: &mut TestAppContext) {
    let (workbench, cx) = open(cx, "");
    insert_snippet(&workbench, cx, "f(${1:x}, ${2:y})$0;");
    assert!(in_snippet(&workbench, cx));
    type_text(&workbench, cx, "ab");
    run(&workbench, cx, "jumpToNextSnippetPlaceholder");
    type_text(&workbench, cx, "c");
    assert_eq!(text(&workbench, cx), "f(ab, c);");
    run(&workbench, cx, "jumpToPrevSnippetPlaceholder");
    let selection = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().selection());
    assert_eq!((selection.start().column, selection.end().column), (2, 4));
    run(&workbench, cx, "jumpToNextSnippetPlaceholder");
    run(&workbench, cx, "jumpToNextSnippetPlaceholder");
    assert!(!in_snippet(&workbench, cx));
    let head = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().selection().head);
    assert_eq!(head.column, 8);
}

/// Escape (`leaveSnippet`) ends the session; a choice placeholder shows its options in the suggest widget.
#[gpui::test]
fn a_choice_shows_its_options_and_escape_leaves(cx: &mut TestAppContext) {
    let (workbench, cx) = open(cx, "");
    insert_snippet(&workbench, cx, "${1|one,two|} $2");
    assert_eq!(shown(&workbench, cx), ["one", "two"]);
    run(&workbench, cx, "selectNextSuggestion");
    run(&workbench, cx, "acceptSelectedSuggestion");
    assert_eq!(text(&workbench, cx), "two ");
    assert!(in_snippet(&workbench, cx));
    run(&workbench, cx, "leaveSnippet");
    assert!(!in_snippet(&workbench, cx));
}

/// A manual trigger with no word shows every word; a cursor move off the line ends the session.
#[gpui::test]
fn a_manual_trigger_lists_every_word_and_a_move_off_the_line_cancels(cx: &mut TestAppContext) {
    let (workbench, cx) = open(cx, "foo bar\n");
    run(&workbench, cx, "editor.action.triggerSuggest");
    assert_eq!(shown(&workbench, cx), ["bar", "foo"]);
    run(&workbench, cx, "cursorUp");
    assert!(shown(&workbench, cx).is_empty());
}
