use std::path::Path;

use exthost::scan::scan_extension;
use gpui::TestAppContext;

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

#[gpui::test]
fn a_shown_model_loses_its_ls_and_ps_and_an_undo_brings_them_back(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-unusual-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.txt");
    std::fs::write(&file, "a\u{2028}b\nc\u{2029}").unwrap();
    let host = HostSync::new(std::sync::mpsc::channel().0);
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), host, cx);
        workbench.skip_dialogs = true;
        workbench.open_file(file.clone(), true, window, cx);
        workbench
    });
    cx.run_until_parked();
    let text = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().lines.join("\n"));
    assert_eq!(text(cx), "ab\nc");

    workbench.update(cx, |workbench, cx| {
        workbench.editors.active_mut().unwrap().undo();
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(text(cx), "a\u{2028}b\nc\u{2029}");
    std::fs::remove_dir_all(&dir).unwrap();
}
