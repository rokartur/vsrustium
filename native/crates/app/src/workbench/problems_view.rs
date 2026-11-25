//! The Problems view (markersView.ts, markersTreeViewer.ts, markersModel.ts): the markers of every resource as a
//! tree, a resource row with its file icon, folder and count over its markers, each with its severity icon, message,
//! source, code and position. A click opens the marker's location. The filter is in problems_filter.rs, the table
//! in problems_table.rs.
// Not ported yet: multi-line messages, related information rows, quick fix actions, indent guides, row focus and
// selection (the first row focused on open).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{Context, Div, Window, div, prelude::*, px};
use serde_json::json;

use super::Workbench;
use super::panel::PanelView;
use super::problems_filter::{ProblemsFilter, ViewMode};
use super::problems_table::ProblemsTable;
use crate::commands::Outcome;
use crate::markers::{ERROR, INFO, Marker, ReadOptions, WARNING};
use crate::markers_filter::{FilterOptions, FilteredMarker, FilteredResource, marker_code};

/// markers.ts `Markers.MARKERS_CONTAINER_ID` and `MARKERS_VIEW_ID`.
pub const MARKERS_CONTAINER_ID: &str = "workbench.panel.markers";
pub(super) const MARKERS_VIEW_ID: &str = "workbench.panel.markers.view";
const ROW_HEIGHT: f32 = 22.;

pub struct ProblemsView {
    /// Resources whose markers are hidden, by `URI.toString()`.
    pub collapsed: HashSet<String>,
    pub filter: ProblemsFilter,
    pub table: ProblemsTable,
}

impl ProblemsView {
    pub fn new(cx: &mut gpui::App) -> Self {
        Self { collapsed: HashSet::new(), filter: ProblemsFilter::new(cx), table: ProblemsTable::default() }
    }
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

fn resource_name(resource: &ResourceMarkers) -> String {
    resource.path.file_name().map_or_else(|| resource.resource.clone(), |name| name.to_string_lossy().into_owned())
}

impl Workbench {
    /// markers.contribution.ts registers the Problems container in the panel at order 0.
    pub(super) fn register_problems_panel_view(&mut self) {
        self.restore_problems_filter();
        self.register_problems_filter_actions();
        self.register_panel_view(PanelView {
            id: MARKERS_CONTAINER_ID,
            view_id: MARKERS_VIEW_ID,
            title: "Problems",
            open: |workbench, window, cx| {
                workbench.open_panel_view(MARKERS_CONTAINER_ID, cx);
                window.focus(&workbench.problems_focus, cx);
            },
            render: |workbench, _, cx| workbench.problems_body(cx).into_any_element(),
            actions: |workbench, window, cx| workbench.problems_title_actions(window, cx).into_any_element(),
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
                    window.focus(&workbench.problems_focus, cx);
                }
            }
            Outcome::done()
        });
        self.commands.register("workbench.actions.view.toggleProblems", |workbench, _, window, cx| {
            match workbench.panel_view_visible(MARKERS_CONTAINER_ID) {
                true => workbench.close_panel_view(MARKERS_CONTAINER_ID, cx),
                false => {
                    workbench.open_panel_view(MARKERS_CONTAINER_ID, cx);
                    window.focus(&workbench.problems_focus, cx);
                }
            }
            Outcome::done()
        });
        self.commands.register("workbench.actions.treeView.workbench.panel.markers.view.collapseAll", |workbench, _, _, cx| {
            workbench.problems.collapsed = workbench.diagnostics.service.read(ReadOptions::default()).iter().map(|marker| marker.resource.clone()).collect();
            cx.notify();
            Outcome::done()
        });
    }

    /// The `${MARKERS_VIEW_ID}.active` key (the view is registered and its `when` holds), `focusedView` while
    /// the view's content has focus, and the filter's keys.
    pub(super) fn problems_context(&self, window: &Window, context: &mut std::collections::HashMap<String, serde_json::Value>) {
        context.insert(format!("{MARKERS_VIEW_ID}.active"), json!(true));
        if self.problems_focus.is_focused(window) {
            context.insert("focusedView".into(), json!(MARKERS_VIEW_ID));
        }
        self.problems_filter_context(window, context);
    }

    /// markersView.ts `getResourceMarkers`: only the active editor's resource with Show Active File Only.
    fn problems_shown_resources<'a>(&self, all: &'a [ResourceMarkers]) -> Vec<&'a ResourceMarkers> {
        let active = self.editors.active().map(|editor| crate::editor::uri_string(&editor.document().scheme, &editor.document().path));
        all.iter().filter(|resource| !self.problems.filter.active_file || active.as_ref() == Some(&resource.resource)).collect()
    }

    /// markersView.ts `getFilterStats`: the shown markers and all markers.
    pub(super) fn problems_filter_stats(&self) -> (usize, usize) {
        let all = resource_markers(self.diagnostics.service.read(ReadOptions::default()));
        let total = all.iter().map(|resource| resource.markers.len()).sum();
        let resources = self.problems_shown_resources(&all);
        let options = self.problems_filter_options();
        let filtered = match self.problems.filter.view_mode {
            ViewMode::Tree => resources.iter().filter_map(|resource| options.filter_resource(&resource.path.to_string_lossy(), &resource_name(resource), &resource.markers)).map(|shown| shown.markers.len()).sum(),
            ViewMode::Table => self.problems_table_markers(&resources, &options).len(),
        };
        (filtered, total)
    }

    fn problems_body(&mut self, cx: &mut Context<Self>) -> Div {
        let all = resource_markers(self.diagnostics.service.read(ReadOptions::default()));
        let total: usize = all.iter().map(|resource| resource.markers.len()).sum();
        let active_file = self.problems.filter.active_file;
        let resources = self.problems_shown_resources(&all);
        let options = self.problems_filter_options();
        let list = div().id("problems-tree").track_focus(&self.problems_focus).flex_1().min_h_0().overflow_y_scroll().flex().flex_col().text_size(px(13.));
        match self.problems.filter.view_mode {
            ViewMode::Tree => {
                let (rows, filtered) = self.problems_tree_rows(&resources, &options, cx);
                if filtered > 0 {
                    return div().flex_1().min_h_0().flex().flex_col().child(list.children(rows));
                }
            }
            ViewMode::Table => {
                let rows = self.problems_table_markers(&resources, &options);
                if !rows.is_empty() {
                    return div().flex_1().min_h_0().flex().flex_col().child(self.problems_table(rows, list, cx));
                }
            }
        }
        // markersView.ts `renderMessage`: the message area at the top of the view, the widget hidden.
        let message = div().px(px(20.)).py(px(5.)).flex();
        let message = if active_file && !resources.is_empty() || !active_file && total > 0 {
            let link = self.color("textLink.foreground");
            message.child("No results found with provided filter criteria.").child(
                div().id("problems-clear-filters").ml(px(4.)).underline().cursor_pointer().text_color(link).child("Clear Filters").on_click(cx.listener(|workbench, _, window, cx| workbench.clear_problems_filters(window, cx))),
            ).child(".")
        } else if active_file {
            message.child("No problems have been detected in the current file.")
        } else {
            message.child("No problems have been detected in the workspace.")
        };
        div().flex_1().min_h_0().flex().flex_col().child(list.child(message))
    }

    /// The tree's rows and its visible marker count (`MarkersTree.getVisibleItemCount`).
    fn problems_tree_rows(&self, resources: &[&ResourceMarkers], options: &FilterOptions, cx: &mut Context<Self>) -> (Vec<gpui::AnyElement>, usize) {
        let mut rows = Vec::new();
        let mut filtered = 0;
        for resource in resources {
            let name = resource_name(resource);
            let Some(shown) = options.filter_resource(&resource.path.to_string_lossy(), &name, &resource.markers) else { continue };
            filtered += shown.markers.len();
            let collapsed = self.problems.collapsed.contains(&resource.resource);
            rows.push(self.problems_resource_row(resource, &name, &shown, collapsed, cx));
            if !collapsed {
                rows.extend(shown.markers.iter().enumerate().map(|(index, marker)| self.problems_marker_row(marker, index, cx)));
            }
        }
        (rows, filtered)
    }

    /// `HighlightedLabel`: the text with its matches bold in `list.highlightForeground`.
    /// Matches are bold (markers.css); in the tree on `list.filterMatchBackground` (views.css `.pane-body
    /// .monaco-tl-contents`), in the table in `list.highlightForeground` (style.css).
    pub(super) fn problems_label(&self, text: &str, matches: &[std::ops::Range<usize>], table: bool) -> gpui::AnyElement {
        let label = div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap();
        if matches.is_empty() {
            return label.child(text.to_string()).into_any_element();
        }
        let mut style = gpui::HighlightStyle { font_weight: Some(gpui::FontWeight::BOLD), ..Default::default() };
        match table {
            true => style.color = Some(self.color("list.highlightForeground").into()),
            false => style.background_color = self.theme.color("list.filterMatchBackground").map(Into::into),
        }
        // Matches count chars; StyledText takes byte ranges.
        let byte = |char_index: usize| text.char_indices().nth(char_index).map_or(text.len(), |(byte, _)| byte);
        let highlights: Vec<_> = matches.iter().filter(|range| !range.is_empty()).map(|range| (byte(range.start)..byte(range.end), style)).collect();
        label.child(gpui::StyledText::new(text.to_string()).with_highlights(highlights)).into_any_element()
    }

    /// `ResourceMarkersRenderer`: twistie, file icon, name, folder description and the marker count.
    fn problems_resource_row(&self, resource: &ResourceMarkers, name: &str, shown: &FilteredResource, collapsed: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let foreground = self.foreground_or("foreground");
        let icon = self.file_icons.as_ref().and_then(|icons| icons.file_icon(&resource.path, self.languages.language_of(&resource.path, ""), self.theme.kind, 13., ROW_HEIGHT, foreground));
        let workspace = self.workspace();
        let description = resource.path.parent().map(|parent| crate::quick_input::path_label(parent, workspace)).filter(|label| !label.is_empty());
        let key = resource.resource.clone();
        div()
            .id(gpui::SharedString::from(format!("problems-resource-{key}")))
            .h(px(ROW_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .pl(px(15.))
            .cursor_pointer()
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .child(self.icon(if collapsed { "chevron-right" } else { "chevron-down" }, 16., 16., foreground).mr(px(2.)))
            .children(icon.map(|icon| icon.mr(px(6.))))
            .child(div().flex_none().child(self.problems_label(name, &shown.uri_matches, false)))
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
                    .child(shown.markers.len().to_string()),
            )
            .on_click(cx.listener(move |workbench, _, _, cx| {
                if !workbench.problems.collapsed.remove(&key) {
                    workbench.problems.collapsed.insert(key.clone());
                }
                cx.notify();
            }))
            .into_any_element()
    }

    /// `MarkerRenderer`: severity icon, the message's first line, `source(code)` and `[Ln, Col]` at 70% opacity.
    fn problems_marker_row(&self, shown: &FilteredMarker, index: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let marker = &shown.marker;
        let (icon, color) = self.problems_severity_icon(marker.severity);
        let code = marker_code(marker);
        let detail = || div().ml(px(6.)).opacity(0.7).flex_none().flex();
        let first_line = marker.message.split('\n').next().unwrap_or_default();
        let first_matches = shown.line_matches.first().map(Vec::as_slice).unwrap_or_default();
        let target = json!({ "resource": crate::debug::uri_from_string(&marker.resource), "startLineNumber": marker.start_line, "startColumn": marker.start_column });
        div()
            .id(gpui::SharedString::from(format!("problems-marker-{}-{index}", marker.resource)))
            .h(px(ROW_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .pl(px(42.))
            .cursor_pointer()
            .whitespace_nowrap()
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .child(self.icon(icon, 16., 16., color).mx(px(6.)))
            .child(self.problems_label(first_line, first_matches, false))
            .when(marker.source.is_some() || code.is_some(), |row| {
                let source = marker.source.as_deref().map(|source| self.problems_label(source, &shown.source_matches, false));
                let code = code.map(|code| div().flex().child("(").child(self.problems_label(&code, &shown.code_matches, false)).child(")"));
                row.child(detail().children(source).children(code))
            })
            .child(detail().child(format!("[Ln {}, Col {}]", marker.start_line, marker.start_column)))
            .on_click(cx.listener(move |workbench, _, window, cx| workbench.open_marker_related(&target, window, cx)))
            .into_any_element()
    }

    pub(super) fn problems_severity_icon(&self, severity: u8) -> (&'static str, gpui::Rgba) {
        match severity {
            ERROR => ("error", self.color("problemsErrorIcon.foreground")),
            WARNING => ("warning", self.color("problemsWarningIcon.foreground")),
            INFO => ("info", self.color("problemsInfoIcon.foreground")),
            _ => ("lightbulb", self.foreground_or("foreground")),
        }
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
}
