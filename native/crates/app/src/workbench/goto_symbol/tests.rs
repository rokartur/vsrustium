use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};

use super::*;
use crate::commands::Outcome;
use crate::host_sync::{HostMessage, HostSync};
use crate::languages::Languages;
use crate::theme::load_theme;

/// A folder with `a.txt` holding `text`, open with the cursor at the start, and one definition provider.
pub(in crate::workbench) fn open<'a>(cx: &'a mut TestAppContext, name: &str, text: &str) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext, Receiver<HostMessage>, PathBuf) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-goto-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), text).unwrap();
    let workspace = config::workspace::Workspace::folder(dir.clone());
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(workspace.clone()))));
    let (sender, receiver) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(sender), cx);
        workbench.watch_editor_focus(window, cx);
        workbench.open_file(dir.join("a.txt"), true, window, cx);
        window.focus(&workbench.editor_focus, cx);
        workbench.language_features.register(DEFINITION.register, vec![json!(7), json!("*")]);
        workbench.language_features.register(REFERENCES.register, vec![json!(8), json!("*")]);
        workbench
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (workbench, cx, receiver, dir)
}

/// Answers every `method` call sent since the last answer with `answer`.
pub(in crate::workbench) fn answer(receiver: &Receiver<HostMessage>, method: &str, answer: Value, cx: &mut VisualTestContext) {
    for message in receiver.try_iter() {
        if message.method == method {
            let _ = message.reply.expect("a call").send(Ok(answer.clone()));
        }
    }
    cx.run_until_parked();
}

pub(in crate::workbench) fn link(dir: &Path, line: usize, start: usize, end: usize) -> Value {
    json!({ "uri": { "scheme": "file", "path": dir.join("a.txt").to_string_lossy() }, "range": { "startLineNumber": line, "startColumn": start, "endLineNumber": line, "endColumn": end } })
}

pub(in crate::workbench) fn run(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, command: &str) {
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command(command, vec![], window, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn execute_definition_provider_sorts_and_drops_duplicates(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, dir) = open(cx, "execute", "foo\nbar foo\n");
    let uri = json!({ "scheme": "file", "path": dir.join("a.txt").to_string_lossy() });
    let outcome = workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("_executeDefinitionProvider", vec![uri, json!({ "lineNumber": 2, "column": 6 })], window, cx));
    cx.run_until_parked();
    answer(&receiver, "$provideDefinition", json!([link(&dir, 2, 5, 8), link(&dir, 1, 1, 4), link(&dir, 2, 5, 8)]), cx);
    let Outcome::Pending(mut result) = outcome else { panic!("the providers answer later") };
    let links = result.try_recv().unwrap().unwrap().unwrap();
    let lines: Vec<_> = links.as_array().unwrap().iter().map(|link| link["range"]["startLineNumber"].clone()).collect();
    assert_eq!(lines, [json!(1), json!(2)]);
}

#[gpui::test]
fn one_definition_moves_the_cursor_and_none_shows_a_message(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, dir) = open(cx, "reveal", "foo\nbar foo\n");
    run(&workbench, cx, "cursorDown");
    run(&workbench, cx, "editor.action.revealDefinition");
    answer(&receiver, "$provideDefinition", json!([link(&dir, 1, 1, 4)]), cx);
    workbench.read_with(cx, |workbench, _| {
        assert_eq!(workbench.editors.active().unwrap().selection().head, Position::new(0, 0));
        assert!(workbench.goto_symbol.highlight.is_some());
    });
    run(&workbench, cx, "editor.action.revealDefinition");
    answer(&receiver, "$provideDefinition", json!([]), cx);
    workbench.read_with(cx, |workbench, _| assert!(workbench.editor_message_visible()));
}

#[gpui::test]
fn several_definitions_open_the_peek_and_escape_closes_it(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, dir) = open(cx, "peek", "foo\nbar foo\nfoo\n");
    run(&workbench, cx, "editor.action.revealDefinition");
    answer(&receiver, "$provideDefinition", json!([link(&dir, 2, 5, 8), link(&dir, 3, 1, 4)]), cx);
    workbench.read_with(cx, |workbench, _| assert!(workbench.references_peek_visible()));
    run(&workbench, cx, "goToNextReference");
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.editors.active().unwrap().selection().head, Position::new(2, 0)));
    run(&workbench, cx, "closeReferenceSearch");
    workbench.read_with(cx, |workbench, _| assert!(!workbench.references_peek_visible()));
}

#[gpui::test]
fn goto_preference_walks_the_results_with_f12(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, dir) = open(cx, "walk", "foo\nbar foo\nfoo\n");
    workbench.update(cx, |workbench, _| {
        let value = json!("goto");
        workbench.settings.lock().unwrap().update_value("editor.gotoLocation.multipleDefinitions", Some(&value), config::service::Target::User, None, &[]).unwrap();
    });
    run(&workbench, cx, "editor.action.revealDefinition");
    answer(&receiver, "$provideDefinition", json!([link(&dir, 2, 5, 8), link(&dir, 3, 1, 4)]), cx);
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.editors.active().unwrap().selection().head, Position::new(1, 4)));
    run(&workbench, cx, "editor.gotoNextSymbolFromResult");
    workbench.read_with(cx, |workbench, _| {
        assert_eq!(workbench.editors.active().unwrap().selection().head, Position::new(2, 0));
        assert!(workbench.goto_symbol.navigation.is_some());
    });
}

fn cmd() -> gpui::Modifiers {
    gpui::Modifiers { platform: true, ..Default::default() }
}

/// goToDefinitionAtPosition.ts: Cmd over a word with one definition links it with a preview of the definition's
/// source; releasing Cmd removes the link, a Cmd+click goes to the definition.
#[gpui::test]
fn cmd_over_a_word_links_it_and_cmd_click_goes_to_the_definition(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, dir) = open(cx, "link", "function foo() {\n  return 1;\n}\nfoo();\n");
    workbench.update(cx, |workbench, cx| workbench.definition_link_mouse_moved(3, 1.2, cmd(), false, cx));
    cx.run_until_parked();
    let definition = json!({ "uri": { "scheme": "file", "path": dir.join("a.txt").to_string_lossy() }, "range": { "startLineNumber": 1, "startColumn": 1, "endLineNumber": 3, "endColumn": 2 } });
    answer(&receiver, "$provideDefinition", json!([definition]), cx);
    workbench.read_with(cx, |workbench, _| {
        let editor = workbench.editors.active().unwrap();
        assert_eq!(workbench.definition_link_styles(editor, 3, 4).iter().map(|(range, _)| range.clone()).collect::<Vec<_>>(), [0..3]);
        let anchor = super::super::content_hover::Anchor { model_id: editor.model_id, line: 3, start: 1, end: 2 };
        let parts = workbench.definition_link_hover_parts(editor, anchor);
        let super::super::content_hover::PartContent::Markdown(value) = &parts[0].content else { panic!("a markdown part") };
        assert_eq!(value[0]["value"], "\n```plaintext\nfunction foo() {\n  return 1;\n}\n```\n");
    });
    // The same word asks no provider again.
    workbench.update(cx, |workbench, cx| workbench.definition_link_mouse_moved(3, 2.4, cmd(), false, cx));
    assert!(receiver.try_iter().all(|message| message.method != "$provideDefinition"));
    workbench.update(cx, |workbench, cx| workbench.definition_link_modifiers_changed(gpui::Modifiers::default(), cx));
    workbench.read_with(cx, |workbench, _| assert!(workbench.definition_link_styles(workbench.editors.active().unwrap(), 3, 4).is_empty()));
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.definition_link_mouse_down(3, cmd());
        workbench.definition_link_mouse_up(3, 1.2, cmd(), 1, window, cx);
    });
    cx.run_until_parked();
    answer(&receiver, "$provideDefinition", json!([definition]), cx);
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.editors.active().unwrap().selection().head, Position::new(0, 0)));
}

/// Several definitions link the word with a count; without Cmd, or past the line's end, nothing is asked.
#[gpui::test]
fn several_definitions_link_the_word_with_a_count(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, dir) = open(cx, "link-count", "foo\nbar foo\n");
    workbench.update(cx, |workbench, cx| {
        workbench.definition_link_mouse_moved(1, 5.5, gpui::Modifiers::default(), false, cx);
        workbench.definition_link_mouse_moved(1, 7.5, cmd(), false, cx);
    });
    cx.run_until_parked();
    assert!(receiver.try_iter().all(|message| message.method != "$provideDefinition"));
    workbench.update(cx, |workbench, cx| workbench.definition_link_mouse_moved(1, 5.5, cmd(), false, cx));
    cx.run_until_parked();
    answer(&receiver, "$provideDefinition", json!([link(&dir, 1, 1, 4), link(&dir, 2, 5, 8)]), cx);
    workbench.read_with(cx, |workbench, _| {
        let editor = workbench.editors.active().unwrap();
        assert_eq!(workbench.definition_link_styles(editor, 1, 4).iter().map(|(range, _)| range.clone()).collect::<Vec<_>>(), [4..7]);
        let anchor = super::super::content_hover::Anchor { model_id: editor.model_id, line: 1, start: 5, end: 6 };
        let super::super::content_hover::PartContent::Markdown(value) = &workbench.definition_link_hover_parts(editor, anchor)[0].content else { panic!("a markdown part") };
        assert_eq!(value[0]["value"], "Click to show 2 definitions.");
    });
}

/// hoverActions.ts ShowDefinitionPreviewHoverAction: the hover at the cursor shows the definition's preview; a cursor
/// move removes the link.
#[gpui::test]
fn show_definition_preview_hover_shows_the_preview_at_the_cursor(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, dir) = open(cx, "preview-hover", "const foo = 1;\nfoo;\n");
    run(&workbench, cx, "cursorDown");
    run(&workbench, cx, "editor.action.showDefinitionPreviewHover");
    answer(&receiver, "$provideDefinition", json!([link(&dir, 1, 7, 10)]), cx);
    workbench.read_with(cx, |workbench, _| {
        let shown = workbench.content_hover.shown.as_ref().expect("the hover shows");
        let super::super::content_hover::PartContent::Markdown(value) = &shown.parts[0].content else { panic!("a markdown part") };
        assert_eq!(value[0]["value"], "\n```plaintext\nfoo\n```\n");
    });
    run(&workbench, cx, "cursorRight");
    workbench.read_with(cx, |workbench, _| assert!(workbench.definition_link_styles(workbench.editors.active().unwrap(), 1, 4).is_empty()));
}
