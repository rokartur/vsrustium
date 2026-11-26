use std::path::Path;
use std::sync::{Arc, Mutex};

use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::json;

use super::*;
use crate::host_sync::{HostMessage, HostSync};
use crate::languages::Languages;
use crate::theme::load_theme;

/// history.test.ts `HistoryNavigator2`: adding moves an entry to the end, replacing the last drops its older copy,
/// and the cursor stops at both ends.
#[test]
fn history_keeps_distinct_values_with_the_newest_last() {
    let mut history = History::new("1");
    history.add("2");
    history.add("3");
    history.add("2");
    assert_eq!(history.previous(), "3");
    assert_eq!(history.previous(), "1");
    assert_eq!(history.previous(), "1");
    assert_eq!(history.next(), "3");
    history.reset_cursor();
    history.replace_last("1");
    assert_eq!(history.current(), "1");
    assert_eq!(history.previous(), "3");
    assert_eq!(history.previous(), "3", "the first 1 went with the replace");
}

fn run(workbench: &gpui::Entity<Workbench>, id: &str, args: Vec<Value>, cx: &mut VisualTestContext) {
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(id, args, window, cx)));
    cx.run_until_parked();
}

fn input_text(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    workbench.read_with(cx, |workbench, _| workbench.editors.model(INPUT_SCHEME, Path::new("/InteractiveInput-1")).unwrap().document().lines.join("\n"))
}

/// interactive.contribution.ts: `_interactive.open` opens an empty untitled interactive notebook with its input
/// editor focused, `interactive.execute` turns the input into a cell at the end and empties it, the history brings
/// it back, and closing the tab lets the input go in the host.
#[gpui::test]
fn the_interactive_window_executes_its_input_as_cells(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("vsrustium-interactive-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None);
    let (sender, receiver) = std::sync::mpsc::channel::<HostMessage>();
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), Arc::new(Mutex::new(settings)), HostSync::new(sender), cx));
    let answer = workbench.update_in(cx, |workbench, window, cx| match workbench.execute_command("_interactive.open", vec![json!({ "viewColumn": -1, "preserveFocus": false })], window, cx) {
        Outcome::Pending(receiver) => receiver,
        Outcome::Ready(_) => panic!("opening resolves the notebook"),
    });
    cx.run_until_parked();
    let opened = futures::executor::block_on(answer).unwrap().unwrap();
    assert_eq!((opened["notebookUri"]["scheme"].clone(), opened["notebookUri"]["path"].clone()), (json!("untitled"), json!("/Interactive-1.interactive")));
    assert_eq!(opened["inputUri"]["path"], "/InteractiveInput-1");
    assert!(opened["notebookEditorId"].is_string());
    let methods: Vec<&str> = receiver.try_iter().map(|message| message.method).collect();
    assert!(methods.contains(&"$willAddInteractiveDocument"), "{methods:?}");
    assert!(!methods.contains(&"$dataToNotebook"), "an empty interactive window needs no serializer");
    let tab = workbench.read_with(cx, |workbench, _| workbench.editor_name(workbench.editors.active_tab().unwrap()));
    assert_eq!(tab, "Interactive-1");

    run(&workbench, "type", vec![json!({ "text": "1+1" })], cx);
    assert_eq!(input_text(&workbench, cx), "1+1");
    run(&workbench, "interactive.execute", Vec::new(), cx);
    let (sources, dirty) = workbench.read_with(cx, |workbench, _| {
        let model = &workbench.host.notebooks.models[0];
        (model.cells.iter().map(|cell| workbench.cell_text(&cell.uri)).collect::<Vec<_>>(), model.dirty)
    });
    assert_eq!(sources, ["1+1"]);
    assert!(!dirty, "a scratchpad");
    assert_eq!(input_text(&workbench, cx), "");
    run(&workbench, "interactive.history.previous", Vec::new(), cx);
    assert_eq!(input_text(&workbench, cx), "1+1");
    run(&workbench, "interactive.history.next", Vec::new(), cx);
    assert_eq!(input_text(&workbench, cx), "");
    receiver.try_iter().count();
    run(&workbench, "interactive.history.focus", Vec::new(), cx);
    let deltas: Vec<Value> = receiver.try_iter().filter(|message| message.method == "$acceptDocumentsAndEditorsDelta").map(|message| message.args[0].clone()).collect();
    assert!(deltas.iter().any(|delta| delta.get("newActiveEditor").is_some()), "the cell's editor becomes active: {deltas:?}");
    assert!(deltas.iter().all(|delta| delta.get("removedEditors").is_none()), "the input editor stays visible while a cell has the focus: {deltas:?}");
    let saved = workbench.read_with(cx, |workbench, _| workbench.notebook_editor_input(workbench.editors.active_tab().unwrap()).unwrap());
    assert_eq!(saved["id"], "workbench.input.interactive");
    let value: Value = serde_json::from_str(saved["value"].as_str().unwrap()).unwrap();
    assert_eq!((value["inputResource"]["path"].clone(), value["name"].clone()), (json!("/InteractiveInput-1"), json!("Interactive-1")));

    receiver.try_iter().count();
    run(&workbench, "workbench.action.closeActiveEditor", Vec::new(), cx);
    let methods: Vec<&str> = receiver.try_iter().map(|message| message.method).collect();
    assert!(methods.contains(&"$willRemoveInteractiveDocument"), "{methods:?}");
    assert!(workbench.read_with(cx, |workbench, _| workbench.host.notebooks.interactives.is_empty() && workbench.editors.model(INPUT_SCHEME, Path::new("/InteractiveInput-1")).is_none()));
}
