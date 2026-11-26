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
use crate::workbench::view_panes::{OUTLINE_VIEW, TIMELINE_VIEW};

#[gpui::test]
fn focus_view_commands_open_the_explorer_pane_and_focus_it(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("vsrustium-focus-view-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let workspace = config::workspace::Workspace::folder(dir.clone());
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(workspace.clone()))));
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx));
    cx.update(|window, cx| {
        window.activate_window();
        drop(window.draw(cx));
    });
    cx.run_until_parked();

    for view in [TIMELINE_VIEW, OUTLINE_VIEW] {
        workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command(&format!("{view}.focus"), vec![], window, cx));
        cx.run_until_parked();
        workbench.update_in(cx, |workbench, window, _| {
            assert!(workbench.explorer_pane_expanded(view), "{view} expanded");
            let focus = if view == TIMELINE_VIEW { &workbench.timeline.focus } else { &workbench.outline_pane.focus };
            assert!(focus.is_focused(window), "{view} focused");
        });
    }

    let reference: serde_json::Value = serde_json::from_str(include_str!("../../command_palette/reference-commands.json")).unwrap();
    let built_in = ["workbench.explorer.fileView.focus", "workbench.view.search.focus", "workbench.scm.focus", "workbench.view.testing.focus", "workbench.views.extensions.installed.focus", "workbench.action.problems.focus"];
    workbench.read_with(cx, |workbench, _| {
        for id in built_in {
            assert!(reference["commandPalette"].as_array().unwrap().iter().any(|item| item["id"] == id), "{id} is a reference command");
            assert!(workbench.commands.get(id).is_some(), "{id} registered");
        }
    });
}
