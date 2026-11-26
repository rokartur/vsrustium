//! editorPart.ts `saveState` and `doCreateGridControlWithPreviousState`: the grid and each group's file editors in
//! the workspace memento `memento/workbench.parts.editor` under `editorpart.state`, in upstream's shape
//! (`ISerializedGrid` of `ISerializedEditorGroupModel` leaves, file editors as `workbench.editors.files.fileEditorInput`).
//! Upstream saves on shutdown; the port saves on each editor change, as the editor history does.
// Not ported yet: editors other than file, untitled text, notebook, interactive and REPL editors (upstream
// serializes every editor with a serializer), sticky editors, locked groups.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::super::Workbench;
use super::super::notebooks::{NotebookInput, NotebookRestore};
use crate::editor::MODAL_GROUP;
use crate::editor_grid::{EditorGrid, Orientation, SerializedNode};

const MEMENTO_KEY: &str = "memento/workbench.parts.editor";
const STATE_KEY: &str = "editorpart.state";
const FILE_EDITOR_INPUT: &str = "workbench.editors.files.fileEditorInput";
/// untitledTextEditorInput.ts `UntitledTextEditorInput.ID`.
const UNTITLED_EDITOR_INPUT: &str = "workbench.editors.untitledEditorInput";

impl Workbench {
    pub(in crate::workbench) fn save_editor_part_state(&mut self) {
        let state = self.editor_part_state();
        let mut storage = self.workspace_storage.lock().unwrap();
        let mut memento: Value = storage.get(MEMENTO_KEY).and_then(|stored| serde_json::from_str(stored).ok()).unwrap_or_else(|| json!({}));
        if memento[STATE_KEY] == state {
            return;
        }
        memento[STATE_KEY] = state;
        storage.set(MEMENTO_KEY, memento.to_string());
    }

    fn editor_part_state(&self) -> Value {
        let groups: Vec<u32> = self.groups_in_grid_order();
        let (root, orientation, width, height) = match &self.editor_groups.grid {
            Some(grid) => grid.serialize(),
            None => {
                let size = self.editor_groups.part_bounds.get().size;
                (SerializedNode::Branch { children: vec![SerializedNode::Leaf { group: groups[0], size: size.height }], size: size.width }, Orientation::Vertical, size.width, size.height)
            }
        };
        let mru: Vec<u32> = self.editors.group_ids_mru().into_iter().filter(|group| *group != MODAL_GROUP).collect();
        json!({
            "serializedGrid": { "root": self.serialized_node(&root), "orientation": orientation_number(orientation), "width": width, "height": height },
            "activeGroup": mru[0],
            "mostRecentActiveGroups": mru,
        })
    }

    fn serialized_node(&self, node: &SerializedNode) -> Value {
        match node {
            SerializedNode::Leaf { group, size } => json!({ "type": "leaf", "data": self.serialized_group(*group), "size": size }),
            SerializedNode::Branch { children, size } => json!({ "type": "branch", "data": children.iter().map(|child| self.serialized_node(child)).collect::<Vec<_>>(), "size": size }),
        }
    }

    /// `EditorGroupModel.serialize`: the file editors and, with hot exit on (untitledTextEditorHandler.ts
    /// `canSerialize`), the untitled ones in tab order, their MRU and the preview editor by index.
    fn serialized_group(&self, group: u32) -> Value {
        let untitled = self.setting("files.hotExit") != "off";
        let editors: Vec<(_, Option<Value>)> = self
            .editors
            .group_tabs(group)
            .0
            .iter()
            .map(|editor| (editor, self.notebook_editor_input(editor)))
            .filter(|(editor, notebook)| notebook.is_some() || editor.document().scheme == "file" || untitled && editor.document().scheme == "untitled")
            .collect();
        let serialized: Vec<Value> = editors.iter().map(|(editor, notebook)| {
            let document = editor.document();
            if let Some(notebook) = notebook {
                return notebook.clone();
            }
            match document.scheme.as_str() {
                "file" => json!({ "id": FILE_EDITOR_INPUT, "value": json!({ "resourceJSON": exthost::scan::file_uri(&document.path) }).to_string() }),
                _ => {
                    let resource = json!({ "$mid": 1, "scheme": "untitled", "path": document.path.to_string_lossy() });
                    json!({ "id": UNTITLED_EDITOR_INPUT, "value": json!({ "resourceJSON": resource, "modeId": document.language_id }).to_string() })
                }
            }
        }).collect();
        let by_recency = self.editors.group_editors_by_recency(group);
        let mru: Vec<usize> = by_recency.iter().filter_map(|recent| editors.iter().position(|(editor, _)| editor.model_id == recent.model_id)).collect();
        let preview = editors.iter().position(|(editor, _)| editor.preview);
        json!({ "id": group, "editors": serialized, "mru": mru, "preview": preview })
    }

    /// The stored grid and its groups' file editors, in place of the one empty group a window starts with.
    pub(in crate::workbench) fn restore_editor_part_state(&mut self) {
        let stored = self.workspace_storage.lock().unwrap().get(MEMENTO_KEY).and_then(|stored| serde_json::from_str::<Value>(stored).ok());
        let Some(state) = stored.map(|memento| memento[STATE_KEY].clone()).filter(|state| !state["serializedGrid"].is_null()) else { return };
        if self.editors.has_several_groups() || !self.editors.editors.is_empty() {
            return;
        }
        let grid = &state["serializedGrid"];
        let mut leaves = Vec::new();
        let root = parse_node(&grid["root"], &mut leaves);
        // Group ids are the port's: the first leaf takes the window's group, the others new ones.
        let mut ids = Vec::new();
        for (index, _) in leaves.iter().enumerate() {
            ids.push(if index == 0 { self.editors.id } else { self.editors.add_group(false) });
        }
        let root = remap(root, &ids);
        for (leaf, id) in leaves.iter().zip(&ids) {
            self.activate_group(*id);
            self.restore_group(leaf);
        }
        if ids.len() > 1 {
            let orientation = if grid["orientation"] == 1 { Orientation::Horizontal } else { Orientation::Vertical };
            let (width, height) = (grid["width"].as_f64().unwrap_or(0.) as f32, grid["height"].as_f64().unwrap_or(0.) as f32);
            self.editor_groups.grid = Some(EditorGrid::deserialize(root, orientation, width, height));
        }
        let stored_mru: Vec<u32> = state["mostRecentActiveGroups"].as_array().into_iter().flatten().filter_map(|group| group.as_u64()).map(|group| group as u32).collect();
        for stored in stored_mru.iter().rev() {
            if let Some(index) = leaves.iter().position(|leaf| leaf["id"].as_u64() == Some(*stored as u64)) {
                self.activate_group(ids[index]);
            }
        }
        self.editors.columns = if ids.len() > 1 { self.groups_in_grid_order() } else { Vec::new() };
        self.host.send_changes(&self.editors);
    }

    fn restore_group(&mut self, group: &Value) {
        let mut opened = Vec::new();
        let active = group["mru"][0].as_u64();
        for (index, editor) in group["editors"].as_array().into_iter().flatten().enumerate() {
            let value: Value = editor["value"].as_str().and_then(|value| serde_json::from_str(value).ok()).unwrap_or_default();
            // Notebook, interactive and REPL editors resolve their notebook first (notebooks.rs).
            if let Some(input) = NotebookInput::parse(editor["id"].as_str().unwrap_or_default(), &value) {
                let preview = group["preview"].as_u64() == Some(index as u64);
                self.notebook_views.restores.push(NotebookRestore { group: self.editors.id, index, preview, active: active == Some(index as u64), input });
                continue;
            }
            let Some(path) = value["resourceJSON"]["path"].as_str().map(PathBuf::from) else { continue };
            let document = match editor["id"].as_str() {
                // A file deleted since is left out, as `FileEditorInput` would show its error placeholder.
                Some(FILE_EDITOR_INPUT) => match self.read_document(&path) {
                    Ok(document) => document,
                    Err(_) => continue,
                },
                // Its text comes from its backup (backup_tracker.rs `restore_backups`).
                Some(UNTITLED_EDITOR_INPUT) => crate::editor::TextDocument::untitled(&path, "", value["modeId"].as_str(), &self.languages, &self.settings.lock().unwrap()),
                _ => continue,
            };
            self.editors.open(document, group["preview"].as_u64() == Some(index as u64));
            opened.push(index);
        }
        let mru = group["mru"].as_array().into_iter().flatten().filter_map(|index| index.as_u64());
        let mru: Vec<usize> = mru.filter_map(|index| opened.iter().position(|opened| *opened as u64 == index)).collect();
        for index in mru.into_iter().rev() {
            self.editors.activate(index);
        }
    }
}

fn orientation_number(orientation: Orientation) -> u8 {
    match orientation {
        Orientation::Vertical => 0,
        Orientation::Horizontal => 1,
    }
}

/// sash.ts `Orientation`: VERTICAL 0, HORIZONTAL 1.
/// The node with its leaves numbered in order; `leaves` gets each leaf's group data.
fn parse_node(node: &Value, leaves: &mut Vec<Value>) -> SerializedNode {
    let size = node["size"].as_f64().unwrap_or(0.) as f32;
    match node["type"].as_str() {
        Some("branch") => SerializedNode::Branch { children: node["data"].as_array().into_iter().flatten().map(|child| parse_node(child, leaves)).collect(), size },
        _ => {
            leaves.push(node["data"].clone());
            SerializedNode::Leaf { group: leaves.len() as u32 - 1, size }
        }
    }
}

fn remap(node: SerializedNode, ids: &[u32]) -> SerializedNode {
    match node {
        SerializedNode::Leaf { group, size } => SerializedNode::Leaf { group: ids[group as usize], size },
        SerializedNode::Branch { children, size } => SerializedNode::Branch { children: children.into_iter().map(|child| remap(child, ids)).collect(), size },
    }
}
