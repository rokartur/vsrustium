use std::path::Path;

use exthost::scan::scan_extension;
use gpui::TestAppContext;
use serde_json::json;

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

#[gpui::test]
fn one_undo_takes_back_a_workspace_edit_in_every_file(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let theme = load_theme(&[scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap()], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-bulk-undo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
    std::fs::write(&a, "alpha").unwrap();
    std::fs::write(&b, "beta").unwrap();
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
        workbench.open_file(b.clone(), true, window, cx);
        workbench.open_file(a.clone(), true, window, cx);
        window.focus(&workbench.editor_focus, cx);
        workbench
    });
    cx.run_until_parked();
    let edit = |path: &Path, text: &str| json!({ "resource": { "scheme": "file", "path": path.to_string_lossy() }, "textEdit": { "range": { "startLineNumber": 1, "startColumn": 1, "endLineNumber": 1, "endColumn": 1 }, "text": text } });
    let edits = json!([edit(&a, "1"), edit(&b, "2")]);
    workbench.update(cx, |workbench, cx| workbench.apply_workspace_text_edits(&edits, &json!([]), cx)).unwrap();
    let texts = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| [&a, &b].map(|path| workbench.editors.model("file", path).unwrap().document().lines[0].clone()));
    assert_eq!(texts(cx), ["1alpha", "2beta"]);

    let run = |id: &str, cx: &mut gpui::VisualTestContext| {
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(id, Vec::new(), window, cx)));
        cx.run_until_parked();
    };
    run("undo", cx);
    assert!(cx.has_pending_prompt(), "Would you like to undo 'Workspace Edit' across all files?");
    cx.simulate_prompt_answer("Undo in 2 Files");
    cx.run_until_parked();
    assert_eq!(texts(cx), ["alpha", "beta"]);
    run("redo", cx);
    assert_eq!(texts(cx), ["1alpha", "2beta"]);

    run("undo", cx);
    cx.simulate_prompt_answer("Undo this File");
    cx.run_until_parked();
    assert_eq!(texts(cx), ["alpha", "2beta"]);
    std::fs::remove_dir_all(dir).unwrap();
}
