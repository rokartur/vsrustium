//! What the main thread tells the extension host about open text documents, the visible editor and the
//! tabs: mainThreadDocumentsAndEditors.ts (`$acceptDocumentsAndEditorsDelta`), mainThreadEditorTabs.ts
//! (`$acceptTabOperation`), mainThreadEditors.ts (`$acceptEditorPropertiesChanged`) and the
//! `onLanguage` activation languageService.ts sends when a model of a new language is created.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::Shared;

use config::model::Change;
use config::service::Settings;
use exthost::extension_management::Job;
use exthost::main_thread_configuration::changed_args;
use exthost::scan::ScannedExtension;
use files::operations::{self, FileEdit, FileUndo, WorkingCopyFiles};
use serde_json::{Value, json};

use crate::diff_editor::{DIFF_EDITOR_SCHEME, DiffInput};
use crate::multi_diff_editor::MULTI_DIFF_EDITOR_SCHEME;
use crate::editor::{ContentChanged, EditReason, EditorGroup, OpenEditor, Position, Selection, uri_string, uri_value};
use crate::notebook::{CELL_SCHEME, NOTEBOOK_EDITOR_SCHEME};
use crate::theme::ThemeKind;

#[derive(Debug)]
pub struct HostMessage {
    pub proxy: &'static str,
    pub method: &'static str,
    pub args: Vec<Value>,
    /// Where the host's answer goes, for `HostSync::call`.
    pub reply: Option<oneshot::Sender<Result<Value, Value>>>,
    /// The host method takes a `CancellationToken`; dropping the reply's receiver cancels it.
    pub cancellable: bool,
    /// Working copy file operations, run on the host thread so they reach its file operation participants.
    pub files: Option<FileWork>,
    /// An install or uninstall, run on the host thread, which owns the extensions and tells the host.
    pub extensions: Option<(Job, ExtensionsReply)>,
}

/// The answer of one `$activateByEvent`, shared by every caller that asks for the same event.
pub type Activation = Shared<oneshot::Receiver<Result<Value, Value>>>;

/// The running extensions after a `Job`, or why it failed.
pub type ExtensionsReply = oneshot::Sender<Result<Vec<ScannedExtension>, String>>;

/// Explorer file edits (`IExplorerService.applyBulkEdit`), the undo or redo of earlier ones, or the write of a
/// saved model of an extension's file system provider.
pub enum FileJob {
    Apply(Vec<FileEdit>),
    Reverse(FileUndo),
    Write(files::Resource, Vec<u8>),
}

pub struct FileWork {
    job: FileJob,
    reply: oneshot::Sender<files::Result<FileUndo>>,
}

impl std::fmt::Debug for FileWork {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("FileWork")
    }
}

impl FileWork {
    /// Runs the job and answers with what reverses it.
    pub fn run(self, fs: &mut impl WorkingCopyFiles) {
        let result = match self.job {
            FileJob::Apply(edits) => operations::apply(fs, edits, None),
            FileJob::Reverse(mut undo) => undo.reverse(fs).map(|()| undo),
            FileJob::Write(resource, bytes) => files::write_file(fs, &resource, &bytes).map(|()| FileUndo::default()),
        };
        let _ = self.reply.send(result);
    }
}

pub struct HostSync {
    sender: Sender<HostMessage>,
    /// The documents the host has, in the order they were added: their URI string and `UriComponents`.
    documents: Vec<(String, Value)>,
    /// The snapshot ids of the visible editors the host has, one per group showing a text editor.
    editors: Vec<String>,
    /// mainThreadEditors.ts `_editorPositionData`: each visible text editor's view column, as last sent.
    editor_positions: serde_json::Map<String, Value>,
    /// The id of the active editor the host has.
    active_editor: Option<String>,
    /// The lines of the visible editor the host has as its `visibleRanges`.
    visible_lines: Option<Vec<(usize, usize)>>,
    /// The groups the host has in view column order with their tabs, and the active group; session.rs starts
    /// with one empty group 0.
    tabs: Vec<(u32, Vec<Value>)>,
    active_group: u32,
    /// The search editors' tabs by their model's path; the workbench keeps them current.
    pub search_tabs: HashMap<PathBuf, SearchTab>,
    /// The webview panels' tabs by their editor's path; no text model backs them.
    pub webview_tabs: HashMap<PathBuf, WebviewTab>,
    activations: HashMap<String, Activation>,
    /// The activation events the running extensions declare (`ExtensionDescriptionRegistry`'s activation map).
    pub activation_events: HashSet<String>,
    /// `whenInstalledExtensionsRegistered`: set once the host's handshake is done; until then commands do not wait for
    /// their `onCommand` activation.
    pub installed_extensions_registered: bool,
    /// `configurationScopes` go to the host in a development build only (`MainThreadConfiguration.send_scopes`).
    pub configuration_scopes: bool,
    /// `workbench.editor.untitled.labelFormat` is `content`: untitled tabs take their first line's words.
    pub untitled_names_from_content: bool,
    /// The notebooks, their editors and kernels; a cell's text change is a change of its notebook too.
    pub notebooks: crate::notebook::Notebooks,
}

/// `TabModelOperationKind`.
const TAB_OPEN: u8 = 0;
const TAB_CLOSE: u8 = 1;
const TAB_UPDATE: u8 = 2;
const TAB_MOVE: u8 = 3;

/// The one editor group's code editor widget; it is reused for every model the group shows.
const CODE_EDITOR_ID: &str = "vs.editor.ICodeEditor:1";

impl HostSync {
    pub fn new(sender: Sender<HostMessage>) -> Self {
        HostSync { sender, documents: Vec::new(), editors: Vec::new(), editor_positions: serde_json::Map::new(), active_editor: None, visible_lines: None, tabs: vec![(0, Vec::new())], active_group: 0, search_tabs: HashMap::new(), webview_tabs: HashMap::new(), activations: HashMap::new(), activation_events: HashSet::new(), installed_extensions_registered: false, configuration_scopes: false, untitled_names_from_content: true, notebooks: Default::default() }
    }

    /// Sends what changed in `group` since the last call: activation for new languages, the document
    /// and editor delta, then the tab model.
    pub fn send_changes(&mut self, group: &EditorGroup) {
        let is_entry = |editor: &&OpenEditor| matches!(editor.document().scheme.as_str(), DIFF_EDITOR_SCHEME | MULTI_DIFF_EDITOR_SCHEME | crate::merge_editor::MERGE_EDITOR_SCHEME | NOTEBOOK_EDITOR_SCHEME);
        let is_text = |editor: &&OpenEditor| !self.webview_tabs.contains_key(&editor.document().path) && !is_entry(editor);
        // A custom text editor's model stays a document, without a text editor (customTextEditorModel.ts).
        let is_document = |editor: &&OpenEditor| self.webview_tabs.get(&editor.document().path).is_none_or(|tab| tab.custom_text) && !is_entry(editor);
        let open: Vec<&OpenEditor> = group.all_editors().filter(is_document).collect();
        let mut added: Vec<&OpenEditor> = open.iter().copied().filter(|editor| !self.documents.iter().any(|(uri, _)| *uri == editor.document().uri_string())).collect();
        // Editors of one model in several groups are one document.
        let mut seen = Vec::new();
        added.retain(|editor| {
            let uri = editor.document().uri_string();
            let new = !seen.contains(&uri);
            seen.push(uri);
            new
        });
        let removed: Vec<(String, Value)> =
            self.documents.iter().filter(|(uri, _)| !open.iter().any(|editor| editor.document().uri_string() == *uri)).cloned().collect();
        for editor in &added {
            for event in [format!("onLanguage:{}", editor.document().language_id), "onLanguage".to_string()] {
                // `activate_by_event`, inlined: `is_text` still borrows `self.webview_tabs`.
                if !self.activations.contains_key(&event) {
                    let activation = self.call("ExtHostExtensionService", "$activateByEvent", vec![json!(event), json!(0)]).shared();
                    self.activations.insert(event, activation);
                }
            }
        }

        let mut delta = serde_json::Map::new();
        if !removed.is_empty() {
            delta.insert("removedDocuments".into(), removed.iter().map(|(_, uri)| uri.clone()).collect());
        }
        if !added.is_empty() {
            delta.insert("addedDocuments".into(), added.iter().map(|editor| model_added_data(editor)).collect());
        }
        // mainThreadDocumentsAndEditors.ts: a text editor per group showing one, at its group's view column.
        let visible: Vec<(usize, &OpenEditor, crate::editor::Viewport)> = group
            .column_ids()
            .into_iter()
            .enumerate()
            .flat_map(|(column, id)| {
                let Some((editor, viewport)) = group.group_active(id) else { return Vec::new() };
                // Every cell of a notebook editor is a code editor the host sees, the focused one first.
                // shortcut: every cell, where the notebook list renders only the cells in view.
                let cells = group.group_active_tab(id).map(|tab| crate::workbench::notebook_cells(&self.notebooks, group, tab)).unwrap_or_default();
                let editors = std::iter::once(editor).chain(cells);
                editors.filter(|editor| is_text(editor)).map(|editor| (column, editor, viewport)).collect()
            })
            .collect();
        let mut seen_cells = Vec::new();
        let visible: Vec<(usize, &OpenEditor, crate::editor::Viewport)> = visible
            .into_iter()
            .filter(|(_, editor, _)| {
                let id = snapshot_id(editor);
                let new = !seen_cells.contains(&id);
                seen_cells.push(id);
                new
            })
            .collect();
        // The Output view's editor is visible without a view column (`editorPosition` undefined).
        let panel = group.panel_editor_shown.then(|| group.group_active(crate::editor::PANEL_GROUP)).flatten().filter(|(editor, _)| is_text(editor));
        let visible: Vec<(Option<usize>, &OpenEditor, crate::editor::Viewport)> =
            visible.into_iter().map(|(column, editor, viewport)| (Some(column), editor, viewport)).chain(panel.map(|(editor, viewport)| (None, editor, viewport))).collect();
        let ids: Vec<String> = visible.iter().map(|(_, editor, _)| snapshot_id(editor)).collect();
        let removed_editors: Vec<&String> = self.editors.iter().filter(|id| !ids.contains(id)).collect();
        if !removed_editors.is_empty() {
            delta.insert("removedEditors".into(), json!(removed_editors));
        }
        let mut added_editors = Vec::new();
        for (column, editor, viewport) in visible.iter().filter(|(_, editor, _)| !self.editors.contains(&snapshot_id(editor))) {
            let visible_lines = editor.visible_model_ranges(*viewport);
            let mut data = text_editor_add_data(editor, &visible_lines);
            data["editorPosition"] = json!(column);
            added_editors.push(data);
            if group.active().is_some_and(|active| std::ptr::eq(active, *editor)) {
                self.visible_lines = Some(visible_lines);
            }
        }
        if !added_editors.is_empty() {
            delta.insert("addedEditors".into(), Value::Array(added_editors));
        }
        let active_id = group.active().filter(is_text).map(snapshot_id);
        if active_id != self.active_editor || delta.contains_key("removedEditors") || delta.contains_key("addedEditors") {
            delta.insert("newActiveEditor".into(), json!(active_id));
        }
        if !delta.is_empty() {
            self.send("ExtHostDocumentsAndEditors", "$acceptDocumentsAndEditorsDelta", vec![Value::Object(delta)]);
        }
        self.documents.retain(|document| !removed.contains(document));
        self.documents.extend(added.iter().map(|editor| (editor.document().uri_string(), editor.document().uri())));
        self.editors = ids;
        self.active_editor = active_id;
        // `_updateActiveAndVisibleTextEditors`: the columns, when one changed (a group closed or moved).
        let positions: serde_json::Map<String, Value> = visible.iter().filter_map(|(column, editor, _)| Some((snapshot_id(editor), json!((*column)?)))).collect();
        if positions != self.editor_positions {
            self.send("ExtHostEditors", "$acceptEditorPositionData", vec![Value::Object(positions.clone())]);
            self.editor_positions = positions;
        }
        self.send_tabs(group);
    }

    /// One `$acceptTabOperation` per tab that closed, opened, moved or changed, as mainThreadEditorTabs.ts
    /// sends them: a whole `$acceptEditorTabModel` fires no `onDidChangeTabs`, which TypeScript's
    /// TabResourceTracker needs before it reports diagnostics.
    /// A change to the groups themselves (added, removed, reordered, another active one) sends the whole model.
    pub fn send_tabs(&mut self, group: &EditorGroup) {
        let groups: Vec<(u32, Vec<Value>)> = group.column_ids().into_iter().map(|id| (id, self.group_tab_dtos(group, id))).collect();
        let same_groups = groups.iter().map(|(id, _)| *id).eq(self.tabs.iter().map(|(id, _)| *id));
        if !same_groups || self.active_group != group.id {
            let model: Vec<Value> = groups.iter().enumerate().map(|(column, (id, tabs))| json!({ "groupId": id, "isActive": *id == group.id, "viewColumn": column, "tabs": tabs })).collect();
            self.send("ExtHostEditorTabs", "$acceptEditorTabModel", vec![Value::Array(model)]);
            self.tabs = groups;
            self.active_group = group.id;
            return;
        }
        for (id, tabs) in groups {
            self.send_group_tabs(id, tabs);
        }
    }

    fn group_tab_dtos(&self, group: &EditorGroup, id: u32) -> Vec<Value> {
        let (editors, active) = group.group_tabs(id);
        let dirty = |editor: &OpenEditor, diff: &DiffInput| group.model(&diff.modified.scheme, &diff.modified.path).unwrap_or(editor).is_dirty();
        editors
            .iter()
            .enumerate()
            .map(|(index, editor)| {
                let active = active == Some(index);
                if let Some(diff) = group.diffs.get(&editor.model_id) {
                    return diff_tab_dto(id, editor, diff, active, dirty(editor, diff));
                }
                if let Some(multi_diff) = group.multi_diffs.get(&editor.model_id) {
                    return multi_diff.tab_dto(id, editor, active);
                }
                if let Some(merge) = group.merges.get(&editor.model_id) {
                    return merge.tab_dto(id, editor, active, group.model(&merge.result.scheme, &merge.result.path).unwrap_or(editor).is_dirty());
                }
                tab_dto(id, editor, active, self)
            })
            .collect()
    }

    fn send_group_tabs(&mut self, group: u32, tabs: Vec<Value>) {
        let slot = self.tabs.iter().position(|(id, _)| *id == group).expect("the host has the group");
        let mut sent = std::mem::take(&mut self.tabs[slot].1);
        for index in (0..sent.len()).rev() {
            if !tabs.iter().any(|tab| tab["id"] == sent[index]["id"]) {
                let closed = sent.remove(index);
                self.send_tab_operation(group, TAB_CLOSE, index, closed, None);
            }
        }
        for (index, tab) in tabs.iter().enumerate() {
            match sent.iter().position(|old| old["id"] == tab["id"]) {
                None => {
                    sent.insert(index, tab.clone());
                    self.send_tab_operation(group, TAB_OPEN, index, tab.clone(), None);
                }
                Some(old_index) if old_index != index => {
                    let moved = sent.remove(old_index);
                    sent.insert(index, tab.clone());
                    self.send_tab_operation(group, TAB_MOVE, index, moved.clone(), Some(old_index));
                    if moved != *tab {
                        self.send_tab_operation(group, TAB_UPDATE, index, tab.clone(), None);
                    }
                }
                Some(_) if sent[index] != *tab => {
                    sent[index] = tab.clone();
                    self.send_tab_operation(group, TAB_UPDATE, index, tab.clone(), None);
                }
                Some(_) => {}
            }
        }
        self.tabs[slot].1 = sent;
    }

    fn send_tab_operation(&self, group: u32, kind: u8, index: usize, tab: Value, old_index: Option<usize>) {
        let mut operation = json!({ "groupId": group, "index": index, "tabDto": tab, "kind": kind });
        if let Some(old_index) = old_index {
            operation["oldIndex"] = json!(old_index);
        }
        self.send("ExtHostEditorTabs", "$acceptTabOperation", vec![operation]);
    }

    /// Tells the host the text of the editor at `index` changed (mainThreadDocuments.ts ModelTracker), then the
    /// dirty state and tabs when it flipped, then where the cursor went.
    pub fn send_content_change(&mut self, group: &EditorGroup, index: usize, change: &ContentChanged) {
        let editor = group.tab_model(index);
        self.send_model_change(group, editor, change);
        self.send_selection(editor, "keyboard");
    }

    /// The text of `editor`, shown or a model without an editor, changed: the change, then the dirty state and tabs when it flipped, the tabs also for an untitled model.
    pub fn send_model_change(&mut self, group: &EditorGroup, editor: &OpenEditor, change: &ContentChanged) {
        let document = &editor.document();
        let uri = document.uri();
        let changes: Vec<Value> = change
            .changes
            .iter()
            .map(|change| {
                json!({
                    "range": { "startLineNumber": change.start_line, "startColumn": change.start_column, "endLineNumber": change.end_line, "endColumn": change.end_column },
                    "rangeOffset": change.range_offset,
                    "rangeLength": change.range_length,
                    "text": change.text,
                })
            })
            .collect();
        let event = json!({
            "changes": changes,
            "eol": document.eol,
            "versionId": change.version_id,
            "isUndoing": change.is_undoing,
            "isRedoing": change.is_redoing,
            "isFlush": false,
            "isEolChange": matches!(change.reason, EditReason::EolChange),
            "detailedReason": detailed_reason(change.reason),
        });
        let dirty = editor.is_dirty();
        self.send("ExtHostDocuments", "$acceptModelChanged", vec![uri.clone(), event, json!(dirty)]);
        if document.scheme == CELL_SCHEME {
            self.send_cell_content_change(&uri);
        }
        if dirty != change.was_dirty {
            self.send("ExtHostDocuments", "$acceptDirtyStateChanged", vec![uri, json!(dirty)]);
        }
        // untitledTextEditorInput.ts `onDidChangeLabel`: a new first line renames an untitled tab.
        if dirty != change.was_dirty || document.scheme == "untitled" {
            self.send_tabs(group);
        }
    }

    /// notebookTextModel.ts `_bindCellContentHandler`: a cell's text changed, so its notebook did.
    fn send_cell_content_change(&mut self, cell_uri: &Value) {
        let Some((notebook, _)) = crate::notebook::parse_cell_uri(cell_uri) else { return };
        let Some(model) = self.notebooks.model_mut(&crate::notebook::uri_string(&notebook)) else { return };
        let Some(index) = model.index_of_cell(cell_uri) else { return };
        let was_dirty = model.dirty;
        let event = model.cell_content_changed(index);
        let uri = model.uri.clone();
        self.send("ExtHostNotebookDocuments", "$acceptModelChanged", vec![uri.clone(), json!({ exthost::rpc::WITH_BUFFERS_KEY: event }), json!(true), Value::Null]);
        if !was_dirty {
            self.send("ExtHostNotebookDocuments", "$acceptDirtyStateChanged", vec![uri, json!(true)]);
        }
    }

    /// `editor`, open or a model the host opened, was written to disk while `was_dirty`.
    pub fn send_saved(&mut self, group: &EditorGroup, editor: &OpenEditor, was_dirty: bool) {
        if was_dirty {
            self.send_dirty_state(group, editor, false);
        }
        self.send("ExtHostDocuments", "$acceptModelSaved", vec![editor.document().uri()]);
    }

    /// The dirty state of `editor` flipped without a text change, then the tabs that show it.
    pub fn send_dirty_state(&mut self, group: &EditorGroup, editor: &OpenEditor, dirty: bool) {
        self.send("ExtHostDocuments", "$acceptDirtyStateChanged", vec![editor.document().uri(), json!(dirty)]);
        self.send_tabs(group);
    }

    /// mainThreadEditor.ts `_updatePropertiesNow` on a scroll or layout: the visible lines, when they changed.
    pub fn send_visible_ranges(&mut self, group: &EditorGroup) {
        let Some(editor) = group.active() else { return };
        let lines = editor.visible_model_ranges(group.viewport);
        if self.visible_lines.as_ref() == Some(&lines) {
            return;
        }
        let ranges = visible_ranges(editor, &lines);
        self.visible_lines = Some(lines);
        self.send("ExtHostEditors", "$acceptEditorPropertiesChanged", vec![json!(snapshot_id(editor)), json!({ "options": null, "selections": null, "visibleRanges": ranges })]);
    }

    /// mainThreadEditor.ts `_updatePropertiesNow` after the model's indentation options changed.
    pub fn send_options(&self, editor: &OpenEditor) {
        self.send("ExtHostEditors", "$acceptEditorPropertiesChanged", vec![json!(snapshot_id(editor)), json!({ "options": options(editor), "selections": null, "visibleRanges": null })]);
    }

    /// Tells the host where the cursor of the visible editor moved.
    pub fn send_selection(&self, editor: &OpenEditor, source: &str) {
        let selections = json!({ "selections": selections(editor), "source": source });
        self.send("ExtHostEditors", "$acceptEditorPropertiesChanged", vec![json!(snapshot_id(editor)), json!({ "options": null, "selections": selections, "visibleRanges": null })]);
    }

    /// A setting the workbench wrote: mainThreadConfiguration.ts sends every configuration change.
    pub fn send_configuration_change(&self, settings: &Settings, change: &Change) {
        self.send("ExtHostConfiguration", "$acceptConfigurationChanged", changed_args(settings, self.configuration_scopes, change));
    }

    /// mainThreadTheming.ts: the theme type behind `window.activeColorTheme.kind`, sent at startup and on each theme change.
    pub fn send_color_theme(&self, kind: ThemeKind) {
        let scheme = match kind {
            ThemeKind::Dark => "dark",
            ThemeKind::Light => "light",
            ThemeKind::HighContrastDark => "hcDark",
            ThemeKind::HighContrastLight => "hcLight",
        };
        self.send("ExtHostTheming", "$onColorThemeChange", vec![json!(scheme)]);
    }

    /// mainThreadLanguages.ts: a color theme change tells `onDidChangeSyntaxHighlighting` listeners.
    pub fn send_syntax_highlighting_theme_changed(&self) {
        self.send("ExtHostLanguages", "$acceptSyntaxHighlightingThemeChanged", Vec::new());
    }

    /// mainThreadLanguages.ts: `$acceptLanguageIds` with the registered languages, what `getLanguages` answers.
    pub fn send_language_ids(&self, languages: &crate::languages::Languages) {
        self.send("ExtHostLanguages", "$acceptLanguageIds", vec![json!(languages.ids())]);
    }

    /// `send` for callbacks that outlive a borrow of the workbench (a terminal's input).
    pub fn notifier(&self) -> impl Fn(&'static str, &'static str, Vec<Value>) + 'static {
        let sender = self.sender.clone();
        move |proxy, method, args| {
            let _ = sender.send(HostMessage { proxy, method, args, reply: None, cancellable: false, files: None, extensions: None });
        }
    }

    /// `call_cancellable` for requests made one after another from a task (a provider asked when the one before it
    /// gave no answer).
    pub fn caller(&self) -> impl Fn(&'static str, &'static str, Vec<Value>) -> oneshot::Receiver<Result<Value, Value>> + use<> {
        let sender = self.sender.clone();
        move |proxy, method, args| {
            let (reply, receiver) = oneshot::channel();
            let _ = sender.send(HostMessage { proxy, method, args, reply: Some(reply), cancellable: true, files: None, extensions: None });
            receiver
        }
    }

    pub(crate) fn send(&self, proxy: &'static str, method: &'static str, args: Vec<Value>) {
        // The session thread is gone once the host exits; the workbench keeps working without it.
        let _ = self.sender.send(HostMessage { proxy, method, args, reply: None, cancellable: false, files: None, extensions: None });
    }

    /// mainThreadWorkspace.ts `_onDidChangeWorkspace`: the session's search folders, then `$acceptWorkspaceData`.
    pub fn send_workspace_data(&self, workspace: &config::workspace::Workspace) {
        let folders: Vec<Value> = workspace.folders.iter().map(|folder| Value::String(folder.path.to_string_lossy().into_owned())).collect();
        self.send("workbench", "setWorkspaceFolders", folders);
        self.send("ExtHostWorkspace", "$acceptWorkspaceData", vec![exthost::session::workspace_data_of(workspace)]);
    }

    /// `MessageType.Terminate`: the session thread tells the host to deactivate its extensions and exit.
    pub fn stop_extension_host(&self) {
        self.send("workbench", "stopExtensionHost", Vec::new());
    }

    /// Runs file edits on the host thread; with no host they run here on the disk, without participants.
    pub fn run_files(&self, job: FileJob) -> oneshot::Receiver<files::Result<FileUndo>> {
        let (reply, receiver) = oneshot::channel();
        let message = HostMessage { proxy: "", method: "", args: Vec::new(), reply: None, cancellable: false, files: Some(FileWork { job, reply }), extensions: None };
        if let Err(unsent) = self.sender.send(message) {
            unsent.0.files.expect("the message carries file work").run(&mut files::disk::Disk);
        }
        receiver
    }

    /// Reads the activation events of the extensions the host runs.
    pub fn set_running_extensions(&mut self, extensions: &[ScannedExtension]) {
        self.activation_events = extensions.iter().flat_map(|extension| exthost::activation_events::read_activation_events(&extension.id, &extension.description)).collect();
    }

    /// `ExtensionHostManager.activateByEvent`: one request per event; a repeat shares the first one's answer.
    pub fn activate_by_event(&mut self, event: &str) -> Activation {
        if let Some(activation) = self.activations.get(event) {
            return activation.clone();
        }
        // ActivationKind.Normal
        let activation = self.call("ExtHostExtensionService", "$activateByEvent", vec![json!(event), json!(0)]).shared();
        self.activations.insert(event.to_owned(), activation.clone());
        activation
    }

    /// Sends a request whose answer arrives on the receiver; a host that is gone drops the sender.
    pub fn call(&self, proxy: &'static str, method: &'static str, args: Vec<Value>) -> oneshot::Receiver<Result<Value, Value>> {
        let (reply, receiver) = oneshot::channel();
        let _ = self.sender.send(HostMessage { proxy, method, args, reply: Some(reply), cancellable: false, files: None, extensions: None });
        receiver
    }

    /// Installs or uninstalls on the host thread; a host that is gone drops the sender.
    pub fn run_extensions(&self, job: Job) -> oneshot::Receiver<Result<Vec<ScannedExtension>, String>> {
        let (reply, receiver) = oneshot::channel();
        let _ = self.sender.send(HostMessage { proxy: "", method: "", args: Vec::new(), reply: None, cancellable: false, files: None, extensions: Some((job, reply)) });
        receiver
    }

    /// `call` of a host method that takes a `CancellationToken`: dropping the receiver cancels the request.
    pub fn call_cancellable(&self, proxy: &'static str, method: &'static str, args: Vec<Value>) -> oneshot::Receiver<Result<Value, Value>> {
        let (reply, receiver) = oneshot::channel();
        let _ = self.sender.send(HostMessage { proxy, method, args, reply: Some(reply), cancellable: true, files: None, extensions: None });
        receiver
    }
}

/// `TextEditorSnapshot.id`: the code editor's id and the model's id.
pub fn snapshot_id(editor: &OpenEditor) -> String {
    format!("{CODE_EDITOR_ID},$model{}", editor.model_id)
}

fn model_added_data(editor: &OpenEditor) -> Value {
    let document = &editor.document();
    json!({
        "uri": document.uri(),
        "versionId": editor.text_model().version_id,
        "lines": document.lines,
        "EOL": document.eol,
        "languageId": document.language_id,
        "isDirty": editor.is_dirty(),
        "encoding": document.encoding,
    })
}

fn text_editor_add_data(editor: &OpenEditor, visible_lines: &[(usize, usize)]) -> Value {
    let document = &editor.document();
    json!({
        "id": snapshot_id(editor),
        "documentUri": document.uri(),
        "options": options(editor),
        "selections": selections(editor),
        "visibleRanges": visible_ranges(editor, visible_lines),
        "editorPosition": 0,
    })
}

/// `IResolvedTextEditorConfiguration`: TextEditorCursorStyle.Line, RenderLineNumbersType.On.
fn options(editor: &OpenEditor) -> Value {
    let indentation = editor.document().indentation;
    json!({
        "tabSize": indentation.tab_size,
        "indentSize": indentation.indent_size(),
        "originalIndentSize": indentation.indent_size.map_or(json!("tabSize"), |size| json!(size)),
        "insertSpaces": indentation.insert_spaces,
        "cursorStyle": 1,
        "lineNumbers": 1,
    })
}

/// `getVisibleRanges` without folding: one range over whole lines.
fn visible_ranges(editor: &OpenEditor, lines: &[(usize, usize)]) -> Value {
    let range = |&(first, last): &(usize, usize)| json!({ "startLineNumber": first + 1, "startColumn": 1, "endLineNumber": last + 1, "endColumn": utf16_len(&editor.document().lines[last]) + 1 });
    Value::Array(lines.iter().map(range).collect())
}

/// The cursors' `ISelection`s, primary first, 1-based with UTF-16 columns as the text model counts them.
fn selections(editor: &OpenEditor) -> Value {
    let lines = &editor.document().lines;
    let column = |position: Position| utf16_len(&lines[position.line].chars().take(position.column).collect::<String>()) + 1;
    let selection = |&Selection { anchor, head }: &Selection| {
        json!({ "selectionStartLineNumber": anchor.line + 1, "selectionStartColumn": column(anchor), "positionLineNumber": head.line + 1, "positionColumn": column(head) })
    };
    Value::Array(editor.selections.iter().map(selection).collect())
}

/// `TextModelEditSource.metadata` (textModelEditSource.ts).
fn detailed_reason(reason: EditReason) -> Value {
    match reason {
        EditReason::Cursor { kind, detailed_source } => json!({ "source": "cursor", "kind": kind, "detailedSource": detailed_source }),
        EditReason::Unknown(name) => json!({ "source": "unknown", "name": name }),
        EditReason::ApplyEdits => json!({ "source": "applyEdits" }),
        EditReason::EolChange => json!({ "source": "eolChange" }),
    }
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The tab's id in `$acceptTabOperation`, which `$moveTab` and `$closeTab` name a tab by.
pub fn tab_id(group: u32, editor: &OpenEditor) -> String {
    // Every scheme the file service reads opens as a file editor input; untitled text has its own.
    let type_id = if editor.document().scheme == "untitled" { "workbench.editors.untitledEditorInput" } else { "workbench.editors.files.fileEditorInput" };
    format!("{group}~default-{type_id}-{} ", editor.document().uri_string())
}

fn tab_dto(group: u32, editor: &OpenEditor, active: bool, sync: &HostSync) -> Value {
    let (search_tabs, webview_tabs) = (&sync.search_tabs, &sync.webview_tabs);
    let uri = editor.document().uri();
    let label = match editor.document().scheme.as_str() {
        "untitled" => crate::untitled::name(editor, sync.untitled_names_from_content),
        _ => editor.document().name(),
    };
    let mut tab = json!({
        "id": tab_id(group, editor),
        "label": label,
        // TabInputKind.TextInput
        "input": { "kind": 1, "uri": uri },
        "editorId": "default",
        "isActive": active,
        "isPinned": false,
        "isPreview": editor.preview,
        "isDirty": editor.is_dirty(),
    });
    // A search editor is no text resource input: TabInputKind.UnknownInput, its editor id is its type id.
    if let Some(search) = search_tabs.get(&editor.document().path).filter(|_| editor.document().scheme == "search-editor") {
        let type_id = "workbench.editorinputs.searchEditorInput";
        tab["id"] = json!(format!("{group}~{type_id}-{type_id}-{} ", search.resource));
        tab["label"] = json!(search.label);
        tab["input"] = json!({ "kind": 0 });
        tab["editorId"] = json!(type_id);
    }
    // mainThreadEditorTabs.ts: TabInputKind.NotebookInput, its editor id is the notebook type.
    if editor.document().scheme == NOTEBOOK_EDITOR_SCHEME
        && let Some(model) = sync.notebooks.model(&editor.document().path.to_string_lossy())
    {
        let resource = model.uri_string();
        tab["id"] = json!(format!("{group}~{}-workbench.editor.notebook-{resource} ", model.view_type));
        tab["label"] = json!(crate::notebook::label(&model.uri));
        tab["input"] = json!({ "kind": 4, "notebookType": model.view_type, "uri": model.uri });
        tab["editorId"] = json!(model.view_type);
        tab["isDirty"] = json!(model.dirty);
        // TabInputKind.InteractiveEditorInput of an interactive window.
        if let Some(interactive) = sync.notebooks.interactives.iter().find(|interactive| interactive.notebook == resource && !interactive.repl) {
            tab["id"] = json!(format!("{group}~workbench.input.interactive-interactive-{resource} "));
            tab["label"] = json!(crate::notebook::interactive_name(&model.uri, interactive));
            tab["input"] = json!({ "kind": 9, "uri": model.uri, "inputBoxUri": interactive.input });
            tab["editorId"] = json!("interactive");
        }
        // A REPL editor: ReplEditorInput is a NotebookEditorInput of type `jupyter-notebook` and editor `repl`.
        if let Some(interactive) = sync.notebooks.interactives.iter().find(|interactive| interactive.notebook == resource && interactive.repl) {
            tab["id"] = json!(format!("{group}~repl-workbench.editorinputs.replEditorInput-{resource} "));
            tab["label"] = json!(crate::notebook::interactive_name(&model.uri, interactive));
            tab["input"] = json!({ "kind": 4, "notebookType": "jupyter-notebook", "uri": model.uri });
            tab["editorId"] = json!("repl");
        }
    }
    // mainThreadEditorTabs.ts: TabInputKind.WebviewEditorInput or CustomEditorInput, its editor id is the view type.
    if let Some(webview) = webview_tabs.get(&editor.document().path) {
        let type_id = if webview.custom_resource.is_some() { "workbench.editors.webviewEditor" } else { "workbench.editors.webviewInput" };
        tab["id"] = json!(format!("{group}~{}-{type_id}-{} ", webview.view_type, editor.document().uri_string()));
        tab["label"] = json!(webview.label);
        tab["input"] = match &webview.custom_resource {
            Some(uri) => json!({ "kind": 6, "viewType": webview.view_type, "uri": uri }),
            None if !webview.view_type.starts_with("mainThreadWebview-") => json!({ "kind": 0 }),
            None => json!({ "kind": 7, "viewType": webview.view_type }),
        };
        tab["editorId"] = json!(webview.view_type);
    }
    tab
}

/// mainThreadEditorTabs.ts for a `DiffEditorInput` of text: TabInputKind.TextDiffInput, the id from both sides'
/// resources (`supportSideBySide: BOTH`, modified first), dirty when the modified model is.
fn diff_tab_dto(group: u32, editor: &OpenEditor, diff: &DiffInput, active: bool, dirty: bool) -> Value {
    let (original, modified) = (uri_string(&diff.original.scheme, &diff.original.path), uri_string(&diff.modified.scheme, &diff.modified.path));
    let uri = |side: &crate::diff_editor::Side| uri_value(&side.scheme, &side.path);
    json!({
        "id": format!("{group}~default-workbench.editors.diffEditorInput-{modified}-{original} "),
        "label": diff.name(),
        "input": { "kind": 2, "original": uri(&diff.original), "modified": uri(&diff.modified) },
        "editorId": "default",
        "isActive": active,
        "isPinned": false,
        "isPreview": editor.preview,
        "isDirty": dirty,
    })
}

/// What a search editor's tab shows the extension host: `SearchEditorInput.getName` and its resource, the backing
/// file's URI or the model's.
#[derive(Clone, PartialEq, Debug)]
pub struct SearchTab {
    pub label: String,
    pub resource: String,
}

pub struct WebviewTab {
    pub label: String,
    /// `mainThreadWebview-<viewType>`, a custom editor's own view type.
    pub view_type: String,
    /// A custom editor's resource.
    pub custom_resource: Option<Value>,
    /// A custom text editor, whose tab is its resource's text model.
    pub custom_text: bool,
}


#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{Receiver, channel};

    use super::*;
    use crate::editor::{Indentation, TextDocument, Viewport};

    fn document(path: &str, language_id: &str) -> TextDocument {
        TextDocument {
            scheme: "file".into(),
            path: PathBuf::from(path),
            lines: vec!["héllo".into(), String::new()],
            eol: "\n",
            encoding: "utf8".into(),
            language_id: language_id.into(),
            indentation: Indentation { tab_size: 4, indent_size: None, insert_spaces: true },
            off_side: false,
            folding_markers: None,
            block_comment: None,
            too_large_for_tokenization: false,
            tokens: Default::default(),
        }
    }

    /// The messages sent, without the view columns (`$acceptEditorPositionData`), which one test checks.
    fn drain(receiver: &Receiver<HostMessage>) -> Vec<HostMessage> {
        receiver.try_iter().filter(|message| message.method != "$acceptEditorPositionData").collect()
    }

    /// uriIdentityService.test.ts 'extUri (isEqual)' with the disk's path casing ignored.
    #[test]
    fn a_file_in_another_case_finds_the_editor_opened_first() {
        let mut group = EditorGroup::default();
        group.open(document("/w/Readme.md", "markdown"), false);
        assert_eq!(group.find(std::path::Path::new("/w/README.md")), Some(0));
        assert_eq!(group.find(std::path::Path::new("/w/Readme.md")), Some(0));
        assert_eq!(group.find(std::path::Path::new("/W2/readme.md")), None);
    }

    #[test]
    fn sends_documents_editors_tabs_and_language_activation_as_they_change() {
        let (sender, receiver) = channel();
        let mut sync = HostSync::new(sender);
        let mut group = EditorGroup::default();
        group.viewport = Viewport { width: 400., height: 360., char_width: 7.2, line_height: 18., sticky_scroll_lines: 5, top_whitespace: 0., stop_rendering_line_after: None };

        group.open(document("/w/a b.txt", "plaintext"), false);
        sync.send_changes(&group);
        let sent = drain(&receiver);
        let methods: Vec<_> = sent.iter().map(|message| message.method).collect();
        assert_eq!(methods, ["$activateByEvent", "$activateByEvent", "$acceptDocumentsAndEditorsDelta", "$acceptTabOperation"]);
        assert_eq!(sent[0].args, [json!("onLanguage:plaintext"), json!(0)]);
        let delta = &sent[2].args[0];
        assert_eq!(delta["addedDocuments"][0]["lines"], json!(["héllo", ""]));
        assert_eq!(delta["addedEditors"][0]["id"], "vs.editor.ICodeEditor:1,$model1");
        assert_eq!(delta["addedEditors"][0]["visibleRanges"][0]["endLineNumber"], 2);
        assert_eq!(delta["newActiveEditor"], "vs.editor.ICodeEditor:1,$model1");
        assert!(delta.get("removedEditors").is_none());
        let operation = &sent[3].args[0];
        assert_eq!((operation["kind"].clone(), operation["index"].clone()), (json!(TAB_OPEN), json!(0)));
        let tab = &operation["tabDto"];
        assert_eq!(tab["id"], "0~default-workbench.editors.files.fileEditorInput-file:///w/a%20b.txt ");
        assert_eq!(tab["isActive"], true);

        // A second document of the same language needs no activation; the editor switches models.
        group.open(document("/w/b.txt", "plaintext"), true);
        group.active_mut().unwrap().move_to(Position::new(0, 2), false);
        sync.send_changes(&group);
        let sent = drain(&receiver);
        let methods: Vec<_> = sent.iter().map(|message| message.method).collect();
        assert_eq!(methods, ["$acceptDocumentsAndEditorsDelta", "$acceptTabOperation", "$acceptTabOperation"]);
        let delta = &sent[0].args[0];
        assert_eq!(delta["removedEditors"], json!(["vs.editor.ICodeEditor:1,$model1"]));
        assert_eq!(delta["addedEditors"][0]["selections"][0]["positionColumn"], 3);
        assert_eq!((sent[1].args[0]["kind"].clone(), sent[1].args[0]["tabDto"]["isActive"].clone()), (json!(TAB_UPDATE), json!(false)));
        assert_eq!((sent[2].args[0]["kind"].clone(), sent[2].args[0]["index"].clone()), (json!(TAB_OPEN), json!(1)));
        assert_eq!(sent[2].args[0]["tabDto"]["isPreview"], true);

        group.close(1);
        sync.send_changes(&group);
        let sent = drain(&receiver);
        assert_eq!((sent[1].args[0]["kind"].clone(), sent[1].args[0]["index"].clone()), (json!(TAB_CLOSE), json!(1)));
        assert_eq!((sent[2].args[0]["kind"].clone(), sent[2].args[0]["tabDto"]["isActive"].clone()), (json!(TAB_UPDATE), json!(true)));
        let delta = &sent[0].args[0];
        assert_eq!(delta["removedDocuments"][0]["path"], "/w/b.txt");
        assert_eq!(delta["newActiveEditor"], "vs.editor.ICodeEditor:1,$model1");

        group.close(0);
        sync.send_changes(&group);
        let delta = &drain(&receiver)[0].args[0];
        assert_eq!(delta["newActiveEditor"], Value::Null);
        assert!(delta.get("addedEditors").is_none());
    }

    #[test]
    fn a_split_sends_both_groups_with_an_editor_each_on_one_document() {
        let (sender, receiver) = channel();
        let mut sync = HostSync::new(sender);
        let mut group = EditorGroup::default();
        group.open(document("/w/a.txt", "plaintext"), false);
        sync.send_changes(&group);
        drain(&receiver);
        let second = group.add_group(true);
        group.activate_group(second);
        group.columns = vec![0, second];
        sync.send_changes(&group);
        let mut sent: Vec<HostMessage> = receiver.try_iter().collect();
        let methods: Vec<_> = sent.iter().map(|message| message.method).collect();
        assert_eq!(methods, ["$acceptDocumentsAndEditorsDelta", "$acceptEditorPositionData", "$acceptEditorTabModel"]);
        // mainThreadEditors.ts `_getTextEditorPositionData`: both editors with their group's column.
        let mut columns: Vec<Value> = sent.remove(1).args[0].as_object().unwrap().values().cloned().collect();
        columns.sort_by_key(|column| column.as_u64());
        assert_eq!(columns, [json!(0), json!(1)]);
        let delta = &sent[0].args[0];
        assert!(delta.get("addedDocuments").is_none());
        assert_eq!(delta["addedEditors"].as_array().unwrap().len(), 1);
        assert_eq!(delta["addedEditors"][0]["editorPosition"], 1);
        assert_eq!(delta["newActiveEditor"], delta["addedEditors"][0]["id"]);
        let model = &sent[1].args[0];
        assert_eq!((model[0]["groupId"].clone(), model[0]["isActive"].clone(), model[1]["groupId"].clone(), model[1]["viewColumn"].clone()), (json!(0), json!(false), json!(second), json!(1)));
        assert_eq!(model[1]["tabs"][0]["id"], format!("{second}~default-workbench.editors.files.fileEditorInput-file:///w/a.txt "));

        group.close(0);
        group.activate_group(0);
        group.remove_group(second);
        group.columns = Vec::new();
        sync.send_changes(&group);
        let sent = drain(&receiver);
        assert_eq!(sent[0].args[0]["removedEditors"].as_array().unwrap().len(), 1);
        assert!(sent[0].args[0].get("removedDocuments").is_none());
        assert_eq!(sent[1].method, "$acceptEditorTabModel");
    }

    #[test]
    fn sends_content_changes_with_the_dirty_state_when_it_flips() {
        let (sender, receiver) = channel();
        let mut sync = HostSync::new(sender);
        let mut group = EditorGroup::default();
        group.open(document("/w/a.txt", "plaintext"), true);
        sync.send_changes(&group);
        drain(&receiver);
        let editor = group.active_mut().unwrap();
        editor.move_to(Position::new(0, 5), false);
        let change = editor.type_text("!").unwrap();
        sync.send_content_change(&group, group.active_index().unwrap(), &change);
        let sent = drain(&receiver);
        let methods: Vec<_> = sent.iter().map(|message| message.method).collect();
        assert_eq!(methods, ["$acceptModelChanged", "$acceptDirtyStateChanged", "$acceptTabOperation", "$acceptEditorPropertiesChanged"]);
        let event = &sent[0].args[1];
        assert_eq!(event["changes"][0]["range"], json!({ "startLineNumber": 1, "startColumn": 6, "endLineNumber": 1, "endColumn": 6 }));
        assert_eq!((event["changes"][0]["rangeOffset"].clone(), event["versionId"].clone()), (json!(5), json!(2)));
        assert_eq!(event["detailedReason"], json!({ "source": "cursor", "kind": "type", "detailedSource": "keyboard" }));
        assert_eq!(sent[0].args[2], true);
        let tab = &sent[2].args[0]["tabDto"];
        assert_eq!((tab["isDirty"].clone(), tab["isPreview"].clone()), (json!(true), json!(false)));
        assert_eq!(sent[3].args[1]["selections"]["selections"][0]["positionColumn"], 7);

        // Still dirty: no dirty state or tab update.
        let change = group.active_mut().unwrap().type_text("?").unwrap();
        sync.send_content_change(&group, group.active_index().unwrap(), &change);
        assert_eq!(drain(&receiver).len(), 2);

        let change = group.active_mut().unwrap().undo().unwrap();
        sync.send_content_change(&group, group.active_index().unwrap(), &change);
        let sent = drain(&receiver);
        assert_eq!((sent[0].args[1]["isUndoing"].clone(), sent[0].args[1]["detailedReason"].clone()), (json!(true), json!({ "source": "applyEdits" })));
        assert_eq!(sent[1].args[1], false);
    }

    /// The real host on Bun takes every message of an open, an edit and a save without an error reply,
    /// and TypeScript reports diagnostics for the text the edit made.
    #[test]
    #[ignore = "starts the extension host: VSCODE_GPUI_EXTENSIONS=<built extensions> cargo test -p app -- --ignored live_host"]
    fn live_host_takes_edits_and_typescript_sees_the_new_text() {
        use exthost::rpc::Arg;
        use exthost::scan::scan_builtin_extensions;
        use exthost::session::{Session, SessionPaths, init_data, workspace_data};
        use exthost::{ExtensionHost, HostOptions, Runtime};
        use std::time::{Duration, Instant};

        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let folder = std::env::temp_dir().join(format!("code-gpui-live-{}", std::process::id()));
        std::fs::create_dir_all(folder.join("src")).unwrap();
        std::fs::write(folder.join("src/main.ts"), "export const answer = 42;\n").unwrap();
        // Built extensions, as main.rs reads them; port/tools/pixel-check points this at the reference checkout.
        let extensions_dir = std::env::var_os("VSCODE_GPUI_EXTENSIONS").map(PathBuf::from).unwrap_or_else(|| app_root.join("extensions"));
        let extensions = scan_builtin_extensions(&extensions_dir).unwrap();
        let user_data = folder.join(".user-data");
        let workspace = config::workspace::Workspace::folder(folder.clone());
        let paths = SessionPaths { app_root: &app_root, user_data: &user_data, workspace: Some(&workspace) };
        let init = init_data(&paths, &extensions, "1.147.0");
        let entry = app_root.join("out-exthost/bootstrap-exthost.js");
        let host = ExtensionHost::start(HostOptions { runtime: Runtime::Bun, entry: &entry, extra_env: vec![], extra_args: vec![] }, &init).unwrap();
        let mut session = Session::new(host);
        session.start(&extensions, Some(workspace_data(&init, &workspace))).unwrap();

        let (sender, receiver) = channel();
        let mut sync = HostSync::new(sender);
        let forward = |session: &mut Session| {
            for message in receiver.try_iter() {
                let args = message.args.into_iter().map(Arg::Json).collect();
                session.call(message.proxy, message.method, args, Duration::from_secs(30)).unwrap_or_else(|error| panic!("{error:#}"));
            }
        };
        let diagnostics = |session: &Session| session.log.host_calls.get("MainThreadDiagnostics.$changeMany").copied().unwrap_or(0);
        let pump_until = |session: &mut Session, seconds: u64, done: &dyn Fn(&Session) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(seconds);
            while !done(session) && Instant::now() < deadline {
                session.pump(Duration::from_millis(100)).unwrap();
            }
        };

        let languages = crate::languages::Languages::new(&extensions);
        let mut group = EditorGroup::default();
        group.open(TextDocument::read(&folder.join("src/main.ts"), &languages, &crate::default_settings().lock().unwrap()).unwrap(), false);
        sync.send_changes(&group);
        forward(&mut session);
        pump_until(&mut session, 20, &|session| session.log.activated.iter().any(|id| id == "vscode.typescript-language-features"));
        assert!(session.log.activated.iter().any(|id| id == "vscode.typescript-language-features"), "{:#?}", session.log.activation_errors);
        // Whatever TypeScript reports for the clean file comes first.
        pump_until(&mut session, 10, &|_| false);
        let before = diagnostics(&session);

        let editor = group.active_mut().unwrap();
        editor.move_to(Position::new(1, 0), false);
        let change = editor.type_text("const broken: number = 'text';\n").unwrap();
        sync.send_content_change(&group, group.active_index().unwrap(), &change);
        forward(&mut session);
        pump_until(&mut session, 60, &|session| diagnostics(session) > before);
        assert!(diagnostics(&session) > before, "no diagnostics after the edit; console: {:#?}", session.log.console);

        let was_dirty = group.active().unwrap().is_dirty();
        group.active_mut().unwrap().save().unwrap();
        sync.send_saved(&group, group.active().unwrap(), was_dirty);
        forward(&mut session);
        assert!(!session.log.console.iter().any(|line| line.starts_with("[unexpected]")), "{:#?}", session.log.console);
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn sends_the_color_theme_type_the_host_reads() {
        let (sender, receiver) = channel();
        HostSync::new(sender).send_color_theme(ThemeKind::HighContrastLight);
        let sent = drain(&receiver);
        assert_eq!((sent[0].proxy, sent[0].method), ("ExtHostTheming", "$onColorThemeChange"));
        assert_eq!(sent[0].args, [json!("hcLight")]);
    }

    #[test]
    fn sends_the_registered_language_ids_plaintext_first() {
        let (sender, receiver) = channel();
        HostSync::new(sender).send_language_ids(&crate::languages::Languages::new(&[]));
        let sent = drain(&receiver);
        assert_eq!((sent[0].proxy, sent[0].method), ("ExtHostLanguages", "$acceptLanguageIds"));
        assert_eq!(sent[0].args[0][0], json!("plaintext"));
    }

    #[test]
    fn a_search_editor_tab_has_its_name_and_an_unknown_input() {
        let (sender, receiver) = channel();
        let mut sync = HostSync::new(sender);
        let mut group = EditorGroup::default();
        group.open(TextDocument { scheme: "search-editor".into(), path: PathBuf::from("/1"), ..document("/1", "search-result") }, false);
        sync.search_tabs.insert(PathBuf::from("/1"), SearchTab { label: "Search: hello".into(), resource: "file:///w/a.code-search".into() });
        sync.send_changes(&group);
        let sent = drain(&receiver);
        let tab = &sent.iter().find(|message| message.method == "$acceptTabOperation").unwrap().args[0]["tabDto"];
        let type_id = "workbench.editorinputs.searchEditorInput";
        assert_eq!(tab["label"], "Search: hello");
        assert_eq!(tab["editorId"], type_id);
        assert_eq!(tab["input"], json!({ "kind": 0 }));
        assert_eq!(tab["id"], format!("0~{type_id}-{type_id}-file:///w/a.code-search "));
    }
}
