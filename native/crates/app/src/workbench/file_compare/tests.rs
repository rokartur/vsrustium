use std::path::Path;

use exthost::scan::scan_extension;
use gpui::TestAppContext;
use serde_json::json;

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

#[gpui::test]
fn the_compare_commands_open_diff_editors(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let theme = load_theme(&[scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap()], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-file-compare-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
    std::fs::write(&a, "one\ntwo\n").unwrap();
    std::fs::write(&b, "one\nTWO\n").unwrap();
    let settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
        workbench
    });
    let uri = |path: &Path| json!({ "scheme": "file", "path": path.to_string_lossy() });
    let run = |id: &str, args: Vec<serde_json::Value>, cx: &mut gpui::VisualTestContext| {
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(id, args, window, cx)));
        cx.run_until_parked();
    };
    let active_diff = |cx: &mut gpui::VisualTestContext| {
        workbench.read_with(cx, |workbench, _| {
            let diff = workbench.editors.active_diff().expect("a diff editor");
            let name = workbench.editor_name(workbench.editors.active_tab().unwrap());
            (diff.original.scheme.clone(), diff.original.path.clone(), diff.modified.path.clone(), name)
        })
    };

    workbench.update_in(cx, |workbench, window, cx| workbench.open_file(a.clone(), true, window, cx));
    cx.simulate_input("x");
    cx.simulate_keystrokes("cmd-k d");
    cx.run_until_parked();
    assert_eq!(active_diff(cx), ("showModifications".into(), a.clone(), a.clone(), "a.txt (in file) \u{2194} a.txt".into()));
    let saved_text = workbench.read_with(cx, |workbench, _| workbench.editors.model("showModifications", &a).unwrap().document().lines.clone());
    assert_eq!(saved_text, ["one", "two", ""], "the original side is the file on disk");
    let diff_tab_dirty = workbench.read_with(cx, |workbench, _| workbench.tab_shows_dirty(workbench.editors.active_index().unwrap()));
    assert!(diff_tab_dirty, "the diff tab shows its modified side's dirty state");
    let cursors = workbench.read_with(cx, |workbench, _| {
        let file_tab = workbench.editors.editors.iter().find(|editor| editor.document().is("file", &a)).unwrap();
        let cursor = |editor: &crate::editor::OpenEditor| (editor.selections[0].start().line, editor.selections[0].start().column);
        (cursor(workbench.editors.active().unwrap()), cursor(file_tab))
    });
    assert_eq!(cursors, ((0, 0), (0, 1)), "the diff's modified editor starts at its first change; the text editor keeps its cursor");

    run("selectForCompare", vec![uri(&a)], cx);
    let selected = workbench.update_in(cx, |workbench, window, _| workbench.key_context(window)["resourceSelectedForCompare"].clone());
    assert_eq!(selected, json!(true));
    run("compareFiles", vec![uri(&b)], cx);
    assert_eq!(active_diff(cx), ("file".into(), a.clone(), b.clone(), "a.txt \u{2194} b.txt".into()));
    std::fs::remove_dir_all(dir).unwrap();
}
