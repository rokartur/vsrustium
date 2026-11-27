//! The Problems view (markersView.ts, markersTreeViewer.ts, markersModel.ts): the markers of every resource as a
//! tree, a resource row with its file icon, folder and count over its markers, each with its severity icon, message,
//! source, code and position, its related information below. A click opens the marker's location. The filter box
//! and its menu (markersViewActions.ts, markersFilterOptions.ts) pick what shows.
// shortcut: no indent guides, multi-selection, or focus apart from selection.

use std::cell::Cell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{Bounds, Context, Div, FocusHandle, KeyDownEvent, Pixels, Task, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::input_box_view::{InputId, InputView};
use super::panel::PanelView;
use super::problems_table::{ProblemsTable, TableItem, table_items};
use super::view_filter::ViewFilter;
use crate::commands::Outcome;
use crate::history::HistoryNavigator;
use crate::input_box::InputBox;
use crate::markers::{ERROR, INFO, Marker, ReadOptions, WARNING};
use crate::markers_filter::{FilesExclude, FilterOptions, Highlights, Visibility, filter_marker, filter_related, filter_resource, marker_code, marker_lines};
use crate::menus::MenuAction;

/// markers.ts `Markers.MARKERS_CONTAINER_ID` and `MARKERS_VIEW_ID`.
pub const MARKERS_CONTAINER_ID: &str = "workbench.panel.markers";
pub(super) const MARKERS_VIEW_ID: &str = "workbench.panel.markers.view";
/// markers.ts `MARKERS_VIEW_STORAGE_ID`: the view's memento in workspace storage.
const MEMENTO_KEY: &str = "memento/workbench.panel.markers";
const ROW_HEIGHT: f32 = 22.;
const FILTER_PLACEHOLDER: &str = "Filter (e.g. text, **/*.ts, !**/node_modules/**, @source:ts)";
/// HistoryInputBox's `HistoryNavigator` limit.
const FILTER_HISTORY_LIMIT: usize = 100;

pub struct ProblemsView {
    /// Resources whose markers are hidden, by `URI.toString()`.
    pub collapsed: HashSet<String>,
    /// Markers whose related information is hidden, by resource and index.
    collapsed_markers: HashSet<(String, usize)>,
    /// Markers shown on one line while `multiline` is on, or on all lines while it is off.
    toggled_lines: HashSet<(String, usize)>,
    pub(super) filter: InputBox,
    pub(super) filter_view: InputView,
    pub(super) filter_focus: FocusHandle,
    /// The text applies 300 ms after the last edit (`FilterWidget.onDidChangeFilterText`).
    filtering: Option<Task<()>>,
    /// The applied filter text.
    text: String,
    /// The filter box's history (`filterHistory`).
    filter_history: HistoryNavigator,
    show_errors: bool,
    show_warnings: bool,
    show_infos: bool,
    /// `useFilesExclude`: hide `files.exclude` matches ("Show Excluded Files" unchecked).
    excluded_files: bool,
    active_file: bool,
    /// `MarkersViewModel.multiline`: every marker shows all its message lines.
    multiline: bool,
    /// `MarkersViewMode.Table`; `None` until set, then `problems.defaultViewMode` decides.
    table: Option<bool>,
    pub(super) table_view: ProblemsTable,
    menu_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The focused and selected row.
    selected: Option<Row>,
    /// The light bulb of the selected marker's row, which Cmd+. opens.
    selected_light_bulb: Rc<Cell<Bounds<Pixels>>>,
}

/// A row of the tree: a resource by `URI.toString()`, a marker by resource and index, or a marker's related
/// information by its index.
#[derive(Clone, Debug, PartialEq)]
enum Row {
    Resource(String),
    Marker(String, usize),
    Related(String, usize, usize),
}

impl ProblemsView {
    pub fn new(cx: &mut gpui::App) -> ProblemsView {
        ProblemsView {
            collapsed: HashSet::new(),
            collapsed_markers: HashSet::new(),
            toggled_lines: HashSet::new(),
            filter: InputBox::default(),
            filter_view: InputView::default(),
            filter_focus: cx.focus_handle(),
            filtering: None,
            text: String::new(),
            filter_history: HistoryNavigator::new(Vec::new(), FILTER_HISTORY_LIMIT),
            show_errors: true,
            show_warnings: true,
            show_infos: true,
            excluded_files: false,
            active_file: false,
            multiline: true,
            table: None,
            table_view: ProblemsTable::default(),
            menu_bounds: Rc::default(),
            selected: None,
            selected_light_bulb: Rc::default(),
        }
    }

    /// `IMarkersPanelState` as markersView.ts reads it: missing severities are shown, multi-line is on.
    fn restore(&mut self, memento: &Value) {
        self.text = memento["filter"].as_str().unwrap_or_default().to_owned();
        self.filter.set_value(self.text.clone());
        let history = memento["filterHistory"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
        self.filter_history = HistoryNavigator::new(history, FILTER_HISTORY_LIMIT);
        self.show_errors = memento["showErrors"] != false;
        self.show_warnings = memento["showWarnings"] != false;
        self.show_infos = memento["showInfos"] != false;
        self.excluded_files = memento["useFilesExclude"] == true;
        self.active_file = memento["activeFile"] == true;
        self.multiline = memento["multiline"] != false;
        self.table = memento["viewMode"].as_str().map(|mode| mode == "table");
    }

    fn memento(&self) -> Value {
        json!({
            "filter": self.text,
            "filterHistory": self.filter_history.elements(),
            "showErrors": self.show_errors,
            "showWarnings": self.show_warnings,
            "showInfos": self.show_infos,
            "useFilesExclude": self.excluded_files,
            "activeFile": self.active_file,
            "multiline": self.multiline,
            "viewMode": self.table.map(|table| if table { "table" } else { "tree" }),
        })
    }

    /// `checkMoreFilters`: a filter of the menu is not at its default.
    fn more_filters(&self) -> bool {
        !self.show_errors || !self.show_warnings || !self.show_infos || self.excluded_files || self.active_file
    }

    fn shows_all_lines(&self, key: &(String, usize)) -> bool {
        self.multiline != self.toggled_lines.contains(key)
    }
}

/// A resource the filter keeps, with the markers and related information it keeps.
pub struct ShownResource<'a> {
    pub resource: &'a ResourceMarkers,
    pub highlights: Highlights,
    pub markers: Vec<ShownMarker<'a>>,
}

pub struct ShownMarker<'a> {
    pub index: usize,
    pub marker: &'a Arc<Marker>,
    pub highlights: Highlights,
    pub related: Vec<(&'a Value, Highlights)>,
}

/// markersTreeViewer.ts `Filter` over the tree: a resource shows when the filter keeps it or one of its markers; a
/// marker's related information shows under it.
pub fn filter_resources<'a>(resources: &'a [ResourceMarkers], options: &FilterOptions) -> Vec<ShownResource<'a>> {
    let mut shown = Vec::new();
    for resource in resources {
        let path = crate::uri::Uri::parse(&resource.resource).map(|uri| uri.path).unwrap_or_else(|_| resource.path.to_string_lossy().into_owned());
        let basename = resource.path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let (visibility, highlights) = filter_resource(options, &path, &basename);
        if visibility == Visibility::Hidden {
            continue;
        }
        let mut markers = Vec::new();
        for (index, marker) in resource.markers.iter().enumerate() {
            let (marker_visibility, highlights) = filter_marker(options, marker, visibility);
            if marker_visibility == Visibility::Hidden {
                continue;
            }
            let related: Vec<(&Value, Highlights)> = related_information(marker)
                .iter()
                .filter_map(|related| {
                    let basename = related_basename(related);
                    let (visibility, highlights) = filter_related(options, &basename, related["message"].as_str().unwrap_or_default(), marker_visibility);
                    (visibility == Visibility::Visible).then_some((related, highlights))
                })
                .collect();
            if marker_visibility == Visibility::Visible || !related.is_empty() {
                markers.push(ShownMarker { index, marker, highlights, related });
            }
        }
        if visibility == Visibility::Visible || !markers.is_empty() {
            shown.push(ShownResource { resource, highlights, markers });
        }
    }
    shown
}

fn related_information(marker: &Marker) -> &[Value] {
    marker.related_information.as_array().map_or(&[], Vec::as_slice)
}

/// markersModel.ts `Marker.toString`: the marker as tab-indented JSON, its resource and related resources as paths.
fn marker_json(marker: &Marker) -> String {
    let path = |uri: &Value| super::host_requests::uri_parts(uri).map(|(_, path)| path.to_string_lossy().into_owned()).unwrap_or_default();
    let mut json = serde_json::Map::new();
    json.insert("resource".into(), json!(crate::uri::Uri::parse(&marker.resource).map(|uri| uri.path).unwrap_or_default()));
    json.insert("owner".into(), json!(marker.owner));
    if !marker.code.is_null() {
        json.insert("code".into(), marker.code.clone());
    }
    json.insert("severity".into(), json!(marker.severity));
    json.insert("message".into(), json!(marker.message));
    if let Some(source) = &marker.source {
        json.insert("source".into(), json!(source));
    }
    for (key, value) in [("startLineNumber", marker.start_line), ("startColumn", marker.start_column), ("endLineNumber", marker.end_line), ("endColumn", marker.end_column)] {
        json.insert(key.into(), json!(value));
    }
    let related = related_information(marker);
    if !related.is_empty() {
        let related: Vec<Value> = related.iter().map(|related| {
            let mut related = related.clone();
            related["resource"] = json!(path(&related["resource"]));
            related
        }).collect();
        json.insert("relatedInformation".into(), json!(related));
    }
    if let Some(version) = marker.model_version_id {
        json.insert("modelVersionId".into(), json!(version));
    }
    if !marker.tags.is_empty() {
        json.insert("tags".into(), json!(marker.tags));
    }
    if let Some(origin) = &marker.origin {
        json.insert("origin".into(), json!(origin));
    }
    let mut out = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, serde_json::ser::PrettyFormatter::with_indent(b"\t"));
    serde::Serialize::serialize(&Value::Object(json), &mut serializer).expect("JSON values serialize");
    String::from_utf8(out).expect("serde_json writes UTF-8")
}

fn related_basename(related: &Value) -> String {
    let path = super::host_requests::uri_parts(&related["resource"]).map(|(_, path)| path).unwrap_or_default();
    path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

/// `ResourceMarkers`: a resource's file path and its markers in `_compareMarkers` order.
pub struct ResourceMarkers {
    pub resource: String,
    pub path: PathBuf,
    pub markers: Vec<Arc<Marker>>,
}

/// markersModel.ts: each resource's markers by severity then position; resources by their first marker's severity,
/// then path.
pub fn resource_markers(mut markers: Vec<Arc<Marker>>) -> Vec<ResourceMarkers> {
    markers.sort_by(|a, b| b.severity.cmp(&a.severity).then((a.start_line, a.start_column, a.end_line, a.end_column).cmp(&(b.start_line, b.start_column, b.end_line, b.end_column))));
    let mut resources: Vec<ResourceMarkers> = Vec::new();
    for marker in markers {
        match resources.iter_mut().find(|resource| resource.resource == marker.resource) {
            Some(resource) => resource.markers.push(marker),
            None => {
                let path = crate::editor_history::file_path(&marker.resource).unwrap_or_else(|| PathBuf::from(&marker.resource));
                resources.push(ResourceMarkers { resource: marker.resource.clone(), path, markers: vec![marker] });
            }
        }
    }
    resources.sort_by(|a, b| b.markers[0].severity.cmp(&a.markers[0].severity).then_with(|| a.path.cmp(&b.path)));
    resources
}

impl Workbench {
    /// markers.contribution.ts registers the Problems container in the panel at order 0.
    pub(super) fn register_problems_panel_view(&mut self) {
        self.register_panel_view(PanelView {
            id: MARKERS_CONTAINER_ID,
            view_id: MARKERS_VIEW_ID,
            title: "Problems",
            open: |workbench, window, cx| {
                workbench.open_panel_view(MARKERS_CONTAINER_ID, cx);
                workbench.focus_problems(window, cx);
            },
            render: |workbench, window, cx| {
                let filter = (!workbench.view_filter_in_header(MARKERS_CONTAINER_ID)).then(|| workbench.problems_filter_box(window, cx));
                div().flex_1().min_h_0().flex().flex_col().children(filter).child(workbench.problems_body(window, cx)).into_any_element()
            },
            // The navigation group: the filter, View as Tree/Table (order 3), Collapse All in the tree (order 2).
            actions: |workbench, window, cx| {
                let table = workbench.problems_table_mode();
                let (icon, command) = match table {
                    true => ("list-tree", "workbench.actions.table.workbench.panel.markers.view.viewAsTree"),
                    false => ("list-flat", "workbench.actions.table.workbench.panel.markers.view.viewAsTable"),
                };
                div()
                    .flex()
                    .items_center()
                    .when(workbench.view_filter_in_header(MARKERS_CONTAINER_ID), |actions| actions.child(workbench.problems_filter_box(window, cx)))
                    .when(!table, |actions| actions.child(workbench.search_action("collapse-all", "workbench.actions.treeView.workbench.panel.markers.view.collapseAll", true, cx).mr(px(4.))))
                    .child(workbench.search_action(icon, command, true, cx))
                    .into_any_element()
            },
            visible: |_| true,
            // markersView.ts: the tab counts every marker the view shows.
            badge: |workbench| Some(workbench.diagnostics.service.read(ReadOptions::default()).len()),
        });
        // viewsService.ts `registerOpenViewAction`: a focused view's command hides its part, else it opens focused.
        self.commands.register("workbench.actions.view.problems", |workbench, _, window, cx| {
            match workbench.panel_view_visible(MARKERS_CONTAINER_ID) && workbench.problems_focus.is_focused(window) {
                true => workbench.hide_panel(cx),
                false => {
                    workbench.open_panel_view(MARKERS_CONTAINER_ID, cx);
                    workbench.focus_problems(window, cx);
                }
            }
            Outcome::done()
        });
        self.commands.register("workbench.actions.view.toggleProblems", |workbench, _, window, cx| {
            match workbench.panel_view_visible(MARKERS_CONTAINER_ID) {
                true => workbench.close_panel_view(MARKERS_CONTAINER_ID, cx),
                false => {
                    workbench.open_panel_view(MARKERS_CONTAINER_ID, cx);
                    workbench.focus_problems(window, cx);
                }
            }
            Outcome::done()
        });
        self.commands.register("workbench.actions.treeView.workbench.panel.markers.view.collapseAll", |workbench, _, _, cx| {
            workbench.problems.collapsed = workbench.diagnostics.service.read(ReadOptions::default()).iter().map(|marker| marker.resource.clone()).collect();
            workbench.problems.selected = None;
            cx.notify();
            Outcome::done()
        });
        let toggles: [(&str, fn(&mut ProblemsView) -> &mut bool); 5] = [
            ("toggleErrors", |view| &mut view.show_errors),
            ("toggleWarnings", |view| &mut view.show_warnings),
            ("toggleInfos", |view| &mut view.show_infos),
            ("toggleActiveFile", |view| &mut view.active_file),
            ("toggleExcludedFiles", |view| &mut view.excluded_files),
        ];
        for (name, flag) in toggles {
            self.commands.register(format!("workbench.actions.{MARKERS_VIEW_ID}.{name}"), move |workbench, _, _, cx| {
                let flag = flag(&mut workbench.problems);
                *flag = !*flag;
                workbench.problems_state_changed(cx);
                Outcome::done()
            });
        }
        self.commands.register("problems.action.focusFilter", |workbench, _, window, cx| {
            window.focus(&workbench.problems.filter_focus, cx);
            Outcome::done()
        });
        self.commands.register("problems.action.focusProblemsFromFilter", |workbench, _, window, cx| {
            window.focus(&workbench.problems_focus, cx);
            Outcome::done()
        });
        self.commands.register("problems.action.showQuickFixes", |workbench, _, window, cx| {
            if matches!(workbench.problems.selected, Some(Row::Marker(..))) {
                let anchor = workbench.problems.selected_light_bulb.get();
                workbench.show_problems_quick_fixes(anchor, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("problems.action.open", |workbench, _, window, cx| {
            workbench.open_problems_row(window, cx);
            Outcome::done()
        });
        // `MARKER_COPY_ACTION_ID` and `MARKER_COPY_MESSAGE_ACTION_ID`: the selected markers, or every marker shown.
        self.commands.register("problems.action.copy", |workbench, _, _, cx| {
            let markers: Vec<String> = workbench.selected_problem_markers().iter().map(|marker| marker_json(marker)).collect();
            if !markers.is_empty() {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(format!("[{}]", markers.join(","))));
            }
            Outcome::done()
        });
        self.commands.register("problems.action.copyMessage", |workbench, _, _, cx| {
            let messages: Vec<String> = workbench.selected_problem_markers().iter().map(|marker| marker.message.clone()).collect();
            if !messages.is_empty() {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(messages.join("\n")));
            }
            Outcome::done()
        });
        self.commands.register("problems.action.copyRelatedInformationMessage", |workbench, _, _, cx| {
            let Some(Row::Related(uri, index, related)) = workbench.problems.selected.clone() else { return Outcome::done() };
            let marker = workbench.problems_resources().into_iter().find(|resource| resource.resource == uri).and_then(|resource| resource.markers.get(index).cloned());
            if let Some(message) = marker.as_ref().and_then(|marker| related_information(marker).get(related)).and_then(|related| related["message"].as_str()) {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(message.to_owned()));
            }
            Outcome::done()
        });
        self.commands.register("problems.action.clearFilterText", |workbench, _, window, cx| {
            workbench.edit_problems_filter(|input| input.set_value(String::new()), window, cx);
            Outcome::done()
        });
        for (command, table) in [("viewAsTree", false), ("viewAsTable", true)] {
            self.commands.register(format!("workbench.actions.table.{MARKERS_VIEW_ID}.{command}"), move |workbench, _, _, cx| {
                workbench.problems.table = Some(table);
                workbench.problems_state_changed(cx);
                Outcome::done()
            });
        }
        for (id, multiline) in [("problems.action.showMultilineMessage", true), ("problems.action.showSinglelineMessage", false)] {
            self.commands.register(id, move |workbench, _, _, cx| {
                workbench.problems.multiline = multiline;
                workbench.problems.toggled_lines.clear();
                workbench.problems_state_changed(cx);
                Outcome::done()
            });
        }
    }

    pub(super) fn restore_problems_state(&mut self) {
        let memento = self.workspace_storage.lock().unwrap().get(MEMENTO_KEY).and_then(|raw| serde_json::from_str::<Value>(raw).ok());
        self.problems.restore(&memento.unwrap_or(Value::Null));
    }

    /// The `${MARKERS_VIEW_ID}.active` key, the filter keys and `focusedView` while the view or its filter has focus.
    pub(super) fn problems_context(&self, window: &Window, context: &mut std::collections::HashMap<String, Value>) {
        context.insert(format!("{MARKERS_VIEW_ID}.active"), json!(true));
        let filter_focused = self.problems.filter_focus.is_focused(window);
        let tree_focused = self.problems_focus.is_focused(window);
        context.insert("problemsFilterFocus".into(), json!(filter_focused));
        context.insert("problemsViewMode".into(), json!(if self.problems_table_mode() { "table" } else { "tree" }));
        context.insert("problems.filter.errors".into(), json!(self.problems.show_errors));
        context.insert("problems.filter.warnings".into(), json!(self.problems.show_warnings));
        context.insert("problems.filter.info".into(), json!(self.problems.show_infos));
        context.insert("problems.filter.activeFile".into(), json!(self.problems.active_file));
        context.insert("problems.filter.excludedFiles".into(), json!(self.problems.excluded_files));
        if filter_focused || tree_focused {
            context.insert("focusedView".into(), json!(MARKERS_VIEW_ID));
        }
        if filter_focused {
            // contextScopedHistoryWidget.ts: the filter is a history input.
            for key in ["inputFocus", "historyNavigationWidgetFocus", "historyNavigationForwardsEnabled", "historyNavigationBackwardsEnabled"] {
                context.insert(key.into(), json!(true));
            }
        }
        let selected = self.problems.selected.as_ref().filter(|_| tree_focused);
        context.insert("problemsVisibility".into(), json!(!self.problems_table_mode()));
        context.insert("problemFocus".into(), json!(matches!(selected, Some(Row::Marker(..)))));
        context.insert("relatedInformationFocus".into(), json!(matches!(selected, Some(Row::Related(..)))));
    }

    /// markersView.ts `focus`: the tree takes focus and, with nothing selected, `setMarkerSelection` selects the
    /// first marker shown.
    fn focus_problems(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.problems_focus, cx);
        if self.problems.selected.is_none() {
            let first = self.problems_rows().into_iter().find(|row| matches!(row, Row::Marker(..)));
            self.select_problems_row(first, cx);
        }
        cx.notify();
    }

    /// The tree's rows in order: each resource, then, unless collapsed, its markers with their related information.
    fn problems_rows(&self) -> Vec<Row> {
        let resources = self.problems_resources();
        let mut rows = Vec::new();
        for resource in filter_resources(&resources, &self.problems_filter_options()) {
            let uri = resource.resource.resource.clone();
            rows.push(Row::Resource(uri.clone()));
            if self.problems.collapsed.contains(&uri) {
                continue;
            }
            for marker in &resource.markers {
                rows.push(Row::Marker(uri.clone(), marker.index));
                if !self.problems.collapsed_markers.contains(&(uri.clone(), marker.index)) {
                    rows.extend((0..marker.related.len()).map(|related| Row::Related(uri.clone(), marker.index, related)));
                }
            }
        }
        rows
    }

    /// The list's keys: Up and Down move the selection, Enter and Space open a marker or toggle a resource.
    fn problems_tree_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.problems_table_mode() {
            return;
        }
        let rows = self.problems_rows();
        let index = self.problems.selected.as_ref().and_then(|selected| rows.iter().position(|row| row == selected));
        let next = match event.keystroke.key.as_str() {
            "down" => index.map_or(0, |index| (index + 1).min(rows.len().saturating_sub(1))),
            "up" => index.map_or(0, |index| index.saturating_sub(1)),
            "enter" | "space" => {
                cx.stop_propagation();
                return self.open_problems_row(window, cx);
            }
            _ => return,
        };
        cx.stop_propagation();
        self.select_problems_row(rows.get(next).cloned(), cx);
    }

    /// The selection moves to `row`; a marker's row asks for its quick fixes (`showLightBulb`).
    fn select_problems_row(&mut self, row: Option<Row>, cx: &mut Context<Self>) {
        if let Some(Row::Marker(uri, index)) = &row {
            let marker = self.problems_resources().into_iter().find(|resource| resource.resource == *uri).and_then(|resource| resource.markers.get(*index).cloned());
            if let Some(marker) = marker {
                self.problems_marker_hovered(&marker, cx);
            }
        }
        self.problems.selected = row;
        cx.notify();
    }

    /// `getSelectedMarkers`: a selected resource's markers or the selected marker, else every resource's markers.
    fn selected_problem_markers(&self) -> Vec<Arc<Marker>> {
        let resources = self.problems_resources();
        match &self.problems.selected {
            Some(Row::Resource(uri)) => resources.iter().filter(|resource| resource.resource == *uri).flat_map(|resource| resource.markers.clone()).collect(),
            Some(Row::Marker(uri, index)) => resources.iter().filter(|resource| resource.resource == *uri).filter_map(|resource| resource.markers.get(*index).cloned()).collect(),
            _ => resources.iter().flat_map(|resource| resource.markers.clone()).collect(),
        }
    }

    fn open_problems_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.problems.selected.clone() else { return };
        let resources = self.problems_resources();
        let marker_at = |uri: &str, index: usize| resources.iter().find(|resource| resource.resource == uri).and_then(|resource| resource.markers.get(index)).cloned();
        match row {
            Row::Resource(uri) => {
                if !self.problems.collapsed.remove(&uri) {
                    self.problems.collapsed.insert(uri);
                }
                cx.notify();
            }
            Row::Marker(uri, index) => {
                let Some(marker) = marker_at(&uri, index) else { return };
                let target = json!({ "resource": crate::debug::uri_from_string(&marker.resource), "startLineNumber": marker.start_line, "startColumn": marker.start_column });
                self.open_marker_related(&target, window, cx);
            }
            Row::Related(uri, index, related) => {
                let Some(marker) = marker_at(&uri, index) else { return };
                let Some(target) = related_information(&marker).get(related).cloned() else { return };
                self.open_marker_related(&target, window, cx);
            }
        }
    }

    /// `getDefaultViewMode`: the stored mode, else `problems.defaultViewMode`.
    fn problems_table_mode(&self) -> bool {
        self.problems.table.unwrap_or_else(|| self.setting("problems.defaultViewMode") == "table")
    }

    /// `FilterWidget.onDidChangeFilterText` after its 300 ms delay: the filter text applies.
    pub(super) fn edit_problems_filter(&mut self, edit: impl FnOnce(&mut InputBox), window: &mut Window, cx: &mut Context<Self>) {
        let before = self.problems.filter.value.clone();
        edit(&mut self.problems.filter);
        cx.notify();
        if self.problems.filter.value == before {
            return;
        }
        self.problems.filtering = Some(cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(300)).await;
            let _ = workbench.update(cx, |workbench, cx| {
                let view = &mut workbench.problems;
                view.text = view.filter.value.clone();
                view.filter_history.add_to_history(&view.text, false);
                workbench.problems_state_changed(cx);
            });
        }));
    }

    /// `history.showPrevious` and `history.showNext` in the filter box.
    pub(super) fn show_problems_filter_history_value(&mut self, previous: bool, window: &mut Window, cx: &mut Context<Self>) {
        let view = &mut self.problems;
        let value = match previous {
            true => view.filter_history.show_previous_value(&view.filter.value),
            false => Some(view.filter_history.show_next_value(&view.filter.value)),
        };
        if let Some(value) = value {
            self.edit_problems_filter(|input| input.set_value(value), window, cx);
        }
    }

    /// `saveState`: the filters and the line mode in the workspace memento.
    fn problems_state_changed(&mut self, cx: &mut Context<Self>) {
        self.workspace_storage.lock().unwrap().set(MEMENTO_KEY, self.problems.memento().to_string());
        cx.notify();
    }

    /// `viewFilterSubmenu` of the Problems view: the severities, then the active file and excluded files filters.
    fn show_problems_filter_menu(&mut self, anchor: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let view = &self.problems;
        let item = |name: &str, title: &str, checked: bool| MenuAction::Command { id: format!("workbench.actions.{MARKERS_VIEW_ID}.{name}"), title: title.into(), enabled: true, checked, alt: None };
        let actions = vec![
            item("toggleErrors", "Show Errors", view.show_errors),
            item("toggleWarnings", "Show Warnings", view.show_warnings),
            item("toggleInfos", "Show Infos", view.show_infos),
            MenuAction::Separator,
            item("toggleActiveFile", "Show Active File Only", view.active_file),
            item("toggleExcludedFiles", "Show Excluded Files", !view.excluded_files),
        ];
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }

    fn problems_filter_box(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let (total, shown) = self.problems_counts();
        self.view_filter_box(
            ViewFilter {
                container: MARKERS_CONTAINER_ID,
                input: InputId::ProblemsFilter,
                focused: self.problems.filter_focus.is_focused(window),
                placeholder: FILTER_PLACEHOLDER,
                more_filters_checked: self.problems.more_filters(),
                badge: (total != shown && total != 0).then(|| format!("Showing {shown} of {total}")),
                menu_bounds: self.problems.menu_bounds.clone(),
                show_menu: Workbench::show_problems_filter_menu,
            },
            cx,
        )
    }

    /// `updateFilter`'s `FilterOptions`: `files.exclude` per workspace folder while "Show Excluded Files" is off.
    fn problems_filter_options(&self) -> FilterOptions {
        let view = &self.problems;
        let files_exclude = |resource: Option<&std::path::Path>| {
            let overrides = config::configuration::Overrides { resource, override_identifier: None };
            let value = self.settings.lock().unwrap().get_value(Some("files.exclude"), &overrides);
            value.and_then(|value| value.as_object().cloned()).unwrap_or_default()
        };
        let exclude = match (view.excluded_files, self.workspace()) {
            (false, _) => FilesExclude::ByRoot(Vec::new()),
            (true, Some(workspace)) => FilesExclude::ByRoot(workspace.folders.iter().map(|folder| (folder.path.to_string_lossy().into_owned(), files_exclude(Some(&folder.path)))).collect()),
            (true, None) => FilesExclude::Global(files_exclude(None)),
        };
        FilterOptions::new(&view.text, exclude, view.show_warnings, view.show_errors, view.show_infos)
    }

    /// `getResourceMarkers`: every resource, or only the active editor's with "Show Active File Only".
    fn problems_resources(&self) -> Vec<ResourceMarkers> {
        let mut resources = resource_markers(self.diagnostics.service.read(ReadOptions::default()));
        if self.problems.active_file {
            let active = self.editors.active().map(|editor| editor.document().uri_string());
            resources.retain(|resource| Some(&resource.resource) == active.as_ref());
        }
        resources
    }

    /// `getFilterStats`: every marker, and the markers the filter shows.
    fn problems_counts(&self) -> (usize, usize) {
        let total = self.diagnostics.service.read(ReadOptions::default()).len();
        let resources = self.problems_resources();
        let shown = match self.problems_table_mode() {
            true => self.problems_table_items(&resources).len(),
            false => filter_resources(&resources, &self.problems_filter_options()).iter().map(|resource| resource.markers.len()).sum(),
        };
        (total, shown)
    }

    fn problems_table_items(&self, resources: &[ResourceMarkers]) -> Vec<TableItem> {
        let markers: Vec<Arc<Marker>> = resources.iter().flat_map(|resource| resource.markers.iter().cloned()).collect();
        let workspace = self.workspace();
        let path_of = |marker: &Marker| crate::uri::Uri::parse(&marker.resource).map(|uri| uri.path).unwrap_or_default();
        let file_label = |marker: &Marker| match crate::editor_history::file_path(&marker.resource) {
            Some(path) => crate::quick_input::path_label(&path, workspace),
            None => marker.resource.clone(),
        };
        table_items(&markers, &self.problems_filter_options(), path_of, file_label)
    }

    fn problems_body(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        let resources = self.problems_resources();
        let shown = filter_resources(&resources, &self.problems_filter_options());
        let focused = self.problems_focus.is_focused(window);
        let list = div()
            .id("problems-tree")
            .track_focus(&self.problems_focus)
            .on_key_down(cx.listener(|workbench, event: &KeyDownEvent, window, cx| workbench.problems_tree_key_down(event, window, cx)))
            .flex_1().min_h_0().overflow_y_scroll().flex().flex_col().text_size(px(13.));
        let total = self.diagnostics.service.read(ReadOptions::default()).len();
        let table = self.problems_table_mode().then(|| self.problems_table_items(&resources));
        let filtered = match &table {
            Some(items) => items.len(),
            None => shown.iter().map(|resource| resource.markers.len()).sum(),
        };
        if filtered == 0 {
            return div().flex_1().min_h_0().flex().flex_col().child(list.child(self.problems_message(total, &resources, cx)));
        }
        if let Some(items) = table {
            return div().flex_1().min_h_0().flex().flex_col().child(self.problems_table(&items, list, cx));
        }
        let selected = |row: Row| self.problems.selected.as_ref() == Some(&row);
        let mut rows = Vec::new();
        for resource in &shown {
            let collapsed = self.problems.collapsed.contains(&resource.resource.resource);
            let row = Row::Resource(resource.resource.resource.clone());
            rows.push((self.list_row_selection(self.problems_resource_row(resource, collapsed, cx), selected(row.clone()), focused).into_any_element(), 1, selected(row)));
            if collapsed {
                continue;
            }
            for marker in &resource.markers {
                let key = (marker.marker.resource.clone(), marker.index);
                let related_collapsed = self.problems.collapsed_markers.contains(&key);
                let row = Row::Marker(key.0.clone(), key.1);
                rows.push((self.list_row_selection(self.problems_marker_row(marker, related_collapsed, focused, cx), selected(row.clone()), focused).into_any_element(), 2, selected(row)));
                if !related_collapsed {
                    for (index, (related, highlights)) in marker.related.iter().enumerate() {
                        let row = Row::Related(key.0.clone(), key.1, index);
                        rows.push((self.list_row_selection(self.problems_related_row(&key, index, related, highlights, cx), selected(row.clone()), focused).into_any_element(), 3, selected(row)));
                    }
                }
            }
        }
        let depths: Vec<usize> = rows.iter().map(|(_, depth, _)| *depth).collect();
        let active: Vec<usize> = rows.iter().enumerate().filter(|(_, (_, _, selected))| *selected).map(|(index, _)| index).collect();
        let group = gpui::SharedString::from("problems-tree");
        let guides = self.tree_guides_in_group(group.clone(), depths, &active);
        // `.monaco-tl-row` starts at the rows' 4px margin, its guides 16px further (TreeGuides adds 3px to the inset).
        let rows = rows.into_iter().enumerate().map(|(index, (row, _, _))| div().relative().flex_none().child(row).children(guides.row(index, 4.)));
        div().flex_1().min_h_0().flex().flex_col().child(list.group(group).children(rows))
    }

    /// `renderMessage`: no problems at all, none in the active file, or none the filters let through with a Clear
    /// Filters link.
    fn problems_message(&self, total: usize, resources: &[ResourceMarkers], cx: &mut Context<Self>) -> Div {
        let message = div().pl(px(20.)).line_height(px(22.));
        let filtered_out = match self.problems.active_file {
            true => !resources.is_empty(),
            false => total > 0,
        };
        if !filtered_out {
            // A block, so the text wraps in a narrow view as the reference's does.
            let text = if self.problems.active_file { "No problems have been detected in the current file." } else { "No problems have been detected in the workspace." };
            return message.child(text);
        }
        let message = message.flex().flex_wrap();
        let link = div()
            .id("problems-clear-filters")
            .ml(px(4.))
            .cursor_pointer()
            .underline()
            .child("Clear Filters")
            .on_click(cx.listener(|workbench, _, window, cx| workbench.clear_problems_filters(window, cx)));
        message.child("No results found with provided filter criteria.").child(link).child(".")
    }

    /// `clearFilters`: the text, the excluded files and every severity back to their defaults.
    fn clear_problems_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = &mut self.problems;
        (view.excluded_files, view.show_errors, view.show_warnings, view.show_infos) = (false, true, true, true);
        self.edit_problems_filter(|input| input.set_value(String::new()), window, cx);
        self.problems_state_changed(cx);
    }

    /// views.css `.pane-body .monaco-tl-contents .highlight` with markers.css: bold on `list.filterMatchBackground`.
    fn filter_match(&self, text: &str, matches: &[std::ops::Range<usize>]) -> Div {
        let byte_at = |chars: usize| text.char_indices().nth(chars).map_or(text.len(), |(byte, _)| byte);
        let style = gpui::HighlightStyle { background_color: self.theme.color("list.filterMatchBackground").map(Into::into), font_weight: Some(gpui::FontWeight::BOLD), ..Default::default() };
        let highlights: Vec<_> = matches.iter().map(|range| (byte_at(range.start)..byte_at(range.end), style)).collect();
        div().min_w_0().truncate().child(gpui::StyledText::new(text.to_owned()).with_highlights(highlights))
    }

    /// `ResourceMarkersRenderer`: twistie, file icon, name, folder description and the marker count.
    fn problems_resource_row(&self, shown: &ShownResource, collapsed: bool, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let resource = shown.resource;
        let foreground = self.foreground_or("foreground");
        let icon = self.file_icons.as_ref().and_then(|icons| icons.file_icon(&resource.path, self.languages.language_of(&resource.path, ""), self.theme.kind, 13., ROW_HEIGHT, foreground));
        let name = resource.path.file_name().map_or_else(|| resource.resource.clone(), |name| name.to_string_lossy().into_owned());
        let workspace = self.workspace();
        let description = resource.path.parent().map(|parent| crate::quick_input::path_label(parent, workspace)).filter(|label| !label.is_empty());
        let key = resource.resource.clone();
        div()
            .id(gpui::SharedString::from(format!("problems-resource-{key}")))
            .h(px(ROW_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .mx(px(4.))
            .rounded(px(4.))
            .pl(px(11.))
            .cursor_pointer()
            .child(self.icon(if collapsed { "chevron-right" } else { "chevron-down" }, 16., 16., foreground).mr(px(2.)))
            .children(icon.map(|icon| icon.mr(px(6.))))
            .child(self.filter_match(&name, &shown.highlights.uri))
            .children(description.map(|description| div().ml(px(6.5)).text_size(px(11.7)).opacity(0.7).min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(description)))
            .child(
                div()
                    .ml(px(8.))
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
                    // `updateCount`: the visible markers.
                    .child(shown.markers.len().to_string()),
            )
            .on_click(cx.listener(move |workbench, _, _, cx| {
                workbench.select_problems_row(Some(Row::Resource(key.clone())), cx);
                if !workbench.problems.collapsed.remove(&key) {
                    workbench.problems.collapsed.insert(key.clone());
                }
                cx.notify();
            }))
    }

    /// `MarkerRenderer`: severity icon, the message (every line with multi-line on, the first otherwise),
    /// `source(code)` and `[Ln, Col]` at 70% opacity on the first line, the multi-line toggle for longer messages; a
    /// twistie before the icon when the marker has related information.
    fn problems_marker_row(&self, shown: &ShownMarker, related_collapsed: bool, tree_focused: bool, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let marker = shown.marker;
        let (icon, color) = match marker.severity {
            ERROR => ("error", self.color("problemsErrorIcon.foreground")),
            WARNING => ("warning", self.color("problemsWarningIcon.foreground")),
            INFO => ("info", self.color("problemsInfoIcon.foreground")),
            _ => ("lightbulb", self.foreground_or("foreground")),
        };
        let foreground = self.foreground_or("foreground");
        let key = (marker.resource.clone(), shown.index);
        let lines = marker_lines(&marker.message);
        let all_lines = self.problems.shows_all_lines(&key);
        let line_count = if all_lines { lines.len() } else { 1 };
        let line_matches = |index: usize| shown.highlights.lines.get(index).cloned().unwrap_or_default();
        let message = |index: usize| {
            let line = lines[index];
            let line = if line.chars().count() > 1000 { format!("{}...", line.chars().take(1000).collect::<String>()) } else { line.to_owned() };
            self.filter_match(&line, &line_matches(index))
        };
        let source = marker.source.clone().unwrap_or_default();
        let code = marker_code(marker);
        let mut details = div().flex().flex_none().items_center();
        if !source.is_empty() || code.is_some() {
            details = details.child(div().ml(px(6.)).opacity(0.7).child(self.filter_match(&source, &shown.highlights.source)));
            if let Some(code) = &code {
                let code = format!("({code})");
                let matches: Vec<_> = shown.highlights.code.iter().map(|range| range.start + 1..range.end + 1).collect();
                details = details.child(div().opacity(0.7).child(self.filter_match(&code, &matches)));
            }
        }
        details = details.child(div().ml(px(6.)).opacity(0.7).child(format!("[Ln {}, Col {}]", marker.start_line, marker.start_column)));
        let toggle_key = key.clone();
        let multiline_toggle = (lines.len() > 1).then(|| {
            self.icon(if all_lines { "chevron-up" } else { "chevron-down" }, 20., 16., foreground)
                .id(gpui::SharedString::from(format!("problems-multiline-{}-{}", key.0, key.1)))
                .ml(px(4.))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
                .on_click(cx.listener(move |workbench, _, _, cx| {
                    cx.stop_propagation();
                    if !workbench.problems.toggled_lines.remove(&toggle_key) {
                        workbench.problems.toggled_lines.insert(toggle_key.clone());
                    }
                    cx.notify();
                }))
        });
        let first = div().h(px(ROW_HEIGHT)).flex().items_center().min_w_0().child(message(0).flex_shrink_1()).child(details).children(multiline_toggle);
        let rest = (1..line_count).map(|index| div().h(px(ROW_HEIGHT)).flex().items_center().min_w_0().child(message(index)));
        let has_related = !related_information(marker).is_empty();
        let twistie_key = key.clone();
        let twistie = has_related.then(|| {
            self.icon(if related_collapsed { "chevron-right" } else { "chevron-down" }, 16., 16., foreground)
                .id(gpui::SharedString::from(format!("problems-twistie-{}-{}", key.0, key.1)))
                .absolute()
                .left(px(20.))
                .top(px(3.))
                .on_click(cx.listener(move |workbench, _, _, cx| {
                    cx.stop_propagation();
                    if !workbench.problems.collapsed_markers.remove(&twistie_key) {
                        workbench.problems.collapsed_markers.insert(twistie_key.clone());
                    }
                    cx.notify();
                }))
        });
        let target = json!({ "resource": crate::debug::uri_from_string(&marker.resource), "startLineNumber": marker.start_line, "startColumn": marker.start_column });
        let group = gpui::SharedString::from(format!("problems-marker-{}-{}", key.0, key.1));
        // markers.css: on a hovered row the light bulb takes the severity icon's place.
        let quick_fix = self.problems_quick_fix_icon(marker);
        let selected = self.problems.selected == Some(Row::Marker(key.0.clone(), key.1));
        // listWidget.ts: a selected row of the focused list draws its codicons in `list.activeSelectionIconForeground`;
        // unset, the CSS variable is invalid and the icon inherits the row's `list.activeSelectionForeground`.
        let selection_icon = self.theme.color("list.activeSelectionIconForeground").or_else(|| self.theme.color("list.activeSelectionForeground"));
        let color = selection_icon.filter(|_| selected && tree_focused).unwrap_or(color);
        let severity = self.icon(icon, 16., 16., color).mx(px(6.)).when(quick_fix.is_some(), |icon| icon.group_hover(group.clone(), |icon| icon.invisible()));
        // markers.css: the selected row shows its light bulb without the pointer over it.
        let severity = severity.when(selected && quick_fix.is_some(), |icon| icon.invisible());
        let light_bulb = quick_fix.map(|light_bulb| {
            let bounds = if selected { self.problems.selected_light_bulb.clone() } else { Rc::default() };
            self.icon(light_bulb, 20., 20., self.foreground_or("foreground"))
                .id("problems-quick-fix")
                .absolute()
                .left(px(4.))
                .top(px(1.))
                .rounded(px(5.))
                .cursor_pointer()
                .when(!selected, |icon| icon.invisible())
                .group_hover(group.clone(), |icon| icon.visible())
                .hover(|icon| icon.bg(self.color("toolbar.hoverBackground")))
                .child(super::output_view::bounds_tracker(bounds.clone()))
                .on_click(cx.listener(move |workbench, _, window, cx| {
                    cx.stop_propagation();
                    workbench.show_problems_quick_fixes(bounds.get(), window, cx);
                }))
        });
        let hovered = marker.clone();
        let selected_key = key.clone();
        div()
            .id(group.clone())
            .group(group)
            .relative()
            .flex_none()
            .flex()
            .mx(px(4.))
            .rounded(px(4.))
            .pl(px(38.))
            .pr(px(6.))
            .cursor_pointer()
            .whitespace_nowrap()
            .children(twistie)
            .child(div().relative().h(px(ROW_HEIGHT)).flex().items_center().child(severity).children(light_bulb))
            .on_hover(cx.listener(move |workbench, hovered_now: &bool, _, cx| {
                if *hovered_now {
                    workbench.problems_marker_hovered(&hovered, cx);
                }
            }))
            .child(div().flex_1().min_w_0().flex().flex_col().child(first).children(rest))
            .on_click(cx.listener(move |workbench, _, window, cx| {
                workbench.select_problems_row(Some(Row::Marker(selected_key.0.clone(), selected_key.1)), cx);
                workbench.open_marker_related(&target, window, cx);
            }))
    }

    /// `RelatedInformationRenderer`: the file name, `[Ln, Col]`, a colon and the message.
    fn problems_related_row(&self, marker: &(String, usize), index: usize, related: &Value, highlights: &Highlights, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let basename = related_basename(related);
        let message = related["message"].as_str().unwrap_or_default().to_owned();
        let target = related.clone();
        let row = Row::Related(marker.0.clone(), marker.1, index);
        div()
            .id(gpui::SharedString::from(format!("problems-related-{}-{}-{index}", marker.0, marker.1)))
            .h(px(ROW_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .mx(px(4.))
            .rounded(px(4.))
            .pl(px(66.))
            .pr(px(6.))
            .cursor_pointer()
            .whitespace_nowrap()
            .child(self.filter_match(&basename, &highlights.uri).flex_none())
            .child(div().ml(px(4.)).opacity(0.7).flex_none().child(format!("[Ln {}, Col {}]", related["startLineNumber"], related["startColumn"])))
            .child(div().opacity(0.7).pr(px(4.)).flex_none().child(":"))
            .child(self.filter_match(&message, &highlights.message))
            .on_click(cx.listener(move |workbench, _, window, cx| {
                workbench.select_problems_row(Some(row.clone()), cx);
                workbench.open_marker_related(&target, window, cx);
            }))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn marker(resource: &str, severity: u8, line: usize) -> Arc<Marker> {
        Arc::new(Marker::new("owner", resource, &json!({ "severity": severity, "message": "m", "startLineNumber": line, "startColumn": 1, "endLineNumber": line, "endColumn": 2 })).unwrap())
    }

    #[test]
    fn resources_sort_by_their_worst_marker_then_path_and_markers_by_severity_then_position() {
        let markers = vec![marker("file:///b.ts", WARNING, 3), marker("file:///a.ts", WARNING, 9), marker("file:///c.ts", ERROR, 5), marker("file:///a.ts", ERROR, 7), marker("file:///a.ts", WARNING, 2)];
        let resources = resource_markers(markers);
        let order: Vec<(&str, Vec<(u8, usize)>)> = resources.iter().map(|resource| (resource.resource.as_str(), resource.markers.iter().map(|marker| (marker.severity, marker.start_line)).collect())).collect();
        assert_eq!(order, [("file:///a.ts", vec![(ERROR, 7), (WARNING, 2), (WARNING, 9)]), ("file:///c.ts", vec![(ERROR, 5)]), ("file:///b.ts", vec![(WARNING, 3)])]);
    }

    #[test]
    fn the_filter_keeps_markers_whose_related_information_matches() {
        let related = json!([{ "resource": { "scheme": "file", "path": "/w/other.ts" }, "message": "declared here", "startLineNumber": 4, "startColumn": 2, "endLineNumber": 4, "endColumn": 5 }]);
        let with_related = Arc::new(Marker::new("owner", "file:///w/a.ts", &json!({ "severity": ERROR, "message": "m", "startLineNumber": 1, "startColumn": 1, "endLineNumber": 1, "endColumn": 2, "relatedInformation": related })).unwrap());
        let resources = resource_markers(vec![with_related, marker("file:///w/b.ts", WARNING, 3)]);
        let shown = |filter: &str| -> Vec<(String, usize, usize)> {
            let options = FilterOptions::new(filter, FilesExclude::ByRoot(Vec::new()), true, true, true);
            filter_resources(&resources, &options).iter().flat_map(|resource| resource.markers.iter().map(|marker| (marker.marker.resource.clone(), marker.index, marker.related.len()))).collect()
        };
        assert_eq!(shown(""), [("file:///w/a.ts".to_owned(), 0, 1), ("file:///w/b.ts".to_owned(), 0, 0)]);
        assert_eq!(shown("declared"), [("file:///w/a.ts".to_owned(), 0, 1)]);
        assert_eq!(shown("b.ts"), [("file:///w/b.ts".to_owned(), 0, 0)]);
        assert_eq!(shown("!**/a.ts"), [("file:///w/b.ts".to_owned(), 0, 0)]);
    }

    #[test]
    fn text_globs_and_severities_pick_the_shown_markers() {
        let marker = |message: &str, severity: u8, source: &str| Arc::new(Marker::new("owner", "file:///w/a.ts", &json!({ "severity": severity, "message": message, "source": source, "startLineNumber": 1, "startColumn": 1, "endLineNumber": 1, "endColumn": 2 })).unwrap());
        let resources = resource_markers(vec![marker("missing semicolon", ERROR, "ts"), marker("unused variable", WARNING, "eslint"), marker("hint", INFO, "ts")]);
        let shown = |options: FilterOptions| -> Vec<String> { filter_resources(&resources, &options).iter().flat_map(|resource| resource.markers.iter().map(|marker| marker.marker.message.clone())).collect() };
        let text = |filter: &str| shown(FilterOptions::new(filter, FilesExclude::ByRoot(Vec::new()), true, true, true));
        let all = ["missing semicolon", "unused variable", "hint"];
        assert_eq!(text(""), all);
        assert_eq!(text("unused"), ["unused variable"]);
        assert_eq!(text("!unused"), ["missing semicolon", "hint"]);
        assert!(text("nothing").is_empty());
        // A file name match or an include glob shows every marker of the file.
        assert_eq!(text("a.ts"), all);
        assert_eq!(text(".ts"), all);
        assert!(text("!.ts").is_empty());
        assert_eq!(text("@source:ts"), ["missing semicolon", "hint"]);
        assert_eq!(shown(FilterOptions::new("", FilesExclude::ByRoot(Vec::new()), false, true, false)), ["missing semicolon"]);
    }

    /// markersView.ts `focus`: opening the view selects the first marker; Up, Down and Enter move through the rows.
    #[gpui::test]
    fn focusing_the_view_selects_the_first_marker_and_keys_move_the_selection(cx: &mut gpui::TestAppContext) {
        use crate::workbench::goto_symbol::tests::{open, run};
        let (workbench, cx, _, _) = open(cx, "problems-selection", "one\ntwo\n");
        let resource = workbench.update(cx, |workbench, cx| {
            let resource = workbench.editors.active().unwrap().document().uri_string();
            let marker = |line: usize| json!({ "severity": ERROR, "message": "bad", "startLineNumber": line, "startColumn": 1, "endLineNumber": line, "endColumn": 2 });
            workbench.diagnostics.service.change_one("test", &resource, &[marker(1), marker(2)], false);
            cx.notify();
            resource
        });
        run(&workbench, cx, "workbench.actions.view.problems");
        let selected = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.problems.selected.clone());
        assert_eq!(selected(cx), Some(Row::Marker(resource.clone(), 0)));
        cx.simulate_keystrokes("down");
        assert_eq!(selected(cx), Some(Row::Marker(resource.clone(), 1)));
        run(&workbench, cx, "problems.action.copy");
        let copied = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
        assert!(copied.starts_with("[{\n\t\"resource\": \"") && copied.contains("\t\"startLineNumber\": 2,\n"), "{copied}");
        cx.simulate_keystrokes("up up");
        assert_eq!(selected(cx), Some(Row::Resource(resource.clone())));
        cx.simulate_keystrokes("enter");
        assert!(workbench.read_with(cx, |workbench, _| workbench.problems.collapsed.contains(&resource)), "Enter on a resource collapses it");
        cx.simulate_keystrokes("enter down");
        cx.simulate_keystrokes("down enter");
        cx.run_until_parked();
        let cursor = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().selection().head);
        assert_eq!(cursor, crate::editor::Position::new(1, 0), "Enter opens the selected marker");
    }
}
