//! The working copy backup tracker (workingCopyBackupTracker.ts, browser and electron-browser): dirty text files and
//! untitled editors are backed up a second after each change, the backup goes once they are saved or reverted, a
//! shutdown backs them all up for hot exit (`files.hotExit`), and the backups of the last session open as dirty
//! editors in the background.

use std::collections::HashMap;
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::channel::oneshot;
use gpui::{Context, Task, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::custom_documents::CUSTOM_EDITOR_SCHEME;
use super::lifecycle::ShutdownReason;
use crate::backups;
use crate::editor::{OpenEditor, TextDocument};
use crate::uri::Uri;

/// `BACKUP_SCHEDULE_DELAYS[AutoSaveMode.OFF]`.
const BACKUP_DELAY: Duration = Duration::from_millis(1000);

#[derive(Default)]
pub struct BackupTracker {
    /// `environmentService.backupPath`: `Backups/<workspace id>`; none while developing an extension.
    pub(super) home: Option<PathBuf>,
    /// The alternative version each backed up model had when its backup was written.
    backed_up: HashMap<(String, PathBuf), u32>,
    /// The pending backup of each model and the version it is for.
    pending: HashMap<(String, PathBuf), (u32, Task<()>)>,
    /// `suspendBackupOperations` while the shutdown handles modified working copies.
    suspended: bool,
    custom: DocumentBackups,
    notebooks: DocumentBackups,
    /// `pendingOperations` of the backup service: each backup being written holds its sender until it is done.
    operations: Vec<oneshot::Receiver<()>>,
}

/// The backups of one kind of document backed up through the extension host, by its resource as JSON and view type.
#[derive(Default)]
struct DocumentBackups {
    /// The version each backed up document had.
    backed_up: HashMap<(String, String), u64>,
    pending: HashMap<(String, String), (u64, Task<()>)>,
}

#[derive(Clone, Copy)]
enum DocumentKind {
    Custom,
    Notebook,
}

impl BackupTracker {
    fn documents(&mut self, kind: DocumentKind) -> &mut DocumentBackups {
        match kind {
            DocumentKind::Custom => &mut self.custom,
            DocumentKind::Notebook => &mut self.notebooks,
        }
    }

    /// A backup starts writing: the sender to drop once it is written or failed.
    pub(super) fn start_operation(&mut self) -> oneshot::Sender<()> {
        self.operations.retain_mut(|operation| matches!(operation.try_recv(), Ok(None)));
        let (done, operation) = oneshot::channel();
        self.operations.push(operation);
        done
    }

    /// `joinBackups`: the backups being written now.
    pub(super) fn take_operations(&mut self) -> Vec<oneshot::Receiver<()>> {
        std::mem::take(&mut self.operations)
    }
}

/// What the window's backup folder is: its workspace, its folder, or an empty window's folder.
enum BackupWorkspace<'a> {
    Workspace { id: &'a str, config: &'a Path },
    Folder { id: &'a str, folder: &'a Path },
    /// An empty window takes its restored backup folder, else the first empty window backup of the last session,
    /// else a new folder.
    Empty(Option<String>),
}

/// `registerWorkspaceBackup`, `registerFolderBackup` and `registerEmptyWindowBackup`: the backup folder of the
/// window, registered in `Backups/workspaces.json`.
fn backup_home(user_data: &Path, workspace: BackupWorkspace) -> PathBuf {
    let (key, entry, folder) = match workspace {
        BackupWorkspace::Workspace { id, config } => ("rootURIWorkspaces", json!({ "workspace": { "id": id, "configURIPath": file_uri(config) } }), id.to_owned()),
        BackupWorkspace::Folder { id, folder } => ("folders", json!({ "folderUri": file_uri(folder) }), id.to_owned()),
        BackupWorkspace::Empty(restored) => {
            // `createEmptyWorkspaceIdentifier`: `Date.now()` plus up to a second at random.
            let new_id = || {
                let millis = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis());
                (millis + u128::from(rand_below_1000())).to_string()
            };
            let folder = restored.or_else(|| backups::empty_window_backups(user_data).into_iter().next()).unwrap_or_else(new_id);
            ("emptyWindows", json!({ "backupFolder": folder }), folder)
        }
    };
    backups::register(user_data, key, entry);
    user_data.join("Backups").join(folder)
}

fn rand_below_1000() -> u32 {
    std::collections::hash_map::RandomState::new().hash_one(std::process::id()) as u32 % 1000
}

fn file_uri(path: &Path) -> String {
    Uri::new("file", "", &path.to_string_lossy(), "", "").map_or_else(|_| String::new(), |uri| uri.to_string(false))
}

impl Workbench {
    /// Starts tracking with the window's backup folder in `user_data` (none while developing an extension, as
    /// `doOpenInBrowserWindow` registers none) and opens the backups the last session left in it.
    pub fn start_backups(&mut self, user_data: &Path, restored_folder: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if self.extension_development {
            return;
        }
        let id = self.workspace().map(exthost::session::workspace_id);
        let backup_workspace = match (self.workspace(), &id) {
            (Some(workspace), Some(id)) => match &workspace.file {
                Some(config) => BackupWorkspace::Workspace { id, config },
                None => BackupWorkspace::Folder { id, folder: &workspace.folders[0].path },
            },
            _ => BackupWorkspace::Empty(restored_folder.and_then(|folder| folder.file_name().map(|name| name.to_string_lossy().into_owned()))),
        };
        self.lifecycle.backups.home = Some(backup_home(user_data, backup_workspace));
        self.restore_backups(window, cx);
    }

    /// `restoreBackups`: each backup without an open editor opens pinned in the background, its model holding the
    /// backed up text; an editor the editor part restored takes its backup's text (the model resolves from it).
    fn restore_backups(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(home) = self.lifecycle.backups.home.clone() else { return };
        let active = self.editors.active_index().map(|index| self.editors.editors[index].model_id);
        let mut restored = false;
        for path in backups::list(&home) {
            let Some(backup) = backups::resolve(&path) else { continue };
            if let Some(view_type) = crate::notebook::view_type_of_working_copy(&backup.type_id) {
                self.restore_notebook_backup(backup.resource.to_json(), view_type.to_owned(), window, cx);
                continue;
            }
            if crate::notebook::repl_notebook_type_of_working_copy(&backup.type_id).is_some() {
                self.restore_repl_backup(backup.resource.to_json(), window, cx);
                continue;
            }
            if !backup.type_id.is_empty() {
                continue;
            }
            if backup.resource.scheme == CUSTOM_EDITOR_SCHEME {
                if let Some(meta) = &backup.meta {
                    self.restore_custom_document(meta);
                    restored = true;
                }
                continue;
            }
            let scheme = backup.resource.scheme.clone();
            let model_path = PathBuf::from(&backup.resource.path);
            match self.editors.model(&scheme, &model_path) {
                Some(model) if model.is_dirty() => continue,
                Some(_) => {}
                None => {
                    let document = match scheme.as_str() {
                        "file" => match self.read_document(&model_path) {
                            Ok(document) => document,
                            // shortcut: a backup of a file that is gone stays on disk; upstream opens it as an orphaned model.
                            Err(error) => {
                                eprintln!("[backup tracker] restoring {}: {error}", model_path.display());
                                continue;
                            }
                        },
                        "untitled" => TextDocument::untitled(&model_path, "", None, &self.languages, &self.settings.lock().unwrap()),
                        _ => continue,
                    };
                    self.editors.open(document, false);
                }
            }
            self.update_model(&scheme, &model_path, &String::from_utf8_lossy(&backup.content), false);
            let version = self.editors.model(&scheme, &model_path).expect("just opened").alternative_version_id();
            self.lifecycle.backups.backed_up.insert((scheme, model_path), version);
            restored = true;
        }
        if !restored {
            return;
        }
        // `inactive: true`: the editor that was active stays so.
        if let Some(index) = active.and_then(|id| self.editors.editors.iter().position(|editor| editor.model_id == id)) {
            self.editors.activate(index);
        }
        self.editors_changed(cx);
        self.resolve_custom_editors(cx);
    }

    /// Runs from every frame: a changed dirty model schedules its backup, a model no longer dirty (saved, reverted,
    /// closed) loses it.
    pub(super) fn backup_models_changed(&mut self, cx: &mut Context<Self>) {
        if self.lifecycle.backups.home.is_none() || self.lifecycle.backups.suspended {
            return;
        }
        let mut dirty = Vec::new();
        for model in self.editors.editors.iter().chain(&self.editors.models).filter(|model| tracked(model) && model.is_dirty()) {
            let document = model.document();
            dirty.push(((document.scheme.clone(), document.path.clone()), model.alternative_version_id()));
        }
        let tracker = &mut self.lifecycle.backups;
        let gone: Vec<(String, PathBuf)> = tracker.backed_up.keys().chain(tracker.pending.keys()).filter(|key| !dirty.iter().any(|(dirty, _)| dirty == *key)).cloned().collect();
        for key in gone {
            self.discard_backup(&key);
        }
        for (key, version) in dirty {
            let tracker = &self.lifecycle.backups;
            if tracker.backed_up.get(&key) == Some(&version) || tracker.pending.get(&key).is_some_and(|(pending, _)| *pending == version) {
                continue;
            }
            let scheduled = key.clone();
            let task = cx.spawn(async move |workbench, cx| {
                cx.background_executor().timer(BACKUP_DELAY).await;
                let _ = workbench.update(cx, |workbench, _| {
                    workbench.lifecycle.backups.pending.remove(&scheduled);
                    workbench.backup_model(&scheduled);
                });
            });
            self.lifecycle.backups.pending.insert(key, (version, task));
        }
        self.backup_documents_changed(DocumentKind::Custom, cx);
        self.backup_documents_changed(DocumentKind::Notebook, cx);
    }

    /// The same for custom documents and notebooks: a dirty one backs up a second after its last change, a clean
    /// or closed one loses its backup.
    fn backup_documents_changed(&mut self, kind: DocumentKind, cx: &mut Context<Self>) {
        let dirty = self.dirty_documents(kind);
        let documents = self.lifecycle.backups.documents(kind);
        let gone: Vec<(String, String)> = documents.backed_up.keys().chain(documents.pending.keys()).filter(|key| !dirty.iter().any(|(dirty, _)| dirty == *key)).cloned().collect();
        for key in gone {
            let documents = self.lifecycle.backups.documents(kind);
            documents.pending.remove(&key);
            documents.backed_up.remove(&key);
            if let Ok(resource) = serde_json::from_str::<Value>(&key.0) {
                match kind {
                    DocumentKind::Custom => self.discard_custom_document_backup(&resource, &key.1),
                    DocumentKind::Notebook => self.discard_notebook_backup(&resource, &key.1),
                }
            }
        }
        for (key, version) in dirty {
            let documents = self.lifecycle.backups.documents(kind);
            if documents.backed_up.get(&key) == Some(&version) || documents.pending.get(&key).is_some_and(|(pending, _)| *pending == version) {
                continue;
            }
            let scheduled = key.clone();
            let task = cx.spawn(async move |workbench, cx| {
                cx.background_executor().timer(BACKUP_DELAY).await;
                let Ok(backup) = workbench.update(cx, |workbench, cx| {
                    // Dropping this task would cancel it before it records the backup below.
                    if let Some((_, running)) = workbench.lifecycle.backups.documents(kind).pending.remove(&scheduled) {
                        running.detach();
                    }
                    workbench.backup_document(kind, &scheduled, cx)
                }) else {
                    return;
                };
                if let Ok(Some(version)) = backup.await {
                    let _ = workbench.update(cx, |workbench, _| workbench.lifecycle.backups.documents(kind).backed_up.insert(scheduled, version));
                }
            });
            self.lifecycle.backups.documents(kind).pending.insert(key, (version, task));
        }
    }

    /// The dirty documents of `kind`, by key, with their versions.
    fn dirty_documents(&self, kind: DocumentKind) -> Vec<((String, String), u64)> {
        match kind {
            DocumentKind::Custom => self.webview_panels.custom_editors.documents.iter().filter(|document| document.is_dirty()).map(|document| (document_key(&document.resource, &document.view_type), document.version)).collect(),
            DocumentKind::Notebook => self.host.notebooks.models.iter().filter(|model| model.dirty).map(|model| (document_key(&model.uri, &model.view_type), model.version_id)).collect(),
        }
    }

    /// The backup of the document at `key`: answers the version it is for, None when it failed or is gone.
    fn backup_document(&mut self, kind: DocumentKind, key: &(String, String), cx: &mut Context<Self>) -> oneshot::Receiver<Option<u64>> {
        let found = match kind {
            DocumentKind::Custom => self.webview_panels.custom_editors.documents.iter().position(|document| document_key(&document.resource, &document.view_type) == *key).map(|index| self.backup_custom_document(index, cx)),
            DocumentKind::Notebook => serde_json::from_str::<Value>(&key.0).ok().map(|uri| self.backup_notebook(&crate::notebook::uri_string(&uri), cx)),
        };
        found.unwrap_or_else(|| {
            let (sender, receiver) = oneshot::channel();
            let _ = sender.send(None);
            receiver
        })
    }

    /// `backupBeforeShutdown` of the custom documents and notebooks not backed up at their version: answers whether
    /// each worked.
    pub(super) fn backup_documents_before_shutdown(&mut self, cx: &mut Context<Self>) -> Vec<oneshot::Receiver<Option<u64>>> {
        let mut backups = Vec::new();
        for kind in [DocumentKind::Custom, DocumentKind::Notebook] {
            self.lifecycle.backups.documents(kind).pending.clear();
            if self.lifecycle.backups.home.is_none() {
                continue;
            }
            for (key, version) in self.dirty_documents(kind) {
                if self.lifecycle.backups.documents(kind).backed_up.get(&key) != Some(&version) {
                    backups.push(self.backup_document(kind, &key, cx));
                }
            }
        }
        backups
    }

    /// `doBackup` of one model: its text and, for a file, the stat it was read with.
    fn backup_model(&mut self, key: &(String, PathBuf)) -> bool {
        let Some(home) = self.lifecycle.backups.home.clone() else { return true };
        let Some(model) = self.editors.model(&key.0, &key.1).filter(|model| model.is_dirty()) else { return true };
        let resource = model_uri(model);
        let meta = (key.0 == "file").then(|| file_meta(&key.1, self.text_files.orphaned.contains(&key.1))).flatten();
        let content = model.document().lines.join(model.document().eol);
        let version = model.alternative_version_id();
        match backups::write(&home, &resource, "", meta.as_ref(), content.as_bytes()) {
            Ok(()) => {
                self.lifecycle.backups.backed_up.insert(key.clone(), version);
                true
            }
            Err(error) => {
                eprintln!("[backup tracker] backing up {}: {error}", key.1.display());
                false
            }
        }
    }

    fn discard_backup(&mut self, key: &(String, PathBuf)) {
        let tracker = &mut self.lifecycle.backups;
        tracker.pending.remove(key);
        tracker.backed_up.remove(key);
        if let (Some(home), Ok(resource)) = (&tracker.home, Uri::new(&key.0, "", &key.1.to_string_lossy(), "", "")) {
            backups::discard(home, &resource, "");
        }
    }

    /// `shouldBackupBeforeShutdown`: whether hot exit backs up the modified editors instead of asking. On macOS a
    /// closed window does not quit the app, so only quit, reload and (with `onExitAndWindowClose` in a folder)
    /// closing or loading another workspace back up.
    pub(super) fn backs_up_on_shutdown(&self, reason: ShutdownReason) -> bool {
        let hot_exit = self.setting("files.hotExit");
        if hot_exit == "off" {
            return false;
        }
        if self.extension_development {
            return true;
        }
        let in_workspace = self.workspace().is_some();
        match reason {
            ShutdownReason::Quit | ShutdownReason::Reload => true,
            ShutdownReason::Close | ShutdownReason::Load => in_workspace && hot_exit == "onExitAndWindowClose",
        }
    }

    /// `backupBeforeShutdown`: every modified editor backed up now, pending timers cancelled; false when one failed.
    pub(super) fn backup_before_shutdown(&mut self) -> bool {
        let tracker = &mut self.lifecycle.backups;
        tracker.pending.clear();
        if tracker.home.is_none() {
            return true;
        }
        let dirty: Vec<(String, PathBuf)> = self.editors.editors.iter().chain(&self.editors.models).filter(|model| tracked(model) && model.is_dirty()).map(|model| (model.document().scheme.clone(), model.document().path.clone())).collect();
        let mut all = true;
        for key in dirty {
            if self.lifecycle.backups.backed_up.get(&key) != self.editors.model(&key.0, &key.1).map(OpenEditor::alternative_version_id).as_ref() {
                all &= self.backup_model(&key);
            }
        }
        all
    }

    /// `onBeforeShutdownWithoutModified` and `noVeto`: with nothing modified the window's backups all go.
    pub(super) fn discard_all_backups(&mut self) {
        let tracker = &mut self.lifecycle.backups;
        tracker.pending.clear();
        tracker.backed_up.clear();
        tracker.custom = DocumentBackups::default();
        tracker.notebooks = DocumentBackups::default();
        if let Some(home) = &tracker.home {
            for backup in backups::list(home) {
                let _ = std::fs::remove_file(backup);
            }
        }
    }

    /// `suspendBackupOperations` and its `resume`.
    pub(super) fn suspend_backups(&mut self, suspended: bool) {
        self.lifecycle.backups.suspended = suspended;
        if suspended {
            self.lifecycle.backups.pending.clear();
            self.lifecycle.backups.custom.pending.clear();
            self.lifecycle.backups.notebooks.pending.clear();
        }
    }
}

fn document_key(resource: &Value, view_type: &str) -> (String, String) {
    (resource.to_string(), view_type.to_owned())
}

/// The text working copies the port backs up: text files and untitled editors.
fn tracked(model: &OpenEditor) -> bool {
    matches!(model.document().scheme.as_str(), "file" | "untitled")
}

fn model_uri(model: &OpenEditor) -> Uri {
    let document = model.document();
    Uri::new(&document.scheme, "", &document.path.to_string_lossy(), "", "").expect("file and untitled paths make URIs")
}

/// textFileEditorModel.ts `backup` meta: the file's `mtime`, `ctime`, `size`, `etag` and whether it is orphaned.
pub(super) fn file_meta(path: &Path, orphaned: bool) -> Option<Value> {
    let metadata = std::fs::metadata(path).ok()?;
    let millis = |time: std::io::Result<SystemTime>| time.ok().and_then(|time| time.duration_since(UNIX_EPOCH).ok()).map_or(0, |since| since.as_millis() as u64);
    let (mtime, ctime, size) = (millis(metadata.modified()), millis(metadata.created()), metadata.len());
    Some(json!({ "mtime": mtime, "ctime": ctime, "size": size, "etag": etag(mtime, size), "orphaned": orphaned }))
}

/// files.ts `etag`: `mtime.toString(29) + size.toString(31)`.
fn etag(mtime: u64, size: u64) -> String {
    radix(mtime, 29) + &radix(size, 31)
}

fn radix(mut value: u64, base: u64) -> String {
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut text = Vec::new();
    loop {
        text.push(digits[(value % base) as usize]);
        value /= base;
        if value == 0 {
            break;
        }
    }
    text.reverse();
    String::from_utf8(text).expect("ASCII digits")
}

#[cfg(test)]
mod tests;
