use std::path::Path;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use config::workspace::Workspace;
use exthost::scan::scan_extension;
use gpui::TestAppContext;
use serde_json::json;

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

#[gpui::test]
fn folders_added_and_removed_in_a_workspace_reach_its_file_the_explorer_settings_and_host(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let theme = load_theme(&[scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap()], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-workspace-editing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for folder in ["User", "a", "b/.vscode", "c"] {
        std::fs::create_dir_all(dir.join(folder)).unwrap();
    }
    std::fs::write(dir.join("b/.vscode/settings.json"), r#"{ "files.trimFinalNewlines": true }"#).unwrap();
    let file = dir.join("w.code-workspace");
    std::fs::write(&file, "{\n\t\"folders\": [\n\t\t{\n\t\t\t\"path\": \"a\"\n\t\t}\n\t]\n}").unwrap();
    let workspace = Workspace::open(file.clone());
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(workspace.clone()))));
    let (sender, messages) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings.clone(), HostSync::new(sender), cx));
    cx.run_until_parked();
    _ = messages.try_iter().count();
    let folder_names = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.workspace().unwrap().folders.iter().map(|folder| folder.name.clone()).collect::<Vec<_>>());

    let added = workbench.update_in(cx, |workbench, window, cx| workbench.update_workspace_folders(1, 0, vec![(dir.join("b/"), Some("B".into())), (dir.join("a"), None)], window, cx));
    assert_eq!(added, Ok(()));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\n\t\"folders\": [\n\t\t{\n\t\t\t\"path\": \"a\"\n\t\t},\n\t\t{\n\t\t\t\"name\": \"B\",\n\t\t\t\"path\": \"b\"\n\t\t}\n\t]\n}");
    assert_eq!(folder_names(cx), ["a", "B"]);
    let overrides = config::configuration::Overrides { resource: Some(&dir.join("b/x.txt")), ..Default::default() };
    assert_eq!(settings.lock().unwrap().get_value(Some("files.trimFinalNewlines"), &overrides), Some(json!(true)), "the added folder's settings apply");
    let sent: Vec<_> = messages.try_iter().filter(|message| message.method == "$acceptWorkspaceData").collect();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].args[0]["folders"][1]["name"], "B");

    let removed = workbench.update_in(cx, |workbench, window, cx| workbench.update_workspace_folders(1, 1, Vec::new(), window, cx));
    assert_eq!(removed, Ok(()));
    assert_eq!(folder_names(cx), ["a"]);
    assert_eq!(settings.lock().unwrap().get_value(Some("files.trimFinalNewlines"), &overrides), Some(json!(false)));

    std::fs::write(&file, r#"{ "folders": [{ "path": "a" }, { "path": "c" }] }"#).unwrap();
    workbench.update(cx, |workbench, cx| workbench.reload_changed_settings_files(cx));
    assert_eq!(folder_names(cx), ["a", "c"], "an edit of the workspace file on disk changes the folders");
}
