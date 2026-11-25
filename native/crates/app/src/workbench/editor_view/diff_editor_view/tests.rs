use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::host_sync::{HostMessage, HostSync};
use crate::languages::Languages;
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
    let (sender, receiver) = channel();
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(sender), cx));
    (dir, workbench, receiver, cx)
}

fn file_uri(path: &Path) -> Value {
    json!({ "scheme": "file", "path": path.to_string_lossy() })
}

fn tab_operations(receiver: &Receiver<HostMessage>) -> Vec<Value> {
    receiver.try_iter().filter(|message| message.method == "$acceptTabOperation").map(|message| message.args[0].clone()).collect()
}

#[gpui::test]
fn workbench_diff_opens_a_diff_editor_of_two_files(cx: &mut TestAppContext) {
    let (dir, workbench, receiver, cx) = workbench_in("diff-open", cx);
    let (a, b) = (file_uri(&dir.join("a.txt")), file_uri(&dir.join("b.txt")));
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("_workbench.diff", vec![a.clone(), b.clone(), json!("DIFF")], window, cx)));
    cx.run_until_parked();

    let (names, active, changes) = workbench.read_with(cx, |workbench, _| {
        let names: Vec<String> = workbench.editors.editors.iter().map(|editor| workbench.editor_name(editor)).collect();
        let active = workbench.editors.active().unwrap().document().name();
        let diff = workbench.editors.active_diff().unwrap().diff.as_ref().unwrap();
        let changes: Vec<String> = diff.diff.changes.iter().map(|change| format!("{}->{}", change.original, change.modified)).collect();
        (names, active, changes)
    });
    assert_eq!(names, ["DIFF"]);
    // The code editor edits the modified side.
    assert_eq!(active, "b.txt");
    assert_eq!(changes, ["[2,3)->[2,3)", "[4,4)->[4,5)"]);

    // mainThreadEditorTabs.ts: a TextDiffInput tab, both resources in its id.
    let opened = tab_operations(&receiver).into_iter().rfind(|operation| operation["kind"] == 0).unwrap();
    let tab = &opened["tabDto"];
    assert_eq!(tab["input"]["kind"], 2);
    assert_eq!(tab["input"]["original"]["path"], a["path"]);
    assert_eq!(tab["input"]["modified"]["path"], b["path"]);
    assert_eq!(tab["label"], "DIFF");
    let (a_uri, b_uri) = (format!("file://{}", dir.join("a.txt").display()), format!("file://{}", dir.join("b.txt").display()));
    assert_eq!(tab["id"], format!("0~default-workbench.editors.diffEditorInput-{b_uri}-{a_uri} "));

    // The same sides activate the open diff editor; another label is not another editor.
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("_workbench.diff", vec![a.clone(), b.clone()], window, cx)));
    cx.run_until_parked();
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.editors.editors.len()), 1);

    // Typing into the modified side recomputes the diff on the next frame.
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.editors.active_mut().unwrap().move_to(crate::editor::Position::new(1, 0), false);
        window.focus(&workbench.editor_focus, cx);
    });
    cx.simulate_input("x");
    cx.run_until_parked();
    let (dirty, changes) = workbench.read_with(cx, |workbench, _| {
        let diff = workbench.editors.active_diff().unwrap().diff.as_ref().unwrap();
        assert_eq!(diff.versions.1, workbench.editors.tab_model(0).text_model().version_id);
        (workbench.editors.tab_model(0).is_dirty(), diff.diff.changes.iter().map(|change| format!("{}->{}", change.original, change.modified)).collect::<Vec<_>>())
    });
    assert!(dirty);
    assert_eq!(changes, ["[2,3)->[2,3)", "[4,4)->[4,5)"]);
    let text = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().lines[1].clone());
    assert_eq!(text, "xTWO");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn compute_document_diff_answers_with_line_ranges_and_inner_changes(cx: &mut TestAppContext) {
    let (dir, workbench, _receiver, cx) = workbench_in("diff-document", cx);
    let (a, b) = (file_uri(&dir.join("a.txt")), file_uri(&dir.join("b.txt")));
    // Without models the editor worker has nothing to diff.
    let missing = workbench.update_in(cx, |workbench, window, cx| workbench.host_request("MainThreadDocumentDiff", "$computeDocumentDiff", &[a.clone(), b.clone(), json!(false), json!(0), json!(false)], window, cx));
    assert_eq!(missing, Ok(Value::Null));
    for uri in [&a, &b] {
        workbench.update_in(cx, |workbench, window, cx| workbench.host_request("MainThreadDocuments", "$tryOpenDocument", &[uri.clone()], window, cx)).unwrap();
    }
    let result = workbench.update_in(cx, |workbench, window, cx| workbench.host_request("MainThreadDocumentDiff", "$computeDocumentDiff", &[a, b, json!(false), json!(0), json!(false)], window, cx)).unwrap();
    assert_eq!(result["identical"], false);
    assert_eq!(result["changes"][0]["originalRange"], json!({ "startLineNumber": 2, "startColumn": 1, "endLineNumber": 3, "endColumn": 1 }));
    assert_eq!(result["changes"][0]["innerChanges"][0]["modifiedRange"], json!({ "startLineNumber": 2, "startColumn": 1, "endLineNumber": 2, "endColumn": 4 }));
    assert_eq!(result["changes"][1]["modifiedRange"], json!({ "startLineNumber": 4, "startColumn": 1, "endLineNumber": 5, "endColumn": 1 }));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn swap_sides_shows_the_modified_file_as_the_original(cx: &mut TestAppContext) {
    let (dir, workbench, _receiver, cx) = workbench_in("diff-swap", cx);
    let (a, b) = (file_uri(&dir.join("a.txt")), file_uri(&dir.join("b.txt")));
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("_workbench.diff", vec![a, b], window, cx)));
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.compareEditor.swapSides", Vec::new(), window, cx)));
    cx.run_until_parked();

    let (active, changes, zones) = workbench.read_with(cx, |workbench, _| {
        let diff = workbench.editors.active_diff().unwrap();
        let changes: Vec<String> = diff.diff.as_ref().unwrap().diff.changes.iter().map(|change| format!("{}->{}", change.original, change.modified)).collect();
        let zones = workbench.editors.model("file", &dir.join("b.txt")).unwrap().view_zones.len();
        (workbench.editors.active().unwrap().document().name(), changes, zones)
    });
    assert_eq!(active, "a.txt");
    assert_eq!(changes, ["[2,3)->[2,3)", "[4,5)->[4,4)"]);
    // The zones the diff placed in the former modified editor are gone.
    assert_eq!(zones, 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn workbench_diff_opens_in_the_given_column(cx: &mut TestAppContext) {
    let (dir, workbench, _receiver, cx) = workbench_in("diff-column", cx);
    let (a, b) = (file_uri(&dir.join("a.txt")), file_uri(&dir.join("b.txt")));
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("_workbench.diff", vec![a, b, json!("DIFF"), json!([1, { "preview": false }])], window, cx)));
    cx.run_until_parked();
    let (columns, active, diff_shown) = workbench.read_with(cx, |workbench, _| (workbench.editors.column_ids(), workbench.editors.id, workbench.editors.active_diff().is_some()));
    assert_eq!((columns.len(), columns[1], diff_shown), (2, active, true), "ViewColumn.Two is the second group");
    std::fs::remove_dir_all(dir).unwrap();
}
