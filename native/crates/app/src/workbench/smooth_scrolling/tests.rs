use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

fn workbench_with(smooth: bool, cx: &mut TestAppContext) -> (std::path::PathBuf, gpui::Entity<Workbench>, &mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-smooth-{smooth}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    std::fs::write(dir.join("User/settings.json"), format!(r#"{{ "editor.smoothScrolling": {smooth} }}"#)).unwrap();
    let file = dir.join("a.txt");
    std::fs::write(&file, (0..200).map(|line| format!("line {line}\n")).collect::<String>()).unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.open_file(file.clone(), true, window, cx);
        workbench
    });
    cx.run_until_parked();
    (dir, workbench, cx)
}

fn run_bottom(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) {
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("cursorBottom", vec![], window, cx)));
}

fn scroll_top(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> f32 {
    workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().scroll.top)
}

/// scrollable.ts `setScrollPositionSmooth`: the reveal of Cmd+Down moves the scroll position over 125 ms with
/// `editor.smoothScrolling`; a second reveal to the same target keeps the animation.
#[gpui::test]
fn a_cursor_reveal_scrolls_smoothly_with_the_setting(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_with(true, cx);
    run_bottom(&workbench, cx);
    assert_eq!(scroll_top(&workbench, cx), 0., "the animation starts from the current position");
    cx.executor().advance_clock(Duration::from_millis(40));
    let middle = scroll_top(&workbench, cx);
    run_bottom(&workbench, cx);
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    let end = scroll_top(&workbench, cx);
    assert!(0. < middle && middle < end, "{middle} between 0 and {end}");
    assert!(workbench.read_with(cx, |workbench, _| workbench.smooth_scrolling.is_none()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn a_cursor_reveal_scrolls_at_once_without_the_setting(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_with(false, cx);
    run_bottom(&workbench, cx);
    assert!(scroll_top(&workbench, cx) > 0.);
    std::fs::remove_dir_all(dir).unwrap();
}
