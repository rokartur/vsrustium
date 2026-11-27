use std::path::Path;
use std::sync::{Arc, Mutex};

use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::json;

use super::*;
use crate::host_sync::{HostMessage, HostSync};
use crate::languages::Languages;
use crate::theme::load_theme;

fn run(workbench: &gpui::Entity<Workbench>, id: &str, args: Vec<Value>, cx: &mut VisualTestContext) {
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(id, args, window, cx)));
    cx.run_until_parked();
}

fn sources(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    workbench.read_with(cx, |workbench, _| workbench.host.notebooks.models[0].cells.iter().map(|cell| workbench.cell_text(&cell.uri)).collect())
}

#[test]
fn the_label_names_the_untitled_number_or_the_file() {
    assert_eq!(repl_label(&json!({ "scheme": "untitled", "path": "Untitled-3.ipynb" })), "REPL - 3");
    assert_eq!(repl_label(&json!({ "scheme": "untitled", "path": "Untitled-3" })), "REPL - Untitled-3");
    assert_eq!(repl_label(&json!({ "scheme": "file", "path": "/work/a.ipynb" })), "REPL - a.ipynb");
}

/// repl.contribution.ts and replEditorInput.ts: a notebook shown as `repl` gets an input box cell after its markdown
/// cell, the list shows the cells before it, `repl.execute` inserts the input before it and empties it, and closing
/// the scratchpad asks nothing and reverts the notebook softly.
#[gpui::test]
fn a_notebook_shown_as_repl_executes_its_last_cell(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("vsrustium-repl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None);
    let (sender, receiver) = std::sync::mpsc::channel::<HostMessage>();
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), Arc::new(Mutex::new(settings)), HostSync::new(sender), cx));
    let uri = json!({ "scheme": "untitled", "authority": "", "path": "Untitled-1.ipynb", "query": "", "fragment": "" });
    let key = notebook::uri_string(&uri);
    let data = json!({ "cells": [{ "cellKind": 1, "language": "markdown", "source": "# Notes", "outputs": [], "metadata": {} }], "metadata": {} });
    let shown = workbench.update_in(cx, |workbench, window, cx| {
        workbench.host.notebooks.held.push(key.clone());
        workbench.add_notebook(uri.clone(), "jupyter-notebook", &data, true, cx);
        workbench.handle_notebook_request("MainThreadNotebookEditors", "$tryShowNotebookDocument", &[uri.clone(), json!("repl"), json!({ "preserveFocus": false })], window, cx)
    });
    cx.run_until_parked();
    let Outcome::Ready(Ok(editor_id)) = shown else { panic!("the REPL editor shows at once") };
    assert!(editor_id.is_string());
    assert_eq!(sources(&workbench, cx), ["# Notes", ""], "an input box cell after the markdown cell");
    let (tab, rows, active_editor) = workbench.update_in(cx, |workbench, window, _| {
        let tab = workbench.editors.active_tab().unwrap();
        let rows = workbench.notebook_rows(tab.model_id, &workbench.host.notebooks.models[0]).len();
        (workbench.editor_name(tab), rows, workbench.key_context(window)["activeEditor"].clone())
    });
    assert_eq!((tab.as_str(), rows), ("REPL - 1", 1), "the list leaves the input cell out");
    assert_eq!(active_editor, "workbench.editor.repl");
    let status = workbench.update_in(cx, |workbench, window, _| workbench.repl_input_status(window).map(|(input, cells)| (input.document().lines.clone(), cells.map(|(_, index, count)| (index, count)))));
    assert_eq!(status, Some((vec![String::new()], Some((0, 1)))), "the status bar takes the input's end of line and the list's cell entries");
    let delta = receiver.try_iter().find(|message| message.method == "$acceptDocumentAndEditorsDelta" && json!(message.args).to_string().contains("addedEditors")).expect("the editor reaches the host");
    assert!(json!(delta.args).to_string().contains(r#""viewType":"repl""#), "{:?}", delta.args);

    let hint = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.input_hint_keybinding().map(|chord| chord.key_caps()));
    assert_eq!(hint(cx), None, "no keybindings loaded yet");
    workbench.update(cx, |workbench, cx| workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("User/keybindings.json"), cx));
    assert_eq!(hint(cx), Some(vec!["Enter".to_owned()]));
    workbench.update(cx, |workbench, _| workbench.update_setting("interactiveWindow.executeWithShiftEnter", json!(true)));
    assert_eq!(hint(cx), Some(vec!["⇧".to_owned(), "Enter".to_owned()]));
    workbench.update(cx, |workbench, _| workbench.update_setting("interactiveWindow.executeWithShiftEnter", json!(false)));

    run(&workbench, "type", vec![json!({ "text": "1+1" })], cx);
    assert_eq!(sources(&workbench, cx), ["# Notes", "1+1"], "the input box is the last cell");
    run(&workbench, "repl.execute", Vec::new(), cx);
    assert_eq!(sources(&workbench, cx), ["# Notes", "1+1", ""]);
    let most_recent = workbench.update_in(cx, |workbench, window, _| workbench.key_context(window)["mostRecentReplEditor"].clone());
    assert_eq!(most_recent, json!(key));
    run(&workbench, "interactive.history.previous", Vec::new(), cx);
    assert_eq!(sources(&workbench, cx)[2], "1+1");

    run(&workbench, "repl.focusLastItemExecuted", Vec::new(), cx);
    let focus = workbench.read_with(cx, |workbench, _| workbench.host.notebooks.editors[0].focus);
    assert_eq!(focus, 1, "the executed cell, not the input");
    run(&workbench, "notebook.cell.edit", Vec::new(), cx);
    run(&workbench, "type", vec![json!({ "text": "x" })], cx);
    assert_eq!(sources(&workbench, cx)[1], "1+1", "history cells are read-only");

    let (dirty, badge) = workbench.read_with(cx, |workbench, _| (workbench.tab_shows_dirty(workbench.editors.active_index().unwrap()), workbench.dirty_working_copy_count()));
    assert!(dirty, "the tab shows its notebook's dirty state");
    assert_eq!(badge, 1, "the explorer's badge counts the dirty notebook");
    receiver.try_iter().count();
    run(&workbench, "workbench.action.closeActiveEditor", Vec::new(), cx);
    let open = workbench.read_with(cx, |workbench, _| workbench.editors.active_tab().is_some());
    assert!(!open, "a scratchpad closes without asking");
    let messages: Vec<HostMessage> = receiver.try_iter().collect();
    assert!(messages.iter().any(|message| message.method == "$acceptDirtyStateChanged" && message.args[1] == false), "reverted softly");
    let most_recent = workbench.update_in(cx, |workbench, window, _| workbench.key_context(window).get("mostRecentReplEditor").cloned());
    assert_eq!(most_recent, None);
}
