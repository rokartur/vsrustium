use std::path::Path;

use exthost::scan::scan_extension;
use gpui::TestAppContext;
use serde_json::json;

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::menus::MenuAction;
use crate::theme::load_theme;
use crate::workbench::Workbench;

#[gpui::test]
fn a_view_with_hidden_visibility_stays_out_until_toggled_from_the_views_submenu(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let npm = scan_extension(&app_root.join("extensions/npm"), true).unwrap();
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.set_tree_views(&[npm]);
        workbench
    });
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("_setContext", vec![json!("npm:showScriptExplorer"), json!(true)], window, cx)));
    let toggle = |workbench: &mut Workbench| {
        let views = workbench.views_submenu("explorer");
        views.into_iter().find_map(|action| match action {
            MenuAction::Command { id, title, enabled, checked, .. } if id == "npm.toggleVisibility" => Some((title, enabled, checked)),
            _ => None,
        })
    };
    workbench.update(cx, |workbench, _| {
        assert!(!workbench.shown_tree_views().contains(&"npm"));
        assert_eq!(toggle(workbench), Some(("NPM Scripts".to_string(), true, false)));
    });
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("npm.toggleVisibility", vec![], window, cx)));
    workbench.update(cx, |workbench, _| {
        assert!(workbench.shown_tree_views().contains(&"npm"));
        assert_eq!(toggle(workbench), Some(("NPM Scripts".to_string(), true, true)));
        let stored = workbench.profile_storage.lock().unwrap().get("workbench.explorer.views.state.hidden").map(str::to_owned);
        assert_eq!(stored.as_deref(), Some(r#"[{"id":"npm","isHidden":false}]"#));
    });
}
