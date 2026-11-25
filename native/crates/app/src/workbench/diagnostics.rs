//! mainThreadDiagnostics.ts, the marker decorations of open models (markerDecorationsService.ts) and the
//! Problems status bar entry (markers.contribution.ts `MarkersStatusBarContributions`).

use std::collections::HashMap;
use std::ops::Range;

use gpui::{AnyElement, Bounds, Context, Div, HighlightStyle, PathBuilder, Rgba, StrikethroughStyle, Window, canvas, div, fill, point, prelude::*, px, size};
use serde_json::{Value, json};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use super::host_requests::uri_parts;
use crate::editor::marker_decorations::{MAX_MARKER_DECORATIONS, MarkerDecoration};
use crate::editor::{LINE_HEIGHT, OpenEditor, expand_tabs};
use crate::markers::{DEPRECATED, ERROR, HINT, INFO, MarkerService, MarkerStatistics, ReadOptions, UNNECESSARY, WARNING};

/// `MainThreadDiagnostics.extHostId`: the window's one extension host is the first.
const EXT_HOST_ID: &str = "extHost1";

#[derive(Default)]
pub struct Diagnostics {
    pub service: MarkerService,
    /// The `UriComponents` of every resource with markers, by `URI.toString()`, for `$acceptMarkersChange`.
    uris: HashMap<String, Value>,
}

impl Workbench {
    /// `$changeMany(owner, [uri, markers, isTruncated][])`; markers without an origin get the host's.
    pub(crate) fn change_many_markers(&mut self, owner: &str, entries: &Value, cx: &mut Context<Self>) {
        let mut changed = Vec::new();
        for entry in entries.as_array().into_iter().flatten() {
            let Ok((scheme, path)) = uri_parts(&entry[0]) else { continue };
            let resource = crate::editor::uri_string(scheme, &path);
            let mut markers = entry[1].as_array().cloned().unwrap_or_default();
            for marker in &mut markers {
                if marker["origin"].is_null() {
                    marker["origin"] = json!(EXT_HOST_ID);
                }
            }
            self.diagnostics.uris.insert(resource.clone(), entry[0].clone());
            changed.extend(self.diagnostics.service.change_one(owner, &resource, &markers, entry[2] == true));
        }
        self.markers_changed(changed, cx);
    }

    /// The files that have markers of `owner` (`IMarkerService.read({ owner })`).
    pub(super) fn marker_resources_of(&self, owner: &str) -> Vec<std::path::PathBuf> {
        let mut paths = Vec::new();
        for marker in self.diagnostics.service.read(ReadOptions { owner: Some(owner), ..Default::default() }) {
            let path = self.diagnostics.uris.get(&marker.resource).and_then(|uri| uri["path"].as_str()).map(std::path::PathBuf::from);
            if let Some(path) = path.filter(|path| !paths.contains(path)) {
                paths.push(path);
            }
        }
        paths
    }

    /// `$clear(owner)`.
    pub(crate) fn clear_markers(&mut self, owner: &str, cx: &mut Context<Self>) {
        let changed = self.diagnostics.service.change_all(owner, &[]);
        self.markers_changed(changed, cx);
    }

    /// `onMarkerChanged`: the decorations of the changed models, and `_forwardMarkers` of what other sources than
    /// the host reported.
    fn markers_changed(&mut self, mut resources: Vec<String>, cx: &mut Context<Self>) {
        resources.dedup();
        if resources.is_empty() {
            return;
        }
        let mut data = Vec::new();
        for resource in &resources {
            let markers = self.diagnostics.service.read(ReadOptions { resource: Some(resource), ..Default::default() });
            let others: Vec<Value> = markers.iter().filter(|marker| marker.origin.as_deref() != Some(EXT_HOST_ID)).map(|marker| marker.to_data()).collect();
            let uri = match markers.is_empty() {
                true => self.diagnostics.uris.remove(resource),
                false => self.diagnostics.uris.get(resource).cloned(),
            };
            data.extend(uri.map(|uri| json!([uri, others])));
        }
        self.host.send("ExtHostDiagnostics", "$acceptMarkersChange", vec![json!(data)]);
        self.update_marker_decorations(Some(&resources));
        self.marker_navigation_markers_changed();
        self.auto_save_markers_changed(&resources, cx);
        cx.notify();
    }

    /// `_updateDecorations` of every model, or of the models of `resources`.
    pub(super) fn update_marker_decorations(&mut self, resources: Option<&[String]>) {
        let service = &self.diagnostics.service;
        for editor in self.editors.editors.iter_mut().chain(&mut self.editors.models) {
            let resource = editor.document.uri_string();
            if resources.is_some_and(|resources| !resources.contains(&resource)) {
                continue;
            }
            let markers = service.read(ReadOptions { resource: Some(&resource), take: Some(MAX_MARKER_DECORATIONS), ..Default::default() });
            editor.update_markers(&markers);
        }
    }

    /// The marker decorations' `className`s on model line `line`, lowest z-index first: the squiggles of
    /// codeEditorWidget.ts and editor.css; `x` maps a char column to its left edge.
    pub(super) fn marker_line_decorations(&self, editor: &OpenEditor, line: usize, char_width: f32, x: impl Fn(usize) -> f32) -> Vec<AnyElement> {
        let mut decorations: Vec<&MarkerDecoration> = editor.markers.iter().filter(|decoration| decoration.start.line <= line && line <= decoration.end.line).collect();
        decorations.sort_by_key(|decoration| decoration.marker.severity);
        let line_length = editor.document.lines[line].chars().count();
        let mut elements = Vec::new();
        for decoration in decorations {
            let from = if line == decoration.start.line { decoration.start.column } else { 0 };
            let to = if line == decoration.end.line { decoration.end.column } else { line_length };
            let left = x(from);
            // `showIfCollapsed`: an empty range is 1ch wide.
            let width = if decoration.start == decoration.end { char_width } else { x(to) - left };
            let squiggle = |key: &str| (self.theme.color(&format!("{key}.foreground")), self.theme.color(&format!("{key}.background")), self.theme.color(&format!("{key}.border")));
            let marker = &decoration.marker;
            let element = match marker.severity {
                HINT if marker.has_tag(DEPRECATED) => continue,
                HINT if marker.has_tag(UNNECESSARY) => {
                    let border = self.theme.color("editorUnnecessaryCode.border").filter(|_| self.setting("editor.showUnused") != false);
                    // shortcut: GPUI has no dashed bottom border alone, so .squiggly-unnecessary's 2px dashed border is solid.
                    border.map(|border| div().absolute().top(px(LINE_HEIGHT)).h(px(2.)).left(px(left)).w(px(width)).bg(border).into_any_element())
                }
                HINT => Some(hint_decoration(left, width, self.theme.color("editorHint.foreground"), self.theme.color("editorHint.border"))),
                INFO => Some(squiggly_decoration(left, width, squiggle("editorInfo"))),
                WARNING => Some(squiggly_decoration(left, width, squiggle("editorWarning"))),
                _ => Some(squiggly_decoration(left, width, squiggle("editorError"))),
            };
            elements.extend(element);
        }
        elements
    }

    /// The marker decorations' `inlineClassName`s on model line `line`, as byte ranges of its tab-expanded text:
    /// unnecessary code at `editorUnnecessaryCode.opacity` with `editor.showUnused`, deprecated code struck through
    /// with `editor.showDeprecated`.
    pub(super) fn marker_text_styles(&self, editor: &OpenEditor, line: usize, tab_size: usize) -> Vec<(Range<usize>, HighlightStyle)> {
        let text = &editor.document.lines[line];
        let byte = |column: usize| expand_tabs(&text.chars().take(column).collect::<String>(), tab_size).len();
        let fade = match self.setting("editor.showUnused") != false {
            true => self.theme.color("editorUnnecessaryCode.opacity").map(|opacity| 1. - opacity.a),
            false => None,
        };
        let strikethrough = (self.setting("editor.showDeprecated") != false).then(|| StrikethroughStyle { thickness: px(1.), color: self.theme.color("editor.foreground").map(Into::into) });
        let mut styles = Vec::new();
        for decoration in editor.markers.iter().filter(|decoration| decoration.start.line <= line && line <= decoration.end.line) {
            let style = match (decoration.marker.has_tag(DEPRECATED), decoration.marker.has_tag(UNNECESSARY)) {
                (true, _) => HighlightStyle { strikethrough, ..Default::default() },
                (false, true) => HighlightStyle { fade_out: fade, ..Default::default() },
                (false, false) => continue,
            };
            let from = if line == decoration.start.line { decoration.start.column } else { 0 };
            let to = if line == decoration.end.line { decoration.end.column } else { text.chars().count() };
            if from < to {
                styles.push((byte(from)..byte(to), style));
            }
        }
        styles.sort_by_key(|(range, _)| range.start);
        styles
    }

    /// The marker decorations' `overviewRuler` line ranges in `OverviewRulerLane.Right` by z-index: infos, warnings, errors.
    pub(super) fn marker_ruler_lines(&self, editor: &OpenEditor) -> [(Vec<(usize, usize)>, Rgba); 3] {
        let lines = |severity: u8| {
            let mut lines: Vec<(usize, usize)> = editor.markers.iter().filter(|decoration| decoration.marker.severity == severity).map(|decoration| (decoration.start.line, decoration.end.line)).collect();
            lines.sort();
            lines
        };
        [
            (lines(INFO), self.color("editorOverviewRuler.infoForeground")),
            (lines(WARNING), self.color("editorOverviewRuler.warningForeground")),
            (lines(ERROR), self.color("editorOverviewRuler.errorForeground")),
        ]
    }

    /// The marker decorations' inline `minimap` colors by z-index: infos, warnings, errors; hints have none.
    pub(super) fn marker_minimap_ranges(&self, editor: &OpenEditor) -> Vec<(crate::editor::Position, crate::editor::Position, Rgba)> {
        let mut ranges = Vec::new();
        for (severity, key) in [(INFO, "minimap.infoHighlight"), (WARNING, "minimap.warningHighlight"), (ERROR, "minimap.errorHighlight")] {
            let Some(color) = self.theme.color(key) else { continue };
            ranges.extend(editor.markers.iter().filter(|decoration| decoration.marker.severity == severity).map(|decoration| (decoration.start, decoration.end, color)));
        }
        ranges
    }

    /// The `status.problems` entry: `$(error) N $(warning) N`, `$(info) N` only with infos.
    pub(super) fn problems_status_item(&self, foreground: Rgba) -> Div {
        let stats = self.diagnostics.service.statistics();
        let mut counts = vec![("error", stats.errors), ("warning", stats.warnings)];
        if stats.infos > 0 {
            counts.push(("info", stats.infos));
        }
        let mut item = div().relative().h_full().cursor_pointer().mx(px(3.)).px(px(5.)).flex().items_center();
        for (index, (icon, count)) in counts.iter().enumerate() {
            let separator = if index + 1 < counts.len() { " " } else { "" };
            item = item.child(self.icon(icon, 16., 16., foreground)).child(format!(" {}{separator}", pack_number(*count)));
        }
        item.child(self.hover_target("status.problems", HoverContent::Text(markers_tooltip(stats)), HoverOptions::status_bar()))
    }
}

/// `.squiggly-error` and its kin: the theme's 6x3 squiggle tiled along the bottom of the line from the left, the
/// `::before` background over the line, and with a border color a 4px double border under it.
fn squiggly_decoration(left: f32, width: f32, (foreground, background, border): (Option<Rgba>, Option<Rgba>, Option<Rgba>)) -> AnyElement {
    let squiggle = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let Some(color) = foreground else { return };
            paint_squiggles(bounds, color, window);
        },
    )
    .size_full();
    let mut decoration = div().absolute().top_0().h_full().left(px(left)).w(px(width));
    if let Some(background) = background {
        decoration = decoration.child(div().absolute().inset_0().bg(background));
    }
    decoration = decoration.child(div().absolute().inset_0().overflow_hidden().child(squiggle));
    if let Some(border) = border {
        // A 4px double border: two 1px lines with a 2px gap, below the 18px box as CSS content-box draws it.
        decoration = decoration.child(div().absolute().top(px(LINE_HEIGHT)).h(px(1.)).w_full().bg(border)).child(div().absolute().top(px(LINE_HEIGHT + 3.)).h(px(1.)).w_full().bg(border));
    }
    decoration.into_any_element()
}

/// `getSquigglySVGData`'s three polygons per 6px tile, the tile's bottom on the box's bottom.
fn paint_squiggles(bounds: Bounds<gpui::Pixels>, color: Rgba, window: &mut Window) {
    const POLYGONS: [[(f32, f32); 4]; 3] = [[(5.5, 0.), (2.5, 3.), (1.1, 3.), (4.1, 0.)], [(4., 0.), (6., 2.), (6., 0.6), (5.4, 0.)], [(0., 2.), (1., 3.), (2.4, 3.), (0., 0.6)]];
    let top = bounds.bottom() - px(3.);
    let mut path = PathBuilder::fill();
    let mut tile = 0.;
    while tile < f32::from(bounds.size.width) {
        for polygon in POLYGONS {
            let at = |(x, y): (f32, f32)| point(bounds.left() + px(tile + x), top + px(y));
            path.move_to(at(polygon[0]));
            for corner in &polygon[1..] {
                path.line_to(at(*corner));
            }
            path.close();
        }
        tile += 6.;
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// `.squiggly-hint`: `getDotDotDotSVGData`'s three 1px-radius dots at the bottom left, not repeated, and with a
/// border color a 2px dotted border under the box.
fn hint_decoration(left: f32, width: f32, foreground: Option<Rgba>, border: Option<Rgba>) -> AnyElement {
    let dots = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let Some(color) = foreground else { return };
            for x in [0., 4., 8.] {
                let origin = point(bounds.left() + px(x), bounds.bottom() - px(3.));
                window.paint_quad(fill(Bounds::new(origin, size(px(2.), px(2.))), color).corner_radii(px(1.)));
            }
        },
    )
    .size_full();
    let mut decoration = div().absolute().top_0().h_full().left(px(left)).w(px(width)).child(div().absolute().inset_0().overflow_hidden().child(dots));
    if let Some(border) = border {
        // shortcut: GPUI has no dotted border, so the 2px dotted border is solid.
        decoration = decoration.child(div().absolute().top(px(LINE_HEIGHT)).h(px(2.)).w_full().bg(border));
    }
    decoration.into_any_element()
}

/// `getMarkersTooltip`.
fn markers_tooltip(stats: MarkerStatistics) -> String {
    let mut titles = Vec::new();
    if stats.errors > 0 {
        titles.push(format!("Errors: {}", stats.errors));
    }
    if stats.warnings > 0 {
        titles.push(format!("Warnings: {}", stats.warnings));
    }
    if stats.infos > 0 {
        titles.push(format!("Infos: {}", stats.infos));
    }
    match titles.is_empty() {
        true => "No Problems".into(),
        false => titles.join(", "),
    }
}

/// `packNumber`.
fn pack_number(count: usize) -> String {
    match count {
        10000.. => "10K+".into(),
        1000.. => format!("{}K", count.to_string().chars().next().unwrap_or('0')),
        _ => count.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_entry_packs_counts_and_lists_only_nonzero_kinds() {
        assert_eq!([pack_number(999), pack_number(1500), pack_number(10000)], ["999", "1K", "10K+"]);
        assert_eq!(markers_tooltip(MarkerStatistics::default()), "No Problems");
        assert_eq!(markers_tooltip(MarkerStatistics { errors: 2, warnings: 0, infos: 1, unknowns: 3 }), "Errors: 2, Infos: 1");
    }
}
