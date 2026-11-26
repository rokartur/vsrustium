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
    // One folder per call: tests run in parallel threads of one process and would remove each other's.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let run = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("vsrustium-notebook-editor-{}-{run}", std::process::id()));
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
fn a_click_on_the_text_of_another_code_cell_edits_that_cell(cx: &mut TestAppContext) {
    let cells = json!([
        { "cellKind": 2, "language": "python", "source": "a = 1", "outputs": [], "metadata": {} },
        { "cellKind": 2, "language": "python", "source": "b = 2", "outputs": [], "metadata": {} },
    ]);
    let (workbench, cx, _) = workbench_with_cells(cells, cx);
    let handle = workbench.read_with(cx, |workbench, _| workbench.host.notebooks.models[0].cells[1].handle);
    let bounds = cx.debug_bounds(Box::leak(format!("notebook-cell-editor-{handle}").into_boxed_str())).unwrap();
    cx.simulate_click(bounds.origin + gpui::point(gpui::px(60.), gpui::px(8.)), gpui::Modifiers::default());
    workbench.update_in(cx, |workbench, window, _| {
        assert_eq!(workbench.active_notebook_editor().unwrap().focus, 1);
        assert!(workbench.editor_focus.is_focused(window));
        let editor = workbench.editors.active().unwrap();
        assert_eq!(editor.document().lines[0], "b = 2");
        assert_eq!(editor.selection().head.line, 0);
    });
}

#[gpui::test]
fn notebook_find_goes_through_the_matches_of_all_cells_and_replaces_them(cx: &mut TestAppContext) {
    let cells = json!([
        { "cellKind": 2, "language": "python", "source": "x = 1\nx", "outputs": [], "metadata": {} },
        { "cellKind": 2, "language": "python", "source": "y = x", "outputs": [], "metadata": {} },
        { "cellKind": 1, "language": "markdown", "source": "x md", "outputs": [], "metadata": {} },
    ]);
    let (workbench, cx, _) = workbench_with_cells(cells, cx);
    cx.simulate_keystrokes("cmd-f");
    workbench.update_in(cx, |workbench, window, cx| {
        assert!(workbench.notebook_views.find.find_focus.is_focused(window), "Cmd+F in the cell list focuses the notebook find input");
        workbench.edit_notebook_find_input(super::super::input_box_view::InputId::NotebookFind, |input| input.set_value("x".into()), cx);
    });
    cx.run_until_parked();
    let label = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.notebook_find_count_label());
    assert_eq!(label(cx), "1 of 3", "a markdown preview's source is left to the rendered markdown filter");
    cx.simulate_keystrokes("enter");
    cx.simulate_keystrokes("enter");
    assert_eq!(label(cx), "3 of 3");
    workbench.update_in(cx, |workbench, window, _| {
        assert_eq!(workbench.active_notebook_editor().unwrap().focus, 1, "the match's cell takes the focus");
        assert!(workbench.notebook_views.find.find_focus.is_focused(window), "the find input keeps the keyboard");
        let editor = workbench.editors.active().unwrap();
        assert_eq!((editor.selection().start(), editor.selection().end()), (crate::editor::Position::new(0, 4), crate::editor::Position::new(0, 5)));
    });
    cx.simulate_keystrokes("shift-enter");
    assert_eq!(label(cx), "2 of 3");
    run(&workbench, "_notebook.find.filter.markupPreview", cx);
    assert_eq!(label(cx), "2 of 4", "without the rendered markdown filter the markdown source counts");
    workbench.update_in(cx, |workbench, _, cx| {
        workbench.notebook_views.find.state.replace_string = "z".into();
        workbench.notebook_find_replace_all(cx);
    });
    cx.run_until_parked();
    assert_eq!(sources(&workbench, cx), ["z = 1\nz", "y = z", "z md"]);
    assert_eq!(label(cx), "No results");
    cx.simulate_keystrokes("escape");
    workbench.update_in(cx, |workbench, window, _| {
        assert!(!workbench.notebook_find_visible());
        assert!(workbench.notebook_views.list_focus.is_focused(window), "the focus returns to the cell list");
    });
}

#[gpui::test]
fn a_reopened_notebook_editor_brings_back_its_focused_cell_editor_and_selection(cx: &mut TestAppContext) {
    let cells = json!([
        { "cellKind": 2, "language": "python", "source": "a = 1", "outputs": [], "metadata": {} },
        { "cellKind": 2, "language": "python", "source": "b = 22", "outputs": [], "metadata": {} },
    ]);
    let (workbench, cx, _) = workbench_with_cells(cells, cx);
    cx.simulate_keystrokes("down enter end shift-left");
    let key = workbench.read_with(cx, |workbench, _| workbench.host.notebooks.models[0].uri_string());
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.host.notebooks.held.push(key.clone());
        drop(workbench.execute_command("workbench.action.closeActiveEditor", Vec::new(), window, cx));
    });
    cx.run_until_parked();
    workbench.read_with(cx, |workbench, _| assert!(workbench.active_notebook_editor().is_none()));
    workbench.update_in(cx, |workbench, window, cx| workbench.show_notebook(&key, &json!({}), window, cx).unwrap());
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, _| {
        assert_eq!(workbench.active_notebook_editor().unwrap().focus, 1);
        assert!(workbench.editor_focus.is_focused(window), "the cell editor had the keyboard");
        let editor = workbench.editors.active().unwrap();
        assert_eq!(editor.document().lines[0], "b = 22");
        assert_eq!((editor.selection().anchor, editor.selection().head), (crate::editor::Position::new(0, 6), crate::editor::Position::new(0, 5)));
    });
}

#[gpui::test]
fn a_markdown_cell_toolbar_goes_with_its_more_menu(cx: &mut TestAppContext) {
    let cells = json!([
        { "cellKind": 2, "language": "python", "source": "x", "outputs": [], "metadata": {} },
        { "cellKind": 1, "language": "markdown", "source": "# Title", "outputs": [], "metadata": {} },
    ]);
    let (workbench, cx, _) = workbench_with_cells(cells, cx);
    let (tab, handle) = workbench.read_with(cx, |workbench, _| (workbench.host.notebooks.editors[0].tab_model_id, workbench.host.notebooks.models[0].cells[1].handle));
    workbench.update(cx, |workbench, cx| {
        workbench.notebook_views.views.entry(tab).or_default().hovered = Some(handle);
        cx.notify();
    });
    cx.run_until_parked();
    let more = Box::leak(format!("notebook-cell-toolbar-more-{handle}").into_boxed_str());
    let center = cx.debug_bounds(more).unwrap().center();
    cx.simulate_click(center, gpui::Modifiers::default());
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds(more).is_none(), "the hidden webview never reports the pointer leaving the preview for the menu");
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

/// editActions.ts `ChangeCellLanguageAction`: the status bar's language item opens the picker (Auto Detect, then
/// markdown and the cell's language, then the others by id); markdown turns the code cell into a markup cell with
/// its text, another language back into a code cell, each one undo step.
#[gpui::test]
fn the_cell_language_picker_changes_the_language_and_the_kind(cx: &mut TestAppContext) {
    let (workbench, cx, _) = workbench_with_cells(json!([{ "cellKind": 2, "language": "python", "source": "x", "outputs": [], "metadata": {} }]), cx);
    let languages = json!({ "contributes": { "languages": [
        { "id": "python", "aliases": ["Python"] },
        { "id": "rust", "aliases": ["Rust"] },
        { "id": "markdown", "aliases": ["Markdown"] },
        { "id": "css", "aliases": ["CSS"] },
    ] } });
    workbench.update(cx, |workbench, _| workbench.languages = Languages::new(&[exthost::scan::ScannedExtension { id: "port.languages".into(), location: "/ext".into(), description: languages }]));
    let handle = workbench.read_with(cx, |workbench, _| workbench.host.notebooks.models[0].cells[0].handle);
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("notebook.cell.changeLanguage", vec![json!({ "$mid": 13, "cell": { "handle": handle, "index": 0 } })], window, cx)));
    cx.run_until_parked();
    let rows = workbench.read_with(cx, |workbench, _| {
        let input = workbench.quick_inputs.current().unwrap();
        input.order.iter().map(|row| &input.rows[*row]).map(|row| format!("{}{} {}", row.separator.as_ref().map(|label| format!("--{label}--")).unwrap_or_default(), row.item.label, row.item.description.clone().unwrap_or_default())).collect::<Vec<_>>()
    });
    assert_eq!(rows, ["Auto Detect ", "--languages (identifier)--Python (python) - Current Language", "Markdown (markdown)", "----Binary (code-text-binary)", "CSS (css)", "Plain Text (plaintext)", "Rust (rust)"]);
    workbench.update_in(cx, |workbench, window, cx| {
        let input = workbench.quick_inputs.find_mut(|owner| matches!(owner, crate::main_thread_quick_open::Owner::EditorStatus(_))).unwrap();
        let handle = input.rows.iter().find(|row| row.item.label == "Markdown").unwrap().item.handle;
        input.set_active_items(&[handle]);
        workbench.flush_quick_input_events(window, cx);
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let cell = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| {
        let cell = &workbench.host.notebooks.models[0].cells[0];
        (cell.kind, cell.language.clone(), workbench.cell_text(&cell.uri))
    });
    assert_eq!(cell(cx), (crate::notebook::MARKUP, "markdown".into(), "x".into()));
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("notebook.cell.changeLanguage", vec![json!({ "start": 0, "end": 1 }), json!("rust")], window, cx)));
    cx.run_until_parked();
    assert_eq!(cell(cx), (crate::notebook::CODE, "rust".into(), "x".into()));
    run(&workbench, "undo", cx);
    assert_eq!(cell(cx).0, crate::notebook::MARKUP);
    run(&workbench, "undo", cx);
    assert_eq!(cell(cx), (crate::notebook::CODE, "python".into(), "x".into()));
}

/// cellOutput.ts `_attachToolbar`: each output's `...` menu has the `NotebookOutputToolbar` actions that hold for it
/// (Clear Cell Outputs on the first output, no Copy on a stream after a stream, Show Empty Outputs while one is
/// hidden) and Change Presentation first for an output with more than one; their commands copy, show and re-pick.
#[gpui::test]
fn the_output_toolbar_menu_and_its_commands(cx: &mut TestAppContext) {
    let item = |mime: &str, text: &str| json!({ "mime": mime, "valueBytes": text.as_bytes() });
    let outputs = json!([
        { "outputId": "a", "items": [item("application/vnd.code.notebook.stdout", "hello\n")] },
        { "outputId": "b", "items": [item("application/vnd.code.notebook.stdout", "world\n")] },
        { "outputId": "c", "items": [item("text/html", "<b>x</b>"), item("text/plain", "x")] },
    ]);
    let (workbench, cx, _) = workbench_with_cells(json!([{ "cellKind": 2, "language": "python", "source": "x", "outputs": outputs, "metadata": {} }]), cx);
    let renderers = json!({ "contributes": { "notebookRenderer": [
        { "id": "builtin", "displayName": "Built-in", "mimeTypes": ["text/plain", "application/vnd.code.notebook.stdout"], "entrypoint": "a.js" },
        { "id": "html", "displayName": "HTML", "mimeTypes": ["text/html"], "entrypoint": "b.js" },
    ] } });
    workbench.update(cx, |workbench, _| workbench.extensions = vec![exthost::scan::ScannedExtension { id: "port.renderers".into(), location: "/ext".into(), description: renderers }]);
    let menu = |index: usize, cx: &mut VisualTestContext| {
        workbench.update_in(cx, |workbench, window, _| {
            let editor = workbench.active_notebook_editor().unwrap();
            let model = workbench.host.notebooks.model(&editor.notebook).unwrap();
            let actions = workbench.output_menu_actions(editor.tab_model_id, model, 0, index, window);
            actions.into_iter().map(|action| match action {
                crate::menus::MenuAction::Command { title, .. } => title,
                other => format!("{other:?}"),
            }).collect::<Vec<_>>()
        })
    };
    assert_eq!(menu(0, cx), ["Clear Cell Outputs", "Copy Cell Output", "Show Empty Outputs"], "the unmeasured outputs after the first are hidden");
    assert_eq!(menu(1, cx), ["Show Empty Outputs"]);
    assert_eq!(menu(2, cx), ["Change Presentation", "Copy Cell Output", "Show Empty Outputs"]);
    let uri = workbench.read_with(cx, |workbench, _| workbench.host.notebooks.models[0].uri.clone());
    let context = |output: &str| vec![json!({ "start": 0, "end": 1, "outputId": output }), uri.clone()];
    let command = |id: &str, args: Vec<serde_json::Value>, cx: &mut VisualTestContext| {
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(id, args, window, cx)));
        cx.run_until_parked();
    };
    command("notebook.cellOutput.copy", context("a"), cx);
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some("hello\nworld".into()), "a stream copies the streams from it on");
    command("notebook.cellOuput.showEmptyOutputs", context("a"), cx);
    assert_eq!(menu(0, cx), ["Clear Cell Outputs", "Copy Cell Output"]);
    command("notebook.output.pickMimetype", context("c"), cx);
    let rows = workbench.read_with(cx, |workbench, _| {
        let input = workbench.quick_inputs.current().unwrap();
        input.order.iter().map(|row| &input.rows[*row]).map(|row| format!("{} {}", row.item.label, row.item.description.clone().unwrap_or_default())).collect::<Vec<_>>()
    });
    assert_eq!(rows, ["text/html Currently Active", "text/plain "]);
    workbench.update_in(cx, |workbench, window, cx| {
        let input = workbench.quick_inputs.find_mut(|_| true).unwrap();
        let handle = input.rows.iter().find(|row| row.item.label == "text/plain").unwrap().item.handle;
        input.set_active_items(&[handle]);
        workbench.flush_quick_input_events(window, cx);
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let picked = workbench.read_with(cx, |workbench, _| {
        let model = &workbench.host.notebooks.models[0];
        workbench.output_presentations(&model.view_type, &model.cells[0].outputs[2])[0].0.clone()
    });
    assert_eq!(picked, "text/plain", "the picked presentation shows from now on");
}

/// NotebookEditorSerializer: the editor part saves a notebook tab as `workbench.input.notebook` and opens it again
/// from workspace storage once its notebook resolves.
#[gpui::test]
fn a_notebook_editor_comes_back_from_workspace_storage(cx: &mut TestAppContext) {
    let (workbench, cx, _) = workbench_with_cells(json!([{ "cellKind": 2, "language": "python", "source": "x", "outputs": [], "metadata": {} }]), cx);
    let saved = workbench.update(cx, |workbench, _| {
        workbench.save_editor_part_state();
        workbench.workspace_storage.lock().unwrap().get("memento/workbench.parts.editor").map(str::to_owned).unwrap()
    });
    let state: serde_json::Value = serde_json::from_str(&saved).unwrap();
    let editor = &state["editorpart.state"]["serializedGrid"]["root"]["data"][0]["data"]["editors"][0];
    assert_eq!(editor["id"], "workbench.input.notebook");
    let value: serde_json::Value = serde_json::from_str(editor["value"].as_str().unwrap()).unwrap();
    assert_eq!((value["viewType"].clone(), value["resource"]["path"].clone()), (json!("jupyter-notebook"), json!("Untitled-1.ipynb")));
    let key = workbench.update(cx, |workbench, _| {
        let key = workbench.host.notebooks.models[0].uri_string();
        workbench.host.notebooks.held.push(key.clone());
        key
    });
    run(&workbench, "workbench.action.closeAllEditors", cx);
    assert!(workbench.read_with(cx, |workbench, _| workbench.editors.editors.is_empty()));
    let storage = Arc::new(Mutex::new(storage::Storage::in_memory()));
    storage.lock().unwrap().set("memento/workbench.parts.editor", saved);
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.restore_workspace_state(storage);
        workbench.restore_notebook_editors(window, cx);
    });
    cx.run_until_parked();
    let tabs = workbench.read_with(cx, |workbench, _| workbench.editors.editors.iter().map(|editor| (editor.document().path.to_string_lossy().into_owned(), editor.preview)).collect::<Vec<_>>());
    assert_eq!(tabs, [(key, false)]);
}

/// layoutActions.ts: the notebook editor title's gear opens Customize Notebook..., whose toggles show their short
/// titles, checked by and flipping their settings.
#[gpui::test]
fn the_customize_notebook_submenu(cx: &mut TestAppContext) {
    let (workbench, cx, _) = workbench_with_cells(json!([{ "cellKind": 2, "language": "python", "source": "x", "outputs": [], "metadata": {} }]), cx);
    let menu = |cx: &mut VisualTestContext| {
        workbench.update_in(cx, |workbench, window, _| {
            let context = workbench.key_context(window);
            let title = crate::menus::menu_actions(&workbench.menus, crate::menus::MenuId::EditorTitle, &context).into_iter().any(|action| matches!(action, crate::menus::MenuAction::Submenu { ref title, .. } if title == "Customize Notebook..."));
            assert!(title, "the gear is in the notebook's editor title");
            let actions = crate::menus::menu_actions(&workbench.menus, crate::menus::MenuId::NotebookEditorLayoutConfigure, &context);
            actions.into_iter().filter_map(|action| match action {
                crate::menus::MenuAction::Command { title, checked, .. } => Some((title, checked)),
                _ => None,
            }).collect::<Vec<_>>()
        })
    };
    let entry = |title: &str, checked: bool| (title.to_owned(), checked);
    assert_eq!(menu(cx), [entry("Customize Notebook Layout", false), entry("Line Numbers", false), entry("Breadcrumbs", true), entry("Toggle Cell Toolbar Position", false)]);
    run(&workbench, "notebook.toggleLineNumbersFromEditorTitle", cx);
    assert_eq!(menu(cx)[1], entry("Line Numbers", true));
}

/// The titles of menu actions: separators as `-`, submenus as `<title> >`.
fn titles(actions: &[crate::menus::MenuAction]) -> Vec<String> {
    let mut titles = Vec::new();
    for action in actions {
        titles.push(match action {
            crate::menus::MenuAction::Command { title, .. } => title.clone(),
            crate::menus::MenuAction::Submenu { title, .. } => format!("{title} >"),
            crate::menus::MenuAction::Separator => "-".to_owned(),
        });
    }
    titles
}

fn run_on_cell(workbench: &gpui::Entity<Workbench>, id: &str, index: usize, cx: &mut VisualTestContext) {
    let args = vec![json!({ "start": index, "end": index + 1, "ui": true })];
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(id, args, window, cx)));
    cx.run_until_parked();
}

/// cellToolbars.ts: a cell's buttons, `...` dropdown and delete button from `MenuId.NotebookCellTitle` and
/// `NotebookCellDelete`, as the reference shows them for an ipynb without the Jupyter extension.
#[gpui::test]
fn the_cell_toolbar_menus(cx: &mut TestAppContext) {
    let cells = json!([
        { "cellKind": 1, "language": "markdown", "source": "# Title", "outputs": [], "metadata": {} },
        { "cellKind": 2, "language": "python", "source": "x", "outputs": [], "metadata": {} },
    ]);
    let (workbench, cx, _) = workbench_with_cells(cells, cx);
    let toolbar = |index: usize, cx: &mut VisualTestContext| {
        workbench.update_in(cx, |workbench, window, _| {
            let model = &workbench.host.notebooks.models[0];
            let row = workbench.notebook_rows(workbench.host.notebooks.editors[0].tab_model_id, model).into_iter().nth(index).unwrap();
            let (primary, more, delete) = workbench.cell_toolbar_actions(model, &row, window);
            (titles(&primary), titles(&more), titles(&delete))
        })
    };
    let (primary, more, delete) = toolbar(0, cx);
    assert_eq!(primary, ["Run Cells In Section", "Edit Cell", "Split Cell"]);
    let overflow = ["Cut Cell", "Copy Cell", "Paste Cell", "-", "Insert Cell >", "-", "Change Cell to Code", "Join With Previous Cell", "Join With Next Cell", "Join Selected Cells", "Copy Cell Down", "-", "Show Cell Line Numbers", "Toggle Cell Toolbar Position"];
    assert_eq!(more, overflow);
    assert_eq!(delete, ["Delete Cell"]);
    // `notebookMissingKernelExtension`: running above and below shows without a kernel.
    let (primary, more, _) = toolbar(1, cx);
    assert_eq!(primary, ["Execute Above Cells", "Execute Cell and Below", "Split Cell"]);
    assert_eq!(more[6], "Change Cell to Markdown");
}

/// foldingController.ts: Fold Cell and Unfold Cell on the innermost section of the focused markdown cell; collapsed
/// sections hide their cells and keep the sections inside them collapsed.
#[gpui::test]
fn markdown_headers_fold_their_sections(cx: &mut TestAppContext) {
    let cells = json!([
        { "cellKind": 1, "language": "markdown", "source": "# A", "outputs": [], "metadata": {} },
        { "cellKind": 2, "language": "python", "source": "x", "outputs": [], "metadata": {} },
        { "cellKind": 1, "language": "markdown", "source": "## B", "outputs": [], "metadata": {} },
        { "cellKind": 2, "language": "python", "source": "y", "outputs": [], "metadata": {} },
    ]);
    let (workbench, cx, _) = workbench_with_cells(cells, cx);
    let rows = |cx: &mut VisualTestContext| {
        workbench.read_with(cx, |workbench, _| {
            let model = &workbench.host.notebooks.models[0];
            let rows = workbench.notebook_rows(workbench.host.notebooks.editors[0].tab_model_id, model);
            (rows.iter().map(|row| row.hidden).collect::<Vec<_>>(), rows.iter().map(|row| row.fold).collect::<Vec<_>>(), workbench.host.notebooks.editors[0].focus)
        })
    };
    assert_eq!(rows(cx), (vec![false; 4], vec![Some((false, 3)), None, Some((false, 1)), None], 0));
    workbench.update(cx, |workbench, _| workbench.host.notebooks.editors[0].focus = 3);
    // A code cell folds nothing downwards.
    run(&workbench, "notebook.fold", cx);
    assert_eq!(rows(cx).0, [false; 4]);
    workbench.update(cx, |workbench, _| workbench.host.notebooks.editors[0].focus = 2);
    run(&workbench, "notebook.fold", cx);
    assert_eq!(rows(cx), (vec![false, false, false, true], vec![Some((false, 3)), None, Some((true, 1)), None], 2));
    workbench.update(cx, |workbench, _| workbench.host.notebooks.editors[0].focus = 1);
    run(&workbench, "notebook.fold", cx);
    assert_eq!(rows(cx).0, [false, false, false, true], "a code cell folds nothing");
    workbench.update(cx, |workbench, _| workbench.host.notebooks.editors[0].focus = 0);
    run(&workbench, "notebook.fold", cx);
    assert_eq!(rows(cx).0, [false, true, true, true]);
    run(&workbench, "notebook.unfold", cx);
    assert_eq!(rows(cx).0, [false, false, false, true], "the inner section stays collapsed");
}

/// cellCommands.ts, notebookClipboard.ts and cellEditorOptions.ts on the toolbar's cell.
#[gpui::test]
fn the_cell_toolbar_commands(cx: &mut TestAppContext) {
    let cells = json!([
        { "cellKind": 2, "language": "python", "source": "a\nb", "outputs": [], "metadata": {} },
        { "cellKind": 2, "language": "python", "source": "c", "outputs": [], "metadata": {} },
        { "cellKind": 1, "language": "markdown", "source": "# T", "outputs": [], "metadata": {} },
    ]);
    let (workbench, cx, _) = workbench_with_cells(cells, cx);
    run_on_cell(&workbench, "notebook.cell.copy", 1, cx);
    run_on_cell(&workbench, "notebook.cell.paste", 2, cx);
    assert_eq!(sources(&workbench, cx), ["a\nb", "c", "# T", "c"]);
    run_on_cell(&workbench, "notebook.cell.cut", 0, cx);
    assert_eq!(sources(&workbench, cx), ["c", "# T", "c"]);
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("a\nb"));
    run_on_cell(&workbench, "notebook.cell.joinBelow", 0, cx);
    assert_eq!(sources(&workbench, cx), ["c\n# T", "c"]);
    // From the toolbar the cell editor has no cursor: the split point is the start.
    run_on_cell(&workbench, "notebook.cell.split", 0, cx);
    assert_eq!(sources(&workbench, cx), ["", "c\n# T", "c"]);
    run_on_cell(&workbench, "notebook.cell.changeToMarkdown", 2, cx);
    run_on_cell(&workbench, "notebook.cell.moveUp", 2, cx);
    let kinds = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.host.notebooks.models[0].cells.iter().map(|cell| cell.kind).collect::<Vec<_>>());
    assert_eq!(kinds(cx), [crate::notebook::CODE, crate::notebook::MARKUP, crate::notebook::CODE]);
    workbench.update_in(cx, |workbench, window, cx| window.focus(&workbench.editor_focus, cx));
    run(&workbench, "undo", cx);
    assert_eq!(kinds(cx), [crate::notebook::CODE, crate::notebook::CODE, crate::notebook::MARKUP]);
    run_on_cell(&workbench, "notebook.cell.insertMarkdownCellAbove", 0, cx);
    assert_eq!(sources(&workbench, cx), ["", "", "c\n# T", "c"]);
    assert_eq!(kinds(cx)[0], crate::notebook::MARKUP);
    let line_numbers = |cx: &mut VisualTestContext| {
        workbench.read_with(cx, |workbench, _| {
            let model = &workbench.host.notebooks.models[0];
            model.cells.iter().map(|cell| workbench.cell_model(&cell.uri).is_some_and(|editor| workbench.cell_line_numbers(&editor.document().uri_string()))).collect::<Vec<_>>()
        })
    };
    run_on_cell(&workbench, "notebook.cell.toggleLineNumbers", 1, cx);
    assert_eq!(line_numbers(cx), [false, true, false, false]);
}
