//! contentHoverWidget.ts and the participants' `renderHoverParts`: the hover box above or below its anchor, the
//! marker, markdown and unicode rows, their copy buttons, the status bar, and the `hoverHighlight` decoration.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, Bounds, BoxShadow, ClickEvent, ClipboardItem, Context, Div, MouseMoveEvent, Size, Stateful, Window, canvas,
    deferred, div, point, prelude::*, px, rgba,
};
use serde_json::Value;

use super::Workbench;
use super::content_hover::{MouseTarget, Part, PartContent, Shown};
use super::editor_message::horizontal_in_page;
use super::markdown_view::MarkdownStyle;
use crate::editor::{LINE_HEIGHT, OpenEditor, Position};
use crate::markdown::{MarkdownString, parse, plain_text};
use crate::markers::{ERROR, HINT, INFO, Marker, WARNING};

const UI_FONT: &str = ".SystemUIFont";
const EDITOR_FONT: &str = "Menlo";
const EDITOR_FONT_SIZE: f32 = 12.;
/// cornerRadius-large.
pub(super) const RADIUS: f32 = 8.;
/// resizableContentWidget.ts TOP_HEIGHT and BOTTOM_HEIGHT.
const TOP_HEIGHT: f32 = 30.;
const BOTTOM_HEIGHT: f32 = 24.;

impl Workbench {
    /// contentHoverRendered.ts `_createEditorDecorations`: `.hoverHighlight` over the union of the parts' ranges on
    /// model line `index`.
    pub(super) fn content_hover_highlight(&self, editor: &OpenEditor, index: usize, char_width: f32) -> Option<Div> {
        let shown = self.content_hover.shown.as_ref().filter(|shown| shown.anchor.model_id == editor.model_id)?;
        let (start, end) = highlight_range(&shown.parts)?;
        if index < start.line || index > end.line {
            return None;
        }
        let text = &editor.document().lines[index];
        let from = if index == start.line { start.column } else { 0 };
        let to = if index == end.line { end.column } else { text.chars().count() };
        let (left, right) = editor.line_layout(index).range(from, to);
        let (left, right) = (left * char_width, right * char_width);
        Some(div().absolute().top_0().h_full().left(px(left)).w(px(right - left)).bg(self.color("editor.hoverHighlightBackground")))
    }

    /// The widget over the text area: at the anchor's start, pulled toward the parts' start (`showAtSecondaryPosition`),
    /// above the line when `editor.hover.above` and it fits, else below (`_findPositionPreference`).
    pub(super) fn content_hover_widget(&self, editor: &OpenEditor, content_left: f32, char_width: f32, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let shown = self.content_hover.shown.as_ref().filter(|shown| shown.anchor.model_id == editor.model_id)?;
        let anchor = shown.anchor;
        if anchor.line >= editor.document().lines.len() || editor.folding.is_hidden(anchor.line) {
            return None;
        }
        let mut secondary = anchor.start;
        for part in shown.parts.iter().filter(|part| part.start.line == anchor.line && part.end.line == anchor.line) {
            secondary = secondary.min(part.start.column);
        }
        let view_line = editor.position_view_line(Position::new(anchor.line, anchor.start), false);
        let segment = editor.segment(view_line);
        let primary_left = editor.segment_visible_column(segment, anchor.start) * char_width;
        let secondary_left = editor.segment_visible_column(segment, secondary.max(segment.start)) * char_width;
        // The measuring canvas fills the padding box: add the 1px border on each side.
        let measured = self.content_hover.widget_bounds.get().map(|bounds| Size { width: bounds.size.width + 2., height: bounds.size.height + 2. });
        let mut size = measured.unwrap_or(Size { width: 0., height: 0. });
        if measured.is_some() {
            size.height = self.content_hover.content_height.get() + 2.;
        }
        // contentWidgets.ts `_reduceAnchorCoordinates`: a full-width glyph is 1em of the editor font.
        let left = secondary_left.max(primary_left - size.width + EDITOR_FONT_SIZE);
        let editor_bounds = self.text_area_bounds.get();
        let window_size = window.viewport_size().map(f32::from);
        let left = horizontal_in_page(left - editor.scroll.left + content_left, size.width, editor_bounds, window_size);
        let anchor_top = editor.line_top(view_line, self.editors.viewport) - editor.scroll.top;
        let space_above = editor_bounds.origin.y + anchor_top - TOP_HEIGHT;
        let space_below = window_size.height - (editor_bounds.origin.y + anchor_top + LINE_HEIGHT) - BOTTOM_HEIGHT;
        let height = size.height.min(space_above.max(space_below).min(size.height));
        let above = match self.hover_settings(editor).above {
            true => height <= space_above.min(size.height),
            false => height > space_below.min(size.height),
        };
        let top = if above { anchor_top - size.height } else { anchor_top + LINE_HEIGHT };
        // contentHoverWidget.ts `_updateMaxDimensions` and `_findMaximumRenderingHeight`.
        let max_width = (editor_bounds.size.width * 0.66).max(750.);
        let max_height = (editor_bounds.size.height / 4.).max(250.).min(if above { space_above } else { space_below });
        let high_contrast = self.theme.kind.is_high_contrast();
        let widget_bounds = self.content_hover.widget_bounds.clone();
        let measure = canvas(
            move |bounds, window, _| {
                let bounds: Bounds<f32> = bounds.map(f32::from);
                if widget_bounds.get().map(|old| old.size) != Some(bounds.size) {
                    window.refresh();
                }
                widget_bounds.set(Some(bounds));
            },
            |_, _, _, _| (),
        )
        .absolute()
        .inset_0();
        let widget = div()
            .id("content-hover")
            .absolute()
            .left(px(left))
            .top(px(top))
            .max_w(px(max_width))
            .border_1()
            .border_color(self.color("editorHoverWidget.border"))
            .rounded(px(RADIUS))
            .text_color(self.foreground_or("editorHoverWidget.foreground"))
            .font_family(UI_FONT)
            .text_size(px(EDITOR_FONT_SIZE))
            .line_height(px(LINE_HEIGHT))
            .cursor_default()
            .occlude()
            .when(measured.is_none(), |widget| widget.invisible())
            .when(high_contrast, |widget| widget.bg(self.color("editorHoverWidget.background")))
            .when(!high_contrast, |widget| {
                // --vscode-shadow-lg under frostedGlass.css's `.monaco-resizable-hover`.
                widget.child(self.frosted_glass("editorHoverWidget.background", RADIUS)).shadow(vec![BoxShadow {
                    color: rgba(0x00000024).into(),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(12.),
                    spread_radius: px(0.),
                    inset: false,
                }])
            })
            .on_mouse_move(cx.listener(|workbench, event: &MouseMoveEvent, _, cx| {
                workbench.content_hover_mouse_moved(MouseTarget::Widget, event.position, event.modifiers, event.pressed_button.is_some(), cx);
            }))
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            // contentHoverWidgetWrapper.ts `_onMouseLeave`: leaving the widget for outside the editor hides it.
            .on_hover(cx.listener(|workbench, hovered: &bool, window, cx| {
                let mouse = window.mouse_position().map(f32::from);
                if !*hovered && !workbench.text_area_bounds.get().contains(&mouse) {
                    workbench.hide_content_hover(cx);
                }
            }))
            .child(measure)
            .child(self.content_hover_rows(shown, editor, max_width, max_height, window, cx));
        let widget = widget.with_animation(("content-hover-fade-in", self.content_hover.shows), Animation::new(Duration::from_millis(100)), |widget, delta| widget.opacity(delta));
        Some(deferred(widget).with_priority(1).into_any_element())
    }

    /// The participants' rows in `hoverOrdinal` order, each participant's parts in its own order, then the status bar.
    fn content_hover_rows(&self, shown: &Shown, editor: &OpenEditor, max_width: f32, max_height: f32, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let mut parts: Vec<&Part> = shown.parts.iter().collect();
        parts.sort_by_key(|part| (part.participant, part.ordinal));
        let mut rows = Vec::new();
        for (index, part) in parts.iter().enumerate() {
            let (row, copy_text) = match &part.content {
                PartContent::Marker(marker) => (self.marker_hover_row(marker, index, cx), marker_accessible_content(marker)),
                PartContent::Markdown(contents) => self.markdown_hover_row(contents, index, max_width, &editor.document().language_id, cx),
                PartContent::ColorPicker => {
                    rows.push(div().child(self.color_picker_row(cx)));
                    continue;
                }
            };
            rows.push(self.hover_row_with_copy(row, index, copy_text, cx));
        }
        let first_marker = parts.iter().find_map(|part| match &part.content {
            PartContent::Marker(marker) => Some(marker.clone()),
            PartContent::Markdown(_) | PartContent::ColorPicker => None,
        });
        if let Some(marker) = first_marker {
            rows.push(self.marker_hover_status_bar(marker, window, cx));
        }
        let mut separator = self.color("editorHoverWidget.border");
        separator.a *= 0.5;
        let content_height = self.content_hover.content_height.clone();
        let measure = canvas(
            move |bounds, window, _| {
                let height = f32::from(bounds.size.height);
                if content_height.get() != height {
                    content_height.set(height);
                    window.refresh();
                }
            },
            |_, _, _, _| (),
        )
        .absolute()
        .inset_0();
        let mut column = div().relative().flex().flex_col().child(measure);
        for (index, row) in rows.into_iter().enumerate() {
            // hoverContribution.ts: `.hover-row:not(:first-child):not(:empty)` has a top border at half alpha.
            column = column.child(row.when(index > 0, |row| row.border_t_1().border_color(separator)));
        }
        div().id("content-hover-contents").max_h(px(max_height)).overflow_y_scroll().child(column)
    }

    /// markerHoverParticipant.ts `_renderMarkerHover`: the severity icon, the message in the editor font, the source
    /// and code at 60% opacity, and the related information.
    fn marker_hover_row(&self, marker: &Marker, index: usize, cx: &mut Context<Self>) -> Div {
        let (icon, color) = match marker.severity {
            ERROR => ("error", self.color("problemsErrorIcon.foreground")),
            WARNING => ("warning", self.color("problemsWarningIcon.foreground")),
            INFO => ("info", self.color("problemsInfoIcon.foreground")),
            _ => ("lightbulb", self.foreground_or("editorHoverWidget.foreground")),
        };
        let mut message = div().min_w_0().flex().flex_col().child(div().whitespace_normal().child(marker.message.clone()));
        let details = match &marker.code {
            Value::Object(code) => {
                let value = code.get("value").and_then(Value::as_str).unwrap_or_default().to_string();
                let target = code.get("target").cloned().unwrap_or(Value::Null);
                let link = div()
                    .id(("marker-hover-code", index))
                    .cursor_pointer()
                    .flex()
                    .child("(")
                    .child(div().underline().child(value))
                    .child(")")
                    .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        workbench.open_marker_code(&target, window, cx);
                    }));
                Some(div().flex().children(marker.source.clone()).child(link))
            }
            Value::String(code) => Some(div().child(match &marker.source {
                Some(source) => format!("{source}({code})"),
                None => format!("({code})"),
            })),
            _ => marker.source.clone().map(|source| div().child(source)),
        };
        if let Some(details) = details {
            message = message.child(details.opacity(0.6).pl(px(6.)));
        }
        let link = self.color("textLink.foreground");
        for (related_index, related) in marker.related_information.as_array().into_iter().flatten().enumerate() {
            let resource = super::host_requests::uri_parts(&related["resource"]).ok().map(|(scheme, path)| crate::editor::uri_string(scheme, &path));
            let basename = resource.as_deref().and_then(|uri| uri.rsplit('/').next()).unwrap_or_default().to_string();
            let label = format!("{basename}({}, {}): ", related["startLineNumber"], related["startColumn"]);
            let related_message = related["message"].as_str().unwrap_or_default().to_string();
            let item = related.clone();
            message = message.child(
                div()
                    .mt(px(8.))
                    .child(div().id(("marker-hover-related", index * 1000 + related_index)).cursor_pointer().text_color(link).child(label).on_click(cx.listener(
                        move |workbench, _: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            workbench.hide_content_hover(cx);
                            workbench.open_marker_related(&item, window, cx);
                        },
                    )))
                    .child(related_message),
            );
        }
        div()
            .flex()
            .gap(px(8.))
            .px(px(8.))
            .py(px(4.))
            .font_family(EDITOR_FONT)
            .child(div().flex_none().h(px(LINE_HEIGHT)).flex().items_center().child(self.icon(icon, 16., 16., color)))
            .child(message)
    }

    /// markdownHoverParticipant.ts `renderMarkdown`: each non-empty markdown string in its own `.hover-contents`.
    fn markdown_hover_row(&self, contents: &[Value], index: usize, max_width: f32, model_language: &str, cx: &mut Context<Self>) -> (Div, String) {
        let style = MarkdownStyle { font_size: EDITOR_FONT_SIZE, line_height: LINE_HEIGHT, foreground: self.foreground_or("editorHoverWidget.foreground") };
        let mut row = div().min_w_0().flex().flex_col();
        let mut texts = Vec::new();
        for (content_index, content) in contents.iter().enumerate() {
            if crate::markdown::is_empty(content) {
                continue;
            }
            let blocks = parse(&MarkdownString::from_value(content));
            texts.push(plain_text(&blocks));
            let blocks = self.markdown_blocks(&format!("content-hover-{index}-{content_index}"), &blocks, style, model_language, cx);
            row = row.child(div().px(px(8.)).py(px(4.)).max_w(px(max_width)).child(blocks));
        }
        (row, texts.join("\n"))
    }

    /// hoverCopyButton.ts on every part: a copy button in the top right corner while the row is hovered.
    fn hover_row_with_copy(&self, row: Div, index: usize, text: String, cx: &mut Context<Self>) -> Div {
        let group = format!("content-hover-row-{index}");
        let toolbar_hover = self.color("toolbar.hoverBackground");
        let button = div()
            .id(("content-hover-copy", index))
            .absolute()
            .top(px(4.))
            .right(px(4.))
            .px(px(4.))
            .py(px(2.))
            .rounded(px(3.))
            .opacity(0.)
            .group_hover(group.clone(), |style| style.opacity(1.))
            .hover(move |style| style.bg(toolbar_hover).cursor_pointer())
            .child(self.icon("copy", 16., 16., self.foreground_or("foreground")))
            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                // shortcut: GPUI has no screen reader announcements, so "Copied to clipboard" is not read out; add it with them.
                if !text.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                }
            }));
        div().group(group).relative().flex().pr(px(20.)).child(row).child(button)
    }

    /// markerHoverParticipant.ts `_renderMarkerStatusbar` for the most severe marker: View Problem, then the quick fix
    /// placeholder of a writable editor.
    fn marker_hover_status_bar(&self, marker: std::sync::Arc<Marker>, window: &Window, cx: &mut Context<Self>) -> Div {
        let marker_for_fixes = marker.clone();
        let mut actions = div().w_full().flex().px(px(8.)).bg(self.color("editorHoverWidget.statusBarBackground"));
        if matches!(marker.severity, ERROR | WARNING | INFO) {
            let label = self.hover_action_label("View Problem", "editor.action.marker.next", window);
            let action = div()
                .id("content-hover-view-problem")
                .mr(px(16.))
                .cursor_pointer()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .text_color(self.color("textLink.foreground"))
                .child(label)
                .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    workbench.hide_content_hover(cx);
                    workbench.show_marker_navigation_at(&marker, window, cx);
                    window.focus(&workbench.editor_focus, cx);
                }));
            actions = actions.child(action);
        }
        if self.active_editor_readonly().is_none() {
            actions = match self.marker_quick_fix_status(&marker_for_fixes) {
                super::code_actions::QuickFixStatus::Text(text) => actions.child(div().child(text)),
                super::code_actions::QuickFixStatus::QuickFix => {
                    let label = self.hover_action_label("Quick Fix...", "editor.action.quickFix", window);
                    let bounds = self.code_actions.marker_fixes.label_bounds.clone();
                    let measure = gpui::canvas(move |element_bounds, _, _| bounds.set(element_bounds.map(f32::from)), |_, _, _, _| ()).absolute().inset_0();
                    let action = div()
                        .id("content-hover-quick-fix")
                        .relative()
                        .mr(px(16.))
                        .cursor_pointer()
                        .whitespace_nowrap()
                        .text_color(self.color("textLink.foreground"))
                        .child(measure)
                        .child(label)
                        .on_click(cx.listener(|workbench, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            workbench.show_marker_quick_fixes(cx);
                        }));
                    actions.child(action)
                }
            };
        }
        div().flex().font_family(UI_FONT).text_size(px(12.)).line_height(px(22.)).child(actions)
    }

    /// statusBar `addAction`: the label with the command's keybinding.
    fn hover_action_label(&self, label: &str, command: &str, window: &Window) -> String {
        let context = self.key_context(window);
        match self.keybindings.resolver.lookup_primary_keybinding(command, &context) {
            Some(item) => format!("{label} ({})", crate::keybindings::label(&item.keybinding)),
            None => label.to_owned(),
        }
    }
}

/// `Range.plusRange` over the parts.
fn highlight_range(parts: &[Part]) -> Option<(Position, Position)> {
    let mut range: Option<(Position, Position)> = None;
    for part in parts {
        range = Some(match range {
            None => (part.start, part.end),
            Some((start, end)) => (start.min(part.start), end.max(part.end)),
        });
    }
    range
}

/// markerHoverParticipant.ts `getAccessibleContent`.
pub(super) fn marker_accessible_content(marker: &Marker) -> String {
    let severity = match marker.severity {
        ERROR => "Error",
        WARNING => "Warning",
        INFO => "Info",
        HINT => "Hint",
        _ => "",
    };
    let mut lines = vec![format!("{severity}: {}", marker.message)];
    for related in marker.related_information.as_array().into_iter().flatten() {
        let resource = super::host_requests::uri_parts(&related["resource"]).ok().map(|(scheme, path)| crate::editor::uri_string(scheme, &path));
        let basename = resource.as_deref().and_then(|uri| uri.rsplit('/').next()).unwrap_or_default().to_string();
        lines.push(format!("{basename}({}, {}): {}", related["startLineNumber"], related["startColumn"], related["message"].as_str().unwrap_or_default()));
    }
    lines.join("\n")
}
