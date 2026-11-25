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
    std::fs::write(dir.join("base.txt"), "one\ntwo\nthree").unwrap();
    std::fs::write(dir.join("incoming.txt"), "one\nTWO incoming\nthree").unwrap();
    std::fs::write(dir.join("current.txt"), "one\nTWO current\nthree").unwrap();
    std::fs::write(dir.join("result.txt"), "one\n<<<<<<< HEAD\nTWO current\n=======\nTWO incoming\n>>>>>>> theirs\nthree").unwrap();
    let (sender, receiver) = channel();
    let settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(sender), cx));
    (dir, workbench, receiver, cx)
}

fn file_uri(path: &Path) -> Value {
    json!({ "scheme": "file", "path": path.to_string_lossy() })
}

fn run(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, command: &str, args: Vec<Value>) {
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(command, args, window, cx)));
    cx.run_until_parked();
}

#[gpui::test]
fn open_merge_editor_resets_the_conflict_and_accepts_incoming(cx: &mut TestAppContext) {
    let (dir, workbench, receiver, cx) = workbench_in("merge-open", cx);
    let args = json!({
        "base": file_uri(&dir.join("base.txt")),
        "input1": { "uri": file_uri(&dir.join("incoming.txt")), "title": "Incoming", "description": "theirs" },
        "input2": { "uri": file_uri(&dir.join("current.txt")), "title": "Current", "detail": "HEAD" },
        "output": file_uri(&dir.join("result.txt")),
    });
    run(&workbench, cx, "_open.mergeEditor", vec![args]);

    let (name, result, unhandled) = workbench.read_with(cx, |workbench, _| {
        let name = workbench.editor_name(&workbench.editors.editors[0]);
        let result = workbench.editors.active().unwrap().document.lines.join("\n");
        (name, result, workbench.editors.active_merge().unwrap().model.as_ref().unwrap().unhandled_count())
    });
    assert_eq!(name, "Merging: result.txt");
    // mergeEditorInputModel.ts: conflict markers reset the result to base with the conflict unhandled.
    assert_eq!(result, "one\ntwo\nthree");
    assert_eq!(unhandled, 1);

    let opened = receiver.try_iter().filter(|message| message.method == "$acceptTabOperation").map(|message| message.args[0].clone()).filter(|operation| operation["kind"] == 0).last().unwrap();
    assert_eq!(opened["tabDto"]["input"]["kind"], 3);
    assert_eq!(opened["tabDto"]["input"]["result"]["path"], json!(dir.join("result.txt").to_string_lossy()));

    run(&workbench, cx, "merge.goToNextUnhandledConflict", Vec::new());
    run(&workbench, cx, "merge.toggleActiveConflictInput1", Vec::new());
    let (result, unhandled) = workbench.read_with(cx, |workbench, _| {
        let result = workbench.editors.active().unwrap().document.lines.join("\n");
        (result, workbench.editors.active_merge().unwrap().model.as_ref().unwrap().unhandled_count())
    });
    assert_eq!(result, "one\nTWO incoming\nthree");
    assert_eq!(unhandled, 0);

    run(&workbench, cx, "merge.acceptAllInput2", Vec::new());
    let result = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document.lines.join("\n"));
    assert_eq!(result, "one\nTWO incoming\nTWO current\nthree");

    run(&workbench, cx, "mergeEditor.resetResultToBaseAndAutoMerge", Vec::new());
    let result = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document.lines.join("\n"));
    assert_eq!(result, "one\ntwo\nthree");
}
