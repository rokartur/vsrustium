use std::path::{Path, PathBuf};

use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::editor::{Position, Selection};
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

fn workbench_in<'a>(name: &str, cx: &'a mut TestAppContext) -> (PathBuf, gpui::Entity<Workbench>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("far.js"), "one\ntwo\nthree").unwrap();
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx));
    (dir, workbench, cx)
}

fn open(workbench: &gpui::Entity<Workbench>, args: Vec<Value>, cx: &mut VisualTestContext) -> Result<Value, Value> {
    workbench.update_in(cx, |workbench, window, cx| workbench.api_open_editor(&args, window, cx))
}

fn file_uri(path: &Path) -> Value {
    json!({ "scheme": "file", "path": path.to_string_lossy() })
}

#[gpui::test]
fn workbench_open_shows_files_and_untitled_files_and_rejects_other_strings(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_in("workbench-open", cx);
    let selection = json!({ "startLineNumber": 2, "startColumn": 1, "endLineNumber": 2, "endColumn": 4 });
    assert_eq!(open(&workbench, vec![file_uri(&dir.join("far.js")), json!([1, { "selection": selection }])], cx), Ok(Value::Null));
    let selection = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().selection());
    assert_eq!(selection, Selection { anchor: Position { line: 1, column: 0 }, head: Position { line: 1, column: 3 } });

    // commands.test.ts #138925: an untitled resource with a path is dirty from the start.
    let untitled = json!({ "scheme": "untitled", "path": dir.join("untitled-file.txt").to_string_lossy() });
    assert_eq!(open(&workbench, vec![untitled], cx), Ok(Value::Null));
    let (name, dirty) = workbench.read_with(cx, |workbench, _| {
        let editor = workbench.editors.active().unwrap();
        (editor.document().name(), editor.is_dirty())
    });
    assert_eq!((name.as_str(), dirty), ("untitled-file.txt", true));
    // untitledTextEditorModel.ts `revert`: emptied and clean, never read from or written to its file; the input closes.
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.files.revert", Vec::new(), window, cx)));
    let names = workbench.read_with(cx, |workbench, _| workbench.editors.editors.iter().map(|editor| editor.document().name()).collect::<Vec<_>>());
    assert_eq!(names, ["far.js"]);
    assert!(!dir.join("untitled-file.txt").exists());

    assert!(open(&workbench, vec![json!("file:///some/path/not/http")], cx).is_err());
    assert_eq!(open(&workbench, vec![json!({ "scheme": "command", "path": "workbench.action.quit" })], cx), Ok(Value::Null));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn view_columns_pick_or_add_groups_in_grid_order(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_in("view-columns", cx);
    let show = |position: i64, cx: &mut VisualTestContext| {
        let uri = file_uri(&dir.join("far.js"));
        workbench.update_in(cx, |workbench, window, cx| workbench.try_show_text_document(&uri, &json!({ "position": position }), window, cx)).unwrap();
        workbench.read_with(cx, |workbench, _| (workbench.groups_in_grid_order().len(), workbench.groups_in_grid_order().iter().position(|group| *group == workbench.editors.id).unwrap()))
    };
    // window.test.ts `editor, assign and check view columns`: One, Two and Three are the first three groups.
    assert_eq!(show(0, cx), (1, 0));
    assert_eq!(show(1, cx), (2, 1));
    assert_eq!(show(2, cx), (3, 2));
    assert_eq!(show(0, cx), (3, 0));
    // ACTIVE keeps the group, SIDE takes the neighbour to the right.
    assert_eq!(show(-1, cx), (3, 0));
    assert_eq!(show(-2, cx), (3, 1));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn workbench_open_reads_a_provider_resource_through_the_host(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let theme = load_theme(&[scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap()], "Dark Modern").unwrap();
    let (sender, messages) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(sender), cx));
    let uri = json!({ "scheme": "fake-fs", "path": "/a/b.txt" });
    let outcome = workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("_workbench.open", vec![uri.clone()], window, cx));
    let crate::commands::Outcome::Pending(mut opened) = outcome else { panic!("the open waits for the provider") };
    let asked = messages.try_iter().find(|message| message.method == "resolveTextContent").expect("asked the host");
    assert_eq!(asked.args, [uri]);
    let _ = asked.reply.expect("a call").send(Ok(json!(b"from the provider".to_vec())));
    cx.run_until_parked();
    assert_eq!(opened.try_recv().unwrap(), Some(Ok(Value::Null)));
    let text = workbench.read_with(cx, |workbench, _| workbench.editors.active().map(|editor| editor.document().lines.join("\n")));
    assert_eq!(text.as_deref(), Some("from the provider"));
}

#[gpui::test]
fn workbench_open_opens_in_the_given_column(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_in("workbench-open-column", cx);
    let uri = file_uri(&dir.join("far.js"));
    let outcome = workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("_workbench.open", vec![uri, json!([1, {}])], window, cx));
    assert!(matches!(outcome, crate::commands::Outcome::Ready(Ok(_))));
    let (columns, active) = workbench.read_with(cx, |workbench, _| (workbench.editors.column_ids(), workbench.editors.id));
    assert_eq!((columns.len(), columns[1]), (2, active), "ViewColumn.Two is the second group");
    std::fs::remove_dir_all(dir).unwrap();
}
