//! mainThreadCustomEditors.ts with contrib/customEditor: the custom editors extensions contribute, picked by
//! editorResolverService.ts and drawn as webview editors. A custom text editor's tab is its resource's text model, so
//! saving, undo, dirty state and moves are the model's; a custom document's tab is a webview panel's.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use exthost::main_thread::serialized_error;
use files::glob::{self, Options};
use futures::channel::oneshot;
use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::host_requests::uri_parts;
use super::media_preview::{media_kind, media_preview_contributions};
use super::webview_panels::{WEBVIEW_PANEL_SCHEME, WebviewPanel};
use crate::commands::Outcome;
use crate::editor::{ContentChanged, OpenEditor, TextDocument};
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Entry, Event, Item, Kind, QuickInput};

/// `DEFAULT_EDITOR_ASSOCIATION.id`: the text editor.
pub(super) const TEXT_EDITOR_ID: &str = "default";
const CONFIGURE_DEFAULT_ID: &str = "promptOpenWith.configureDefault";
const ASSOCIATIONS_SETTING: &str = "workbench.editorAssociations";
const DIFF_ASSOCIATIONS_SETTING: &str = "workbench.diffEditorAssociations";

/// editorResolverService.ts `priorityToRank`; the text editor ranks 2.
/// `hasExclusiveMatch`.
pub(super) const RANK_EXCLUSIVE: u8 = 5;
const RANK_DEFAULT: u8 = 4;
pub(super) const RANK_BUILTIN: u8 = 3;
const RANK_TEXT_EDITOR: u8 = 2;
/// `RegisteredEditorPriority.option`.
const RANK_OPTION: u8 = 1;
const RESOLVER_CACHE_KEY: &str = "editorOverrideService.cache";

#[derive(Default)]
pub struct CustomEditors {
    /// `$registerTextEditorProvider` and `$registerCustomEditorProvider`, by view type.
    providers: HashMap<String, Provider>,
    /// Opens waiting for their view type's provider, which `onCustomEditor:<viewType>` registers.
    pending: Vec<PendingOpen>,
    /// Opens answered once their editor's `$resolveCustomEditor` settles (WebviewEditor.setInput awaits `resolveWebview`).
    opening: HashMap<String, Vec<oneshot::Sender<Result<Value, Value>>>>,
    next_handle: u64,
    /// The custom documents the host created, by resource and view type (custom_documents.rs).
    pub(super) documents: Vec<super::custom_documents::CustomDocument>,
}

struct Provider {
    extension: Value,
    /// `IWebviewPanelOptions`.
    options: Value,
    text: bool,
    supports_move: bool,
}

struct PendingOpen {
    view_type: String,
    resource: Value,
    options: Value,
    reply: oneshot::Sender<Result<Value, Value>>,
}

/// The open with picker's resource, the active editor's id, and the editor id of each item handle.
#[derive(Clone)]
pub struct OpenWithPicker {
    resource: Value,
    current: String,
    ids: Vec<String>,
    configure_default: bool,
}

/// The custom editor a webview panel shows.
pub(super) struct CustomEditor {
    pub(super) resource: Value,
    /// `$resolveCustomEditor` was sent for the panel's handle.
    pub(super) resolved: bool,
    /// The backup a restored editor creates its document from.
    pub(super) backup_id: Option<String>,
}

impl Workbench {
    pub(crate) fn handle_custom_editors_request(&mut self, method: &str, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let arg = |index: usize| args.get(index).cloned().unwrap_or(Value::Null);
        let view_type = arg(1).as_str().unwrap_or_default().to_owned();
        match method {
            "$registerTextEditorProvider" | "$registerCustomEditorProvider" => {
                if self.webview_panels.custom_editors.providers.contains_key(&view_type) {
                    return Outcome::Ready(Err(serialized_error(&format!("Provider for {view_type} already registered"))));
                }
                let provider = Provider { extension: arg(0), options: arg(2), text: method == "$registerTextEditorProvider", supports_move: arg(3)["supportsMove"] == true };
                self.webview_panels.custom_editors.providers.insert(view_type, provider);
                self.open_pending_custom_editors(window, cx);
            }
            "$unregisterEditorProvider" => {
                self.webview_panels.custom_editors.providers.remove(arg(0).as_str().unwrap_or_default());
            }
            "$onDidEdit" => self.push_custom_document_edit(&arg(0), &view_type, arg(2).as_i64().unwrap_or_default(), cx),
            "$onContentChange" => self.change_custom_document_content(&arg(0), &view_type, cx),
            _ => {}
        }
        Outcome::done()
    }

    /// `_workbench.open` and `_workbench.openWith` through editorResolverService.ts: without an override the custom
    /// editor that ranks above the text editor opens the resource; `default` is the text editor.
    pub(super) fn open_with_resolver(&mut self, resource: &Value, override_id: Option<&str>, options: Value, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let view_type = match (override_id, uri_parts(resource)) {
            (Some("default"), _) | (None, Err(_)) => None,
            (Some(view_type), _) => Some(view_type.to_owned()),
            (None, Ok((scheme, path))) => self.custom_editor_for(scheme, &path),
        };
        match view_type {
            Some(view_type) if self.is_notebook_type(&view_type) => {
                self.detach_custom_editors(resource, None);
                self.open_notebook(resource.clone(), view_type, options, window, cx)
            }
            Some(view_type) => match media_kind(&view_type) {
                Some(kind) => {
                    self.detach_custom_editors(resource, None);
                    self.open_media_preview(kind, &view_type, resource, &options, window, cx)
                }
                None => self.open_custom_editor(view_type, resource.clone(), options, window, cx),
            },
            None => {
                self.detach_custom_editors(resource, None);
                self.api_open_editor_resolving(vec![resource.clone(), json!([null, options])], window, cx)
            }
        }
    }

    /// editorResolverService.ts `getDefaultEditorSelection` for a resource: the view type of the custom editor that
    /// opens it, None for the text editor.
    // shortcut: of two default editors for one resource the first opens, where upstream also asks which to use.
    pub(super) fn custom_editor_for(&self, scheme: &str, path: &Path) -> Option<String> {
        let (view_type, _) = self.default_editor(scheme, path, true, false);
        (view_type != TEXT_EDITOR_ID).then_some(view_type)
    }

    /// editorResolverService.ts `cacheEditors` on `onWillSaveState`: the globs of every editor but the optional ones,
    /// and of the user's associations, whose resources the next start opens only after the extensions registered.
    pub(super) fn store_editor_resolver_cache(&self) {
        let mut patterns: Vec<String> = Vec::new();
        for contribution in self.editor_contributions().into_iter().filter(|contribution| contribution.rank != RANK_OPTION) {
            patterns.extend(contribution.patterns);
        }
        patterns.extend(self.editor_associations().into_iter().map(|(pattern, _)| pattern));
        patterns.sort();
        patterns.dedup();
        self.profile_storage.lock().unwrap().set(RESOLVER_CACHE_KEY, Value::from(patterns).to_string());
    }

    /// The previous session's `editorOverrideService.cache`, read and removed on startup as the resolver does.
    pub fn take_editor_resolver_cache(&mut self) -> Vec<String> {
        let mut storage = self.profile_storage.lock().unwrap();
        let cache = storage.get(RESOLVER_CACHE_KEY).and_then(|raw| serde_json::from_str(raw).ok()).unwrap_or_default();
        storage.delete(RESOLVER_CACHE_KEY);
        cache
    }

    /// The command line's file on startup. editorResolverService.ts `resolveEditor` runs before the extensions
    /// registered their editors and waits for them only when the cache or a user association names the file's glob,
    /// so on a first start a `.ipynb` opens as JSON in the text editor. The media previews are registered by then.
    pub fn open_startup_file(&mut self, path: PathBuf, resolver_cache: &[String], window: &mut Window, cx: &mut Context<Self>) {
        let waits_for_extensions = resource_matcher("file", &path).is_some_and(|matches| {
            resolver_cache.iter().any(|pattern| matches(pattern))
                || self.editor_associations().iter().any(|(pattern, view_type)| view_type != TEXT_EDITOR_ID && matches(pattern))
                || media_preview_contributions().iter().flat_map(|contribution| &contribution.patterns).any(|pattern| matches(pattern))
        });
        match waits_for_extensions {
            true => self.open_file(path, true, window, cx),
            false => {
                self.open_text_file(path, true, window, cx);
                self.set_editor_type_control_before_registration();
            }
        }
    }

    /// The contributed custom editors, then the built-in media previews.
    fn editor_contributions(&self) -> Vec<Contribution> {
        let mut contributions = custom_editor_contributions(&self.extensions);
        contributions.extend(media_preview_contributions());
        contributions
    }

    /// `getEditorMatches(resource).defaultRule`: the id of the editor that opens the resource, `default` for the text
    /// editor, and the glob a new default would be written for. Without `associations` it is the natural default;
    /// with `diff`, the diff editor's (`EditorAssociationType.DiffEditor`).
    pub(super) fn default_editor(&self, scheme: &str, path: &Path, associations: bool, diff: bool) -> (String, String) {
        let extension_pattern = format!("*{}", path.extension().map(|extension| format!(".{}", extension.to_string_lossy())).unwrap_or_default());
        let text_editor = (TEXT_EDITOR_ID.to_owned(), extension_pattern.clone());
        let Some(matches) = resource_matcher(scheme, path) else { return text_editor };
        if associations && let Some((pattern, view_type)) = self.matching_associations(&matches, diff).into_iter().next() {
            return (view_type, pattern);
        }
        let mut best: Option<(u8, &str, String)> = None;
        let contributions = self.editor_contributions();
        for contribution in &contributions {
            let Some(pattern) = contribution.patterns.iter().filter(|pattern| matches(pattern)).max_by_key(|pattern| pattern.len()) else { continue };
            let rank = contribution.rank_for(diff);
            let better = best.as_ref().is_none_or(|(best_rank, best_pattern, _)| (rank, pattern.len()) > (*best_rank, best_pattern.len()));
            if rank > RANK_TEXT_EDITOR && better {
                best = Some((rank, pattern, contribution.view_type.clone()));
            }
        }
        best.map_or(text_editor, |(_, pattern, view_type)| (view_type, pattern.to_owned()))
    }

    /// customEditors.ts `UndoCommand` and `RedoCommand` implementations: a custom text editor's are its model's, a
    /// custom document's are its edits (`undoRedoService.undo(resource)`).
    pub(super) fn custom_text_editor_undo(&mut self, id: &str, command: fn(&mut OpenEditor) -> Option<ContentChanged>, cx: &mut Context<Self>) -> Option<Outcome> {
        if !matches!(id, "undo" | "redo") {
            return None;
        }
        let editor = self.editors.active()?;
        if editor.document().scheme == WEBVIEW_PANEL_SCHEME {
            let path = editor.document().path.clone();
            return Some(self.undo_custom_document(&path, id == "redo", cx));
        }
        let custom_text = self.webview_panels.panels.iter().any(|panel| panel.custom.is_some() && panel.scheme != WEBVIEW_PANEL_SCHEME && editor.document().is(&panel.scheme, &panel.path));
        custom_text.then(|| self.run_on_active_editor(command, cx))
    }

    /// editorCommands.ts `reopenEditorWith` on the active editor: `override_id` is an editor id, None asks with the
    /// open with picker.
    pub(super) fn reopen_active_editor_with(&mut self, override_id: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some((resource, current)) = self.active_editor_type() else { return Outcome::done() };
        match override_id {
            Some(editor_id) => self.reopen_with(&resource, &current, &editor_id, false, window, cx),
            None => {
                self.show_open_with_picker(resource, current, false, window, cx);
                Outcome::done()
            }
        }
    }

    /// The active editor's resource and the id of the editor showing it (`activeEditor.editorId`): a notebook's type,
    /// a custom editor's view type or the text editor; a webview panel has no resource to reopen.
    pub(super) fn active_editor_type(&self) -> Option<(Value, String)> {
        if let Some(model) = self.active_notebook_editor().and_then(|editor| self.host.notebooks.model(&editor.notebook)) {
            return Some((model.uri.clone(), model.view_type.clone()));
        }
        let editor = self.editors.active()?;
        match self.webview_panels.panels.iter().find(|panel| editor.document().is(&panel.scheme, &panel.path)) {
            Some(panel) => panel.custom.as_ref().map(|custom| (custom.resource.clone(), panel.view_type.clone())),
            None => Some((editor.document().uri(), TEXT_EDITOR_ID.to_owned())),
        }
    }

    /// `group.replaceEditors`: a text model's editor stays and gains or loses its webview; a custom document's or a
    /// saved notebook's editor closes first.
    // shortcut: the replacement opens where the group opens editors, not in the closed one's place.
    pub(super) fn reopen_with(&mut self, resource: &Value, current: &str, editor_id: &str, preserve_focus: bool, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        // binaryFileEditor.ts `openInternal`: the text editor picked for a binary file opens it as text.
        if editor_id == TEXT_EDITOR_ID
            && let Some(editor) = self.editors.active().filter(|editor| matches!(editor.document().open_error, Some(crate::editor::OpenError::Binary(_))))
        {
            let path = editor.document().path.clone();
            self.reopen_file_editor(&path, false, true, cx);
            return Outcome::done();
        }
        if current == editor_id {
            return Outcome::done();
        }
        let notebook_dirty = self.active_notebook_editor().and_then(|editor| self.host.notebooks.model(&editor.notebook)).is_some_and(|model| model.dirty);
        let replaced = |scheme: &str| scheme == WEBVIEW_PANEL_SCHEME || (scheme == crate::notebook::NOTEBOOK_EDITOR_SCHEME && !notebook_dirty);
        if let Some(index) = self.editors.active_index().filter(|&index| replaced(&self.editors.editors[index].document().scheme)) {
            self.editors.close(index);
            self.editors_changed(cx);
        }
        self.open_with_resolver(resource, Some(editor_id), json!({ "preserveFocus": preserve_focus }), window, cx)
    }

    /// editorResolverService.ts `getEditorMatches` (in a diff editor with `diff`): the editors that open the resource
    /// (by glob or a user association) by priority, then the longer glob, the text editor registering `*` as builtin;
    /// as id, label, provider, rank and whether it is an unconfigured `*` option editor
    /// (`isUnconfiguredUniversalOptionalEditorMatch`).
    pub(super) fn resource_editors(&self, scheme: &str, path: &Path, diff: bool) -> Vec<(String, String, String, u8, bool)> {
        let mut editors = vec![(RANK_BUILTIN, 1, (TEXT_EDITOR_ID.to_owned(), "Text Editor".to_owned(), "Built-in".to_owned(), RANK_BUILTIN, false))];
        if let Some(matches) = resource_matcher(scheme, path) {
            let associated: Vec<String> = self.matching_associations(&matches, diff).into_iter().map(|(_, view_type)| view_type).collect();
            for contribution in self.editor_contributions() {
                let is_associated = associated.contains(&contribution.view_type);
                let pattern = contribution.patterns.iter().filter(|pattern| matches(pattern)).max_by_key(|pattern| pattern.len());
                if !is_associated && pattern.is_none() {
                    continue;
                }
                let rank = contribution.rank_for(diff);
                let universal_option = !is_associated && rank == RANK_OPTION && pattern.is_some_and(|pattern| pattern == "*");
                let glob_length = pattern.map_or(0, String::len);
                editors.push((rank, glob_length, (contribution.view_type, contribution.display_name, contribution.provider, rank, universal_option)));
            }
        }
        editors.sort_by_key(|(rank, glob_length, _)| std::cmp::Reverse((*rank, *glob_length)));
        editors.into_iter().map(|(_, _, editor)| editor).collect()
    }

    /// `getAssociationsForResourceByType`: a diff takes `workbench.diffEditorAssociations`, else the editor
    /// associations whose editor is not `explicit` for diffs; the longest glob first.
    fn matching_associations(&self, matches: &impl Fn(&str) -> bool, diff: bool) -> Vec<(String, String)> {
        let matching = |setting| self.associations(setting).into_iter().filter(|(pattern, _)| matches(pattern)).collect::<Vec<_>>();
        if !diff {
            return matching(ASSOCIATIONS_SETTING);
        }
        let diff_associations = matching(DIFF_ASSOCIATIONS_SETTING);
        if !diff_associations.is_empty() {
            return diff_associations;
        }
        let contributions = self.editor_contributions();
        let explicit = |view_type: &str| contributions.iter().any(|contribution| contribution.view_type == view_type && contribution.diff_rank == 0);
        matching(ASSOCIATIONS_SETTING).into_iter().filter(|(_, view_type)| !explicit(view_type)).collect()
    }

    /// openWithEditorPicker.ts `doPick`: the text editor, then the resource's editors by rank, and the entry that
    /// picks a new default; `configure_default` is the picker that entry opens.
    pub(super) fn show_open_with_picker(&mut self, resource: Value, current: String, configure_default: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Ok((scheme, path)) = uri_parts(&resource) else { return };
        let (default_id, pattern) = self.default_editor(scheme, &path, true, false);
        let mut entries = Vec::new();
        let mut ids = Vec::new();
        let mut editors = self.resource_editors(scheme, &path, false);
        editors.sort_by_key(|(id, ..)| id != TEXT_EDITOR_ID);
        for (id, label, detail, _, _) in editors {
            let description = match (id == current, id == default_id) {
                (true, true) => Some("Active and Default".to_owned()),
                (true, false) => Some("Active".to_owned()),
                (false, true) => Some("Default".to_owned()),
                (false, false) => None,
            };
            entries.push(Entry::Item(Item { handle: ids.len() as i64, label, description, detail: Some(detail), pickable: true, ..Item::default() }));
            ids.push(id);
        }
        if !configure_default && path.extension().is_some() {
            entries.push(Entry::Separator(None));
            let label = format!("Configure default editor for '{pattern}'...");
            entries.push(Entry::Item(Item { handle: ids.len() as i64, label, pickable: true, ..Item::default() }));
            ids.push(CONFIGURE_DEFAULT_ID.to_owned());
        }
        let basename = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some(match configure_default {
            true => format!("Select new default editor for '{pattern}'"),
            false => format!("Select editor for '{basename}'"),
        });
        input.can_accept_in_background = true;
        input.set_items(entries);
        let id = self.quick_inputs.add(input, Owner::OpenWith(OpenWithPicker { resource, current, ids, configure_default }));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub fn open_with_picker_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Accept(in_background) => {
                let Some(input) = self.quick_inputs.get(id) else { return };
                let Some(&handle) = input.selected_handles().first() else { return };
                let Some(Owner::OpenWith(picker)) = self.quick_inputs.owner(id) else { return };
                let picker = picker.clone();
                self.hide_quick_input_by_id(id, window, cx);
                let editor_id = &picker.ids[handle as usize];
                if editor_id == CONFIGURE_DEFAULT_ID {
                    return self.show_open_with_picker(picker.resource, picker.current, true, window, cx);
                }
                if picker.configure_default {
                    self.set_default_editor(&picker.resource, editor_id, false);
                }
                drop(self.reopen_with(&picker.resource, &picker.current, editor_id, in_background, window, cx));
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }

    /// editorResolverService.ts `setDefaultEditor`: the association for the default rule's glob, removed when the
    /// editor is the one the resource opens with without it; a diff's goes to `workbench.diffEditorAssociations`.
    pub(super) fn set_default_editor(&mut self, resource: &Value, editor_id: &str, diff: bool) {
        let Ok((scheme, path)) = uri_parts(resource) else { return };
        let setting = if diff { DIFF_ASSOCIATIONS_SETTING } else { ASSOCIATIONS_SETTING };
        let (default_id, pattern) = self.default_editor(scheme, &path, true, diff);
        let (natural_id, _) = self.default_editor(scheme, &path, false, diff);
        let mut associations = match self.setting(setting) {
            Value::Object(associations) => associations,
            _ => serde_json::Map::new(),
        };
        if editor_id == natural_id {
            if associations.remove(&pattern).is_some() {
                return self.update_setting(setting, Value::Object(associations));
            }
            if default_id == editor_id {
                return;
            }
        }
        associations.insert(pattern, json!(editor_id));
        self.update_setting(setting, Value::Object(associations));
    }

    /// `workbench.editorAssociations` as glob and editor id, the longest glob first.
    // shortcut: reads the merged value where upstream reads the user's; workspace associations come with settings scopes.
    fn editor_associations(&self) -> Vec<(String, String)> {
        self.associations(ASSOCIATIONS_SETTING)
    }

    fn associations(&self, setting: &str) -> Vec<(String, String)> {
        let mut associations: Vec<(String, String)> = match self.setting(setting) {
            Value::Object(object) => object.into_iter().filter_map(|(pattern, view_type)| Some((pattern, view_type.as_str()?.to_owned()))).collect(),
            _ => Vec::new(),
        };
        associations.sort_by_key(|(pattern, _)| std::cmp::Reverse(pattern.len()));
        associations
    }

    /// Opens `resource` in the custom editor of `view_type` once its provider is registered, activating
    /// `onCustomEditor:<viewType>` for it (customEditors.ts).
    fn open_custom_editor(&mut self, view_type: String, resource: Value, options: Value, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        drop(self.host.activate_by_event(&format!("onCustomEditor:{view_type}")));
        let (reply, receiver) = oneshot::channel();
        self.webview_panels.custom_editors.pending.push(PendingOpen { view_type, resource, options, reply });
        self.open_pending_custom_editors(window, cx);
        Outcome::Pending(receiver)
    }

    fn open_pending_custom_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let custom_editors = &mut self.webview_panels.custom_editors;
        let (ready, waiting): (Vec<PendingOpen>, Vec<PendingOpen>) =
            std::mem::take(&mut custom_editors.pending).into_iter().partition(|open| custom_editors.providers.contains_key(&open.view_type));
        custom_editors.pending = waiting;
        for open in ready {
            match self.show_custom_editor(&open.view_type, &open.resource, &open.options, window, cx) {
                Ok(handle) if self.webview_panel_mut(&handle).is_some_and(|panel| panel.custom.as_ref().is_some_and(|custom| !custom.resolved)) => {
                    self.webview_panels.custom_editors.opening.entry(handle).or_default().push(open.reply);
                }
                result => _ = open.reply.send(result.map(|_| Value::Null)),
            }
        }
        self.resolve_custom_editors(cx);
    }

    /// A custom text editor attaches a webview to the resource's text editor, opened or shown as `$tryShowTextDocument`
    /// does; a custom document gets a webview panel's editor of its own, one per resource and view type.
    fn show_custom_editor(&mut self, view_type: &str, resource: &Value, options: &Value, window: &mut Window, cx: &mut Context<Self>) -> Result<String, Value> {
        let (scheme, path) = uri_parts(resource)?;
        let scheme = scheme.to_owned();
        let provider = &self.webview_panels.custom_editors.providers[view_type];
        let (text, extension, retain) = (provider.text, provider.extension.clone(), provider.options["retainContextWhenHidden"] == true);
        let find_enabled = provider.options["enableFindWidget"] == true;
        let title = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let backup_id = options["backupId"].as_str().map(str::to_owned);
        let custom = Some(CustomEditor { resource: resource.clone(), resolved: false, backup_id });
        let shows = |panel: &WebviewPanel| panel.view_type == view_type && panel.custom.as_ref().is_some_and(|custom| custom.resource == *resource);
        let handle = match self.webview_panels.panels.iter().find(|panel| shows(panel)) {
            Some(panel) => panel.handle.clone(),
            None if text => {
                self.detach_custom_editors(resource, Some(view_type));
                let handle = self.next_custom_editor_handle();
                self.webview_panels.panels.push(WebviewPanel::new(handle.clone(), scheme, path, view_type, title, &extension, Value::Null, retain, custom));
                handle
            }
            None => self.add_custom_document_editor(view_type, title, &extension, retain, custom, options["pinned"] != true),
        };
        if let Some(panel) = self.webview_panel_mut(&handle) {
            panel.find_enabled = find_enabled;
        }
        if text {
            self.try_show_text_document(resource, options, window, cx)?;
        } else {
            // shortcut: one panel per resource, so a custom editor open in another group shows there; upstream opens a
            // second editor of the resource in the active group.
            let (group, index) = self.webview_editor_group(&handle).expect("the panel's editor is open");
            self.activate_group(group);
            self.editors.activate(index);
            if options["pinned"] == true {
                self.editors.pin(index);
            }
            self.editors_changed(cx);
        }
        if options["preserveFocus"] != true {
            self.focus_webview_panel(&handle, window, cx);
        }
        Ok(handle)
    }

    /// A custom document's own editor: a webview panel with the placeholder model `webview-panel/webview-<viewType>-<handle>`.
    pub(super) fn add_custom_document_editor(&mut self, view_type: &str, title: String, extension: &Value, retain: bool, custom: Option<CustomEditor>, preview: bool) -> String {
        let handle = self.next_custom_editor_handle();
        let placeholder = PathBuf::from(format!("webview-panel/webview-{view_type}-{handle}"));
        let document = TextDocument::from_text(WEBVIEW_PANEL_SCHEME, &placeholder, "", "utf8".into(), None, &self.languages, &self.settings.lock().unwrap());
        self.webview_panels.panels.push(WebviewPanel::new(handle.clone(), WEBVIEW_PANEL_SCHEME.into(), placeholder, view_type, title, extension, Value::Null, retain, custom));
        self.editors.open(document, preview);
        handle
    }

    /// The registered provider's extension description and whether it retains hidden webviews.
    pub(super) fn custom_editor_provider(&self, view_type: &str) -> Option<(Value, bool)> {
        let provider = self.webview_panels.custom_editors.providers.get(view_type)?;
        Some((provider.extension.clone(), provider.options["retainContextWhenHidden"] == true))
    }

    /// Removes the custom text editors of `resource` other than `keep`, as reopening it with another editor does.
    fn detach_custom_editors(&mut self, resource: &Value, keep: Option<&str>) {
        let panels = std::mem::take(&mut self.webview_panels.panels);
        for panel in panels {
            let detached = panel.scheme != WEBVIEW_PANEL_SCHEME && Some(panel.view_type.as_str()) != keep && panel.custom.as_ref().is_some_and(|custom| custom.resource == *resource);
            match detached {
                true => self.host.send("ExtHostWebviewPanels", "$onDidDisposeWebviewPanel", vec![json!(panel.handle)]),
                false => self.webview_panels.panels.push(panel),
            }
        }
    }

    pub(super) fn next_custom_editor_handle(&mut self) -> String {
        self.webview_panels.custom_editors.next_handle += 1;
        format!("custom-editor-{}", self.webview_panels.custom_editors.next_handle)
    }

    /// mainThreadCustomEditors.ts `resolveWebview` for each custom editor not resolved yet: a custom document is
    /// created first, once per resource and view type. The host has the text editor's document already, since
    /// `editors_changed` sent it on the same channel.
    pub(super) fn resolve_custom_editors(&mut self, cx: &mut Context<Self>) {
        let active = self.webview_panels.panels.iter().position(|panel| self.editors.active().is_some_and(|editor| editor.document().is(&panel.scheme, &panel.path)));
        let providers = &self.webview_panels.custom_editors.providers;
        let mut resolves = Vec::new();
        for index in 0..self.webview_panels.panels.len() {
            let panel = &self.webview_panels.panels[index];
            let Some(provider) = providers.get(&panel.view_type) else { continue };
            let Some(custom) = panel.custom.as_ref().filter(|custom| !custom.resolved) else { continue };
            let document_open = self.webview_panels.panels.iter().any(|other| {
                other.view_type == panel.view_type && other.custom.as_ref().is_some_and(|other| other.resolved && other.resource == custom.resource)
            });
            let create = (!provider.text && !document_open).then(|| vec![custom.resource.clone(), json!(panel.view_type), json!(custom.backup_id), Value::Null]);
            let init = json!({ "title": panel.title, "contentOptions": {}, "options": provider.options, "active": active == Some(index) });
            // EditorGroupColumn 0, the one group.
            let resolve = vec![custom.resource.clone(), json!(panel.handle), json!(panel.view_type), init, json!(0)];
            resolves.push((panel.handle.clone(), panel.view_type.clone(), create, resolve));
            self.webview_panels.panels[index].custom.as_mut().expect("checked above").resolved = true;
        }
        for (handle, view_type, create, resolve) in resolves {
            let document = create.as_ref().map(|args| (args[0].clone(), args[2].is_string()));
            let created = create.map(|args| self.host.call("ExtHostCustomEditors", "$createCustomDocument", args));
            // mainThreadCustomEditors.ts resolves the editor once `$createCustomDocument` answered: the host's
            // `$resolveCustomEditor` looks the document up.
            cx.spawn(async move |workbench, cx| {
                let canceled = |_| Err(serialized_error("Canceled"));
                let result = match created {
                    Some(created) => created.await.unwrap_or_else(canceled),
                    None => Ok(Value::Null),
                };
                if let (Ok(created), Some((resource, from_backup))) = (&result, document) {
                    let editable = created["editable"] == true;
                    let view_type = view_type.clone();
                    let _ = workbench.update(cx, |workbench, cx| workbench.custom_document_created(resource, view_type, editable, from_backup, cx));
                }
                let result = match result {
                    Ok(_) => match workbench.update(cx, |workbench, _| workbench.host.call("ExtHostCustomEditors", "$resolveCustomEditor", resolve)) {
                        Ok(resolved) => resolved.await.unwrap_or_else(canceled),
                        Err(_) => return,
                    },
                    Err(error) => Err(error),
                };
                let _ = workbench.update(cx, |workbench, _| {
                    if let Err(error) = result {
                        eprintln!("[custom editor] {view_type}: {error}");
                        workbench.show_resolve_failure(&handle, &view_type);
                    }
                    for reply in workbench.webview_panels.custom_editors.opening.remove(&handle).unwrap_or_default() {
                        let _ = reply.send(Ok(Value::Null));
                    }
                });
            })
            .detach();
        }
    }

    /// mainThreadWebviews.ts `getWebviewResolvedFailedContent`.
    fn show_resolve_failure(&mut self, handle: &str, view_type: &str) {
        let Some(panel) = self.webview_panel_mut(handle) else { return };
        let escaped = view_type.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;");
        panel.html = format!(
            "<!DOCTYPE html>\n\t\t<html>\n\t\t\t<head>\n\t\t\t\t<meta http-equiv=\"Content-type\" content=\"text/html;charset=UTF-8\">\n\t\t\t\t<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none';\">\n\t\t\t</head>\n\t\t\t<body>An error occurred while loading view: {escaped}</body>\n\t\t</html>"
        );
        panel.update_content();
    }

    /// Custom text editors whose resource `move_models` moved: with `moveCustomTextEditor` the provider follows it
    /// (`$onMoveCustomEditor`), else the editor reopens at the target (`CustomEditorInput.rename`) in a new webview,
    /// which `resolve_custom_editors` resolves once the host has the moved document.
    pub(super) fn move_custom_editors(&mut self, source_scheme: &str, source: &Path, target_scheme: &str, target: &Path) {
        for index in 0..self.webview_panels.panels.len() {
            let panel = &self.webview_panels.panels[index];
            let Some(rest) = panel.path.strip_prefix(source).ok().filter(|_| panel.scheme == source_scheme && panel.custom.is_some()) else { continue };
            let path = if rest.as_os_str().is_empty() { target.to_path_buf() } else { target.join(rest) };
            let Some(resource) = self.editors.editors.iter().find(|editor| editor.document().is(target_scheme, &path)).map(|editor| editor.document().uri()) else { continue };
            let supports_move = self.webview_panels.custom_editors.providers.get(&panel.view_type).is_some_and(|provider| provider.supports_move);
            let new_handle = (!supports_move).then(|| self.next_custom_editor_handle());
            let panel = &mut self.webview_panels.panels[index];
            panel.scheme = target_scheme.to_owned();
            panel.title = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            panel.path = path;
            let custom = panel.custom.as_mut().expect("checked above");
            custom.resource = resource.clone();
            match new_handle {
                None => self.host.send("ExtHostCustomEditors", "$onMoveCustomEditor", vec![json!(panel.handle), resource, json!(panel.view_type)]),
                Some(handle) => {
                    let old_handle = std::mem::replace(&mut panel.handle, handle);
                    custom.resolved = false;
                    panel.webview = None;
                    panel.html.clear();
                    panel.state = Value::Null;
                    self.host.send("ExtHostWebviewPanels", "$onDidDisposeWebviewPanel", vec![json!(old_handle)]);
                }
            }
        }
    }
}

/// editorResolverService.ts `globMatchesResource` for one resource.
pub(super) fn resource_matcher(scheme: &str, path: &Path) -> Option<impl Fn(&str) -> bool> {
    // These schemes match no glob.
    if matches!(scheme, "extension" | WEBVIEW_PANEL_SCHEME | "vscode-workspace-trust" | "vscode-settings") {
        return None;
    }
    let full_path = format!("{scheme}:{}", path.display());
    let basename = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    Some(move |pattern: &str| {
        let target = if pattern.contains('/') { full_path.as_str() } else { basename.as_str() };
        glob::match_pattern(pattern, target, Options { ignore_case: true, ..Options::default() })
    })
}

/// A `contributes.customEditors` entry as contributedCustomEditors.ts reads it.
pub(super) struct Contribution {
    pub(super) view_type: String,
    pub(super) display_name: String,
    /// `providerDisplayName`.
    pub(super) provider: String,
    /// `selector[].filenamePattern`.
    pub(super) patterns: Vec<String>,
    /// `priorityToRank` of `getPriorityFromContribution`.
    pub(super) rank: u8,
    /// The rank in a diff editor: `priority.diffEditor`, `explicit` when only `textEditor` is given.
    pub(super) diff_rank: u8,
}

impl Contribution {
    fn rank_for(&self, diff: bool) -> u8 {
        if diff { self.diff_rank } else { self.rank }
    }
}

/// `getSinglePriorityFromContribution` as a rank.
fn rank_of(priority: Option<&str>, is_builtin: bool) -> u8 {
    match priority {
        Some("exclusive") => RANK_EXCLUSIVE,
        Some("option") => RANK_OPTION,
        Some("explicit") => 0,
        Some("builtin") if is_builtin => RANK_BUILTIN,
        _ => RANK_DEFAULT,
    }
}

fn custom_editor_contributions(extensions: &[exthost::scan::ScannedExtension]) -> Vec<Contribution> {
    let mut contributions = Vec::new();
    for extension in extensions {
        let is_builtin = extension.description["isBuiltin"] == true;
        let provider = match is_builtin {
            true => "Built-in".to_owned(),
            false => extension.description["displayName"].as_str().map_or(extension.id.clone(), str::to_owned),
        };
        for contribution in extension.description["contributes"]["customEditors"].as_array().into_iter().flatten() {
            let Some(view_type) = contribution["viewType"].as_str() else { continue };
            let priority = &contribution["priority"];
            let rank = rank_of(priority.as_str().or(priority["textEditor"].as_str()), is_builtin);
            let diff_rank = match priority.as_str() {
                Some(_) => rank,
                None => priority["diffEditor"].as_str().map_or(0, |diff| rank_of(Some(diff), is_builtin)),
            };
            let selectors = contribution["selector"].as_array().into_iter().flatten();
            let patterns = selectors.filter_map(|selector| selector["filenamePattern"].as_str()).map(str::to_owned).collect();
            let display_name = contribution["displayName"].as_str().unwrap_or_default().to_owned();
            contributions.push(Contribution { view_type: view_type.to_owned(), display_name, provider: provider.clone(), patterns, rank, diff_rank });
        }
        // notebookServiceImpl.ts `_registerContributionPoint`: each notebook type is an editor, `default` unless it says otherwise.
        for contribution in extension.description["contributes"]["notebooks"].as_array().into_iter().flatten() {
            let Some(view_type) = contribution["type"].as_str() else { continue };
            let rank = match contribution["priority"].as_str() {
                None | Some("default") => RANK_DEFAULT,
                Some(_) => RANK_OPTION,
            };
            let selectors = contribution["selector"].as_array().into_iter().flatten();
            let patterns = selectors.filter_map(|selector| selector["filenamePattern"].as_str().or(selector["filenamePattern"]["include"].as_str())).map(str::to_owned).collect();
            let display_name = contribution["displayName"].as_str().unwrap_or_default().to_owned();
            contributions.push(Contribution { view_type: view_type.to_owned(), display_name, provider: provider.clone(), patterns, rank, diff_rank: rank });
        }
    }
    contributions
}

#[cfg(test)]
mod tests {
    use super::*;
    use exthost::scan::ScannedExtension;

    fn extension(builtin: bool, custom_editors: Value) -> ScannedExtension {
        let description = json!({ "isBuiltin": builtin, "displayName": "Hex", "contributes": { "customEditors": custom_editors } });
        ScannedExtension { id: "pub.hex".into(), location: PathBuf::new(), description }
    }

    #[test]
    fn ranks_contributions_as_contributed_custom_editors_does() {
        let editors = json!([
            { "viewType": "a", "displayName": "A", "selector": [{ "filenamePattern": "*.png" }] },
            { "viewType": "b", "priority": "option", "selector": [{ "filenamePattern": "*.png" }, { "filenamePattern": "**/x/*.bin" }] },
            { "viewType": "c", "priority": "builtin" },
            { "viewType": "d", "priority": { "textEditor": "explicit" } },
        ]);
        let ranks = |builtin| custom_editor_contributions(&[extension(builtin, editors.clone())]).into_iter().map(|contribution| contribution.rank).collect::<Vec<_>>();
        assert_eq!(ranks(false), [RANK_DEFAULT, 1, RANK_DEFAULT, 0]);
        assert_eq!(ranks(true), [RANK_DEFAULT, 1, RANK_BUILTIN, 0]);
        let contributions = custom_editor_contributions(&[extension(false, editors)]);
        assert_eq!((contributions[1].provider.as_str(), contributions[1].patterns.len()), ("Hex", 2));
    }

    #[test]
    fn matches_globs_as_glob_matches_resource_does() {
        let matches = resource_matcher("file", Path::new("/w/x/Image.PNG")).expect("file resources match globs");
        assert!(matches("*.png"));
        assert!(matches("**/x/*.png"));
        assert!(!matches("*.bin"));
        assert!(resource_matcher(WEBVIEW_PANEL_SCHEME, Path::new("webview-panel/a.png")).is_none());
    }
}
