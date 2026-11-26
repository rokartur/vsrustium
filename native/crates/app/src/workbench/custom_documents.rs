//! mainThreadCustomEditors.ts `MainThreadCustomEditorModel`: the working copy of a custom document. An editable
//! document keeps the edit ids the extension reports for undo and redo, is dirty from an unsaved edit, a content
//! change or a restored backup, saves, saves as, reverts and backs up through the extension host. Its webview
//! editors show the dirty state through their placeholder model (`custom_document_dirty`).

use std::path::{Path, PathBuf};

use base64::Engine;
use futures::channel::oneshot;
use gpui::Context;
use serde_json::{Value, json};

use super::Workbench;
use super::host_requests::uri_parts;
use super::webview_panels::WEBVIEW_PANEL_SCHEME;
use crate::backups;
use crate::commands::Outcome;
use crate::uri::Uri;

/// `Schemas.vscodeCustomEditor`, the scheme of a custom document's working copy.
pub(super) const CUSTOM_EDITOR_SCHEME: &str = "vscode-custom-editor";

pub(in crate::workbench) struct CustomDocument {
    pub(super) resource: Value,
    pub(super) view_type: String,
    editable: bool,
    edits: Vec<i64>,
    current_edit_index: isize,
    save_point: isize,
    dirty_from_content_change: bool,
    from_backup: bool,
    /// One more per change: the version a backup is for.
    pub(super) version: u64,
    /// One more per save started: a save another one overtook does not move the save point (`_ongoingSave`).
    save_generation: u64,
}

impl CustomDocument {
    pub(super) fn is_dirty(&self) -> bool {
        if self.dirty_from_content_change {
            return true;
        }
        if !self.edits.is_empty() {
            return self.save_point != self.current_edit_index;
        }
        self.from_backup
    }

    /// `spliceEdits`: the edits past the current one go, `insert` takes their place; the removed ids.
    fn splice_edits(&mut self, insert: Option<i64>) -> Vec<i64> {
        let start = (self.current_edit_index + 1) as usize;
        let removed = self.edits.split_off(start.min(self.edits.len()));
        self.edits.extend(insert);
        removed
    }

    fn mark_saved(&mut self) {
        self.dirty_from_content_change = false;
        self.save_point = self.current_edit_index;
        self.from_backup = false;
    }
}

impl Workbench {
    /// `MainThreadCustomEditorModel.create` once `$createCustomDocument` answered.
    pub(super) fn custom_document_created(&mut self, resource: Value, view_type: String, editable: bool, from_backup: bool, cx: &mut Context<Self>) {
        let document = CustomDocument {
            resource,
            view_type,
            editable,
            edits: Vec::new(),
            current_edit_index: -1,
            save_point: -1,
            dirty_from_content_change: false,
            from_backup,
            version: 0,
            save_generation: 0,
        };
        let documents = &mut self.webview_panels.custom_editors.documents;
        documents.retain(|open| !(open.resource == document.resource && open.view_type == document.view_type));
        documents.push(document);
        let index = documents.len() - 1;
        self.sync_custom_document_dirty(index, cx);
    }

    /// `$disposeCustomDocument`: the document's edits go with it (`undoService.removeElements`).
    pub(super) fn custom_document_disposed(&mut self, resource: &Value, view_type: &str) {
        self.webview_panels.custom_editors.documents.retain(|document| !(document.resource == *resource && document.view_type == view_type));
    }

    fn custom_document_index(&self, resource: &Value, view_type: &str) -> Option<usize> {
        self.webview_panels.custom_editors.documents.iter().position(|document| document.resource == *resource && document.view_type == view_type)
    }

    /// The document a custom editor's placeholder model shows.
    fn custom_document_of(&self, path: &Path) -> Option<usize> {
        let panel = self.webview_panels.panels.iter().find(|panel| panel.scheme == WEBVIEW_PANEL_SCHEME && panel.path == path)?;
        self.custom_document_index(&panel.custom.as_ref()?.resource, &panel.view_type)
    }

    /// `change`: a change that flips the dirty state reaches the document's editors and the host's tabs.
    fn change_custom_document(&mut self, index: usize, change: impl FnOnce(&mut CustomDocument), cx: &mut Context<Self>) {
        let document = &mut self.webview_panels.custom_editors.documents[index];
        let was_dirty = document.is_dirty();
        change(document);
        document.version += 1;
        if document.is_dirty() != was_dirty {
            self.sync_custom_document_dirty(index, cx);
        }
    }

    fn sync_custom_document_dirty(&mut self, index: usize, cx: &mut Context<Self>) {
        let document = &self.webview_panels.custom_editors.documents[index];
        let dirty = document.is_dirty();
        let shows = |panel: &&super::webview_panels::WebviewPanel| {
            panel.scheme == WEBVIEW_PANEL_SCHEME && panel.view_type == document.view_type && panel.custom.as_ref().is_some_and(|custom| custom.resource == document.resource)
        };
        let paths: Vec<PathBuf> = self.webview_panels.panels.iter().filter(shows).map(|panel| panel.path.clone()).collect();
        for path in paths {
            if let Some(model) = self.editors.model_mut(WEBVIEW_PANEL_SCHEME, &path) {
                model.text_model_mut().custom_document_dirty = dirty;
            }
        }
        self.host.send_tabs(&self.editors);
        cx.notify();
    }

    /// `$onDidEdit`: `pushEdit`, the edit an undo element of the resource.
    pub(super) fn push_custom_document_edit(&mut self, resource: &Value, view_type: &str, edit_id: i64, cx: &mut Context<Self>) {
        let Some(index) = self.custom_document_index(resource, view_type).filter(|index| self.webview_panels.custom_editors.documents[*index].editable) else { return };
        let mut removed = Vec::new();
        self.change_custom_document(
            index,
            |document| {
                removed = document.splice_edits(Some(edit_id));
                document.current_edit_index = document.edits.len() as isize - 1;
            },
            cx,
        );
        self.dispose_custom_document_edits(index, removed);
    }

    /// `$onContentChange`: dirty without an undo element.
    pub(super) fn change_custom_document_content(&mut self, resource: &Value, view_type: &str, cx: &mut Context<Self>) {
        let Some(index) = self.custom_document_index(resource, view_type) else { return };
        self.change_custom_document(index, |document| document.dirty_from_content_change = true, cx);
    }

    fn dispose_custom_document_edits(&mut self, index: usize, removed: Vec<i64>) {
        if removed.is_empty() {
            return;
        }
        let document = &self.webview_panels.custom_editors.documents[index];
        self.host.send("ExtHostCustomEditors", "$disposeEdits", vec![document.resource.clone(), json!(document.view_type), json!(removed)]);
    }

    /// The `undo` and `redo` of the document's current edit, answered once the extension applied it.
    pub(super) fn undo_custom_document(&mut self, path: &Path, redo: bool, cx: &mut Context<Self>) -> Outcome {
        let Some(index) = self.custom_document_of(path) else { return Outcome::done() };
        let document = &self.webview_panels.custom_editors.documents[index];
        let edit = match redo {
            false if document.current_edit_index >= 0 => document.edits[document.current_edit_index as usize],
            true if document.current_edit_index < document.edits.len() as isize - 1 => document.edits[(document.current_edit_index + 1) as usize],
            _ => return Outcome::done(),
        };
        if !document.editable {
            return Outcome::done();
        }
        self.change_custom_document(index, |document| document.current_edit_index += if redo { 1 } else { -1 }, cx);
        let document = &self.webview_panels.custom_editors.documents[index];
        let method = if redo { "$redo" } else { "$undo" };
        Outcome::Pending(self.host.call("ExtHostCustomEditors", method, vec![document.resource.clone(), json!(document.view_type), json!(edit), json!(document.is_dirty())]))
    }

    /// `saveCustomEditor` from the editor's placeholder model: `$onSave`, then the save point; an untitled
    /// document saves as the file the save dialog picks.
    pub(super) fn save_custom_document(&mut self, path: &Path, cx: &mut Context<Self>) -> oneshot::Receiver<bool> {
        let (sender, receiver) = oneshot::channel();
        let Some(index) = self.custom_document_of(path).filter(|index| self.webview_panels.custom_editors.documents[*index].editable) else {
            let _ = sender.send(false);
            return receiver;
        };
        let document = &mut self.webview_panels.custom_editors.documents[index];
        if uri_parts(&document.resource).is_ok_and(|(scheme, _)| scheme == "untitled") {
            return self.save_custom_document_as(path, cx);
        }
        document.save_generation += 1;
        let (generation, resource, view_type) = (document.save_generation, document.resource.clone(), document.view_type.clone());
        let saved = self.host.call_cancellable("ExtHostCustomEditors", "$onSave", vec![resource.clone(), json!(view_type)]);
        cx.spawn(async move |workbench, cx| {
            let saved = matches!(saved.await, Ok(Ok(_)));
            let _ = workbench.update(cx, |workbench, cx| {
                let Some(index) = workbench.custom_document_index(&resource, &view_type) else { return };
                if saved && workbench.webview_panels.custom_editors.documents[index].save_generation == generation {
                    workbench.change_custom_document(index, CustomDocument::mark_saved, cx);
                }
            });
            let _ = sender.send(saved);
        })
        .detach();
        receiver
    }

    /// `CustomEditorInput.saveAs`: the save dialog, `$onSaveAs` to the target, and the editor reopened on it
    /// (`rename`).
    pub(super) fn save_custom_document_as(&mut self, path: &Path, cx: &mut Context<Self>) -> oneshot::Receiver<bool> {
        let (sender, receiver) = oneshot::channel();
        let Some(index) = self.custom_document_of(path) else {
            let _ = sender.send(false);
            return receiver;
        };
        let document = &self.webview_panels.custom_editors.documents[index];
        let (resource, view_type, editable) = (document.resource.clone(), document.view_type.clone(), document.editable);
        let source = uri_parts(&resource).map(|(_, path)| path).unwrap_or_default();
        let picked = self.pick_file_to_save(source.clone(), cx);
        let placeholder = path.to_path_buf();
        cx.spawn(async move |workbench, cx| {
            let Ok(Some(target)) = picked.await else {
                let _ = sender.send(false);
                return;
            };
            let target_uri = Uri::new("file", "", &target.to_string_lossy(), "", "").map(|uri| uri.to_json()).unwrap_or_default();
            let saved = match editable {
                true => {
                    let call = workbench.update(cx, |workbench, _| workbench.host.call_cancellable("ExtHostCustomEditors", "$onSaveAs", vec![resource.clone(), json!(view_type), target_uri.clone()]));
                    match call {
                        Ok(call) => matches!(call.await, Ok(Ok(_))),
                        Err(_) => false,
                    }
                }
                // A read-only document's file is copied over.
                false => std::fs::copy(&source, &target).is_ok(),
            };
            let _ = workbench.update(cx, |workbench, cx| {
                if saved && let Some(index) = workbench.custom_document_index(&resource, &view_type) {
                    workbench.change_custom_document(index, CustomDocument::mark_saved, cx);
                }
                if saved {
                    workbench.retarget_custom_editor(&placeholder, target_uri, cx);
                }
            });
            let _ = sender.send(saved);
        })
        .detach();
        receiver
    }

    /// `revert`: `$revert` unless nothing changed since the save point, then back to it.
    pub(super) fn revert_custom_document(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(index) = self.custom_document_of(path) else { return };
        let document = &self.webview_panels.custom_editors.documents[index];
        if !document.editable || (document.current_edit_index == document.save_point && !document.dirty_from_content_change && !document.from_backup) {
            return;
        }
        drop(self.host.call_cancellable("ExtHostCustomEditors", "$revert", vec![document.resource.clone(), json!(document.view_type)]));
        let mut removed = Vec::new();
        self.change_custom_document(
            index,
            |document| {
                document.dirty_from_content_change = false;
                document.from_backup = false;
                document.current_edit_index = document.save_point;
                removed = document.splice_edits(None);
            },
            cx,
        );
        self.dispose_custom_document_edits(index, removed);
    }

    /// `backup`: `$backup` answers the backup id the meta keeps; the backup file holds no content. Answers the
    /// document's version the backup is for, None when it failed.
    pub(super) fn backup_custom_document(&mut self, index: usize, cx: &mut Context<Self>) -> oneshot::Receiver<Option<u64>> {
        let (sender, receiver) = oneshot::channel();
        let document = &self.webview_panels.custom_editors.documents[index];
        let (resource, view_type, version) = (document.resource.clone(), document.view_type.clone(), document.version);
        let shows = |panel: &&super::webview_panels::WebviewPanel| panel.view_type == view_type && panel.custom.as_ref().is_some_and(|custom| custom.resource == resource);
        let (Some(home), Some(panel)) = (self.lifecycle.backups.home.clone(), self.webview_panels.panels.iter().find(shows)) else {
            let _ = sender.send(None);
            return receiver;
        };
        let mut meta = json!({
            "viewType": view_type,
            "editorResource": resource,
            "customTitle": panel.title,
            "backupId": "",
            "webview": { "origin": panel.origin, "options": panel.content_options, "state": panel.state },
        });
        if let Some((extension, _)) = self.custom_editor_provider(&view_type) {
            meta["extension"] = json!({ "id": panel.extension_id, "location": extension["location"] });
        }
        let backup = self.host.call_cancellable("ExtHostCustomEditors", "$backup", vec![resource.clone(), json!(view_type)]);
        cx.background_executor()
            .spawn(async move {
                let Ok(Ok(Value::String(backup_id))) = backup.await else {
                    let _ = sender.send(None);
                    return;
                };
                meta["backupId"] = json!(backup_id);
                let written = working_copy_uri(&resource, &view_type).and_then(|uri| backups::write(&home, &uri, Some(&meta), "").map_err(|error| error.to_string()));
                if let Err(error) = &written {
                    eprintln!("[backup tracker] backing up {view_type} document: {error}");
                }
                let _ = sender.send(written.ok().map(|()| version));
            })
            .detach();
        receiver
    }

    pub(super) fn discard_custom_document_backup(&self, resource: &Value, view_type: &str) {
        if let (Some(home), Ok(uri)) = (&self.lifecycle.backups.home, working_copy_uri(resource, view_type)) {
            backups::discard(home, &uri);
        }
    }

    /// `rename` after a save as: the editor shows a new document of `target` in a new webview, as reopening it
    /// does, and the old document is released.
    fn retarget_custom_editor(&mut self, placeholder: &Path, target: Value, cx: &mut Context<Self>) {
        let handle = self.next_custom_editor_handle();
        let Some(panel) = self.webview_panels.panels.iter_mut().find(|panel| panel.scheme == WEBVIEW_PANEL_SCHEME && panel.path == placeholder) else { return };
        let Some(custom) = panel.custom.as_mut() else { return };
        let old_resource = std::mem::replace(&mut custom.resource, target.clone());
        custom.resolved = false;
        custom.backup_id = None;
        let old_handle = std::mem::replace(&mut panel.handle, handle);
        panel.title = uri_parts(&target).ok().and_then(|(_, path)| path.file_name().map(|name| name.to_string_lossy().into_owned())).unwrap_or_default();
        panel.webview = None;
        panel.html.clear();
        panel.state = Value::Null;
        let view_type = panel.view_type.clone();
        self.host.send("ExtHostWebviewPanels", "$onDidDisposeWebviewPanel", vec![json!(old_handle)]);
        let still_open = self.webview_panels.panels.iter().any(|panel| panel.view_type == view_type && panel.custom.as_ref().is_some_and(|custom| custom.resource == old_resource));
        if !still_open {
            self.custom_document_disposed(&old_resource, &view_type);
            self.host.send("ExtHostCustomEditors", "$disposeCustomDocument", vec![old_resource, json!(view_type)]);
        }
        self.resolve_custom_editors(cx);
        cx.notify();
    }

    /// `ComplexCustomWorkingCopyEditorHandler.createEditor`: the backed up document opens pinned in the background
    /// in its custom editor, its document created from the backup id once the provider registers, its webview with
    /// the backed up state.
    pub(super) fn restore_custom_document(&mut self, meta: &Value) {
        let (Some(view_type), Some(backup_id)) = (meta["viewType"].as_str(), meta["backupId"].as_str()) else { return };
        let resource = meta["editorResource"].clone();
        if self.webview_panels.panels.iter().any(|panel| panel.view_type == view_type && panel.custom.as_ref().is_some_and(|custom| custom.resource == resource)) {
            return;
        }
        drop(self.host.activate_by_event(&format!("onCustomEditor:{view_type}")));
        let retain = self.custom_editor_provider(view_type).is_some_and(|(_, retain)| retain);
        let title = meta["customTitle"].as_str().unwrap_or_default().to_owned();
        let custom = Some(super::custom_editors::CustomEditor { resource, resolved: false, backup_id: Some(backup_id.to_owned()) });
        let handle = self.add_custom_document_editor(view_type, title, &meta["extension"], retain, custom, false);
        if let Some(panel) = self.webview_panel_mut(&handle) {
            panel.state = meta["webview"]["state"].clone();
        }
    }
}

/// `toWorkingCopyResource`: `vscode-custom-editor://<view type>/<base64 of the URI>?<the URI as JSON>`.
pub(super) fn working_copy_uri(resource: &Value, view_type: &str) -> Result<Uri, String> {
    let resource = Uri::from_json(resource)?;
    let authority: String = view_type.chars().map(|char| if char.is_ascii_alphanumeric() || char == '-' || char == '_' { char } else { '-' }).collect();
    let text = resource.with(|uri| {
        uri.query.clear();
        uri.fragment.clear();
    })?;
    let utf16: Vec<u8> = text.to_string(true).encode_utf16().flat_map(u16::to_le_bytes).collect();
    let path = format!("/{}", base64::engine::general_purpose::STANDARD.encode(utf16));
    let mut json = serde_json::Map::new();
    json.insert("$mid".into(), json!(1));
    for (key, value) in [("path", &resource.path), ("scheme", &resource.scheme), ("authority", &resource.authority), ("query", &resource.query), ("fragment", &resource.fragment)] {
        if !value.is_empty() {
            json.insert(key.into(), json!(value));
        }
    }
    Uri::new(CUSTOM_EDITOR_SCHEME, &authority, &path, &Value::Object(json).to_string(), "")
}

#[cfg(test)]
mod tests;
