//! markersTable.ts: the Problems view as a table of every marker the filter keeps, with severity, code, message,
//! file and source columns.
// shortcut: the columns keep their weighted widths, without the header sashes that resize them.

use std::sync::Arc;

use gpui::{AnyElement, Context, Div, Styled, div, prelude::*, px};
use serde_json::json;

use super::Workbench;
use super::keybindings_editor_view::highlighted;
use crate::filters::{Matches, matches_fuzzy};
use crate::markers::{ERROR, INFO, Marker, WARNING};
use crate::markers_filter::{FilterOptions, marker_code, matches_fuzzy2};

/// `MarkersTableVirtualDelegate.ROW_HEIGHT` and `HEADER_ROW_HEIGHT`.
const ROW_HEIGHT: f32 = 24.;

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

impl Workbench {
    /// The header (600 weight, 10px left padding) and a 24px row per item. Column weights: Code 1 (100 to 300px),
    /// Message 4, File 2, Source 1 (100 to 300px) after the 36px severity column.
    pub(super) fn problems_table(&self, items: &[TableItem], cx: &mut Context<Self>) -> Vec<AnyElement> {
        let header = self.problems_table_row(div().font_weight(gpui::FontWeight::SEMIBOLD), [div(), div().child("Code"), div().child("Message"), div().child("File"), div().child("Source")]);
        let mut rows = vec![header.into_any_element()];
        let highlight = self.color("list.highlightForeground");
        for (index, item) in items.iter().enumerate() {
            let marker = &item.marker;
            let (icon, color) = match marker.severity {
                ERROR => ("error", self.color("problemsErrorIcon.foreground")),
                WARNING => ("warning", self.color("problemsWarningIcon.foreground")),
                INFO => ("info", self.color("problemsInfoIcon.foreground")),
                _ => ("lightbulb", self.foreground_or("foreground")),
            };
            // `MarkerCodeColumnRenderer`: `source(code)` when both are set, else "-".
            let code = match (&marker.source, marker_code(marker)) {
                (Some(source), Some(code)) => div()
                    .flex()
                    .child(highlighted(source, &item.source, highlight).flex_none())
                    .child("(")
                    .child(highlighted(&code, &item.code, highlight).flex_none())
                    .child(")"),
                _ => div().child("-"),
            };
            let position = format!("[Ln {}, Col {}]", marker.start_line, marker.start_column);
            let file = div().flex().min_w_0().child(highlighted(&item.file, &item.file_matches, highlight)).child(div().ml(px(6.)).opacity(0.7).flex_none().child(position));
            let source = highlighted(marker.source.as_deref().unwrap_or_default(), &item.source, highlight);
            let cells = [div().child(self.icon(icon, 16., 16., color)), code, highlighted(&marker.message, &item.message, highlight), file, source];
            let target = json!({ "resource": crate::debug::uri_from_string(&marker.resource), "startLineNumber": marker.start_line, "startColumn": marker.start_column });
            let row = self
                .problems_table_row(div(), cells)
                .id(("problems-table-row", index))
                .cursor_pointer()
                .hover(|row| row.bg(self.color("list.hoverBackground")))
                .on_click(cx.listener(move |workbench, _, window, cx| workbench.open_marker_related(&target, window, cx)));
            rows.push(row.into_any_element());
        }
        rows
    }

    fn problems_table_row(&self, row: Div, cells: [Div; 5]) -> Div {
        let [severity, code, message, file, source] = cells;
        let cell = |content: Div, weight: f32| {
            let mut cell = content.h_full().pl(px(10.)).flex().items_center().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().flex_basis(px(0.));
            cell.style().flex_grow = Some(weight);
            cell
        };
        row.h(px(ROW_HEIGHT))
            .flex_none()
            .flex()
            .child(cell(severity, 0.).w(px(36.)).flex_none())
            .child(cell(code, 1.).min_w(px(100.)).max_w(px(300.)))
            .child(cell(message, 4.))
            .child(cell(file, 2.))
            .child(cell(source, 1.).min_w(px(100.)).max_w(px(300.)))
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
