use std::path::Path;

use exthost::scan::scan_extension;
use gpui::TestAppContext;

use crate::editor::MODAL_GROUP;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

#[gpui::test]
fn keybindings_json_opens_in_the_modal_group_and_escape_closes_it(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-modal-group-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.txt");
    std::fs::write(&file, "hello").unwrap();
    let keybindings = dir.join("keybindings.json");
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), keybindings.clone(), cx);
        workbench.open_file(file.clone(), true, window, cx);
        workbench
    });
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.openGlobalKeybindingsFile", vec![], window, cx)));
    cx.run_until_parked();
    workbench.read_with(cx, |workbench, _| {
        assert_eq!(workbench.editors.id, MODAL_GROUP);
        assert_eq!(workbench.editors.active().unwrap().document().path, keybindings);
        // The main part keeps its one group and no grid.
        assert!(!workbench.editors.has_several_groups());
        assert_eq!(workbench.editors.group_tabs(0).0.len(), 1);
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    workbench.read_with(cx, |workbench, _| {
        assert!(!workbench.modal_group_open());
        assert_eq!(workbench.editors.id, 0);
        assert_eq!(workbench.editors.active().unwrap().document().path, file);
    });
}

#[gpui::test]
fn use_modal_off_opens_keybindings_json_in_the_active_group(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-modal-group-off-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let keybindings = dir.join("keybindings.json");
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        std::fs::write(dir.join("settings.json"), r#"{ "workbench.editor.useModal": "off" }"#).unwrap();
        let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.clone(), None);
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), std::sync::Arc::new(std::sync::Mutex::new(settings)), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), keybindings.clone(), cx);
        workbench
    });
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.openGlobalKeybindingsFile", vec![], window, cx)));
    cx.run_until_parked();
    workbench.read_with(cx, |workbench, _| {
        assert_eq!(workbench.editors.id, 0);
        assert!(!workbench.modal_group_open());
    });
}

/// preferencesService.ts `openGlobalKeybindingSettings(true)` with `workbench.settings.openDefaultKeybindings`: the
/// default keybindings in the active group, keybindings.json in a group added to its right, not in the modal group.
#[gpui::test]
fn open_default_keybindings_opens_keybindings_json_beside_the_defaults(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-default-keybindings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let keybindings = dir.join("keybindings.json");
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        std::fs::write(dir.join("settings.json"), r#"{ "workbench.settings.openDefaultKeybindings": true }"#).unwrap();
        let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.clone(), None);
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), std::sync::Arc::new(std::sync::Mutex::new(settings)), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), keybindings.clone(), cx);
        workbench
    });
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.openGlobalKeybindingsFile", vec![], window, cx)));
    cx.run_until_parked();
    workbench.read_with(cx, |workbench, _| {
        assert!(!workbench.modal_group_open());
        assert_ne!(workbench.editors.id, 0);
        assert_eq!(workbench.editors.active().unwrap().document().path, keybindings);
        let (defaults, active) = workbench.editors.group_tabs(0);
        assert_eq!(defaults[active.unwrap()].document().path, Path::new("//defaultsettings/keybindings.json"));
    });
}
