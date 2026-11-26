use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::extension_management::Job;
use exthost::scan::{ScannedExtension, scan_extension};
use gpui::TestAppContext;
use serde_json::json;

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;
use crate::workspace_trust::{Startup, WorkspaceTrust};

#[gpui::test]
fn trusting_the_window_starts_the_extensions_it_enables_without_a_reload(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-trust-change-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (sender, messages) = std::sync::mpsc::channel();
    let untrusting = ScannedExtension { id: "port.untrusting".into(), location: PathBuf::from("/extensions/untrusting"), description: json!({ "main": "./extension.js" }) };
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(sender), cx);
        workbench.extensions_viewlet.builtins = vec![untrusting.clone()];
        let shared = Arc::new(Mutex::new(storage::Storage::in_memory()));
        let workspace_storage = workbench.workspace_storage.clone();
        let startup = Startup { enabled: true, workspace: None, empty_window_trusted: false, workspace_storage: &mut workspace_storage.lock().unwrap() };
        workbench.workspace_trust = WorkspaceTrust::new(shared, startup);
        workbench
    });
    assert!(!workbench.read_with(cx, |workbench, _| workbench.workspace_trust.is_trusted()));
    while messages.try_recv().is_ok() {}

    workbench.update_in(cx, |workbench, window, cx| workbench.set_workspace_trust(true, window, cx));
    cx.run_until_parked();
    let sent: Vec<_> = messages.try_iter().collect();
    let delta = sent.iter().find_map(|message| match &message.extensions {
        Some((Job::Delta(to_add, to_remove), _)) => Some((to_add.iter().map(|extension| extension.id.clone()).collect::<Vec<_>>(), to_remove.clone())),
        _ => None,
    });
    assert_eq!(delta, Some((vec!["port.untrusting".to_owned()], Vec::new())), "`$deltaExtensions` adds what trust enables");
    let methods: Vec<&str> = sent.iter().map(|message| message.method).collect();
    assert!(methods.contains(&"$onDidGrantWorkspaceTrust"), "{methods:?}");
    assert!(workbench.read_with(cx, |workbench, _| workbench.workspace_trust.is_trusted()));
    std::fs::remove_dir_all(dir).unwrap();
}
