//! editor/contrib/gotoError: the marker list of markerNavigationService.ts, the `MarkerController` of gotoError.ts
//! and its `MarkerNavigationWidget` (gotoErrorWidget.ts), a peek view (peekView.ts) in a view zone (zoneWidget.ts).

use std::sync::Arc;

use gpui::{AnyElement, ClickEvent, Context, PathBuilder, Window, canvas, div, point, prelude::*, px};
use serde_json::Value;

use super::Workbench;
use super::hover::HoverOptions;
use crate::commands::Outcome;
use crate::editor::marker_decorations::validate;
use crate::editor::{LINE_HEIGHT, OpenEditor, Position, Selection, VERTICAL_SCROLLBAR_WIDTH, VerticalReveal, utf16_column, visible_column, word_at};
use crate::markers::{ERROR, INFO, Marker, MarkerService, ReadOptions, WARNING};

/// zoneWidget.ts: the arrow is a third of a line high, the frame `frameWidth` 1.
const ARROW_HEIGHT: f32 = 6.;
const FRAME_WIDTH: f32 = 1.;
/// peekView.ts `_doLayout`: `ceil(lineHeight * 1.2)`.
const HEAD_HEIGHT: f32 = 22.;
const UI_FONT: &str = ".SystemUIFont";
const EDITOR_FONT: &str = "Menlo";

/// A 1-based line and UTF-16 column, as markers count.
type MarkerPosition = (usize, usize);

/// markerNavigationService.ts `MarkerList` over the errors, warnings and infos of one resource, or of all.
pub(super) struct MarkerList {
    resource: Option<String>,
    markers: Vec<Arc<Marker>>,
    next: Option<usize>,
}

impl MarkerList {
    fn new(resource: Option<String>, service: &MarkerService, by_position: bool) -> Self {
        let mut list = MarkerList { resource, markers: Vec::new(), next: None };
        list.update(service, by_position);
        list
    }

    /// `updateMarker`: whether the sorted markers differ in resource, range, severity or message.
    fn update(&mut self, service: &MarkerService, by_position: bool) -> bool {
        let mut markers = service.read(ReadOptions { resource: self.resource.as_deref(), severities: Some(ERROR | WARNING | INFO), ..Default::default() });
        markers.sort_by(|a, b| {
            let by_range = range_of(a).cmp(&range_of(b));
            let by_severity = b.severity.cmp(&a.severity);
            a.resource.cmp(&b.resource).then(if by_position { by_range.then(by_severity) } else { by_severity.then(by_range) })
        });
        let same = |a: &Arc<Marker>, b: &Arc<Marker>| a.resource == b.resource && range_of(a) == range_of(b) && a.severity == b.severity && a.message == b.message;
        if markers.len() == self.markers.len() && markers.iter().zip(&self.markers).all(|(a, b)| same(a, b)) {
            return false;
        }
        self.markers = markers;
        true
    }

    /// `selected`: the marker with its 1-based index and the count.
    fn selected(&self) -> Option<(Arc<Marker>, usize, usize)> {
        let index = self.next?;
        Some((self.markers.get(index)?.clone(), index + 1, self.markers.len()))
    }

    /// `_initIdx`: in the model's resource, the marker at or after `position` (an empty marker spans its word);
    /// in another resource, the first marker of the resource after it, or the last one before it going back.
    fn init_index(&mut self, forward: bool, resource: &str, lines: &[String], position: MarkerPosition) {
        let count = self.markers.len();
        let next = match self.markers.iter().position(|marker| marker.resource == resource) {
            None => {
                let index = self.markers.partition_point(|marker| marker.resource.as_str() < resource);
                if forward { index } else { (count + index - 1) % count }
            }
            Some(first) => {
                let mut found = None;
                for (index, marker) in self.markers.iter().enumerate().skip(first) {
                    if marker.resource != resource {
                        break;
                    }
                    let (start, end) = word_range(marker, lines);
                    if contains((start, end), position) || position <= start {
                        found = Some((index, !contains((start, end), position)));
                        break;
                    }
                }
                match found {
                    None if forward => 0,
                    None => count - 1,
                    // Went past the position: going back starts at the marker before.
                    Some((index, true)) if !forward => index.checked_sub(1).unwrap_or(count - 1),
                    Some((index, _)) => index,
                }
            }
        };
        // Forward past the last resource the index is the count and nothing is selected, as upstream.
        self.next = Some(next);
    }

    /// `move`: whether the selected marker changed.
    fn move_to(&mut self, forward: bool, resource: &str, lines: &[String], position: MarkerPosition) -> bool {
        let count = self.markers.len();
        if count == 0 {
            return false;
        }
        let old = self.next;
        match self.next {
            None => self.init_index(forward, resource, lines, position),
            Some(index) if forward => self.next = Some((index + 1) % count),
            Some(index) => self.next = Some((index + count - 1) % count),
        }
        old != self.next
    }

    /// `find`: the marker of `resource` containing `position`.
    fn find(&self, resource: &str, position: MarkerPosition) -> Option<(Arc<Marker>, usize, usize)> {
        let first = self.markers.iter().position(|marker| marker.resource == resource)?;
        let index = (first..self.markers.len()).find(|&index| contains(range_of(&self.markers[index]), position))?;
        Some((self.markers[index].clone(), index + 1, self.markers.len()))
    }
}

fn range_of(marker: &Marker) -> (MarkerPosition, MarkerPosition) {
    ((marker.start_line, marker.start_column), (marker.end_line, marker.end_column))
}

/// `Range.containsPosition`.
fn contains((start, end): (MarkerPosition, MarkerPosition), position: MarkerPosition) -> bool {
    start <= position && position <= end
}

/// An empty marker's range grows to `getWordAtPosition` of its start.
fn word_range(marker: &Marker, lines: &[String]) -> (MarkerPosition, MarkerPosition) {
    let (start, end) = range_of(marker);
    if start != end {
        return (start, end);
    }
    let Some(line) = lines.get(start.0 - 1) else { return (start, end) };
    let column = validate(lines, start.0, start.1).column;
    match word_at(line, column) {
        Some(word) => ((start.0, utf16_column(line, word.start) + 1), (start.0, utf16_column(line, word.end) + 1)),
        None => (start, end),
    }
}

fn marker_position(editor: &OpenEditor, position: Position) -> MarkerPosition {
    (position.line + 1, utf16_column(&editor.document.lines[position.line], position.column) + 1)
}

/// The `MarkerController` session: the list and the widget, from the first navigation until it closes.
#[derive(Default)]
pub(super) struct MarkerNavigation {
    session: Option<Session>,
}

struct Session {
    list: MarkerList,
    model_id: u32,
    widget: Option<MarkerWidget>,
}

struct MarkerWidget {
    zone_id: u32,
    marker: Arc<Marker>,
    index: usize,
    total: usize,
    stale: bool,
}

impl Workbench {
    pub(super) fn register_marker_navigation_commands(&mut self) {
        let actions = [("editor.action.marker.next", true, false), ("editor.action.marker.prev", false, false), ("editor.action.marker.nextInFiles", true, true), ("editor.action.marker.prevInFiles", false, true)];
        for (id, forward, multi_file) in actions {
            self.commands.register(id, move |workbench, _, window, cx| {
                workbench.navigate_markers(forward, multi_file, window, cx);
                Outcome::done()
            });
        }
        self.commands.register("closeMarkersNavigation", |workbench, _, window, cx| {
            workbench.close_marker_navigation(true, window, cx);
            Outcome::done()
        });
    }

    /// `markersNavigationVisible`: set from the first navigation until the session ends, shown or not.
    pub(super) fn markers_navigation_visible(&self) -> bool {
        self.marker_navigation.session.is_some()
    }

    /// `MarkerController.navigate`.
    fn navigate_markers(&mut self, forward: bool, multi_file: bool, window: &mut Window, cx: &mut Context<Self>) {
        let by_position = self.setting("problems.sortOrder") == "position";
        let Some(editor) = self.editors.active() else { return };
        let resource = editor.document.uri_string();
        let position = marker_position(editor, editor.selection().head);
        let model_id = editor.model_id;
        let filter = (!multi_file).then(|| resource.clone());
        // `_getOrCreateModel`: a list of other resources ends the session; the new list starts past the cursor.
        if self.marker_navigation.session.as_ref().is_none_or(|session| session.list.resource != filter) {
            let reuse_position = self.marker_navigation.session.is_some();
            self.end_marker_session();
            let mut list = MarkerList::new(filter, &self.diagnostics.service, by_position);
            let editor = self.editors.active().expect("checked above");
            if reuse_position {
                list.move_to(true, &resource, &editor.document.lines, position);
            }
            self.marker_navigation.session = Some(Session { list, model_id, widget: None });
        }
        let editor = self.editors.active().expect("checked above");
        let session = self.marker_navigation.session.as_mut().expect("created above");
        session.list.move_to(forward, &resource, &editor.document.lines, position);
        let Some((marker, index, total)) = session.list.selected() else { return cx.notify() };
        if marker.resource == resource {
            return self.show_at_marker(marker, index, total, window, cx);
        }
        // Another resource: its editor opens with the marker selected near the top and navigates there.
        self.end_marker_session();
        let Some(path) = crate::editor_history::file_path(&marker.resource) else { return cx.notify() };
        self.open_file(path, false, window, cx);
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut().filter(|editor| editor.document.uri_string() == marker.resource) else { return cx.notify() };
        let lines = &editor.document.lines;
        let (start, end) = (validate(lines, marker.start_line, marker.start_column), validate(lines, marker.end_line, marker.end_column));
        editor.set_selections(vec![Selection { anchor: start, head: end }]);
        editor.reveal_range(start, end, VerticalReveal::NearTop, viewport);
        self.host.send_selection(editor, "code.jump");
        window.focus(&self.editor_focus, cx);
        self.navigate_markers(forward, multi_file, window, cx);
    }

    /// `MarkerNavigationWidget.showAtMarker`: the zone below the cursor when the marker contains it, else below the
    /// marker's start, the cursor moved there, revealed, and the editor focused.
    fn show_at_marker(&mut self, marker: Arc<Marker>, index: usize, total: usize, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("the session's editor is active");
        let lines = &editor.document.lines;
        let (start, end) = (validate(lines, marker.start_line, marker.start_column), validate(lines, marker.end_line, marker.end_column));
        let head = editor.selection().head;
        let position = if start <= head && head <= end { head } else { start };
        let session = self.marker_navigation.session.as_mut().expect("navigating");
        if let Some(widget) = session.widget.take() {
            editor.remove_view_zone(widget.zone_id);
        }
        // `_getMaximumHeightInLines`: max(12, 80% of the editor).
        let height_in_lines = (3 + message_lines(&marker).min(17)) as f32;
        let height_in_lines = height_in_lines.min(12f32.max(viewport.height / LINE_HEIGHT * 0.8));
        let zone_id = editor.add_view_zone(position, height_in_lines * LINE_HEIGHT);
        session.widget = Some(MarkerWidget { zone_id, marker, index, total, stale: false });
        editor.set_selections(vec![Selection::caret(position)]);
        // `_showImpl.revealRange`: the line and the zone below it, the last line near the top.
        let below = Position::new((position.line + 1).min(lines_len(editor) - 1), 0);
        match position.line + 1 == lines_len(editor) {
            true => editor.reveal_range(position, position, VerticalReveal::NearTop, viewport),
            false => editor.reveal_range(Position::new(position.line, 0), below, VerticalReveal::Simple, viewport),
        }
        editor.reveal_range(position, position, VerticalReveal::CenterIfOutsideViewport, viewport);
        self.host.send_selection(editor, "api");
        self.editor_scrolled(cx);
        window.focus(&self.editor_focus, cx);
        cx.notify();
    }

    /// `MarkerController.close`.
    fn close_marker_navigation(&mut self, focus_editor: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.end_marker_session();
        if focus_editor {
            window.focus(&self.editor_focus, cx);
        }
        cx.notify();
    }

    /// `_cleanUp`: the widget's zone goes with the session.
    fn end_marker_session(&mut self) {
        let Some(session) = self.marker_navigation.session.take() else { return };
        let Some(widget) = session.widget else { return };
        if let Some(editor) = self.editors.editors.iter_mut().chain(&mut self.editors.models).find(|editor| editor.model_id == session.model_id) {
            editor.remove_view_zone(widget.zone_id);
        }
    }

    /// The controller's editor listeners, run on every render: another model ends the session
    /// (`onDidChangeModel`), a cursor outside the selected marker resets the list (`onDidChangeCursorPosition`).
    pub(super) fn follow_marker_navigation_cursor(&mut self) {
        let Some(session) = &self.marker_navigation.session else { return };
        let Some(editor) = self.editors.active().filter(|editor| editor.model_id == session.model_id) else { return self.end_marker_session() };
        let position = marker_position(editor, editor.selection().head);
        let session = self.marker_navigation.session.as_mut().expect("checked above");
        if !session.list.selected().is_some_and(|(marker, ..)| contains(range_of(&marker), position)) {
            session.list.next = None;
        }
    }

    /// The list's `onDidChange`: the widget shows the marker now at its position, or its old one as stale.
    pub(super) fn marker_navigation_markers_changed(&mut self) {
        let by_position = self.setting("problems.sortOrder") == "position";
        let Some(session) = self.marker_navigation.session.as_mut() else { return };
        if !session.list.update(&self.diagnostics.service, by_position) {
            return;
        }
        session.list.next = None;
        let Some(widget) = session.widget.as_mut() else { return };
        let Some(editor) = self.editors.editors.iter().find(|editor| editor.model_id == session.model_id) else { return };
        let Some(zone) = editor.view_zone(widget.zone_id) else { return };
        match session.list.find(&editor.document.uri_string(), marker_position(editor, zone.anchor)) {
            Some((marker, index, total)) => (widget.marker, widget.index, widget.total, widget.stale) = (marker, index, total, false),
            None => widget.stale = true,
        }
    }

    /// The widget over its zone's whitespace, `left` 0 and as wide as the editor without the minimap and the vertical
    /// scrollbar (`_getWidth`), with the arrow at the zone's position (`Arrow`).
    pub(super) fn marker_navigation_widget(&self, editor: &OpenEditor, content_left: f32, char_width: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.marker_navigation.session.as_ref().filter(|session| session.model_id == editor.model_id)?;
        let widget = session.widget.as_ref()?;
        let viewport = self.editors.viewport;
        let zone = editor.view_zone(widget.zone_id)?;
        let top = editor.view_zone_top(widget.zone_id, viewport)? - editor.scroll.top;
        let width = content_left + viewport.width - VERTICAL_SCROLLBAR_WIDTH;
        let (frame_key, header_key, icon) = match widget.marker.severity {
            WARNING => ("editorMarkerNavigationWarning.background", "editorMarkerNavigationWarning.headerBackground", "warning"),
            INFO => ("editorMarkerNavigationInfo.background", "editorMarkerNavigationInfo.headerBackground", "info"),
            _ => ("editorMarkerNavigationError.background", "editorMarkerNavigationError.headerBackground", "error"),
        };
        let icon_color = match widget.marker.severity {
            WARNING => self.color("problemsWarningIcon.foreground"),
            INFO => self.color("problemsInfoIcon.foreground"),
            _ => self.color("problemsErrorIcon.foreground"),
        };
        let frame = self.color(frame_key);
        // zoneWidget.ts `show`: the arrow is not pretty at column 1 and moves to column 2.
        let line = &editor.document.lines[zone.anchor.line];
        let arrow_column = zone.anchor.column.max(1).min(line.chars().count().max(1));
        let arrow_x = content_left + visible_column(line, arrow_column, editor.document.indentation.tab_size) as f32 * char_width - editor.scroll.left;
        let arrow = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let (x, y) = (bounds.origin.x, bounds.origin.y);
                let mut path = PathBuilder::fill();
                path.move_to(point(x, y + px(ARROW_HEIGHT)));
                path.line_to(point(x + px(2. * ARROW_HEIGHT), y + px(ARROW_HEIGHT)));
                path.line_to(point(x + px(ARROW_HEIGHT), y));
                path.close();
                if let Ok(path) = path.build() {
                    window.paint_path(path, frame);
                }
            },
        )
        .absolute()
        .top_0()
        .left(px(arrow_x - ARROW_HEIGHT))
        .w(px(2. * ARROW_HEIGHT))
        .h(px(ARROW_HEIGHT));
        let container_height = zone.height - 2. * ARROW_HEIGHT - 2. * FRAME_WIDTH;
        let file_name = editor.document.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let detail = match widget.total > 1 {
            true => format!("{} of {} problems", widget.index, widget.total),
            false => format!("{} of {} problem", widget.index, widget.total),
        };
        let title = div()
            .ml(px(20.))
            .min_w_0()
            .overflow_hidden()
            .flex()
            .items_baseline()
            .text_size(px(13.))
            .child(div().self_center().mr(px(4.)).child(self.icon(icon, 16., 16., icon_color)))
            .child(div().whitespace_nowrap().text_color(self.color("peekViewTitleLabel.foreground")).child(file_name))
            .child(div().whitespace_nowrap().ml(px(0.5 * 0.9 * 13.)).text_size(px(0.9 * 13.)).text_color(self.color("peekViewTitleDescription.foreground")).child(detail));
        // The title menu (next, previous) before the peek view's close action.
        let actions = div()
            .flex_1()
            .flex()
            .justify_end()
            .items_center()
            .pr(px(2.))
            .child(self.peek_action("arrow-down", "Go to Next Problem (Error, Warning, Info)", "editor.action.marker.next", cx))
            .child(self.peek_action("arrow-up", "Go to Previous Problem (Error, Warning, Info)", "editor.action.marker.prev", cx))
            .child(self.peek_action("close", "Close", "closeMarkersNavigation", cx));
        let head = div()
            .h(px(HEAD_HEIGHT))
            .line_height(px(HEAD_HEIGHT))
            .flex()
            .justify_between()
            .flex_nowrap()
            .font_family(UI_FONT)
            .when_some(self.theme.color(header_key), |head, color| head.bg(color))
            .child(title)
            .child(actions);
        let body = div()
            .h(px(container_height - HEAD_HEIGHT))
            .border_t(px(1.))
            .border_color(frame)
            .bg(self.color("editorMarkerNavigation.background"))
            .child(self.marker_message(&widget.marker, widget.stale, cx));
        let container = div()
            .absolute()
            .top(px(ARROW_HEIGHT))
            .left_0()
            .right_0()
            .h(px(container_height + 2. * FRAME_WIDTH))
            .overflow_hidden()
            .border_t(px(FRAME_WIDTH))
            .border_b(px(FRAME_WIDTH))
            .border_color(frame)
            .child(head)
            .child(body);
        let widget = div()
            .id("marker-navigation-widget")
            .absolute()
            .left_0()
            .top(px(top))
            .w(px(width))
            .h(px(zone.height))
            .cursor_default()
            .occlude()
            .child(container)
            .child(arrow);
        Some(widget.into_any_element())
    }

    /// `MessageWidget.update`: the message lines in the editor font, the source and code after the last one, then the
    /// related information, scrolled inside the body.
    // shortcut: GPUI has no screen reader announcements, so the aria label is not read out; add one with them.
    // shortcut: the body scrolls without the 6px scrollbars of its ScrollableElement; add them with a scrollbar element.
    fn marker_message(&self, marker: &Marker, stale: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let link = self.color("textLink.activeForeground");
        let lines: Vec<&str> = marker.message.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)).collect();
        let mut details = div().flex().pl(px(6.));
        if let Some(source) = &marker.source {
            details = details.child(div().opacity(0.6).child(source.clone()));
        }
        match &marker.code {
            Value::String(code) => details = details.child(div().opacity(0.6).child(format!("({code})"))),
            Value::Object(code) => {
                let value = code.get("value").and_then(Value::as_str).unwrap_or_default().to_string();
                let target = code.get("target").cloned().unwrap_or(Value::Null);
                details = details.child(
                    div()
                        .id("marker-code-link")
                        .flex()
                        .opacity(0.6)
                        .cursor_pointer()
                        .child("(")
                        .child(div().underline().text_color(link).child(value))
                        .child(")")
                        .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            // The opener service shows web links in the browser, other resources in an editor.
                            let part = |key: &str| target[key].as_str().unwrap_or_default().to_string();
                            match part("scheme").as_str() {
                                "http" | "https" => {
                                    let query = Some(part("query")).filter(|query| !query.is_empty()).map(|query| format!("?{query}")).unwrap_or_default();
                                    let fragment = Some(part("fragment")).filter(|fragment| !fragment.is_empty()).map(|fragment| format!("#{fragment}")).unwrap_or_default();
                                    cx.open_url(&format!("{}://{}{}{query}{fragment}", part("scheme"), part("authority"), part("path")));
                                }
                                _ => {
                                    workbench.execute_command("vscode.open", vec![target.clone()], window, cx);
                                }
                            }
                        })),
                )
            }
            _ => {}
        }
        let has_details = marker.source.is_some() || !marker.code.is_null();
        let mut message = div().flex().flex_col();
        let (last, first_lines) = lines.split_last().expect("split yields a line");
        for line in first_lines {
            message = message.child(div().h(px(LINE_HEIGHT)).child(line.to_string()));
        }
        message = message.child(div().flex().h(px(LINE_HEIGHT)).child(last.to_string()).when(has_details, |row| row.child(details)));
        let mut related_block = div();
        let related = marker.related_information.as_array().cloned().unwrap_or_default();
        if !related.is_empty() {
            let mut node = div().pt(px((LINE_HEIGHT * 0.66).floor()));
            for (index, item) in related.into_iter().enumerate() {
                let resource = super::host_requests::uri_parts(&item["resource"]).ok().map(|(scheme, path)| crate::editor::uri_string(scheme, &path));
                let basename = resource.as_deref().and_then(|uri| uri.rsplit('/').next()).unwrap_or_default().to_string();
                let label = format!("{basename}({}, {}): ", item["startLineNumber"], item["startColumn"]);
                let message = item["message"].as_str().unwrap_or_default().to_string();
                node = node.child(
                    div()
                        .flex()
                        .h(px(LINE_HEIGHT))
                        .child(div().id(("marker-related", index)).cursor_pointer().text_color(link).child(label).on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                            workbench.open_related_information(&item, window, cx);
                        })))
                        .child(message),
                );
            }
            related_block = related_block.child(node);
        }
        div()
            .id("marker-widget-message")
            .size_full()
            .overflow_scroll()
            .when(stale, |body| body.opacity(0.6).italic())
            .child(div().pt(px(8.)).pr(px(12.)).pl(px(20.)).whitespace_nowrap().font_family(EDITOR_FONT).text_size(px(12.)).line_height(px(LINE_HEIGHT)).child(message).child(related_block))
    }

    /// `onDidSelectRelatedInformation`: the resource opens pinned with the cursor at the related range's start and
    /// the session closes without focusing the old editor.
    fn open_related_information(&mut self, related: &Value, window: &mut Window, cx: &mut Context<Self>) {
        self.close_marker_navigation(false, window, cx);
        let Ok((scheme, path)) = super::host_requests::uri_parts(&related["resource"]) else { return };
        if scheme != "file" {
            return;
        }
        self.open_file(path, true, window, cx);
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut() else { return };
        let line = related["startLineNumber"].as_u64().unwrap_or(1) as usize;
        let column = related["startColumn"].as_u64().unwrap_or(1) as usize;
        let position = validate(&editor.document.lines, line, column);
        editor.set_selections(vec![Selection::caret(position)]);
        editor.reveal_range(position, position, VerticalReveal::CenterIfOutsideViewport, viewport);
        self.host.send_selection(editor, "code.jump");
        self.editor_scrolled(cx);
    }

    /// An action of the peek view's action bar: a 16px codicon in a 22px box with the toolbar hover.
    fn peek_action(&self, icon: &str, title: &'static str, command: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        let hover = self.color("toolbar.hoverBackground");
        self.icon(icon, 22., 16., self.foreground_or("icon.foreground"))
            .id(command)
            .rounded(px(5.))
            .cursor_pointer()
            .hover(move |action| action.bg(hover))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                workbench.execute_command(command, Vec::new(), window, cx);
            }))
            .child(self.action_hover(title, command, HoverOptions::mouse()))
    }
}

fn lines_len(editor: &OpenEditor) -> usize {
    editor.document.lines.len()
}

/// `MessageWidget._lines`: the message lines, and with related information a spacer line and one per item.
fn message_lines(marker: &Marker) -> usize {
    let related = marker.related_information.as_array().map_or(0, Vec::len);
    marker.message.split('\n').count() + if related > 0 { 1 + related } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn service(markers: &[(&str, u8, [usize; 4])]) -> MarkerService {
        let mut service = MarkerService::default();
        let mut by_resource: Vec<(String, Vec<Value>)> = Vec::new();
        for (resource, severity, [start_line, start_column, end_line, end_column]) in markers {
            let data = json!({ "severity": severity, "message": "m", "startLineNumber": start_line, "startColumn": start_column, "endLineNumber": end_line, "endColumn": end_column });
            match by_resource.iter_mut().find(|(uri, _)| uri == resource) {
                Some((_, list)) => list.push(data),
                None => by_resource.push((resource.to_string(), vec![data])),
            }
        }
        for (resource, data) in by_resource {
            service.change_one("owner", &resource, &data, false);
        }
        service
    }

    fn lines() -> Vec<String> {
        (0..20).map(|_| "let value = 1;".to_string()).collect()
    }

    #[test]
    fn moves_from_the_cursor_and_wraps() {
        let service = service(&[("file:///a", ERROR, [2, 1, 2, 4]), ("file:///a", WARNING, [5, 1, 5, 4]), ("file:///a", INFO, [9, 1, 9, 4])]);
        let mut list = MarkerList::new(Some("file:///a".into()), &service, true);
        assert!(list.move_to(true, "file:///a", &lines(), (3, 1)));
        assert_eq!(list.selected().map(|(marker, index, total)| (marker.start_line, index, total)), Some((5, 2, 3)));
        list.move_to(true, "file:///a", &lines(), (3, 1));
        list.move_to(true, "file:///a", &lines(), (3, 1));
        assert_eq!(list.selected().map(|(marker, ..)| marker.start_line), Some(2));
        list.next = None;
        list.move_to(false, "file:///a", &lines(), (3, 1));
        assert_eq!(list.selected().map(|(marker, ..)| marker.start_line), Some(2));
        list.next = None;
        list.move_to(false, "file:///a", &lines(), (12, 1));
        assert_eq!(list.selected().map(|(marker, ..)| marker.start_line), Some(9));
    }

    #[test]
    fn sorts_by_severity_unless_by_position_and_finds_other_resources() {
        let service = service(&[("file:///a", INFO, [1, 1, 1, 4]), ("file:///a", ERROR, [5, 1, 5, 4]), ("file:///c", ERROR, [1, 1, 1, 2])]);
        let list = MarkerList::new(None, &service, false);
        assert_eq!(list.markers.iter().map(|marker| (marker.resource.as_str(), marker.start_line)).collect::<Vec<_>>(), [("file:///a", 5), ("file:///a", 1), ("file:///c", 1)]);
        let mut list = MarkerList::new(None, &service, true);
        assert_eq!(list.markers[0].start_line, 1);
        list.move_to(true, "file:///b", &lines(), (1, 1));
        assert_eq!(list.selected().map(|(marker, ..)| marker.resource.clone()), Some("file:///c".to_string()));
        list.next = None;
        list.move_to(false, "file:///b", &lines(), (1, 1));
        assert_eq!(list.selected().map(|(marker, index, _)| (marker.resource.clone(), index)), Some(("file:///a".to_string(), 2)));
        assert_eq!(list.find("file:///a", (5, 2)).map(|(_, index, _)| index), Some(2));
    }

    #[test]
    fn an_empty_marker_spans_its_word() {
        let service = service(&[("file:///a", ERROR, [1, 6, 1, 6])]);
        let mut list = MarkerList::new(Some("file:///a".into()), &service, true);
        // The cursor at the end of `value` is inside the word, so going back selects the marker itself.
        list.move_to(false, "file:///a", &lines(), (1, 10));
        assert_eq!(list.selected().map(|(_, index, _)| index), Some(1));
    }
}
