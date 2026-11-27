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
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\nfour").unwrap();
    std::fs::write(dir.join("b.txt"), "one\nTWO\nthree\ninserted\nfour").unwrap();
    let (sender, receiver) = channel();
    let settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(sender), cx));
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
fn first_diff_opened_keeps_a_visible_change_in_place(cx: &mut TestAppContext) {
    let (dir, workbench, _receiver, cx) = workbench_in("diff-first-reveal", cx);
    let original = "1\n\n3\n4\n\n6\n\t7\n    8\n9\n";
    std::fs::write(dir.join("a.txt"), original).unwrap();
    std::fs::write(dir.join("b.txt"), format!("{original}changed line\n")).unwrap();
    let (a, b) = (file_uri(&dir.join("a.txt")), file_uri(&dir.join("b.txt")));
    workbench.update(cx, |workbench, _| workbench.settings.lock().unwrap().update_value_derived("diffEditor.renderSideBySide", Some(&json!(false))).unwrap());
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("_workbench.diff", vec![a, b], window, cx)));
    cx.run_until_parked();
    // textDiffEditor.ts `revealFirstDiff` centers the change only when it is outside the laid-out viewport.
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().scroll.top), 0.);
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
fn hide_unchanged_regions_collapses_both_sides(cx: &mut TestAppContext) {
    let (dir, workbench, _receiver, cx) = workbench_in("diff-unchanged", cx);
    let lines: Vec<String> = (1..=100).map(|line| format!("line {line}")).collect();
    std::fs::write(dir.join("a.txt"), lines.join("\n")).unwrap();
    let mut changed = lines.clone();
    changed[49] = "changed".into();
    std::fs::write(dir.join("b.txt"), changed.join("\n")).unwrap();
    let (a, b) = (file_uri(&dir.join("a.txt")), file_uri(&dir.join("b.txt")));
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.update_setting("diffEditor.hideUnchangedRegions.enabled", json!(true));
        drop(workbench.execute_command("_workbench.diff", vec![a, b], window, cx));
    });
    cx.run_until_parked();
    let state = |workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext| {
        workbench.read_with(cx, |workbench, _| {
            let diff = workbench.editors.active_diff().unwrap();
            let hidden = workbench.editors.active().unwrap().folding.hidden().to_vec();
            (hidden, diff.placed_unchanged.len())
        })
    };
    // Line 50 changed: lines 1-46 and 54-100 hide, with 3 context lines each side and a placeholder above each.
    assert_eq!(state(&workbench, cx), (vec![0..46, 53..100], 2));

    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("diffEditor.showAllUnchangedRegions", Vec::new(), window, cx)));
    cx.run_until_parked();
    assert_eq!(state(&workbench, cx), (vec![], 0));

    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("diffEditor.collapseAllUnchangedRegions", Vec::new(), window, cx)));
    cx.run_until_parked();
    assert_eq!(state(&workbench, cx).1, 2);

    // A cursor moved onto a hidden line reveals it from the closer side.
    workbench.update_in(cx, |workbench, _, cx| {
        workbench.editors.active_mut().unwrap().move_to(crate::editor::Position::new(57, 0), false);
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(state(&workbench, cx).0, vec![0..46, 58..100]);

    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("diffEditor.toggleCollapseUnchangedRegions", Vec::new(), window, cx)));
    cx.run_until_parked();
    assert_eq!(state(&workbench, cx), (vec![], 0));
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

/// diffEditorHelper.ts: the stopped-early prompt, whose Remove Limit sets `diffEditor.maxComputationTime` to 0.
#[gpui::test]
fn a_diff_stopped_early_offers_to_remove_the_limit(cx: &mut TestAppContext) {
    let (dir, workbench, _receiver, cx) = workbench_in("diff-quit-early", cx);
    workbench.update(cx, |workbench, _| workbench.prompt_diff_quit_early(5000));
    let (message, action) = workbench.read_with(cx, |workbench, _| {
        let item = workbench.notifications.model.items.first().unwrap();
        (item.message.text(), item.primary[0].clone())
    });
    assert_eq!(message, "The diff algorithm was stopped early (after 5000 ms.)");
    assert_eq!(action.label, "Remove Limit");
    let crate::notifications::Run::Command(command, args) = action.run else { panic!("a command") };
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(&command, args, window, cx)));
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.setting("diffEditor.maxComputationTime"), json!(0)));
    std::fs::remove_dir_all(dir).unwrap();
}

/// diffEditorHelper.ts `hintWhitespace`: only a diff of trimmed whitespace shows the button.
#[gpui::test]
fn a_whitespace_only_diff_offers_to_show_whitespace(cx: &mut TestAppContext) {
    let (dir, workbench, _receiver, cx) = workbench_in("diff-whitespace", cx);
    std::fs::write(dir.join("c.txt"), "one  \ntwo\nthree\nfour").unwrap();
    let hint_after = |other: &str, cx: &mut VisualTestContext| {
        let (a, other) = (file_uri(&dir.join("a.txt")), file_uri(&dir.join(other)));
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("_workbench.diff", vec![a, other], window, cx)));
        cx.run_until_parked();
        workbench.update(cx, |workbench, cx| {
            workbench.update_diff_editor();
            workbench.whitespace_hint_widget(0., cx).is_some()
        })
    };
    assert!(!hint_after("b.txt", cx), "a diff with changes");
    assert!(hint_after("c.txt", cx), "trailing whitespace only");
    std::fs::remove_dir_all(dir).unwrap();
}
