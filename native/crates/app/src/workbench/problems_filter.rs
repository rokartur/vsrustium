//! The Problems view's filter (viewFilter.ts `FilterWidget` in the panel title, markersViewActions.ts
//! `MarkersFilters`, the markers.contribution.ts filter and view mode actions). Kept in the workspace memento
//! `memento/workbench.panel.markers` as markersView.ts `saveState` writes it.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui::{Bounds, Context, Div, FocusHandle, MouseButton, MouseDownEvent, Pixels, Task, Window, div, prelude::*, px};
use config::configuration::Overrides;
use serde_json::{Map, Value, json};

use super::Workbench;
use super::input_box_view::{InputId, InputLook, InputView};
use super::output_view::bounds_tracker;
use super::problems_view::MARKERS_VIEW_ID;
use crate::commands::Outcome;
use crate::history::HistoryNavigator;
use crate::input_box::InputBox;
use crate::markers_filter::{FilesExclude, FilterOptions};
use crate::menus::MenuAction;
use crate::theme::ThemeKind;

const MEMENTO_KEY: &str = "memento/workbench.panel.markers";
/// HistoryInputBox's `HistoryNavigator` limit.
const HISTORY_LIMIT: usize = 100;
const PLACEHOLDER: &str = "Filter (e.g. text, **/*.ts, !**/node_modules/**, @source:ts)";

/// markers.ts `MarkersViewMode`.
#[derive(Clone, Copy, PartialEq)]
pub enum ViewMode {
    Tree,
    Table,
}

pub struct ProblemsFilter {
    pub input: InputBox,
    pub view: InputView,
    pub focus: FocusHandle,
    filtering: Option<Task<()>>,
    pub history: HistoryNavigator,
    /// The filter text in effect, 300 ms after the last edit (`delayedFilterUpdate`).
    pub text: String,
    pub show_errors: bool,
    pub show_warnings: bool,
    pub show_infos: bool,
    /// Show Active File Only.
    pub active_file: bool,
    /// `useFilesExclude`: hide `files.exclude` matches (Show Excluded Files unchecked).
    pub excluded_files: bool,
    pub view_mode: ViewMode,
    menu_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl ProblemsFilter {
    pub fn new(cx: &mut gpui::App) -> Self {
        Self {
            input: InputBox::default(),
            view: InputView::default(),
            focus: cx.focus_handle(),
            filtering: None,
            history: HistoryNavigator::new(Vec::new(), HISTORY_LIMIT),
            text: String::new(),
            show_errors: true,
            show_warnings: true,
            show_infos: true,
            active_file: false,
            excluded_files: false,
            view_mode: ViewMode::Tree,
            menu_bounds: Rc::default(),
        }
    }
}

/// The five filter toggles of `viewFilterSubmenu`: command suffix, title, group.
const TOGGLES: [(&str, &str); 5] = [("toggleErrors", "Show Errors"), ("toggleWarnings", "Show Warnings"), ("toggleInfos", "Show Infos"), ("toggleActiveFile", "Show Active File Only"), ("toggleExcludedFiles", "Show Excluded Files")];

impl Workbench {
    /// The memento's state, or `problems.defaultViewMode` for a view mode never chosen.
    pub(super) fn restore_problems_filter(&mut self) {
        let stored: Value = self.workspace_storage.lock().unwrap().get(MEMENTO_KEY).and_then(|text| serde_json::from_str(text).ok()).unwrap_or_default();
        let default_mode = match self.setting("problems.defaultViewMode").as_str() {
            Some("table") => ViewMode::Table,
            _ => ViewMode::Tree,
        };
        let filter = &mut self.problems.filter;
        filter.text = stored["filter"].as_str().unwrap_or_default().to_string();
        filter.input.set_value(filter.text.clone());
        let history = stored["filterHistory"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
        filter.history = HistoryNavigator::new(history, HISTORY_LIMIT);
        filter.show_errors = stored["showErrors"] != false;
        filter.show_warnings = stored["showWarnings"] != false;
        filter.show_infos = stored["showInfos"] != false;
        filter.excluded_files = stored["useFilesExclude"] == true;
        filter.active_file = stored["activeFile"] == true;
        filter.view_mode = match stored["viewMode"].as_str() {
            Some("table") => ViewMode::Table,
            Some("tree") => ViewMode::Tree,
            _ => default_mode,
        };
    }

    /// markersView.ts `saveState`, run on every change where upstream runs it on shutdown.
    fn save_problems_filter(&self) {
        let filter = &self.problems.filter;
        let memento = json!({
            "filter": filter.text,
            "filterHistory": filter.history.elements(),
            "showErrors": filter.show_errors,
            "showWarnings": filter.show_warnings,
            "showInfos": filter.show_infos,
            "useFilesExclude": filter.excluded_files,
            "activeFile": filter.active_file,
            "multiline": false,
            "viewMode": if filter.view_mode == ViewMode::Table { "table" } else { "tree" },
        });
        self.workspace_storage.lock().unwrap().set(MEMENTO_KEY, memento.to_string());
    }

    pub(super) fn register_problems_filter_actions(&mut self) {
        for (suffix, _) in TOGGLES {
            self.commands.register(format!("workbench.actions.{MARKERS_VIEW_ID}.{suffix}"), move |workbench, _, _, cx| {
                let filter = &mut workbench.problems.filter;
                let flag = match suffix {
                    "toggleErrors" => &mut filter.show_errors,
                    "toggleWarnings" => &mut filter.show_warnings,
                    "toggleInfos" => &mut filter.show_infos,
                    "toggleActiveFile" => &mut filter.active_file,
                    _ => &mut filter.excluded_files,
                };
                *flag = !*flag;
                workbench.save_problems_filter();
                cx.notify();
                Outcome::done()
            });
        }
        for (suffix, mode) in [("viewAsTable", ViewMode::Table), ("viewAsTree", ViewMode::Tree)] {
            self.commands.register(format!("workbench.actions.table.{MARKERS_VIEW_ID}.{suffix}"), move |workbench, _, _, cx| {
                workbench.problems.filter.view_mode = mode;
                workbench.save_problems_filter();
                cx.notify();
                Outcome::done()
            });
        }
        self.commands.register("problems.action.focusFilter", |workbench, _, window, cx| {
            window.focus(&workbench.problems.filter.focus, cx);
            Outcome::done()
        });
        self.commands.register("problems.action.focusProblemsFromFilter", |workbench, _, window, cx| {
            window.focus(&workbench.problems_focus, cx);
            Outcome::done()
        });
        self.commands.register("problems.action.clearFilterText", |workbench, _, window, cx| {
            workbench.edit_problems_filter(|input| input.set_value(String::new()), window, cx);
            Outcome::done()
        });
    }

    /// `FilterWidget.onDidChangeFilterText` after the 300 ms delayer: the text applies and joins the history.
    pub(super) fn edit_problems_filter(&mut self, edit: impl FnOnce(&mut InputBox), window: &mut Window, cx: &mut Context<Self>) {
        let before = self.problems.filter.input.value.clone();
        edit(&mut self.problems.filter.input);
        cx.notify();
        if self.problems.filter.input.value == before {
            return;
        }
        self.problems.filter.filtering = Some(cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(Duration::from_millis(300)).await;
            let _ = workbench.update(cx, |workbench, cx| {
                let filter = &mut workbench.problems.filter;
                filter.text = filter.input.value.clone();
                filter.history.add_to_history(&filter.text, false);
                workbench.save_problems_filter();
                cx.notify();
            });
        }));
    }

    /// `history.showPrevious` and `history.showNext` in the filter box.
    pub(super) fn show_problems_filter_history_value(&mut self, previous: bool, window: &mut Window, cx: &mut Context<Self>) {
        let filter = &mut self.problems.filter;
        let value = match previous {
            true => filter.history.show_previous_value(&filter.input.value),
            false => Some(filter.history.show_next_value(&filter.input.value)),
        };
        if let Some(value) = value {
            self.edit_problems_filter(|input| input.set_value(value), window, cx);
        }
    }

    /// markersView.ts `updateFilter`: the options from the text, the toggles and, with `useFilesExclude`, each
    /// folder's `files.exclude`.
    pub(super) fn problems_filter_options(&self) -> FilterOptions {
        let filter = &self.problems.filter;
        let files_exclude = match filter.excluded_files {
            false => FilesExclude::Roots(Vec::new()),
            true => {
                let settings = self.settings.lock().unwrap();
                let exclude = |root: Option<&std::path::Path>| match settings.get_value(Some("files.exclude"), &Overrides { resource: root, ..Overrides::default() }) {
                    Some(Value::Object(expression)) => expression,
                    _ => Map::new(),
                };
                let folders: Vec<_> = self.workspace().into_iter().flat_map(|workspace| &workspace.folders).map(|folder| folder.path.clone()).collect();
                match folders.is_empty() {
                    true => FilesExclude::Global(exclude(None)),
                    false => FilesExclude::Roots(folders.iter().map(|folder| (folder.to_string_lossy().into_owned(), exclude(Some(folder)))).collect()),
                }
            }
        };
        FilterOptions::new(&filter.text, files_exclude, filter.show_warnings, filter.show_errors, filter.show_infos)
    }

    /// markers.ts context keys of the filter.
    pub(super) fn problems_filter_context(&self, window: &Window, context: &mut HashMap<String, Value>) {
        let filter = &self.problems.filter;
        let focused = filter.focus.is_focused(window);
        context.insert("problemsFilterFocus".into(), json!(focused));
        context.insert("problems.filter.errors".into(), json!(filter.show_errors));
        context.insert("problems.filter.warnings".into(), json!(filter.show_warnings));
        context.insert("problems.filter.info".into(), json!(filter.show_infos));
        context.insert("problems.filter.activeFile".into(), json!(filter.active_file));
        context.insert("problems.filter.excludedFiles".into(), json!(filter.excluded_files));
        context.insert("problemsViewMode".into(), json!(if filter.view_mode == ViewMode::Table { "table" } else { "tree" }));
        if focused {
            // contextScopedHistoryWidget.ts: the filter is a history input.
            context.insert("inputFocus".into(), json!(true));
            context.insert("historyNavigationWidgetFocus".into(), json!(true));
            context.insert("historyNavigationForwardsEnabled".into(), json!(true));
            context.insert("historyNavigationBackwardsEnabled".into(), json!(true));
        }
    }

    /// markersView.ts `checkMoreFilters`: the filter button shows checked while a toggle hides markers.
    fn problems_more_filters(&self) -> bool {
        let filter = &self.problems.filter;
        !filter.show_errors || !filter.show_warnings || !filter.show_infos || filter.excluded_files || filter.active_file
    }

    /// `viewFilterSubmenu` of the Problems view: severities, then the active file and excluded files.
    fn show_problems_filter_menu(&mut self, anchor: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let filter = &self.problems.filter;
        let checked = [filter.show_errors, filter.show_warnings, filter.show_infos, filter.active_file, !filter.excluded_files];
        let mut actions = Vec::new();
        for (index, ((suffix, title), checked)) in TOGGLES.iter().zip(checked).enumerate() {
            if index == 3 {
                actions.push(MenuAction::Separator);
            }
            actions.push(MenuAction::Command { id: format!("workbench.actions.{MARKERS_VIEW_ID}.{suffix}"), title: title.to_string(), enabled: true, checked, alt: None });
        }
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }

    /// The view's title actions: the filter, Collapse All (tree), View as Table or View as Tree.
    pub(super) fn problems_title_actions(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let (mode_icon, mode_command) = match self.problems.filter.view_mode {
            ViewMode::Tree => ("list-flat", "workbench.actions.table.workbench.panel.markers.view.viewAsTable"),
            ViewMode::Table => ("list-tree", "workbench.actions.table.workbench.panel.markers.view.viewAsTree"),
        };
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(self.problems_filter_box(window, cx))
            .when(self.problems.filter.view_mode == ViewMode::Tree, |actions| actions.child(self.search_action("collapse-all", "workbench.actions.treeView.workbench.panel.markers.view.collapseAll", true, cx)))
            .child(self.search_action(mode_icon, mode_command, true, cx))
    }

    /// viewFilter.ts `FilterWidget` in a panel title (views.css): 150px to 200px, 400px while focused, 10px right
    /// margin, the filter submenu button at the input's right.
    fn problems_filter_box(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let filter = &self.problems.filter;
        let focused = filter.focus.is_focused(window);
        let height = if self.theme.kind == ThemeKind::Light { 25. } else { 24. };
        let border = if focused { self.color("focusBorder") } else { self.theme.color("input.border").unwrap_or(self.color("input.background")) };
        let checked = self.problems_more_filters();
        let foreground = if checked { self.foreground_or("inputOption.activeForeground") } else { self.foreground_or("icon.foreground") };
        let bounds = filter.menu_bounds.clone();
        // `updateBadge`: "Showing {0} of {1}" while the filters hide markers.
        let (filtered, total) = self.problems_filter_stats();
        let badge = (total > 0 && filtered != total).then(|| {
            div().flex_none().my(px(4.)).mr(px(2.)).px(px(8.)).rounded(px(2.)).bg(self.color("badge.background")).text_color(self.foreground_or("badge.foreground")).child(format!("Showing {filtered} of {total}"))
        });
        let filter_button = self
            .icon("filter", 20., 16., foreground)
            .id("problems-filter-menu")
            .relative()
            .rounded(px(4.))
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .when(checked, |button| button.bg(self.color("inputOption.activeBackground")).border_color(self.color("inputOption.activeBorder")))
            .when(!checked, |button| button.hover(|button| button.bg(self.color("toolbar.hoverBackground"))))
            .child(bounds_tracker(bounds.clone()))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                workbench.show_problems_filter_menu(bounds.get(), window, cx);
            }));
        div()
            .relative()
            .w(px(self.panel_filter_width(focused)))
            .flex_shrink(1.)
            .min_w(px(150.))
            .mr(px(10.))
            .h(px(height))
            .flex()
            .items_center()
            .pl(px(4.))
            .pr(px(2.))
            .border_1()
            .rounded(px(4.))
            .cursor_text()
            .text_size(px(12.))
            .bg(self.color("input.background"))
            .border_color(border)
            .text_color(self.foreground_or("input.foreground"))
            .child(div().flex_1().min_w_0().h_full().flex().child(self.input_text(InputId::ProblemsFilter, InputLook { password: false, placeholder: Some(PLACEHOLDER.to_owned()), focused }, cx)))
            .children(badge)
            .child(filter_button)
            .child(self.panel_filter_tracker())
    }

    /// markersView.ts `clearFilters`.
    pub(super) fn clear_problems_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let filter = &mut self.problems.filter;
        filter.excluded_files = false;
        filter.show_errors = true;
        filter.show_warnings = true;
        filter.show_infos = true;
        self.edit_problems_filter(|input| input.set_value(String::new()), window, cx);
        self.save_problems_filter();
    }
}
