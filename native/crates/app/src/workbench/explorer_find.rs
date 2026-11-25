//! The Explorer's find widget: abstractTree.ts `FindWidget` and `FindController`, asyncDataTree.ts
//! `AsyncFindController`, and explorerViewer.ts `ExplorerFindProvider`, which searches the folders with ripgrep
//! and highlights the matches or shows only them (tree.css `.monaco-tree-type-filter`).

use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use config::configuration::Overrides;
use gpui::{AnyElement, BoxShadow, Context, Div, FocusHandle, HighlightStyle, Keystroke, MouseButton, Stateful, StyledText, Task, Window, deferred, div, point, prelude::*, px};
use search::query::Query;
use search::ripgrep::Cancellation;
use serde_json::{Value, json};

use super::Workbench;
use super::find_widget::HISTORY_LIMIT;
use super::input_box_view::{InputId, InputLook, InputView};
use crate::commands::Outcome;
use crate::filters::{FuzzyScoreOptions, create_matches, fuzzy_score};
use crate::history::HistoryNavigator;
use crate::input_box::InputBox;

/// asyncDataTree.ts `AsyncFindController.taskQueue`: `new ThrottledDelayer(250)`.
const FIND_DELAY: Duration = Duration::from_millis(250);
/// `searchInWorkspace`: the file search's `maxResults`.
const MAX_FILE_RESULTS: usize = 512;
/// The input's width in the reference: `.monaco-tree-type-filter-input` is the widget's flex 1.
const INPUT_WIDTH: f32 = 155.;

pub struct ExplorerFind {
    pub focus: FocusHandle,
    pub view: InputView,
    pub input: InputBox,
    history: HistoryNavigator,
    open: bool,
    /// `AbstractFindController.previousPattern`: the value a reopened widget starts with.
    previous_pattern: String,
    /// The Filter and Fuzzy Match toggles; unset ones follow `workbench.list.defaultFindMode` and
    /// `workbench.list.defaultFindMatchType`.
    filter_mode: Option<bool>,
    fuzzy: Option<bool>,
    /// `activeSession`: a find ran since the pattern was last empty.
    session: bool,
    /// `activeFindMetadata` with the pattern it was found for.
    result: Option<FindResult>,
    /// The widget's warning message (`renderMessage`).
    message: Option<String>,
    cancellation: Option<Arc<Cancellation>>,
    _find: Option<Task<()>>,
}

struct FindResult {
    pattern: String,
    fuzzy: bool,
    filter_mode: bool,
    /// The matching files and folders.
    matches: Vec<PathBuf>,
}

impl ExplorerFind {
    pub fn new(cx: &mut gpui::App) -> Self {
        ExplorerFind {
            focus: cx.focus_handle(),
            view: InputView::default(),
            input: InputBox::default(),
            history: HistoryNavigator::new(Vec::new(), HISTORY_LIMIT),
            open: false,
            previous_pattern: String::new(),
            filter_mode: None,
            fuzzy: None,
            session: false,
            result: None,
            message: None,
            cancellation: None,
            _find: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }
}

impl Workbench {
    /// listCommands.ts `list.find`, `list.closeFind`, `list.toggleFindMode` and `list.toggleFindMatchType`, for the
    /// Explorer, the only tree with a find widget.
    pub(super) fn register_explorer_find_commands(&mut self) {
        type Run = fn(&mut Workbench, &mut Window, &mut Context<Workbench>);
        let commands: [(&str, Run); 4] = [
            ("list.find", |workbench, window, cx| workbench.open_explorer_find(window, cx)),
            ("list.closeFind", |workbench, window, cx| workbench.close_explorer_find(window, cx)),
            ("list.toggleFindMode", |workbench, window, cx| {
                let filter_mode = !workbench.explorer_find_filter_mode();
                workbench.explorer.find.filter_mode = Some(filter_mode);
                workbench.apply_explorer_find_pattern(window, cx);
            }),
            ("list.toggleFindMatchType", |workbench, window, cx| {
                let fuzzy = !workbench.explorer_find_fuzzy();
                workbench.explorer.find.fuzzy = Some(fuzzy);
                workbench.apply_explorer_find_pattern(window, cx);
            }),
        ];
        for (id, run) in commands {
            self.commands.register(id, move |workbench, _, window, cx| {
                let focused = workbench.explorer.focus.is_focused(window) || workbench.explorer.find.focus.is_focused(window);
                if focused && workbench.folder.is_some() {
                    run(workbench, window, cx);
                }
                Outcome::done()
            });
        }
    }

    fn explorer_find_filter_mode(&self) -> bool {
        self.explorer.find.filter_mode.unwrap_or_else(|| self.setting("workbench.list.defaultFindMode") == json!("filter"))
    }

    fn explorer_find_fuzzy(&self) -> bool {
        self.explorer.find.fuzzy.unwrap_or_else(|| self.setting("workbench.list.defaultFindMatchType") != json!("contiguous"))
    }

    /// `AbstractFindController.open`: the widget shows with the last pattern selected, or an open one takes the focus.
    fn open_explorer_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let find = &mut self.explorer.find;
        window.focus(&find.focus.clone(), cx);
        if !find.open {
            find.open = true;
            find.input.set_value(find.previous_pattern.clone());
            self.apply_explorer_find_pattern(window, cx);
        }
        let find = &mut self.explorer.find;
        find.input.select_all();
        // `FindWidget.select`: "Reposition to last in history".
        find.history.add_to_history(&find.input.value, true);
        self.blink_caret(InputId::ExplorerFind, window, cx);
        cx.notify();
    }

    /// `AbstractFindController.close`: the pattern is kept for the next open, and the tree takes the focus.
    fn close_explorer_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let find = &mut self.explorer.find;
        if !find.open {
            return;
        }
        find.open = false;
        find.previous_pattern = std::mem::take(&mut find.input.value);
        find.input = InputBox::default();
        find.view.reset();
        self.apply_explorer_find_pattern(window, cx);
        window.focus(&self.explorer.focus.clone(), cx);
    }

    pub(super) fn edit_explorer_find_input(&mut self, edit: impl FnOnce(&mut InputBox), window: &mut Window, cx: &mut Context<Self>) {
        let input = &mut self.explorer.find.input;
        let before = input.value.clone();
        edit(input);
        cx.notify();
        if input.value != before {
            self.apply_explorer_find_pattern(window, cx);
        }
    }

    /// `FindWidget`'s keydown: Enter returns to the tree, Up and Down walk the history, Down past its end returns
    /// to the tree.
    pub(super) fn explorer_find_input_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let modifiers = keystroke.modifiers;
        if modifiers.platform || modifiers.alt || modifiers.shift || modifiers.control {
            return false;
        }
        let find = &mut self.explorer.find;
        match keystroke.key.as_str() {
            "enter" | "down" if keystroke.key == "enter" || find.history.is_last() || find.history.is_nowhere() => {
                find.history.add_to_history(&find.input.value, false);
                window.focus(&self.explorer.focus.clone(), cx);
            }
            "down" => {
                let value = find.history.show_next_value(&find.input.value);
                self.edit_explorer_find_input(|input| input.set_value(value), window, cx);
            }
            "up" => {
                if let Some(value) = find.history.show_previous_value(&find.input.value) {
                    self.edit_explorer_find_input(|input| input.set_value(value), window, cx);
                }
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// `AsyncFindController.applyPattern`: the message clears, the running find stops, and after the throttle delay
    /// an empty pattern ends the session while any other one searches the folders.
    fn apply_explorer_find_pattern(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let find = &mut self.explorer.find;
        find.message = None;
        if let Some(cancellation) = find.cancellation.take() {
            cancellation.cancel();
        }
        let pattern = if find.open { find.input.value.clone() } else { String::new() };
        let (filter_mode, fuzzy) = (self.explorer_find_filter_mode(), self.explorer_find_fuzzy());
        let queries = if pattern.is_empty() { Vec::new() } else { self.explorer_find_queries(&pattern, fuzzy) };
        let rg = search::ripgrep::rg_path(&self.app_root);
        let cancellation = Arc::new(Cancellation::default());
        self.explorer.find.cancellation = Some(cancellation.clone());
        self.explorer.find._find = Some(cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(FIND_DELAY).await;
            if pattern.is_empty() {
                let _ = workbench.update_in(cx, |workbench, _, cx| workbench.end_explorer_find_session(cx));
                return;
            }
            let search_cancellation = cancellation.clone();
            let found = cx.background_spawn(async move { search_folders(&queries, &pattern, fuzzy, &rg, &search_cancellation).map(|matches| (pattern, matches)) }).await;
            let Some((pattern, (matches, hit_max_results))) = found.filter(|_| !cancellation.is_cancelled()) else { return };
            let result = FindResult { pattern, fuzzy, filter_mode, matches };
            let _ = workbench.update_in(cx, |workbench, _, cx| workbench.show_explorer_find_result(result, hit_max_results, cx));
        }));
        cx.notify();
    }

    /// `searchInWorkspace`'s two file searches of each folder: the files whose name matches, at most 512, and the
    /// files under a folder whose name matches.
    fn explorer_find_queries(&self, pattern: &str, fuzzy: bool) -> Vec<(PathBuf, Query, Query)> {
        let segment = segment_match_pattern(&pattern.to_lowercase(), fuzzy);
        let folders: Vec<PathBuf> = self.workspace().iter().flat_map(|workspace| &workspace.folders).map(|folder| folder.path.clone()).collect();
        let settings = self.settings.clone();
        let setting = |folder: &Path, key: &str| settings.lock().unwrap().get_value(Some(key), &Overrides { resource: Some(folder), ..Overrides::default() }).unwrap_or(Value::Null);
        folders
            .into_iter()
            .map(|folder| {
                let options = |file_pattern: String, max_results: Option<usize>| {
                    json!({
                        "filePattern": file_pattern,
                        "shouldGlobSearch": true,
                        "ignoreGlobCase": true,
                        "maxResults": max_results,
                        "disregardIgnoreFiles": setting(&folder, "explorer.excludeGitIgnore") != json!(true),
                    })
                };
                let roots = std::slice::from_ref(&folder);
                let files = search::query::file_query(roots, &options(format!("**/{segment}"), Some(MAX_FILE_RESULTS)), &setting);
                let folders = search::query::file_query(roots, &options(format!("**/{segment}/**"), None), &setting);
                (folder.clone(), files, folders)
            })
            .collect()
    }

    /// `AsyncFindController.applyPatternAsync` once the find returns: the filter session shows only the matches,
    /// the highlight session marks them, the focus moves to the first row whose name matches (`FindController
    /// .applyPattern`), and the widget names an empty or capped result.
    fn show_explorer_find_result(&mut self, result: FindResult, hit_max_results: bool, cx: &mut Context<Self>) {
        let scroll_top = -f32::from(self.explorer.scroll.offset().y);
        let Some(tree) = self.folder.as_mut() else { return };
        let mut restored_scroll_top = None;
        if result.filter_mode {
            let mut marked = HashSet::new();
            for path in &result.matches {
                marked.extend(path.ancestors().take_while(|ancestor| tree.workspace.folders.iter().any(|folder| ancestor.starts_with(&folder.path))).map(Path::to_path_buf));
            }
            tree.set_filter(marked, scroll_top);
        } else {
            restored_scroll_top = tree.end_filter();
        }
        // `focusNext(0, true)`: from the focused row, or the first one when the filter hides it.
        let rows = tree.rows();
        let start = tree.focused.as_ref().and_then(|focused| rows.iter().position(|row| &row.path == focused)).unwrap_or(0);
        let first_match = (0..rows.len()).map(|offset| (start + offset) % rows.len()).find(|&index| label_matches(&result.pattern, &rows[index].name, result.fuzzy).is_some());
        if let Some(index) = first_match {
            tree.focused = Some(rows[index].path.clone());
        }
        if let Some(scroll_top) = restored_scroll_top {
            self.explorer.scroll.set_offset(point(px(0.), px(-scroll_top)));
        }
        if let Some(index) = first_match {
            self.explorer.scroll.scroll_to_item(index);
        }
        let find = &mut self.explorer.find;
        find.message = match (result.matches.is_empty(), hit_max_results) {
            (true, _) => Some("No results found.".to_string()),
            (false, true) => Some("The result set only contains a subset of all matches. Be more specific in your search to narrow down the results.".to_string()),
            (false, false) => None,
        };
        find.session = true;
        find.result = Some(result);
        cx.notify();
    }

    /// `deactivateFindSession`: every item shows again without highlights.
    fn end_explorer_find_session(&mut self, cx: &mut Context<Self>) {
        let find = &mut self.explorer.find;
        if !find.session {
            return;
        }
        find.session = false;
        find.result = None;
        if let Some(scroll_top) = self.folder.as_mut().and_then(|tree| tree.end_filter()) {
            self.explorer.scroll.set_offset(point(px(0.), px(-scroll_top)));
        }
        cx.notify();
    }

    /// The find keys of the Explorer's context (listService.ts `treeFindOpen`, files.ts `explorerFindProviderActive`).
    pub(super) fn explorer_find_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        context.insert("listSupportsFind".into(), json!(true));
        context.insert("treeFindOpen".into(), json!(self.explorer.find.open));
        context.insert("explorerFindProviderActive".into(), json!(self.folder.as_ref().is_some_and(|tree| tree.is_filtering())));
    }

    /// A row's name with the find highlights (views.css `.highlight`: `list.filterMatchBackground`), and a collapsed
    /// folder's count of the matches inside it (`FilesRenderer.renderStat`'s CountBadge).
    pub(super) fn explorer_find_label(&self, name: String, path: &Path, collapsed_folder: bool) -> AnyElement {
        let Some(result) = &self.explorer.find.result else { return name.into_any_element() };
        let highlights = label_matches(&result.pattern, &name, result.fuzzy).unwrap_or_default();
        let badge_count = match collapsed_folder && !result.filter_mode {
            true => result.matches.iter().filter(|matched| matched.starts_with(path) && matched.as_path() != path).count(),
            false => 0,
        };
        // shortcut: no outline, GPUI text runs cannot draw one; `list.filterMatchBorder` is set in high contrast only.
        let background = self.color("list.filterMatchBackground");
        let byte = |chars: usize| name.char_indices().nth(chars).map_or(name.len(), |(byte, _)| byte);
        let styles: Vec<(Range<usize>, HighlightStyle)> = highlights.iter().map(|range| (byte(range.start)..byte(range.end), HighlightStyle { background_color: Some(background.into()), ..Default::default() })).collect();
        let text = StyledText::new(name.clone()).with_highlights(styles);
        if badge_count == 0 {
            return text.into_any_element();
        }
        // countBadge.css with explorerViewer.ts's colors: the filter match background and border, the badge text color.
        let badge = div()
            .ml(px(5.))
            .min_w(px(18.))
            .h(px(18.))
            .px(px(5.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(11.))
            .text_size(px(11.))
            .bg(background)
            .text_color(self.color("badge.foreground"))
            .when_some(self.theme.color("list.filterMatchBorder"), |badge, border| badge.border_1().border_color(border))
            .child(badge_count.to_string());
        div().flex().items_center().child(text).child(badge).into_any_element()
    }

    /// tree.css `.monaco-tree-type-filter`: the input with its toggles and the close action, at the tree's top right.
    pub(super) fn explorer_find_widget(&self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let find = &self.explorer.find;
        if !find.open {
            return None;
        }
        let focused = find.focus.is_focused(window);
        let filter_mode = self.explorer_find_filter_mode();
        let border = match (&find.message, focused) {
            (Some(_), _) => self.color("inputValidation.warningBorder"),
            (None, true) => self.color("focusBorder"),
            (None, false) => self.color("input.border"),
        };
        let placeholder = if filter_mode { "Type to filter" } else { "Type to search" };
        // inputBox.ts: the message shows only while the input has the focus.
        let message = find.message.clone().filter(|_| focused).map(|message| {
            // inputBox.css `.monaco-inputbox-message`, in a context view over the rows.
            deferred(
                div()
                    .absolute()
                    // Over the input's bottom border, half a point left of its outer edge as in the reference capture.
                    .top(px(21.))
                    .left(px(-1.5))
                    .w(px(INPUT_WIDTH + 1.))
                    .p(px(4.8))
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .bg(self.color("inputValidation.warningBackground"))
                    .text_color(self.foreground_or("inputValidation.warningForeground"))
                    .border_1()
                    .border_color(self.color("inputValidation.warningBorder"))
                    .child(message),
            )
        });
        let input = div()
            .relative()
            .flex_none()
            .w(px(INPUT_WIDTH))
            .h(px(23.))
            .flex()
            .bg(self.color("input.background"))
            .border_1()
            .border_color(border)
            .rounded(px(4.))
            .child(div().flex_1().h_full().ml(px(4.)).mr(px(44.)).flex().child(self.input_text(InputId::ExplorerFind, InputLook { password: false, placeholder: Some(placeholder.into()), focused }, cx)))
            .child(
                div()
                    .absolute()
                    // tree.css `.monaco-findInput > .controls`: 2px from the input's outer top and right, inside its 1px border.
                    .top(px(1.))
                    .right(px(1.))
                    .flex()
                    .child(self.explorer_find_toggle("list-filter", "list.toggleFindMode", filter_mode, cx))
                    .child(self.explorer_find_toggle("search-fuzzy", "list.toggleFindMatchType", self.explorer_find_fuzzy(), cx)),
            )
            .children(message);
        let close = self
            .icon("close", 20., 16., self.foreground_or("icon.foreground"))
            .id("explorer-find-close")
            .ml(px(4.))
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|close| close.bg(self.color("toolbar.hoverBackground")))
            .on_click(cx.listener(|workbench, _, window, cx| workbench.close_explorer_find(window, cx)));
        // `0 0 8px 2px`: GPUI's blur_radius is the sigma, half the CSS blur.
        let shadow = self.theme.color("listFilterWidget.shadow").map(|color| BoxShadow { color: color.into(), offset: point(px(0.), px(0.)), blur_radius: px(4.), spread_radius: px(2.), inset: false });
        Some(
            div()
                .absolute()
                .top_0()
                .right(px(10.))
                .flex()
                .items_center()
                .p(px(3.))
                .border_1()
                .border_color(self.color("widget.border"))
                .rounded_b(px(4.))
                .bg(self.color("listFilterWidget.background"))
                .shadow(shadow.into_iter().collect())
                .cursor_default()
                .child(input)
                .child(close),
        )
    }

    /// A `TreeFindToggle` (toggle.css `.monaco-custom-toggle`); a click keeps the focus in the input.
    fn explorer_find_toggle(&self, icon: &'static str, command: &'static str, checked: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let color = if checked { self.foreground_or("inputOption.activeForeground") } else { self.foreground_or("editorWidget.foreground") };
        self.icon(icon, 20., 16., color)
            .id(icon)
            .ml(px(2.))
            .rounded(px(3.))
            .border_1()
            .border_color(if checked { self.color("inputOption.activeBorder") } else { gpui::transparent_black().into() })
            .cursor_pointer()
            .when(checked, |toggle| toggle.bg(self.color("inputOption.activeBackground")))
            .when(!checked, |toggle| toggle.hover(|toggle| toggle.bg(self.color("inputOption.hoverBackground"))))
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_click(cx.listener(move |workbench, _, window, cx| {
                window.focus(&workbench.explorer.find.focus.clone(), cx);
                drop(workbench.execute_command(command, Vec::new(), window, cx));
            }))
    }
}

/// `fuzzyMatchingGlobPattern` and `continousMatchingGlobPattern`.
fn segment_match_pattern(pattern_lowercase: &str, fuzzy: bool) -> String {
    match fuzzy {
        true => format!("*{}*", pattern_lowercase.chars().map(String::from).collect::<Vec<_>>().join("*")),
        false => format!("*{pattern_lowercase}*"),
    }
}

/// `FindFilter.filter`'s label score: `fuzzyScore` with a weak first match, or `contiguousFuzzyScore`; the char
/// ranges to highlight.
fn label_matches(pattern: &str, label: &str, fuzzy: bool) -> Option<Vec<Range<usize>>> {
    let (pattern_lowercase, label_lowercase) = (pattern.to_lowercase(), label.to_lowercase());
    if !fuzzy {
        let byte = label_lowercase.find(&pattern_lowercase)?;
        let start = label_lowercase[..byte].chars().count();
        return Some(vec![start..start + pattern_lowercase.chars().count()]);
    }
    let options = FuzzyScoreOptions { first_match_can_be_weak: true, boost_full_match: true };
    let score = fuzzy_score(pattern, &pattern_lowercase, 0, label, &label_lowercase, 0, options)?;
    Some(create_matches(Some(&score)))
}

/// `getSearchResults`: each folder's matching files, and the folders whose name matches, from the files under
/// them (`getMatchingDirectoriesFromFiles`); `None` once cancelled. Also whether a search hit its limit.
fn search_folders(queries: &[(PathBuf, Query, Query)], pattern: &str, fuzzy: bool, rg: &Path, cancellation: &Cancellation) -> Option<(Vec<PathBuf>, bool)> {
    let segment = segment_match_pattern(&pattern.to_lowercase(), fuzzy);
    let glob_options = files::glob::Options { ignore_case: true, ..files::glob::Options::default() };
    let mut matches = Vec::new();
    let mut hit_max_results = false;
    for (root, files_query, folders_query) in queries {
        let mut files = Vec::new();
        let mut under_folders = Vec::new();
        let files_limit = search::file_search::search(files_query, rg, cancellation, &mut |folder, relative| files.push(folder.join(relative)));
        let folders_limit = search::file_search::search(folders_query, rg, cancellation, &mut |_, relative| under_folders.push(relative.to_string()));
        if cancellation.is_cancelled() {
            return None;
        }
        for result in [&files_limit, &folders_limit] {
            match result {
                Ok(limit_hit) => hit_max_results |= limit_hit,
                Err(error) => eprintln!("explorer find: {error}"),
            }
        }
        let mut directories = HashSet::new();
        for relative in &under_folders {
            let segments: Vec<&str> = relative.split('/').collect();
            for depth in 1..segments.len() {
                directories.insert(segments[..depth].join("/"));
            }
        }
        let mut directories: Vec<String> = directories.into_iter().filter(|directory| files::glob::match_pattern(&segment, directory.rsplit('/').next().unwrap_or(directory), glob_options)).collect();
        directories.sort();
        matches.extend(files);
        matches.extend(directories.into_iter().map(|directory| root.join(directory)));
    }
    Some((matches, hit_max_results))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_patterns_match_fuzzy_or_contiguous() {
        assert_eq!(segment_match_pattern("ts", true), "*t*s*");
        assert_eq!(segment_match_pattern("ts", false), "*ts*");
    }

    #[test]
    fn label_matches_highlight_the_found_characters() {
        assert_eq!(label_matches("ts", "main.ts", false), Some(vec![5..7]));
        assert_eq!(label_matches("ts", "notes.txt", false), None);
        assert!(label_matches("ts", "notes.txt", true).is_some());
        // fuzzyScore prefers a word start: the "t" after the dot.
        assert_eq!(label_matches("t", "notes.txt", true), Some(vec![6..7]));
    }
}
