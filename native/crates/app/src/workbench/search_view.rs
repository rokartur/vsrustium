//! The Search view as searchView.ts, searchWidget.ts and searchResultsView.ts draw it (media/searchview.css): the
//! search and replace inputs with their toggles, the include and exclude inputs behind "...", the message line and
//! the results tree of file headers with their count badges and match previews. The commands and the search run are
//! in search_actions.rs, the results model in search_model.rs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use super::list_scroll::ListId;
use gpui::{ClickEvent, Context, Div, FocusHandle, MouseButton, MouseDownEvent, Rgba, Task, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list};
use search::ripgrep::Cancellation;

use super::Workbench;
use super::input_box_view::{InputId, InputLook, InputView};
use super::find_widget::HISTORY_LIMIT;
use crate::file_icons::FolderIcon;
use crate::history::HistoryNavigator;
use crate::input_box::InputBox;
use crate::search_editor::SearchConfiguration;
use crate::search_model::{MessageLink, PatternInfo, ReplacePattern, SearchResult, compare_paths};

/// The view the side bar shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SideBarView {
    Explorer,
    Search,
    Scm,
    Extensions,
    Debug,
    Testing,
    /// A container of `viewsContainers.activitybar`, by its `workbench.view.extension.<id>` id.
    Contributed(&'static str),
}

/// The search view's inputs, in their tab order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SearchField {
    Query,
    Replace,
    Include,
    Exclude,
    /// The search editor's context lines input (`contextLinesInput`).
    ContextLines,
}

pub const SEARCH_FIELDS: [SearchField; 4] = [SearchField::Query, SearchField::Replace, SearchField::Include, SearchField::Exclude];

pub struct SearchInput {
    pub value: InputBox,
    pub history: HistoryNavigator,
    pub view: InputView,
    pub focus: FocusHandle,
}

/// `SearchUIState`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SearchUiState {
    Idle,
    Searching,
    SlowSearch,
}

#[derive(Clone, PartialEq, Debug)]
pub enum MessagePart {
    Text(String),
    Link(MessageLink),
}

/// A row of the results tree, the key its focus and collapse state are kept under.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum RowKey {
    Folder(PathBuf),
    File(PathBuf),
    /// The file and the match's start and end, 0-based lines and UTF-16 columns.
    Match(PathBuf, [usize; 4]),
}

#[derive(Clone, Debug)]
pub struct ResultRow {
    pub key: RowKey,
    /// 1 for the top level.
    pub depth: usize,
    pub kind: RowKind,
}

#[derive(Clone, Debug)]
pub enum RowKind {
    /// A workspace folder of a multi-root workspace, or a folder of the tree view, with its files' indexes.
    Folder { label: String, files: Vec<usize>, collapsed: bool },
    File { file: usize, collapsed: bool },
    Match { file: usize, index: usize },
}

pub struct SearchView {
    pub inputs: [SearchInput; 5],
    /// The results tree.
    pub focus: FocusHandle,
    /// The replace toggle is a button: a click focuses it and leaves the inputs.
    pub toggle_replace_focus: FocusHandle,
    pub scroll: UniformListScrollHandle,
    pub replace_active: bool,
    pub details_visible: bool,
    pub match_case: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub preserve_case: bool,
    pub only_open_editors: bool,
    pub use_excludes_and_ignore_files: bool,
    /// The search editor's context lines toggle (`showContextToggle`).
    pub show_context: bool,
    /// `search.defaultViewMode` until a view action changes it: the tree view groups files by folder.
    pub tree_view: bool,
    pub result: SearchResult,
    /// The content pattern of the shown results, with smart case applied; models that change are searched with it.
    pub pattern: Option<search::query::PatternInfo>,
    /// The collapse state a row was given by the user or by a collapse or expand action.
    pub collapsed: HashMap<RowKey, bool>,
    pub focused: Option<RowKey>,
    /// Follows the focus (`selectionNavigation`) except on a right click, which moves only the focus.
    pub selected: Vec<RowKey>,
    /// Where a Shift range starts (listWidget.ts `anchor`).
    pub anchor: Option<RowKey>,
    /// The row under the mouse: it shows its actions in place of its count badge.
    pub hovered: Option<RowKey>,
    pub state: SearchUiState,
    /// The search input's error: an invalid regex, a search path that does not exist.
    pub validation: Option<String>,
    /// The message line; `None` hides it.
    pub message: Option<Vec<MessagePart>>,
    /// The completed search's warnings, below the message line.
    pub warnings: Vec<String>,
    pub(super) cancellation: Option<Arc<Cancellation>>,
    pub(super) run: Option<Task<()>>,
    /// `triggerQueryDelayer`: a search typed into the inputs waits for `search.searchOnTypeDebouncePeriod`.
    pub(super) delayed: Option<Task<()>>,
    /// `addToSearchHistoryDelayer`: a search joins the histories of the search and pattern inputs after 2 seconds.
    pub(super) history_update: Option<Task<()>>,
    /// `_replaceHistoryDelayer`: the replace value joins its history 500 ms after it changes.
    pub(super) replace_history_update: Option<Task<()>>,
}

impl SearchView {
    pub fn new(cx: &mut Context<Workbench>) -> Self {
        let input = || SearchInput { value: InputBox::default(), history: HistoryNavigator::new(Vec::new(), HISTORY_LIMIT), view: InputView::default(), focus: cx.focus_handle() };
        let inputs = [input(), input(), input(), input(), input()];
        SearchView {
            inputs,
            focus: cx.focus_handle(),
            toggle_replace_focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            replace_active: false,
            details_visible: false,
            match_case: false,
            whole_word: false,
            regex: false,
            preserve_case: false,
            only_open_editors: false,
            use_excludes_and_ignore_files: true,
            show_context: false,
            tree_view: false,
            result: SearchResult::default(),
            pattern: None,
            collapsed: HashMap::new(),
            focused: None,
            selected: Vec::new(),
            anchor: None,
            hovered: None,
            state: SearchUiState::Idle,
            validation: None,
            message: None,
            warnings: Vec::new(),
            cancellation: None,
            run: None,
            delayed: None,
            history_update: None,
            replace_history_update: None,
        }
    }

    pub fn focus_and_select(&mut self, key: Option<RowKey>) {
        self.selected = key.iter().cloned().collect();
        self.anchor = key.clone();
        self.focused = key;
    }

    pub fn input(&self, field: SearchField) -> &SearchInput {
        &self.inputs[field as usize]
    }

    pub fn input_mut(&mut self, field: SearchField) -> &mut SearchInput {
        &mut self.inputs[field as usize]
    }

    pub fn value(&self, field: SearchField) -> &str {
        &self.input(field).value.value
    }

    /// The search the inputs and toggles ask for, as `searchEditorInput`'s configuration holds it.
    pub fn configuration(&self) -> SearchConfiguration {
        SearchConfiguration {
            query: self.value(SearchField::Query).to_string(),
            files_to_include: self.value(SearchField::Include).to_string(),
            files_to_exclude: self.value(SearchField::Exclude).to_string(),
            is_regexp: self.regex,
            is_case_sensitive: self.match_case,
            use_exclude_settings_and_ignore_files: self.use_excludes_and_ignore_files,
            match_whole_word: self.whole_word,
            // `getContextLines`: `+value` of the input while the toggle is checked.
            context_lines: if self.show_context { self.value(SearchField::ContextLines).trim().parse().unwrap_or(0) } else { 0 },
            show_includes_excludes: self.details_visible,
            only_open_editors: self.only_open_editors,
        }
    }

    /// The replace pattern the previews and the replace actions use (`SearchModel.replacePattern`).
    pub fn replace_pattern(&self) -> Option<ReplacePattern> {
        let pattern = self.pattern.as_ref()?;
        let info = PatternInfo { pattern: &pattern.pattern, is_reg_exp: pattern.is_reg_exp, is_case_sensitive: pattern.is_case_sensitive, is_word_match: pattern.is_word_match };
        Some(ReplacePattern::new(self.value(SearchField::Replace), &info))
    }

    /// The rows of the results tree as it is expanded now, in tree order; `None` for every row expanded. `folders` are the workspace folders: in a
    /// multi-root workspace each heads its files; files outside them come last.
    pub fn rows(&self, folders: &[PathBuf], collapse_results: Option<&str>) -> Vec<ResultRow> {
        let mut rows = Vec::new();
        let single_file = self.result.file_count() == 1;
        for (folder_index, folder) in folders.iter().enumerate().chain([(folders.len(), &PathBuf::new())]) {
            let files: Vec<usize> = (0..self.result.files.len()).filter(|&file| self.result.files[file].folder == folder_index).collect();
            if files.is_empty() {
                continue;
            }
            let outside = folder_index == folders.len();
            let mut depth = 1;
            if folders.len() > 1 && !outside {
                let label = folder.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                let key = RowKey::Folder(folder.clone());
                let collapsed = self.is_collapsed(&key, self.count(&files), single_file, collapse_results);
                rows.push(ResultRow { key, depth: 1, kind: RowKind::Folder { label, files: files.clone(), collapsed } });
                if collapsed {
                    continue;
                }
                depth = 2;
            }
            match self.tree_view && !outside {
                true => self.folder_rows(folder, &files, depth, single_file, collapse_results, &mut rows),
                false => {
                    for file in files {
                        self.file_rows(file, depth, single_file, collapse_results, &mut rows);
                    }
                }
            }
        }
        rows
    }

    /// The tree view below `folder`: its subfolders first, a chain of folders with nothing else in them compressed into
    /// one `a/b` row (`isIncompressible`), then its files.
    fn folder_rows(&self, folder: &Path, files: &[usize], depth: usize, single_file: bool, collapse_results: Option<&str>, rows: &mut Vec<ResultRow>) {
        let mut children: Vec<(PathBuf, Vec<usize>)> = Vec::new();
        let mut own_files = Vec::new();
        for &file in files {
            let Some(child) = child_folder(folder, &self.result.files[file].path) else {
                own_files.push(file);
                continue;
            };
            match children.iter_mut().find(|(known, _)| *known == child) {
                Some((_, child_files)) => child_files.push(file),
                None => children.push((child, vec![file])),
            }
        }
        children.sort_by(|(one, _), (other, _)| compare_paths(&one.to_string_lossy(), &other.to_string_lossy()));
        for (child, child_files) in children {
            let child = self.compressed(child, &child_files);
            let label = child.strip_prefix(folder).unwrap_or(&child).to_string_lossy().into_owned();
            let key = RowKey::Folder(child.clone());
            let collapsed = self.is_collapsed(&key, self.count(&child_files), single_file, collapse_results);
            rows.push(ResultRow { key, depth, kind: RowKind::Folder { label, files: child_files.clone(), collapsed } });
            if !collapsed {
                self.folder_rows(&child, &child_files, depth + 1, single_file, collapse_results, rows);
            }
        }
        for file in own_files {
            self.file_rows(file, depth, single_file, collapse_results, rows);
        }
    }

    /// The deepest folder below which all of `files` are, with no file of their own on the way down.
    fn compressed(&self, mut folder: PathBuf, files: &[usize]) -> PathBuf {
        loop {
            let mut next: Option<PathBuf> = None;
            for &file in files {
                match (child_folder(&folder, &self.result.files[file].path), &next) {
                    (None, _) => return folder,
                    (Some(child), Some(known)) if child != *known => return folder,
                    (Some(child), _) => next = Some(child),
                }
            }
            folder = next.expect("files is not empty");
        }
    }

    fn file_rows(&self, file: usize, depth: usize, single_file: bool, collapse_results: Option<&str>, rows: &mut Vec<ResultRow>) {
        let file_match = &self.result.files[file];
        let key = RowKey::File(file_match.path.clone());
        let collapsed = self.is_collapsed(&key, file_match.matches.len(), single_file, collapse_results);
        rows.push(ResultRow { key, depth, kind: RowKind::File { file, collapsed } });
        if collapsed {
            return;
        }
        for (index, found) in file_match.matches.iter().enumerate() {
            let range = &found.range;
            let key = RowKey::Match(file_match.path.clone(), [range.start_line, range.start_column, range.end_line, range.end_column]);
            rows.push(ResultRow { key, depth: depth + 1, kind: RowKind::Match { file, index } });
        }
    }

    fn count(&self, files: &[usize]) -> usize {
        files.iter().map(|&file| self.result.files[file].matches.len()).sum()
    }

    /// `shouldCollapseAccordingToConfig` unless the row was toggled; a single file opens with fewer than 50 matches
    /// (`expandIfSingularResult`). Without `search.collapseResults` every row is expanded.
    fn is_collapsed(&self, key: &RowKey, count: usize, single_file: bool, collapse_results: Option<&str>) -> bool {
        let Some(collapse_results) = collapse_results else { return false };
        if let Some(collapsed) = self.collapsed.get(key) {
            return *collapsed;
        }
        if single_file && collapse_results != "alwaysCollapse" {
            return matches!(key, RowKey::File(_)) && count >= 50;
        }
        collapse_results == "alwaysCollapse" || (count > 10 && collapse_results != "alwaysExpand")
    }
}

/// The folder below `folder` that `path` is in, `None` for a file of `folder` itself.
fn child_folder(folder: &Path, path: &Path) -> Option<PathBuf> {
    let mut components = path.strip_prefix(folder).ok()?.components();
    let first = components.next()?;
    components.next()?;
    Some(folder.join(first))
}

impl Workbench {
    /// `SearchContext.ViewHasSomeCollapsibleKey`: some folder or file row of the results is expanded.
    pub(super) fn search_has_expanded_rows(&self) -> bool {
        let rows = self.search.rows(&self.search_folders(), Some(&self.collapse_results()));
        rows.iter().any(|row| matches!(row.kind, RowKind::Folder { collapsed: false, .. } | RowKind::File { collapsed: false, .. }))
    }

    /// The Search view's title actions (searchActionsTopBar.ts), in their menu order.
    pub(super) fn search_title_actions(&self, cx: &mut Context<Self>) -> Div {
        let view = &self.search;
        let has_pattern = !view.value(SearchField::Query).is_empty();
        let has_results = !view.result.is_empty();
        let can_collapse = self.search_has_expanded_rows();
        let refresh = match view.state {
            SearchUiState::SlowSearch => ("search-stop", "search.action.cancel", true),
            _ => ("refresh", "search.action.refreshSearchResults", has_pattern),
        };
        // ClearSearchResultsAction: enabled with results, a pattern or a message (`hasSearchResults || ...`).
        let can_clear = has_results || has_pattern || view.message.is_some() || !view.value(SearchField::Replace).is_empty();
        let view_mode = match view.tree_view {
            // searchIcons.ts: View as List shows `searchShowAsTree` (list-tree), View as Tree `searchShowAsList` (list-flat).
            true => ("list-tree", "search.action.viewAsList"),
            false => ("list-flat", "search.action.viewAsTree"),
        };
        let collapse = match can_collapse || !has_results {
            true => ("collapse-all", "search.action.collapseSearchResults", has_results),
            false => ("expand-all", "search.action.expandSearchResults", true),
        };
        // `searchNewEditorIcon` is the new-file codicon.
        let new_editor = ("new-file", "search.action.openNewEditorFromView", true);
        let actions = [refresh, ("clear-all", "search.action.clearSearchResults", can_clear), new_editor, (view_mode.0, view_mode.1, has_results), collapse];
        let mut bar = div().flex().gap(px(4.));
        for (icon, command, enabled) in actions {
            bar = bar.child(self.search_action(icon, command, enabled, cx));
        }
        bar
    }

    /// An action of an action bar: 22px, hover background, disabled in disabledForeground.
    pub(super) fn search_action(&self, icon: &str, command: &'static str, enabled: bool, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let foreground = if enabled { self.foreground_or("icon.foreground") } else { self.color("disabledForeground") };
        let hover = self.color("toolbar.hoverBackground");
        self.icon(icon, 22., 16., foreground).id(command).rounded(px(4.)).when(enabled, |action| {
            action.cursor_pointer().hover(|action| action.bg(hover)).on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                workbench.execute_command(command, Vec::new(), window, cx);
            }))
        })
    }

    /// The Search view's content below the title: the widgets, the message line and the results tree.
    pub(super) fn search_view(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let view = &self.search;
        let foreground = self.foreground_or("sideBar.foreground");
        // searchview.css: .toggle-replace-button spans both inputs, 16px wide at the container's left.
        let toggle_replace = div()
            .id("search-toggle-replace")
            .track_focus(&view.toggle_replace_focus)
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .w(px(16.))
            .rounded(px(4.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
            .child(self.icon(if view.replace_active { "chevron-down" } else { "chevron-right" }, 16., 16., foreground))
            .on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| {
                window.focus(&workbench.search.toggle_replace_focus.clone(), cx);
                workbench.toggle_replace(None, window, cx);
            }));
        let query_toggles = div()
            .flex()
            .gap(px(2.))
            .child(self.search_toggle("case-sensitive", view.match_case, true, cx.listener(|workbench, _: &ClickEvent, window, cx| workbench.toggle_search_option(SearchOption::MatchCase, window, cx))))
            .child(self.search_toggle("whole-word", view.whole_word, true, cx.listener(|workbench, _: &ClickEvent, window, cx| workbench.toggle_search_option(SearchOption::WholeWord, window, cx))))
            .child(self.search_toggle("regex", view.regex, true, cx.listener(|workbench, _: &ClickEvent, window, cx| workbench.toggle_search_option(SearchOption::Regex, window, cx))));
        let query = self.search_input_box(InputId::Search(SearchField::Query), "Search", 26., query_toggles, window, cx);
        let replace = view.replace_active.then(|| {
            let preserve_case = self.search_toggle("preserve-case", view.preserve_case, true, cx.listener(|workbench, _: &ClickEvent, window, cx| workbench.toggle_search_option(SearchOption::PreserveCase, window, cx)));
            let replace_all = self.search_action("replace-all", "search.action.replaceAll", !view.result.is_empty(), cx);
            div()
                .mt(px(6.))
                .flex()
                .child(div().w(px(198.)).child(self.search_input_box(InputId::Search(SearchField::Replace), "Replace", 26., div().child(preserve_case), window, cx)))
                .child(div().ml(px(4.)).pt(px(1.5)).child(replace_all))
        });
        let widget = div().relative().pl(px(18.)).child(toggle_replace).child(query).children(replace);
        let more = div()
            .id("search-details")
            .absolute()
            .top_0()
            .right(px(-2.))
            .w(px(25.))
            .h(px(16.))
            .cursor_pointer()
            .flex()
            .justify_center()
            .child(self.icon("ellipsis", 16., 16., foreground))
            .on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| workbench.toggle_query_details(None, true, window, cx)));
        let details = view.details_visible.then(|| {
            // The changed files toggle needs source control, which the port does not have: disabled, as upstream draws
            // it without a repository.
            let changed_files = self.search_toggle("edit-code", false, false, |_: &ClickEvent, _: &mut Window, _: &mut gpui::App| {});
            let open_editors = self.search_toggle("book", view.only_open_editors, true, cx.listener(|workbench, _: &ClickEvent, window, cx| workbench.toggle_search_option(SearchOption::OnlyOpenEditors, window, cx)));
            let excludes = self.search_toggle("exclude", view.use_excludes_and_ignore_files, true, cx.listener(|workbench, _: &ClickEvent, window, cx| workbench.toggle_search_option(SearchOption::UseExcludes, window, cx)));
            let heading = |title: &'static str| div().pt(px(4.)).h(px(22.2)).text_size(px(11.)).line_height(px(18.2)).child(title);
            div()
                .pb(px(4.))
                .flex()
                .flex_col()
                .child(heading("files to include"))
                .child(self.search_input_box(InputId::Search(SearchField::Include), "e.g. *.ts, src/**/include", 25., div().flex().gap(px(2.)).child(changed_files).child(open_editors), window, cx))
                .child(heading("files to exclude"))
                .child(self.search_input_box(InputId::Search(SearchField::Exclude), "e.g. *.ts, src/**/exclude", 25., div().child(excludes), window, cx))
        });
        let query_details = div().relative().ml(px(18.)).min_h(px(13.)).child(more).children(details);
        // searchview.css .search-widgets-container: margin 0 12px 0 2px inside the pane's 4px wider body.
        let widgets = div().ml(px(2.)).mr(px(8.)).py(px(6.)).flex().flex_col().child(widget).child(query_details);
        div()
            .flex_1()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(widgets)
            .children(self.search_message(cx))
            .children(view.warnings.iter().map(|warning| div().px(px(22.)).pb(px(8.)).text_color(self.color("search.resultsInfoForeground")).child(warning.clone())))
            .child(self.search_results(window, cx))
    }

    /// A pattern input (findInput.ts): the value with its toggles at the right, the focus border when focused, the
    /// validation message under it.
    pub(super) fn search_input_box(&self, id: InputId, placeholder: &str, height: f32, controls: Div, window: &Window, cx: &mut Context<Self>) -> Div {
        // searchEditor.ts styles its inputs with `searchEditor.textInputBorder`.
        let (view, field, border_key) = match id {
            InputId::Search(field) => (&self.search, field, "input.border"),
            InputId::SearchEditor(field) => (&self.search_editor_widget.view, field, "searchEditor.textInputBorder"),
            _ => unreachable!("{id:?} is not a search input"),
        };
        let focused = view.input(field).focus.is_focused(window);
        let invalid = field == SearchField::Query && view.validation.is_some();
        let border = match (invalid, focused) {
            (true, _) => self.color("inputValidation.errorBorder"),
            (false, true) => self.color("focusBorder"),
            (false, false) => self.theme.color(border_key).unwrap_or(self.color("input.background")),
        };
        let message = view.validation.clone().filter(|_| invalid && focused).map(|message| {
            div()
                .absolute()
                .top(px(height))
                .left_0()
                .right_0()
                .p(px(4.))
                .text_size(px(12.))
                .line_height(px(17.))
                .bg(self.color("inputValidation.errorBackground"))
                .text_color(self.foreground_or("inputValidation.errorForeground"))
                .border_1()
                .border_color(self.color("inputValidation.errorBorder"))
                .child(message)
        });
        // The pattern inputs show their placeholder only while focused (`showPlaceholderOnFocus`); HistoryInputBox
        // `addSuffix` names the Up and Down keys on a focused input with a history.
        let pattern = matches!(field, SearchField::Include | SearchField::Exclude);
        let placeholder = match focused && !view.input(field).history.elements().is_empty() {
            true => format!("{placeholder} (\u{21C5} for history)"),
            false => placeholder.to_string(),
        };
        let placeholder = Some(placeholder).filter(|_| focused || !pattern);
        div()
            .relative()
            .h(px(height))
            .flex()
            .items_center()
            .pl(px(6.))
            .pr(px(2.))
            .border_1()
            .rounded(px(4.))
            .bg(self.color("input.background"))
            .border_color(border)
            .text_color(self.foreground_or("input.foreground"))
            .child(div().flex_1().h_full().flex().child(self.input_text(id, InputLook { password: false, placeholder, focused }, cx)))
            .child(controls)
            .children(message)
    }

    /// `Toggle` (toggle.ts): 20px with a 3px radius, the checked one in inputOption colors.
    pub(super) fn search_toggle(&self, icon: &str, checked: bool, enabled: bool, on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static) -> impl IntoElement {
        let foreground = match (checked, enabled) {
            (_, false) => self.color("disabledForeground"),
            (true, true) => self.foreground_or("inputOption.activeForeground"),
            (false, true) => self.foreground_or("input.foreground"),
        };
        let hover = self.color("inputOption.hoverBackground");
        self.icon(icon, 20., 16., foreground)
            .id(gpui::SharedString::from(format!("search-toggle-{icon}")))
            .rounded(px(3.))
            .border_1()
            .border_color(gpui::transparent_black())
            .when(checked, |toggle| toggle.bg(self.color("inputOption.activeBackground")).border_color(self.color("inputOption.activeBorder")))
            .when(enabled, |toggle| toggle.cursor_pointer().when(!checked, |toggle| toggle.hover(|toggle| toggle.bg(hover))).on_click(on_click))
    }

    /// The message line (`.messages`): text with links in textLink colors.
    fn search_message(&self, cx: &mut Context<Self>) -> Option<Div> {
        let parts = self.search.message.as_ref()?;
        let link_color = self.color("textLink.foreground");
        let link_hover = self.color("textLink.activeForeground");
        let children = parts.iter().enumerate().map(|(index, part)| match part {
            MessagePart::Text(text) => div().whitespace_nowrap().child(text.clone()).into_any_element(),
            MessagePart::Link(link) => {
                let link = *link;
                div()
                    .id(("search-message-link", index))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .text_color(link_color)
                    .hover(|style| style.text_color(link_hover))
                    .child(link.label())
                    .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| workbench.search_message_link(link, window, cx)))
                    .into_any_element()
            }
        });
        // searchview.css: .messages pulls 5px up into the widgets' padding; .message has 22px at each side and 8px below.
        Some(div().mt(px(-5.)).px(px(22.)).pb(px(8.)).flex_none().flex().flex_wrap().line_height(px(18.2)).text_color(self.color("search.resultsInfoForeground")).children(children))
    }

    /// The results tree (searchResultsView.ts): 22px rows; a hovered or focused row shows its actions in place of its
    /// count badge.
    fn search_results(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = Rc::new(self.search.rows(&self.search_folders(), Some(&self.collapse_results())));
        let tree_focused = self.search.focus.is_focused(window);
        let count = rows.len();
        let depths: Vec<usize> = rows.iter().map(|row| row.depth).collect();
        let is_active = |row: &ResultRow| self.search.focused.as_ref() == Some(&row.key) || self.search.selected.contains(&row.key);
        let active: Vec<usize> = rows.iter().enumerate().filter(|(_, row)| is_active(row)).map(|(index, _)| index).collect();
        let guides = Rc::new(self.tree_guides(ListId::Search, depths.clone(), &active, window));
        let sticky = self.tree_sticky_rows(ListId::Search, &depths);
        let sticky_rows = rows.clone();
        let horizontal = self.horizontal_scrolling(ListId::Search);
        let widest = self.widest_search_row(&rows);
        let list = uniform_list(
            "search-results-list",
            count,
            cx.processor(move |workbench, range: std::ops::Range<usize>, _window, cx| {
                // The row's content box starts 4px into the element search_row returns.
                range.map(|index| div().relative().w_full().when(horizontal, |row| row.whitespace_nowrap()).child(workbench.search_row(&rows[index], index, tree_focused, false, cx)).children(guides.row(index, 4.)).into_any_element()).collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.search.scroll)
        .when(horizontal, |list| list.with_horizontal_sizing_behavior(gpui::ListHorizontalSizingBehavior::Unconstrained).with_width_from_item(Some(widest)))
        .size_full();
        let sticky_elements = sticky.iter().map(|row| self.search_row(&sticky_rows[row.index], row.index, tree_focused, true, cx)).collect();
        let sticky_container = self.tree_sticky_container(ListId::Search, &sticky, sticky_elements, cx);
        div().id("search-results").relative().track_focus(&self.search.focus).flex_1().min_h_0().child(list).child(self.list_scroll_layer(ListId::Search, cx)).children(sticky_container)
    }

    /// A sticky row is never drawn focused or selected (`StickyScrollWidget`).
    fn search_row(&self, row: &ResultRow, index: usize, tree_focused: bool, sticky: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let foreground = self.foreground_or("sideBar.foreground");
        let is_focused = !sticky && self.search.focused.as_ref() == Some(&row.key);
        let is_selected = !sticky && self.search.selected.contains(&row.key);
        let (selection, selection_foreground) = match tree_focused {
            true => (self.color("list.activeSelectionBackground"), self.theme.color("list.activeSelectionForeground")),
            false => (self.color("list.inactiveSelectionBackground"), self.theme.color("list.inactiveSelectionForeground")),
        };
        let focus_outline = self.theme.color("list.focusOutline").or(self.theme.color("focusBorder")).filter(|_| tree_focused && is_focused);
        let indent = self.tree_indent() * (row.depth - 1) as f32;
        let (content, collapsed) = match &row.kind {
            RowKind::Folder { label, files, collapsed } => (self.search_folder_row(&row.key, label, files, *collapsed, is_focused, is_selected, foreground, cx), Some(*collapsed)),
            RowKind::File { file, collapsed } => (self.search_file_row(&row.key, *file, is_focused, is_selected, foreground, cx), Some(*collapsed)),
            RowKind::Match { file, index } => (self.search_match_row(&row.key, *file, *index, is_focused, is_selected, cx), None),
        };
        let twistie = match collapsed {
            Some(true) => self.icon("chevron-right", 16., 16., foreground),
            Some(false) => self.icon("chevron-down", 16., 16., foreground),
            None => div().size(px(16.)).flex_none(),
        };
        let key = row.key.clone();
        let row = div()
            .id(("search-result", index))
            // list.css: .monaco-list.mouse-support .monaco-list-row is cursor: pointer.
            .cursor_pointer()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .pl(px(11. + indent))
            .flex()
            .items_center()
            .rounded(px(4.))
            .when(!is_focused && !is_selected, |row| row.hover(|row| row.bg(self.color("list.hoverBackground"))))
            .when(is_focused && !is_selected && tree_focused, |row| row.when_some(self.theme.color("list.focusBackground"), |row, color| row.bg(color)))
            .when(is_selected, |row| row.bg(selection).when_some(selection_foreground, |row, color| row.text_color(color)))
            .when_some(focus_outline, |row, color| row.child(div().absolute().inset_0().rounded(px(4.)).border_1().border_color(color)))
            .child(twistie)
            .child(content.ml(px(3.)))
            .on_hover({
                let key = key.clone();
                cx.listener(move |workbench, hovered: &bool, _, cx| {
                    match hovered {
                        true => workbench.search.hovered = Some(key.clone()),
                        false if workbench.search.hovered.as_ref() == Some(&key) => workbench.search.hovered = None,
                        false => return,
                    }
                    cx.notify();
                })
            })
            .on_click({
                let key = key.clone();
                cx.listener(move |workbench, event: &ClickEvent, window, cx| workbench.search_row_clicked(&key, event.click_count(), event.modifiers(), window, cx))
            })
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| workbench.show_search_context_menu(&key, event.position, window, cx)));
        // A uniform list sizes its items to their content; the wrapper takes the list's width.
        div().w_full().h(px(22.)).px(px(4.)).flex().child(row).into_any_element()
    }

    /// `FolderMatchRenderer`: the folder's icon, its name and its count badge.
    #[allow(clippy::too_many_arguments)]
    fn search_folder_row(&self, key: &RowKey, label: &str, files: &[usize], collapsed: bool, focused: bool, selected: bool, foreground: Rgba, cx: &mut Context<Self>) -> Div {
        let count = files.iter().map(|&file| self.search.result.files[file].matches.len()).sum();
        let label_line = if self.theme.kind.is_high_contrast() { 20. } else { 22. };
        let RowKey::Folder(path) = key else { unreachable!("a folder row has a folder key") };
        let name = path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default();
        let is_root = self.folder.as_ref().is_some_and(|tree| tree.is_root(path));
        let folder = FolderIcon { name: &name, is_root, expanded: !collapsed };
        let icon = self.file_icons.as_ref().and_then(|icons| icons.folder_icon(folder, self.theme.kind, 13., label_line, foreground));
        let mut actions = Vec::new();
        if self.search.replace_active {
            actions.push(("replace-all", RowAction::Replace));
        }
        actions.push(("close", RowAction::Dismiss));
        let label = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .children(icon.map(|icon| icon.mr(px(6.))))
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label.to_string()));
        self.search_label_row(key, label, Some(count), &actions, focused, selected, cx)
    }

    /// `FileMatchRenderer`: the file icon, its name, the folder it is in as the description in the list view, the
    /// badge.
    fn search_file_row(&self, key: &RowKey, file: usize, focused: bool, selected: bool, foreground: Rgba, cx: &mut Context<Self>) -> Div {
        let file_match = &self.search.result.files[file];
        let label_line = if self.theme.kind.is_high_contrast() { 20. } else { 22. };
        let icon = self.file_icons.as_ref().and_then(|icons| icons.file_icon(&file_match.path, self.languages.language_of(&file_match.path, ""), self.theme.kind, 13., label_line, foreground));
        let description = match self.search.tree_view {
            true => None,
            false => self.search_file_description(file_match),
        };
        // iconlabel.css: .label-description is 0.9em at 0.7 opacity, 0.5em after the name.
        let description = description.map(|description| div().ml(px(6.5)).text_size(px(11.7)).opacity(0.7).child(description));
        let label = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .children(icon.map(|icon| icon.mr(px(6.))))
            .child(div().flex_none().child(file_match.name()))
            .children(description.map(|description| description.min_w_0().overflow_hidden().text_ellipsis()));
        let mut actions = Vec::new();
        if self.search.replace_active {
            actions.push(("replace-all", RowAction::Replace));
        }
        actions.push(("close", RowAction::Dismiss));
        self.search_label_row(key, label, Some(file_match.matches.len()), &actions, focused, selected, cx)
    }

    /// `MatchRenderer`: the line around the match, the match highlighted, or struck through beside its replacement.
    fn search_match_row(&self, key: &RowKey, file: usize, index: usize, focused: bool, selected: bool, cx: &mut Context<Self>) -> Div {
        let found = &self.search.result.files[file].matches[index];
        let preview = found.preview();
        let high_contrast = self.theme.kind.is_high_contrast();
        let replace = match self.search.replace_active && !self.search.value(SearchField::Replace).is_empty() {
            true => self.search.replace_pattern().map(|pattern| found.replace_string(&pattern, self.search.preserve_case)),
            false => None,
        };
        let highlight = |text: String, background: &str, border: Option<&str>| {
            // searchview.css: an inline span, 15px tall in the 22px row; an unset border color drops the border.
            div()
                .flex_none()
                .line_height(px(15.))
                .when_some(border.and_then(|border| self.theme.color(border)), |span, color| span.border_1().border_color(color).when(high_contrast, |span| span.border_dashed()))
                .when(!high_contrast, |span| span.bg(self.color(background)))
                .child(text)
        };
        let matched = match &replace {
            Some(replace) => div()
                .flex()
                .flex_none()
                .child(highlight(preview.inside.clone(), "diffEditor.removedTextBackground", Some("diffEditor.removedTextBackground")).line_through())
                .when(!replace.is_empty(), |span| span.child(highlight(replace.clone(), "diffEditor.insertedTextBackground", Some("diffEditor.insertedLineBackground")))),
            None => highlight(preview.inside.clone(), "editor.findMatchHighlightBackground", Some("editor.findMatchHighlightBorder")),
        };
        let line_number = (self.setting("search.showLineNumbers") == true).then(|| {
            div().ml(px(7.)).mr(px(4.)).opacity(0.7).text_size(px(11.7)).flex_none().child(format!("{}:", found.range.start_line + 1))
        });
        let text = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .children(line_number)
            .child(div().flex_none().child(preview.before))
            .child(matched)
            .child(div().min_w_0().overflow_hidden().text_ellipsis().child(keep_before_ellipsis(&preview.after)));
        let mut actions = Vec::new();
        if self.search.replace_active {
            actions.push(("replace", RowAction::Replace));
        }
        actions.push(("close", RowAction::Dismiss));
        self.search_label_row(key, text, None, &actions, focused, selected, cx)
    }

    /// A row's label with the count badge, hidden while the row is hovered or focused, and the inline actions shown then
    /// (searchview.css `.monaco-action-bar`, `.monaco-count-badge`).
    fn search_label_row(&self, key: &RowKey, label: Div, count: Option<usize>, actions: &[(&'static str, RowAction)], focused: bool, selected: bool, cx: &mut Context<Self>) -> Div {
        let foreground = self.foreground_or("icon.foreground");
        let hover = self.color("toolbar.hoverBackground");
        let actions = actions.iter().map(|&(icon, action)| {
            let key = key.clone();
            self.icon(icon, 20., 16., foreground)
                .id(gpui::SharedString::from(format!("search-row-{icon}")))
                .mr(px(2.6))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|action| action.bg(hover))
                .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    workbench.search_row_action(&key, action, window, cx);
                }))
        });
        // Hover is tracked in the view, not with group_hover: a display toggled by hover between prepaint and paint panics.
        let show_actions = focused || selected || self.search.hovered.as_ref() == Some(key);
        let action_bar = div().pl(px(5.2)).pr(px(10.4)).flex_none().items_center().when(show_actions, |bar| bar.flex()).when(!show_actions, |bar| bar.hidden()).children(actions);
        let badge = count.filter(|_| !show_actions).map(|count| {
            div()
                // An inline-block on the 22px line: its top sits 3px below the row's.
                .self_start()
                .mt(px(3.))
                .mr(px(12.))
                .min_w(px(18.))
                .h(px(18.))
                .px(px(5.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(11.))
                .text_size(px(11.))
                .bg(self.color("badge.background"))
                .text_color(self.foreground_or("badge.foreground"))
                .when_some(self.theme.color("contrastBorder"), |badge, border| badge.border_1().border_color(border))
                .child(count.to_string())
        });
        div().flex_1().min_w_0().h_full().flex().items_center().child(label).children(badge).child(action_bar)
    }

    fn search_file_description(&self, file_match: &crate::search_model::FileMatch) -> Option<String> {
        let parent = file_match.path.parent()?;
        let folders = self.search_folders();
        let folder = folders.get(file_match.folder);
        match folder {
            Some(folder) => {
                let relative = parent.strip_prefix(folder).ok()?.to_string_lossy().into_owned();
                match folders.len() > 1 {
                    true => Some(format!("{}{}", folder.file_name()?.to_string_lossy(), if relative.is_empty() { String::new() } else { format!(" \u{2022} {relative}") })),
                    false => Some(relative).filter(|relative| !relative.is_empty()),
                }
            }
            None => Some(self.tildify(parent)),
        }
    }

    /// The home folder as `~` (labelService `tildify`).
    pub(super) fn tildify(&self, path: &Path) -> String {
        let text = path.to_string_lossy().into_owned();
        match std::env::var("HOME") {
            Ok(home) if !home.is_empty() && text.starts_with(&home) => format!("~{}", &text[home.len()..]),
            _ => text,
        }
    }

    pub(super) fn collapse_results(&self) -> String {
        self.setting("search.collapseResults").as_str().unwrap_or("alwaysExpand").to_string()
    }
}

/// An inline action of a result row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowAction {
    Replace,
    Dismiss,
}

/// The search view's toggles.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SearchOption {
    MatchCase,
    WholeWord,
    Regex,
    PreserveCase,
    OnlyOpenEditors,
    UseExcludes,
}

/// gpui drops whitespace and ASCII punctuation before a truncation "…", CSS `text-overflow` keeps them
/// ("the …"); a zero-width space after each one ends that trim.
pub(super) fn keep_before_ellipsis(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    for c in text.chars() {
        kept.push(c);
        if c.is_whitespace() || c.is_ascii_punctuation() {
            kept.push('\u{200B}');
        }
    }
    kept
}
