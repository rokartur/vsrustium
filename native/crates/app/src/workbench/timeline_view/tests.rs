use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use gpui::{Modifiers, TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use super::{TIMELINE_VIEW_ID, now_ms};
use crate::host_sync::{HostMessage, HostSync};
use crate::menus::{MenuAction, MenuId, menu_actions};
use crate::workbench::Workbench;

fn workbench_in<'a>(name: &str, cx: &'a mut TestAppContext) -> (PathBuf, gpui::Entity<Workbench>, Receiver<HostMessage>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "one\ntwo").unwrap();
    std::fs::write(dir.join("b.txt"), "one\nTWO").unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None)));
    let workspace = Some(config::workspace::Workspace::folder(dir.clone()));
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), workspace, crate::languages::Languages::new(&[]), settings, HostSync::new(sender), cx));
    cx.run_until_parked();
    workbench.update(cx, |workbench, cx| workbench.expand_explorer_pane(TIMELINE_VIEW_ID, cx));
    (dir, workbench, receiver, cx)
}

fn timeline_requests(receiver: &Receiver<HostMessage>) -> Vec<HostMessage> {
    receiver.try_iter().filter(|message| message.method == "$getTimeline").collect()
}

fn answer(request: &mut HostMessage, items: Value) {
    let source = request.args[0].clone();
    request.reply.take().unwrap().send(Ok(json!({ "source": source, "items": items }))).unwrap();
}

fn file_uri(path: &Path) -> Value {
    json!({ "$mid": 1, "scheme": "file", "authority": "", "path": path.to_string_lossy(), "query": "", "fragment": "" })
}

fn titles(actions: Vec<MenuAction>) -> Vec<String> {
    actions
        .into_iter()
        .filter_map(|action| match action {
            MenuAction::Command { title, .. } | MenuAction::Submenu { title, .. } => Some(title),
            MenuAction::Separator => None,
        })
        .collect()
}

fn title_menu(workbench: &Workbench, window: &gpui::Window) -> Vec<String> {
    let menus = workbench.timeline_menus();
    titles(menu_actions(&menus, MenuId::TimelineTitle, &workbench.timeline_menu_context(&menus, window)))
}

fn register_git(workbench: &mut Workbench, workspace: bool, cx: &mut gpui::Context<Workbench>) {
    drop(workbench.handle_timeline_request("$registerTimelineProvider", &[json!({ "id": "git", "label": "Git History", "scheme": ["file", "git"] })], cx));
    if workspace {
        drop(workbench.handle_timeline_request("$registerWorkspaceTimelineProvider", &[json!({ "id": "git", "label": "Git History" })], cx));
    }
}

#[gpui::test]
fn the_workspace_timeline_shows_without_an_editor_and_pins(cx: &mut TestAppContext) {
    let (dir, workbench, receiver, cx) = workbench_in("timeline-workspace", cx);
    workbench.update_in(cx, |workbench, _, cx| {
        register_git(workbench, true, cx);
        workbench.update_timeline(cx);
    });
    let mut requests = timeline_requests(&receiver);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].args[1], Value::Null, "the workspace timeline has no uri");
    workbench.update_in(cx, |workbench, window, _| {
        let context = workbench.key_context(window);
        assert_eq!(context["timelineForWorkspace"], json!(true));
        assert_eq!(context["timelineHasWorkspaceProvider"], json!(true));
        assert_eq!(workbench.timeline.model.message(&Default::default(), true, 1).as_deref(), Some("Loading workspace timeline..."));
        assert!(!title_menu(workbench, window).contains(&"Pin Workspace Timeline".to_string()));
    });

    answer(&mut requests[0], json!([{ "handle": "git|abc", "id": "abc", "label": "Commit abc", "timestamp": now_ms() - 60_000, "contextValue": "git:workspace:commit" }]));
    cx.run_until_parked();
    workbench.read_with(cx, |workbench, _| {
        assert_eq!(workbench.timeline_row_handles(), ["git|abc"]);
        assert_eq!(workbench.timeline.model.target_label(), "Workspace");
    });

    // mainThreadTimeline.ts `$emitWorkspaceTimelineChangeEvent` reloads the shown workspace timeline.
    workbench.update_in(cx, |workbench, _, cx| drop(workbench.handle_timeline_request("$emitWorkspaceTimelineChangeEvent", &[json!("git")], cx)));
    let requests = timeline_requests(&receiver);
    assert_eq!(requests.len(), 1);
    assert_eq!((requests[0].args[1].clone(), requests[0].args[2]["limit"].clone()), (Value::Null, json!(50)));

    // An editor's resource replaces the workspace timeline, which "Pin Workspace Timeline" brings back pinned.
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.open_file(dir.join("a.txt"), true, window, cx);
        workbench.update_timeline(cx);
    });
    cx.run_until_parked();
    let requests = timeline_requests(&receiver);
    assert_eq!(requests.last().unwrap().args[1]["path"], json!(dir.join("a.txt").to_string_lossy()));
    workbench.update_in(cx, |workbench, window, cx| {
        assert!(!workbench.timeline.model.workspace);
        assert!(title_menu(workbench, window).contains(&"Pin Workspace Timeline".to_string()));
        drop(workbench.execute_command("timeline.showWorkspaceTimeline", Vec::new(), window, cx));
        workbench.update_timeline(cx);
        assert!(workbench.timeline.model.workspace);
        assert!(workbench.timeline.pinned);
        assert_eq!(workbench.key_context(window)["timelineFollowActiveEditor"], json!(false));
        assert!(title_menu(workbench, window).contains(&"Unpin Current Timeline".to_string()));
    });
    assert_eq!(timeline_requests(&receiver).last().unwrap().args[1], Value::Null);

    // Without a workspace provider, an editor without a resource has nothing to show.
    workbench.update_in(cx, |workbench, window, cx| {
        drop(workbench.handle_timeline_request("$unregisterWorkspaceTimelineProvider", &[json!("git")], cx));
        drop(workbench.execute_command("workbench.action.closeAllEditors", Vec::new(), window, cx));
        drop(workbench.execute_command("timeline.toggleFollowActiveEditor", Vec::new(), window, cx));
        workbench.update_timeline(cx);
    });
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.update_timeline(cx);
        assert!(!workbench.timeline.model.has_target());
        assert_eq!(workbench.key_context(window)["timelineHasWorkspaceProvider"], json!(false));
        assert_eq!(workbench.timeline.model.message(&Default::default(), true, 1).as_deref(), Some("The active editor cannot provide timeline information."));
    });
}

#[gpui::test]
fn two_selected_items_compare_older_against_newer(cx: &mut TestAppContext) {
    let (dir, workbench, receiver, cx) = workbench_in("timeline-compare", cx);
    workbench.update_in(cx, |workbench, window, cx| {
        register_git(workbench, false, cx);
        workbench.open_file(dir.join("a.txt"), true, window, cx);
        workbench.update_timeline(cx);
    });
    cx.run_until_parked();
    let mut requests = timeline_requests(&receiver);
    let now = now_ms();
    let items = json!([
        { "handle": "git|new", "id": "new", "timestamp": now - 120_000, "resourceUri": file_uri(&dir.join("b.txt")), "contextValue": "git:file:commit" },
        { "handle": "git|old", "id": "old", "label": "First", "timestamp": now - 86_400_000, "resourceUri": file_uri(&dir.join("a.txt")), "contextValue": "git:file:commit" },
    ]);
    answer(requests.last_mut().unwrap(), items);
    cx.run_until_parked();

    let item_menu = |workbench: &Workbench, handle: &str, window: &gpui::Window| {
        let menus = workbench.timeline_menus();
        let item = workbench.timeline.model.item(handle).unwrap();
        let context = workbench.timeline_item_context(item, workbench.timeline_menu_context(&menus, window));
        (titles(menu_actions(&menus, MenuId::TimelineItemContext, &context)), context)
    };
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.click_timeline_row("git|old", Modifiers::default(), 1, window, cx);
        assert!(!item_menu(workbench, "git|old", window).0.contains(&"Compare Selected".to_string()));
        workbench.click_timeline_row("git|new", Modifiers { platform: true, ..Modifiers::default() }, 1, window, cx);
        assert_eq!(workbench.timeline.selected, ["git|old", "git|new"]);
        let (titles, context) = item_menu(workbench, "git|new", window);
        assert!(titles.contains(&"Compare Selected".to_string()));
        assert_eq!((context["timelineItem"].clone(), context["listDoubleSelection"].clone()), (json!("git:file:commit"), json!(true)));
        // `TimelineActionRunner`: the item, the uri, the source and the selected items as action contexts.
        let args = workbench.timeline_item_args(workbench.timeline.model.item("git|new").unwrap());
        assert_eq!(args[0], json!({ "$mid": 12, "handle": "git|new", "source": "git", "uri": file_uri(&dir.join("a.txt")) }));
        assert_eq!((args[2].clone(), args[3].as_array().unwrap().len()), (json!("git"), 2));
        drop(workbench.execute_command("timeline.compareSelected", Vec::new(), window, cx));
    });
    cx.run_until_parked();
    workbench.read_with(cx, |workbench, _| {
        let names: Vec<String> = workbench.editors.editors.iter().map(|editor| workbench.editor_name(editor)).collect();
        assert_eq!(names, ["a.txt", "a.txt (First) \u{2194} b.txt (2 mins ago)"]);
        assert!(workbench.editors.active_diff().is_some());
        assert_eq!(workbench.editors.active().unwrap().document().name(), "b.txt", "the newer side is modified");
    });
}

#[gpui::test]
fn keys_move_the_focus_select_a_range_and_type_to_a_label(cx: &mut TestAppContext) {
    let (dir, workbench, receiver, cx) = workbench_in("timeline-keys", cx);
    workbench.update_in(cx, |workbench, window, cx| {
        register_git(workbench, false, cx);
        workbench.open_file(dir.join("a.txt"), true, window, cx);
        workbench.update_timeline(cx);
    });
    cx.run_until_parked();
    let mut requests = timeline_requests(&receiver);
    let items = json!([{ "handle": "git|1", "label": "alpha", "timestamp": 3 }, { "handle": "git|2", "label": "beta", "timestamp": 2 }, { "handle": "git|3", "label": "gamma", "timestamp": 1 }]);
    answer(requests.last_mut().unwrap(), items);
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, cx| window.focus(&workbench.timeline.focus.clone(), cx));
    cx.simulate_keystrokes("down");
    workbench.read_with(cx, |workbench, _| {
        assert!(workbench.timeline.selected.is_empty(), "an arrow moves the focus only");
        assert_eq!(workbench.timeline.focused.as_deref(), Some("git|1"));
    });
    cx.simulate_keystrokes("shift-down");
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.timeline.selected, ["git|1", "git|2"]));
    cx.simulate_keystrokes("escape g");
    workbench.read_with(cx, |workbench, _| {
        assert!(workbench.timeline.selected.is_empty());
        assert_eq!(workbench.timeline.focused.as_deref(), Some("git|3"));
    });
    workbench.update(cx, |workbench, _| {
        let entry = crate::menus::Entry::Command { id: "test.item".into(), precondition: None, toggled: None, alt: None };
        workbench.menus.push(crate::menus::MenuItem { menu: MenuId::TimelineItemContext, entry, title: "Test Item".into(), group: "1_test".into(), order: 1., when: None });
    });
    cx.simulate_keystrokes("shift-f10");
    workbench.read_with(cx, |workbench, _| assert!(workbench.context_menu.is_some(), "Shift+F10 shows the focused row's menu"));
}

#[gpui::test]
fn a_click_opens_the_comparison_and_keeps_the_row_selected_and_focused(cx: &mut TestAppContext) {
    let (dir, workbench, receiver, cx) = workbench_in("timeline-click", cx);
    workbench.update_in(cx, |workbench, window, cx| {
        register_git(workbench, false, cx);
        workbench.open_file(dir.join("a.txt"), true, window, cx);
        workbench.update_timeline(cx);
    });
    cx.run_until_parked();
    let mut requests = timeline_requests(&receiver);
    let command = json!({ "id": "_workbench.diff", "arguments": [file_uri(&dir.join("b.txt")), file_uri(&dir.join("a.txt")), "a.txt (old) \u{2194} a.txt", null] });
    answer(requests.last_mut().unwrap(), json!([{ "handle": "git|1", "label": "old", "timestamp": now_ms() - 60_000, "command": command }]));
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, cx| workbench.click_timeline_row("git|1", Modifiers::default(), 1, window, cx));
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, _| {
        assert!(workbench.editors.active_diff().is_some());
        assert_eq!(workbench.timeline.selected, ["git|1"]);
        assert!(workbench.timeline.focus.is_focused(window), "a click opens the comparison with preserveFocus");
    });
}
