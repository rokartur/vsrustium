//! cellOutput.ts `_attachToolbar` and cellOutputActions.ts: the `...` toolbar left of each output with the
//! `MenuId.NotebookOutputToolbar` actions and Change Presentation, the output commands, and which outputs show
//! (codeCellViewModel.ts `updateOutputHeight`).

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{Bounds, ClickEvent, Context, Div, Image, ImageFormat, MouseButton, Pixels, Window, canvas, prelude::*, px};
use serde_json::{Value, json};

use super::super::Workbench;
use super::super::hover::{HoverContent, HoverOptions};
use super::CellRow;
use crate::commands::Outcome;
use crate::file_dialog::FileDialog;
use crate::menus::{MenuAction, MenuId, menu_actions};
use crate::notebook::layout;
use crate::notebook::renderers::{self, Renderer};
use crate::notebook::{self, NotebookModel};
use crate::quick_input::{Entry, Item, Kind, QuickInput};

/// codeCellViewModel.ts: a visible output is at least this tall.
const MIN_OUTPUT_HEIGHT: f32 = 28.;
/// cellOutputTextHelper.ts `TEXT_BASED_MIMETYPES`.
const TEXT_BASED_MIMETYPES: [&str; 11] = [
    "text/latex",
    "text/html",
    "application/vnd.code.notebook.error",
    "application/vnd.code.notebook.stdout",
    "application/x.notebook.stdout",
    "application/x.notebook.stream",
    "application/vnd.code.notebook.stderr",
    "application/x.notebook.stderr",
    "text/plain",
    "text/markdown",
    "application/json",
];
/// cellOutput.ts `JUPYTER_RENDERER_MIMETYPES`: an unrendered one offers the Jupyter extension's renderers.
const JUPYTER_RENDERER_MIMETYPES: [&str; 18] = [
    "application/geo+json",
    "application/vdom.v1+json",
    "application/vnd.dataresource+json",
    "application/vnd.plotly.v1+json",
    "application/vnd.vega.v2+json",
    "application/vnd.vega.v3+json",
    "application/vnd.vega.v4+json",
    "application/vnd.vega.v5+json",
    "application/vnd.vegalite.v1+json",
    "application/vnd.vegalite.v2+json",
    "application/vnd.vegalite.v3+json",
    "application/vnd.vegalite.v4+json",
    "application/x-nteract-model-debug+json",
    "image/svg+xml",
    "text/latex",
    "text/vnd.plotly.v1+html",
    "application/vnd.jupyter.widget-view+json",
    "application/vnd.code.notebook.error",
];
const PICK_MIMETYPE: &str = "notebook.output.pickMimetype";
const COPY_OUTPUT: &str = "notebook.cellOutput.copy";

/// notebookCommon.ts `isTextStreamMime`.
fn is_text_stream_mime(mime: &str) -> bool {
    mime == "application/vnd.code.notebook.stdout" || mime == "application/vnd.code.notebook.stderr"
}

impl Workbench {
    /// codeCellViewModel.ts `updateOutputHeight`: the first output, one measured above zero and one Show Empty
    /// Outputs made visible show, at least 28px tall; the others are hidden (`None`).
    pub(in crate::workbench) fn output_heights(&self, tab: u32, cell: &notebook::Cell) -> Vec<Option<f32>> {
        let view = self.notebook_views.views.get(&tab);
        let measured = view.and_then(|view| view.webview.as_ref()).map(|layer| &layer.output_heights);
        let mut heights = Vec::with_capacity(cell.outputs.len());
        for (index, output) in cell.outputs.iter().enumerate() {
            let height = measured.and_then(|measured| measured.get(&output.id)).copied().unwrap_or(0.);
            let visible = index == 0 || height > 0. || view.is_some_and(|view| view.shown_empty.contains(&output.id));
            heights.push(visible.then_some(height.max(MIN_OUTPUT_HEIGHT)));
        }
        heights
    }

    /// `resolveMimeTypes`: the output's mimes with their renderers (notebook/renderers.rs `output_mime_types`), in
    /// the display order Change Presentation left, with the renderers it picked first.
    pub(in crate::workbench) fn output_presentations(&self, view_type: &str, output: &notebook::Output) -> Vec<(String, Option<Renderer>)> {
        let setting = self.setting("notebook.displayOrder");
        let display_order: Vec<String> = match &self.notebook_views.mime_order {
            Some((from, order)) if *from == setting => order.clone(),
            _ => setting.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect(),
        };
        let renderers = renderers::renderers(&self.extensions);
        let mimes: Vec<&str> = output.items.iter().map(|item| item.mime.as_str()).collect();
        let preferred = |mime: &str| self.notebook_views.preferred_renderers.get(&(view_type.to_owned(), mime.to_owned())).map(String::as_str);
        renderers::output_mime_types(&display_order, &mimes, &renderers, preferred).into_iter().map(|(mime, renderer)| (mime.to_owned(), renderer.cloned())).collect()
    }

    /// The `...` toolbars of a code cell's visible outputs, 32px left of each output and 4px below its top.
    pub(in crate::workbench) fn output_toolbars(&self, tab: u32, model: &NotebookModel, row: &CellRow, cx: &mut Context<Self>) -> Vec<gpui::Stateful<Div>> {
        let cell = &model.cells[row.index];
        let mut toolbars = Vec::new();
        let mut offset = row.layout.output_top;
        for (index, (output, height)) in cell.outputs.iter().zip(self.output_heights(tab, cell)).enumerate() {
            let Some(height) = height else { continue };
            let top = offset;
            offset += height;
            let presentations = self.output_presentations(&model.view_type, output);
            if presentations.is_empty() || (index > 0 && presentations.len() == 1 && !copy_enabled(cell, index, &presentations)) {
                continue;
            }
            let (handle, output_id) = (cell.handle, output.id.clone());
            let bounds = Rc::new(Cell::new(Bounds::default()));
            let measured = bounds.clone();
            let id = gpui::SharedString::from(format!("notebook-output-more-{}", output.id));
            let toolbar = self
                .icon("toolbar-more", 22., 16., self.color("icon.foreground"))
                .id(id)
                .absolute()
                .left(px(layout::CODE_CELL_LEFT_MARGIN + layout::CELL_RUN_GUTTER - 32.))
                .top(px(top + 7.))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(canvas(move |element_bounds, _, _| measured.set(element_bounds), |_, _, _, _| {}).absolute().size_full())
                .child(self.hover_target(gpui::SharedString::from(format!("notebook-output-more-hover-{output_id}")), HoverContent::Text("More Actions...".into()), HoverOptions::mouse()))
                .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| workbench.show_output_menu(tab, handle, &output_id, bounds.get(), window, cx)));
            toolbars.push(toolbar);
        }
        toolbars
    }

    /// The toolbar's dropdown: Change Presentation when the output has more than one presentation, then the
    /// `MenuId.NotebookOutputToolbar` actions for this output, Copy Cell Output only where copying is enabled.
    fn show_output_menu(&mut self, tab: u32, handle: u64, output_id: &str, anchor: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.host.notebooks.editor(tab) else { return };
        let Some(model) = self.host.notebooks.model(&editor.notebook) else { return };
        let Some(cell_index) = model.index_of_handle(handle) else { return };
        let cell = &model.cells[cell_index];
        let Some(index) = cell.outputs.iter().position(|output| output.id == output_id) else { return };
        let actions = self.output_menu_actions(tab, model, cell_index, index, window);
        let args = vec![json!({ "start": cell_index, "end": cell_index + 1, "outputId": output_id }), model.uri.clone()];
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, args, window, cx);
    }

    /// What the dropdown of output `index` of cell `cell_index` shows (`updateMenuToolbar`).
    pub(in crate::workbench) fn output_menu_actions(&self, tab: u32, model: &NotebookModel, cell_index: usize, index: usize, window: &Window) -> Vec<MenuAction> {
        let cell = &model.cells[cell_index];
        let presentations = self.output_presentations(&model.view_type, &cell.outputs[index]);
        let editable = !self.host.notebooks.interactives.iter().any(|interactive| interactive.notebook == model.uri_string());
        let hidden = self.output_heights(tab, cell).iter().any(Option::is_none);
        let mut context = self.key_context(window);
        context.insert("notebookCellHasOutputs".into(), json!(true));
        context.insert("notebookCellIsFirstOutput".into(), json!(index == 0));
        context.insert("notebookCellOutputMimetype".into(), json!(presentations.first().map(|(mime, _)| mime.clone())));
        context.insert("notebookCellHasHiddenOutputs".into(), json!(hidden));
        context.insert("notebookEditable".into(), json!(editable));
        context.insert("notebookCellEditable".into(), json!(editable));
        let copy = copy_enabled(cell, index, &presentations);
        let mut actions: Vec<MenuAction> = menu_actions(&self.menus, MenuId::NotebookOutputToolbar, &context)
            .into_iter()
            .filter(|action| copy || !matches!(action, MenuAction::Command { id, .. } if id == COPY_OUTPUT))
            .collect();
        if presentations.len() > 1 {
            actions.insert(0, MenuAction::Command { id: PICK_MIMETYPE.into(), title: "Change Presentation".into(), enabled: true, checked: false, alt: None });
        }
        actions
    }

    pub(in crate::workbench) fn register_notebook_output_commands(&mut self) {
        self.commands.register(COPY_OUTPUT, |workbench, args, _, cx| {
            let Some((key, cell_index, index)) = workbench.output_from_args(&args) else { return Outcome::done() };
            let model = workbench.host.notebooks.model(&key).expect("found above");
            let cell = &model.cells[cell_index];
            let mime = workbench.output_presentations(&model.view_type, &cell.outputs[index]).into_iter().next().map(|(mime, _)| mime);
            match mime.as_deref().and_then(image_format) {
                Some(format) => {
                    let item = cell.outputs[index].items.iter().find(|item| Some(&item.mime) == mime.as_ref()).expect("a presentation of the output");
                    cx.write_to_clipboard(gpui::ClipboardItem::new_image(&Image::from_bytes(format, item.data.clone())));
                }
                None => {
                    if let Some(text) = output_text(cell, index, mime.as_deref()) {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                    }
                }
            }
            Outcome::done()
        });
        self.commands.register("notebook.cellOutput.saveImage", |workbench, args, _, cx| {
            let Some((key, cell_index, index)) = workbench.output_from_args(&args) else { return Outcome::done() };
            let model = workbench.host.notebooks.model(&key).expect("found above");
            let output = &model.cells[cell_index].outputs[index];
            let Some(mime) = workbench.output_presentations(&model.view_type, output).into_iter().next().map(|(mime, _)| mime).filter(|mime| mime.starts_with("image/")) else { return Outcome::done() };
            let Some(data) = output.items.iter().find(|item| item.mime == mime).map(|item| item.data.clone()) else { return Outcome::done() };
            let extension = image_extension(&mime);
            let default_path = super::super::host_requests::uri_parts(&model.uri).ok().filter(|(scheme, _)| *scheme == "file").and_then(|(_, path)| path.parent().map(|folder| folder.join(format!("image.{extension}"))));
            let dialog = FileDialog { filters: vec![("Image Files".into(), vec![extension.into()])], default_path, ..FileDialog::default() };
            let picked = workbench.show_save_dialog(dialog, cx);
            cx.spawn(async move |_, _| {
                let Ok(Some(file)) = picked.await else { return };
                if let Err(error) = std::fs::write(&file, &data) {
                    eprintln!("Failed to save image output {error}");
                }
            })
            .detach();
            Outcome::done()
        });
        self.commands.register("notebook.cellOuput.showEmptyOutputs", |workbench, args, _, cx| {
            let Some((key, cell_index, _)) = workbench.output_from_args(&args) else { return Outcome::done() };
            let Some(tab) = workbench.host.notebooks.editors.iter().find(|editor| editor.notebook == key).map(|editor| editor.tab_model_id) else { return Outcome::done() };
            let cell = &workbench.host.notebooks.model(&key).expect("found above").cells[cell_index];
            let hidden: Vec<String> = cell.outputs.iter().zip(workbench.output_heights(tab, cell)).skip(1).filter(|(_, height)| height.is_none()).map(|(output, _)| output.id.clone()).collect();
            workbench.notebook_views.views.entry(tab).or_default().shown_empty.extend(hidden);
            cx.notify();
            Outcome::done()
        });
        self.commands.register(PICK_MIMETYPE, |workbench, args, window, cx| {
            if let Some((key, cell_index, index)) = workbench.output_from_args(&args) {
                workbench.pick_output_presentation(&key, cell_index, index, window, cx);
            }
            Outcome::done()
        });
    }

    /// The notebook, cell index and output index an output action runs on: `{ start, outputId }` and the notebook
    /// URI from the toolbar, else the active cell's first output (`getOutputViewModelFromContext`).
    fn output_from_args(&self, args: &[Value]) -> Option<(String, usize, usize)> {
        let (key, cells) = self.cells_from_args(args)?;
        let cell_index = *cells.first()?;
        let cell = &self.host.notebooks.model(&key)?.cells[cell_index];
        let index = match args.first().and_then(|context| context["outputId"].as_str()) {
            Some(id) => cell.outputs.iter().position(|output| output.id == id)?,
            None if cell.outputs.is_empty() => return None,
            None => 0,
        };
        Some((key, cell_index, index))
    }

    /// `_pickActiveMimeTypeRenderer`: the output's presentations, the active one first selected; a pick becomes the
    /// preferred renderer of its mime and moves the mime ahead of the output's others in the display order.
    fn pick_output_presentation(&mut self, key: &str, cell_index: usize, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.host.notebooks.model(key).expect("found by the caller");
        let view_type = model.view_type.clone();
        let output = &model.cells[cell_index].outputs[index];
        let presentations = self.output_presentations(&view_type, output);
        let active = presentations.iter().position(|(_, renderer)| renderer.is_some()).unwrap_or(0);
        let mut items = Vec::new();
        let mut unsupported = Vec::new();
        for (position, (mime, renderer)) in presentations.iter().enumerate() {
            let detail = match renderer {
                Some(renderer) => format!("{} ({})", if renderer.display_name.is_empty() { &renderer.id } else { &renderer.display_name }, renderer.extension_id),
                None => "renderer not available".into(),
            };
            let item = Item { handle: position as i64, label: mime.clone(), detail: Some(detail), picked: position == active, description: (position == active).then(|| "Currently Active".into()), pickable: true, ..Default::default() };
            match renderer {
                Some(_) => items.push(Entry::Item(item)),
                None => unsupported.push(Entry::Item(item)),
            }
        }
        let install = presentations.len() as i64;
        if presentations.iter().any(|(mime, renderer)| renderer.is_none() && JUPYTER_RENDERER_MIMETYPES.contains(&mime.as_str())) {
            unsupported.push(Entry::Item(Item { handle: install, label: "Install additional renderers from the marketplace".into(), pickable: true, ..Default::default() }));
        }
        items.push(Entry::Separator(None));
        items.extend(unsupported);
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select mimetype to render for current output".into());
        input.set_items(items);
        input.set_active_items(&[active as i64]);
        let picked = self.choose(input, window, cx);
        let mimes: Vec<String> = presentations.iter().map(|(mime, _)| mime.clone()).collect();
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(handle)) = picked.await else { return };
            let Some(handle) = handle.as_i64() else { return };
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                if handle == install {
                    drop(workbench.execute_command("workbench.extensions.search", vec![json!("@id:ms-toolsai.jupyter")], window, cx));
                    return;
                }
                if handle as usize == active {
                    return;
                }
                let (mime, renderer) = &presentations[handle as usize];
                if let Some(renderer) = renderer {
                    workbench.notebook_views.preferred_renderers.insert((view_type.clone(), mime.clone()), renderer.id.clone());
                }
                let setting = workbench.setting("notebook.displayOrder");
                let mut order = match workbench.notebook_views.mime_order.take() {
                    Some((from, order)) if from == setting => order,
                    _ => setting.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect(),
                };
                let others: Vec<&str> = mimes.iter().map(String::as_str).collect();
                renderers::prioritize_mime(&mut order, mime, &others);
                workbench.notebook_views.mime_order = Some((setting, order));
                cx.notify();
            });
        })
        .detach();
    }
}

/// cellOutput.ts `shouldEnableCopy`. Upstream tests `TEXT_BASED_MIMETYPES.indexOf(mime)` for truth, so every mime
/// but `text/latex` (index 0) counts as text; kept as is. A stream output after a stream output copies from the first.
fn copy_enabled(cell: &notebook::Cell, index: usize, presentations: &[(String, Option<Renderer>)]) -> bool {
    if !presentations.iter().any(|(mime, _)| mime != TEXT_BASED_MIMETYPES[0] || mime.starts_with("image/")) {
        return false;
    }
    if index > 0 && is_text_stream_mime(&presentations[0].0) {
        let previous = &cell.outputs[index - 1];
        return !previous.items.first().is_some_and(|item| is_text_stream_mime(&item.mime));
    }
    true
}

/// cellOutputTextHelper.ts `copyCellOutput`: the picked mime's item when it is text based, else the first text
/// based item; a stream joins the stream outputs from this one on (`getOutputStreamText`).
fn output_text(cell: &notebook::Cell, index: usize, mime: Option<&str>) -> Option<String> {
    let output = &cell.outputs[index];
    let item = match mime.filter(|mime| TEXT_BASED_MIMETYPES.contains(mime)) {
        Some(mime) => output.items.iter().find(|item| item.mime == mime),
        None => output.items.iter().find(|item| TEXT_BASED_MIMETYPES.contains(&item.mime.as_str())),
    }?;
    if is_text_stream_mime(&item.mime) {
        let mut text = String::new();
        for output in &cell.outputs[index..] {
            let Some(stream) = output.items.iter().find(|item| is_text_stream_mime(&item.mime)) else { break };
            text.push_str(&String::from_utf8_lossy(&stream.data));
        }
        return Some(text.trim().to_owned());
    }
    Some(output_item_text(&item.mime, &item.data))
}

/// `getOutputText`: the first 100000 bytes, an error as its stack (or name and message) without ANSI colors.
fn output_item_text(mime: &str, data: &[u8]) -> String {
    const CHAR_LIMIT: usize = 100_000;
    let mut text = String::from_utf8_lossy(&data[..data.len().min(CHAR_LIMIT)]).into_owned();
    if data.len() > CHAR_LIMIT {
        text.push_str("...(truncated)");
    } else if mime == "application/vnd.code.notebook.error" {
        static ANSI: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)\\u001b\[[0-9;]*m").expect("valid pattern"));
        text = ANSI.replace_all(&text, "").into_owned();
        if let Ok(error) = serde_json::from_str::<HashMap<String, Value>>(&text) {
            text = match error.get("stack").and_then(Value::as_str).filter(|stack| !stack.is_empty()) {
                Some(stack) => stack.to_owned(),
                None => format!("{}: {}", error.get("name").and_then(Value::as_str).unwrap_or("undefined"), error.get("message").and_then(Value::as_str).unwrap_or("undefined")),
            };
        }
    }
    text.trim().to_owned()
}

/// The clipboard image format of an image mime (webviewPreloads.ts copies the rendered `<img>` as PNG).
fn image_format(mime: &str) -> Option<ImageFormat> {
    match mime {
        "image/png" => Some(ImageFormat::Png),
        "image/jpeg" | "image/jpg" => Some(ImageFormat::Jpeg),
        "image/gif" => Some(ImageFormat::Gif),
        "image/svg+xml" => Some(ImageFormat::Svg),
        "image/webp" => Some(ImageFormat::Webp),
        "image/bmp" => Some(ImageFormat::Bmp),
        "image/tiff" => Some(ImageFormat::Tiff),
        _ => None,
    }
}

/// SaveCellOutputImageAction's `mimeToExt`, `png` for the rest.
fn image_extension(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        _ => "png",
    }
}
