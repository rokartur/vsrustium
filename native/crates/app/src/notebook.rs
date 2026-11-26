//! The notebook model of contrib/notebook/common/model/notebookTextModel.ts with notebookCommon.ts: cells with their
//! outputs and metadata, the edits on them and the raw events mainThreadNotebookDocuments.ts sends the extension
//! host. A cell's text is a workbench text model at the cell's `vscode-notebook-cell` URI, so it is not kept here.

pub mod kernels;
pub mod layout;
pub mod outline;
pub mod renderers;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::editor::uri_value;

pub const CELL_SCHEME: &str = "vscode-notebook-cell";
/// The scheme of a notebook editor's tab in an editor group; its path is the notebook's URI string.
pub const NOTEBOOK_EDITOR_SCHEME: &str = "vscode-notebook-editor";

/// notebookServiceImpl.ts and the notebook parts of mainThreadNotebookDocumentsAndEditors.ts.
#[derive(Default)]
pub struct Notebooks {
    pub models: Vec<NotebookModel>,
    pub serializers: Vec<Serializer>,
    pub kernels: kernels::Kernels,
    /// One per notebook editor tab, by the tab's model id.
    pub editors: Vec<NotebookEditor>,
    /// Notebooks the extension host opened (`BoundModelReferenceCollection`): kept while no editor shows them.
    pub held: Vec<String>,
    /// What the extension host has: notebook URI strings, editor ids, the active editor, the visible ones.
    pub sent_documents: Vec<(String, Value)>,
    pub sent_editors: Vec<String>,
    pub sent_active: Option<String>,
    pub sent_visible: Vec<String>,
}

/// A `$registerNotebookSerializer`.
pub struct Serializer {
    pub handle: u64,
    pub view_type: String,
    /// `TransientOptions`.
    pub options: Value,
    /// `INotebookContributionData`: `displayName`, `filenamePattern`, `providerDisplayName`.
    pub registration: Value,
}

/// A notebook editor (notebookEditorWidget.ts) of one tab.
pub struct NotebookEditor {
    pub tab_model_id: u32,
    pub id: String,
    pub notebook: String,
    /// The focused cell's index.
    pub focus: usize,
    /// `ICellRange`s.
    pub selections: Vec<(usize, usize)>,
}

impl Notebooks {
    pub fn model(&self, uri: &str) -> Option<&NotebookModel> {
        self.models.iter().find(|model| model.uri_string() == uri)
    }

    pub fn model_mut(&mut self, uri: &str) -> Option<&mut NotebookModel> {
        self.models.iter_mut().find(|model| model.uri_string() == uri)
    }

    /// The notebook and index of a cell's text model.
    pub fn cell(&self, cell_uri: &Value) -> Option<(&NotebookModel, usize)> {
        let (notebook, _) = parse_cell_uri(cell_uri)?;
        let model = self.model(&uri_string(&notebook))?;
        Some((model, model.index_of_cell(cell_uri)?))
    }

    pub fn serializer(&self, view_type: &str) -> Option<&Serializer> {
        self.serializers.iter().find(|serializer| serializer.view_type == view_type)
    }

    pub fn editor(&self, tab_model_id: u32) -> Option<&NotebookEditor> {
        self.editors.iter().find(|editor| editor.tab_model_id == tab_model_id)
    }
}

/// `CellKind.Markup`.
pub const MARKUP: u64 = 1;
/// `CellKind.Code`.
pub const CODE: u64 = 2;

/// `NotebookCellsChangeType`.
mod change {
    pub const MODEL_CHANGE: u64 = 1;
    pub const MOVE: u64 = 2;
    pub const CELL_LANGUAGE: u64 = 5;
    pub const CELL_METADATA: u64 = 7;
    pub const OUTPUT: u64 = 8;
    pub const OUTPUT_ITEM: u64 = 9;
    pub const CELL_CONTENT: u64 = 10;
    pub const CELL_INTERNAL_METADATA: u64 = 12;
}

/// `CellEditType`.
mod edit {
    pub const REPLACE: u64 = 1;
    pub const OUTPUT: u64 = 2;
    pub const METADATA: u64 = 3;
    pub const CELL_LANGUAGE: u64 = 4;
    pub const DOCUMENT_METADATA: u64 = 5;
    pub const MOVE: u64 = 6;
    pub const OUTPUT_ITEMS: u64 = 7;
    pub const PARTIAL_METADATA: u64 = 8;
    pub const PARTIAL_INTERNAL_METADATA: u64 = 9;
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct OutputItem {
    pub mime: String,
    #[serde(rename = "valueBytes", alias = "data", deserialize_with = "bytes")]
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Output {
    #[serde(rename = "outputId")]
    pub id: String,
    #[serde(default)]
    pub metadata: Value,
    #[serde(alias = "outputs")]
    pub items: Vec<OutputItem>,
}

/// A buffer the workbench received as bytes, or wrote itself as `{"$vsbuffer": base64}`.
fn bytes<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    match &value[exthost::rpc::VSBUFFER_KEY] {
        Value::String(encoded) => STANDARD.decode(encoded).map_err(serde::de::Error::custom),
        _ => serde_json::from_value(value).map_err(serde::de::Error::custom),
    }
}

impl Output {
    /// `NotebookDto.toNotebookOutputDto`, its bytes as buffers.
    pub fn dto(&self) -> Value {
        json!({ "outputId": self.id, "metadata": self.metadata, "items": self.items.iter().map(OutputItem::dto).collect::<Vec<_>>() })
    }
}

impl OutputItem {
    pub fn dto(&self) -> Value {
        json!({ "mime": self.mime, "valueBytes": { exthost::rpc::VSBUFFER_KEY: STANDARD.encode(&self.data) } })
    }
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub handle: u64,
    pub uri: Value,
    pub kind: u64,
    pub language: String,
    pub mime: Option<String>,
    pub metadata: Value,
    pub internal_metadata: Value,
    pub outputs: Vec<Output>,
}

/// A cell an edit created: the workbench makes its text model from `source`.
#[derive(Debug, PartialEq)]
pub struct NewCellText {
    pub uri: Value,
    pub source: String,
    pub language: String,
}

/// What `apply_edits` changed, for the workbench and the extension host.
#[derive(Debug, Default)]
pub struct Applied {
    /// The `$acceptModelChanged` event, `None` when nothing changed.
    pub event: Option<Value>,
    pub document_metadata_changed: bool,
    pub added: Vec<NewCellText>,
    pub removed: Vec<Cell>,
    /// `computeUndoRedo`: the edits that undo these, the last applied first. A replace's inverse names the cells it
    /// brings back in `restore` (their URIs) for the caller, which has their text, to fill in `cells`.
    pub undo: Vec<Value>,
}

pub struct NotebookModel {
    pub uri: Value,
    pub view_type: String,
    pub metadata: Value,
    pub cells: Vec<Cell>,
    pub version_id: u64,
    next_handle: u64,
    pub dirty: bool,
    /// The serializer's `TransientOptions`: `transientOutputs`, `transientCellMetadata`, `transientDocumentMetadata`.
    pub transient: Value,
    /// Cell edits of the notebook editor to undo and redo; the cells' text edits are on their models' stacks, all
    /// in one order by stamp (notebookTextModel.ts pushes both to the notebook's undo group).
    pub undo: Vec<CellsUndo>,
    pub redo: Vec<CellsUndo>,
}

/// A cell edit on the undo stack: the edits that revert it and the ones that apply it again.
#[derive(Clone, Debug)]
pub struct CellsUndo {
    pub stamp: u64,
    pub undo: Vec<Value>,
    pub redo: Vec<Value>,
}

impl NotebookModel {
    /// `NotebookTextModel` from `NotebookData` (a `NotebookDataDto`); the new cells' texts are returned with it.
    pub fn new(uri: Value, view_type: &str, data: &Value, transient: Value) -> (Self, Vec<NewCellText>) {
        let mut model = NotebookModel { uri, view_type: view_type.to_owned(), metadata: object_or_empty(&data["metadata"]), cells: Vec::new(), version_id: 0, next_handle: 0, dirty: false, transient, undo: Vec::new(), redo: Vec::new() };
        let cells = data["cells"].as_array().cloned().unwrap_or_default();
        let (cells, texts) = model.create_cells(&cells);
        model.cells = cells;
        (model, texts)
    }

    pub fn uri_string(&self) -> String {
        uri_string(&self.uri)
    }

    pub fn index_of_cell(&self, cell_uri: &Value) -> Option<usize> {
        self.cells.iter().position(|cell| cell.uri["fragment"] == cell_uri["fragment"])
    }

    pub fn index_of_handle(&self, handle: u64) -> Option<usize> {
        self.cells.iter().position(|cell| cell.handle == handle)
    }

    /// `ExtHostNotebook` `INotebookModelAddedData`; `text` reads a cell's text model.
    pub fn added_data(&self, text: impl Fn(&Cell) -> String) -> Value {
        json!({
            "uri": self.uri,
            "versionId": self.version_id,
            "cells": self.cells.iter().map(|cell| cell_dto(cell, &text(cell))).collect::<Vec<_>>(),
            "viewType": self.view_type,
            "metadata": self.metadata,
        })
    }

    /// `createSnapshot` as a `NotebookDataDto` without the serializer's transient parts.
    pub fn snapshot(&self, text: impl Fn(&Cell) -> String) -> Value {
        let transient_outputs = self.transient["transientOutputs"] == true;
        let cells: Vec<Value> = self
            .cells
            .iter()
            .map(|cell| {
                json!({
                    "source": text(cell),
                    "language": cell.language,
                    "mime": cell.mime,
                    "cellKind": cell.kind,
                    "outputs": if transient_outputs { Vec::new() } else { cell.outputs.iter().map(Output::dto).collect() },
                    "metadata": without_keys(&cell.metadata, &self.transient["transientCellMetadata"]),
                    "internalMetadata": cell.internal_metadata,
                })
            })
            .collect();
        let mut metadata = without_keys(&self.metadata, &self.transient["transientDocumentMetadata"]);
        // notebookServiceImpl.ts keeps it for the ipynb serializer to write the file's indentation back.
        if let Some(indent) = self.metadata["indentAmount"].as_str().filter(|indent| !indent.is_empty()) {
            metadata["indentAmount"] = json!(indent);
        }
        json!({ "metadata": metadata, "cells": cells })
    }

    /// The bytes of every output item: `createSnapshot` refuses a backup past `notebook.backup.sizeLimit`.
    pub fn output_size(&self) -> usize {
        self.cells.iter().flat_map(|cell| &cell.outputs).flat_map(|output| &output.items).map(|item| item.data.len()).sum()
    }

    /// `applyEdits` (`ICellEditOperation`s, as `ICellEditOperationDto`s): applied from the last index to the first, as
    /// `_doApplyEdits` orders them; one event carries every change and the next version id.
    pub fn apply_edits(&mut self, edits: &[Value]) -> Result<Applied, String> {
        let mut ordered = Vec::new();
        for (original, edit) in edits.iter().enumerate() {
            let kind = edit["editType"].as_u64().unwrap_or_default();
            let index = if let Some(index) = edit["index"].as_u64() {
                Some(index as usize)
            } else if let Some(handle) = edit["handle"].as_u64() {
                Some(self.index_of_handle(handle).ok_or_else(|| format!("model cannot find cell with handle {handle}"))?)
            } else if let Some(output_id) = edit["outputId"].as_str() {
                // An output cleared by an earlier edit is no error: the edit is dropped.
                match self.cells.iter().position(|cell| cell.outputs.iter().any(|output| output.id == output_id)) {
                    Some(index) => Some(index),
                    None => continue,
                }
            } else if kind == edit::DOCUMENT_METADATA {
                None
            } else {
                return Err(format!("Invalid cell edit: {edit}"));
            };
            let end = match kind {
                edit::DOCUMENT_METADATA => None,
                edit::REPLACE => Some(index.unwrap_or_default() + edit["count"].as_u64().unwrap_or_default() as usize),
                _ => index,
            };
            ordered.push((edit, index, end, original));
        }
        ordered.sort_by(|a, b| match (a.2, b.2) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, _) => std::cmp::Ordering::Less,
            (_, None) => std::cmp::Ordering::Greater,
            (Some(a_end), Some(b_end)) => b_end.cmp(&a_end).then(b.3.cmp(&a.3)),
        });
        // Edits on one index: the others in reverse, then the replaces.
        let mut runs: Vec<Vec<(&Value, Option<usize>)>> = Vec::new();
        for (edit, index, _, _) in ordered {
            match runs.last_mut().filter(|run| run[0].1 == index) {
                Some(run) => run.push((edit, index)),
                None => runs.push(vec![(edit, index)]),
            }
        }
        let mut applied = Applied::default();
        let mut events = Vec::new();
        let mut dirtying = false;
        for run in runs {
            let (replaces, mut others): (Vec<_>, Vec<_>) = run.into_iter().partition(|(edit, _)| edit["editType"] == edit::REPLACE);
            others.reverse();
            for (edit, index) in others.into_iter().chain(replaces) {
                dirtying |= self.apply_edit(edit, index, &mut events, &mut applied)?;
            }
        }
        if events.is_empty() && !applied.document_metadata_changed {
            return Ok(applied);
        }
        self.version_id += 1;
        self.dirty |= dirtying;
        applied.event = Some(json!({ "rawEvents": events, "versionId": self.version_id }));
        Ok(applied)
    }

    /// One edit; whether it makes the notebook dirty (a change the serializer keeps).
    fn apply_edit(&mut self, edit: &Value, index: Option<usize>, events: &mut Vec<Value>, applied: &mut Applied) -> Result<bool, String> {
        let kind = edit["editType"].as_u64().unwrap_or_default();
        let cell_index = |model: &Self| index.filter(|index| *index < model.cells.len()).ok_or_else(|| format!("model index out of range {index:?}"));
        match kind {
            edit::REPLACE => {
                let index = index.unwrap_or_default().min(self.cells.len());
                let count = (edit["count"].as_u64().unwrap_or_default() as usize).min(self.cells.len() - index);
                let dtos = edit["cells"].as_array().cloned().unwrap_or_default();
                if count == 0 && dtos.is_empty() {
                    return Ok(false);
                }
                let (cells, texts) = self.create_cells(&dtos);
                let dtos: Vec<Value> = cells.iter().zip(&texts).map(|(cell, text)| cell_dto(cell, &text.source)).collect();
                let added = cells.len();
                let removed: Vec<Cell> = self.cells.splice(index..index + count, cells).collect();
                let restore: Vec<Value> = removed.iter().map(|cell| cell.uri.clone()).collect();
                applied.undo.insert(0, json!({ "editType": edit::REPLACE, "index": index, "count": added, "cells": [], "restore": restore }));
                applied.removed.extend(removed);
                applied.added.extend(texts);
                events.push(json!({ "kind": change::MODEL_CHANGE, "changes": [[index, count, dtos]] }));
                Ok(true)
            }
            edit::MOVE => {
                let (index, length, new_index) = (index.unwrap_or_default(), edit["length"].as_u64().unwrap_or_default() as usize, edit["newIdx"].as_u64().unwrap_or_default() as usize);
                if index + length > self.cells.len() || new_index + length > self.cells.len() {
                    return Err(format!("model index out of range {index}"));
                }
                let moved: Vec<Cell> = self.cells.drain(index..index + length).collect();
                self.cells.splice(new_index..new_index, moved);
                applied.undo.insert(0, json!({ "editType": edit::MOVE, "index": new_index, "length": length, "newIdx": index }));
                events.push(json!({ "kind": change::MOVE, "index": index, "length": length, "newIdx": new_index }));
                Ok(true)
            }
            edit::OUTPUT => {
                let index = cell_index(self)?;
                let outputs: Vec<Output> = serde_json::from_value(edit["outputs"].clone()).map_err(|error| error.to_string())?;
                let cell = &mut self.cells[index];
                if edit["append"] == true {
                    cell.outputs.extend(outputs);
                } else if cell.outputs == outputs {
                    return Ok(false);
                } else {
                    cell.outputs = outputs;
                }
                events.push(json!({ "kind": change::OUTPUT, "index": index, "outputs": cell.outputs.iter().map(Output::dto).collect::<Vec<_>>() }));
                Ok(self.transient["transientOutputs"] != true)
            }
            edit::OUTPUT_ITEMS => {
                let index = cell_index(self)?;
                let output_id = edit["outputId"].as_str().unwrap_or_default();
                let items: Vec<OutputItem> = serde_json::from_value(edit["items"].clone()).map_err(|error| error.to_string())?;
                let append = edit["append"] == true;
                let output = self.cells[index].outputs.iter_mut().find(|output| output.id == output_id).expect("found by its id");
                match append {
                    true => {
                        output.items.extend(items.iter().cloned());
                        merge_streams(&mut output.items);
                    }
                    false => output.items = items.clone(),
                }
                events.push(json!({ "kind": change::OUTPUT_ITEM, "index": index, "outputId": output_id, "outputItems": items.iter().map(OutputItem::dto).collect::<Vec<_>>(), "append": append }));
                Ok(self.transient["transientOutputs"] != true)
            }
            edit::METADATA | edit::PARTIAL_METADATA => {
                let index = cell_index(self)?;
                let old = self.cells[index].metadata.clone();
                let metadata = match kind {
                    edit::METADATA => object_or_empty(&edit["metadata"]),
                    _ => merged(&old, &edit["metadata"]),
                };
                if metadata == old {
                    return Ok(false);
                }
                self.cells[index].metadata = metadata.clone();
                applied.undo.insert(0, json!({ "editType": edit::METADATA, "index": index, "metadata": old }));
                events.push(json!({ "kind": change::CELL_METADATA, "index": index, "metadata": metadata }));
                let transient = &self.transient["transientCellMetadata"];
                Ok(without_keys(&old, transient) != without_keys(&metadata, transient))
            }
            edit::PARTIAL_INTERNAL_METADATA => {
                let index = cell_index(self)?;
                let internal_metadata = merged(&self.cells[index].internal_metadata, &edit["internalMetadata"]);
                self.cells[index].internal_metadata = internal_metadata.clone();
                events.push(json!({ "kind": change::CELL_INTERNAL_METADATA, "index": index, "internalMetadata": internal_metadata }));
                Ok(false)
            }
            edit::CELL_LANGUAGE => {
                let index = cell_index(self)?;
                let language = edit["language"].as_str().unwrap_or_default().to_owned();
                if self.cells[index].language == language {
                    return Ok(false);
                }
                let old = std::mem::replace(&mut self.cells[index].language, language.clone());
                applied.undo.insert(0, json!({ "editType": edit::CELL_LANGUAGE, "index": index, "language": old }));
                events.push(json!({ "kind": change::CELL_LANGUAGE, "index": index, "language": language }));
                Ok(true)
            }
            edit::DOCUMENT_METADATA => {
                let metadata = object_or_empty(&edit["metadata"]);
                if metadata == self.metadata {
                    return Ok(false);
                }
                let transient = &self.transient["transientDocumentMetadata"];
                let dirtying = without_keys(&self.metadata, transient) != without_keys(&metadata, transient);
                let old = std::mem::replace(&mut self.metadata, metadata);
                applied.undo.insert(0, json!({ "editType": edit::DOCUMENT_METADATA, "metadata": old }));
                applied.document_metadata_changed = true;
                Ok(dirtying)
            }
            _ => Err(format!("Invalid cell edit: {edit}")),
        }
    }

    /// `_bindCellContentHandler` for a text change of the cell at `index`: the next version, dirty.
    pub fn cell_content_changed(&mut self, index: usize) -> Value {
        self.version_id += 1;
        self.dirty = true;
        json!({ "rawEvents": [{ "kind": change::CELL_CONTENT, "index": index }], "versionId": self.version_id })
    }

    /// `ICellDto2`s as new cells with the next handles.
    fn create_cells(&mut self, dtos: &[Value]) -> (Vec<Cell>, Vec<NewCellText>) {
        let mut cells = Vec::new();
        let mut texts = Vec::new();
        for dto in dtos {
            let handle = self.next_handle;
            self.next_handle += 1;
            let uri = cell_uri(&self.uri, handle);
            let language = dto["language"].as_str().unwrap_or_default().to_owned();
            let outputs = serde_json::from_value(dto["outputs"].clone()).unwrap_or_default();
            texts.push(NewCellText { uri: uri.clone(), source: dto["source"].as_str().unwrap_or_default().to_owned(), language: language.clone() });
            cells.push(Cell {
                handle,
                uri,
                kind: dto["cellKind"].as_u64().unwrap_or(CODE),
                language,
                mime: dto["mime"].as_str().map(str::to_owned),
                metadata: object_or_empty(&dto["metadata"]),
                internal_metadata: object_or_empty(&dto["internalMetadata"]),
                outputs,
            });
        }
        (cells, texts)
    }
}

/// `NotebookDto.toNotebookCellDto`.
fn cell_dto(cell: &Cell, text: &str) -> Value {
    let lines: Vec<&str> = text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)).collect();
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    json!({
        "handle": cell.handle,
        "uri": cell.uri,
        "eol": eol,
        "source": lines,
        "language": cell.language,
        "mime": cell.mime,
        "cellKind": cell.kind,
        "outputs": cell.outputs.iter().map(Output::dto).collect::<Vec<_>>(),
        "metadata": cell.metadata,
        "internalMetadata": cell.internal_metadata,
    })
}

/// notebookDocumentService.ts `generate`: the notebook's URI with the cell scheme and a fragment of the handle in
/// base 7 (its length as a letter first) and the notebook's scheme in URL-safe base64.
pub fn cell_uri(notebook: &Value, handle: u64) -> Value {
    const LENGTHS: [char; 10] = ['W', 'X', 'Y', 'Z', 'a', 'b', 'c', 'd', 'e', 'f'];
    let mut digits = Vec::new();
    let mut rest = handle;
    loop {
        digits.push(char::from_digit((rest % 7) as u32, 7).expect("a base 7 digit"));
        rest /= 7;
        if rest == 0 {
            break;
        }
    }
    let radix7: String = digits.into_iter().rev().collect();
    let prefix = LENGTHS.get(radix7.len() - 1).copied().unwrap_or('z');
    let scheme = URL_SAFE.encode(notebook["scheme"].as_str().unwrap_or_default());
    with_scheme_and_fragment(notebook, CELL_SCHEME, &format!("{prefix}{radix7}s{scheme}"))
}

/// `uri.with({ scheme, fragment })` of `UriComponents`; a revived URI's cached `external` and `fsPath` would
/// otherwise still name the old one.
fn with_scheme_and_fragment(uri: &Value, scheme: &str, fragment: &str) -> Value {
    json!({ "$mid": 1, "scheme": scheme, "authority": uri["authority"], "path": uri["path"], "query": uri["query"], "fragment": fragment })
}

/// notebookDocumentService.ts `parse`: the notebook URI and handle of a cell URI.
pub fn parse_cell_uri(cell: &Value) -> Option<(Value, u64)> {
    if cell["scheme"] != CELL_SCHEME {
        return None;
    }
    let fragment = cell["fragment"].as_str()?;
    let (handle, scheme) = fragment.split_once('s')?;
    let handle = u64::from_str_radix(handle.trim_start_matches(|c: char| ('W'..='Z').contains(&c) || ('a'..='f').contains(&c)), 7).ok()?;
    let scheme = String::from_utf8(URL_SAFE.decode(scheme).ok()?).ok()?;
    Some((with_scheme_and_fragment(cell, &scheme, ""), handle))
}

/// `URI.toString()` of `UriComponents`, enough to compare notebooks.
pub fn uri_string(uri: &Value) -> String {
    let part = |key: &str| uri[key].as_str().unwrap_or_default();
    let path = match part("authority") {
        "" => part("path").to_owned(),
        authority => format!("//{authority}{}", part("path")),
    };
    let mut text = crate::editor::uri_string(part("scheme"), std::path::Path::new(&path));
    if !part("query").is_empty() {
        text.push('?');
        text.push_str(part("query"));
    }
    if !part("fragment").is_empty() {
        text.push('#');
        text.push_str(part("fragment"));
    }
    text
}

/// `NotebookWorkingCopyTypeIdentifier.create` with the view type as the notebook type: a notebook's backup type id.
pub fn working_copy_type_id(view_type: &str) -> String {
    format!("notebook/{view_type}/{view_type}")
}

/// SimpleNotebookWorkingCopyEditorHandler `_getViewType`: the view type of a notebook backup's type id, `None` for
/// other type ids, a notebook type differing from its view type, and the interactive window.
pub fn view_type_of_working_copy(type_id: &str) -> Option<&str> {
    let (notebook_type, view_type) = type_id.strip_prefix("notebook/")?.split_once('/')?;
    (notebook_type == view_type && !view_type.contains('/') && view_type != "interactive").then_some(view_type)
}

/// The tab label of a notebook: its resource's base name (`EditorInput.getName`).
pub fn label(uri: &Value) -> String {
    let path = uri["path"].as_str().unwrap_or_default();
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

/// `URI.from({ scheme: untitled, path: Untitled-N<ending>, query: viewType })` for `createUntitledUri`.
pub fn untitled_uri(counter: usize, ending: &str, view_type: &str) -> Value {
    let mut uri = uri_value("untitled", std::path::Path::new(&format!("Untitled-{counter}{ending}")));
    uri["query"] = json!(view_type);
    uri
}

/// notebookCellOutputTextModel.ts `optimizeOutputItems`: an output of stream items only keeps one item per mime,
/// their bytes in order.
// shortcut: `compressOutputItemStreams`' cursor-up and carriage return folding is not ported; plain appends only.
fn merge_streams(items: &mut Vec<OutputItem>) {
    let is_stream = |item: &OutputItem| matches!(item.mime.as_str(), "application/vnd.code.notebook.stdout" | "application/vnd.code.notebook.stderr");
    if items.len() < 2 || !items.iter().all(is_stream) {
        return;
    }
    let mut merged: Vec<OutputItem> = Vec::new();
    for item in items.drain(..) {
        match merged.iter_mut().find(|existing| existing.mime == item.mime) {
            Some(existing) => existing.data.extend(item.data),
            None => merged.push(item),
        }
    }
    *items = merged;
}

fn object_or_empty(value: &Value) -> Value {
    match value {
        Value::Object(_) => value.clone(),
        _ => json!({}),
    }
}

/// A partial metadata edit: each key set, a `null` one removed.
fn merged(old: &Value, partial: &Value) -> Value {
    let mut map = old.as_object().cloned().unwrap_or_default();
    for (key, value) in partial.as_object().into_iter().flatten() {
        match value {
            Value::Null => _ = map.remove(key),
            value => _ = map.insert(key.clone(), value.clone()),
        }
    }
    Value::Object(map)
}

/// The metadata without the keys a `TransientMetadata` (`{ key: true }`) marks.
fn without_keys(metadata: &Value, transient: &Value) -> Value {
    let map: Map<String, Value> = metadata.as_object().into_iter().flatten().filter(|(key, _)| transient[key.as_str()] != true).map(|(key, value)| (key.clone(), value.clone())).collect();
    Value::Object(map)
}

#[cfg(test)]
mod tests;
