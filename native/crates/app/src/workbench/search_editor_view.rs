//! The search editor (searchEditor.ts, searchEditorActions.ts, searchEditor.contribution.ts): an editor of a
//! `search-editor` model holding the results text of its own search, with the search's configuration beside it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use gpui::{Context, Div, Styled, Task, Window, div, prelude::FluentBuilder, px};
use search::ripgrep::Cancellation;
use serde_json::Value;

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::{ContentChanged, OpenEditor, Position, Selection, TextDocument, char_column, visible_column};
use crate::search_editor::{SearchConfiguration, TextRange, search_editor_name, serialize_search_result_for_editor};
use crate::search_model::{SearchResult, utf16_len};

/// `SearchEditorConstants.SearchEditorScheme`.
pub const SEARCH_EDITOR_SCHEME: &str = "search-editor";

/// The search editors by their model's path.
pub type SearchEditors = HashMap<PathBuf, SearchEditor>;

pub struct SearchEditor {
    /// The configuration of the last search (`SearchConfigurationModel`), which names the editor.
    pub config: SearchConfiguration,
    /// `setMatchRanges`: the matches in the results text, decorated as `searchEditorFindMatch`.
    pub match_ranges: Vec<TextRange>,
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
        self.commands.register("rerunSearchEditorSearch", |workbench, _, window, cx| {
            if let Some(path) = workbench.active_search_editor() {
                workbench.run_search_editor(path, window, cx);
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
            Some(search_editor) => search_editor_name(None, &search_editor.config.query),
            None => editor.document.name(),
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
            None => self.open_search_editor(config.clone(), String::new()),
        };
        self.editors_changed(cx);
        let search_on_type = self.setting("search.searchOnType") == true;
        let trigger = args["triggerSearch"].as_bool();
        if trigger == Some(true) || (trigger != Some(false) && search_on_type && !config.query.is_empty()) {
            self.run_search_editor(path, window, cx);
        }
        window.focus(&self.editor_focus, cx);
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
            let path = self.open_search_editor(config, serialized.text);
            self.search_editors.get_mut(&path).expect("just opened").match_ranges = serialized.match_ranges;
        } else {
            let path = self.open_search_editor(SearchConfiguration { context_lines, ..config }, String::new());
            self.run_search_editor(path, window, cx);
        }
        window.focus(&self.editor_focus, cx);
        self.editors_changed(cx);
    }

    /// A pinned search editor over a new untitled `search-editor` model with `text`; returns its model's path.
    fn open_search_editor(&mut self, config: SearchConfiguration, text: String) -> PathBuf {
        let path = (1..).map(|n| PathBuf::from(format!("/{n}"))).find(|path| !self.search_editors.contains_key(path)).expect("a free name");
        let document = TextDocument::from_text(SEARCH_EDITOR_SCHEME, &path, &text, "utf8".into(), Some("search-result"), &self.languages, &self.settings.lock().unwrap());
        self.editors.open(document, false);
        self.search_editors.insert(path.clone(), SearchEditor { config, match_ranges: Vec::new(), cancellation: None, run: None });
        path
    }

    /// `doRunSearch` and `onSearchComplete`: the search editor's configuration searched; the serialized results
    /// replace its text, which stays clean, and the cursor goes to the start (`resetCursor`).
    fn run_search_editor(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let config = self.search_editors[&path].config.clone();
        if config.query.is_empty() {
            return;
        }
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
            let _ = workbench.update(cx, |workbench, cx| {
                let folders = workbench.search_folders();
                for (file, results) in found {
                    let folder = folders.iter().position(|folder| file.starts_with(folder)).unwrap_or(folders.len());
                    result.add(folder, &file, &results);
                }
                workbench.search_editor_complete(&path, &config, &result, finished, cx);
            });
        }));
    }

    fn search_editor_complete(&mut self, path: &Path, config: &SearchConfiguration, result: &SearchResult, finished: Result<bool, String>, cx: &mut Context<Self>) {
        let Ok(limit_hit) = finished else { return };
        // A search editor closed or searched again meanwhile drops these results.
        if !self.search_editors.get(path).is_some_and(|editor| editor.config == *config) {
            return;
        }
        let workspace = self.workspace();
        let serialized = serialize_search_result_for_editor(result, limit_hit, |file| crate::quick_input::path_label(file, workspace));
        self.replace_clean_model_text(SEARCH_EDITOR_SCHEME, path, serialized.text, cx);
        let editor = self.search_editors.get_mut(path).expect("checked above");
        editor.match_ranges = serialized.match_ranges;
        editor.cancellation = None;
        let viewport = self.editors.viewport;
        if let Some(model) = self.editors.model_mut(SEARCH_EDITOR_SCHEME, path) {
            model.selections = vec![Selection::caret(Position::new(0, 0))];
            model.set_scroll(0., 0., viewport);
        }
        cx.notify();
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

    /// The search editors whose models closed go.
    pub(super) fn drop_closed_search_editors(&mut self) {
        let editors = &self.editors;
        self.search_editors.retain(|path, _| editors.model(SEARCH_EDITOR_SCHEME, path).is_some());
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
}


