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

/// `<div></div>` in a.html with `editor.linkedEditing` on, the cursor after `<d`, and one provider.
fn open(cx: &mut TestAppContext) -> (gpui::Entity<Workbench>, &mut VisualTestContext, Receiver<HostMessage>) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-linked-editing-{}", std::process::id()));
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();
    std::fs::write(dir.join("a.txt"), "<div></div>\n").unwrap();
    std::fs::write(dir.join(".vscode/settings.json"), r#"{ "editor.linkedEditing": true }"#).unwrap();
    let workspace = config::workspace::Workspace::folder(dir.clone());
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(workspace.clone()))));
    let (sender, receiver) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(sender), cx);
        workbench.watch_editor_focus(window, cx);
        workbench.open_file(dir.join("a.txt"), true, window, cx);
        window.focus(&workbench.editor_focus, cx);
        let editor = workbench.editors.active_mut().unwrap();
        editor.set_selections(vec![crate::editor::Selection::caret(Position::new(0, 2))]);
        workbench.language_features.register(REGISTER, vec![json!(3), json!("*")]);
        workbench.linked_editing_reinitialize(false, cx);
        workbench
    });
    (workbench, cx, receiver)
}

fn range(line: u64, start: u64, end: u64) -> Value {
    json!({ "startLineNumber": line, "startColumn": start, "endLineNumber": line, "endColumn": end })
}

/// Waits out the debounce, then answers every `$provideLinkedEditingRanges` asked so far.
fn answer_ranges(receiver: &Receiver<HostMessage>, ranges: Value, cx: &mut VisualTestContext) -> usize {
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    let mut asked = 0;
    for message in receiver.try_iter().filter(|message| message.method == "$provideLinkedEditingRanges") {
        asked += 1;
        let _ = message.reply.expect("a call").send(Ok(json!({ "ranges": ranges })));
    }
    cx.run_until_parked();
    asked
}

fn type_text(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, text: &str) {
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("type", vec![json!({ "text": text })], window, cx));
    cx.run_until_parked();
}

fn text(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().lines[0].clone())
}

#[gpui::test]
fn typing_in_the_reference_range_copies_it_to_the_mirror(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx);
    assert_eq!(answer_ranges(&receiver, json!([range(1, 8, 11), range(1, 2, 5)]), cx), 1);
    workbench.update_in(cx, |workbench, window, _| {
        // The range holding the cursor comes first.
        assert_eq!(workbench.linked_editing.ranges, [((1, 2), (1, 5)), ((1, 8), (1, 11))]);
        assert_eq!(workbench.key_context(window)["LinkedEditingInputVisible"], json!(true));
    });
    type_text(&workbench, cx, "x");
    assert_eq!(text(&workbench, cx), "<dxiv></dxiv>");
    // The mirrored edit changed the version, so the ranges are asked again.
    assert_eq!(answer_ranges(&receiver, json!([range(1, 2, 6), range(1, 9, 13)]), cx), 1);
    type_text(&workbench, cx, "y");
    assert_eq!(text(&workbench, cx), "<dxyiv></dxyiv>");
    // One undo takes back the typing and the mirrored edits together.
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("undo", vec![], window, cx));
    cx.run_until_parked();
    assert_eq!(text(&workbench, cx), "<div></div>");
}

#[gpui::test]
fn a_reference_that_is_no_longer_a_word_ends_linked_editing(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx);
    answer_ranges(&receiver, json!([range(1, 2, 5), range(1, 8, 11)]), cx);
    type_text(&workbench, cx, " ");
    assert_eq!(text(&workbench, cx), "<d iv></div>");
    workbench.read_with(cx, |workbench, _| assert!(workbench.linked_editing.ranges.is_empty()));
}

#[gpui::test]
fn escape_ends_linked_editing(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx);
    answer_ranges(&receiver, json!([range(1, 2, 5), range(1, 8, 11)]), cx);
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("cancelLinkedEditingInput", vec![], window, cx));
    cx.run_until_parked();
    type_text(&workbench, cx, "x");
    assert_eq!(text(&workbench, cx), "<dxiv></div>");
}
