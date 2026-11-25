//! The Search view's commands, context keys and search run (searchView.ts, searchWidget.ts, searchActions*.ts,
//! replaceService.ts).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use config::configuration::Overrides;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{AppContext, ClipboardItem, Context, Keystroke, PromptLevel, Task, Window};
use search::ripgrep::Cancellation;
use search::search_paths::{self, WorkspaceFolder};
use search::text_search::SearchRange;
use serde_json::{Value, json};

use super::Workbench;
use super::explorer_actions::confirm;
use super::find_widget::HISTORY_LIMIT;
use super::input_box_view::InputId;
use super::search_view::{MessagePart, RowAction, RowKey, RowKind, SEARCH_FIELDS, SearchField, SearchOption, SearchUiState, SideBarView};
use crate::anything_quick_access::TextRange;
use crate::commands::Outcome;
use crate::editor::char_column;
use crate::editor::find::search::SearchParams;
use crate::history::HistoryNavigator;
use crate::input_box::InputBox;
use crate::menus::MenuId;
use crate::search_editor::SearchConfiguration;
use crate::search_model::{MessageLink, SearchResult, after_replace_all_message, no_results_message, replace_all_confirmation_message, result_count_message, utf16_len};

#[cfg(test)]
mod tests;

/// `SearchHistoryService.SEARCH_HISTORY_KEY`, workspace storage.
const SEARCH_HISTORY_KEY: &str = "workbench.search.history";
/// `SearchUIState` turns slow after this long (`doSearch`'s slowTimer).
const SLOW_SEARCH: Duration = Duration::from_secs(2);
/// `searchMaxResultsWarning`.
const LIMIT_HIT_WARNING: &str = "The result set only contains a subset of all matches. Be more specific in your search to narrow down the results.";
/// The text searchWidget.ts matches a regex against to scale the search-on-type delay.
const MATCHINESS_TEXT: &str = "\n\t\t\t\t\t\t\t\t~!@#$%^&*()_+\n\t\t\t\t\t\t\t\t`1234567890-=\n\t\t\t\t\t\t\t\tqwertyuiop[]\\\n\t\t\t\t\t\t\t\tQWERTYUIOP{}|\n\t\t\t\t\t\t\t\tasdfghjkl;'\n\t\t\t\t\t\t\t\tASDFGHJKL:\"\n\t\t\t\t\t\t\t\tzxcvbnm,./\n\t\t\t\t\t\t\t\tZXCVBNM<>? ";

impl Workbench {
    pub(super) fn register_search_actions(&mut self) {
        self.commands.register("workbench.view.search", |workbench, _, window, cx| {
            workbench.open_search_view(cx);
            workbench.focus_search_input(SearchField::Query, true, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.view.explorer", |workbench, _, window, cx| {
            workbench.side_bar_view = SideBarView::Explorer;
            window.focus(&workbench.explorer.focus.clone(), cx);
            cx.notify();
            Outcome::done()
        });
        self.commands.register("workbench.action.findInFiles", |workbench, args, window, cx| {
            let args = args.into_iter().next().unwrap_or(Value::Null);
            workbench.find_in_files(&args, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.replaceInFiles", |workbench, _, window, cx| {
            workbench.open_search_view(cx);
            workbench.toggle_replace(Some(true), window, cx);
            let updated = workbench.update_text_from_selection(false, window, cx);
            workbench.focus_search_widget(true, updated, window, cx);
            Outcome::done()
        });
        self.commands.register("search.action.refreshSearchResults", |workbench, _, window, cx| {
            workbench.trigger_search(false, None, window, cx);
            Outcome::done()
        });
        self.commands.register("search.action.cancel", |workbench, _, window, cx| {
            workbench.cancel_search(true, window, cx);
            Outcome::done()
        });
        self.commands.register("search.action.clearSearchResults", |workbench, _, _, cx| {
            workbench.clear_search_results(true, cx);
            Outcome::done()
        });
        self.commands.register("search.action.collapseSearchResults", |workbench, _, _, cx| {
            workbench.collapse_deepest_expanded_level(cx);
            Outcome::done()
        });
        self.commands.register("search.action.expandSearchResults", |workbench, _, _, cx| {
            workbench.set_search_rows_collapsed(false, cx);
            Outcome::done()
        });
        self.commands.register("search.action.viewAsTree", |workbench, _, _, cx| {
            workbench.search.tree_view = true;
            cx.notify();
            Outcome::done()
        });
        self.commands.register("search.action.viewAsList", |workbench, _, _, cx| {
            workbench.search.tree_view = false;
            cx.notify();
            Outcome::done()
        });
        self.commands.register("search.action.replaceAll", |workbench, _, window, cx| workbench.replace_all(window, cx));
        for (id, option) in [
            ("toggleSearchCaseSensitive", SearchOption::MatchCase),
            ("toggleSearchWholeWord", SearchOption::WholeWord),
            ("toggleSearchRegex", SearchOption::Regex),
            ("toggleSearchPreserveCase", SearchOption::PreserveCase),
        ] {
            self.commands.register(id, move |workbench, _, window, cx| {
                workbench.toggle_search_option(option, window, cx);
                Outcome::done()
            });
        }
        self.commands.register("workbench.action.search.toggleQueryDetails", |workbench, _, window, cx| {
            workbench.toggle_query_details(None, true, window, cx);
            Outcome::done()
        });
        self.commands.register("closeReplaceInFilesWidget", |workbench, _, window, cx| {
            workbench.toggle_replace(Some(false), window, cx);
            workbench.focus_search_input(SearchField::Query, false, window, cx);
            Outcome::done()
        });
        self.commands.register("search.focus.nextInputBox", |workbench, _, window, cx| {
            workbench.focus_next_search_input(true, window, cx);
            Outcome::done()
        });
        self.commands.register("search.focus.previousInputBox", |workbench, _, window, cx| {
            workbench.focus_next_search_input(false, window, cx);
            Outcome::done()
        });
        self.commands.register("search.action.focusSearchFromResults", |workbench, _, window, cx| {
            workbench.focus_next_search_input(false, window, cx);
            Outcome::done()
        });
        self.commands.register("search.action.focusSearchList", |workbench, _, window, cx| {
            workbench.focus_search_list(window, cx);
            Outcome::done()
        });
        self.commands.register("search.action.openResult", |workbench, _, window, cx| {
            let Some(key) = workbench.search.focused.clone() else { return Outcome::done() };
            match key {
                RowKey::Folder(_) => workbench.toggle_search_row(&key, cx),
                _ => workbench.open_search_row(&key, false, true, window, cx),
            }
            Outcome::done()
        });
        self.commands.register("search.action.remove", |workbench, _, window, cx| {
            if let Some(key) = workbench.search.focused.clone() {
                workbench.search_row_action(&key, RowAction::Dismiss, window, cx);
            }
            Outcome::done()
        });
        for id in ["search.action.replace", "search.action.replaceAllInFile", "search.action.replaceAllInFolder"] {
            self.commands.register(id, |workbench, _, window, cx| {
                if let Some(key) = workbench.search.focused.clone() {
                    workbench.search_row_action(&key, RowAction::Replace, window, cx);
                }
                Outcome::done()
            });
        }
        self.commands.register("search.action.focusNextSearchResult", |workbench, _, window, cx| {
            workbench.select_search_match(true, window, cx);
            Outcome::done()
        });
        self.commands.register("search.action.focusPreviousSearchResult", |workbench, _, window, cx| {
            workbench.select_search_match(false, window, cx);
            Outcome::done()
        });
        self.commands.register("search.action.copyPath", |workbench, _, _, cx| {
            if let Some(RowKey::File(path) | RowKey::Folder(path)) = &workbench.search.focused {
                cx.write_to_clipboard(ClipboardItem::new_string(workbench.tildify(path)));
            }
            Outcome::done()
        });
        self.commands.register("search.action.copyMatch", |workbench, _, _, cx| {
            if let Some(text) = workbench.search.focused.as_ref().map(|key| workbench.search_row_text(key)) {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            Outcome::done()
        });
        self.commands.register("search.action.clearHistory", |workbench, _, _, _| {
            for field in SEARCH_FIELDS {
                workbench.search.input_mut(field).history.clear();
            }
            workbench.workspace_storage.lock().unwrap().delete(SEARCH_HISTORY_KEY);
            Outcome::done()
        });
        self.commands.register("search.action.copyAll", |workbench, _, _, cx| {
            let files = &workbench.search.result.files;
            let text: Vec<String> = (0..files.len()).map(|file| workbench.search_file_text(file)).collect();
            cx.write_to_clipboard(ClipboardItem::new_string(text.join("\n\n")));
            Outcome::done()
        });
        self.commands.register("search.action.revealInSideBar", |workbench, _, window, cx| {
            if let Some(RowKey::File(path)) = workbench.search.focused.clone() {
                workbench.execute_command("workbench.view.explorer", Vec::new(), window, cx);
                workbench.select_explorer_item(&path, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("search.action.expandRecursively", |workbench, _, _, cx| {
            if let Some(key) = workbench.search.focused.clone() {
                workbench.expand_search_row_recursively(&key, cx);
            }
            Outcome::done()
        });
        for (id, include) in [("search.action.restrictSearchToFolder", true), ("search.action.excludeFromSearch", false)] {
            self.commands.register(id, move |workbench, _, window, cx| {
                if let Some(RowKey::Folder(path) | RowKey::File(path)) = workbench.search.focused.clone() {
                    workbench.search_with_folder(&path, include, window, cx);
                }
                Outcome::done()
            });
        }
        for (id, exclude) in [("search.action.excludeFileTypeFromSearch", true), ("search.action.includeFileTypeInSearch", false)] {
            self.commands.register(id, move |workbench, _, window, cx| {
                if let Some(RowKey::File(path)) = workbench.search.focused.clone() {
                    workbench.modify_search_file_type_pattern(&path, exclude, window, cx);
                }
                Outcome::done()
            });
        }
        self.register_search_list_commands();
    }

    /// `addToHistory` of the inputs, saved as `_saveSearchHistoryService` does. Upstream saves on shutdown; the port saves
    /// on each change, as its storage writes synchronously and the app can exit without a shutdown phase.
    fn add_to_search_history(&mut self, fields: &[SearchField]) {
        for &field in fields {
            let input = self.search.input_mut(field);
            input.history.add_to_history(&input.value.value, false);
        }
        self.save_search_history();
    }

    /// `SearchHistoryService.save`: the non-empty histories under `workbench.search.history`, removed when all are empty.
    fn save_search_history(&self) {
        let mut history = serde_json::Map::new();
        for (key, field) in [("search", SearchField::Query), ("replace", SearchField::Replace), ("exclude", SearchField::Exclude), ("include", SearchField::Include)] {
            let elements = self.search.input(field).history.elements();
            if !elements.is_empty() {
                history.insert(key.into(), json!(elements));
            }
        }
        let mut storage = self.workspace_storage.lock().unwrap();
        match history.is_empty() {
            true => storage.delete(SEARCH_HISTORY_KEY),
            false => storage.set(SEARCH_HISTORY_KEY, Value::Object(history).to_string()),
        }
    }

    /// The workspace's storage arrived: `SearchHistoryService.load`, prepended to the inputs' histories (`prependHistory`).
    pub(super) fn restore_search_history(&mut self) {
        let stored = self.workspace_storage.lock().unwrap().get(SEARCH_HISTORY_KEY).and_then(|raw| serde_json::from_str::<Value>(raw).ok()).unwrap_or_default();
        for (key, field) in [("search", SearchField::Query), ("replace", SearchField::Replace), ("exclude", SearchField::Exclude), ("include", SearchField::Include)] {
            let mut elements: Vec<String> = stored[key].as_array().into_iter().flatten().filter_map(|element| element.as_str().map(str::to_owned)).collect();
            let history = &mut self.search.input_mut(field).history;
            elements.extend(history.elements().iter().cloned());
            *history = HistoryNavigator::new(elements, HISTORY_LIMIT);
        }
    }

    /// `history.showPrevious` and `history.showNext` on a Search view input; the new value searches as typing does.
    pub(super) fn show_search_history_value(&mut self, field: SearchField, previous: bool, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.search.input_mut(field);
        let value = match previous {
            true => input.history.show_previous_value(&input.value.value),
            false => Some(input.history.show_next_value(&input.value.value)),
        };
        self.save_search_history();
        if let Some(value) = value {
            self.edit_search_input(field, |input| input.set_value(value), window, cx);
        }
    }

    /// searchView.ts `onContextMenu`: the SearchContext menu for the right-clicked row, which takes the focus but not
    /// the selection.
    pub(super) fn show_search_context_menu(&mut self, key: &RowKey, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.search.focused = Some(key.clone());
        window.focus(&self.search.focus.clone(), cx);
        let anchor = gpui::Bounds { origin: position, size: gpui::Size::default() };
        let context = self.key_context(window);
        self.show_context_menu(MenuId::SearchContext, &context, anchor, Vec::new(), window, cx);
    }

    /// `forcedExpandRecursively`: the row and every row below it expanded.
    fn expand_search_row_recursively(&mut self, key: &RowKey, cx: &mut Context<Self>) {
        let (RowKey::Folder(folder) | RowKey::File(folder)) = key else { return };
        self.search.collapsed.insert(key.clone(), false);
        loop {
            let collapsed: Vec<RowKey> = self
                .search
                .rows(&self.search_folders(), Some(&self.collapse_results()))
                .into_iter()
                .filter(|row| matches!(row.kind, RowKind::Folder { collapsed: true, .. } | RowKind::File { collapsed: true, .. }))
                .filter(|row| matches!(&row.key, RowKey::Folder(path) | RowKey::File(path) if path.starts_with(folder)))
                .map(|row| row.key)
                .collect();
            if collapsed.is_empty() {
                break;
            }
            for key in collapsed {
                self.search.collapsed.insert(key, false);
            }
        }
        cx.notify();
    }

    /// searchActionsFind.ts `searchWithFolderCommand` from the results and searchView.ts `_searchWithIncludeOrExclude`:
    /// the folder of `path` goes into the include or exclude input.
    fn search_with_folder(&mut self, path: &Path, include: bool, window: &mut Window, cx: &mut Context<Self>) {
        let folder = match path.is_dir() {
            true => path.to_path_buf(),
            false => path.parent().unwrap_or(path).to_path_buf(),
        };
        let workspace_folders: Vec<_> = self.workspace().iter().flat_map(|workspace| &workspace.folders).map(|folder| (folder.path.clone(), folder.name.clone())).collect();
        let folders: Vec<WorkspaceFolder> = workspace_folders.iter().map(|(path, name)| WorkspaceFolder { path, name }).collect();
        let workspace = search_paths::Workspace { folders: &folders, single_folder: folders.len() == 1 && self.workspace().is_some_and(|workspace| workspace.file.is_none()), home: None };
        let folder_paths = search_paths::resolve_resources_for_search_includes(&[folder], &workspace);
        if folder_paths.is_empty() || folder_paths.iter().any(|path| path == ".") {
            self.search.input_mut(SearchField::Include).value.set_value(String::new());
            self.focus_search_widget(true, false, window, cx);
            return;
        }
        if !self.search.details_visible {
            self.toggle_query_details(Some(true), true, window, cx);
        }
        let field = if include { SearchField::Include } else { SearchField::Exclude };
        self.search.input_mut(field).value.set_value(folder_paths.join(", "));
        self.focus_search_input(SearchField::Query, false, window, cx);
    }

    /// searchActionsFind.ts `modifySearchFileTypePattern`: `*.ext` of the file added to the exclude or include input.
    fn modify_search_file_type_pattern(&mut self, path: &Path, exclude: bool, window: &mut Window, cx: &mut Context<Self>) {
        let file_name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let field = if exclude { SearchField::Exclude } else { SearchField::Include };
        let current = self.search.value(field).to_string();
        let updated = merge_search_pattern_if_not_exists(&current, &extract_search_file_pattern(&file_name));
        if updated == current {
            return;
        }
        self.search.input_mut(field).value.set_value(updated);
        self.toggle_query_details(Some(true), false, window, cx);
        self.trigger_search(false, None, window, cx);
    }

    /// The list commands (listCommands.ts) act on the focused list; the results tree is the only list that takes them.
    fn register_search_list_commands(&mut self) {
        type Run = fn(&mut Workbench, &mut Window, &mut Context<Workbench>);
        let commands: [(&str, Run); 12] = [
            ("list.focusDown", |workbench, window, cx| workbench.move_search_focus(1, window, cx)),
            ("list.focusUp", |workbench, window, cx| workbench.move_search_focus(-1, window, cx)),
            ("list.focusPageDown", |workbench, window, cx| workbench.move_search_focus(workbench.search_page_size(), window, cx)),
            ("list.focusPageUp", |workbench, window, cx| workbench.move_search_focus(-workbench.search_page_size(), window, cx)),
            ("list.focusFirst", |workbench, window, cx| workbench.move_search_focus(isize::MIN, window, cx)),
            ("list.focusLast", |workbench, window, cx| workbench.move_search_focus(isize::MAX, window, cx)),
            ("list.select", |workbench, window, cx| drop(workbench.execute_command("search.action.openResult", Vec::new(), window, cx))),
            ("list.toggleExpand", |workbench, _, cx| {
                if let Some(key) = workbench.search.focused.clone() {
                    workbench.toggle_search_row(&key, cx);
                }
            }),
            ("list.expand", |workbench, window, cx| workbench.expand_search_row(true, window, cx)),
            ("list.collapse", |workbench, window, cx| workbench.expand_search_row(false, window, cx)),
            ("list.collapseAll", |workbench, _, cx| workbench.set_search_rows_collapsed(true, cx)),
            ("list.clear", |workbench, _, cx| {
                workbench.search.focus_and_select(None);
                cx.notify();
            }),
        ];
        for (id, run) in commands {
            self.commands.register(id, move |workbench, _, window, cx| {
                if workbench.search.focus.is_focused(window) {
                    run(workbench, window, cx);
                }
                Outcome::done()
            });
        }
    }

    fn set_search_rows_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        for row in self.search.rows(&self.search_folders(), None) {
            if !matches!(row.kind, RowKind::Match { .. }) {
                self.search.collapsed.insert(row.key, collapsed);
            }
        }
        cx.notify();
    }

    /// The rows one page of the results shows.
    fn search_page_size(&self) -> isize {
        let height = self.search.scroll.0.borrow().base_handle.bounds().size.height;
        (f32::from(height) / 22.).floor().max(1.) as isize
    }

    /// The Search view's context keys (searchView.ts, constants.ts `SearchContext`).
    pub(super) fn search_context(&self, window: &Window, context: &mut std::collections::HashMap<String, Value>) {
        let visible = self.side_bar_view == SideBarView::Search;
        context.insert("searchViewletVisible".into(), json!(visible));
        if !visible {
            return;
        }
        let view = &self.search;
        let input = self.focused_input(window).and_then(|id| match id {
            InputId::Search(field) => Some(field),
            _ => None,
        });
        let tree_focused = view.focus.is_focused(window);
        let focused = input.is_some() || tree_focused;
        context.insert("activeViewlet".into(), json!("workbench.view.search"));
        context.insert("searchViewletFocus".into(), json!(focused));
        context.insert("sideBarFocus".into(), json!(focused));
        context.insert("inputBoxFocus".into(), json!(input.is_some()));
        context.insert("searchInputBoxFocus".into(), json!(input == Some(SearchField::Query)));
        context.insert("replaceInputBoxFocus".into(), json!(input == Some(SearchField::Replace)));
        context.insert("patternIncludesInputBoxFocus".into(), json!(input == Some(SearchField::Include)));
        context.insert("patternExcludesInputBoxFocus".into(), json!(input == Some(SearchField::Exclude)));
        if input.is_some() {
            context.insert("inputFocus".into(), json!(true));
            // contextScopedHistoryWidget.ts: the inputs are history widgets, navigable both ways.
            context.insert("historyNavigationWidgetFocus".into(), json!(true));
            context.insert("historyNavigationForwardsEnabled".into(), json!(true));
            context.insert("historyNavigationBackwardsEnabled".into(), json!(true));
        }
        context.insert("replaceActive".into(), json!(view.replace_active));
        context.insert("hasSearchResult".into(), json!(!view.result.is_empty()));
        context.insert("viewHasSearchPattern".into(), json!(!view.value(SearchField::Query).is_empty()));
        context.insert("viewHasReplacePattern".into(), json!(!view.value(SearchField::Replace).is_empty()));
        context.insert("viewHasFilePattern".into(), json!(!view.value(SearchField::Include).is_empty() || !view.value(SearchField::Exclude).is_empty()));
        context.insert("inTreeView".into(), json!(view.tree_view));
        context.insert("searchState".into(), json!(view.state as u8));
        if !tree_focused {
            return;
        }
        context.insert("listFocus".into(), json!(true));
        context.insert("searchResultListFocused".into(), json!(true));
        let focus = view.focused.as_ref();
        let is_file = matches!(focus, Some(RowKey::File(_)));
        let is_folder = matches!(focus, Some(RowKey::Folder(_)));
        let is_match = matches!(focus, Some(RowKey::Match(..)));
        let first = self.search.rows(&self.search_folders(), Some(&self.collapse_results())).first().map(|row| row.key.clone());
        context.insert("firstMatchFocus".into(), json!(focus.is_some() && focus == first.as_ref()));
        context.insert("fileMatchOrMatchFocus".into(), json!(focus.is_some()));
        context.insert("fileMatchOrFolderMatchFocus".into(), json!(is_file || is_folder));
        context.insert("fileMatchOrFolderMatchWithResourceFocus".into(), json!(is_file || is_folder));
        context.insert("fileMatchFocus".into(), json!(is_file));
        context.insert("folderMatchFocus".into(), json!(is_folder));
        context.insert("folderMatchWithResourceFocus".into(), json!(is_folder));
        context.insert("matchFocus".into(), json!(is_match));
        context.insert("isEditableItem".into(), json!(true));
    }

    pub(super) fn search_folders(&self) -> Vec<PathBuf> {
        self.workspace().iter().flat_map(|workspace| &workspace.folders).map(|folder| folder.path.clone()).collect()
    }

    fn open_search_view(&mut self, cx: &mut Context<Self>) {
        self.side_bar_view = SideBarView::Search;
        cx.notify();
    }

    /// `searchWidget.focus(select, focusReplace)`.
    fn focus_search_widget(&mut self, select: bool, focus_replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let field = if focus_replace && self.search.replace_active { SearchField::Replace } else { SearchField::Query };
        self.focus_search_input(field, select, window, cx);
    }

    pub(super) fn focus_search_input(&mut self, field: SearchField, select: bool, window: &mut Window, cx: &mut Context<Self>) {
        if select {
            self.search.input_mut(field).value.select_all();
        }
        window.focus(&self.search.input(field).focus.clone(), cx);
        self.blink_caret(InputId::Search(field), window, cx);
        cx.notify();
    }

    fn focus_search_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.search.focus.clone(), cx);
        if self.search.focused.is_none() {
            self.search.focus_and_select(self.search.rows(&self.search_folders(), Some(&self.collapse_results())).first().map(|row| row.key.clone()));
        }
        cx.notify();
    }

    /// `findInFilesCommand` with `search.mode` "view".
    fn find_in_files(&mut self, args: &Value, window: &mut Window, cx: &mut Context<Self>) {
        self.open_search_view(cx);
        self.toggle_replace(Some(args["replace"].is_string()), window, cx);
        let updated = match args["query"].is_string() {
            true => false,
            false => self.update_text_from_selection(!args["replace"].is_string(), window, cx),
        };
        self.set_search_parameters(args, window, cx);
        if let Some(show) = args["showIncludesExcludes"].as_bool() {
            self.toggle_query_details(Some(show), false, window, cx);
        }
        self.focus_search_widget(true, updated, window, cx);
    }

    /// `setSearchParameters`.
    fn set_search_parameters(&mut self, args: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let view = &mut self.search;
        if let Some(value) = args["isCaseSensitive"].as_bool() {
            view.match_case = value;
        }
        if let Some(value) = args["matchWholeWord"].as_bool() {
            view.whole_word = value;
        }
        if let Some(value) = args["isRegex"].as_bool() {
            view.regex = value;
        }
        for (key, field) in [("filesToInclude", SearchField::Include), ("filesToExclude", SearchField::Exclude), ("query", SearchField::Query)] {
            if let Some(value) = args[key].as_str() {
                view.input_mut(field).value.set_value(value.to_string());
            }
        }
        view.input_mut(SearchField::Replace).value.set_value(args["replace"].as_str().unwrap_or_default().to_string());
        if let Some(value) = args["preserveCase"].as_bool() {
            view.preserve_case = value;
        }
        if let Some(value) = args["useExcludeSettingsAndIgnoreFiles"].as_bool() {
            view.use_excludes_and_ignore_files = value;
        }
        if let Some(value) = args["onlyOpenEditors"].as_bool() {
            view.only_open_editors = value;
        }
        self.search.validation = self.validate_search_input();
        if args["triggerSearch"] == true {
            self.trigger_search(true, None, window, cx);
        }
    }

    /// `updateTextFromSelection`: the active editor's selection, or the word at the cursor, seeds the search input
    /// (`getSelectionTextFromEditor`); setting the value searches on type.
    fn update_text_from_selection(&mut self, allow_unselected_word: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let seed = self.setting("editor.find.seedSearchStringFromSelection");
        if seed == false || seed == "never" {
            return false;
        }
        // getSearchTextFromEditor: nothing is seeded while the focus is in the Search view.
        let view = &self.search;
        if view.inputs.iter().map(|input| &input.focus).chain([&view.focus, &view.toggle_replace_focus]).any(|focus| focus.is_focused(window)) {
            return false;
        }
        let allow_unselected_word = allow_unselected_word && self.setting("search.seedWithNearestWord") == true;
        let Some(editor) = self.editors.active() else { return false };
        let selection = editor.selection();
        let lines = &editor.document.lines;
        let text = match selection.is_empty() {
            true if allow_unselected_word => {
                let line = &lines[selection.head.line];
                let Some(word) = crate::editor::word_at(line, selection.head.column) else { return false };
                line.chars().skip(word.start).take(word.len()).collect()
            }
            true => return false,
            false => crate::editor::text_between(lines, selection.start(), selection.end()),
        };
        let text = match self.search.regex {
            true => crate::editor::find::search::escape_regexp_characters(&text),
            false => text,
        };
        self.search.input_mut(SearchField::Query).value.set_value(text);
        self.search.validation = self.validate_search_input();
        if let Some(delay) = self.search_on_type_delay() {
            self.trigger_search(true, Some(delay), window, cx);
        }
        true
    }

    /// `edit_input_box` for a search input: the value changes, then the view reacts as the widget's change events do.
    pub(super) fn edit_search_input(&mut self, field: SearchField, edit: impl FnOnce(&mut InputBox), window: &mut Window, cx: &mut Context<Self>) {
        let before = self.search.value(field).to_string();
        edit(&mut self.search.input_mut(field).value);
        cx.notify();
        if self.search.value(field) == before {
            return;
        }
        match field {
            SearchField::Query => {
                self.search.validation = self.validate_search_input();
                if let Some(delay) = self.search_on_type_delay() {
                    self.trigger_search(true, Some(delay), window, cx);
                }
            }
            SearchField::Replace => {
                self.search.replace_history_update = Some(cx.spawn(async |workbench, cx| {
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                    let _ = workbench.update(cx, |workbench, _| workbench.add_to_search_history(&[SearchField::Replace]));
                }));
            }
            SearchField::Include | SearchField::Exclude => {
                let delay = self.debounce_period();
                self.trigger_search(true, Some(delay), window, cx);
            }
        }
    }

    /// `onSearchInputChanged`: the debounce period, scaled by how much a regex matches.
    fn search_on_type_delay(&self) -> Option<Duration> {
        let period = self.debounce_period();
        if !self.search.regex {
            return Some(period);
        }
        let regex = onig::Regex::new(self.search.value(SearchField::Query)).ok()?;
        let matchiness = regex.find_iter(MATCHINESS_TEXT).count();
        let multiplier = match matchiness {
            0..50 => 1,
            50..100 => 5,
            _ => 10,
        };
        Some(period * multiplier)
    }

    fn debounce_period(&self) -> Duration {
        Duration::from_millis(self.setting("search.searchOnTypeDebouncePeriod").as_u64().unwrap_or(300))
    }

    /// `validateSearchInput`: a regex must compile.
    fn validate_search_input(&self) -> Option<String> {
        let value = self.search.value(SearchField::Query);
        if value.is_empty() || !self.search.regex {
            return None;
        }
        onig::Regex::new(value).err().map(|error| format!("Invalid regular expression: /{value}/: {error}"))
    }

    /// The keys of the search inputs (`onSearchInputKeyDown`, `onReplaceInputKeyDown`, patternInputWidget.ts).
    pub(super) fn search_input_key(&mut self, field: SearchField, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let modifiers = keystroke.modifiers;
        let plain = !modifiers.control && !modifiers.platform && !modifiers.alt && !modifiers.shift;
        match keystroke.key.as_str() {
            "enter" if modifiers.control && !modifiers.platform && !modifiers.alt && !modifiers.shift && matches!(field, SearchField::Query | SearchField::Replace) => {
                self.edit_input_box(InputId::Search(field), window, cx, |input| input.insert(None, "\n"));
            }
            "enter" if plain => self.trigger_search(true, None, window, cx),
            "escape" if plain && matches!(field, SearchField::Include | SearchField::Exclude) => self.cancel_search(false, window, cx),
            "escape" if plain && field == SearchField::Query => self.cancel_search(true, window, cx),
            "tab" if !modifiers.control && !modifiers.platform && !modifiers.alt => self.focus_next_search_input(!modifiers.shift, window, cx),
            _ => return false,
        }
        true
    }

    /// `focusNextInputBox` and `focusPreviousInputBox`: the visible inputs in order, the results after the last.
    fn focus_next_search_input(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let visible: Vec<SearchField> = SEARCH_FIELDS
            .into_iter()
            .filter(|field| match field {
                SearchField::Query => true,
                SearchField::Replace => self.search.replace_active,
                SearchField::Include | SearchField::Exclude => self.search.details_visible,
            })
            .collect();
        let current = self.focused_input(window).and_then(|id| match id {
            InputId::Search(field) => visible.iter().position(|known| *known == field),
            _ => None,
        });
        let next = match (current, forward) {
            (Some(index), true) => visible.get(index + 1).copied(),
            (Some(0), false) => None,
            (Some(index), false) => Some(visible[index - 1]),
            (None, false) => visible.last().copied(),
            (None, true) => None,
        };
        match next {
            Some(field) => self.focus_search_input(field, true, window, cx),
            None if forward && !self.search.result.is_empty() => self.focus_search_list(window, cx),
            None => {}
        }
    }

    /// `onToggleReplaceButton` when `show` differs from the replace input's visibility.
    pub(super) fn toggle_replace(&mut self, show: Option<bool>, _window: &mut Window, cx: &mut Context<Self>) {
        let show = show.unwrap_or(!self.search.replace_active);
        self.search.replace_active = show;
        cx.notify();
    }

    /// `toggleQueryDetails`: shown details take the focus into the include input.
    pub(super) fn toggle_query_details(&mut self, show: Option<bool>, move_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let show = show.unwrap_or(!self.search.details_visible);
        self.search.details_visible = show;
        cx.notify();
        if !move_focus {
            return;
        }
        match show {
            true => self.focus_search_input(SearchField::Include, true, window, cx),
            false => self.focus_search_widget(true, false, window, cx),
        }
    }

    /// A toggle of the widgets: the query toggles and the include and exclude ones search again, preserve case only
    /// changes the replace previews.
    pub(super) fn toggle_search_option(&mut self, option: SearchOption, window: &mut Window, cx: &mut Context<Self>) {
        let view = &mut self.search;
        match option {
            SearchOption::MatchCase => view.match_case = !view.match_case,
            SearchOption::WholeWord => view.whole_word = !view.whole_word,
            SearchOption::Regex => view.regex = !view.regex,
            SearchOption::PreserveCase => view.preserve_case = !view.preserve_case,
            SearchOption::OnlyOpenEditors => view.only_open_editors = !view.only_open_editors,
            SearchOption::UseExcludes => view.use_excludes_and_ignore_files = !view.use_excludes_and_ignore_files,
        }
        cx.notify();
        if option == SearchOption::PreserveCase {
            return;
        }
        self.search.validation = self.validate_search_input();
        self.trigger_search(true, None, window, cx);
    }

    /// A link of the message line.
    pub(super) fn search_message_link(&mut self, link: MessageLink, window: &mut Window, cx: &mut Context<Self>) {
        match link {
            MessageLink::SearchAgain => self.trigger_search(false, None, window, cx),
            // `onSearchAgain`.
            MessageLink::SearchAgainInAllFiles => {
                self.search.input_mut(SearchField::Include).value.set_value(String::new());
                self.search.input_mut(SearchField::Exclude).value.set_value(String::new());
                self.search.only_open_editors = false;
                self.trigger_search(false, None, window, cx);
            }
            // `onOpenSettings`.
            MessageLink::OpenSettings => {
                let command = if self.folder.is_some() { "workbench.action.openWorkspaceSettings" } else { "workbench.action.openSettings" };
                let query = json!({ "query": "@id:files.exclude,search.exclude,search.useParentIgnoreFiles,search.useGlobalIgnoreFiles,search.useIgnoreFiles" });
                self.execute_command(command, vec![query], window, cx);
            }
            // `onEnableExcludes`, `onDisableSearchInOpenEditors`.
            MessageLink::EnableExcludes | MessageLink::DisableOpenEditors => {
                self.toggle_query_details(Some(true), false, window, cx);
                match link {
                    MessageLink::EnableExcludes => self.search.use_excludes_and_ignore_files = true,
                    _ => self.search.only_open_editors = false,
                }
                self.trigger_search(true, None, window, cx);
            }
            MessageLink::OpenInEditor => {
                self.execute_command("search.action.openInEditor", Vec::new(), window, cx);
            }
            MessageLink::OpenFolder => {
                self.execute_command("workbench.action.files.openFolder", Vec::new(), window, cx);
            }
        }
    }

    /// `triggerQueryChange`: a search typed into an input waits for `delay` and runs only with `search.searchOnType`.
    pub(super) fn trigger_search(&mut self, preserve_focus: bool, delay: Option<Duration>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(delay) = delay else {
            self.search.delayed = None;
            self.run_search(preserve_focus, window, cx);
            return;
        };
        if self.setting("search.searchOnType") != true {
            return;
        }
        self.search.delayed = Some(cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(delay).await;
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.run_search(preserve_focus, window, cx));
        }));
    }

    /// `_onQueryChanged`, `validateQuery` and `doSearch`: the text search over the workspace folders, the open editors'
    /// models in place of their files.
    fn run_search(&mut self, preserve_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.validation.is_some() {
            return;
        }
        let config = self.search.configuration();
        if config.query.is_empty() {
            self.clear_search_results(false, cx);
            self.search.message = None;
            return;
        }
        let query = match self.text_query(&config) {
            Ok(query) => query,
            Err(error) => return self.show_search_error(error, cx),
        };
        self.search.history_update = Some(cx.spawn(async |workbench, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ = workbench.update(cx, |workbench, _| workbench.add_to_search_history(&[SearchField::Query, SearchField::Exclude, SearchField::Include]));
        }));
        if !preserve_focus {
            self.focus_search_widget(false, false, window, cx);
        }
        if let Some(cancellation) = self.search.cancellation.take() {
            cancellation.cancel();
        }
        self.search.validation = None;
        self.search.state = SearchUiState::Searching;
        self.search.focus_and_select(None);
        self.search.collapsed.clear();
        self.search.warnings.clear();
        let mut result = SearchResult::default();
        let local = self.add_open_editor_matches(&query, config.only_open_editors, &mut result);
        self.search.result = result;
        self.search.pattern = query.content_pattern.clone();
        self.update_result_count_message();
        cx.notify();
        let (cancellation, mut receiver, search) = self.spawn_text_search(query, local, cx);
        self.search.cancellation = Some(cancellation.clone());
        let slow = cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(SLOW_SEARCH).await;
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.search.state = SearchUiState::SlowSearch;
                cx.notify();
            });
        });
        let (include, exclude) = (config.files_to_include.trim().to_string(), config.files_to_exclude.trim().to_string());
        self.search.run = Some(cx.spawn_in(window, async move |workbench, cx| {
            while let Some(first) = receiver.next().await {
                let mut batch = vec![first];
                while let Ok(next) = receiver.try_recv() {
                    batch.push(next);
                }
                let updated = workbench.update(cx, |workbench, cx| {
                    let folders = workbench.search_folders();
                    for (path, results) in batch {
                        let folder = folders.iter().position(|folder| path.starts_with(folder)).unwrap_or(folders.len());
                        workbench.search.result.add(folder, &path, &results);
                    }
                    workbench.update_result_count_message();
                    cx.notify();
                });
                if updated.is_err() {
                    return;
                }
            }
            let finished = search.await;
            drop(slow);
            let _ = workbench.update(cx, |workbench, cx| workbench.search_complete(finished, &cancellation, &include, &exclude, cx));
        }));
    }

    /// `queryBuilder.text` over the workspace folders, restricted to the open editors when asked, and `validateQuery`:
    /// missing folders drop out; when none exists the first one is the error.
    pub(super) fn text_query(&self, config: &SearchConfiguration) -> Result<search::query::Query, String> {
        let include = config.files_to_include.trim();
        let exclude = config.files_to_exclude.trim();
        let use_excludes = config.use_exclude_settings_and_ignore_files;
        let pattern = json!({ "pattern": config.query, "isRegExp": config.is_regexp, "isCaseSensitive": config.is_case_sensitive, "isWordMatch": config.match_whole_word });
        let options = json!({
            "maxResults": self.setting("search.maxResults"),
            "disregardIgnoreFiles": (!use_excludes).then_some(true),
            "disregardExcludeSettings": (!use_excludes).then_some(true),
            "ignoreGlobCase": true,
            "previewOptions": { "matchLines": 1, "charsPerLine": if config.is_regexp { 10000 } else { 1000 } },
            "surroundingContext": config.context_lines,
            "isSmartCase": self.setting("search.smartCase"),
        });
        let workspace_folders: Vec<_> = self.workspace().iter().flat_map(|workspace| &workspace.folders).map(|folder| (folder.path.clone(), folder.name.clone())).collect();
        let folders: Vec<WorkspaceFolder> = workspace_folders.iter().map(|(path, name)| WorkspaceFolder { path, name }).collect();
        let home = std::env::var("HOME").ok();
        let workspace = search_paths::Workspace { folders: &folders, single_folder: folders.len() == 1 && self.workspace().is_some_and(|workspace| workspace.file.is_none()), home: home.as_deref() };
        let settings = self.settings.clone();
        let setting = move |folder: &Path, key: &str| settings.lock().unwrap().get_value(Some(key), &Overrides { resource: Some(folder), ..Overrides::default() }).unwrap_or(Value::Null);
        let mut query = search_paths::expanded_text_query(&pattern, &workspace, include, exclude, &options, &setting)?;
        if config.only_open_editors {
            let open_editors: Vec<PathBuf> = self.editors.editors.iter().filter(|editor| editor.document.scheme == "file").map(|editor| editor.document.path.clone()).collect();
            search_paths::restrict_to_open_editors(&mut query, &open_editors, &workspace);
        }
        let searched = query.folder_queries.len();
        query.folder_queries.retain(|folder_query| folder_query.folder.exists());
        if searched > 0 && query.folder_queries.is_empty() {
            let missing = search_paths::expanded_text_query(&pattern, &workspace, include, exclude, &options, &setting).map(|query| query.folder_queries[0].folder.clone());
            return Err(format!("Search path not found: {}", missing.unwrap_or_default().display()));
        }
        Ok(query)
    }

    /// `getOpenEditorResults`: an open editor's model answers for its file, with or without matches; returns the
    /// paths the text search skips.
    pub(super) fn add_open_editor_matches(&self, query: &search::query::Query, only_open_editors: bool, result: &mut SearchResult) -> Vec<PathBuf> {
        let workspace_folders: Vec<_> = self.workspace().iter().flat_map(|workspace| &workspace.folders).map(|folder| (folder.path.clone(), folder.name.clone())).collect();
        let content_pattern = query.content_pattern.as_ref().expect("a text query has a content pattern");
        let word_separators = self.setting("editor.wordSeparators").as_str().unwrap_or_default().to_string();
        let params = SearchParams { search_string: &content_pattern.pattern, is_regex: content_pattern.is_reg_exp, match_case: content_pattern.is_case_sensitive, word_separators: content_pattern.is_word_match.then_some(word_separators.as_str()) };
        let mut local = Vec::new();
        for editor in &self.editors.editors {
            let document = &editor.document;
            if !matches!(document.scheme.as_str(), "file" | "untitled") || !search::query::path_included_in_query(query, &document.path) {
                continue;
            }
            let in_folder = workspace_folders.iter().any(|(folder, _)| document.path.starts_with(folder)) || query.folder_queries.iter().any(|folder_query| document.path.starts_with(&folder_query.folder));
            if only_open_editors || document.scheme == "untitled" || in_folder || !query.using_search_paths {
                local.push(document.path.clone());
                let folder = search_folder_index(&workspace_folders, &document.path);
                result.set_model_matches(folder, &document.path, &document.lines, &params);
                if let Some(file) = result.files.iter_mut().find(|file| file.path == document.path) {
                    file.add_model_context(&document.lines, query.surrounding_context);
                }
            }
        }
        local
    }

    /// Runs the text search on a background thread; the files it finds, except `skip`, arrive on the receiver.
    pub(super) fn spawn_text_search(&self, query: search::query::Query, skip: Vec<PathBuf>, cx: &mut Context<Self>) -> (Arc<Cancellation>, UnboundedReceiver<(PathBuf, Vec<Value>)>, Task<Result<bool, String>>) {
        let rg = search::ripgrep::rg_path(&self.app_root);
        let cancellation = Arc::new(Cancellation::default());
        let (sender, receiver) = futures::channel::mpsc::unbounded::<(PathBuf, Vec<Value>)>();
        let search_cancellation = cancellation.clone();
        let search = cx.background_spawn(async move {
            match query.folder_queries.is_empty() {
                true => Ok(false),
                false => search::text_search::search(&query, &rg, &search_cancellation, &mut |path, results| {
                    if !skip.iter().any(|known| known == path) {
                        let _ = sender.unbounded_send((path.to_path_buf(), results));
                    }
                }),
            }
        });
        (cancellation, receiver, search)
    }

    /// `onSearchComplete` and `onSearchError`.
    fn search_complete(&mut self, finished: Result<bool, String>, cancellation: &Cancellation, include: &str, exclude: &str, cx: &mut Context<Self>) {
        self.search.state = SearchUiState::Idle;
        self.search.cancellation = None;
        let limit_hit = match finished {
            Ok(limit_hit) => limit_hit,
            Err(error) if !cancellation.is_cancelled() => return self.show_search_error(error, cx),
            Err(_) => false,
        };
        let completed = !cancellation.is_cancelled();
        if self.search.result.is_empty() {
            let (message, link) = no_results_message(completed, self.search.only_open_editors, include, exclude);
            self.search.message = Some(vec![MessagePart::Text(message), MessagePart::Link(link)]);
            if self.folder.is_none() {
                self.search.message = Some(vec![MessagePart::Text("You have not opened or specified a folder. Only open files are currently searched - ".into()), MessagePart::Link(MessageLink::OpenFolder)]);
            }
        } else {
            self.update_result_count_message();
        }
        if completed && limit_hit {
            self.search.warnings.push(LIMIT_HIT_WARNING.to_string());
        }
        cx.notify();
    }

    /// `onQueryValidationError`: the search input shows the error, the results go.
    fn show_search_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.search.state = SearchUiState::Idle;
        self.search.validation = Some(error);
        self.search.result = Default::default();
        cx.notify();
    }

    /// `updateSearchResultCount`.
    fn update_result_count_message(&mut self) {
        let view = &self.search;
        let files = view.result.file_count();
        let mut parts = vec![MessagePart::Text(result_count_message(view.result.count(), files))];
        if files > 0 {
            if !view.use_excludes_and_ignore_files {
                parts.extend([MessagePart::Text(" - exclude settings and ignore files are disabled (".into()), MessagePart::Link(MessageLink::EnableExcludes), MessagePart::Text(")".into())]);
            }
            if view.only_open_editors {
                parts.extend([MessagePart::Text(" - searching only in open files (".into()), MessagePart::Link(MessageLink::DisableOpenEditors), MessagePart::Text(")".into())]);
            }
            parts.extend([MessagePart::Text(" - ".into()), MessagePart::Link(MessageLink::OpenInEditor)]);
        }
        self.search.message = Some(parts);
    }

    /// `cancelSearch`: a running search stops; its results stay.
    fn cancel_search(&mut self, focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cancellation) = self.search.cancellation.as_ref() else { return };
        cancellation.cancel();
        if focus {
            self.focus_search_widget(true, false, window, cx);
        }
    }

    /// `clearSearchResults`: with `clear_input` the inputs empty too, the include and exclude ones once the others are.
    pub(super) fn clear_search_results(&mut self, clear_input: bool, cx: &mut Context<Self>) {
        self.search.result = Default::default();
        self.search.focus_and_select(None);
        self.search.message = None;
        self.search.warnings.clear();
        self.search.validation = None;
        if clear_input {
            if self.search.value(SearchField::Query).is_empty() && self.search.value(SearchField::Replace).is_empty() {
                self.search.input_mut(SearchField::Include).value.set_value(String::new());
                self.search.input_mut(SearchField::Exclude).value.set_value(String::new());
            }
            self.search.input_mut(SearchField::Query).value.set_value(String::new());
            self.search.input_mut(SearchField::Replace).value.set_value(String::new());
        }
        self.search.delayed = None;
        if let Some(cancellation) = self.search.cancellation.take() {
            cancellation.cancel();
        }
        self.search.run = None;
        self.search.state = SearchUiState::Idle;
        cx.notify();
    }

    /// A model with results changed: its file's matches are found again in it (`FileMatchImpl.updateMatchesForModel`).
    pub(super) fn search_model_changed(&mut self, scheme: &str, path: &Path) {
        let Some(pattern) = self.search.pattern.as_ref() else { return };
        let Some(file) = self.search.result.files.iter().find(|file| file.path == path) else { return };
        let Some(model) = self.editors.model(scheme, path) else { return };
        let word_separators = self.setting("editor.wordSeparators").as_str().unwrap_or_default().to_string();
        let params = SearchParams { search_string: &pattern.pattern, is_regex: pattern.is_reg_exp, match_case: pattern.is_case_sensitive, word_separators: pattern.is_word_match.then_some(word_separators.as_str()) };
        let folder = file.folder;
        self.search.result.set_model_matches(folder, path, &model.document.lines, &params);
        self.update_result_count_message();
    }

    /// A click on a result row: a match opens in a preview editor, keeping the focus (`onPointer`), or pinned and
    /// focused on a double click; a file or folder row expands or collapses.
    pub(super) fn search_row_clicked(&mut self, key: &RowKey, click_count: usize, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.search.focus.clone(), cx);
        self.search.focus_and_select(Some(key.clone()));
        cx.notify();
        match key {
            RowKey::Match(..) if click_count == 1 => self.open_search_row(key, true, false, window, cx),
            RowKey::Match(..) => self.open_search_row(key, false, true, window, cx),
            _ if click_count == 1 => self.toggle_search_row(key, cx),
            _ => {}
        }
    }

    fn toggle_search_row(&mut self, key: &RowKey, cx: &mut Context<Self>) {
        let rows = self.search.rows(&self.search_folders(), Some(&self.collapse_results()));
        let collapsed = rows.iter().find(|row| row.key == *key).and_then(|row| match row.kind {
            RowKind::Folder { collapsed, .. } | RowKind::File { collapsed, .. } => Some(collapsed),
            RowKind::Match { .. } => None,
        });
        if let Some(collapsed) = collapsed {
            self.search.collapsed.insert(key.clone(), !collapsed);
            cx.notify();
        }
    }

    /// `list.expand` and `list.collapse`: a collapsed row expands, an expanded one collapses, else the focus moves to
    /// the first child or the parent.
    fn expand_search_row(&mut self, expand: bool, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.search.rows(&self.search_folders(), Some(&self.collapse_results()));
        let Some(index) = rows.iter().position(|row| Some(&row.key) == self.search.focused.as_ref()) else { return };
        let row = &rows[index];
        let collapsed = match row.kind {
            RowKind::Folder { collapsed, .. } | RowKind::File { collapsed, .. } => Some(collapsed),
            RowKind::Match { .. } => None,
        };
        match (expand, collapsed) {
            (true, Some(true)) | (false, Some(false)) => {
                self.search.collapsed.insert(row.key.clone(), !expand);
                cx.notify();
            }
            (true, Some(false)) => self.move_search_focus(1, window, cx),
            (false, _) => {
                if let Some(parent) = rows[..index].iter().rev().find(|parent| parent.depth < row.depth) {
                    self.search.focus_and_select(Some(parent.key.clone()));
                    self.search.scroll.scroll_to_item(rows.iter().position(|known| known.key == parent.key).unwrap_or(0), gpui::ScrollStrategy::Top);
                    cx.notify();
                }
            }
            (true, None) => {}
        }
    }

    /// `list.focusDown` and the others: the focus moves by `step` rows, without wrapping; the focused match opens in a
    /// preview editor, keeping the focus (`selectionNavigation`).
    fn move_search_focus(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.search.rows(&self.search_folders(), Some(&self.collapse_results()));
        if rows.is_empty() {
            return;
        }
        let current = rows.iter().position(|row| Some(&row.key) == self.search.focused.as_ref());
        let last = rows.len() as isize - 1;
        let next = match (current, step) {
            (_, isize::MIN) => 0,
            (_, isize::MAX) => last,
            (None, _) => 0,
            (Some(index), step) => (index as isize + step).clamp(0, last),
        } as usize;
        let key = rows[next].key.clone();
        self.search.focus_and_select(Some(key.clone()));
        self.search.scroll.scroll_to_item(next, gpui::ScrollStrategy::Top);
        cx.notify();
        if matches!(key, RowKey::Match(..)) {
            self.open_search_row(&key, true, false, window, cx);
        }
    }

    /// `selectNextMatch` and `selectPreviousMatch`: the next match in tree order, around the end, revealed and opened
    /// keeping the focus.
    fn select_search_match(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let folders = self.search_folders();
        let rows = self.search.rows(&folders, None);
        let matches: Vec<usize> = (0..rows.len()).filter(|&index| matches!(rows[index].key, RowKey::Match(..))).collect();
        if matches.is_empty() {
            return;
        }
        let current = rows.iter().position(|row| Some(&row.key) == self.search.focused.as_ref());
        let next = match (current, forward) {
            (None, true) => matches[0],
            (None, false) => matches[matches.len() - 1],
            (Some(index), true) => matches.iter().copied().find(|&row| row > index).unwrap_or(matches[0]),
            (Some(index), false) => matches.iter().rev().copied().find(|&row| row < index).unwrap_or(matches[matches.len() - 1]),
        };
        let key = rows[next].key.clone();
        for ancestor in rows[..next].iter().filter(|row| !matches!(row.kind, RowKind::Match { .. })) {
            if let RowKey::File(path) | RowKey::Folder(path) = &ancestor.key
                && let RowKey::Match(file, _) = &key
                && file.starts_with(path)
            {
                self.search.collapsed.insert(ancestor.key.clone(), false);
            }
        }
        self.search.focus_and_select(Some(key.clone()));
        let visible = self.search.rows(&folders, Some(&self.collapse_results()));
        if let Some(index) = visible.iter().position(|row| row.key == key) {
            self.search.scroll.scroll_to_item(index, gpui::ScrollStrategy::Center);
        }
        cx.notify();
        self.open_search_row(&key, true, false, window, cx);
    }

    /// `searchView.open`: the match's file opens with the match selected (`getEditorSelectionFromMatch`), a file's
    /// last match for a file row.
    // shortcut: with a replace string upstream opens the replace preview diff editor, which waits for the diff editor.
    fn open_search_row(&mut self, key: &RowKey, preserve_focus: bool, pinned: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, found)) = self.search_row_match(key) else { return };
        let mut range = found.range;
        if self.search.replace_active && !self.search.value(SearchField::Replace).is_empty()
            && let Some(pattern) = self.search.replace_pattern()
        {
            let replace = found.replace_string(&pattern, self.search.preserve_case);
            range = SearchRange { start_line: range.start_line, start_column: range.start_column, end_line: range.start_line, end_column: range.start_column + utf16_len(&replace) };
        }
        self.open_file(path.clone(), pinned, window, cx);
        let Some(editor) = self.editors.active().filter(|editor| editor.document.path == path) else { return };
        let lines = &editor.document.lines;
        let column = |line: usize, utf16: usize| lines.get(line).map_or(utf16, |text| char_column(text, utf16)) + 1;
        let text_range = TextRange { start_line_number: range.start_line + 1, start_column: column(range.start_line, range.start_column), end_line_number: range.end_line + 1, end_column: column(range.end_line, range.end_column) };
        self.select_range(&path, text_range, cx);
        match preserve_focus {
            true => window.focus(&self.search.focus.clone(), cx),
            false => window.focus(&self.editor_focus.clone(), cx),
        }
    }

    fn search_row_match(&self, key: &RowKey) -> Option<(PathBuf, crate::search_model::Match)> {
        match key {
            RowKey::Match(path, range) => {
                let file = self.search.result.files.iter().find(|file| file.path == *path)?;
                let found = file.matches.iter().find(|found| [found.range.start_line, found.range.start_column, found.range.end_line, found.range.end_column] == *range)?;
                Some((path.clone(), found.clone()))
            }
            RowKey::File(path) => {
                let file = self.search.result.files.iter().find(|file| file.path == *path)?;
                Some((path.clone(), file.matches.last()?.clone()))
            }
            RowKey::Folder(_) => None,
        }
    }

    /// The file indexes and match indexes a row stands for.
    fn search_row_targets(&self, key: &RowKey) -> Vec<(usize, Option<usize>)> {
        let files = &self.search.result.files;
        match key {
            RowKey::Match(path, range) => {
                let Some(file) = files.iter().position(|file| file.path == *path) else { return Vec::new() };
                let index = files[file].matches.iter().position(|found| [found.range.start_line, found.range.start_column, found.range.end_line, found.range.end_column] == *range);
                index.map(|index| vec![(file, Some(index))]).unwrap_or_default()
            }
            RowKey::File(path) => files.iter().position(|file| file.path == *path).map(|file| vec![(file, None)]).unwrap_or_default(),
            RowKey::Folder(folder) => (0..files.len()).filter(|&file| files[file].path.starts_with(folder)).map(|file| (file, None)).collect(),
        }
    }

    /// The inline actions of a row: Dismiss removes it from the results, Replace replaces the match or every match of
    /// the file or folder (`ReplaceAction`, `ReplaceAllAction`, `ReplaceAllInFolderAction`). The focus moves to the row
    /// that takes its place.
    pub(super) fn search_row_action(&mut self, key: &RowKey, action: RowAction, window: &mut Window, cx: &mut Context<Self>) {
        let folders = self.search_folders();
        let position = self.search.rows(&folders, Some(&self.collapse_results())).iter().position(|row| row.key == *key);
        let targets = self.search_row_targets(key);
        match action {
            RowAction::Dismiss => self.dismiss_search_targets(&targets),
            RowAction::Replace => {
                if !self.search.replace_active {
                    return;
                }
                self.replace_search_targets(&targets, cx);
                // A file or folder replaced leaves the results; a replaced match follows its model.
                if !matches!(key, RowKey::Match(..)) {
                    self.dismiss_search_targets(&targets);
                }
            }
        }
        self.update_result_count_message();
        let rows = self.search.rows(&folders, Some(&self.collapse_results()));
        if self.search.focused.as_ref() == Some(key) || self.search.focused.is_none() {
            self.search.focus_and_select(position.and_then(|index| rows.get(index.min(rows.len().saturating_sub(1)))).map(|row| row.key.clone()));
        }
        window.focus(&self.search.focus.clone(), cx);
        cx.notify();
    }

    fn dismiss_search_targets(&mut self, targets: &[(usize, Option<usize>)]) {
        for &(file, index) in targets.iter().rev() {
            match index {
                Some(index) => self.search.result.remove_match(file, index),
                None => self.search.result.remove_file(file),
            }
        }
    }

    /// `ReplaceService.replace`: one bulk edit of the matches' replace strings, then every edited model saves.
    fn replace_search_targets(&mut self, targets: &[(usize, Option<usize>)], cx: &mut Context<Self>) {
        let Some(pattern) = self.search.replace_pattern() else { return };
        let mut edits = Vec::new();
        let mut paths = Vec::new();
        for &(file, index) in targets {
            let file_match = &self.search.result.files[file];
            let scheme = if self.editors.model("untitled", &file_match.path).is_some() { "untitled" } else { "file" };
            paths.push((scheme, file_match.path.clone()));
            let indexes: Vec<usize> = match index {
                Some(index) => vec![index],
                None => (0..file_match.matches.len()).collect(),
            };
            for index in indexes {
                let found = &file_match.matches[index];
                let range = &found.range;
                edits.push(json!({
                    "resource": { "scheme": scheme, "path": file_match.path.to_string_lossy() },
                    "textEdit": {
                        "range": { "startLineNumber": range.start_line + 1, "startColumn": range.start_column + 1, "endLineNumber": range.end_line + 1, "endColumn": range.end_column + 1 },
                        "text": found.replace_string(&pattern, self.search.preserve_case),
                    },
                }));
            }
        }
        if let Err(error) = self.apply_workspace_text_edits(&Value::Array(edits), &Value::Null, cx) {
            eprintln!("search replace: {error}");
            return;
        }
        for (scheme, path) in paths {
            drop(self.save_model(scheme.to_string(), path, cx));
        }
    }

    /// `replaceAll`: confirmed, every match is replaced, the results go and the message tells what was replaced.
    fn replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let result = &self.search.result;
        if !self.search.replace_active || result.is_empty() {
            return Outcome::done();
        }
        let (occurrences, files) = (result.count(), result.file_count());
        let replace_value = self.search.value(SearchField::Replace).to_string();
        let message = replace_all_confirmation_message(occurrences, files, &replace_value);
        cx.spawn_in(window, async move |workbench, cx| {
            if !confirm(&workbench, cx, PromptLevel::Info, message, None, "Replace").await {
                return;
            }
            let _ = workbench.update(cx, |workbench, cx| {
                let targets: Vec<(usize, Option<usize>)> = (0..workbench.search.result.files.len()).map(|file| (file, None)).collect();
                workbench.replace_search_targets(&targets, cx);
                workbench.search.result = Default::default();
                workbench.search.focus_and_select(None);
                workbench.search.message = Some(vec![MessagePart::Text(after_replace_all_message(occurrences, files, &replace_value))]);
                cx.notify();
            });
        })
        .detach();
        Outcome::done()
    }

    /// `collapseDeepestExpandedLevel`: the expanded rows at the deepest level that has any collapse.
    fn collapse_deepest_expanded_level(&mut self, cx: &mut Context<Self>) {
        let rows = self.search.rows(&self.search_folders(), Some(&self.collapse_results()));
        let expanded: Vec<_> = rows.iter().filter(|row| matches!(row.kind, RowKind::Folder { collapsed: false, .. } | RowKind::File { collapsed: false, .. })).collect();
        let Some(deepest) = expanded.iter().map(|row| row.depth).max() else { return };
        for row in expanded.into_iter().filter(|row| row.depth == deepest) {
            self.search.collapsed.insert(row.key.clone(), true);
        }
        cx.notify();
    }

    /// `copyMatchCommand`: a match as `line,column: text`, a file as its path and its matches, a folder as its files.
    fn search_row_text(&self, key: &RowKey) -> String {
        match key {
            RowKey::Match(..) => self.search_row_match(key).map(|(_, found)| search_match_text(&found, 0)).unwrap_or_default(),
            _ => self.search_row_targets(key).iter().map(|&(file, _)| self.search_file_text(file)).collect::<Vec<_>>().join("\n\n"),
        }
    }

    /// searchActionsCopy.ts `fileMatchToString`.
    fn search_file_text(&self, file: usize) -> String {
        let file_match = &self.search.result.files[file];
        let rows: Vec<String> = file_match.matches.iter().map(|found| search_match_text(found, 2)).collect();
        format!("{}\n{}", self.tildify(&file_match.path), rows.join("\n"))
    }
}

/// searchActionsCopy.ts `matchToString`.
fn search_match_text(found: &crate::search_model::Match, indent: usize) -> String {
    let prefix = format!("{},{}", found.range.start_line + 1, found.range.start_column + 1);
    format!("{}{prefix}: {}", " ".repeat(indent), found.full_match_text(true))
}

/// searchActionsFind.ts `extractSearchFilePattern`: `*.` and everything after the name's first dot.
fn extract_search_file_pattern(file_name: &str) -> String {
    match file_name.split_once('.') {
        Some((_, extension)) => format!("*.{extension}"),
        None => file_name.to_string(),
    }
}

/// searchActionsFind.ts `mergeSearchPatternIfNotExists`.
fn merge_search_pattern_if_not_exists(current: &str, pattern: &str) -> String {
    if current.trim().is_empty() {
        return pattern.to_string();
    }
    if current.split(',').map(str::trim).any(|existing| existing == pattern) {
        return current.to_string();
    }
    format!("{current}, {pattern}")
}

fn search_folder_index(folders: &[(PathBuf, String)], path: &Path) -> usize {
    folders.iter().position(|(folder, _)| path.starts_with(folder)).unwrap_or(folders.len())
}