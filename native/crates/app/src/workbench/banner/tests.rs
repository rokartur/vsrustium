use std::path::Path;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::TestAppContext;

use super::*;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;

/// window.ts: a folder inside the packaged app's installation folder shows the warning; the close button hides it.
#[gpui::test]
fn a_folder_in_the_installation_folder_shows_the_warning_until_closed(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("vsrustium-banner-{}", std::process::id()));
    let folder = dir.join("VSRustium.app/folder");
    std::fs::create_dir_all(&folder).unwrap();
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let theme = load_theme(&[scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap()], "Dark Modern").unwrap();
    let workspace = config::workspace::Workspace::folder(folder);
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(workspace.clone()))));
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx));
    let item = |workbench: &gpui::Entity<Workbench>, cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.banner_item().map(|item| (item.id, item.message)));
    assert_eq!(item(&workbench, cx), None, "run from the checkout the app is not installed");
    workbench.update(cx, |workbench, _| workbench.app_root = dir.join("VSRustium.app/Contents/Resources/app"));
    let install = crate::quick_input::path_label(&dir.join("VSRustium.app"), None);
    let message = format!("Files you store within the installation folder ('{install}') may be OVERWRITTEN or DELETED IRREVERSIBLY without warning at update time.");
    assert_eq!(item(&workbench, cx), Some(("appRootWarning.banner", message)));
    workbench.update_in(cx, |workbench, window, cx| {
        _ = workbench.execute_command("workbench.action.focusBanner", vec![], window, cx);
        _ = workbench.execute_command("workbench.banner.focusPreviousAction", vec![], window, cx);
        assert_eq!(workbench.banner.focused_action, Some(0), "no links: the close button");
        assert!(workbench.banner.focus.is_focused(window));
        workbench.close_banner("appRootWarning.banner", None, cx);
    });
    assert_eq!(item(&workbench, cx), None);
    std::fs::remove_dir_all(&dir).unwrap();
}
