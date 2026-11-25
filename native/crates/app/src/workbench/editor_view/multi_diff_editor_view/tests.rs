use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::host_sync::{HostMessage, HostSync};
use crate::languages::Languages;
use crate::multi_diff_editor::Row;
use crate::theme::load_theme;
use crate::workbench::Workbench;

fn workbench_in<'a>(name: &str, cx: &'a mut TestAppContext) -> (PathBuf, gpui::Entity<Workbench>, Receiver<HostMessage>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\nfour").unwrap();
    std::fs::write(dir.join("b.txt"), "one\nTWO\nthree\ninserted\nfour").unwrap();
    std::fs::write(dir.join("c.txt"), "alpha\nbeta").unwrap();
    let (sender, receiver) = channel();
    let settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(sender), cx));
    (dir, workbench, receiver, cx)
}

fn file_uri(path: &Path) -> Value {
    json!({ "scheme": "file", "path": path.to_string_lossy() })
}

#[gpui::test]
fn open_multi_diff_editor_shows_every_file_and_collapses_them(cx: &mut TestAppContext) {
    let (dir, workbench, receiver, cx) = workbench_in("multi-diff-open", cx);
    let (a, b, c) = (file_uri(&dir.join("a.txt")), file_uri(&dir.join("b.txt")), file_uri(&dir.join("c.txt")));
    let options = json!({
        "multiDiffSourceUri": { "scheme": "scm-history-item", "path": "/repo/1..2" },
        "title": "Changes",
        "resources": [{ "originalUri": a, "modifiedUri": b }, { "modifiedUri": c }],
    });
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("_workbench.openMultiDiffEditor", vec![options.clone()], window, cx)));
    cx.run_until_parked();

    let (names, rows, added) = workbench.read_with(cx, |workbench, _| {
        let names: Vec<String> = workbench.editors.editors.iter().map(|editor| workbench.editor_name(editor)).collect();
        let input = workbench.editors.active_multi_diff().unwrap();
        let rows = input.files[0].rows(&workbench.editors.model("file", &dir.join("a.txt")).unwrap().document().lines, 5, true);
        (names, rows, input.files[1].status())
    });
    assert_eq!(names, ["Changes (2 files)"]);
    let line = |original, modified| Row::Line { original, modified };
    assert_eq!(rows, [line(Some(0), Some(0)), line(Some(1), Some(1)), line(Some(2), Some(2)), line(None, Some(3)), line(Some(3), Some(4))]);
    assert_eq!(added, Some("A"));

    // mainThreadEditorTabs.ts: a MultiDiffEditorInput tab lists the files with both sides.
    let opened = receiver.try_iter().filter(|message| message.method == "$acceptTabOperation").map(|message| message.args[0].clone()).filter(|operation| operation["kind"] == 0).last().unwrap();
    assert_eq!(opened["tabDto"]["input"]["kind"], 11);
    assert_eq!(opened["tabDto"]["input"]["diffEditors"].as_array().unwrap().len(), 1);
    assert_eq!(opened["tabDto"]["input"]["diffEditors"][0]["modified"]["path"], b["path"]);

    // The same source activates the open editor.
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("_workbench.openMultiDiffEditor", vec![options], window, cx)));
    cx.run_until_parked();
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.editors.editors.len()), 1);

    let collapsed = |workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active_multi_diff().unwrap().files.iter().map(|file| file.collapsed).collect::<Vec<_>>());
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("multiDiffEditor.collapseAll", Vec::new(), window, cx)));
    cx.run_until_parked();
    assert_eq!(collapsed(&workbench, cx), [true, true]);
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("multiDiffEditor.expandAll", Vec::new(), window, cx)));
    cx.run_until_parked();
    assert_eq!(collapsed(&workbench, cx), [false, false]);

    // actions.ts: the editor title's items read these keys.
    let context = workbench.update_in(cx, |workbench, window, _| workbench.key_context(window));
    assert_eq!((&context["activeEditor"], &context["multiDiffEditorAllCollapsed"]), (&json!("multiDiffEditor"), &json!(false)));

    // Next Change walks b.txt's changes, then the added file's, and wraps.
    let mut cursors = Vec::new();
    for _ in 0..4 {
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("multiDiffEditor.goToNextChange", Vec::new(), window, cx)));
        cursors.push(workbench.read_with(cx, |workbench, _| workbench.editors.active_multi_diff().unwrap().cursor));
    }
    assert_eq!(cursors, [Some((0, 2)), Some((0, 4)), Some((1, 1)), Some((0, 2))]);
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("multiDiffEditor.goToPreviousChange", Vec::new(), window, cx)));
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.editors.active_multi_diff().unwrap().cursor), Some((1, 1)));

    // The models stay open while the editor shows them.
    workbench.update(cx, |workbench, _| workbench.editors.release_models(std::time::Instant::now() + std::time::Duration::from_secs(3600)));
    assert!(workbench.read_with(cx, |workbench, _| workbench.editors.model("file", &dir.join("c.txt")).is_some()));

    // Open File opens the modified file.
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("multiDiffEditor.goToFile", vec![c], window, cx)));
    cx.run_until_parked();
    let active = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().name());
    assert_eq!(active, "c.txt");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn a_file_that_does_not_load_shows_the_binary_placeholder_and_reveal_scrolls_to_its_line(cx: &mut TestAppContext) {
    let (dir, workbench, _receiver, cx) = workbench_in("multi-diff-reveal", cx);
    let lines: Vec<String> = (1..=200).map(|line| format!("line {line}")).collect();
    std::fs::write(dir.join("long-a.txt"), lines.join("\n")).unwrap();
    let mut changed = lines.clone();
    changed[149] = "changed".into();
    std::fs::write(dir.join("long-b.txt"), changed.join("\n")).unwrap();
    let missing = file_uri(&dir.join("missing.bin"));
    let long_b = file_uri(&dir.join("long-b.txt"));
    let options = json!({
        "title": "Commit",
        "resources": [{ "originalUri": missing, "modifiedUri": missing }, { "originalUri": file_uri(&dir.join("long-a.txt")), "modifiedUri": long_b }],
        "reveal": { "modifiedUri": long_b, "range": { "startLineNumber": 150, "startColumn": 1, "endLineNumber": 150, "endColumn": 1 } },
    });
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("_workbench.openMultiDiffEditor", vec![options], window, cx)));
    cx.run_until_parked();
    let (unloadable, cursor, scroll_top) = workbench.read_with(cx, |workbench, _| {
        let input = workbench.editors.active_multi_diff().unwrap();
        (input.files.iter().map(|file| file.unloadable).collect::<Vec<_>>(), input.cursor, input.scroll_top)
    });
    assert_eq!(unloadable, [true, false]);
    assert_eq!(cursor, Some((1, 150)));
    // Line 150 sits below the binary file (header 40, placeholder 100), so the list scrolls past it.
    assert!(scroll_top > 140., "scrolled to {scroll_top}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn a_history_item_source_lists_its_changes_and_the_file_list_toggles(cx: &mut TestAppContext) {
    let (dir, workbench, receiver, cx) = workbench_in("multi-diff-history", cx);
    let (a, b) = (file_uri(&dir.join("a.txt")), file_uri(&dir.join("b.txt")));
    let query = json!({ "repositoryId": "scm3", "historyItemId": "abc", "historyItemParentId": "def" }).to_string();
    let options = json!({ "title": "abc - subject", "multiDiffSourceUri": { "scheme": "scm-history-item", "path": "/repo", "query": query } });
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("_workbench.openMultiDiffEditor", vec![options], window, cx)));
    cx.run_until_parked();
    // ScmHistoryItemResolver: the repository's history provider answers with the item's changes.
    let call = receiver.try_iter().find(|message| message.method == "$provideHistoryItemChanges").unwrap();
    assert_eq!(call.args, [json!(3), json!("abc"), json!("def")]);
    drop(call.reply.unwrap().send(Ok(json!([{ "uri": b, "originalUri": a, "modifiedUri": b }]))));
    cx.run_until_parked();
    let name = workbench.read_with(cx, |workbench, _| workbench.editor_name(&workbench.editors.editors[0]));
    assert_eq!(name, "abc - subject (1 file)");

    // The file list shows with `multiDiffEditor.experimental.fileList` until Toggle Files hides it.
    let visible = |workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext| workbench.update_in(cx, |workbench, window, _| workbench.key_context(window)["multiDiffEditorFileListVisible"].clone());
    assert_eq!(visible(&workbench, cx), json!(false));
    workbench.update(cx, |workbench, _| workbench.update_setting("multiDiffEditor.experimental.fileList", json!(true)));
    assert_eq!(visible(&workbench, cx), json!(true));
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("multiDiffEditor.toggleFileList", Vec::new(), window, cx)));
    assert_eq!(visible(&workbench, cx), json!(false));
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("multiDiffEditor.focusFileList", Vec::new(), window, cx)));
    assert_eq!(visible(&workbench, cx), json!(true));
    cx.run_until_parked();

    // The layout debug state opens as a jsonl model.
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("multiDiffEditor.openLayoutDebug", Vec::new(), window, cx)));
    let line = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().lines[0].clone());
    let state: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(state["items"][0]["collapsed"], false);
    std::fs::remove_dir_all(dir).unwrap();
}
