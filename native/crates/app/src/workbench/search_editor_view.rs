//! The search editor (searchEditor.ts, searchEditorActions.ts, searchEditor.contribution.ts): an editor of a
//! `search-editor` model holding the results text of its own search, with the search's configuration beside it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::channel::oneshot;
use gpui::{Context, Div, Styled, Task, Window, div, prelude::FluentBuilder, px};
use search::ripgrep::Cancellation;
use serde_json::Value;

use super::Workbench;
use super::search_view::SearchField;
use crate::commands::Outcome;
use crate::host_sync::SearchTab;
use crate::editor::{uri_string, ContentChanged, EditReason, OpenEditor, Position, Selection, TextDocument, VerticalReveal, char_column, utf16_column, visible_column};
use crate::search_editor::{
    SearchConfiguration, TextRange, adjacent_match, parse_serialized_search_editor, resolve_result_path, result_block_lines, result_definition, search_editor_name, serialize_search_configuration,
    serialize_search_result_for_editor, suggest_file_name,
};
use crate::search_model::{SearchResult, utf16_len};

/// `SearchEditorConstants.SearchEditorScheme`.
pub const SEARCH_EDITOR_SCHEME: &str = "search-editor";

/// `Memento(SearchEditorInput.ID)` in workspace storage.
const SEARCH_EDITOR_MEMENTO_KEY: &str = "memento/workbench.editorinputs.searchEditorInput";

/// The search editors by their model's path.
pub type SearchEditors = HashMap<PathBuf, SearchEditor>;

pub struct SearchEditor {
    /// The configuration of the last search (`SearchConfigurationModel`), which names the editor.
    pub config: SearchConfiguration,
    /// `setMatchRanges`: the matches in the results text, decorated as `searchEditorFindMatch`.
    pub match_ranges: Vec<TextRange>,
    /// `backingUri`: the `.code-search` file the editor was opened from or saved to.
    pub backing_file: Option<PathBuf>,
    cancellation: Option<Arc<Cancellation>>,
    run: Option<Task<()>>,
}

impl Workbench {
    pub(super) fn register_search_editor_actions(&mut self) {
        for (command, location) in [("search.action.openNewEditor", "new"), ("search.action.openEditor", "reuse")] {
            self.commands.register(command, move |workbench, args, window, cx| {
                let mut args = args.into_iter().next().filter(Value::is_object).unwrap_or_else(|| serde_json::json!({}));
                args.as_object_mut().expect("checked above").entry("location").or_insert(location.into());
                workbench.open_new_search_editor(&args, window, cx);
                Outcome::done()
            });
        }
        // `openSearchEditor`: the Search view's toolbar action takes the view's patterns and toggles.
        self.commands.register("search.action.openNewEditorFromView", |workbench, _, window, cx| {
            let config = workbench.search.configuration();
            let show_includes_excludes = !config.files_to_include.is_empty() || !config.files_to_exclude.is_empty() || !config.use_exclude_settings_and_ignore_files;
            let args = serde_json::json!({
                "filesToInclude": config.files_to_include,
                "onlyOpenEditors": config.only_open_editors,
                "filesToExclude": config.files_to_exclude,
                "isRegexp": config.is_regexp,
                "isCaseSensitive": config.is_case_sensitive,
                "matchWholeWord": config.match_whole_word,
                "useExcludeSettingsAndIgnoreFiles": config.use_exclude_settings_and_ignore_files,
                "showIncludesExcludes": show_includes_excludes,
            });
            workbench.open_new_search_editor(&args, window, cx);
            Outcome::done()
        });
        self.commands.register("search.action.openInEditor", |workbench, _, window, cx| {
            workbench.open_search_editor_from_results(window, cx);
            Outcome::done()
        });
        self.commands.register("search.searchEditor.action.deleteFileResults", |workbench, _, _, cx| {
            if workbench.active_search_editor().is_some() {
                workbench.delete_search_editor_file_results(cx);
            }
            Outcome::done()
        });
        self.commands.register("selectAllSearchEditorMatches", |workbench, _, window, cx| {
            if let Some(path) = workbench.active_search_editor() {
                workbench.select_all_search_editor_matches(&path, window, cx);
            }
            Outcome::done()
        });
        // `cleanState`: the editor is no longer dirty, its text stays.
        self.commands.register("cleanSearchEditorState", |workbench, _, _, cx| {
            if let Some(index) = workbench.active_search_editor().and_then(|_| workbench.editors.active_index()) {
                let editor = &mut workbench.editors.editors[index];
                let was_dirty = editor.is_dirty();
                editor.mark_saved();
                workbench.host.send_saved(&workbench.editors, &workbench.editors.editors[index], was_dirty);
                cx.notify();
            }
            Outcome::done()
        });
        self.commands.register("rerunSearchEditorSearch", |workbench, _, window, cx| {
            if workbench.active_search_editor().is_some() {
                workbench.trigger_search_editor(true, Duration::ZERO, None, window, cx);
            }
            Outcome::done()
        });
    }

    /// The model path of the active editor when it is a search editor.
    pub(super) fn active_search_editor(&self) -> Option<PathBuf> {
        let document = &self.editors.active()?.document;
        (document.scheme == SEARCH_EDITOR_SCHEME).then(|| document.path.clone())
    }

    /// `EditorInput.getName`: "Search: {query}" for a search editor (`SearchEditorInput.getName`), the file name otherwise.
    pub(super) fn editor_name(&self, editor: &crate::editor::OpenEditor) -> String {
        match self.search_editors.get(&editor.document.path).filter(|_| editor.document.scheme == SEARCH_EDITOR_SCHEME) {
            Some(search_editor) => search_editor_name(search_editor.backing_file.as_deref(), &search_editor.config.query),
            None if editor.document.scheme == crate::output::OUTPUT_SCHEME => "Output".into(),
            None if editor.document.scheme == super::terminal_view::TERMINAL_SCHEME => "Terminal".into(),
            None => self.webview_title(&editor.document.path).unwrap_or_else(|| editor.document.name()),
        }
    }

    /// `openNewSearchEditor`: the query is the active editor's selection, or the word at its cursor with
    /// `search.seedWithNearestWord`; location `reuse` opens the most recent search editor in place of a new one.
    // shortcut: `${...}` variables in the arguments are not resolved, the port has no configuration resolver.
    fn open_new_search_editor(&mut self, args: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let reuse = args["location"] == "reuse";
        // Upstream tests the setting for truthiness, so "never" seeds too.
        let seed_setting = self.setting("editor.find.seedSearchStringFromSelection");
        let seed = args["location"] == "new" || !(seed_setting == false || seed_setting == "" || seed_setting.is_null());
        let selected = self.selected_search_text().filter(|_| seed);
        let mut config = SearchConfiguration { query: selected.clone().unwrap_or_default(), ..SearchConfiguration::default() }.with_args(args);
        let existing = self.editors.editors.iter().rposition(|editor| editor.document.scheme == SEARCH_EDITOR_SCHEME).filter(|_| reuse);
        let path = match existing {
            Some(index) => {
                let path = self.editors.editors[index].document.path.clone();
                self.editors.activate(index);
                let editor = self.search_editors.get_mut(&path).expect("an open search editor has its state");
                if selected.is_none() {
                    config.query = editor.config.query.clone();
                }
                editor.config = config.clone();
                path
            }
            None => {
                config = self.new_search_editor_config(&selected, args);
                self.open_search_editor(config.clone(), String::new(), None, false)
            }
        };
        self.editors_changed(cx);
        self.set_search_editor_widget(path, &config);
        let search_on_type = self.setting("search.searchOnType") == true;
        let trigger = args["triggerSearch"].as_bool();
        let focus_results = args["focusResults"].as_bool();
        if trigger == Some(true) || (trigger != Some(false) && search_on_type && !config.query.is_empty()) {
            self.trigger_search_editor(true, Duration::ZERO, focus_results, window, cx);
        }
        // `selectQuery` on a reused editor without a selection, then `focusSearchInput`.
        if focus_results != Some(true) {
            self.focus_search_editor_input(SearchField::Query, existing.is_some() && selected.is_none(), window, cx);
        }
    }

    /// `getOrMakeSearchEditorInput` from `rawData` without results: the defaults, the last search editor's
    /// configuration with `search.searchEditor.reusePriorSearchConfiguration`, the arguments, then
    /// `search.searchEditor.defaultNumberOfContextLines` unless the arguments set the context lines or it is null.
    fn new_search_editor_config(&self, selected: &Option<String>, args: &Value) -> SearchConfiguration {
        let mut config = SearchConfiguration::default();
        if self.setting("search.searchEditor.reusePriorSearchConfiguration") == true {
            let memento = self.workspace_storage.lock().unwrap().get(SEARCH_EDITOR_MEMENTO_KEY).and_then(|raw| serde_json::from_str::<Value>(raw).ok());
            if let Some(memento) = memento {
                config = config.with_args(&memento["searchConfig"]);
            }
        }
        // `query: undefined` in the arguments still overrides the prior query.
        config.query = selected.clone().unwrap_or_default();
        config = config.with_args(args);
        if let Some(lines) = self.setting("search.searchEditor.defaultNumberOfContextLines").as_u64() {
            config.context_lines = args["contextLines"].as_u64().unwrap_or(lines) as usize;
        }
        config
    }

    /// `SearchEditorInput`'s `onConfigDidUpdate`: the configuration a new search editor can reuse.
    fn remember_search_editor_config(&self, config: &SearchConfiguration) {
        let memento = serde_json::json!({ "searchConfig": config });
        self.workspace_storage.lock().unwrap().set(SEARCH_EDITOR_MEMENTO_KEY, memento.to_string());
    }

    /// `createEditorFromSearchResult`: the Search view's results in a new search editor, searched again with
    /// `search.searchEditor.defaultNumberOfContextLines` when that is not 0.
    fn open_search_editor_from_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pattern) = self.search.pattern.clone() else { return };
        let view = self.search.configuration();
        let config = SearchConfiguration {
            query: pattern.pattern,
            is_regexp: pattern.is_reg_exp,
            is_case_sensitive: pattern.is_case_sensitive,
            match_whole_word: pattern.is_word_match,
            show_includes_excludes: !view.files_to_include.is_empty() || !view.files_to_exclude.is_empty() || !view.use_exclude_settings_and_ignore_files,
            context_lines: 0,
            ..view
        };
        let context_lines = self.setting("search.searchEditor.defaultNumberOfContextLines").as_u64().unwrap_or_default() as usize;
        if context_lines == 0 {
            let workspace = self.workspace();
            let serialized = serialize_search_result_for_editor(&self.search.result, false, |path| crate::quick_input::path_label(path, workspace));
            let path = self.open_search_editor(config.clone(), serialized.text, None, false);
            self.search_editors.get_mut(&path).expect("just opened").match_ranges = serialized.match_ranges;
            self.set_search_editor_widget(path, &config);
        } else {
            let config = SearchConfiguration { context_lines, ..config };
            let path = self.open_search_editor(config.clone(), String::new(), None, false);
            self.set_search_editor_widget(path, &config);
            self.trigger_search_editor(true, Duration::ZERO, Some(false), window, cx);
        }
        window.focus(&self.editor_focus, cx);
        self.editors_changed(cx);
    }

    /// A search editor over a new `search-editor` model with `text`, untitled without `backing_file`; returns its
    /// model's path.
    fn open_search_editor(&mut self, config: SearchConfiguration, text: String, backing_file: Option<PathBuf>, preview: bool) -> PathBuf {
        let path = (1..).map(|n| PathBuf::from(format!("/{n}"))).find(|path| !self.search_editors.contains_key(path)).expect("a free name");
        let document = TextDocument::from_text(SEARCH_EDITOR_SCHEME, &path, &text, "utf8".into(), Some("search-result"), &self.languages, &self.settings.lock().unwrap());
        self.editors.open(document, preview);
        self.search_editors.insert(path.clone(), SearchEditor { config, match_ranges: Vec::new(), backing_file, cancellation: None, run: None });
        path
    }

    /// The `*.code-search` editor resolver (`getOrMakeSearchEditorInput` from `existingFile`): the search editor of
    /// `file`, the open one when there is one, its query widget focused (`SearchEditor.focus`).
    pub(super) fn open_search_editor_file(&mut self, file: PathBuf, pinned: bool, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.search_editors.iter().find(|(_, editor)| editor.backing_file.as_ref() == Some(&file)).map(|(path, _)| path.clone());
        let open_index = open.and_then(|path| self.editors.editors.iter().position(|editor| editor.document.is(SEARCH_EDITOR_SCHEME, &path)));
        match open_index {
            Some(index) => {
                self.editors.activate(index);
                if pinned {
                    self.editors.pin(index);
                }
            }
            None => {
                let parsed = std::fs::read_to_string(&file).map_err(|error| error.to_string()).and_then(|text| parse_serialized_search_editor(&text));
                let (config, text) = match parsed {
                    Ok(parsed) => parsed,
                    // shortcut: a read error is logged, not shown as the reference's editor error placeholder.
                    Err(error) => return eprintln!("opening {}: {error}", file.display()),
                };
                self.open_search_editor(config, text, Some(file), !pinned);
            }
        }
        self.editors_changed(cx);
        match pinned {
            true => self.focus_search_editor_input(SearchField::Query, false, window, cx),
            false => window.focus(&self.explorer.focus, cx),
        }
    }

    /// `SearchEditorInput.save`: the configuration header and the results written to the backing file, or with
    /// none, `saveAs` to a `.code-search` file picked with the query as its suggested name.
    pub(super) fn save_search_editor(&mut self, path: PathBuf, cx: &mut Context<Self>) -> oneshot::Receiver<bool> {
        let (sender, receiver) = oneshot::channel();
        let search_editor = &self.search_editors[&path];
        let model = self.editors.model(SEARCH_EDITOR_SCHEME, &path).expect("saving an open model");
        let version = model.alternative_version_id();
        let contents = serialize_search_configuration(&search_editor.config) + "\n" + &model.document.lines.join("\n");
        if let Some(file) = search_editor.backing_file.clone() {
            let written = std::fs::write(&file, contents).map_err(|error| error.to_string());
            let _ = sender.send(self.finish_save(SEARCH_EDITOR_SCHEME, &path, version, written, cx));
            return receiver;
        }
        let directory = self.workspace().and_then(|workspace| workspace.folders.first()).map(|folder| folder.path.clone());
        let directory = directory.or_else(|| std::env::var_os("HOME").map(PathBuf::from)).unwrap_or_default();
        let picked = cx.prompt_for_new_path(&directory, Some(&suggest_file_name(&search_editor.config.query)));
        cx.spawn(async move |workbench, cx| {
            let Ok(Ok(Some(file))) = picked.await else {
                let _ = sender.send(false);
                return;
            };
            let written = std::fs::write(&file, contents).map_err(|error| error.to_string());
            let saved = workbench.update(cx, |workbench, cx| {
                // The editor can have closed while the dialog was open; the file is written all the same.
                if let Some(editor) = workbench.search_editors.get_mut(&path).filter(|_| written.is_ok()) {
                    editor.backing_file = Some(file);
                    workbench.search_editor_label_changed();
                }
                workbench.finish_save(SEARCH_EDITOR_SCHEME, &path, version, written, cx)
            });
            let _ = sender.send(saved.unwrap_or(false));
        })
        .detach();
        receiver
    }

    /// `SearchEditorInput.revert`: the backing file read again, or the results emptied; the editor is clean.
    pub(super) fn revert_search_editor(&mut self, index: usize, cx: &mut Context<Self>) {
        let path = self.editors.editors[index].document.path.clone();
        let editor = self.search_editors.get_mut(&path).expect("an open search editor has its state");
        let text = match &editor.backing_file {
            None => String::new(),
            Some(file) => match std::fs::read_to_string(file).map_err(|error| error.to_string()).and_then(|text| parse_serialized_search_editor(&text)) {
                Ok((config, text)) => {
                    editor.config = config;
                    text
                }
                Err(error) => return eprintln!("reverting {}: {error}", file.display()),
            },
        };
        editor.match_ranges.clear();
        if self.search_editor_widget.path.as_ref() == Some(&path) {
            self.search_editor_widget.path = None;
            self.load_search_editor_widget();
        }
        self.replace_model_text(SEARCH_EDITOR_SCHEME, &path, text, true, cx);
        self.search_editor_label_changed();
        let model = self.editors.model(SEARCH_EDITOR_SCHEME, &path).expect("still open");
        self.host.send_dirty_state(&self.editors, model, false);
    }

    /// `doRunSearch` and `onSearchComplete`: the widget's configuration becomes the editor's and is searched; the
    /// serialized results replace its text, and the cursor goes to the start with `reset_cursor`.
    pub(super) fn run_search_editor(&mut self, path: PathBuf, reset_cursor: bool, focus_results: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.search_editors.get(&path) else { return };
        let config = match self.search_editor_widget.path.as_ref() == Some(&path) {
            true => self.read_search_editor_widget(cx),
            false => editor.config.clone(),
        };
        if config.query.is_empty() {
            return;
        }
        self.search_editors.get_mut(&path).expect("checked above").config = config.clone();
        self.remember_search_editor_config(&config);
        let Ok(query) = self.text_query(&config) else { return };
        let mut result = SearchResult::default();
        let local = self.add_open_editor_matches(&query, config.only_open_editors, &mut result);
        let (cancellation, mut receiver, search) = self.spawn_text_search(query, local, cx);
        let editor = self.search_editors.get_mut(&path).expect("searched while open");
        if let Some(previous) = editor.cancellation.replace(cancellation) {
            previous.cancel();
        }
        editor.run = Some(cx.spawn_in(window, async move |workbench, cx| {
            let mut found = Vec::new();
            while let Some(file) = receiver.next().await {
                found.push(file);
            }
            let finished = search.await;
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                let folders = workbench.search_folders();
                for (file, results) in found {
                    let folder = folders.iter().position(|folder| file.starts_with(folder)).unwrap_or(folders.len());
                    result.add(folder, &file, &results);
                }
                if workbench.search_editor_complete(&path, &config, &result, finished, reset_cursor, cx) && focus_results {
                    window.focus(&workbench.editor_focus, cx);
                }
            });
        }));
    }

    /// False when the results were dropped.
    fn search_editor_complete(&mut self, path: &Path, config: &SearchConfiguration, result: &SearchResult, finished: Result<bool, String>, reset_cursor: bool, cx: &mut Context<Self>) -> bool {
        let Ok(limit_hit) = finished else { return false };
        // A search editor closed or searched again meanwhile drops these results.
        if !self.search_editors.get(path).is_some_and(|editor| editor.config == *config) {
            return false;
        }
        let workspace = self.workspace();
        let serialized = serialize_search_result_for_editor(result, limit_hit, |file| crate::quick_input::path_label(file, workspace));
        let editor = self.search_editors.get_mut(path).expect("checked above");
        // `setDirty(!untitled)`: new results of a saved search are unsaved.
        let untitled = editor.backing_file.is_none();
        self.replace_model_text(SEARCH_EDITOR_SCHEME, path, serialized.text, untitled, cx);
        let editor = self.search_editors.get_mut(path).expect("checked above");
        editor.match_ranges = serialized.match_ranges;
        editor.cancellation = None;
        self.search_editor_label_changed();
        let viewport = self.editors.viewport;
        if reset_cursor && let Some(model) = self.editors.model_mut(SEARCH_EDITOR_SCHEME, path) {
            model.selections = vec![Selection::caret(Position::new(0, 0))];
            model.set_scroll(0., 0., viewport);
        }
        cx.notify();
        true
    }

    /// The match decorations follow an edit of a search editor's text.
    pub(super) fn search_editor_edited(&mut self, path: &Path, change: &ContentChanged) {
        let Some(editor) = self.search_editors.get_mut(path).filter(|_| self.editors.active().is_some_and(|active| active.document.scheme == SEARCH_EDITOR_SCHEME)) else { return };
        for content_change in &change.changes {
            for range in &mut editor.match_ranges {
                *range = range.after_change(content_change);
            }
        }
    }

    /// searchEditor.css `.searchEditorFindMatch` boxes on line `index` of a search editor: background and a 1px border,
    /// dotted in high contrast.
    pub(super) fn search_editor_line_decorations(&self, editor: &OpenEditor, index: usize, char_width: f32) -> Vec<Div> {
        let Some(search_editor) = self.search_editors.get(&editor.document.path).filter(|_| editor.document.scheme == SEARCH_EDITOR_SCHEME) else { return Vec::new() };
        let text = &editor.document.lines[index];
        let tab_size = editor.document.indentation.tab_size;
        let x = |utf16: usize| visible_column(text, char_column(text, utf16 - 1), tab_size) as f32 * char_width;
        let background = self.theme.color("searchEditor.findMatchBackground");
        let border = self.theme.color("searchEditor.findMatchBorder");
        let mut boxes = Vec::new();
        for range in search_editor.match_ranges.iter().filter(|range| range.start_line <= index + 1 && index + 1 <= range.end_line) {
            let start = if range.start_line == index + 1 { x(range.start_column) } else { 0. };
            let end = if range.end_line == index + 1 { x(range.end_column) } else { x(utf16_len(text) + 1) };
            let mut decoration = div().absolute().top_0().h_full().left(px(start)).w(px(end - start));
            if let Some(background) = background {
                decoration = decoration.bg(background);
            }
            // shortcut: GPUI has no dotted border; the high contrast border is dashed.
            if let Some(border) = border {
                decoration = decoration.border_1().border_color(border).when(self.theme.kind.is_high_contrast(), |decoration| decoration.border_dashed());
            }
            boxes.push(decoration);
        }
        boxes
    }

    /// The search editors whose models closed go; the extension host's tabs of the others follow their names and
    /// backing files.
    pub(super) fn drop_closed_search_editors(&mut self) {
        let editors = &self.editors;
        self.search_editors.retain(|path, _| editors.model(SEARCH_EDITOR_SCHEME, path).is_some());
        self.host.search_tabs = self
            .search_editors
            .iter()
            .map(|(path, editor)| {
                let resource = match &editor.backing_file {
                    Some(file) => uri_string("file", file),
                    None => uri_string(SEARCH_EDITOR_SCHEME, path),
                };
                (path.clone(), SearchTab { label: search_editor_name(editor.backing_file.as_deref(), &editor.config.query), resource })
            })
            .collect();
    }

    /// `onDidChangeLabel`: the tabs again after a search editor's name or backing file changed.
    fn search_editor_label_changed(&mut self) {
        self.drop_closed_search_editors();
        self.host.send_tabs(&self.editors);
    }

    /// `getSelected` and the selection seed of `openNewSearchEditor`.
    fn selected_search_text(&self) -> Option<String> {
        let editor = self.editors.active()?;
        let selection = editor.selection();
        let lines = &editor.document.lines;
        if !selection.is_empty() {
            return Some(crate::editor::text_between(lines, selection.start(), selection.end()));
        }
        if self.setting("search.seedWithNearestWord") != true {
            return None;
        }
        let line = &lines[selection.head.line];
        let word = crate::editor::word_at(line, selection.head.column)?;
        Some(line.chars().skip(word.start).take(word.len()).collect())
    }

    /// `iterateThroughMatches`: the match after the cursor's start, or before it going back, wrapping; selected,
    /// revealed in the center when outside the viewport, and the results get the focus.
    // shortcut: no `alert` of the match, the port has no screen reader announcements yet.
    pub(super) fn select_search_editor_match(&mut self, path: &Path, reverse: bool, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let ranges = &self.search_editors[path].match_ranges;
        let editor = self.editors.active_mut().expect("the search editor is active");
        let lines = &editor.document.lines;
        let start = editor.selection().start();
        let position = (start.line + 1, utf16_column(&lines[start.line], start.column) + 1);
        let Some(range) = adjacent_match(ranges, position, reverse) else { return };
        let selection = range_selection(lines, &range);
        editor.set_selections(vec![selection]);
        editor.reveal_range(selection.start(), selection.end(), VerticalReveal::CenterIfOutsideViewport, viewport);
        self.host.send_selection(editor, "keyboard");
        self.editor_scrolled(cx);
        window.focus(&self.editor_focus, cx);
    }

    /// `focusAllResults`: every match selected, the results focused.
    fn select_all_search_editor_matches(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let ranges = &self.search_editors[path].match_ranges;
        let editor = self.editors.active_mut().expect("the search editor is active");
        let selections: Vec<Selection> = ranges.iter().map(|range| range_selection(&editor.document.lines, range)).collect();
        if selections.is_empty() {
            return;
        }
        editor.set_selections(selections);
        self.host.send_selection(editor, "keyboard");
        window.focus(&self.editor_focus, cx);
        cx.notify();
    }

    /// `deleteResultBlock`: the file blocks under the cursors go in one undoable edit; each cursor lands on the
    /// line after its block, as upstream gives those lines to the model after the edit.
    fn delete_search_editor_file_results(&mut self, cx: &mut Context<Self>) {
        let editor = self.editors.active_mut().expect("the search editor is active");
        let lines = &editor.document.lines;
        let spans: Vec<(usize, usize)> = editor.selections.iter().map(|selection| (selection.start().line, selection.end().line)).collect();
        let (mut deleted, ending_lines) = result_block_lines(lines, &spans);
        deleted.sort_unstable();
        let mut edits = Vec::new();
        let mut index = 0;
        while index < deleted.len() {
            let first = deleted[index];
            while index + 1 < deleted.len() && deleted[index + 1] == deleted[index] + 1 {
                index += 1;
            }
            let last = deleted[index];
            // Range(line, 1, line + 1, 1) past the last line validates to that line's end.
            let end = match last + 1 < lines.len() {
                true => Position::new(last + 1, 0),
                false => Position::new(last, lines[last].chars().count()),
            };
            edits.push((Position::new(first, 0), end, String::new(), false));
            index += 1;
        }
        let line_count = lines.len() - deleted.iter().filter(|line| **line + 1 < lines.len()).count();
        let endings = ending_lines.into_iter().map(|line| Selection::caret(Position::new(line.min(line_count - 1), 0))).collect::<Vec<_>>();
        editor.push_undo_stop();
        let change = editor.execute_edits(edits, Some(endings).filter(|endings| !endings.is_empty()), EditReason::Unknown("deleteResultBlock"));
        editor.push_undo_stop();
        if let Some(change) = change {
            let index = self.editors.active_index().expect("checked above");
            self.host.send_content_change(&self.editors, index, &change);
            let path = self.editors.editors[index].document.path.clone();
            self.search_editor_edited(&path, &change);
        }
        cx.notify();
    }

    /// `onMouseUp` of the results: a double click on a result line goes to its location
    /// (`search.searchEditor.doubleClickBehaviour`), resolved as the search-result extension's definition provider does.
    // shortcut: `peekDefinition` (the single click setting, the double click on a file line) does nothing, the peek
    // view (editor/contrib/peekView) is not ported; `openLocationToSide` opens in the active group, the port has one.
    pub(super) fn search_editor_mouse_up(&mut self, index: usize, visible: f32, click_count: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active().filter(|editor| editor.document.scheme == SEARCH_EDITOR_SCHEME) else { return };
        if click_count != 2 || self.setting("search.searchEditor.doubleClickBehaviour") == "selectWord" {
            return;
        }
        let lines = &editor.document.lines;
        let column = crate::editor::column_at_visible(&lines[index], visible, editor.document.indentation.tab_size);
        let Some((label, line, utf16)) = result_definition(lines, index, utf16_column(&lines[index], column)) else { return };
        let folders: Vec<(String, PathBuf)> = self.workspace().iter().flat_map(|workspace| &workspace.folders).map(|folder| (folder.name.clone(), folder.path.clone())).collect();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let Some(file) = resolve_result_path(&label, &folders, home.as_deref()) else { return };
        let caret = Selection::caret(Position::new(index, column));
        self.editors.active_mut().expect("checked above").set_selections(vec![caret]);
        self.open_file(file.clone(), false, window, cx);
        let Some(target) = self.editors.active().filter(|editor| editor.document.is("file", &file)) else { return };
        let text = target.document.lines.get(line).map_or("", String::as_str);
        let column = char_column(text, utf16) + 1;
        self.select_range(&file, crate::anything_quick_access::TextRange { start_line_number: line + 1, start_column: column, end_line_number: line + 1, end_column: column }, cx);
        window.focus(&self.editor_focus, cx);
    }
}

/// A 1-based UTF-16 range of the search editor's text as a selection.
fn range_selection(lines: &[String], range: &TextRange) -> Selection {
    let position = |line: usize, column: usize| Position::new(line - 1, char_column(&lines[line - 1], column - 1));
    Selection { anchor: position(range.start_line, range.start_column), head: position(range.end_line, range.end_column) }
}


