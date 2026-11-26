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
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("vsrustium-notebook-editor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None);
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), Arc::new(Mutex::new(settings)), HostSync::new(std::sync::mpsc::channel().0), cx));
    let uri = json!({ "scheme": "untitled", "authority": "", "path": "Untitled-1.ipynb", "query": "", "fragment": "" });
    let data = json!({ "cells": [{ "cellKind": 2, "language": "python", "source": "x", "outputs": [], "metadata": {} }], "metadata": {} });
    workbench.update_in(cx, |workbench, window, cx| {
        let key = crate::notebook::uri_string(&uri);
        // As `open_notebook` holds a model until its editor shows it.
        workbench.host.notebooks.held.push(key.clone());
        workbench.add_notebook(uri.clone(), "jupyter-notebook", &data, false, cx);
        workbench.show_notebook(&key, &json!({}), window, cx).unwrap();
        workbench.host.notebooks.held.clear();
        window.focus(&workbench.editor_focus, cx);
    });
    cx.run_until_parked();
    (workbench, cx)
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
fn the_focused_cell_is_the_code_editor_the_workbench_edits(cx: &mut TestAppContext) {
    let (workbench, cx) = notebook_workbench(cx);
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.editors.active().unwrap().document().scheme, crate::notebook::CELL_SCHEME));
    run(&workbench, "notebook.cell.insertCodeCellBelow", cx);
    run(&workbench, "type", cx);
    assert_eq!(sources(&workbench, cx), ["x", "1"]);
}
