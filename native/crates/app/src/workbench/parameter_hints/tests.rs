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

#[test]
fn parameter_label_offsets_find_whole_words_or_take_offsets() {
    let label = "foo(aa: number, a: string)";
    assert_eq!(parameter_label_offsets(label, Some(&json!({ "label": "a" }))), (16, 17));
    assert_eq!(parameter_label_offsets(label, Some(&json!({ "label": [4, 6] }))), (4, 6));
    assert_eq!(parameter_label_offsets(label, Some(&json!({ "label": "b" }))), (0, 0));
    assert_eq!(parameter_label_offsets(label, None), (0, 0));
}

/// A folder with `a.txt` holding `text`, open at its end, with one signature help provider triggered by `(` and `,`.
fn open(cx: &mut TestAppContext) -> (gpui::Entity<Workbench>, &mut VisualTestContext, Receiver<HostMessage>) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-parameter-hints-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "foo").unwrap();
    let workspace = config::workspace::Workspace::folder(dir.clone());
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(workspace.clone()))));
    let (sender, receiver) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(sender), cx);
        workbench.watch_editor_focus(window, cx);
        workbench.open_file(dir.join("a.txt"), true, window, cx);
        window.focus(&workbench.editor_focus, cx);
        workbench.language_features.register(REGISTER, vec![json!(5), json!("*"), json!({ "triggerCharacters": ["("], "retriggerCharacters": [","] })]);
        workbench
    });
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("cursorBottom", vec![], window, cx));
    cx.run_until_parked();
    (workbench, cx, receiver)
}

fn type_text(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, text: &str) {
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("type", vec![json!({ "text": text })], window, cx));
    cx.executor().advance_clock(DELAY);
    cx.run_until_parked();
}

/// Answers the signature help requests sent since the last answer; returns their contexts.
fn answer(receiver: &Receiver<HostMessage>, help: Value, cx: &mut VisualTestContext) -> Vec<Value> {
    let mut contexts = Vec::new();
    for message in receiver.try_iter().filter(|message| message.method == "$provideSignatureHelp") {
        contexts.push(message.args[3].clone());
        let _ = message.reply.expect("a call").send(Ok(help.clone()));
    }
    cx.run_until_parked();
    contexts
}

fn help(active_signature: u64) -> Value {
    let signature = |label: &str| json!({ "label": label, "parameters": [{ "label": "a" }] });
    json!({ "id": 1, "signatures": [signature("foo(a)"), signature("foo(a, b)")], "activeSignature": active_signature, "activeParameter": 0 })
}

#[gpui::test]
fn a_trigger_character_shows_the_hints_and_a_retrigger_keeps_them(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx);
    type_text(&workbench, cx, "(");
    let contexts = answer(&receiver, help(0), cx);
    assert_eq!(contexts, [json!({ "triggerKind": TRIGGER_CHARACTER, "triggerCharacter": "(", "isRetrigger": false, "activeSignatureHelp": null })]);
    workbench.read_with(cx, |workbench, _| assert!(workbench.active_parameter_hints().is_some()));
    type_text(&workbench, cx, ",");
    let contexts = answer(&receiver, help(1), cx);
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0]["isRetrigger"], json!(true));
    assert_eq!(contexts[0]["triggerCharacter"], json!(","));
    assert_eq!(contexts[0]["activeSignatureHelp"]["activeSignature"], json!(0));
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("closeParameterHints", vec![], window, cx));
    workbench.read_with(cx, |workbench, _| assert!(workbench.active_parameter_hints().is_none()));
}

/// Text from the keyboard reaches the editor through the input handler, not the `type` command: it triggers the
/// hints too.
#[gpui::test]
fn keyboard_input_triggers_the_hints(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx);
    cx.simulate_input("(");
    cx.executor().advance_clock(DELAY);
    cx.run_until_parked();
    let contexts = answer(&receiver, help(0), cx);
    assert_eq!(contexts.len(), 1);
    workbench.read_with(cx, |workbench, _| assert!(workbench.active_parameter_hints().is_some()));
}

#[gpui::test]
fn next_and_previous_wrap_with_cycle_and_an_empty_answer_hides(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx);
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("editor.action.triggerParameterHints", vec![], window, cx));
    cx.run_until_parked();
    assert_eq!(answer(&receiver, help(1), cx)[0]["triggerKind"], json!(INVOKE));
    let active = |workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.active_parameter_hints().map(|hints| hints["activeSignature"].clone()));
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("showNextParameterHint", vec![], window, cx));
    assert_eq!(active(&workbench, cx), Some(json!(0)));
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("showPrevParameterHint", vec![], window, cx));
    assert_eq!(active(&workbench, cx), Some(json!(1)));
    type_text(&workbench, cx, "x");
    answer(&receiver, json!({ "signatures": [] }), cx);
    assert_eq!(active(&workbench, cx), None);
}
