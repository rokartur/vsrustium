use std::path::Path;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::quick_input::Event;
use crate::theme::load_theme;
use crate::workbench::Workbench;

fn empty_window(cx: &mut TestAppContext) -> (gpui::Entity<Workbench>, gpui::AnyWindowHandle) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), std::env::temp_dir().join("vsrustium-switch-window-user"), None)));
    let (workbench, vcx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root, None, Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx));
    vcx.run_until_parked();
    (workbench, vcx.update(|window, _| window.window_handle()))
}

/// windowActions.ts `BaseSwitchWindow`: each window with its close button, the next window active, and a row's
/// button closes that window.
#[gpui::test]
fn switch_window_lists_the_windows_and_closes_one_from_its_row(cx: &mut TestAppContext) {
    empty_window(cx);
    let (current, current_window) = empty_window(cx);
    let (_, last_window) = empty_window(cx);
    let vcx = &mut VisualTestContext::from_window(current_window, cx);
    current.update_in(vcx, |workbench, window, cx| _ = workbench.execute_command("workbench.action.switchWindow", vec![], window, cx));
    vcx.run_until_parked();
    let (rows, active, id) = current.read_with(vcx, |workbench, _| {
        let input = workbench.quick_inputs.current().expect("the window picker");
        let rows: Vec<(Option<String>, Option<String>)> = input.order.iter().map(|row| &input.rows[*row].item).map(|item| (item.description.clone(), item.buttons[0].codicon.clone())).collect();
        (rows, input.active_handles(), input.id)
    });
    assert_eq!(rows, [(None, Some("remove-close".into())), (Some("Current Window".into()), Some("window-active".into())), (None, Some("remove-close".into()))]);
    assert_eq!(active, [2]);
    current.update_in(vcx, |workbench, window, cx| workbench.switch_window_event(id, Event::TriggerItemButton(2, 0, None), window, cx));
    vcx.run_until_parked();
    let open: Vec<_> = vcx.windows().into_iter().map(|window| window.window_id()).collect();
    assert!(!open.contains(&last_window.window_id()), "the last window closed");
    assert_eq!(open.len(), 2);
}
