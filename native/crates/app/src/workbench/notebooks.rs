//! The notebook main thread: mainThreadNotebook.ts (serializers), mainThreadNotebookDocuments.ts with
//! notebookEditorModelResolverServiceImpl.ts (open, create, save), mainThreadNotebookDocumentsAndEditors.ts (the
//! documents and editors delta), mainThreadNotebookEditors.ts, mainThreadNotebookKernels.ts with
//! notebookExecutionServiceImpl.ts, bulkCellEdits.ts, and the notebook commands the extension API reaches.

use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use exthost::main_thread::serialized_error;
use exthost::main_thread_files::resource;
use exthost::rpc::{VSBUFFER_KEY, WITH_BUFFERS_KEY};
use futures::channel::oneshot;
use gpui::{AsyncApp, Context, WeakEntity, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::host_requests::uri_parts;
use super::notebook_editor::CellReveal;
use crate::backups;
use crate::commands::Outcome;
use crate::editor::{OpenEditor, TextDocument};
use crate::host_sync::FileJob;
use crate::notebook::kernels::{ExecutionState, association_args};
use crate::notebook::{self, Applied, CELL_SCHEME, NOTEBOOK_EDITOR_SCHEME, NewCellText, NotebookEditor, NotebookModel, Serializer};

type NotebookResult = Result<Value, Value>;

/// `CellExecutionUpdateType`.
const UPDATE_OUTPUT: u64 = 1;
const UPDATE_OUTPUT_ITEMS: u64 = 2;
const UPDATE_EXECUTION_STATE: u64 = 3;
/// mainThreadDocuments.ts `BoundModelReferenceCollection` `_maxAge`.
const HOST_REFERENCE_AGE: std::time::Duration = std::time::Duration::from_secs(3 * 60);

impl Workbench {
    pub(crate) fn handle_notebook_request(&mut self, proxy: &str, method: &str, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let null = Value::Null;
        let arg = |index: usize| args.get(index).unwrap_or(&null);
        let handle = arg(0).as_u64().unwrap_or_default();
        match (proxy, method) {
            ("MainThreadNotebook", "$registerNotebookSerializer") => {
                self.register_notebook_serializer(handle, arg(2).as_str().unwrap_or_default(), arg(3).clone(), arg(4).clone());
                Outcome::done()
            }
            ("MainThreadNotebook", "$registerNotebookCellStatusBarItemProvider") => {
                self.register_cell_status_bar_provider(handle, arg(1).as_u64(), arg(2).as_str().unwrap_or_default().to_owned(), cx);
                Outcome::done()
            }
            ("MainThreadNotebook", "$unregisterNotebookCellStatusBarItemProvider") => {
                self.unregister_cell_status_bar_provider(handle, cx);
                Outcome::done()
            }
            ("MainThreadNotebook", "$emitCellStatusBarEvent") => {
                self.cell_status_bar_event(handle, cx);
                Outcome::done()
            }
            ("MainThreadNotebook", "$unregisterNotebookSerializer") => {
                self.host.notebooks.serializers.retain(|serializer| serializer.handle != handle);
                Outcome::done()
            }
            ("MainThreadNotebook", _) => Outcome::done(),
            ("MainThreadNotebookDocuments", "$tryCreateNotebook") => self.try_create_notebook(arg(0).clone(), cx),
            ("MainThreadNotebookDocuments", "$tryOpenNotebook") => self.try_open_notebook(arg(0).clone(), cx),
            ("MainThreadNotebookDocuments", "$trySaveNotebook") => {
                let saved = self.save_notebook(&notebook::uri_string(arg(0)), cx);
                let (sender, receiver) = oneshot::channel();
                cx.background_executor()
                    .spawn(async move {
                        let _ = sender.send(Ok(json!(saved.await.unwrap_or(false))));
                    })
                    .detach();
                Outcome::Pending(receiver)
            }
            ("MainThreadNotebookEditors", "$tryShowNotebookDocument") => self.try_show_notebook(arg(0).clone(), arg(2).clone(), window, cx),
            ("MainThreadNotebookEditors", "$tryRevealRange") => {
                let Some(tab) = self.host.notebooks.editors.iter().find(|editor| Some(editor.id.as_str()) == arg(0).as_str()).map(|editor| editor.tab_model_id) else { return Outcome::done() };
                let reveal = match arg(2).as_u64() {
                    Some(1) => CellReveal::Center,
                    Some(2) => CellReveal::CenterIfOutside,
                    Some(3) => CellReveal::Top,
                    _ => CellReveal::InView,
                };
                let (start, end) = (arg(1)["start"].as_u64().unwrap_or_default() as usize, arg(1)["end"].as_u64().unwrap_or_default() as usize);
                self.reveal_notebook_cells(tab, start, end, reveal);
                cx.notify();
                Outcome::done()
            }
            ("MainThreadNotebookEditors", "$trySetSelections") => {
                let selections = cell_ranges(arg(1));
                if let Some(editor) = self.host.notebooks.editors.iter_mut().find(|editor| Some(editor.id.as_str()) == arg(0).as_str()) {
                    editor.focus = selections.first().map_or(0, |range| range.0);
                    editor.selections = selections;
                }
                self.editors_changed(cx);
                Outcome::done()
            }
            ("MainThreadNotebookKernels", method) => self.handle_kernel_request(method, args, cx),
            ("MainThreadNotebookRenderers", "$postMessage") => {
                let editor_id = arg(0).as_str().map(str::to_owned);
                let message = json!({ "__vscode_notebook_message": true, "type": "customRendererMessage", "rendererId": arg(1), "message": arg(2) });
                Outcome::Ready(Ok(json!(self.post_to_notebook_webviews(message, |editor| editor_id.as_ref().is_none_or(|id| *id == editor.id)))))
            }
            _ => Outcome::done(),
        }
    }

    /// notebookVariableCommands.ts `_executeNotebookVariableProvider`: the named variables of the notebook's bound
    /// kernel, when it has a variable provider.
    fn execute_notebook_variable_provider(&mut self, resource: Option<&Value>, cx: &mut Context<Self>) -> Outcome {
        let Some(model) = resource.and_then(|uri| self.host.notebooks.model(&notebook::uri_string(uri))) else { return Outcome::Ready(Ok(json!([]))) };
        let (uri, key, view_type) = (model.uri.clone(), model.uri_string(), model.view_type.clone());
        let kernels = &mut self.host.notebooks.kernels;
        let Some(handle) = kernels.matching(&view_type, &key).1.filter(|kernel| kernel.has_variable_provider).map(|kernel| kernel.handle) else { return Outcome::Ready(Ok(json!([]))) };
        let request_id = format!("{handle}variables{}", kernels.variable_request_index);
        kernels.variable_request_index += 1;
        kernels.variable_requests.insert(request_id.clone(), Vec::new());
        let call = self.host.call_cancellable("ExtHostNotebookKernels", "$provideVariables", vec![json!(handle), json!(request_id), uri, Value::Null, json!("named"), json!(0)]);
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let answer = call.await;
            let collected = workbench.update(cx, |workbench, _| workbench.host.notebooks.kernels.variable_requests.remove(&request_id).unwrap_or_default()).unwrap_or_default();
            let _ = sender.send(match answer {
                Ok(Err(error)) => Err(error),
                _ => Ok(json!(collected)),
            });
        })
        .detach();
        Outcome::Pending(receiver)
    }

    pub(super) fn register_notebook_commands(&mut self) {
        self.commands.register("notebook.cell.changeLanguage", |workbench, args, window, cx| {
            workbench.change_cell_language(&args, window, cx);
            Outcome::done()
        });
        self.commands.register("_executeNotebookVariableProvider", |workbench, args, _, cx| workbench.execute_notebook_variable_provider(args.first(), cx));
        self.commands.register("_resolveNotebookContentProvider", |workbench, _, _, _| Outcome::Ready(Ok(json!(workbench.notebook_types().iter().map(NotebookType::info).collect::<Vec<_>>()))));
        self.commands.register("_executeDataToNotebook", |_, args, _, cx| {
            let view_type = args.first().and_then(Value::as_str).unwrap_or_default().to_owned();
            let bytes = buffer_bytes(args.get(1).unwrap_or(&Value::Null));
            let (sender, receiver) = oneshot::channel();
            cx.spawn(async move |workbench, cx| {
                let data = data_to_notebook(&workbench, cx, &view_type, &bytes).await;
                let _ = sender.send(data.map(|data| json!({ WITH_BUFFERS_KEY: buffers_of(data) })));
            })
            .detach();
            Outcome::Pending(receiver)
        });
        self.commands.register("_executeNotebookToData", |_, args, _, cx| {
            let view_type = args.first().and_then(Value::as_str).unwrap_or_default().to_owned();
            // The `SerializableObjectWithBuffers` argument arrives as its `value`.
            let data = args.get(1).map(|data| data.get("value").unwrap_or(data).clone()).unwrap_or_default();
            let data = buffers_of(data);
            let (sender, receiver) = oneshot::channel();
            cx.spawn(async move |workbench, cx| {
                let bytes = notebook_to_data(&workbench, cx, &view_type, data).await;
                let _ = sender.send(bytes.map(|bytes| json!({ VSBUFFER_KEY: STANDARD.encode(bytes) })));
            })
            .detach();
            Outcome::Pending(receiver)
        });
        self.commands.register("_notebook.selectKernel", |workbench, args, _, cx| Outcome::Ready(Ok(json!(workbench.select_kernel(args.first().unwrap_or(&Value::Null), cx)))));
        self.commands.register("notebook.execute", |workbench, args, _, cx| {
            let Some(key) = workbench.notebook_from_args(args.first()) else { return Outcome::done() };
            let model = workbench.host.notebooks.model(&key).expect("a shown notebook");
            let handles = model.cells.iter().filter(|cell| cell.kind == notebook::CODE).map(|cell| cell.handle).collect();
            workbench.execute_notebook_cells(&key, handles, cx)
        });
        self.commands.register("notebook.cell.execute", |workbench, args, _, cx| {
            let Some((key, indices)) = workbench.cells_from_args(&args) else { return Outcome::done() };
            let model = workbench.host.notebooks.model(&key).expect("a shown notebook");
            let handles = indices.into_iter().filter_map(|index| model.cells.get(index)).filter(|cell| cell.kind == notebook::CODE).map(|cell| cell.handle).collect();
            workbench.execute_notebook_cells(&key, handles, cx)
        });
        self.commands.register("notebook.cell.clearOutputs", |workbench, args, _, cx| {
            let Some((key, indices)) = workbench.cells_from_args(&args) else { return Outcome::done() };
            let Some(&index) = indices.first() else { return Outcome::done() };
            let handle = workbench.host.notebooks.model(&key).expect("a shown notebook").cells[index].handle;
            let mut edits = vec![json!({ "editType": 2, "index": index, "outputs": [] })];
            if workbench.host.notebooks.kernels.state(&key, handle) != Some(ExecutionState::Executing) {
                let cleared = json!({ "runStartTime": null, "runStartTimeAdjustment": null, "runEndTime": null, "executionOrder": null, "lastRunSuccess": null });
                edits.push(json!({ "editType": 9, "index": index, "internalMetadata": cleared }));
            }
            Outcome::Ready(workbench.apply_notebook_edits(&key, &edits, cx).map(|_| Value::Null))
        });
        for (id, kind) in [("notebook.cell.insertCodeCellBelow", notebook::CODE), ("notebook.cell.insertCodeCellBelowAndFocusContainer", notebook::CODE), ("notebook.cell.insertMarkdownCellBelow", 1)] {
            self.commands.register(id, move |workbench, _, window, cx| workbench.insert_cell_below(kind, window, cx));
        }
        self.commands.register("notebook.selectKernel", |workbench, _, window, cx| workbench.show_kernel_picker(window, cx));
        self.register_notebook_list_commands();
        self.commands.register("notebook.cell.copyDown", |workbench, args, _, cx| {
            let Some((key, indices)) = workbench.cells_from_args(&args) else { return Outcome::done() };
            let Some(&index) = indices.first() else { return Outcome::done() };
            let model = workbench.host.notebooks.model(&key).expect("a shown notebook");
            let cell = &model.cells[index];
            let source = workbench.cell_text(&cell.uri);
            let data = json!({
                "cellKind": cell.kind,
                "language": cell.language,
                "mime": cell.mime,
                "source": source,
                "outputs": cell.outputs.iter().map(notebook::Output::dto).collect::<Vec<_>>(),
                "metadata": cell.metadata,
                "internalMetadata": cell.internal_metadata,
            });
            let result = workbench.apply_notebook_edits(&key, &[json!({ "editType": 1, "index": index + 1, "count": 0, "cells": [data] })], cx);
            if let Some(editor) = workbench.active_notebook_editor_mut() {
                editor.focus = index + 1;
                editor.selections = vec![(index + 1, index + 2)];
            }
            workbench.editors_changed(cx);
            Outcome::Ready(result.map(|_| Value::Null))
        });
    }

    fn register_notebook_serializer(&mut self, handle: u64, view_type: &str, options: Value, registration: Value) {
        self.host.notebooks.serializers.push(Serializer { handle, view_type: view_type.to_owned(), options, registration });
    }

    /// `$tryCreateNotebook`: an untitled notebook, with the content given (dirty) or what its serializer makes of
    /// nothing.
    fn try_create_notebook(&mut self, options: Value, cx: &mut Context<Self>) -> Outcome {
        let view_type = options["viewType"].as_str().unwrap_or_default().to_owned();
        let Some(ending) = self.notebook_types().iter().find(|kind| kind.view_type == view_type).map(NotebookType::file_ending) else {
            return Outcome::error(format!("UNKNOWN notebook type: {view_type}"));
        };
        let uri = (1..).map(|counter| notebook::untitled_uri(counter, &ending, &view_type)).find(|uri| self.host.notebooks.model(&notebook::uri_string(uri)).is_none()).expect("a free name");
        let content = options.get("content").filter(|content| !content.is_null()).cloned();
        // The untitled model stays until it is disposed: a save as lets it go (`replace_notebook_editor`).
        self.host.notebooks.held.push(notebook::uri_string(&uri));
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let created = resolve_notebook(&workbench, cx, uri.clone(), view_type, content).await;
            let _ = sender.send(created.map(|_| uri));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `$tryOpenNotebook`: the model of the resource, read and deserialized when there is none; the host holds it.
    fn try_open_notebook(&mut self, uri: Value, cx: &mut Context<Self>) -> Outcome {
        let view_type = match self.notebook_view_type(&uri) {
            Ok(view_type) => view_type,
            Err(error) => return Outcome::Ready(Err(error)),
        };
        let key = notebook::uri_string(&uri);
        // `BoundModelReferenceCollection.add`: the host's reference goes after three minutes.
        self.host.notebooks.held.push(key.clone());
        let expiring = key.clone();
        cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(HOST_REFERENCE_AGE).await;
            let _ = workbench.update(cx, |workbench, cx| {
                let held = &mut workbench.host.notebooks.held;
                if let Some(index) = held.iter().position(|held| *held == expiring) {
                    held.remove(index);
                }
                workbench.editors_changed(cx);
            });
        })
        .detach();
        if self.host.notebooks.model(&key).is_some() {
            return Outcome::Ready(Ok(uri));
        }
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let result = resolve_notebook(&workbench, cx, uri.clone(), view_type, None).await;
            if result.is_err() {
                let _ = workbench.update(cx, |workbench, _| workbench.host.notebooks.held.retain(|held| *held != key));
            }
            let _ = sender.send(result.map(|_| uri));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `validateResourceViewType`: the open notebook's type, else the contributed type of the resource
    /// (exclusive, then default, then any).
    fn notebook_view_type(&self, uri: &Value) -> Result<String, Value> {
        if uri["scheme"] == CELL_SCHEME {
            return Err(serialized_error(&format!("CANNOT open a cell-uri as notebook. Tried with {}", notebook::uri_string(uri))));
        }
        if let Some(model) = self.host.notebooks.model(&notebook::uri_string(uri)) {
            return Ok(model.view_type.clone());
        }
        let (scheme, path) = uri_parts(uri)?;
        let types = self.notebook_types();
        let matching: Vec<&NotebookType> = types.iter().filter(|kind| kind.matches(scheme, &path)).collect();
        let pick = matching.iter().find(|kind| kind.priority == "exclusive").or(matching.iter().find(|kind| kind.priority == "default")).or(matching.first());
        match pick {
            Some(kind) => Ok(kind.view_type.clone()),
            None => Err(serialized_error(&format!("Missing viewType for '{}'", notebook::uri_string(uri)))),
        }
    }

    /// The notebook types of `contributes.notebooks` and of serializers registered with their own data.
    fn notebook_types(&self) -> Vec<NotebookType> {
        let mut types = Vec::new();
        for extension in &self.extensions {
            for contribution in extension.description["contributes"]["notebooks"].as_array().into_iter().flatten() {
                let Some(view_type) = contribution["type"].as_str() else { continue };
                let selectors = contribution["selector"].as_array().cloned().unwrap_or_default();
                types.push(NotebookType {
                    view_type: view_type.to_owned(),
                    display_name: contribution["displayName"].as_str().unwrap_or_default().to_owned(),
                    priority: contribution["priority"].as_str().unwrap_or("default").to_owned(),
                    selectors: selectors.iter().map(|selector| (selector["filenamePattern"].clone(), selector["excludeFileNamePattern"].clone())).collect(),
                    options: Value::Null,
                });
            }
        }
        for serializer in &self.host.notebooks.serializers {
            let registration = &serializer.registration;
            match types.iter_mut().find(|kind| kind.view_type == serializer.view_type) {
                Some(kind) => kind.options = serializer.options.clone(),
                None if registration.is_object() => types.push(NotebookType {
                    view_type: serializer.view_type.clone(),
                    display_name: registration["displayName"].as_str().unwrap_or_default().to_owned(),
                    priority: if registration["priority"] == "exclusive" { "exclusive".into() } else { "default".into() },
                    selectors: registration["filenamePattern"].as_array().into_iter().flatten().map(|pattern| (pattern.clone(), Value::Null)).collect(),
                    options: serializer.options.clone(),
                }),
                None => {}
            }
        }
        types
    }

    /// A new notebook model: its cells' text models, then the host hears of the notebook and its cell documents.
    pub(super) fn add_notebook(&mut self, uri: Value, view_type: &str, data: &Value, dirty: bool, cx: &mut Context<Self>) {
        let key = notebook::uri_string(&uri);
        if self.host.notebooks.model(&key).is_some() {
            return;
        }
        let transient = self.host.notebooks.serializer(view_type).map(|serializer| serializer.options.clone()).unwrap_or_default();
        let (mut model, texts) = NotebookModel::new(uri.clone(), view_type, data, transient);
        model.dirty = dirty;
        for text in &texts {
            self.create_cell_model(text);
        }
        self.host.notebooks.models.push(model);
        self.editors_changed(cx);
        if dirty {
            self.host.send("ExtHostNotebookDocuments", "$acceptDirtyStateChanged", vec![uri, json!(true)]);
        }
        // `_postDocumentOpenActivation`
        drop(self.host.activate_by_event(&format!("onNotebook:{view_type}")));
        drop(self.host.activate_by_event("onNotebook:*"));
    }

    fn create_cell_model(&mut self, text: &NewCellText) {
        let (scheme, path) = uri_parts(&text.uri).expect("a cell URI");
        let document = TextDocument::from_text(scheme, &path, &text.source, "utf8".into(), Some(&text.language), &self.languages, &self.settings.lock().unwrap());
        self.editors.open_model(document);
    }

    pub(super) fn cell_text(&self, cell_uri: &Value) -> String {
        let (scheme, path) = uri_parts(cell_uri).expect("a cell URI");
        self.editors.model(scheme, &path).map(|model| model.document().lines.join(model.document().eol)).unwrap_or_default()
    }

    /// `applyEdits` on an open notebook; the host hears of the change, the new cells' documents and the dirty state.
    pub(super) fn apply_notebook_edits(&mut self, key: &str, edits: &[Value], cx: &mut Context<Self>) -> Result<Applied, Value> {
        let model = self.host.notebooks.model_mut(key).ok_or_else(|| serialized_error(&format!("Notebook '{key}' is not open")))?;
        let was_dirty = model.dirty;
        // `onWillAddRemoveCells`: executions of cells about to go are cancelled.
        let removed_handles: Vec<u64> = edits
            .iter()
            .filter(|edit| edit["editType"] == 1)
            .flat_map(|edit| {
                let start = edit["index"].as_u64().unwrap_or_default() as usize;
                let count = edit["count"].as_u64().unwrap_or_default() as usize;
                model.cells.iter().skip(start).take(count).map(|cell| cell.handle).collect::<Vec<_>>()
            })
            .collect();
        let mut applied = model.apply_edits(edits).map_err(|error| serialized_error(&error))?;
        let (uri, dirty, metadata, view_type) = (model.uri.clone(), model.dirty, model.metadata.clone(), model.view_type.clone());
        self.cancel_removed_executions(key, &view_type, &uri, &removed_handles);
        for text in &applied.added {
            self.create_cell_model(text);
        }
        // A replace's inverse brings its cells back with their text, read before their text models go.
        for undo in &mut applied.undo {
            if let Some(restore) = undo.as_object_mut().and_then(|undo| undo.remove("restore")) {
                let cells: Vec<Value> = restore.as_array().into_iter().flatten().filter_map(|uri| applied.removed.iter().find(|cell| cell.uri == *uri)).map(|cell| self.cell_data(cell)).collect();
                undo["cells"] = json!(cells);
            }
        }
        let removed: Vec<(String, std::path::PathBuf)> = applied.removed.iter().filter_map(|cell| uri_parts(&cell.uri).ok()).map(|(scheme, path)| (scheme.to_owned(), path)).collect();
        self.editors.models.retain(|model| !removed.iter().any(|(scheme, path)| model.document().is(scheme, path)));
        if let Some(event) = &applied.event {
            let metadata = if applied.document_metadata_changed { metadata } else { Value::Null };
            self.host.send("ExtHostNotebookDocuments", "$acceptModelChanged", vec![uri.clone(), json!({ WITH_BUFFERS_KEY: event }), json!(dirty), metadata]);
        }
        if dirty != was_dirty {
            self.host.send("ExtHostNotebookDocuments", "$acceptDirtyStateChanged", vec![uri, json!(dirty)]);
        }
        self.editors_changed(cx);
        if applied.event.is_some() {
            self.update_notebook_cell_status_bars(key, cx);
        }
        Ok(applied)
    }

    /// bulkCellEdits.ts: the cell edits of a workspace edit, notebook by notebook.
    pub(crate) fn apply_workspace_cell_edits(&mut self, edits: &[Value], cx: &mut Context<Self>) -> NotebookResult {
        for group in edits.chunk_by(|a, b| a["resource"] == b["resource"]) {
            let key = notebook::uri_string(&group[0]["resource"]);
            let model = self.host.notebooks.model(&key).ok_or_else(|| serialized_error(&format!("Notebook '{key}' is not open")))?;
            if let Some(version) = group[0]["notebookVersionId"].as_u64().filter(|version| *version != model.version_id) {
                return Err(serialized_error(&format!("Notebook '{key}' has changed in the meantime (version {version}, now {})", model.version_id)));
            }
            let cell_edits: Vec<Value> = group.iter().map(|edit| edit["cellEdit"].clone()).collect();
            // `computeUndo`: a workspace edit's cell edits are one step of the notebook's undo stack.
            let applied = self.apply_notebook_edits(&key, &cell_edits, cx)?;
            if !applied.undo.is_empty() {
                self.push_cells_undo(&key, applied.undo, cell_edits);
            }
        }
        Ok(Value::Null)
    }

    /// Saves a notebook: the serializer turns its snapshot into bytes, which are written to its resource.
    pub(super) fn save_notebook(&mut self, key: &str, cx: &mut Context<Self>) -> oneshot::Receiver<bool> {
        let (sender, receiver) = oneshot::channel();
        let Some(model) = self.host.notebooks.model(key).filter(|model| model.uri["scheme"] != "untitled") else {
            let _ = sender.send(false);
            return receiver;
        };
        let snapshot = model.snapshot(|cell| self.cell_text(&cell.uri));
        let (uri, view_type, version) = (model.uri.clone(), model.view_type.clone(), model.version_id);
        let key = key.to_owned();
        cx.spawn(async move |workbench, cx| {
            let bytes = notebook_to_data(&workbench, cx, &view_type, snapshot).await;
            let written = match bytes {
                Ok(bytes) => match workbench.update(cx, |workbench, _| workbench.host.run_files(FileJob::Write(resource(&uri), bytes))) {
                    Ok(write) => matches!(write.await, Ok(Ok(_))),
                    Err(_) => false,
                },
                Err(_) => false,
            };
            let _ = workbench.update(cx, |workbench, cx| {
                let Some(model) = workbench.host.notebooks.model_mut(&key).filter(|_| written) else { return };
                if model.version_id == version && model.dirty {
                    model.dirty = false;
                    workbench.host.send("ExtHostNotebookDocuments", "$acceptDirtyStateChanged", vec![uri.clone(), json!(false)]);
                }
                workbench.host.send("ExtHostNotebookDocuments", "$acceptModelSaved", vec![uri]);
                workbench.editors_changed(cx);
            });
            let _ = sender.send(written);
        })
        .detach();
        receiver
    }

    /// A notebook editor's save (`NotebookEditorInput.save`): an untitled notebook saves as a file picked in the save
    /// dialog.
    pub(super) fn save_notebook_editor(&mut self, key: &str, cx: &mut Context<Self>) -> oneshot::Receiver<bool> {
        match self.host.notebooks.model(key).is_some_and(|model| model.uri["scheme"] == "untitled") {
            true => self.save_notebook_as(key, cx),
            false => self.save_notebook(key, cx),
        }
    }

    /// `NotebookEditorInput.saveAs`: the save dialog at the workspace's folder with the notebook's name, the
    /// serialized notebook written there, the target notebook opened in place of the source editor; an untitled
    /// source is disposed (notebookEditorModel.ts `saveAs` reverts it).
    pub(super) fn save_notebook_as(&mut self, key: &str, cx: &mut Context<Self>) -> oneshot::Receiver<bool> {
        let (sender, receiver) = oneshot::channel();
        let Some(model) = self.host.notebooks.model(key) else {
            let _ = sender.send(false);
            return receiver;
        };
        let name = uri_parts(&model.uri).ok().and_then(|(_, path)| path.file_name().map(|name| name.to_os_string())).unwrap_or_default();
        let suggested = match model.uri["scheme"] == "untitled" {
            true => self.default_file_path().join(name),
            false => uri_parts(&model.uri).map(|(_, path)| path).unwrap_or_default(),
        };
        let snapshot = model.snapshot(|cell| self.cell_text(&cell.uri));
        let (source, view_type, untitled) = (model.uri.clone(), model.view_type.clone(), model.uri["scheme"] == "untitled");
        let picked = self.pick_file_to_save(suggested, cx);
        let key = key.to_owned();
        cx.spawn(async move |workbench, cx| {
            let Ok(Some(target)) = picked.await else {
                let _ = sender.send(false);
                return;
            };
            let written = match notebook_to_data(&workbench, cx, &view_type, snapshot).await {
                Ok(bytes) => match workbench.update(cx, |workbench, _| workbench.host.run_files(FileJob::Write(resource(&exthost::scan::file_uri(&target)), bytes))) {
                    Ok(write) => matches!(write.await, Ok(Ok(_))),
                    Err(_) => false,
                },
                Err(_) => false,
            };
            if written {
                let _ = workbench.update(cx, |workbench, cx| workbench.replace_notebook_editor(&key, &source, untitled, &target, &view_type, cx));
            }
            let _ = sender.send(written);
        })
        .detach();
        receiver
    }

    /// After a save as: the source notebook's editor closes (an untitled source is clean and let go) and the target
    /// opens as a notebook in its place.
    fn replace_notebook_editor(&mut self, key: &str, source: &Value, untitled: bool, target: &std::path::Path, view_type: &str, cx: &mut Context<Self>) {
        if untitled && let Some(model) = self.host.notebooks.model_mut(key) {
            model.dirty = false;
            self.host.send("ExtHostNotebookDocuments", "$acceptDirtyStateChanged", vec![source.clone(), json!(false)]);
        }
        self.host.notebooks.held.retain(|held| held != key);
        if let Some(index) = self.editors.editors.iter().position(|editor| editor.document().is(NOTEBOOK_EDITOR_SCHEME, Path::new(key))) {
            self.editors.close(index);
        }
        let target_uri = exthost::scan::file_uri(target);
        let view_type = view_type.to_owned();
        self.in_window(cx, move |workbench, window, cx| drop(workbench.open_with_resolver(&target_uri, Some(&view_type), json!({ "pinned": true }), window, cx)));
        self.editors_changed(cx);
    }

    /// The stored or untitled file working copy's `backup`: the serializer's bytes of a snapshot (outputs past
    /// `notebook.backup.sizeLimit` KB fail it) with a file's stat as meta. Answers the version the backup is for, None
    /// when it failed.
    pub(super) fn backup_notebook(&mut self, key: &str, cx: &mut Context<Self>) -> oneshot::Receiver<Option<u64>> {
        let (sender, receiver) = oneshot::channel();
        let (Some(home), Some(model)) = (self.lifecycle.backups.home.clone(), self.host.notebooks.model(key)) else {
            let _ = sender.send(None);
            return receiver;
        };
        let limit = self.setting("notebook.backup.sizeLimit").as_u64().unwrap_or(10000) * 1024;
        let (uri, view_type, version) = (model.uri.clone(), model.view_type.clone(), model.version_id);
        if limit > 0 && model.output_size() as u64 > limit {
            eprintln!("[backup tracker] backing up {}: Notebook too large to backup", notebook::label(&uri));
            let _ = sender.send(None);
            return receiver;
        }
        let snapshot = model.snapshot(|cell| self.cell_text(&cell.uri));
        let meta = match uri_parts(&uri) {
            Ok((scheme, path)) if scheme == "file" => super::backup_tracker::file_meta(&path, false),
            _ => None,
        };
        let done = self.lifecycle.backups.start_operation();
        cx.spawn(async move |workbench, cx| {
            let written = match notebook_to_data(&workbench, cx, &view_type, snapshot).await {
                Ok(bytes) => crate::uri::Uri::from_json(&uri).and_then(|resource| backups::write(&home, &resource, &notebook::working_copy_type_id(&view_type), meta.as_ref(), &bytes).map_err(|error| error.to_string())),
                Err(error) => Err(error.to_string()),
            };
            if let Err(error) = &written {
                eprintln!("[backup tracker] backing up {}: {error}", notebook::label(&uri));
            }
            let _ = sender.send(written.ok().map(|()| version));
            drop(done);
        })
        .detach();
        receiver
    }

    pub(super) fn discard_notebook_backup(&self, uri: &Value, view_type: &str) {
        if let (Some(home), Ok(resource)) = (&self.lifecycle.backups.home, crate::uri::Uri::from_json(uri)) {
            backups::discard(home, &resource, &notebook::working_copy_type_id(view_type));
        }
    }

    /// The backed up bytes of a notebook (`resolveFromBackup`), None without a backup.
    fn notebook_backup(&self, uri: &Value, view_type: &str) -> Option<Vec<u8>> {
        let home = self.lifecycle.backups.home.as_ref()?;
        let resource = crate::uri::Uri::from_json(uri).ok()?;
        backups::resolve(&backups::backup_path(home, &resource, &notebook::working_copy_type_id(view_type))).map(|backup| backup.content)
    }

    /// SimpleNotebookWorkingCopyEditorHandler `createEditor` for a backup without an open notebook: the notebook
    /// opens pinned in the background and resolves from its backup.
    pub(super) fn restore_notebook_backup(&mut self, uri: Value, view_type: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_notebook_type(&view_type) || self.host.notebooks.model(&notebook::uri_string(&uri)).is_some() {
            return;
        }
        drop(self.open_notebook(uri, view_type, json!({ "pinned": true, "preserveFocus": true, "inactive": true }), window, cx));
    }

    pub(super) fn save_dirty_notebooks(&mut self, cx: &mut Context<Self>) -> Vec<oneshot::Receiver<bool>> {
        let dirty: Vec<String> = self.host.notebooks.models.iter().filter(|model| model.dirty && model.uri["scheme"] != "untitled").map(|model| model.uri_string()).collect();
        dirty.iter().map(|key| self.save_notebook(key, cx)).collect()
    }

    /// Every dirty notebook saved as its editor saves it: an untitled one through the save dialog.
    pub(super) fn save_dirty_notebook_editors(&mut self, cx: &mut Context<Self>) -> Vec<oneshot::Receiver<bool>> {
        let dirty: Vec<String> = self.host.notebooks.models.iter().filter(|model| model.dirty).map(NotebookModel::uri_string).collect();
        dirty.iter().map(|key| self.save_notebook_editor(key, cx)).collect()
    }

    /// `_workbench.revertAllDirty` for notebooks: a file notebook reads its resource again, an untitled one is clean.
    pub(super) fn revert_dirty_notebooks(&mut self, cx: &mut Context<Self>) {
        let dirty: Vec<String> = self.host.notebooks.models.iter().filter(|model| model.dirty).map(NotebookModel::uri_string).collect();
        for key in dirty {
            self.revert_notebook(&key, cx);
        }
    }

    /// `revert` of one notebook: a file notebook reads its resource again, an untitled one is clean.
    pub(super) fn revert_notebook(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(model) = self.host.notebooks.model(key) else { return };
        let (uri, view_type) = (model.uri.clone(), model.view_type.clone());
        if uri["scheme"] == "untitled" {
            self.host.notebooks.model_mut(key).expect("found above").dirty = false;
            self.host.send("ExtHostNotebookDocuments", "$acceptDirtyStateChanged", vec![uri, json!(false)]);
            return;
        }
        cx.spawn(async move |workbench, cx| {
            let Ok(data) = read_notebook(&workbench, cx, &uri, &view_type).await else { return };
            let _ = workbench.update(cx, |workbench, cx| {
                let key = notebook::uri_string(&uri);
                let Some(model) = workbench.host.notebooks.model_mut(&key) else { return };
                let edits = reset_edits(model, &data);
                if workbench.apply_notebook_edits(&key, &edits, cx).is_ok() {
                    workbench.host.notebooks.model_mut(&key).expect("still open").dirty = false;
                    workbench.host.send("ExtHostNotebookDocuments", "$acceptDirtyStateChanged", vec![uri, json!(false)]);
                    workbench.editors_changed(cx);
                }
            });
        })
        .detach();
    }

    /// `$tryShowNotebookDocument`: the notebook's editor in the group of `position`, opened when that group has none.
    fn try_show_notebook(&mut self, uri: Value, options: Value, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        match self.notebook_view_type(&uri) {
            Ok(view_type) => self.open_notebook(uri, view_type, options, window, cx),
            Err(error) => Outcome::Ready(Err(error)),
        }
    }

    pub(super) fn is_notebook_type(&self, view_type: &str) -> bool {
        self.notebook_types().iter().any(|kind| kind.view_type == view_type)
    }

    /// Shows the notebook of `uri` as `view_type`, resolving its model first when it is not open.
    pub(super) fn open_notebook(&mut self, uri: Value, view_type: String, options: Value, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let key = notebook::uri_string(&uri);
        if self.host.notebooks.model(&key).is_some() {
            return Outcome::Ready(self.show_notebook(&key, &options, window, cx));
        }
        let (sender, receiver) = oneshot::channel();
        // The editor input's model reference: the resolved model stays until its editor shows it.
        let opening = !self.host.notebooks.held.contains(&key);
        if opening {
            self.host.notebooks.held.push(key.clone());
        }
        cx.spawn_in(window, async move |workbench, cx| {
            let result = resolve_notebook(&workbench, cx, uri.clone(), view_type, None).await;
            let shown = workbench.update_in(cx, |workbench, window, cx| {
                let shown = result.and_then(|_| workbench.show_notebook(&key, &options, window, cx));
                if opening {
                    workbench.host.notebooks.held.retain(|held| *held != key);
                }
                shown
            });
            let _ = sender.send(shown.unwrap_or_else(|_| Err(serialized_error("Canceled"))));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// Opens or activates the notebook's editor; the editor id the host knows it by.
    pub(super) fn show_notebook(&mut self, key: &str, options: &Value, window: &mut Window, cx: &mut Context<Self>) -> NotebookResult {
        self.activate_column(&options["position"]);
        let pinned = options["pinned"] != false;
        let previous = self.editors.active_tab().map(|editor| editor.model_id);
        let path = Path::new(key);
        match self.editors.editors.iter().position(|editor| editor.document().is(NOTEBOOK_EDITOR_SCHEME, path)) {
            Some(index) => {
                self.editors.activate(index);
                if pinned {
                    self.editors.pin(index);
                }
            }
            None => {
                let document = TextDocument::from_text(NOTEBOOK_EDITOR_SCHEME, path, "", "utf8".into(), Some("plaintext"), &self.languages, &self.settings.lock().unwrap());
                self.editors.open(document, !pinned);
            }
        }
        let tab_model_id = self.editors.active_tab().expect("just shown").model_id;
        if options["inactive"] == true
            && let Some(index) = previous.and_then(|id| self.editors.editors.iter().position(|editor| editor.model_id == id))
        {
            self.editors.activate(index);
        }
        self.sync_notebook_editors();
        if let Some(selections) = options["selections"].as_array().filter(|selections| !selections.is_empty()) {
            let selections = cell_ranges(&json!(selections));
            let editor = self.host.notebooks.editors.iter_mut().find(|editor| editor.tab_model_id == tab_model_id).expect("synced above");
            editor.focus = selections[0].0;
            editor.selections = selections;
            self.sync_notebook_editors();
        }
        // notebookEditorWidget.ts focuses the cell list, not a cell's editor: the editor takes the focus on a click
        // into a cell or Enter.
        if options["preserveFocus"] != true {
            window.focus(&self.notebook_views.list_focus, cx);
        }
        self.editors_changed(cx);
        self.update_notebook_cell_status_bars(key, cx);
        Ok(json!(self.host.notebooks.editor(tab_model_id).expect("synced above").id))
    }

    /// The notebook editor of every notebook tab; a new tab's editor focuses the first cell.
    fn sync_notebook_editors(&mut self) {
        let mut tabs: Vec<(u32, String)> = Vec::new();
        for group in self.editors.group_ids_mru() {
            let (editors, _) = self.editors.group_tabs(group);
            tabs.extend(editors.iter().filter(|editor| editor.document().scheme == NOTEBOOK_EDITOR_SCHEME).map(|editor| (editor.model_id, editor.document().path.to_string_lossy().into_owned())));
        }
        let notebooks = &mut self.host.notebooks;
        notebooks.editors.retain(|editor| tabs.iter().any(|(id, _)| *id == editor.tab_model_id));
        for (tab_model_id, notebook) in tabs {
            if notebooks.editor(tab_model_id).is_none() {
                notebooks.editors.push(NotebookEditor { tab_model_id, id: format!("$notebookEditor{tab_model_id}"), notebook, focus: 0, selections: vec![(0, 1)] });
            }
        }
        self.editors.stacked_editors.clear();
        for editor in &notebooks.editors {
            let Some(model) = notebooks.model(&editor.notebook) else { continue };
            let Some(cell) = model.cells.get(editor.focus).or(model.cells.last()) else { continue };
            let (scheme, path) = uri_parts(&cell.uri).expect("cell URIs are built by notebook::cell_uri");
            self.editors.stacked_editors.insert(editor.tab_model_id, crate::diff_editor::Side { scheme: scheme.to_owned(), path });
        }
    }

    /// mainThreadNotebookDocumentsAndEditors.ts `_updateState`: the notebooks, the editors of visible notebook tabs and
    /// the active one, as a delta from what the host has. Sent before the text documents, so a cell's document opens
    /// with its notebook there.
    pub(super) fn send_notebook_changes(&mut self) {
        self.sync_notebook_editors();
        self.release_notebooks();
        let visible: Vec<(Option<usize>, u32)> = self
            .groups_in_grid_order()
            .into_iter()
            .enumerate()
            .filter_map(|(column, group)| self.editors.group_active_tab(group).filter(|editor| editor.document().scheme == NOTEBOOK_EDITOR_SCHEME).map(|editor| (Some(column), editor.model_id)))
            .collect();
        let active = self.editors.active_tab().filter(|editor| editor.document().scheme == NOTEBOOK_EDITOR_SCHEME).map(|editor| editor.model_id);
        let notebooks = &self.host.notebooks;
        let documents: Vec<String> = notebooks.models.iter().map(NotebookModel::uri_string).collect();
        let editor_ids: Vec<String> = visible.iter().filter_map(|(_, id)| notebooks.editor(*id)).map(|editor| editor.id.clone()).collect();
        let active_id = active.and_then(|id| notebooks.editor(id)).map(|editor| editor.id.clone());
        let mut delta = serde_json::Map::new();
        let removed_documents: Vec<Value> = notebooks.sent_documents.iter().filter(|(key, _)| !documents.contains(key)).map(|(_, uri)| uri.clone()).collect();
        let added_documents: Vec<Value> = notebooks.models.iter().filter(|model| !notebooks.sent_documents.iter().any(|(key, _)| *key == model.uri_string())).map(|model| model.added_data(|cell| self.cell_text(&cell.uri))).collect();
        let removed_editors: Vec<&String> = notebooks.sent_editors.iter().filter(|id| !editor_ids.contains(id)).collect();
        let mut added_editors = Vec::new();
        for (column, tab_model_id) in &visible {
            let editor = notebooks.editor(*tab_model_id).expect("synced above");
            if notebooks.sent_editors.contains(&editor.id) {
                continue;
            }
            let model = notebooks.model(&editor.notebook).expect("an editor shows an open notebook");
            added_editors.push(json!({
                "id": editor.id,
                "documentUri": model.uri,
                "selections": editor.selections.iter().map(|(start, end)| json!({ "start": start, "end": end })).collect::<Vec<_>>(),
                "visibleRanges": [{ "start": 0, "end": model.cells.len() }],
                "viewColumn": column,
                "viewType": model.view_type,
            }));
        }
        if !removed_documents.is_empty() {
            delta.insert("removedDocuments".into(), json!(removed_documents));
        }
        if !added_documents.is_empty() {
            delta.insert("addedDocuments".into(), json!(added_documents));
        }
        if !removed_editors.is_empty() {
            delta.insert("removedEditors".into(), json!(removed_editors));
        }
        if !added_editors.is_empty() {
            delta.insert("addedEditors".into(), json!(added_editors));
        }
        if active_id != notebooks.sent_active {
            delta.insert("newActiveEditor".into(), json!(active_id));
        }
        if editor_ids != notebooks.sent_visible {
            delta.insert("visibleEditors".into(), json!(editor_ids));
        }
        if delta.is_empty() {
            return;
        }
        self.host.send("ExtHostNotebook", "$acceptDocumentAndEditorsDelta", vec![json!({ WITH_BUFFERS_KEY: delta })]);
        let notebooks = &mut self.host.notebooks;
        notebooks.sent_documents = notebooks.models.iter().map(|model| (model.uri_string(), model.uri.clone())).collect();
        notebooks.sent_editors = editor_ids.clone();
        notebooks.sent_active = active_id;
        notebooks.sent_visible = editor_ids;
    }

    /// A notebook neither the host holds nor an editor shows is disposed, with its cells' text models.
    fn release_notebooks(&mut self) {
        let notebooks = &self.host.notebooks;
        let released: Vec<String> = notebooks
            .models
            .iter()
            .map(NotebookModel::uri_string)
            .filter(|key| !notebooks.held.contains(key) && !notebooks.editors.iter().any(|editor| editor.notebook == *key))
            // `canDispose`: a dirty notebook stays until it is saved or reverted.
            .filter(|key| notebooks.model(key).is_some_and(|model| !model.dirty))
            .collect();
        for key in released {
            let index = self.host.notebooks.models.iter().position(|model| model.uri_string() == key).expect("listed above");
            let model = self.host.notebooks.models.remove(index);
            let cells: Vec<(String, std::path::PathBuf)> = model.cells.iter().filter_map(|cell| uri_parts(&cell.uri).ok()).map(|(scheme, path)| (scheme.to_owned(), path)).collect();
            self.editors.models.retain(|editor| !cells.iter().any(|(scheme, path)| editor.document().is(scheme, path)));
        }
    }


    fn active_notebook_editor_mut(&mut self) -> Option<&mut NotebookEditor> {
        let tab_model_id = self.editors.active_tab().filter(|editor| editor.document().scheme == NOTEBOOK_EDITOR_SCHEME)?.model_id;
        self.host.notebooks.editors.iter_mut().find(|editor| editor.tab_model_id == tab_model_id)
    }

    /// insertCellActions.ts `insertCell` below the focused cell (at the end without one): a code cell in the
    /// language of the code cell above it, else of the notebook's first code cell; the new cell is focused.
    fn insert_cell_below(&mut self, kind: u64, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = self.active_notebook_editor() else { return Outcome::done() };
        let key = editor.notebook.clone();
        let model = self.host.notebooks.model(&key).expect("an editor shows an open notebook");
        let index = if model.cells.is_empty() { 0 } else { (editor.focus + 1).min(model.cells.len()) };
        let code_above = model.cells[..index].iter().rev().chain(&model.cells).find(|cell| cell.kind == notebook::CODE);
        let language = match kind == notebook::CODE {
            true => code_above.map_or("plaintext".to_owned(), |cell| cell.language.clone()),
            false => "markdown".to_owned(),
        };
        let cell = json!({ "cellKind": kind, "language": language, "source": "", "outputs": [], "metadata": {} });
        let insert = json!({ "editType": 1, "index": index, "count": 0, "cells": [cell] });
        if let Err(error) = self.apply_notebook_edits(&key, std::slice::from_ref(&insert), cx) {
            return Outcome::Ready(Err(error));
        }
        self.push_cells_undo(&key, vec![json!({ "editType": 1, "index": index, "count": 1, "cells": [] })], vec![insert]);
        let handle = self.host.notebooks.model(&key).expect("still open").cells[index].handle;
        if let Some(editor) = self.active_notebook_editor_mut() {
            editor.focus = index;
            editor.selections = vec![(index, index + 1)];
        }
        if kind != notebook::CODE {
            self.edit_markup_cell(handle);
        }
        self.editors_changed(cx);
        window.focus(&self.editor_focus, cx);
        Outcome::done()
    }

    /// notebookKernelQuickPickStrategy.ts `KernelPickerFlatStrategy`: the kernels for the active notebook, the
    /// selected one first and described as such; the picked one is bound.
    // shortcut: no "Install/Enable suggested extensions" and "Browse marketplace" items until the gallery (milestone 18).
    fn show_kernel_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(key) = self.active_notebook_editor().map(|editor| editor.notebook.clone()) else { return Outcome::done() };
        let model = self.host.notebooks.model(&key).expect("an editor shows an open notebook");
        let (view_type, uri) = (model.view_type.clone(), model.uri.clone());
        let (all, selected) = self.host.notebooks.kernels.matching(&view_type, &key);
        let selected_id = selected.map(|kernel| kernel.id.clone());
        let mut kernels: Vec<(String, crate::quick_input::Item)> = Vec::new();
        for kernel in all {
            let current = selected_id.as_deref() == Some(kernel.id.as_str());
            let description = match current {
                true => Some(kernel.description.clone().map_or("Currently Selected".to_owned(), |description| format!("{description} - Currently Selected"))),
                false => kernel.description.clone(),
            };
            let item = crate::quick_input::Item { handle: kernels.len() as i64, label: kernel.label.clone(), description, detail: kernel.detail.clone(), pickable: true, ..Default::default() };
            kernels.push((kernel.id.clone(), item));
        }
        kernels.sort_by_key(|(id, _)| selected_id.as_deref() != Some(id.as_str()));
        let mut input = crate::quick_input::QuickInput::new(0, crate::quick_input::Kind::QuickPick);
        input.placeholder = Some("Select kernel".into());
        input.set_items(kernels.iter().map(|(_, item)| crate::quick_input::Entry::Item(item.clone())).collect());
        let ids: Vec<(i64, String)> = kernels.into_iter().map(|(id, item)| (item.handle, id)).collect();
        let picked = self.choose(input, window, cx);
        cx.spawn(async move |workbench, cx| {
            let Ok(Ok(handle)) = picked.await else { return };
            let Some((_, kernel_id)) = handle.as_i64().and_then(|handle| ids.iter().find(|(item, _)| *item == handle)) else { return };
            let _ = workbench.update(cx, |workbench, cx| {
                let associations = workbench.host.notebooks.kernels.select(Some(kernel_id.as_str()), &view_type, &uri);
                for association in &associations {
                    workbench.host.send("ExtHostNotebookKernels", "$acceptNotebookAssociation", association_args(association));
                }
                cx.notify();
            });
        })
        .detach();
        Outcome::done()
    }

    /// The scheme and path of the active notebook editor's notebook.
    pub(super) fn active_notebook_resource(&self) -> Option<(String, std::path::PathBuf)> {
        let model = self.host.notebooks.model(&self.active_notebook_editor()?.notebook)?;
        uri_parts(&model.uri).ok().map(|(scheme, path)| (scheme.to_owned(), path))
    }

    pub(super) fn active_notebook_editor(&self) -> Option<&NotebookEditor> {
        let tab_model_id = self.editors.active_tab().filter(|editor| editor.document().scheme == NOTEBOOK_EDITOR_SCHEME)?.model_id;
        self.host.notebooks.editor(tab_model_id)
    }

    /// `getEditorFromArgsOrActivePane`: the notebook of a URI argument, else the active editor's.
    fn notebook_from_args(&self, uri: Option<&Value>) -> Option<String> {
        match uri.filter(|uri| uri.is_object()) {
            Some(uri) => self.host.notebooks.model(&notebook::uri_string(uri)).map(NotebookModel::uri_string),
            None => self.active_notebook_editor().map(|editor| editor.notebook.clone()),
        }
    }

    /// `parseMultiCellExecutionArgs`: `{ ranges, document }`, a range and a document, else the active editor's
    /// selected cells.
    fn cells_from_args(&self, args: &[Value]) -> Option<(String, Vec<usize>)> {
        let first = args.first().unwrap_or(&Value::Null);
        let (key, ranges) = if first["ranges"].is_array() {
            (self.notebook_from_args(first.get("document"))?, cell_ranges(&first["ranges"]))
        } else if first["start"].is_u64() && first["end"].is_u64() {
            (self.notebook_from_args(args.get(1))?, cell_ranges(&json!([first])))
        } else {
            let editor = self.active_notebook_editor()?;
            (editor.notebook.clone(), vec![(editor.focus, editor.focus + 1)])
        };
        let len = self.host.notebooks.model(&key)?.cells.len();
        let indices = ranges.into_iter().flat_map(|(start, end)| start..end.min(len)).collect();
        Some((key, indices))
    }

    /// `_notebook.selectKernel` with `{ id, extension }`: binds that kernel to the active notebook; without one the
    /// kernel picker would show (it waits for the notebook editor's toolbar).
    fn select_kernel(&mut self, context: &Value, cx: &mut Context<Self>) -> bool {
        let editor = match context["notebookEditorId"].as_str() {
            Some(id) => self.host.notebooks.editors.iter().find(|editor| editor.id == id),
            None => self.active_notebook_editor(),
        };
        let Some(key) = editor.map(|editor| editor.notebook.clone()) else { return false };
        let (Some(id), Some(extension)) = (context["id"].as_str(), context["extension"].as_str()) else { return false };
        let wanted = format!("{extension}/{id}");
        let model = self.host.notebooks.model(&key).expect("an editor shows an open notebook");
        let (view_type, uri) = (model.view_type.clone(), model.uri.clone());
        let (all, selected) = self.host.notebooks.kernels.matching(&view_type, &key);
        if selected.is_some_and(|kernel| kernel.id.eq_ignore_ascii_case(&wanted)) && context["skipIfAlreadySelected"] == true {
            return true;
        }
        let Some(kernel_id) = all.iter().find(|kernel| kernel.id.eq_ignore_ascii_case(&wanted)).map(|kernel| kernel.id.clone()) else { return false };
        let associations = self.host.notebooks.kernels.select(Some(&kernel_id), &view_type, &uri);
        for association in &associations {
            self.host.send("ExtHostNotebookKernels", "$acceptNotebookAssociation", association_args(association));
        }
        cx.notify();
        true
    }

    /// notebookExecutionServiceImpl.ts `executeNotebookCells`: each cell without an execution gets one, the
    /// resolved kernel is bound and runs the cells its languages allow; the outcome resolves once it returns.
    fn execute_notebook_cells(&mut self, key: &str, handles: Vec<u64>, cx: &mut Context<Self>) -> Outcome {
        let model = self.host.notebooks.model(key).expect("an open notebook");
        let (view_type, uri) = (model.view_type.clone(), model.uri.clone());
        let mut started = Vec::new();
        for handle in handles {
            if self.host.notebooks.kernels.start_cell(key, handle) {
                started.push(handle);
                self.initialize_cell_execution(key, handle, cx);
            }
        }
        let Some(kernel) = self.host.notebooks.kernels.resolve(&view_type, key).cloned() else {
            for handle in started {
                self.complete_cell_execution(key, handle, &json!({}), cx);
            }
            return Outcome::done();
        };
        let mut valid = Vec::new();
        for handle in started {
            let model = self.host.notebooks.model(key).expect("an open notebook");
            let language = model.index_of_handle(handle).map(|index| model.cells[index].language.clone()).unwrap_or_default();
            let supported = match kernel.supported_languages.is_empty() {
                true => self.languages.ids().contains(&language),
                false => kernel.supported_languages.contains(&language),
            };
            match supported {
                true => valid.push(handle),
                false => self.complete_cell_execution(key, handle, &json!({}), cx),
            }
        }
        if valid.is_empty() {
            return Outcome::done();
        }
        let associations = self.host.notebooks.kernels.select(Some(&kernel.id), &view_type, &uri);
        for association in &associations {
            self.host.send("ExtHostNotebookKernels", "$acceptNotebookAssociation", association_args(association));
        }
        let executed = self.host.call("ExtHostNotebookKernels", "$executeCells", vec![json!(kernel.handle), uri, json!(valid)]);
        let key = key.to_owned();
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let result = executed.await.unwrap_or(Ok(Value::Null));
            // Executions the kernel never confirmed are completed.
            let _ = workbench.update(cx, |workbench, cx| {
                for handle in valid {
                    if workbench.host.notebooks.kernels.state(&key, handle) == Some(ExecutionState::Unconfirmed) {
                        workbench.complete_cell_execution(&key, handle, &json!({}), cx);
                    }
                }
            });
            let _ = sender.send(result.map(|_| Value::Null));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `CellExecution.initialize`.
    fn initialize_cell_execution(&mut self, key: &str, handle: u64, cx: &mut Context<Self>) {
        let execution_id = exthost::main_thread_storage::generate_uuid();
        let internal = json!({ "executionId": execution_id, "runStartTime": null, "runEndTime": null, "lastRunSuccess": null, "executionOrder": null, "renderDuration": null });
        let _ = self.apply_notebook_edits(key, &[json!({ "editType": 9, "handle": handle, "internalMetadata": internal })], cx);
    }

    /// `CellExecution.complete`, then the execution is gone.
    fn complete_cell_execution(&mut self, key: &str, handle: u64, completion: &Value, cx: &mut Context<Self>) {
        self.host.notebooks.kernels.finish_cell(key, handle);
        self.host.notebooks.kernels.executions.retain(|_, execution| !(execution.notebook == key && execution.cell_handle == handle));
        let Some(model) = self.host.notebooks.model(key) else { return };
        let Some(index) = model.index_of_handle(handle) else { return };
        let start = model.cells[index].internal_metadata["runStartTime"].clone();
        let internal = json!({ "lastRunSuccess": completion["lastRunSuccess"], "runStartTime": start, "runEndTime": completion["runEndTime"], "error": completion["error"] });
        let _ = self.apply_notebook_edits(key, &[json!({ "editType": 9, "handle": handle, "internalMetadata": internal })], cx);
    }

    /// `onWillAddRemoveCells`: running cells about to be removed are cancelled through their notebook's kernel.
    fn cancel_removed_executions(&mut self, key: &str, view_type: &str, uri: &Value, removed: &[u64]) {
        let kernels = &self.host.notebooks.kernels;
        let running: Vec<u64> = removed.iter().copied().filter(|handle| kernels.state(key, *handle).is_some()).collect();
        let Some(kernel) = kernels.resolve(view_type, key).filter(|_| !running.is_empty()) else { return };
        let kernel = kernel.handle;
        drop(self.host.call("ExtHostNotebookKernels", "$cancelCells", vec![json!(kernel), uri.clone(), json!(running)]));
    }

    fn handle_kernel_request(&mut self, method: &str, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let null = Value::Null;
        let arg = |index: usize| args.get(index).unwrap_or(&null);
        let handle = arg(0).as_u64().unwrap_or_default();
        match method {
            "$addKernel" => {
                for association in self.host.notebooks.kernels.add(handle, arg(1)) {
                    self.host.send("ExtHostNotebookKernels", "$acceptNotebookAssociation", association_args(&association));
                }
                Outcome::done()
            }
            "$updateKernel" => {
                self.host.notebooks.kernels.update(handle, arg(1));
                Outcome::done()
            }
            "$removeKernel" => {
                for association in self.host.notebooks.kernels.remove(handle) {
                    self.host.send("ExtHostNotebookKernels", "$acceptNotebookAssociation", association_args(&association));
                }
                Outcome::done()
            }
            "$receiveVariable" => {
                if let Some(collected) = arg(0).as_str().and_then(|id| self.host.notebooks.kernels.variable_requests.get_mut(id)) {
                    collected.push(arg(1).clone());
                }
                Outcome::done()
            }
            "$updateNotebookPriority" => {
                self.host.notebooks.kernels.update_priority(handle, &notebook::uri_string(arg(1)), arg(2).as_i64());
                Outcome::done()
            }
            "$createExecution" => Outcome::Ready(self.create_execution(handle, arg(1).as_str().unwrap_or_default(), arg(2), arg(3).as_u64().unwrap_or_default(), cx)),
            "$updateExecution" => {
                self.update_execution(handle, arg(1), cx);
                Outcome::done()
            }
            "$completeExecution" => {
                if let Some(execution) = self.host.notebooks.kernels.executions.remove(&handle) {
                    self.complete_cell_execution(&execution.notebook, execution.cell_handle, arg(1), cx);
                }
                Outcome::done()
            }
            "$postMessage" => {
                let editor_id = arg(1).as_str().map(str::to_owned);
                let message = json!({ "__vscode_notebook_message": true, "type": "customKernelMessage", "message": arg(2) });
                let notebooks = &self.host.notebooks;
                let bound: Vec<String> = notebooks.models.iter().filter(|model| notebooks.kernels.matching(&model.view_type, &model.uri_string()).1.is_some_and(|kernel| kernel.handle == handle)).map(NotebookModel::uri_string).collect();
                Outcome::Ready(Ok(json!(self.post_to_notebook_webviews(message, |editor| bound.contains(&editor.notebook) && editor_id.as_ref().is_none_or(|id| *id == editor.id)))))
            }
            _ => Outcome::done(),
        }
    }

    /// `$createExecution`: only the notebook's bound kernel may run its cells.
    fn create_execution(&mut self, handle: u64, controller_id: &str, uri: &Value, cell_handle: u64, cx: &mut Context<Self>) -> NotebookResult {
        let key = notebook::uri_string(uri);
        let model = self.host.notebooks.model(&key).ok_or_else(|| serialized_error(&format!("Notebook not found: {key}")))?;
        let (_, selected) = self.host.notebooks.kernels.matching(&model.view_type, &key);
        let selected = selected.map(|kernel| kernel.id.clone());
        if selected.as_deref() != Some(controller_id) {
            return Err(serialized_error(&format!("Kernel is not selected: {} !== {controller_id}", selected.unwrap_or_else(|| "undefined".into()))));
        }
        if self.host.notebooks.kernels.start_cell(&key, cell_handle) {
            self.initialize_cell_execution(&key, cell_handle, cx);
        }
        let execution = notebook::kernels::Execution { notebook: key, cell_handle, state: ExecutionState::Pending };
        self.host.notebooks.kernels.executions.insert(handle, execution);
        Ok(Value::Null)
    }

    /// `$updateExecution`: outputs and the execution state as edits of the cell (`updateToEdit`).
    fn update_execution(&mut self, handle: u64, updates: &Value, cx: &mut Context<Self>) {
        let Some(execution) = self.host.notebooks.kernels.executions.get_mut(&handle) else { return };
        let (key, cell_handle) = (execution.notebook.clone(), execution.cell_handle);
        let mut edits = Vec::new();
        for update in updates.as_array().into_iter().flatten() {
            match update["editType"].as_u64() {
                Some(UPDATE_OUTPUT) => edits.push(json!({ "editType": 2, "handle": update["cellHandle"], "append": update["append"], "outputs": update["outputs"] })),
                Some(UPDATE_OUTPUT_ITEMS) => edits.push(json!({ "editType": 7, "outputId": update["outputId"], "append": update["append"], "items": update["items"] })),
                Some(UPDATE_EXECUTION_STATE) => {
                    execution.state = ExecutionState::Executing;
                    let mut internal = serde_json::Map::new();
                    for key in ["executionOrder", "runStartTime"] {
                        if let Some(value) = update.get(key) {
                            internal.insert(key.into(), value.clone());
                        }
                    }
                    edits.push(json!({ "editType": 9, "handle": cell_handle, "internalMetadata": internal }));
                }
                _ => {}
            }
        }
        if self.host.notebooks.model(&key).is_some_and(|model| model.index_of_handle(cell_handle).is_some()) {
            let _ = self.apply_notebook_edits(&key, &edits, cx);
        }
    }

    /// Opening a cell's URI as an editor shows its notebook (`vscode.open`, `showTextDocument` of a cell document).
    pub(super) fn show_notebook_of_cell(&mut self, cell_uri: &Value, options: &Value, window: &mut Window, cx: &mut Context<Self>) -> Option<NotebookResult> {
        let (model, index) = self.host.notebooks.cell(cell_uri)?;
        let key = model.uri_string();
        let mut options = options.clone();
        options["selections"] = json!([{ "start": index, "end": index + 1 }]);
        if options.get("pinned").is_none() {
            options["pinned"] = json!(true);
        }
        Some(self.show_notebook(&key, &options, window, cx).map(|_| {
            // The focused cell's model is what the active code editor edits.
            self.editors.active().filter(|cell| cell.document().scheme == CELL_SCHEME).map_or(Value::Null, |cell| json!(crate::host_sync::snapshot_id(cell)))
        }))
    }
}

/// The text models of the cells of a notebook tab, in order; none for another tab.
pub(crate) fn notebook_cells<'a>(notebooks: &notebook::Notebooks, editors: &'a crate::editor::EditorGroup, tab: &OpenEditor) -> Vec<&'a OpenEditor> {
    let Some(model) = notebooks.editor(tab.model_id).and_then(|editor| notebooks.model(&editor.notebook)) else { return Vec::new() };
    model.cells.iter().filter_map(|cell| uri_parts(&cell.uri).ok()).filter_map(|(scheme, path)| editors.model(scheme, &path)).collect()
}

/// A contributed or registered notebook type (`NotebookProviderInfo`).
struct NotebookType {
    view_type: String,
    display_name: String,
    priority: String,
    /// `filenamePattern` and `excludeFileNamePattern` of each selector.
    selectors: Vec<(Value, Value)>,
    options: Value,
}

impl NotebookType {
    fn matches(&self, scheme: &str, path: &Path) -> bool {
        let Some(matcher) = super::custom_editors::resource_matcher(scheme, path) else { return false };
        let pattern_matches = |pattern: &Value| pattern.as_str().is_some_and(|pattern| !pattern.is_empty() && matcher(pattern));
        self.selectors.iter().any(|(include, exclude)| match include {
            Value::Object(object) => pattern_matches(&object["include"]) && !pattern_matches(&object["exclude"]),
            include => pattern_matches(include) && !pattern_matches(exclude),
        })
    }

    /// `NotebookProviderInfo.possibleFileEnding`: `.ext` of the first selector shaped like `*.ext`.
    fn file_ending(&self) -> String {
        for (include, _) in &self.selectors {
            let pattern = include.as_str().or(include["include"].as_str()).unwrap_or_default();
            if let Some((_, ending)) = pattern.rsplit_once("*.")
                && !ending.is_empty()
                && ending.chars().all(|c| c.is_alphanumeric() || c == '_')
            {
                return format!(".{ending}");
            }
        }
        String::new()
    }

    /// `_resolveNotebookContentProvider`'s entry.
    fn info(&self) -> Value {
        let patterns: Vec<Value> = self
            .selectors
            .iter()
            .map(|(include, exclude)| match (include, exclude) {
                (Value::Object(_), _) => include.clone(),
                (include, Value::String(exclude)) if !exclude.is_empty() => json!({ "include": include, "exclude": exclude }),
                (include, _) => include.clone(),
            })
            .collect();
        let option = |key: &str| self.options[key].clone();
        json!({
            "viewType": self.view_type,
            "displayName": self.display_name,
            "filenamePattern": patterns,
            "options": { "transientCellMetadata": option("transientCellMetadata"), "transientDocumentMetadata": option("transientDocumentMetadata"), "transientOutputs": option("transientOutputs") == true },
        })
    }
}

/// `ICellRange`s as (start, end).
fn cell_ranges(ranges: &Value) -> Vec<(usize, usize)> {
    ranges.as_array().into_iter().flatten().map(|range| (range["start"].as_u64().unwrap_or_default() as usize, range["end"].as_u64().unwrap_or_default() as usize)).collect()
}

/// The bytes of a buffer argument: a byte array, or `{"$vsbuffer": base64}`.
fn buffer_bytes(value: &Value) -> Vec<u8> {
    match &value[VSBUFFER_KEY] {
        Value::String(encoded) => STANDARD.decode(encoded).unwrap_or_default(),
        _ => serde_json::from_value(value.clone()).unwrap_or_default(),
    }
}

/// A `NotebookDataDto` with its outputs' bytes written as buffers, for a `SerializableObjectWithBuffers`.
fn buffers_of(mut data: Value) -> Value {
    for cell in data["cells"].as_array_mut().into_iter().flatten() {
        for output in cell["outputs"].as_array_mut().into_iter().flatten() {
            for item in output["items"].as_array_mut().into_iter().flatten() {
                let bytes = buffer_bytes(&item["valueBytes"]);
                item["valueBytes"] = json!({ VSBUFFER_KEY: STANDARD.encode(bytes) });
            }
        }
    }
    data
}

/// The edits `reset` makes: every cell replaced by the data's, its metadata set.
fn reset_edits(model: &NotebookModel, data: &Value) -> Vec<Value> {
    vec![
        json!({ "editType": 1, "index": 0, "count": model.cells.len(), "cells": data["cells"] }),
        json!({ "editType": 5, "metadata": data["metadata"] }),
    ]
}

fn canceled<T>(_: T) -> Value {
    serialized_error("Canceled")
}

/// notebookServiceImpl.ts `withNotebookDataProvider`: the serializer of `view_type`, after its
/// `onNotebookSerializer` activation when none is registered yet.
async fn serializer_handle(workbench: &WeakEntity<Workbench>, cx: &mut AsyncApp, view_type: &str) -> Result<u64, Value> {
    let registered = workbench
        .update(cx, |workbench, _| match workbench.host.notebooks.serializer(view_type) {
            Some(serializer) => Ok(serializer.handle),
            None => Err(workbench.host.activate_by_event(&format!("onNotebookSerializer:{view_type}"))),
        })
        .map_err(canceled)?;
    let activation = match registered {
        Ok(handle) => return Ok(handle),
        Err(activation) => activation,
    };
    let _ = activation.await;
    let handle = workbench.update(cx, |workbench, _| workbench.host.notebooks.serializer(view_type).map(|serializer| serializer.handle)).map_err(canceled)?;
    handle.ok_or_else(|| serialized_error(&format!("NO provider registered for view type: '{view_type}'")))
}

/// `$dataToNotebook` of the serializer of `view_type`.
async fn data_to_notebook(workbench: &WeakEntity<Workbench>, cx: &mut AsyncApp, view_type: &str, bytes: &[u8]) -> NotebookResult {
    let handle = serializer_handle(workbench, cx, view_type).await?;
    let call = workbench.update(cx, |workbench, _| workbench.host.call("ExtHostNotebook", "$dataToNotebook", vec![json!(handle), json!({ VSBUFFER_KEY: STANDARD.encode(bytes) })])).map_err(canceled)?;
    call.await.map_err(canceled)?
}

/// `$notebookToData` of the serializer of `view_type`: the bytes.
async fn notebook_to_data(workbench: &WeakEntity<Workbench>, cx: &mut AsyncApp, view_type: &str, data: Value) -> Result<Vec<u8>, Value> {
    let handle = serializer_handle(workbench, cx, view_type).await?;
    let call = workbench.update(cx, |workbench, _| workbench.host.call("ExtHostNotebook", "$notebookToData", vec![json!(handle), json!({ WITH_BUFFERS_KEY: buffers_of(data) })])).map_err(canceled)?;
    Ok(buffer_bytes(&call.await.map_err(canceled)??))
}

/// The resource's bytes (its file system or content provider), deserialized.
async fn read_notebook(workbench: &WeakEntity<Workbench>, cx: &mut AsyncApp, uri: &Value, view_type: &str) -> NotebookResult {
    let bytes = match uri["scheme"] == "untitled" {
        true => Vec::new(),
        false => {
            let read = workbench.update(cx, |workbench, _| workbench.host.call("workbench", "resolveTextContent", vec![uri.clone()])).map_err(canceled)?;
            match read.await.map_err(canceled)?? {
                Value::String(text) => text.into_bytes(),
                bytes => buffer_bytes(&bytes),
            }
        }
    };
    data_to_notebook(workbench, cx, view_type, &bytes).await
}

/// `NotebookEditorModelResolverService.resolve`: the model of `uri`, created from `content` (dirty), its backup
/// (dirty) or its resource.
async fn resolve_notebook(workbench: &WeakEntity<Workbench>, cx: &mut AsyncApp, uri: Value, view_type: String, content: Option<Value>) -> NotebookResult {
    let backup = match content {
        Some(_) => None,
        None => workbench.update(cx, |workbench, _| workbench.notebook_backup(&uri, &view_type)).map_err(canceled)?,
    };
    let dirty = content.is_some() || backup.is_some();
    let data = match (content, backup) {
        (Some(content), _) => content,
        (None, Some(bytes)) => data_to_notebook(workbench, cx, &view_type, &bytes).await?,
        (None, None) => read_notebook(workbench, cx, &uri, &view_type).await?,
    };
    workbench.update(cx, |workbench, cx| workbench.add_notebook(uri, &view_type, &data, dirty, cx)).map_err(canceled)?;
    Ok(Value::Null)
}
