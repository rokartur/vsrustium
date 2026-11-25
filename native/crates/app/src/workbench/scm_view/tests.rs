use std::path::Path;
use std::sync::mpsc::Receiver;

use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::host_sync::{HostMessage, HostSync};
use crate::workbench::Workbench;

fn workbench_in<'a>(name: &str, cx: &'a mut TestAppContext) -> (gpui::Entity<Workbench>, Receiver<HostMessage>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, crate::languages::Languages::new(&[]), settings, HostSync::new(sender), cx));
    (workbench, receiver, cx)
}

fn register(handle: u64, root: &str, hidden: bool) -> Vec<Value> {
    let input = json!({ "scheme": "vscode-scm", "path": format!("git/scm{handle}/input") });
    vec![json!(handle), Value::Null, json!("git"), json!("Git"), json!({ "scheme": "file", "path": root }), Value::Null, json!(hidden), input]
}

fn selections(receiver: &Receiver<HostMessage>) -> Vec<Value> {
    receiver.try_iter().filter(|message| message.method == "$setSelectedSourceControl").map(|message| message.args[0].clone()).collect()
}

#[gpui::test]
fn scm_repositories_focus_the_first_and_edit_the_focused_input(cx: &mut TestAppContext) {
    let (workbench, receiver, cx) = workbench_in("scm-repositories", cx);
    workbench.update_in(cx, |workbench, window, cx| {
        for (handle, root, hidden) in [(0, "/w/zeta", false), (1, "/w/alpha", false), (2, "/w/hidden", true)] {
            drop(workbench.handle_scm_request("$registerSourceControl", &register(handle, root, hidden), window, cx));
        }
    });
    let shown = |workbench: &Workbench| workbench.visible_scm_providers().iter().map(|provider| provider.handle).collect::<Vec<_>>();
    workbench.read_with(cx, |workbench, _| {
        assert_eq!(shown(workbench), [0, 1]);
        assert_eq!(workbench.scm.focused, Some(0));
    });
    assert_eq!(selections(&receiver), [json!(0)]);

    workbench.update_in(cx, |workbench, window, cx| {
        workbench.focus_scm_repository(Some(1));
        workbench.edit_scm_input(|input| input.set_value("message".into()), window, cx);
    });
    workbench.read_with(cx, |workbench, _| {
        assert_eq!(workbench.scm.model.provider(1).unwrap().input.value, "message");
        assert_eq!(workbench.scm.model.provider(0).unwrap().input.value, "");
        assert_eq!(workbench.active_scm_provider().unwrap().handle, 1);
    });
    assert_eq!(selections(&receiver), [json!(1)]);

    // scmViewService.ts `onDidRemoveRepository`: the first shown repository takes the focus.
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.handle_scm_request("$unregisterSourceControl", &[json!(1)], window, cx)));
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.scm.focused, Some(0)));
    assert_eq!(selections(&receiver), [json!(0)]);
}

#[gpui::test]
fn scm_repositories_sort_by_name(cx: &mut TestAppContext) {
    let (workbench, _receiver, cx) = workbench_in("scm-repositories-sort", cx);
    workbench.update_in(cx, |workbench, window, cx| {
        for (handle, root) in [(0, "/w/zeta"), (1, "/w/alpha")] {
            drop(workbench.handle_scm_request("$registerSourceControl", &register(handle, root, false), window, cx));
        }
        workbench.settings.lock().unwrap().update_value_derived("scm.repositories.sortOrder", Some(&json!("name"))).unwrap();
    });
    workbench.read_with(cx, |workbench, _| {
        let names: Vec<String> = workbench.visible_scm_providers().iter().map(|provider| workbench.scm_repository_name(provider)).collect();
        assert_eq!(names, ["alpha", "zeta"]);
    });
}
