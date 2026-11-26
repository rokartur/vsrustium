use std::path::Path;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};

use super::*;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;

/// A folder with `a.txt` (plaintext) open in the active editor.
fn open<'a>(cx: &'a mut TestAppContext, name: &str) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-language-status-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "text\n").unwrap();
    let workspace = config::workspace::Workspace::folder(dir.clone());
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(workspace.clone()))));
    let (sender, _receiver) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(sender), cx);
        workbench.register_language_status();
        workbench.open_file(dir.join("a.txt"), true, window, cx);
        workbench
    });
    cx.run_until_parked();
    (workbench, cx)
}

fn status(id: &str, severity: u64, source: &str, selector: Value) -> Value {
    json!({ "id": id, "name": id, "selector": selector, "severity": severity, "label": format!("$(check) {id}"), "detail": "", "busy": false, "source": source })
}

fn set(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, handle: u64, status: Value) {
    workbench.update(cx, |workbench, cx| workbench.set_language_status(handle, status, cx));
}

fn ids(statuses: &[Status]) -> Vec<String> {
    statuses.iter().map(|status| status.id.clone()).collect()
}

fn view(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> LanguageStatusView {
    workbench.read_with(cx, |workbench, _| workbench.language_status_view())
}

fn run(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, command: &str, id: &str) {
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command(command, vec![json!(id)], window, cx));
}

fn stored(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, key: &str) -> Option<String> {
    workbench.read_with(cx, |workbench, _| workbench.profile_storage.lock().unwrap().get(key).map(str::to_owned))
}

#[gpui::test]
fn statuses_of_the_model_sort_by_severity_source_and_id_then_score(cx: &mut TestAppContext) {
    let (workbench, cx) = open(cx, "order");
    set(&workbench, cx, 1, status("b", 1, "S", json!("plaintext")));
    set(&workbench, cx, 2, status("a", 1, "S", json!("*")));
    set(&workbench, cx, 3, status("z", 3, "Z", json!("*")));
    set(&workbench, cx, 4, status("c", 1, "A", json!("*")));
    set(&workbench, cx, 5, status("r", 3, "A", json!("rust")));
    assert_eq!(ids(&view(&workbench, cx).combined), ["z", "c", "a", "b"]);

    let mut exact = status("same", 1, "T", json!("plaintext"));
    exact["label"] = json!("exact");
    set(&workbench, cx, 6, exact);
    let mut any = status("same", 1, "T", json!("*"));
    any["label"] = json!("any");
    set(&workbench, cx, 7, any);
    let labels: Vec<String> = view(&workbench, cx).combined.iter().filter(|status| status.id == "same").map(|status| status.label.clone()).collect();
    assert_eq!(labels, ["exact", "any"]);
}

#[gpui::test]
fn the_combined_entry_shows_the_first_severity_or_the_spinner_of_an_unpinned_busy_status(cx: &mut TestAppContext) {
    let (workbench, cx) = open(cx, "combined");
    assert_eq!(view(&workbench, cx).combined_text(), None);
    set(&workbench, cx, 1, status("info", 1, "S", json!("*")));
    assert_eq!(view(&workbench, cx).combined_text(), Some("$(bracket)"));
    set(&workbench, cx, 2, status("warning", 2, "S", json!("*")));
    assert_eq!(view(&workbench, cx).combined_text(), Some("$(bracket-dot)"));
    set(&workbench, cx, 3, status("error", 3, "S", json!("*")));
    assert_eq!(view(&workbench, cx).combined_text(), Some("$(bracket-error)"));

    let mut busy = status("info", 1, "S", json!("*"));
    busy["busy"] = json!(true);
    set(&workbench, cx, 1, busy);
    assert_eq!(view(&workbench, cx).combined_text(), Some(BUSY));
    run(&workbench, cx, PIN, "info");
    assert_eq!(view(&workbench, cx).combined_text(), Some("$(bracket-error)"));
    workbench.update(cx, |workbench, cx| {
        let (combined, dedicated) = workbench.language_status_items(workbench.color("statusBar.foreground"), cx);
        assert!(combined.is_some());
        assert_eq!(dedicated.len(), 1);
    });
}

#[gpui::test]
fn pinning_moves_a_status_into_its_own_entry_and_persists(cx: &mut TestAppContext) {
    let (workbench, cx) = open(cx, "pin");
    let mut warning = status("lint", 2, "S", json!("*"));
    warning["command"] = json!({ "id": "lint.run", "title": "Run", "arguments": [1] });
    set(&workbench, cx, 1, warning);
    set(&workbench, cx, 2, status("other", 1, "S", json!("*")));
    let rows = hover_rows(&view(&workbench, cx));
    assert_eq!(rows[0], HoverRow { severity: Some(("info", Some("notificationsInfoIcon.foreground"))), label: "$(check) lint".into(), detail: String::new(), command: Some(("Run".into(), "lint.run".into(), vec![json!(1)])), pinned: false, id: "lint".into() });
    assert_eq!((rows[1].severity, rows[1].pinned), (Some(("check", None)), false));

    run(&workbench, cx, PIN, "lint");
    let pinned = view(&workbench, cx);
    assert_eq!(ids(&pinned.dedicated), ["lint"]);
    assert_eq!(ids(&pinned.combined), ["lint", "other"]);
    assert_eq!(stored(&workbench, cx, KEY_DEDICATED_ITEMS).as_deref(), Some(r#"["lint"]"#));
    let entry = dedicated_entry(&pinned.dedicated[0]);
    assert_eq!((entry.entry_id.as_str(), entry.text.as_str(), entry.kind, entry.command), ("status.languageStatus.lint", "$(check) lint", Some("warning"), Some(json!({ "id": "lint.run", "arguments": [1] }))));
    assert!(hover_rows(&pinned)[0].pinned);

    run(&workbench, cx, UNPIN, "lint");
    assert!(view(&workbench, cx).dedicated.is_empty());
    assert_eq!(stored(&workbench, cx, KEY_DEDICATED_ITEMS), None);
}

#[gpui::test]
fn removing_a_status_drops_it_and_its_entry(cx: &mut TestAppContext) {
    let (workbench, cx) = open(cx, "remove");
    set(&workbench, cx, 1, status("a", 1, "S", json!("*")));
    set(&workbench, cx, 2, status("b", 1, "S", json!("*")));
    run(&workbench, cx, PIN, "a");
    workbench.update(cx, |workbench, cx| workbench.remove_language_status(1, cx));
    let after = view(&workbench, cx);
    assert_eq!(ids(&after.combined), ["b"]);
    assert!(after.dedicated.is_empty());
    workbench.update(cx, |workbench, cx| workbench.remove_language_status(2, cx));
    assert_eq!(view(&workbench, cx).combined_text(), None);
}

#[gpui::test]
fn a_changed_status_wiggles_when_new_to_the_user_or_severe_and_flashes_when_severe(cx: &mut TestAppContext) {
    let (workbench, cx) = open(cx, "attention");
    let attention = |workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| (workbench.language_status.serial, workbench.language_status.wiggle, workbench.language_status.flash));
    set(&workbench, cx, 1, status("a", 1, "S", json!("*")));
    assert_eq!(attention(&workbench, cx), (1, true, false));
    set(&workbench, cx, 1, status("a", 1, "S", json!("*")));
    assert_eq!(attention(&workbench, cx).0, 1, "an unchanged view model does not animate again");

    workbench.update(cx, |workbench, _| {
        for _ in 0..3 {
            workbench.language_status.serial += 1;
            workbench.count_language_status_interaction();
            workbench.count_language_status_interaction();
        }
    });
    assert_eq!(stored(&workbench, cx, KEY_INTERACT_COUNT).as_deref(), Some("3"));
    set(&workbench, cx, 2, status("b", 1, "S", json!("*")));
    assert_eq!((attention(&workbench, cx).1, attention(&workbench, cx).2), (false, false));
    set(&workbench, cx, 3, status("c", 2, "S", json!("*")));
    assert_eq!((attention(&workbench, cx).1, attention(&workbench, cx).2), (true, true));
    let mut busy = status("d", 3, "S", json!("*"));
    busy["busy"] = json!(true);
    set(&workbench, cx, 4, busy);
    assert_eq!((attention(&workbench, cx).1, attention(&workbench, cx).2), (false, false));

    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("editor.inlayHints.Reset", vec![], window, cx));
    assert_eq!(stored(&workbench, cx, KEY_INTERACT_COUNT), None);
}

/// limitIndicator.test.ts "distinguishes diagnostic truncation from editor highlight limits".
#[gpui::test]
fn too_many_problems_warn_whether_the_editor_or_the_source_cut_them(cx: &mut TestAppContext) {
    let (workbench, cx) = open(cx, "limit-problems");
    let uri = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().uri());
    let mut details = Vec::new();
    for (count, truncated) in [(5000, false), (5001, false), (1001, true), (5001, true), (5000, false)] {
        let markers: Vec<Value> = (0..count).map(|index| json!({ "severity": 8, "message": format!("problem {}", index + 1), "startLineNumber": index + 1, "startColumn": 1, "endLineNumber": index + 1, "endColumn": 2 })).collect();
        workbench.update(cx, |workbench, cx| workbench.change_many_markers("test", &json!([[uri.clone(), markers, truncated]]), cx));
        details.push(view(&workbench, cx).combined.iter().filter(|status| status.id == "diagnosticsLimitInfo").map(|status| status.detail.clone()).collect::<Vec<_>>());
    }
    let expected: [&[&str]; 5] = [
        &[],
        &["Only 5000 problems are rendered in the editor for performance reasons."],
        &["Some problems are not shown in the problems view for performance reasons."],
        &["Some problems are not shown in the problems view, and only 5000 problems are rendered in the editor for performance reasons."],
        &[],
    ];
    assert_eq!(details, expected);
}

/// limitIndicator.test.ts "keeps Configure actions for folding and color limits".
#[gpui::test]
fn folding_and_color_limits_warn_with_a_configure_command(cx: &mut TestAppContext) {
    let (workbench, cx) = open(cx, "limit-folding");
    workbench.update(cx, |workbench, _| {
        let editor = workbench.editors.active_mut().unwrap();
        editor.text_model_mut().document.lines = (0..12).flat_map(|_| ["a".to_string(), "  b".to_string()]).collect();
        editor.folding_limit = 10;
        editor.refresh_folding_ranges();
        workbench.color_detector.model_id = Some(workbench.editors.active().unwrap().model_id);
        workbench.color_detector.limited = Some(1);
    });
    let statuses: Vec<(String, String, Option<Value>)> = view(&workbench, cx).combined.into_iter().map(|status| (status.id, status.detail, status.command)).collect();
    let configure = |setting: &str| Some(json!({ "id": "workbench.action.openSettings", "title": "Configure", "arguments": [setting] }));
    assert_eq!(statuses, [
        ("decoratorsLimitInfo".to_string(), "only 1 shown for performance reasons".to_string(), configure("editor.colorDecoratorsLimit")),
        ("foldingLimitInfo".to_string(), "only 10 shown for performance reasons".to_string(), configure("editor.foldingMaximumRegions")),
    ]);
}
