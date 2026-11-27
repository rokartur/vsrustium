use std::path::Path;
use std::sync::mpsc::Receiver;

use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use super::HistoryNavigator;
use crate::host_sync::{HostMessage, HostSync};
use crate::menus::{MenuAction, MenuId};
use crate::workbench::Workbench;

fn workbench_in<'a>(name: &str, cx: &'a mut TestAppContext) -> (gpui::Entity<Workbench>, Receiver<HostMessage>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, crate::languages::Languages::new(&[]), settings, HostSync::new(sender), cx);
        workbench.set_default_keybindings(crate::keybindings::default_keybindings(&[]));
        workbench.set_user_keybindings(Vec::new());
        workbench
    });
    (workbench, receiver, cx)
}

fn register(handle: u64, root: &str) -> Vec<Value> {
    let input = json!({ "scheme": "vscode-scm", "path": format!("git/scm{handle}/input") });
    vec![json!(handle), Value::Null, json!("git"), json!("Git"), json!({ "scheme": "file", "path": root }), Value::Null, json!(false), input]
}

fn request(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, method: &str, args: Vec<Value>) {
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.handle_scm_request(method, &args, window, cx)));
}

fn input(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> (String, (usize, usize)) {
    workbench.read_with(cx, |workbench, _| (workbench.scm.input.value.clone(), workbench.scm.input.selection))
}

#[test]
fn the_history_navigator_keeps_unique_values_up_to_its_capacity() {
    let mut history = HistoryNavigator::new(&["a".into(), "b".into(), "a".into()]);
    assert_eq!(history.values, ["b", "a"]);
    assert_eq!(history.replace_last("b"), "a");
    assert_eq!(history.values, ["b"]);
    for index in 0..150 {
        history.add(index.to_string());
    }
    assert_eq!(history.values.len(), 100);
    assert_eq!(history.values[0], "50");
    assert!(history.is_at_end());
    assert_eq!(history.previous(), "148");
    assert_eq!(history.next(), "149");
    assert_eq!(history.next(), "149");
}

#[gpui::test]
fn up_and_down_cycle_the_input_history_and_it_persists(cx: &mut TestAppContext) {
    let (workbench, receiver, cx) = workbench_in("scm-input-history", cx);
    request(&workbench, cx, "$registerSourceControl", register(0, "/w/repo"));
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.focus_scm_input(window, cx);
        workbench.edit_scm_input(|input| input.set_value("fix bug".into()), window, cx);
    });
    // git clears the input after a commit: the message enters the history.
    request(&workbench, cx, "$setInputBoxValue", vec![json!(0), json!("")]);
    workbench.update_in(cx, |workbench, window, cx| workbench.edit_scm_input(|input| input.set_value("draft".into()), window, cx));

    // The caret is at the end of "draft": Up is a caret move, Alt+Up always shows the previous value.
    cx.simulate_keystrokes("alt-up");
    cx.run_until_parked();
    assert_eq!(input(&workbench, cx), ("fix bug".into(), (0, 0)));
    let context = workbench.update_in(cx, |workbench, window, _| workbench.key_context(window));
    assert_eq!((context.get("scmInputIsInFirstPosition"), context.get("scmInputIsInLastPosition")), (Some(&json!(true)), Some(&json!(false))));
    cx.simulate_keystrokes("up");
    cx.run_until_parked();
    assert_eq!(input(&workbench, cx).0, "fix bug", "the first value stays");

    workbench.update_in(cx, |workbench, window, cx| workbench.edit_scm_input(|input| input.select(7, 7), window, cx));
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    assert_eq!(input(&workbench, cx), ("draft".into(), (5, 5)));
    let changes: Vec<Value> = receiver.try_iter().filter(|message| message.method == "$onInputBoxValueChange").map(|message| message.args[1].clone()).collect();
    assert_eq!(changes.last(), Some(&json!("draft")));

    let stored = workbench.read_with(cx, |workbench, _| workbench.workspace_storage.lock().unwrap().get("scm.history").map(str::to_string));
    let stored: Value = serde_json::from_str(&stored.unwrap()).unwrap();
    assert_eq!(stored[0][0], "Git");
    assert_eq!(stored[0][1]["path"], "/w/repo");
    assert_eq!(stored[0][2], json!(["fix bug", "draft"]));

    // `new SCMInput`: a repository registered again starts with the draft, and the host hears of it.
    request(&workbench, cx, "$unregisterSourceControl", vec![json!(0)]);
    request(&workbench, cx, "$registerSourceControl", register(1, "/w/repo"));
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.scm.model.provider(1).unwrap().input.value, "draft"));
    assert!(receiver.try_iter().any(|message| message.method == "$onInputBoxValueChange" && message.args == [json!(1), json!("draft")]));
}

#[gpui::test]
fn focus_next_input_and_resource_group_cycle(cx: &mut TestAppContext) {
    let (workbench, _receiver, cx) = workbench_in("scm-focus-next", cx);
    let raw = |handle: u64, path: &str| json!([handle, { "scheme": "file", "path": path }, [null, null], "Modified", false, false, "", null, null, null]);
    request(&workbench, cx, "$registerSourceControl", register(0, "/w/alpha"));
    request(&workbench, cx, "$registerSourceControl", register(1, "/w/beta"));
    let groups = json!([[1, "index", "Staged Changes", { "hideWhenEmpty": true }, false], [2, "workingTree", "Changes", {}, false], [3, "merge", "Merge Changes", { "hideWhenEmpty": true }, false]]);
    request(&workbench, cx, "$registerGroups", vec![json!(0), groups, json!([[1, [[0, 0, [raw(1, "/w/alpha/a")]]]], [2, [[0, 0, [raw(2, "/w/alpha/b")]]]]])]);
    let run = |cx: &mut VisualTestContext, id: &str| workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(id, Vec::new(), window, cx)));
    let state = |cx: &mut VisualTestContext| workbench.update_in(cx, |workbench, window, _| (workbench.scm.focused, workbench.scm.input_focus.is_focused(window), workbench.scm.selected));

    run(cx, "workbench.scm.action.focusNextInput");
    assert_eq!(state(cx), (Some(0), true, None), "the focused repository's input first");
    run(cx, "workbench.scm.action.focusNextInput");
    assert_eq!(state(cx).0, Some(1));
    run(cx, "workbench.scm.action.focusNextInput");
    assert_eq!(state(cx).0, Some(0), "wraps");
    run(cx, "workbench.scm.action.focusPreviousInput");
    assert_eq!(state(cx).0, Some(1));

    workbench.update_in(cx, |workbench, _, _| workbench.focus_scm_repository(Some(0)));
    run(cx, "workbench.scm.action.focusNextResourceGroup");
    assert_eq!(state(cx).2, Some((0, 1, None)), "the first shown group");
    run(cx, "workbench.scm.action.focusNextResourceGroup");
    assert_eq!(state(cx).2, Some((0, 2, None)));
    run(cx, "workbench.scm.action.focusNextResourceGroup");
    assert_eq!(state(cx).2, Some((0, 1, None)), "skips the empty merge group");
    run(cx, "workbench.scm.action.focusPreviousResourceGroup");
    assert_eq!(state(cx).2, Some((0, 2, None)));
}

#[gpui::test]
fn the_more_actions_menu_shows_view_and_sort(cx: &mut TestAppContext) {
    let (workbench, _receiver, cx) = workbench_in("scm-more-actions", cx);
    request(&workbench, cx, "$registerSourceControl", register(0, "/w/alpha"));
    request(&workbench, cx, "$registerSourceControl", register(1, "/w/beta"));
    let titles = |cx: &mut VisualTestContext| {
        workbench.update_in(cx, |workbench, window, _| {
            let mut context = workbench.key_context(window);
            workbench.scm_view_context(&mut context);
            let actions = super::flatten_submenus(workbench.menu_actions_outside(MenuId::ScmTitle, "navigation", &context));
            actions.into_iter().filter_map(|action| match action {
                MenuAction::Command { id, checked, enabled, .. } => Some((id, checked, enabled)),
                _ => None,
            }).collect::<Vec<_>>()
        })
    };
    let actions = titles(cx);
    let ids: Vec<&str> = actions.iter().map(|(id, ..)| id.as_str()).collect();
    assert_eq!(ids, ["workbench.scm.action.setListViewMode", "workbench.scm.action.setTreeViewMode", "workbench.scm.action.setSortKey.name", "workbench.scm.action.setSortKey.path", "workbench.scm.action.setSortKey.status"]);
    assert_eq!(actions[3], ("workbench.scm.action.setSortKey.path".into(), true, true));

    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.scm.action.setTreeViewMode", Vec::new(), window, cx)));
    let actions = titles(cx);
    assert_eq!(actions[1], ("workbench.scm.action.setTreeViewMode".into(), true, true));
    assert!(!actions[2].2, "sorting needs the list view mode");

    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.scm.action.collapseAllRepositories", Vec::new(), window, cx)));
    let context = workbench.update_in(cx, |workbench, window, _| {
        let mut context = workbench.key_context(window);
        workbench.scm_view_context(&mut context);
        context
    });
    assert_eq!(context["scmViewAreAllRepositoriesCollapsed"], json!(true));
}
