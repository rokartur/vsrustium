//! The Problems view (markersView.ts, markersTreeViewer.ts, markersModel.ts): the markers of every resource as a
//! tree, a resource row with its file icon, folder and count over its markers, each with its severity icon, message,
//! source, code and position. A click opens the marker's location.
// shortcut: no filter box, View as Table, multi-line messages, related information rows, quick fix actions, indent
// guides or the focused first row on open.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{Context, Div, Window, div, prelude::*, px};
use serde_json::json;

use super::Workbench;
use super::panel::PanelView;
use crate::commands::Outcome;
use crate::markers::{ERROR, INFO, Marker, ReadOptions, WARNING};

/// markers.ts `Markers.MARKERS_CONTAINER_ID` and `MARKERS_VIEW_ID`.
pub const MARKERS_CONTAINER_ID: &str = "workbench.panel.markers";
const MARKERS_VIEW_ID: &str = "workbench.panel.markers.view";
const ROW_HEIGHT: f32 = 22.;

#[derive(Default)]
pub struct ProblemsView {
    /// Resources whose markers are hidden, by `URI.toString()`.
    pub collapsed: HashSet<String>,
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
            title: "Problems",
            render: |workbench, window, cx| workbench.problems_body(window, cx).into_any_element(),
            actions: |workbench, _, cx| div().flex().child(workbench.search_action("collapse-all", "workbench.actions.treeView.workbench.panel.markers.view.collapseAll", true, cx)).into_any_element(),
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

    /// The `${MARKERS_VIEW_ID}.active` key: the view is registered and its `when` holds.
    pub(super) fn problems_context(&self, context: &mut std::collections::HashMap<String, serde_json::Value>) {
        context.insert(format!("{MARKERS_VIEW_ID}.active"), json!(true));
    }

    fn problems_body(&self, _window: &Window, cx: &mut Context<Self>) -> Div {
        let resources = resource_markers(self.diagnostics.service.read(ReadOptions::default()));
        let list = div().id("problems-tree").track_focus(&self.problems_focus).flex_1().min_h_0().overflow_y_scroll().flex().flex_col().text_size(px(13.));
        if resources.is_empty() {
            // markersView.ts `renderMessage`: the message area at the top of the view.
            return div().flex_1().min_h_0().flex().flex_col().child(list.child(div().px(px(20.)).py(px(5.)).child("No problems have been detected in the workspace.")));
        }
        let mut rows = Vec::new();
        for resource in &resources {
            let collapsed = self.problems.collapsed.contains(&resource.resource);
            rows.push(self.problems_resource_row(resource, collapsed, cx));
            if !collapsed {
                rows.extend(resource.markers.iter().enumerate().map(|(index, marker)| self.problems_marker_row(marker, index, cx)));
            }
        }
        div().flex_1().min_h_0().flex().flex_col().child(list.children(rows))
    }

    /// `ResourceMarkersRenderer`: twistie, file icon, name, folder description and the marker count.
    fn problems_resource_row(&self, resource: &ResourceMarkers, collapsed: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
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
            .pl(px(15.))
            .cursor_pointer()
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .child(self.icon(if collapsed { "chevron-right" } else { "chevron-down" }, 16., 16., foreground).mr(px(2.)))
            .children(icon.map(|icon| icon.mr(px(6.))))
            .child(div().flex_none().child(name))
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
                    .child(resource.markers.len().to_string()),
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
    fn problems_marker_row(&self, marker: &Arc<Marker>, index: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (icon, color) = match marker.severity {
            ERROR => ("error", self.color("problemsErrorIcon.foreground")),
            WARNING => ("warning", self.color("problemsWarningIcon.foreground")),
            INFO => ("info", self.color("problemsInfoIcon.foreground")),
            _ => ("lightbulb", self.foreground_or("foreground")),
        };
        let code = match &marker.code {
            serde_json::Value::String(code) => Some(code.clone()),
            serde_json::Value::Object(code) => code.get("value").and_then(serde_json::Value::as_str).map(str::to_string),
            _ => None,
        };
        let detail = |text: String| div().ml(px(6.)).opacity(0.7).flex_none().child(text);
        let first_line = marker.message.lines().next().unwrap_or_default().to_string();
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
            .child(div().min_w_0().overflow_hidden().text_ellipsis().child(first_line))
            .when(marker.source.is_some() || code.is_some(), |row| {
                row.child(detail(format!("{}{}", marker.source.clone().unwrap_or_default(), code.map(|code| format!("({code})")).unwrap_or_default())))
            })
            .child(detail(format!("[Ln {}, Col {}]", marker.start_line, marker.start_column)))
            .on_click(cx.listener(move |workbench, _, window, cx| workbench.open_marker_related(&target, window, cx)))
            .into_any_element()
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
