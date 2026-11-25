//! markersTable.ts: the Problems view as a table of every marker the filter keeps, with severity, Code, Message,
//! File and Source columns sorted by severity, resource and position, and column sashes in a `SplitView` (table.ts).

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{AnyElement, Bounds, Context, Div, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Rgba, Stateful, Task, canvas, div, prelude::*, px};
use serde_json::json;

use super::Workbench;
use super::keybindings_editor_view::highlighted;
use super::output_view::bounds_tracker;
use crate::filters::{Matches, matches_fuzzy};
use crate::markers::{ERROR, INFO, Marker, WARNING};
use crate::markers_filter::{FilterOptions, marker_code, matches_fuzzy2};
use crate::split_view::{SashState, SplitView, View};

/// `MarkersTableVirtualDelegate.HEADER_ROW_HEIGHT` and `ROW_HEIGHT`.
const HEADER_HEIGHT: f32 = 24.;
const ROW_HEIGHT: f32 = 24.;
const SASH_SIZE: f32 = 4.;
/// The columns' weights, minimum and maximum widths; table.ts gives a column without them 120px to infinity.
const COLUMNS: [(&str, f32, f32, f32); 5] = [("", 0., 36., 36.), ("Code", 1., 100., 300.), ("Message", 4., 120., f32::INFINITY), ("File", 2., 120., f32::INFINITY), ("Source", 1., 100., 300.)];

/// `MarkerTableItem`: a marker with the filter text's matches in its columns.
pub struct TableItem {
    pub marker: Arc<Marker>,
    pub file: String,
    pub source: Matches,
    pub code: Matches,
    pub message: Matches,
    pub file_matches: Matches,
}

/// `MarkersTable.reset`: globs, severity, source and text filters on each marker; a negated text keeps the markers
/// it does not match. Sorted by severity, resource, then range.
pub fn table_items(markers: &[Arc<Marker>], options: &FilterOptions, path_of: impl Fn(&Marker) -> String, file_label: impl Fn(&Marker) -> String) -> Vec<TableItem> {
    let mut items = Vec::new();
    for marker in markers {
        let path = path_of(marker);
        let item = |source, code, message, file_matches| TableItem { marker: marker.clone(), file: file_label(marker), source, code, message, file_matches };
        if options.excludes.matches(&path) {
            continue;
        }
        if options.includes.matches(&path) {
            items.push(item(vec![], vec![], vec![], vec![]));
            continue;
        }
        let severity = match marker.severity {
            ERROR => options.show_errors,
            WARNING => options.show_warnings,
            INFO => options.show_infos,
            _ => false,
        };
        if !severity || !options.matches_source_filters(marker.source.as_deref()) {
            continue;
        }
        if options.text.is_empty() {
            items.push(item(vec![], vec![], vec![], vec![]));
            continue;
        }
        let source = marker.source.as_deref().and_then(|source| matches_fuzzy2(&options.text, source));
        let code = marker_code(marker).and_then(|code| matches_fuzzy2(&options.text, &code));
        let message = matches_fuzzy(&options.text, &marker.message);
        let file = matches_fuzzy(&options.text, &file_label(marker));
        let matched = source.is_some() || code.is_some() || message.is_some() || file.is_some();
        if matched != options.negate {
            items.push(item(source.unwrap_or_default(), code.unwrap_or_default(), message.unwrap_or_default(), file.unwrap_or_default()));
        }
    }
    items.sort_by(|a, b| {
        let (a, b) = (&a.marker, &b.marker);
        b.severity.cmp(&a.severity).then_with(|| a.resource.cmp(&b.resource)).then((a.start_line, a.start_column, a.end_line, a.end_column).cmp(&(b.start_line, b.start_column, b.end_line, b.end_column)))
    });
    items
}

/// The table's column layout and sash hover state.
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
    /// The header with the column sashes over the scrolling rows.
    pub(super) fn problems_table(&mut self, items: &[TableItem], list: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        let width = f32::from(self.problems.table_view.bounds.get().size.width);
        if width > 0. && self.problems.table_view.columns.size() != width {
            self.problems.table_view.columns.layout(width);
        }
        let columns: Vec<f32> = self.problems.table_view.columns.sizes();
        let border = if self.problems.table_view.hovered { self.theme.color("tree.tableColumnsBorder").unwrap_or(gpui::transparent_black().into()) } else { gpui::transparent_black().into() };
        let header = div()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .font_weight(FontWeight::SEMIBOLD)
            .children(COLUMNS.iter().zip(&columns).map(|((title, ..), width)| div().w(px(*width)).flex_none().pl(px(10.)).truncate().child(*title)));
        let rows: Vec<AnyElement> = items.iter().enumerate().map(|(index, item)| self.problems_table_row(index, item, &columns, cx)).collect();
        let mut sashes = Vec::new();
        let mut x = 0.;
        for (index, state) in self.problems.table_view.columns.sash_states().into_iter().enumerate() {
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
                workbench.problems.table_view.hovered = *hovered;
                cx.notify();
            }))
            .child(bounds_tracker(self.problems.table_view.bounds.clone()))
            .child(header)
            .child(list.children(rows))
            .children(sashes)
            .when(self.problems.table_view.columns.dragged_sash().is_some(), |table| table.child(self.problems_column_sash_drag(cx)))
    }

    /// A row: severity icon, `source(code)` (else "-"), message, relative file with `[Ln, Col]`, source.
    fn problems_table_row(&self, index: usize, item: &TableItem, columns: &[f32], cx: &mut Context<Self>) -> AnyElement {
        let marker = &item.marker;
        let highlight = self.color("list.highlightForeground");
        let (icon, color) = match marker.severity {
            ERROR => ("error", self.color("problemsErrorIcon.foreground")),
            WARNING => ("warning", self.color("problemsWarningIcon.foreground")),
            INFO => ("info", self.color("problemsInfoIcon.foreground")),
            _ => ("lightbulb", self.foreground_or("foreground")),
        };
        let cell = |width: f32| div().w(px(width)).flex_none().h_full().flex().items_center().pl(px(10.)).overflow_hidden().whitespace_nowrap();
        let code = match (marker.source.as_deref(), marker_code(marker)) {
            (Some(source), Some(code)) => {
                let link = marker.code.get("target").is_some();
                let code_label = div().flex().child("(").child(div().when(link, |code| code.underline().text_color(self.color("textLink.foreground"))).child(highlighted(&code, &item.code, highlight))).child(")");
                div().flex().min_w_0().child(highlighted(source, &item.source, highlight).flex_none()).child(code_label)
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
            .child(cell(columns[2]).child(highlighted(&marker.message, &item.message, highlight)))
            .child(cell(columns[3]).child(highlighted(&item.file, &item.file_matches, highlight).flex_none()).child(div().ml(px(6.)).opacity(0.7).min_w_0().truncate().child(format!("[Ln {}, Col {}]", marker.start_line, marker.start_column))))
            .child(cell(columns[4]).child(highlighted(marker.source.as_deref().unwrap_or_default(), &item.source, highlight)))
            .on_click(cx.listener(move |workbench, _, window, cx| workbench.open_marker_related(&target, window, cx)))
            .into_any_element()
    }

    /// The sash after column `index` (table.css): the column border below the header, `sash.hoverBorder` once
    /// hovered past the hover delay or dragged.
    fn problems_column_sash(&self, index: usize, x: f32, state: SashState, border: Rgba, cx: &mut Context<Self>) -> AnyElement {
        let table = &self.problems.table_view;
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
            let table = &mut workbench.problems.table_view;
            table.sash_hovered = None;
            table.sash_hover = hovered.then(|| {
                cx.spawn_in(window, async move |workbench, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = workbench.update(cx, |workbench, cx| {
                        workbench.problems.table_view.sash_hovered = Some(index);
                        cx.notify();
                    });
                })
            });
            cx.notify();
        }))
        .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, _, cx| {
            cx.stop_propagation();
            let columns = &mut workbench.problems.table_view.columns;
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
                        workbench.problems.table_view.columns.drag_sash(f32::from(event.position.x), event.modifiers.alt);
                        cx.notify();
                    });
                });
                let released = workbench.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                    released.update(cx, |workbench, cx| {
                        workbench.problems.table_view.columns.end_sash_drag();
                        cx.notify();
                    });
                });
            },
        )
        .size_0()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::markers_filter::FilesExclude;

    fn marker(resource: &str, severity: u8, message: &str, line: usize) -> Arc<Marker> {
        Arc::new(Marker::new("owner", resource, &json!({ "severity": severity, "message": message, "source": "ts", "startLineNumber": line, "startColumn": 1, "endLineNumber": line, "endColumn": 2 })).unwrap())
    }

    fn rows(markers: &[Arc<Marker>], filter: &str) -> Vec<(String, usize)> {
        let options = FilterOptions::new(filter, FilesExclude::ByRoot(Vec::new()), true, true, true);
        let path = |marker: &Marker| marker.resource.trim_start_matches("file://").to_owned();
        let file = |marker: &Marker| marker.resource.rsplit('/').next().unwrap().to_owned();
        table_items(markers, &options, path, file).iter().map(|item| (item.file.clone(), item.marker.start_line)).collect()
    }

    #[test]
    fn rows_sort_by_severity_then_resource_then_position_and_text_matches_the_file_too() {
        let markers = [marker("file:///w/b.ts", WARNING, "unused", 1), marker("file:///w/b.ts", ERROR, "missing", 4), marker("file:///w/a.ts", ERROR, "missing", 9), marker("file:///w/a.ts", ERROR, "missing", 2)];
        assert_eq!(rows(&markers, ""), [("a.ts".to_owned(), 2), ("a.ts".to_owned(), 9), ("b.ts".to_owned(), 4), ("b.ts".to_owned(), 1)]);
        assert_eq!(rows(&markers, "unused"), [("b.ts".to_owned(), 1)]);
        assert_eq!(rows(&markers, "!missing"), [("b.ts".to_owned(), 1)]);
        assert_eq!(rows(&markers, "a.ts").len(), 2);
    }
}
