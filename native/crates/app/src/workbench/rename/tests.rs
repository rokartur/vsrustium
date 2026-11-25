use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};

use super::*;
use crate::host_sync::{HostMessage, HostSync};
use crate::languages::Languages;
use crate::theme::load_theme;

/// A folder with `a.txt` holding two uses of `main`, the cursor at its start, and one rename provider.
fn open(cx: &mut TestAppContext, supports_resolve: bool) -> (gpui::Entity<Workbench>, &mut VisualTestContext, Receiver<HostMessage>) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-rename-{}-{supports_resolve}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "main();\nfunction main() {}\n").unwrap();
    let workspace = config::workspace::Workspace::folder(dir.clone());
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(workspace.clone()))));
    let (sender, receiver) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(sender), cx);
        workbench.watch_editor_focus(window, cx);
        workbench.open_file(dir.join("a.txt"), true, window, cx);
        window.focus(&workbench.editor_focus, cx);
        workbench.language_features.register(REGISTER, vec![json!(7), json!("*"), json!(supports_resolve)]);
        workbench
    });
    cx.run_until_parked();
    (workbench, cx, receiver)
}

fn run(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, command: &str) {
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command(command, vec![], window, cx));
    cx.run_until_parked();
}

/// Answers the `method` requests sent since the last answer; returns their arguments.
fn answer(receiver: &Receiver<HostMessage>, method: &str, value: Value, cx: &mut VisualTestContext) -> Vec<Vec<Value>> {
    let mut asked = Vec::new();
    for message in receiver.try_iter().filter(|message| message.method == method) {
        asked.push(message.args.clone());
        let _ = message.reply.expect("a call").send(Ok(value.clone()));
    }
    cx.run_until_parked();
    asked
}

fn range(start_line: u64, start_column: u64, end_line: u64, end_column: u64) -> Value {
    json!({ "startLineNumber": start_line, "startColumn": start_column, "endLineNumber": end_line, "endColumn": end_column })
}

fn rename_edit(dir_uri: &Value) -> Value {
    let edit = |line: u64, column: u64| json!({ "resource": dir_uri, "textEdit": { "range": range(line, column, line, column + 4), "text": "start" } });
    json!({ "edits": [edit(1, 1), edit(2, 10)] })
}

fn document_uri(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Value {
    workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document.uri())
}

fn text(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document.lines.join("\n"))
}

#[gpui::test]
fn the_input_takes_the_resolved_location_and_the_typed_name_applies_the_edits(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx, true);
    run(&workbench, cx, "editor.action.rename");
    let asked = answer(&receiver, "$resolveRenameLocation", json!({ "range": range(1, 1, 1, 5), "text": "main" }), cx);
    assert_eq!(asked[0][2], json!({ "lineNumber": 1, "column": 1 }));
    workbench.update_in(cx, |workbench, window, cx| {
        let widget = workbench.rename.widget.as_ref().expect("the input shows");
        assert_eq!((widget.input.value.as_str(), widget.input.selection), ("main", (0, 4)));
        assert!(workbench.rename.focus.is_focused(window));
        let context = workbench.key_context(window);
        assert_eq!((&context["editorFocus"], &context["editorTextFocus"], &context["renameInputVisible"]), (&json!(true), &json!(false), &json!(true)));
        workbench.rename.widget.as_mut().unwrap().input.set_value("start".into());
        let _ = cx;
    });
    run(&workbench, cx, "acceptRenameInput");
    let uri = document_uri(&workbench, cx);
    let asked = answer(&receiver, "$provideRenameEdits", rename_edit(&uri), cx);
    assert_eq!(asked[0][3], json!("start"));
    assert_eq!(text(&workbench, cx), "start();\nfunction start() {}\n");
    workbench.update_in(cx, |workbench, window, _| {
        assert!(workbench.rename.widget.is_none());
        assert!(workbench.editor_focus.is_focused(window));
        // The active editor's edits are made minimal ("m" to "st", "in" to "rt"), so the caret moves past "st".
        assert_eq!(workbench.editors.active().unwrap().selection().head, crate::editor::Position::new(0, 2));
    });
}

#[gpui::test]
fn without_prepare_rename_the_word_at_the_cursor_is_renamed(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx, false);
    run(&workbench, cx, "editor.action.rename");
    assert!(answer(&receiver, "$resolveRenameLocation", Value::Null, cx).is_empty());
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.rename.widget.as_ref().unwrap().input.value, "main"));
}

#[gpui::test]
fn a_reject_reason_shows_as_a_message_and_no_input(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx, true);
    run(&workbench, cx, "editor.action.rename");
    answer(&receiver, "$resolveRenameLocation", json!({ "rejectReason": "You cannot rename this element." }), cx);
    workbench.read_with(cx, |workbench, _| {
        assert!(workbench.rename.widget.is_none());
        assert!(workbench.editor_message_visible());
    });
}

#[gpui::test]
fn an_unchanged_name_asks_for_no_edits_and_escape_cancels(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx, false);
    run(&workbench, cx, "editor.action.rename");
    run(&workbench, cx, "acceptRenameInput");
    assert!(answer(&receiver, "$provideRenameEdits", Value::Null, cx).is_empty());
    run(&workbench, cx, "editor.action.rename");
    run(&workbench, cx, "cancelRenameInput");
    workbench.update_in(cx, |workbench, window, _| {
        assert!(workbench.rename.widget.is_none());
        assert!(workbench.editor_focus.is_focused(window));
    });
}

#[gpui::test]
fn no_result_falls_back_to_the_next_provider_then_an_info_notification(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx, false);
    run(&workbench, cx, "editor.action.rename");
    workbench.update(cx, |workbench, _| workbench.rename.widget.as_mut().unwrap().input.set_value("start".into()));
    run(&workbench, cx, "acceptRenameInput");
    assert_eq!(answer(&receiver, "$provideRenameEdits", Value::Null, cx).len(), 1);
    assert_eq!(text(&workbench, cx), "main();\nfunction main() {}\n");
    workbench.read_with(cx, |workbench, _| assert!(workbench.notifications.model.items.iter().any(|item| item.message.raw == "No result.")));
}

#[gpui::test]
fn an_edit_in_the_editor_cancels_the_input(cx: &mut TestAppContext) {
    let (workbench, cx, _receiver) = open(cx, false);
    run(&workbench, cx, "editor.action.rename");
    workbench.update_in(cx, |workbench, window, cx| {
        let editor = workbench.editors.active_mut().unwrap();
        let end = editor.document.lines[1].chars().count();
        let at = crate::editor::Position::new(1, end);
        editor.execute_edits(vec![(at, at, " ".into(), false)], None, crate::editor::EditReason::Unknown("test"));
        let _ = (window, cx);
    });
    workbench.update_in(cx, |_, window, _| window.refresh());
    cx.run_until_parked();
    workbench.read_with(cx, |workbench, _| assert!(workbench.rename.widget.is_none()));
}
