use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::commands::Outcome;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

fn workbench(cx: &mut TestAppContext) -> (gpui::Entity<Workbench>, &mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("code-gpui-notifications-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(dir.join("User")).unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let host = HostSync::new(std::sync::mpsc::channel().0);
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, host, cx));
    cx.run_until_parked();
    (workbench, cx)
}

fn show_message(workbench: &gpui::Entity<Workbench>, args: Vec<Value>, cx: &mut VisualTestContext) -> futures::channel::oneshot::Receiver<Result<Value, Value>> {
    let outcome = workbench.update_in(cx, |workbench, window, cx| workbench.show_extension_message(&args, window, cx));
    cx.run_until_parked();
    let Outcome::Pending(reply) = outcome else { panic!("a non-modal message answers when it closes") };
    reply
}

#[gpui::test]
fn extension_message_answers_the_picked_item_and_shows_in_the_bell(cx: &mut TestAppContext) {
    let (workbench, cx) = workbench(cx);
    let commands = json!([{ "title": "Yes", "handle": 0, "isCloseAffordance": false }, { "title": "No", "handle": 1, "isCloseAffordance": false }]);
    let mut reply = show_message(&workbench, vec![json!(1), json!("Hello"), json!({}), commands], cx);
    workbench.update(cx, |workbench, _| {
        let item = &workbench.notifications.model.items[0];
        assert_eq!((item.message.raw.as_str(), item.source.as_deref(), item.expanded), ("Hello", Some("Extension"), true));
        assert!(workbench.notifications.toasts_visible());
        assert_eq!(workbench.notifications_tooltip(0), "1 New Notification");
    });
    let key = workbench.read_with(cx, |workbench, _| workbench.notifications.model.items[0].key);
    workbench.update_in(cx, |workbench, window, cx| workbench.run_notification_action(key, 1, window, cx));
    cx.run_until_parked();
    assert_eq!(reply.try_recv().unwrap(), Some(Ok(json!(1))));
    workbench.update(cx, |workbench, _| {
        assert!(workbench.notifications.model.items.is_empty());
        assert!(!workbench.notifications.toasts_visible());
    });
}

#[gpui::test]
fn closing_a_message_answers_undefined_and_center_hides_toasts(cx: &mut TestAppContext) {
    let (workbench, cx) = workbench(cx);
    let mut reply = show_message(&workbench, vec![json!(3), json!("Broken"), json!({}), json!([])], cx);
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("notifications.showList", Vec::new(), window, cx)));
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, cx| {
        assert!(workbench.notifications.center_visible && !workbench.notifications.toasts_visible());
        assert_eq!(workbench.notifications_tooltip(0), "Hide Notifications");
        assert_eq!(workbench.key_context(window)["notificationCenterVisible"], json!(true));
        drop(workbench.execute_command("notifications.clearAll", Vec::new(), window, cx));
    });
    cx.run_until_parked();
    assert_eq!(reply.try_recv().unwrap(), Some(Ok(Value::Null)));
    workbench.update(cx, |workbench, _| assert!(!workbench.notifications.center_visible && workbench.notifications.model.items.is_empty()));
}

#[gpui::test]
fn do_not_disturb_keeps_info_out_of_toasts_but_not_errors(cx: &mut TestAppContext) {
    let (workbench, cx) = workbench(cx);
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("notifications.toggleDoNotDisturbMode", Vec::new(), window, cx)));
    drop(show_message(&workbench, vec![json!(1), json!("Quiet"), json!({}), json!([])], cx));
    workbench.update(cx, |workbench, _| assert!(!workbench.notifications.toasts_visible()));
    drop(show_message(&workbench, vec![json!(3), json!("Loud"), json!({}), json!([])], cx));
    workbench.update(cx, |workbench, _| assert!(workbench.notifications.toasts_visible()));
}

#[gpui::test]
fn run_commands_argument_errors_notify(cx: &mut TestAppContext) {
    let (workbench, cx) = workbench(cx);
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("runCommands", vec![json!(5)], window, cx)));
    cx.run_until_parked();
    workbench.update(cx, |workbench, _| {
        let item = &workbench.notifications.model.items[0];
        assert_eq!(item.severity, crate::notifications::Severity::Error);
        assert!(item.message.raw.starts_with("'runCommands' has received an argument with incorrect type"));
    });
}

#[gpui::test]
fn the_position_moves_the_bell_between_status_bar_sides_and_the_title_bar(cx: &mut TestAppContext) {
    let (workbench, cx) = workbench(cx);
    let sides = |workbench: &gpui::Entity<Workbench>, command: &str, cx: &mut VisualTestContext| {
        workbench.update_in(cx, |workbench, window, cx| {
            drop(workbench.execute_command(command, Vec::new(), window, cx));
            let foreground = gpui::rgba(0xffffffff);
            let left = workbench.notifications_status_item(true, foreground, cx).is_some();
            let right = workbench.notifications_status_item(false, foreground, cx).is_some();
            let title_bar = workbench.notifications_title_bar_item(foreground, cx).is_some();
            (left, right, title_bar)
        })
    };
    assert_eq!(sides(&workbench, "workbench.action.setNotificationsPosition.bottomLeft", cx), (true, false, false));
    assert_eq!(sides(&workbench, "workbench.action.setNotificationsPosition.topRight", cx), (false, false, true));
    assert_eq!(sides(&workbench, "workbench.action.setNotificationsPosition.bottomRight", cx), (false, true, false));
}

fn progress(workbench: &gpui::Entity<Workbench>, method: &str, args: Vec<Value>, cx: &mut VisualTestContext) {
    workbench.update_in(cx, |workbench, window, cx| match method {
        "start" => drop(workbench.start_progress(&args, window, cx)),
        "report" => drop(workbench.report_progress(&args, window, cx)),
        _ => drop(workbench.end_progress(&args, window, cx)),
    });
    cx.run_until_parked();
}

#[gpui::test]
fn notification_progress_reports_into_its_toast_and_stays_800ms(cx: &mut TestAppContext) {
    let (workbench, cx) = workbench(cx);
    progress(&workbench, "start", vec![json!(1), json!({ "location": 15, "title": "Working", "cancellable": true }), json!("port.messages")], cx);
    progress(&workbench, "report", vec![json!(1), json!({ "message": "half", "increment": 50 })], cx);
    workbench.update(cx, |workbench, cx| {
        let item = &workbench.notifications.model.items[0];
        assert_eq!(item.message.raw, "Working: half");
        assert_eq!((item.progress.as_ref().unwrap().total, item.progress.as_ref().unwrap().worked), (Some(100.), Some(50.)));
        assert_eq!(item.primary[0].label, "Cancel");
        assert_eq!(item.secondary[0].label, "Manage Extension");
        assert!(workbench.notifications.toast_shown(item.key));
        assert!(workbench.window_progress_entry(cx.background_executor().now()).is_none());
    });
    progress(&workbench, "end", vec![json!(1)], cx);
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.notifications.model.items.len()), 1);
    cx.executor().advance_clock(std::time::Duration::from_millis(800));
    cx.run_until_parked();
    assert!(workbench.read_with(cx, |workbench, _| workbench.notifications.model.items.is_empty()));
}

#[gpui::test]
fn window_progress_is_a_silent_notification_shown_in_the_status_bar(cx: &mut TestAppContext) {
    let (workbench, cx) = workbench(cx);
    progress(&workbench, "start", vec![json!(2), json!({ "location": 10, "title": "Indexing", "source": { "label": "Port", "id": "port.messages" } })], cx);
    assert!(workbench.read_with(cx, |workbench, _| workbench.notifications.model.items.is_empty()));
    cx.executor().advance_clock(std::time::Duration::from_millis(150));
    cx.run_until_parked();
    progress(&workbench, "report", vec![json!(2), json!({ "message": "src" })], cx);
    workbench.update(cx, |workbench, _| {
        assert_eq!(workbench.notifications.model.items[0].message.raw, "Indexing: src");
        assert!(!workbench.notifications.toasts_visible());
    });
    cx.executor().advance_clock(std::time::Duration::from_millis(150));
    cx.run_until_parked();
    let entry = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, cx| workbench.window_progress_entry(cx.background_executor().now()));
    let shown = entry(cx).unwrap();
    assert_eq!((shown.text.as_str(), shown.tooltip.as_deref(), shown.show_progress), ("Indexing: src", Some("[Port] Indexing: src"), true));
    progress(&workbench, "end", vec![json!(2)], cx);
    assert!(workbench.read_with(cx, |workbench, _| workbench.notifications.model.items.is_empty()));
    // withWindowProgress keeps the entry 150ms after it showed.
    assert!(entry(cx).is_some());
    cx.executor().advance_clock(std::time::Duration::from_millis(150));
    cx.run_until_parked();
    assert!(entry(cx).is_none());
}

#[gpui::test]
fn window_progress_moves_into_the_open_center_and_back(cx: &mut TestAppContext) {
    let (workbench, cx) = workbench(cx);
    progress(&workbench, "start", vec![json!(2), json!({ "location": 10, "title": "Window progress" })], cx);
    cx.executor().advance_clock(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    let entry = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, cx| workbench.window_progress_entry(cx.background_executor().now()));
    assert!(entry(cx).is_some());
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("notifications.showList", Vec::new(), window, cx)));
    cx.run_until_parked();
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.notifications.model.items.len()), 1);
    assert!(entry(cx).is_none());
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("notifications.hideList", Vec::new(), window, cx)));
    cx.run_until_parked();
    assert!(entry(cx).is_some());
}
