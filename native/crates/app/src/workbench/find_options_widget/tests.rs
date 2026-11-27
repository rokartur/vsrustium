use std::path::Path;

use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

fn shown(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> (bool, bool) {
    workbench.read_with(cx, |workbench, _| (workbench.find.options_widget.shown, workbench.find.options_widget.flash.is_some()))
}

/// findOptionsWidget.ts and multicursor.ts `_highlightFindOptions`: a hidden find widget shows the options widget for
/// an option change (a toggle, a session's overrides) and for a session's next match; a shown one flashes.
#[gpui::test]
fn find_options_show_or_flash_when_highlighted(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-find-options-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.txt");
    std::fs::write(&file, "the cat\nthe dog\nthe end").unwrap();
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.open_file(file.clone(), true, window, cx);
        workbench
    });
    cx.run_until_parked();
    let run = |command: &str, cx: &mut VisualTestContext| workbench.update_in(cx, |workbench, window, cx| workbench.execute_command(command, vec![], window, cx));

    run("editor.action.addSelectionToNextFindMatch", cx);
    assert_eq!(shown(&workbench, cx), (true, false), "an empty cursor's session overrides whole word and case");
    run("editor.action.addSelectionToNextFindMatch", cx);
    assert_eq!(shown(&workbench, cx), (true, false));
    cx.executor().advance_clock(std::time::Duration::from_millis(2100));
    cx.run_until_parked();
    assert_eq!(shown(&workbench, cx), (false, false), "hidden 2 s later");

    run("toggleFindCaseSensitive", cx);
    assert_eq!(shown(&workbench, cx), (true, false));
    cx.executor().advance_clock(std::time::Duration::from_millis(2100));
    cx.run_until_parked();

    run("actions.find", cx);
    run("editor.action.addSelectionToNextFindMatch", cx);
    assert_eq!(shown(&workbench, cx), (false, true), "a shown find widget flashes its options instead");
    std::fs::remove_dir_all(dir).unwrap();
}
