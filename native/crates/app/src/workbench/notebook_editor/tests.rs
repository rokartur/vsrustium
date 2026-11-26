use std::path::Path;
use std::sync::{Arc, Mutex};

use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::json;

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

fn notebook_workbench(cx: &mut TestAppContext) -> (gpui::Entity<Workbench>, &mut VisualTestContext) {
    let (workbench, cx, _) = workbench_with_cells(json!([{ "cellKind": 2, "language": "python", "source": "x", "outputs": [], "metadata": {} }]), cx);
    workbench.update_in(cx, |workbench, window, cx| window.focus(&workbench.editor_focus, cx));
    cx.run_until_parked();
    (workbench, cx)
}

/// An untitled notebook of `cells` shown in its editor, as `vscode.openNotebookDocument` and `showNotebookDocument`.
fn workbench_with_cells(cells: serde_json::Value, cx: &mut TestAppContext) -> (gpui::Entity<Workbench>, &mut VisualTestContext, std::sync::mpsc::Receiver<crate::host_sync::HostMessage>) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("vsrustium-notebook-editor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None);
    let (sender, receiver) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), Arc::new(Mutex::new(settings)), HostSync::new(sender), cx));
    let uri = json!({ "scheme": "untitled", "authority": "", "path": "Untitled-1.ipynb", "query": "", "fragment": "" });
    let data = json!({ "cells": cells, "metadata": {} });
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("User/keybindings.json"), cx);
        let key = crate::notebook::uri_string(&uri);
        // As `open_notebook` holds a model until its editor shows it.
        workbench.host.notebooks.held.push(key.clone());
        workbench.add_notebook(uri.clone(), "jupyter-notebook", &data, false, cx);
        workbench.show_notebook(&key, &json!({}), window, cx).unwrap();
        workbench.host.notebooks.held.clear();
    });
    cx.run_until_parked();
    (workbench, cx, receiver)
}

fn run(workbench: &gpui::Entity<Workbench>, id: &str, cx: &mut VisualTestContext) {
    let args = match id {
        "type" => vec![json!({ "text": "1" })],
        _ => Vec::new(),
    };
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(id, args, window, cx)));
    cx.run_until_parked();
}

/// The cells' sources, in order.
fn sources(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    workbench.read_with(cx, |workbench, _| {
        let model = &workbench.host.notebooks.models[0];
        model.cells.iter().map(|cell| workbench.cell_text(&cell.uri)).collect()
    })
}

#[gpui::test]
fn undo_takes_cell_edits_and_cell_text_edits_in_the_order_they_happened(cx: &mut TestAppContext) {
    let (workbench, cx) = notebook_workbench(cx);
    run(&workbench, "type", cx);
    assert_eq!(sources(&workbench, cx), ["1x"]);
    run(&workbench, "notebook.cell.insertCodeCellBelow", cx);
    assert_eq!(sources(&workbench, cx), ["1x", ""]);
    run(&workbench, "undo", cx);
    assert_eq!(sources(&workbench, cx), ["1x"]);
    run(&workbench, "undo", cx);
    assert_eq!(sources(&workbench, cx), ["x"]);
    run(&workbench, "redo", cx);
    assert_eq!(sources(&workbench, cx), ["1x"]);
    run(&workbench, "redo", cx);
    assert_eq!(sources(&workbench, cx), ["1x", ""]);
}

#[gpui::test]
fn cell_edits_of_a_workspace_edit_are_one_undo_step(cx: &mut TestAppContext) {
    let cells = json!([
        { "cellKind": 2, "language": "python", "source": "a", "outputs": [], "metadata": {} },
        { "cellKind": 2, "language": "python", "source": "b", "outputs": [], "metadata": {} },
    ]);
    let (workbench, cx, _) = workbench_with_cells(cells, cx);
    workbench.update_in(cx, |workbench, window, cx| {
        window.focus(&workbench.editor_focus, cx);
        let resource = workbench.host.notebooks.models[0].uri.clone();
        let edits = [
            json!({ "resource": resource, "cellEdit": { "editType": 1, "index": 0, "count": 1, "cells": [] } }),
            json!({ "resource": resource, "cellEdit": { "editType": 4, "index": 1, "language": "rust" } }),
        ];
        workbench.apply_workspace_cell_edits(&edits, cx).unwrap();
    });
    assert_eq!(sources(&workbench, cx), ["b"]);
    run(&workbench, "undo", cx);
    assert_eq!(sources(&workbench, cx), ["a", "b"]);
    let languages = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.host.notebooks.models[0].cells.iter().map(|cell| cell.language.clone()).collect::<Vec<_>>());
    assert_eq!(languages(cx), ["python", "python"]);
    run(&workbench, "redo", cx);
    assert_eq!(sources(&workbench, cx), ["b"]);
    assert_eq!(languages(cx), ["rust"]);
}

#[gpui::test]
fn the_focused_cell_is_the_code_editor_the_workbench_edits(cx: &mut TestAppContext) {
    let (workbench, cx) = notebook_workbench(cx);
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.editors.active().unwrap().document().scheme, crate::notebook::CELL_SCHEME));
    run(&workbench, "notebook.cell.insertCodeCellBelow", cx);
    run(&workbench, "type", cx);
    assert_eq!(sources(&workbench, cx), ["x", "1"]);
}

#[gpui::test]
fn the_cell_list_takes_the_keyboard_and_enter_and_escape_move_into_and_out_of_a_cell(cx: &mut TestAppContext) {
    let cells = json!([
        { "cellKind": 1, "language": "markdown", "source": "# Title\ntext", "outputs": [], "metadata": {} },
        { "cellKind": 2, "language": "python", "source": "a = 1\nb = 2", "outputs": [], "metadata": {} },
    ]);
    let (workbench, cx, _) = workbench_with_cells(cells, cx);
    let state = |cx: &mut VisualTestContext| {
        workbench.update_in(cx, |workbench, window, _| (workbench.active_notebook_editor().unwrap().focus, workbench.notebook_views.list_focus.is_focused(window), workbench.editor_focus.is_focused(window)))
    };
    assert_eq!(state(cx), (0, true, false), "the notebook opens with the focus on its cell list");
    cx.simulate_keystrokes("down");
    assert_eq!(state(cx), (1, true, false));
    cx.simulate_keystrokes("enter");
    assert_eq!(state(cx), (1, false, true), "Enter edits the focused cell");
    cx.simulate_keystrokes("up");
    assert_eq!(state(cx), (0, true, false), "Up on the first line goes to the cell above, a markdown preview takes no editor focus");
    cx.simulate_keystrokes("enter");
    assert_eq!(state(cx), (0, false, true), "Enter edits the markdown cell");
    cx.simulate_keystrokes("escape");
    assert_eq!(state(cx), (0, true, false), "Escape leaves the cell editor for the list");
    cx.simulate_keystrokes("cmd-down");
    assert_eq!(state(cx), (1, true, false));
    let crumbs = workbench.read_with(cx, |workbench, _| workbench.notebook_breadcrumbs(gpui::rgba(0xffffffff), 22.).unwrap().1.into_iter().map(|(_, label)| label).collect::<Vec<_>>());
    assert_eq!(crumbs, ["Title", "a = 1"], "the breadcrumbs name the header and the code cell below it");
}

#[gpui::test]
fn contributed_cell_status_items_follow_their_provider(cx: &mut TestAppContext) {
    let cells = json!([{ "cellKind": 2, "language": "python", "source": "a = 1", "outputs": [], "metadata": {} }]);
    let (workbench, cx, receiver) = workbench_with_cells(cells, cx);
    let answer = |receiver: &std::sync::mpsc::Receiver<crate::host_sync::HostMessage>, cache: u64, text: &str, cx: &mut VisualTestContext| {
        let mut count = 0;
        for message in receiver.try_iter().filter(|message| message.method == "$provideNotebookCellStatusBarItems") {
            assert_eq!((message.args[0].clone(), message.args[2].clone()), (json!(7), json!(0)));
            count += 1;
            let _ = message.reply.expect("a call").send(Ok(json!({ "cacheId": cache, "items": [{ "text": text, "alignment": 1 }] })));
        }
        cx.run_until_parked();
        count
    };
    let items = |cx: &mut VisualTestContext| {
        workbench.read_with(cx, |workbench, _| {
            let key = workbench.active_notebook_editor().unwrap().notebook.clone();
            let handle = workbench.host.notebooks.model(&key).unwrap().cells[0].handle;
            workbench.notebook_views.status_bars.items(&key, handle).iter().map(|item| item["text"].as_str().unwrap().to_owned()).collect::<Vec<_>>()
        })
    };
    let request = |method: &'static str, args: Vec<serde_json::Value>, cx: &mut VisualTestContext| workbench.update_in(cx, |workbench, window, cx| _ = workbench.handle_notebook_request("MainThreadNotebook", method, &args, window, cx));
    request("$registerNotebookCellStatusBarItemProvider", vec![json!(7), json!(8), json!("jupyter-notebook")], cx);
    assert_eq!(answer(&receiver, 1, "first", cx), 1);
    assert_eq!(items(cx), ["first"]);
    request("$emitCellStatusBarEvent", vec![json!(8)], cx);
    let released = |receiver: &std::sync::mpsc::Receiver<crate::host_sync::HostMessage>| receiver.try_iter().filter(|message| message.method == "$releaseNotebookCellStatusBarItems").map(|message| message.args[0].clone()).collect::<Vec<_>>();
    assert_eq!(answer(&receiver, 2, "second", cx), 1);
    assert_eq!(items(cx), ["second"]);
    assert_eq!(released(&receiver), [json!(1)], "the replaced list is released");
}

#[gpui::test]
fn the_variable_provider_command_collects_what_the_bound_kernel_sends(cx: &mut TestAppContext) {
    let cells = json!([{ "cellKind": 2, "language": "python", "source": "a = 1", "outputs": [], "metadata": {} }]);
    let (workbench, cx, receiver) = workbench_with_cells(cells, cx);
    let kernel = json!({ "id": "pub.ext/k", "notebookType": "jupyter-notebook", "label": "K", "hasVariableProvider": true, "extensionLocation": { "scheme": "file", "path": "/ext" }, "preloads": [] });
    let outcome = workbench.update_in(cx, |workbench, window, cx| {
        workbench.handle_notebook_request("MainThreadNotebookKernels", "$addKernel", &[json!(3), kernel], window, cx);
        let uri = workbench.host.notebooks.models[0].uri.clone();
        workbench.host.notebooks.kernels.select(Some("pub.ext/k"), "jupyter-notebook", &uri);
        workbench.execute_command("_executeNotebookVariableProvider", vec![uri], window, cx)
    });
    let crate::commands::Outcome::Pending(answer) = outcome else { panic!("the command waits for the kernel") };
    let call = receiver.try_iter().find(|message| message.method == "$provideVariables").expect("the kernel is asked");
    assert_eq!((call.args[0].clone(), call.args[1].clone(), call.args[4].clone()), (json!(3), json!("3variables0"), json!("named")));
    workbench.update_in(cx, |workbench, window, cx| {
        let variable = json!({ "variable": { "name": "a", "value": "1" }, "hasNamedChildren": false, "indexedChildrenCount": 0 });
        workbench.handle_notebook_request("MainThreadNotebookKernels", "$receiveVariable", &[json!("3variables0"), variable], window, cx);
    });
    let _ = call.reply.expect("a call").send(Ok(serde_json::Value::Null));
    cx.run_until_parked();
    let collected = futures::executor::block_on(answer).unwrap().unwrap();
    assert_eq!(collected[0]["variable"]["name"], "a");
}
