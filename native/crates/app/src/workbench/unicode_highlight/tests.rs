use std::path::Path;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::TestAppContext;

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

#[gpui::test]
fn the_banner_action_takes_the_focus_out_of_the_editor_with_the_banner(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-unicode-banner-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("many.ts");
    std::fs::write(&file, format!("let a = {}\nlet b", "x\u{430}".repeat(1100))).unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.watch_editor_focus(window, cx);
        workbench.open_file(file.clone(), true, window, cx);
        // Languages::new(&[]) knows no TypeScript; any language but plaintext keeps the ambiguous highlight on.
        workbench.editors.active_mut().unwrap().document_mut().language_id = "typescript".into();
        workbench
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    let highlights = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().highlights.len());
    assert_eq!(highlights(cx), 2);

    let link = cx.debug_bounds("unicode-banner-action").expect("the banner shows past 1000 characters");
    cx.simulate_click(link.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("unicode-banner-action").is_none(), "the highlight is off, so is the banner");
    assert_eq!(highlights(cx), 0, "onDidBlurEditorWidget");
    let focused = workbench.update_in(cx, |workbench, window, cx| workbench.editor_focus.contains_focused(window, cx));
    assert!(!focused);
    std::fs::remove_dir_all(dir).unwrap();
}
