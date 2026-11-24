//! Quick Open's file picks (anythingQuickAccess.ts) in the workbench: the editor history of historyService.ts, the
//! file list a search loads once per picker (FileQueryCacheState), and opening the accepted file.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use config::configuration::Overrides;
use gpui::{AppContext, Context, Task, Window};
use search::ripgrep::Cancellation;
use serde_json::Value;
use storage::Storage;

use super::Workbench;
use super::quick_access::Accept;
use crate::anything_quick_access::{self, FileEntry, FilePick, TextRange};
use crate::editor::{Position, Selection, VerticalReveal};
use crate::editor_history::{EditorHistory, HISTORY_STORAGE_KEY};
use crate::fuzzy_scorer::{self, FuzzyScorerCache};
use crate::quick_input::{Button, Entry, Item};

pub const PLACEHOLDER: &str = "Search files by name (append : to go to line or @ to go to symbol), or paste a link";
/// `SYMBOL_PICKS_MERGE_DELAY`: how long the history picks wait for the file results before showing alone.
const MERGE_DELAY: Duration = Duration::from_millis(200);

/// `pickState` of one Quick Open picker.
pub struct FilePickState {
    pub input: i64,
    last_original_filter: Option<String>,
    last_filter: Option<String>,
    pub last_range: Option<TextRange>,
    scorer_cache: FuzzyScorerCache,
    /// The workspace files; `None` while the search loads them.
    files: Option<Vec<FileEntry>>,
    merge_delay_passed: bool,
    cancellation: Arc<Cancellation>,
    _load: Task<()>,
}

impl Drop for FilePickState {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl Workbench {
    /// Switches to the folder's workspace storage and reads the editor history from it.
    pub fn restore_workspace_state(&mut self, storage: Arc<Mutex<Storage>>) {
        self.workspace_storage = storage;
        let stored = self.workspace_storage.lock().unwrap().get(HISTORY_STORAGE_KEY).map(str::to_owned);
        let excluded = self.history_exclude_matcher();
        self.quick_access.editor_history = EditorHistory::load(stored.as_deref(), |path| !excluded(path));
        self.restore_find_state();
    }

    /// `handleActiveEditorChangeInHistory`. Upstream saves on shutdown only; the port saves on each change, as its
    /// storage writes synchronously and the app can exit without a shutdown phase.
    pub(super) fn record_editor_history(&mut self) {
        let Some(editor) = self.editors.active() else { return };
        let path = editor.document.path.clone();
        if self.quick_access.editor_history.entries.first() == Some(&path) || self.history_exclude_matcher()(&path) {
            return;
        }
        self.quick_access.editor_history.add(&path);
        self.save_editor_history();
    }

    pub(super) fn remove_from_editor_history(&mut self, path: &Path) {
        if self.quick_access.editor_history.remove(path) {
            self.save_editor_history();
        }
    }

    fn save_editor_history(&self) {
        let serialized = self.quick_access.editor_history.serialize();
        self.workspace_storage.lock().unwrap().set(HISTORY_STORAGE_KEY, serialized);
    }

    /// `includeInHistory`: `ExcludeResourceGlobMatcher` over `files.exclude` and `search.exclude`, matching the path
    /// relative to its workspace folder with that folder's settings, and the absolute path too when a pattern is
    /// absolute; a path outside every folder matches the global settings by its absolute path.
    fn history_exclude_matcher(&self) -> impl Fn(&Path) -> bool + use<> {
        let workspace = self.workspace().cloned();
        let settings = self.settings.lock().unwrap();
        let matcher_for = |root: Option<&Path>| {
            let setting = |_: &Path, key: &str| settings.get_value(Some(key), &Overrides { resource: root, ..Overrides::default() }).unwrap_or(Value::Null);
            let expression = search::query::excludes(root.unwrap_or(Path::new("/")), &setting, true).unwrap_or_default();
            let has_absolute_path = expression.keys().any(|key| key.starts_with('/'));
            (files::glob::parse_exclude_expression(&expression, files::glob::Options::default()), has_absolute_path)
        };
        let folder_matchers: Vec<_> = workspace.iter().flat_map(|workspace| &workspace.folders).map(|folder| matcher_for(Some(&folder.path))).collect();
        let global_matcher = matcher_for(None);
        move |path: &Path| {
            let absolute = path.to_string_lossy();
            let folder = workspace.as_ref().and_then(|workspace| workspace.folder_of(path).map(|folder| (workspace, folder)));
            let ((matcher, has_absolute_path), relative) = match folder {
                Some((workspace, folder)) => {
                    let index = workspace.folders.iter().position(|candidate| candidate == folder).expect("folder of this workspace");
                    let relative = path.iter().skip(folder.path.iter().count()).collect::<PathBuf>().to_string_lossy().into_owned();
                    (&folder_matchers[index], Some(relative))
                }
                None => (&global_matcher, None),
            };
            if matcher.matches(relative.as_deref().unwrap_or(&absolute), None, None).is_some() {
                return true;
            }
            let Some(relative) = relative.filter(|_| *has_absolute_path) else { return false };
            matcher.matches(&absolute, None, None).is_some() && !matcher.is_excepted(&relative, None)
        }
    }

    /// `AnythingQuickAccessProvider._getPicks`: `None` keeps the shown picks, which a changed `:line` suffix does
    /// not re-sort and which wait for the file list up to the merge delay.
    pub(super) fn file_picks(&mut self, id: i64, original_filter: &str, window: &Window, cx: &mut Context<Self>) -> Option<(Vec<Entry>, Vec<Accept>, bool)> {
        if self.quick_access.files.as_ref().is_none_or(|state| state.input != id) {
            self.quick_access.files = Some(self.load_file_query_cache(id, window, cx));
        }
        let include_history = self.setting("search.quickOpen.includeHistory").as_bool().unwrap_or(true);
        let sort_by_recency = self.setting("search.quickOpen.history.filterSortOrder") == "recency";
        let workspace = self.workspace().cloned();
        let home = std::env::var("HOME").unwrap_or_default();
        let state = self.quick_access.files.as_mut().expect("set above");

        let filter_with_range = anything_quick_access::extract_range_from_filter(original_filter, &["@"]);
        let filter = filter_with_range.as_ref().map_or(original_filter, |(filter, _)| filter.as_str()).to_owned();
        state.last_range = filter_with_range.map(|(_, range)| range);
        if state.last_original_filter.as_deref() != Some(original_filter) && state.last_filter.as_deref() == Some(filter.as_str()) {
            return None;
        }
        state.last_original_filter = Some(original_filter.to_owned());
        state.last_filter = Some(filter.clone());

        let query = fuzzy_scorer::prepare_query(&filter);
        let history = anything_quick_access::editor_history_picks(&self.quick_access.editor_history.entries, &query, workspace.as_ref(), include_history, sort_by_recency, &mut state.scorer_cache);
        let files = match &state.files {
            _ if query.piece.normalized.is_empty() => Some(Vec::new()),
            Some(files) => Some(anything_quick_access::file_picks(files, &query, &history, workspace.as_ref(), &home, &mut state.scorer_cache)),
            None => None,
        };
        // applyFastAndSlowPicks: history picks alone show once the merge delay passed, and not when empty.
        if files.is_none() && (!state.merge_delay_passed || history.is_empty()) {
            return None;
        }

        let mut entries = Vec::new();
        let mut accepts = Vec::new();
        if query.piece.normalized.is_empty() {
            self.help_picks(id, &mut entries, &mut accepts, window);
        }
        if !history.is_empty() {
            entries.push(Entry::Separator(Some("recently opened".into())));
            for pick in &history {
                entries.push(self.file_pick_entry(pick, accepts.len() as i64, true));
                accepts.push(Accept::File { path: pick.path.clone() });
            }
        }
        let loading = files.is_none();
        let files = files.unwrap_or_default();
        if !files.is_empty() {
            entries.push(Entry::Separator(Some("file results".into())));
            for pick in &files {
                entries.push(self.file_pick_entry(pick, accepts.len() as i64, false));
                accepts.push(Accept::File { path: pick.path.clone() });
            }
        }
        if accepts.is_empty() && !original_filter.is_empty() {
            entries = vec![Entry::Item(Item { handle: 0, label: "No matching results".into(), pickable: true, ..Item::default() })];
            accepts = vec![Accept::Nothing];
        }
        Some((entries, accepts, loading))
    }

    /// `createAnythingPick`: the file icon, and for a history entry the remove button, kept visible as a dirty dot.
    fn file_pick_entry(&self, pick: &FilePick, handle: i64, in_history: bool) -> Entry {
        let mut buttons = Vec::new();
        if in_history {
            let dirty = self.editors.find(&pick.path).is_some_and(|index| self.editors.editors[index].is_dirty());
            let codicon = if dirty { "circle-filled" } else { "close" };
            buttons.push(Button { handle: 0, codicon: Some(codicon.into()), tooltip: Some("Remove from Recently Opened".into()), always_visible: dirty, ..Button::default() });
        }
        Entry::Item(Item {
            handle,
            label: pick.label.clone(),
            description: Some(pick.description.clone()).filter(|description| !description.is_empty()),
            pickable: true,
            file_icon: Some(pick.path.clone()),
            buttons,
            label_highlights: pick.label_highlights.clone(),
            description_highlights: pick.description_highlights.clone(),
            ..Item::default()
        })
    }

    /// `createFileQueryCache`: one search lists the workspace files and the opened files outside it, then the picks
    /// update; the merge delay lets the history picks show alone while it runs.
    fn load_file_query_cache(&self, id: i64, window: &Window, cx: &mut Context<Self>) -> FilePickState {
        let folders: Vec<PathBuf> = self.workspace().iter().flat_map(|workspace| &workspace.folders).map(|folder| folder.path.clone()).collect();
        let settings = self.settings.clone();
        let query = search::query::file_query(&folders, &serde_json::json!({ "filePattern": "", "sortByScore": true }), &|folder, key| {
            settings.lock().unwrap().get_value(Some(key), &Overrides { resource: Some(folder), ..Overrides::default() }).unwrap_or(Value::Null)
        });
        // `getOutOfWorkspaceEditorResources`.
        let extra_files: Vec<FileEntry> = self
            .editors
            .editors
            .iter()
            .map(|editor| &editor.document.path)
            .filter(|path| !folders.iter().any(|folder| path.starts_with(folder)))
            .map(|path| FileEntry { relative_path: path.to_string_lossy().into_owned(), path: path.clone() })
            .collect();
        let rg = search::ripgrep::rg_path(&self.app_root);
        let cancellation = Arc::new(Cancellation::default());
        let search_cancellation = cancellation.clone();
        let load = cx.spawn_in(window, async move |workbench, cx| {
            let merge_delay = cx.background_executor().timer(MERGE_DELAY);
            let search = cx.background_spawn(async move {
                let mut files = extra_files;
                if !query.folder_queries.is_empty() {
                    let result = search::file_search::search(&query, &rg, &search_cancellation, &mut |folder, relative_path| {
                        files.push(FileEntry { relative_path: relative_path.to_owned(), path: folder.join(relative_path) });
                    });
                    if let Err(error) = result {
                        eprintln!("quick open file search: {error}");
                    }
                }
                files
            });
            let mut search = std::pin::pin!(search);
            let finished = futures::select_biased! {
                files = futures::FutureExt::fuse(&mut search) => Some(files),
                _ = futures::FutureExt::fuse(merge_delay) => None,
            };
            let files = match finished {
                Some(files) => files,
                None => {
                    let _ = workbench.update_in(cx, |workbench, window, cx| workbench.file_query_cache_progress(id, None, window, cx));
                    search.await
                }
            };
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.file_query_cache_progress(id, Some(files), window, cx));
        });
        FilePickState {
            input: id,
            last_original_filter: None,
            last_filter: None,
            last_range: None,
            scorer_cache: FuzzyScorerCache::default(),
            files: None,
            merge_delay_passed: false,
            cancellation,
            _load: load,
        }
    }

    /// The merge delay passed (`None`) or the file list loaded: the picker's picks update.
    fn file_query_cache_progress(&mut self, id: i64, files: Option<Vec<FileEntry>>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.quick_access.files.as_mut().filter(|state| state.input == id) else { return };
        match files {
            Some(files) => state.files = Some(files),
            None => state.merge_delay_passed = true,
        }
        // The value did not change; a cleared last filter keeps the `:line` check in file_picks from skipping the update.
        state.last_filter = None;
        self.update_quick_access_picks(id, window, cx);
        self.flush_quick_input_events(window, cx);
        cx.notify();
    }

    /// `openAnything`: the file opens pinned unless `workbench.editor.enablePreviewFromQuickOpen`, at the `:line`
    /// range of the filter, and keeps the focus in the picker when accepted in the background.
    pub(super) fn open_file_pick(&mut self, path: PathBuf, in_background: bool, window: &mut Window, cx: &mut Context<Self>) {
        let enable_preview = self.setting("workbench.editor.enablePreview").as_bool().unwrap_or(true);
        let enable_preview_from_quick_open = self.setting("workbench.editor.enablePreviewFromQuickOpen").as_bool().unwrap_or(false);
        let pinned = in_background || !enable_preview_from_quick_open || !enable_preview;
        let range = self.quick_access.files.as_ref().and_then(|state| state.last_range);
        self.open_file(path.clone(), pinned, window, cx);
        if let Some(range) = range {
            self.select_range(&path, range, cx);
        }
        match in_background {
            true => window.focus(&self.quick_inputs.focus, cx),
            false => window.focus(&self.editor_focus, cx),
        }
    }

    /// `applyTextEditorOptions`: the selection, validated against the text, then revealed in the center.
    fn select_range(&mut self, path: &Path, range: TextRange, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut().filter(|editor| editor.document.path == path) else { return };
        let lines = &editor.document.lines;
        let validate = |line: usize, column: usize| {
            let line = line.clamp(1, lines.len()) - 1;
            Position::new(line, column.clamp(1, lines[line].chars().count() + 1) - 1)
        };
        let selection = Selection { anchor: validate(range.start_line_number, range.start_column), head: validate(range.end_line_number, range.end_column) };
        editor.set_selections(vec![selection]);
        editor.reveal_range(selection.start(), selection.end(), VerticalReveal::Center, viewport);
        self.host.send_selection(editor, "code.navigation");
        self.editor_scrolled(cx);
    }
}

#[cfg(test)]
mod tests;
