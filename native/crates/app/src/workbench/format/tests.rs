use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};

use super::*;
use crate::host_sync::{HostMessage, HostSync};
use crate::languages::Languages;
use crate::theme::load_theme;

fn lines(text: &[&str]) -> Vec<String> {
    text.iter().map(|line| line.to_string()).collect()
}

fn at(line: usize, column: usize) -> Position {
    Position::new(line, column)
}

// editorWebWorker.test.ts
#[test]
fn more_minimal() {
    let model = lines(&["This is line one", "and this is line number two", "it is followed by #3", "and finished with the fourth."]);
    let edits = more_minimal_edits(&model, vec![(at(0, 0), at(0, 16), "This is line One".into())]);
    assert_eq!(edits, [(at(0, 13), at(0, 14), "O".to_string())]);
}

#[test]
fn more_minimal_merges_adjacent_edits() {
    let model = lines(&["one", "two", "three", "four", "five"]);
    let edits = more_minimal_edits(
        &model,
        vec![
            (at(0, 0), at(1, 0), "one\ntwo\nthree\n".into()),
            (at(1, 0), at(2, 0), String::new()),
            (at(2, 0), at(3, 0), String::new()),
            (at(3, 1), at(3, 2), "4".into()),
            (at(4, 2), at(4, 4), "5".into()),
        ],
    );
    let texts: Vec<&str> = edits.iter().map(|edit| edit.2.as_str()).collect();
    assert_eq!(texts, ["4", "5"]);
}

#[test]
fn more_minimal_issue_15385_newline_changes_only() {
    let model = lines(&["{", "\t\"a\":1", "}"]);
    assert!(more_minimal_edits(&model, vec![(at(0, 0), at(2, 1), "{\r\n\t\"a\":1\r\n}".into())]).is_empty());
}

#[test]
fn more_minimal_issue_15385_newline_changes_and_other() {
    let model = lines(&["{", "\t\"a\":1", "}"]);
    let edits = more_minimal_edits(&model, vec![(at(0, 0), at(2, 1), "{\r\n\t\"b\":1\r\n}".into())]);
    assert_eq!(edits, [(at(1, 2), at(1, 3), "b".to_string())]);
}

#[test]
fn more_minimal_issue_15385_newline_changes_and_other_2() {
    let model = lines(&["package main", "func foo() {", "}"]);
    // Range(3, 2, 4, 1000) validated to the end of the model.
    let edits = more_minimal_edits(&model, vec![(at(2, 1), at(2, 1), "\n".into())]);
    assert_eq!(edits, [(at(2, 1), at(2, 1), "\n".to_string())]);
}

#[test]
fn more_minimal_preserves_surrogate_pairs() {
    let original = "𫚭齅5龯蝌䶱aＺr灋㊣珸琱璳AbD龪龫藏㒱㓥㓹3UxヅЁЖa偓兘㎜𫟹";
    let modified = "㈢鿃2磷𬸚﹢㘥怐ND䶮ёㄆホ鿃龬鿍a6纷冬督㊣㈱㔾㖏㕽內冚7畫皗Э𫔍";
    let model = lines(&[original]);
    let edits = more_minimal_edits(&model, vec![(at(0, 0), at(0, original.chars().count()), modified.into())]);
    let mut text: Vec<char> = original.chars().collect();
    for (start, end, replacement) in edits.into_iter().rev() {
        text.splice(start.column..end.column, replacement.chars());
    }
    assert_eq!(text.into_iter().collect::<String>(), modified);
}

#[test]
fn touching_ranges_merge() {
    let ranges = merge_touching(vec![(at(3, 0), at(3, 4)), (at(0, 0), at(1, 2)), (at(1, 2), at(2, 0))]);
    assert_eq!(ranges, [(at(0, 0), at(2, 0)), (at(3, 0), at(3, 4))]);
}

/// A folder with `a.txt` holding `text` and user settings `user_settings`, `a.txt` open and focused.
fn open<'a>(name: &str, text: &str, user_settings: &str, cx: &'a mut TestAppContext) -> (PathBuf, gpui::Entity<Workbench>, &'a mut VisualTestContext, Receiver<HostMessage>) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("vsrustium-format-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    std::fs::write(dir.join("User/settings.json"), user_settings).unwrap();
    std::fs::write(dir.join("a.txt"), text).unwrap();
    let settings = Arc::new(Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None)));
    let (sender, receiver) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(sender), cx);
        workbench.skip_dialogs = true;
        workbench.watch_editor_focus(window, cx);
        workbench.open_file(dir.join("a.txt"), true, window, cx);
        window.focus(&workbench.editor_focus, cx);
        workbench
    });
    cx.run_until_parked();
    (dir, workbench, cx, receiver)
}

fn register(workbench: &gpui::Entity<Workbench>, method: &str, args: Vec<Value>, cx: &mut VisualTestContext) {
    workbench.update(cx, |workbench, cx| {
        workbench.language_features.register(method, args);
        workbench.language_providers_changed(method, cx);
    });
}

fn run(workbench: &gpui::Entity<Workbench>, command: &str, args: Vec<Value>, cx: &mut VisualTestContext) {
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(command, args, window, cx)));
    cx.run_until_parked();
}

/// Answers the `method` requests sent since the last answer; returns their handles.
fn answer(receiver: &Receiver<HostMessage>, method: &str, value: Value, cx: &mut VisualTestContext) -> Vec<u64> {
    let mut handles = Vec::new();
    for message in receiver.try_iter().filter(|message| message.method == method) {
        handles.push(message.args[0].as_u64().unwrap());
        let _ = message.reply.expect("a call").send(Ok(value.clone()));
    }
    cx.run_until_parked();
    handles
}

fn text(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document.lines.join("\n"))
}

fn edit(line: u64, column: u64, end_column: u64, text: &str) -> Value {
    json!({ "range": { "startLineNumber": line, "startColumn": column, "endLineNumber": line, "endColumn": end_column }, "text": text })
}

// formatActionsMultiple.test.ts
#[gpui::test]
fn selects_formatter_argument_case_insensitively(cx: &mut TestAppContext) {
    let (dir, workbench, cx, receiver) = open("argument", "const value = 1;", "{}", cx);
    register(&workbench, DOCUMENT, vec![json!(1), json!("*"), json!({ "value": "publisher.first" }), json!("First")], cx);
    register(&workbench, DOCUMENT, vec![json!(2), json!("*"), json!({ "value": "publisher.requested" }), json!("Requested")], cx);
    run(&workbench, "editor.action.formatDocument.multiple", vec![json!({ "formatter": "PUBLISHER.REQUESTED" })], cx);
    assert_eq!(answer(&receiver, "$provideDocumentFormattingEdits", json!([]), cx), [2]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn format_document_applies_the_only_formatters_edits_as_one_undo_step(cx: &mut TestAppContext) {
    let (dir, workbench, cx, receiver) = open("document", "const x=1", "{}", cx);
    register(&workbench, DOCUMENT, vec![json!(1), json!("*"), json!({ "value": "a.b" }), json!("B")], cx);
    workbench.update_in(cx, |workbench, window, _| assert_eq!(workbench.key_context(window)["editorHasDocumentFormattingProvider"], json!(true)));
    run(&workbench, "editor.action.formatDocument", vec![], cx);
    answer(&receiver, "$provideDocumentFormattingEdits", json!([edit(1, 1, 10, "const x = 1;")]), cx);
    assert_eq!(text(&workbench, cx), "const x = 1;");
    run(&workbench, "undo", vec![], cx);
    assert_eq!(text(&workbench, cx), "const x=1");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn a_range_formatter_formats_the_document_and_the_selection(cx: &mut TestAppContext) {
    let (dir, workbench, cx, receiver) = open("range", "a=1\nb=2", "{}", cx);
    register(&workbench, RANGE, vec![json!(1), json!("*"), json!({ "value": "a.b" }), json!("B"), json!(false)], cx);
    run(&workbench, "editor.action.formatDocument", vec![], cx);
    let asked: Vec<Value> = receiver.try_iter().filter(|message| message.method == "$provideDocumentRangeFormattingEdits").map(|message| message.args[2].clone()).collect();
    assert_eq!(asked, [json!({ "startLineNumber": 1, "startColumn": 1, "endLineNumber": 2, "endColumn": 4 })]);
    run(&workbench, "cursorBottom", vec![], cx);
    run(&workbench, "editor.action.formatSelection", vec![], cx);
    answer(&receiver, "$provideDocumentRangeFormattingEdits", json!([edit(2, 2, 3, " = ")]), cx);
    assert_eq!(text(&workbench, cx), "a=1\nb = 2");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn two_formatters_without_a_default_format_nothing_and_ask_to_configure(cx: &mut TestAppContext) {
    let (dir, workbench, cx, receiver) = open("conflict", "x", "{}", cx);
    register(&workbench, DOCUMENT, vec![json!(1), json!("*"), json!({ "value": "a.one" }), json!("One")], cx);
    register(&workbench, DOCUMENT, vec![json!(2), json!("*"), json!({ "value": "a.two" }), json!("Two")], cx);
    let key = workbench.read_with(cx, |workbench, _| workbench.active_model_key().unwrap());
    let formatters = workbench.read_with(cx, |workbench, _| workbench.document_formatters(workbench.editors.active().unwrap()));
    let analyzed = workbench.read_with(cx, |workbench, _| workbench.analyze_formatter(&formatters, &key, FormattingKind::File).unwrap_err());
    assert_eq!(analyzed, "There are multiple formatters for ''Plain Text'' files. One of them should be configured as default formatter.");
    std::fs::write(dir.join("User/settings.json"), r#"{ "editor.defaultFormatter": "A.TWO" }"#).unwrap();
    workbench.update(cx, |workbench, _| drop(workbench.settings.lock().unwrap().reload_changed_files()));
    let analyzed = workbench.read_with(cx, |workbench, _| workbench.analyze_formatter(&formatters, &key, FormattingKind::File).unwrap());
    assert_eq!(analyzed.handle, 2);
    assert!(answer(&receiver, "$provideDocumentFormattingEdits", json!([]), cx).is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn format_on_save_formats_after_trimming_and_before_the_final_newline(cx: &mut TestAppContext) {
    let settings = r#"{ "editor.formatOnSave": true, "files.trimTrailingWhitespace": true, "files.insertFinalNewline": true }"#;
    let (dir, workbench, cx, receiver) = open("save", "a=1  ", settings, cx);
    register(&workbench, DOCUMENT, vec![json!(1), json!("*"), json!({ "value": "a.b" }), json!("B")], cx);
    run(&workbench, "cursorEnd", vec![], cx);
    run(&workbench, "type", vec![json!({ "text": " " })], cx);
    run(&workbench, "workbench.action.files.save", vec![], cx);
    let asked: Vec<Value> = receiver.try_iter().filter(|message| message.method == "$provideDocumentFormattingEdits").map(|message| {
        let _ = message.reply.expect("a call").send(Ok(json!([edit(1, 1, 4, "a = 1")])));
        message.args[1].clone()
    }).collect();
    assert_eq!(asked.len(), 1);
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "a=1  ", "the write waits for the formatter");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "a = 1\n");
    std::fs::remove_dir_all(dir).unwrap();
}

// formatActionsMultiple.test.ts "Format On Type"
fn on_type<'a>(name: &str, cx: &'a mut TestAppContext) -> (PathBuf, gpui::Entity<Workbench>, &'a mut VisualTestContext, Receiver<HostMessage>) {
    let (dir, workbench, cx, receiver) = open(name, "const x=1\nnext", r#"{ "editor.formatOnType": true }"#, cx);
    register(&workbench, ON_TYPE, vec![json!(1), json!("*"), json!([";"]), json!({ "value": "a.b" })], cx);
    run(&workbench, "cursorEnd", vec![], cx);
    run(&workbench, "type", vec![json!({ "text": ";" })], cx);
    (dir, workbench, cx, receiver)
}

fn complete_on_type(workbench: &gpui::Entity<Workbench>, receiver: &Receiver<HostMessage>, cx: &mut VisualTestContext) -> String {
    let asked = answer(receiver, "$provideOnTypeFormattingEdits", json!([edit(1, 8, 8, " "), edit(1, 9, 9, " ")]), cx);
    assert_eq!(asked.len(), 1);
    text(workbench, cx)
}

#[gpui::test]
fn on_type_applies_pending_edits_when_the_model_is_unchanged(cx: &mut TestAppContext) {
    let (dir, workbench, cx, receiver) = on_type("on-type-applies", cx);
    assert_eq!(complete_on_type(&workbench, &receiver, cx), "const x = 1;\nnext");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn on_type_cancels_pending_edits_after_typing_on_the_trigger_line(cx: &mut TestAppContext) {
    let (dir, workbench, cx, receiver) = on_type("on-type-cancels", cx);
    // A space is no trigger character, so no second request.
    run(&workbench, "type", vec![json!({ "text": " " })], cx);
    assert_eq!(complete_on_type(&workbench, &receiver, cx), "const x=1; \nnext");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn on_type_preserves_pending_edits_after_changes_below_the_trigger_line(cx: &mut TestAppContext) {
    let (dir, workbench, cx, receiver) = on_type("on-type-below", cx);
    workbench.update(cx, |workbench, cx| {
        let editor = workbench.editors.active_mut().unwrap();
        let change = editor.execute_edits(vec![(at(1, 0), at(1, 4), "changed".into(), false)], None, EditReason::Unknown("test")).unwrap();
        workbench.editor_edited(Some(change), false, "api", cx);
    });
    assert_eq!(complete_on_type(&workbench, &receiver, cx), "const x = 1;\nchanged");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn on_type_cancels_pending_edits_when_format_on_type_is_disabled(cx: &mut TestAppContext) {
    let (dir, workbench, cx, receiver) = on_type("on-type-disabled", cx);
    std::fs::write(dir.join("User/settings.json"), r#"{ "editor.formatOnType": false }"#).unwrap();
    workbench.update(cx, |workbench, _| drop(workbench.settings.lock().unwrap().reload_changed_files()));
    assert_eq!(complete_on_type(&workbench, &receiver, cx), "const x=1;\nnext");
    std::fs::remove_dir_all(dir).unwrap();
}
