//! The Problems view as a table (markersTable.ts): severity, Code, Message, File and Source columns over every shown
//! marker, sorted by severity, resource and position, with column sashes in a `SplitView` (table.ts).

use std::cell::Cell;
use std::rc::Rc;

use gpui::{AnyElement, Bounds, Context, Div, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Rgba, Stateful, Task, canvas, div, prelude::*, px};
use serde_json::json;

use super::Workbench;
use super::output_view::bounds_tracker;
use super::problems_view::ResourceMarkers;
use crate::markers_filter::{FilterOptions, TableMarker, marker_code};
use crate::split_view::{SashState, SplitView, View};

/// `MarkersTableVirtualDelegate.HEADER_ROW_HEIGHT` and `ROW_HEIGHT`.
const HEADER_HEIGHT: f32 = 24.;
const ROW_HEIGHT: f32 = 24.;
const SASH_SIZE: f32 = 4.;
/// The columns' weights, minimum and maximum widths; table.ts gives a column without them 120px to infinity.
const COLUMNS: [(&str, f32, f32, f32); 5] = [("", 0., 36., 36.), ("Code", 1., 100., 300.), ("Message", 4., 120., f32::INFINITY), ("File", 2., 120., f32::INFINITY), ("Source", 1., 100., 300.)];

pub struct ProblemsTable {
    columns: SplitView,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    hovered: bool,
    sash_hovered: Option<usize>,
    sash_hover: Option<Task<()>>,
}

impl Default for ProblemsTable {
    fn default() -> Self {
        let views = COLUMNS.iter().map(|(_, weight, minimum, maximum)| View::new(*weight, *minimum, *maximum)).collect();
        Self { columns: SplitView::new(COLUMNS.iter().map(|column| column.1).sum(), views), bounds: Rc::default(), hovered: false, sash_hovered: None, sash_hover: None }
    }
}

impl Workbench {
    /// The shown markers as table rows, each with its file's relative label.
    pub(super) fn problems_table_markers(&self, resources: &[&ResourceMarkers], options: &FilterOptions) -> Vec<(TableMarker, String)> {
        let workspace = self.workspace();
        let mut rows = Vec::new();
        for resource in resources {
            let path = resource.path.to_string_lossy();
            let label = crate::quick_input::path_label(&resource.path, workspace);
            rows.extend(resource.markers.iter().filter_map(|marker| options.filter_table_marker(&path, &label, marker)).map(|row| (row, label.clone())));
        }
        rows.sort_by(|(a, _), (b, _)| {
            let (a, b) = (&a.marker, &b.marker);
            b.severity.cmp(&a.severity).then_with(|| a.resource.cmp(&b.resource)).then((a.start_line, a.start_column, a.end_line, a.end_column).cmp(&(b.start_line, b.start_column, b.end_line, b.end_column)))
        });
        rows
    }

    /// The header with the column sashes over the scrolling rows.
    pub(super) fn problems_table(&mut self, rows: Vec<(TableMarker, String)>, list: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        let width = f32::from(self.problems.table.bounds.get().size.width);
        if width > 0. && self.problems.table.columns.size() != width {
            self.problems.table.columns.layout(width);
        }
        let columns: Vec<f32> = self.problems.table.columns.sizes();
        let border = if self.problems.table.hovered { self.theme.color("tree.tableColumnsBorder").unwrap_or(gpui::transparent_black().into()) } else { gpui::transparent_black().into() };
        let header = div()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .font_weight(FontWeight::SEMIBOLD)
            .children(COLUMNS.iter().zip(&columns).map(|((title, ..), width)| div().w(px(*width)).flex_none().pl(px(10.)).truncate().child(*title)));
        let rows: Vec<AnyElement> = rows.iter().enumerate().map(|(index, (row, label))| self.problems_table_row(index, row, label, &columns, cx)).collect();
        let mut sashes = Vec::new();
        let mut x = 0.;
        for (index, state) in self.problems.table.columns.sash_states().into_iter().enumerate() {
            x += columns[index];
            sashes.push(self.problems_column_sash(index, x, state, border, cx));
        }
        div()
            .id("problems-table")
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .text_size(px(13.))
            .on_hover(cx.listener(|workbench, hovered: &bool, _, cx| {
                workbench.problems.table.hovered = *hovered;
                cx.notify();
            }))
            .child(bounds_tracker(self.problems.table.bounds.clone()))
            .child(header)
            .child(list.children(rows))
            .children(sashes)
            .when(self.problems.table.columns.dragged_sash().is_some(), |table| table.child(self.problems_column_sash_drag(cx)))
    }

    /// A row: severity icon, `source(code)`, message, relative file with `[Ln, Col]`, source (markersTable.ts renderers).
    fn problems_table_row(&self, index: usize, row: &TableMarker, label: &str, columns: &[f32], cx: &mut Context<Self>) -> AnyElement {
        let marker = &row.marker;
        let (icon, color) = self.problems_severity_icon(marker.severity);
        let cell = |width: f32| div().w(px(width)).flex_none().h_full().flex().items_center().pl(px(10.)).overflow_hidden().whitespace_nowrap();
        let code = match (marker.source.as_deref(), marker_code(marker)) {
            (Some(source), Some(code)) => {
                let link = marker.code.get("target").is_some();
                let code_label = div().flex().child("(").child(div().when(link, |code| code.underline().text_color(self.color("textLink.foreground"))).child(self.problems_label(&code, &row.code_matches, true))).child(")");
                div().flex().min_w_0().child(self.problems_label(source, &row.source_matches, true)).child(code_label)
            }
            _ => div().child("-"),
        };
        let target = json!({ "resource": crate::debug::uri_from_string(&marker.resource), "startLineNumber": marker.start_line, "startColumn": marker.start_column });
        div()
            .id(("problems-table-row", index))
            .h(px(ROW_HEIGHT))
            .flex_none()
            .flex()
            // The modern list's rows sit 4px in from the view's edges, their cells keep the header's widths.
            .mx(px(4.))
            .rounded(px(4.))
            .overflow_hidden()
            .cursor_pointer()
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .child(cell(columns[0]).child(self.icon(icon, 16., 16., color)))
            .child(cell(columns[1]).child(code))
            .child(cell(columns[2]).child(self.problems_label(&marker.message, &row.message_matches, true)))
            .child(cell(columns[3]).child(div().flex_none().child(self.problems_label(label, &row.file_matches, true))).child(div().ml(px(6.)).opacity(0.7).min_w_0().truncate().child(format!("[Ln {}, Col {}]", marker.start_line, marker.start_column))))
            .child(cell(columns[4]).child(self.problems_label(marker.source.as_deref().unwrap_or_default(), &row.source_matches, true)))
            .on_click(cx.listener(move |workbench, _, window, cx| workbench.open_marker_related(&target, window, cx)))
            .into_any_element()
    }

    /// The sash after column `index` (table.css): the column border below the header, `sash.hoverBorder` once
    /// hovered past the hover delay or dragged.
    fn problems_column_sash(&self, index: usize, x: f32, state: SashState, border: Rgba, cx: &mut Context<Self>) -> AnyElement {
        let table = &self.problems.table;
        let active = table.sash_hovered == Some(index) || table.columns.dragged_sash() == Some(index);
        let hover_border = self.theme.color("sash.hoverBorder").unwrap_or(self.color("focusBorder"));
        let line = |color: Rgba| div().absolute().left(px(SASH_SIZE / 2.)).top(px(HEADER_HEIGHT)).bottom_0().w(px(1.)).bg(color);
        let sash = div().id(("problems-column-sash", index)).absolute().top_0().bottom_0().left(px(x - SASH_SIZE / 2.)).w(px(SASH_SIZE)).child(line(border)).when(active, |sash| sash.child(line(hover_border)));
        if state == SashState::Disabled {
            return sash.into_any_element();
        }
        sash.cursor(match state {
            SashState::AtMinimum => gpui::CursorStyle::ResizeRight,
            SashState::AtMaximum => gpui::CursorStyle::ResizeLeft,
            _ => gpui::CursorStyle::ResizeColumn,
        })
        .on_hover(cx.listener(move |workbench, hovered: &bool, window, cx| {
            let delay = workbench.sash_hover_delay();
            let table = &mut workbench.problems.table;
            table.sash_hovered = None;
            table.sash_hover = hovered.then(|| {
                cx.spawn_in(window, async move |workbench, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = workbench.update(cx, |workbench, cx| {
                        workbench.problems.table.sash_hovered = Some(index);
                        cx.notify();
                    });
                })
            });
            cx.notify();
        }))
        .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, _, cx| {
            cx.stop_propagation();
            let columns = &mut workbench.problems.table.columns;
            if event.click_count == 2 {
                // table.ts `onDidSashReset`: the column back to its weight's share.
                let width = columns.size();
                columns.resize_view(index, COLUMNS[index].1 / COLUMNS.iter().map(|column| column.1).sum::<f32>() * width);
            } else {
                columns.start_sash_drag(index, f32::from(event.position.x), event.modifiers.alt);
            }
            cx.notify();
        }))
        .into_any_element()
    }

    /// Follows the pointer anywhere in the window while a sash drags.
    fn problems_column_sash_drag(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let workbench = cx.entity();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
                    moved.update(cx, |workbench, cx| {
                        workbench.problems.table.columns.drag_sash(f32::from(event.position.x), event.modifiers.alt);
                        cx.notify();
                    });
                });
                let released = workbench.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                    released.update(cx, |workbench, cx| {
                        workbench.problems.table.columns.end_sash_drag();
                        cx.notify();
                    });
                });
            },
        )
        .size_0()
    }
}
