use std::path::Path;
use std::sync::mpsc::Receiver;

use exthost::scan::scan_extension;
use exthost::session::ForwardedRequest;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::commands::Outcome;
use crate::host_sync::{HostMessage, HostSync};
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

const VIEW: &str = "test.tree";

fn workbench_with<'a>(name: &str, cx: &'a mut TestAppContext) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext, Receiver<HostMessage>) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("vsrustium-tree-view-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = json!({ "name": "trees", "publisher": "test", "version": "1.0.0", "engines": { "vscode": "*" },
        "contributes": { "views": { "explorer": [{ "id": VIEW, "name": "Test Tree" }, { "id": "test.hidden", "name": "Hidden", "when": "never" }] } } });
    std::fs::write(dir.join("package.json"), manifest.to_string()).unwrap();
    let extension = scan_extension(&dir, false).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(sender), cx);
        workbench.set_tree_views(&[extension]);
        workbench
    });
    drop(workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("workbench.view.explorer", Vec::new(), window, cx)));
    draw(cx);
    receiver.try_iter().for_each(drop);
    (workbench, cx, receiver)
}

fn request(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, method: &str, args: Value) -> Outcome {
    let request = ForwardedRequest { req: 1, proxy: "MainThreadTreeViews", method: method.into(), args: args.as_array().unwrap().clone() };
    let outcome = workbench.update_in(cx, |workbench, window, cx| workbench.handle_host_request(request, window, cx));
    cx.run_until_parked();
    outcome
}

/// The host messages sent since the last call, as `method(args)`; `$getChildren` is answered from `children`.
fn sent(receiver: &Receiver<HostMessage>, cx: &mut VisualTestContext, children: &[(Value, Value)]) -> Vec<String> {
    draw(cx);
    let mut sent = Vec::new();
    for message in receiver.try_iter() {
        sent.push(format!("{}({})", message.method, Value::Array(message.args.clone())));
        if let Some(reply) = message.reply {
            let answer = children.iter().find(|(parents, _)| message.args.get(1) == Some(parents)).map_or(Value::Null, |(_, answer)| answer.clone());
            let _ = reply.send(Ok(answer));
        }
    }
    draw(cx);
    sent
}

/// The Explorer lays out its panes and the tree views sync their visibility while the window draws.
fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| drop(window.draw(cx)));
    cx.run_until_parked();
}

fn item(handle: &str, parent: Option<&str>, state: u64) -> Value {
    json!({ "handle": handle, "parentHandle": parent, "label": { "label": handle }, "collapsibleState": state })
}

fn rows(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<(String, usize)> {
    workbench.read_with(cx, |workbench, _| workbench.tree_views.trees[VIEW].rows().into_iter().map(|(item, depth)| (item.handle.clone(), depth)).collect())
}

#[gpui::test]
fn a_collapsed_view_fetches_nothing_until_its_pane_expands(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = workbench_with("collapsed", cx);
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.shown_tree_views()), vec![VIEW]);
    request(&workbench, cx, "$registerTreeViewDataProvider", json!([VIEW, { "showCollapseAll": false }]));
    assert_eq!(sent(&receiver, cx, &[]), vec![format!("$setVisible([\"{VIEW}\",false])")]);
    request(&workbench, cx, "$refresh", json!([VIEW]));
    assert_eq!(sent(&receiver, cx, &[]), Vec::<String>::new(), "treeView.ts: no tree, no fetch");

    workbench.update(cx, |workbench, cx| workbench.expand_pane(crate::workbench::view_panes::PaneContainer::Explorer, VIEW, cx));
    let roots = json!([[0, item("a", None, 1), item("b", None, 0)]]);
    let messages = sent(&receiver, cx, &[(Value::Null, roots)]);
    assert!(messages.contains(&format!("$activateByEvent([\"onView:{VIEW}\",0])")), "{messages:?}");
    assert!(messages.contains(&format!("$setVisible([\"{VIEW}\",true])")), "{messages:?}");
    assert!(messages.contains(&format!("$getChildren([\"{VIEW}\",null])")), "{messages:?}");
    assert_eq!(rows(&workbench, cx), vec![("a".into(), 0), ("b".into(), 0)]);

    workbench.update(cx, |workbench, cx| workbench.toggle_tree_item(VIEW, "a", cx));
    let messages = sent(&receiver, cx, &[(json!(["a"]), json!([[0, item("a/1", Some("a"), 0)]]))]);
    assert_eq!(messages, vec![format!("$setExpanded([\"{VIEW}\",\"a\",true])"), format!("$getChildren([\"{VIEW}\",[\"a\"]])")]);
    assert_eq!(rows(&workbench, cx), vec![("a".into(), 0), ("a/1".into(), 1), ("b".into(), 0)]);

    let renamed = json!([VIEW, { "b": { "handle": "b", "label": { "label": "~~B~~" }, "collapsibleState": 0 } }]);
    request(&workbench, cx, "$refresh", renamed);
    assert_eq!(sent(&receiver, cx, &[]), Vec::<String>::new(), "a changed item with its handle kept is not fetched again");
    assert!(workbench.read_with(cx, |workbench, _| workbench.tree_views.trees[VIEW].items["b"].strikethrough()));
}

#[gpui::test]
fn reveal_opens_the_view_expands_the_parents_and_selects_the_item(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = workbench_with("reveal", cx);
    request(&workbench, cx, "$registerTreeViewDataProvider", json!([VIEW, {}]));
    sent(&receiver, cx, &[]);
    let info = json!({ "item": item("a/1", Some("a"), 0), "parentChain": [item("a", None, 1)] });
    let outcome = request(&workbench, cx, "$reveal", json!([VIEW, info, { "select": true, "focus": false, "expand": false }]));
    let answers = [(Value::Null, json!([[0, item("a", None, 1)]])), (json!(["a"]), json!([[0, item("a/1", Some("a"), 0)]]))];
    // The roots, then the parent's children, then the selection.
    let mut messages = sent(&receiver, cx, &answers);
    messages.extend(sent(&receiver, cx, &answers));
    messages.extend(sent(&receiver, cx, &answers));
    assert_eq!(messages.iter().filter(|message| message.starts_with("$getChildren")).count(), 2, "{messages:?}");
    assert!(messages.contains(&format!("$setSelectionAndFocus([\"{VIEW}\",[\"a/1\"],\"a/1\"])")), "{messages:?}");
    assert_eq!(rows(&workbench, cx), vec![("a".into(), 0), ("a/1".into(), 1)]);
    let Outcome::Pending(mut reply) = outcome else { panic!("$reveal waits for the fetches") };
    assert_eq!(reply.try_recv().unwrap(), Some(Ok(Value::Null)));

    let hidden = request(&workbench, cx, "$reveal", json!(["test.hidden", info, {}]));
    let Outcome::Pending(mut reply) = hidden else { panic!() };
    cx.run_until_parked();
    assert_eq!(reply.try_recv().unwrap(), Some(Ok(Value::Null)), "a view whose when is false has no tree to reveal in");
}
