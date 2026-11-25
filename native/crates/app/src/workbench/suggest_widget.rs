//! suggestWidget.ts and suggestWidgetRenderer.ts: the list of completions under (or above) the cursor, its focus,
//! the Loading and No suggestions messages, and the resolve of the focused item.

use std::time::Duration;

use gpui::{AnyElement, BoxShadow, Context, FontWeight, HighlightStyle, MouseButton, ScrollWheelEvent, StyledText, Task, Window, deferred, div, point, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::editor_message::horizontal_in_page;
use super::suggest_controller::InsertFlags;
use crate::editor::{LINE_HEIGHT, OpenEditor, Position, visible_column};
use crate::filters::create_matches;
use crate::suggest::CompletionItem;
use crate::symbol_icons::completion_kind_icon;

const EDITOR_FONT: &str = "Menlo";
const EDITOR_FONT_SIZE: f32 = 12.;
/// cornerRadius-large.
const RADIUS: f32 = 8.;
/// `getLayoutInfo`: the default size shows 12 items; the widget keeps 22px from the window's edges.
const VISIBLE_ITEMS: usize = 12;
const DEFAULT_WIDTH: f32 = 430.;
const VERTICAL_PADDING: f32 = 22.;
/// suggestWidget.ts `_persistedSize`'s neighbour: whether the docs are expanded, in the profile storage.
const EXPAND_DOCS_KEY: &str = "expandSuggestionDocs";

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(super) enum State {
    #[default]
    Hidden,
    Loading,
    Empty,
    Open,
    Frozen,
}

#[derive(Default)]
pub(super) struct WidgetState {
    pub state: State,
    pub focused: Option<usize>,
    /// The first item in view.
    top: usize,
    is_auto: bool,
    /// `_loadingTimeout`.
    loading: Option<Task<()>>,
    /// The content widget's position: the cursor when shown.
    position: Option<Position>,
    /// `_currentSuggestionDetails`: the resolve of the focused item, cancelled by dropping it.
    resolving: Option<Task<()>>,
    /// `expandSuggestionDocs` of the profile storage, read on the first show.
    pub details: bool,
    details_read: bool,
}

impl Workbench {
    pub(super) fn suggest_widget_visible(&self) -> bool {
        self.suggest.widget.state != State::Hidden
    }

    pub(super) fn suggest_widget_frozen(&self) -> bool {
        self.suggest.widget.state == State::Frozen
    }

    /// `showTriggered`: a manual trigger shows "Loading..." after `delay` ms.
    pub(super) fn show_suggest_triggered(&mut self, auto: bool, cx: &mut Context<Self>) {
        let widget = &mut self.suggest.widget;
        if widget.state != State::Hidden {
            return;
        }
        widget.position = self.editors.active().map(|editor| editor.selection().head);
        widget.is_auto = auto;
        if !auto {
            widget.loading = Some(cx.spawn(async |workbench, cx| {
                cx.background_executor().timer(Duration::from_millis(50)).await;
                let _ = workbench.update(cx, |workbench, cx| {
                    workbench.set_suggest_widget_state(State::Loading);
                    cx.notify();
                });
            }));
        }
    }

    /// `showSuggestions`.
    pub(super) fn show_suggestions(&mut self, index: usize, frozen: bool, auto: bool, no_focus: bool, cx: &mut Context<Self>) {
        let position = self.editors.active().map(|editor| editor.selection().head);
        let count = self.suggest_items().len();
        let widget = &mut self.suggest.widget;
        widget.position = position;
        widget.loading = None;
        widget.resolving = None;
        if frozen && widget.state != State::Empty && widget.state != State::Hidden {
            self.set_suggest_widget_state(State::Frozen);
            cx.notify();
            return;
        }
        if count == 0 {
            self.set_suggest_widget_state(if auto { State::Hidden } else { State::Empty });
            cx.notify();
            return;
        }
        self.set_suggest_widget_state(if frozen { State::Frozen } else { State::Open });
        self.suggest.widget.top = 0;
        self.focus_suggestion(if no_focus { None } else { Some(index) }, cx);
        cx.notify();
    }

    pub(super) fn hide_suggest_widget(&mut self) {
        self.set_suggest_widget_state(State::Hidden);
    }

    /// `_setState`.
    fn set_suggest_widget_state(&mut self, state: State) {
        let widget = &mut self.suggest.widget;
        if widget.state == state {
            return;
        }
        if !widget.details_read {
            widget.details_read = true;
            widget.details = self.profile_storage.lock().unwrap().get(EXPAND_DOCS_KEY) == Some("true");
        }
        let widget = &mut self.suggest.widget;
        widget.state = state;
        if matches!(state, State::Hidden | State::Loading | State::Empty) {
            widget.focused = None;
            widget.resolving = None;
            widget.top = 0;
            if state == State::Hidden {
                widget.loading = None;
            }
            self.suggest_item_focused(None);
        }
    }

    /// The session's items as last filtered.
    fn suggest_items(&self) -> &[CompletionItem] {
        match self.suggest.session.as_ref().and_then(|session| session.model.as_ref()) {
            Some(model) => model.filtered(),
            None => &[],
        }
    }

    /// `_onListFocus`: the focused item is revealed and resolved.
    fn focus_suggestion(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        let count = self.suggest_items().len();
        let index = index.filter(|index| *index < count);
        let widget = &mut self.suggest.widget;
        if widget.focused == index && index.is_some() {
            return;
        }
        widget.focused = index;
        widget.resolving = None;
        let Some(index) = index else {
            self.suggest_item_focused(None);
            return;
        };
        let visible = count.min(VISIBLE_ITEMS);
        if index < widget.top {
            widget.top = index;
        } else if index >= widget.top + visible {
            widget.top = index + 1 - visible;
        }
        let item = self.suggest_items()[index].clone();
        self.suggest_item_focused(Some(&item));
        if item.resolved {
            return;
        }
        let resolve = self.host.call_cancellable("ExtHostLanguageFeatures", "$resolveCompletionItem", vec![json!(item.provider), item.id.clone()]);
        self.suggest.widget.resolving = Some(cx.spawn(async move |workbench, cx| {
            let answer = resolve.await;
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.suggest.widget.resolving = None;
                let Some(model) = workbench.suggest.session.as_mut().and_then(|session| session.model.as_mut()) else { return };
                let Some(focused) = model.item_mut(index).filter(|focused| focused.label == item.label && focused.provider == item.provider) else { return };
                match answer {
                    Ok(Ok(resolved)) if resolved.is_object() => focused.merge_resolved(&resolved),
                    _ => focused.resolved = true,
                }
                let focused = focused.clone();
                workbench.suggest_item_focused(Some(&focused));
                cx.notify();
            });
        }));
    }

    pub(super) fn register_suggest_widget_commands(&mut self) {
        type Move = fn(usize, usize, usize, Option<usize>) -> Option<usize>;
        let moves: [(&str, Move); 6] = [
            ("selectNextSuggestion", |count, _, _, focused| Some(focused.map_or(0, |focused| (focused + 1) % count))),
            ("selectPrevSuggestion", |count, _, _, focused| Some(focused.map_or(count - 1, |focused| (focused + count - 1) % count))),
            ("selectFirstSuggestion", |_, _, _, _| Some(0)),
            ("selectLastSuggestion", |count, _, _, _| Some(count - 1)),
            // list.focusNextPage: the last item in view, else a page further.
            ("selectNextPageSuggestion", |count, top, visible, focused| {
                let last = (top + visible).min(count) - 1;
                Some(match focused {
                    Some(focused) if focused >= last => (focused + visible - 1).min(count - 1),
                    _ => last,
                })
            }),
            ("selectPrevPageSuggestion", |_, top, visible, focused| {
                Some(match focused {
                    Some(focused) if focused <= top => focused.saturating_sub(visible - 1),
                    _ => top,
                })
            }),
        ];
        for (id, step) in moves {
            self.commands.register(id, move |workbench, _, _, cx| {
                match workbench.suggest.widget.state {
                    State::Hidden | State::Loading | State::Empty => {}
                    State::Open | State::Frozen => {
                        let count = workbench.suggest_items().len();
                        if count > 0 {
                            let widget = &workbench.suggest.widget;
                            let next = step(count, widget.top, count.min(VISIBLE_ITEMS), widget.focused);
                            workbench.focus_suggestion(next, cx);
                            cx.notify();
                        }
                    }
                }
                crate::commands::Outcome::done()
            });
        }
        // `toggleDetails`: off always; on for a focused item with details or documentation, stored for the profile.
        self.commands.register("toggleSuggestionDetails", |workbench, _, _, cx| {
            let can_expand = matches!(workbench.suggest.widget.state, State::Open | State::Frozen) && workbench.focused_suggestion().is_some_and(CompletionItem::can_expand);
            let widget = &mut workbench.suggest.widget;
            if widget.details || can_expand {
                widget.details = !widget.details;
                let value = if widget.details { "true" } else { "false" };
                workbench.profile_storage.lock().unwrap().set(EXPAND_DOCS_KEY, value.into());
                cx.notify();
            }
            crate::commands::Outcome::done()
        });
        self.commands.register("focusSuggestion", |workbench, _, _, cx| {
            if workbench.suggest_items().len() > 0 {
                workbench.focus_suggestion(Some(0), cx);
                cx.notify();
            }
            crate::commands::Outcome::done()
        });
    }

    /// The suggest context keys of suggest.ts `Context` and the controller.
    pub(super) fn suggest_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        let Some(editor) = self.editors.active() else { return };
        let uri = editor.document.uri();
        let candidate = crate::language_features::Candidate { uri: &uri, language: &editor.document.language_id };
        context.insert("editorHasCompletionItemProvider".into(), json!(!self.language_features.ordered("$registerCompletionsProvider", &candidate).is_empty()));
        let accept_on_enter = self.suggest_setting(editor, "editor.acceptSuggestionOnEnter");
        context.insert("acceptSuggestionOnEnter".into(), json!(matches!(accept_on_enter.as_str(), Some("on" | "smart") | None)));
        context.insert("suggestionInsertMode".into(), self.suggest_setting(editor, "editor.suggest.insertMode"));
        let head = editor.selection().head;
        let line = editor.document.lines[head.line].clone();
        let smart = accept_on_enter == "smart";
        let visible = self.suggest_widget_visible();
        let auto = self.suggest.session.as_ref().is_some_and(|session| session.options.auto);
        context.insert("suggestWidgetVisible".into(), json!(visible));
        context.insert("suggestWidgetMultipleSuggestions".into(), json!(visible && self.suggest_items().len() > 1));
        let focused = self.suggest.widget.focused.filter(|_| visible).and_then(|index| self.suggest_items().get(index).cloned());
        context.insert("suggestWidgetHasFocusedSuggestion".into(), json!(focused.is_some()));
        context.insert("suggestWidgetDetailsVisible".into(), json!(self.suggest_details_visible()));
        if let Some(item) = focused {
            // The `onDidFocus` listener: with `smart`, an automatic suggestion that would only write what is typed makes no edit.
            let mut makes_text_edit = true;
            let typed = head.column.saturating_sub(item.edit_start.column);
            if smart && auto && item.additional_text_edits.is_null() && item.insert_text_rules & crate::suggest::INSERT_AS_SNIPPET == 0 && typed == item.insert_text.chars().count() {
                let old: String = line.chars().skip(item.edit_start.column).take(typed).collect();
                makes_text_edit = old != item.insert_text;
            }
            context.insert("suggestionMakesTextEdit".into(), json!(makes_text_edit));
            context.insert("suggestionHasInsertAndReplaceRange".into(), json!(item.edit_insert_end != item.edit_replace_end));
            let can_resolve = !item.resolved || !item.documentation.is_null() || item.detail.as_deref() != Some(&item.label);
            context.insert("suggestionCanResolve".into(), json!(can_resolve));
        }
    }

    /// The content widget at the cursor: below the line, or above when it fits better there (`_layout`).
    pub(super) fn suggest_widget(&self, editor: &OpenEditor, content_left: f32, char_width: f32, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.suggest.widget.state;
        if state == State::Hidden {
            return None;
        }
        let position = self.suggest.widget.position?;
        if self.suggest.session.as_ref().is_none_or(|session| session.model_id != editor.model_id) || editor.folding.is_hidden(position.line) {
            return None;
        }
        let text = editor.document.lines.get(position.line)?;
        let left = visible_column(text, position.column, editor.document.indentation.tab_size) as f32 * char_width - editor.scroll.left + content_left;
        let line_top = editor.line_top(editor.view_line(position.line), self.editors.viewport) - editor.scroll.top;
        let editor_bounds = self.text_area_bounds.get();
        let window_size = window.viewport_size().map(f32::from);
        let item_height = LINE_HEIGHT;
        let count = self.suggest_items().len();
        let message = matches!(state, State::Loading | State::Empty);
        // The persisted 430px width is the list's; the 1px borders come on top (432px measured in 1.147.0).
        let (width, height) = match message {
            true => (DEFAULT_WIDTH / 2. + 2., item_height + 2.),
            false => (DEFAULT_WIDTH + 2., count.min(VISIBLE_ITEMS) as f32 * item_height + 2.),
        };
        let cursor_bottom = editor_bounds.origin.y + line_top + LINE_HEIGHT;
        let max_below = (window_size.height - cursor_bottom - VERTICAL_PADDING).min(height);
        let max_above = (editor_bounds.origin.y + line_top - VERTICAL_PADDING).min(height);
        let above = !message && height > max_below && max_above > max_below;
        let height = if above { height.min(max_above.max(item_height + 2.)) } else { height.min(max_below.max(item_height + 2.)) };
        let top = if above { line_top - height } else { line_top + LINE_HEIGHT };
        let left = horizontal_in_page(left, width, editor_bounds, window_size);
        let high_contrast = self.theme.kind.is_high_contrast();
        let mut widget = div()
            .id("suggest-widget")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .h(px(height))
            .flex()
            .flex_col()
            .overflow_hidden()
            .border(px(if high_contrast { 2. } else { 1. }))
            .border_color(self.color("editorSuggestWidget.border"))
            .rounded(px(RADIUS))
            .bg(self.color("editorSuggestWidget.background"))
            .text_color(self.foreground_or("editorSuggestWidget.foreground"))
            .font_family(EDITOR_FONT)
            .text_size(px(EDITOR_FONT_SIZE))
            .line_height(px(item_height))
            .cursor_default()
            .occlude()
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .when(!high_contrast, |widget| {
                // `box-shadow: 0 2px 8px var(--vscode-widget-shadow)`; GPUI's blur radius is half the CSS blur.
                widget.shadow(vec![BoxShadow { color: self.color("widget.shadow").into(), offset: point(px(0.), px(2.)), blur_radius: px(4.), spread_radius: px(0.), inset: false }])
            });
        if message {
            let text = if state == State::Loading { "Loading..." } else { "No suggestions." };
            widget = widget.flex_row().items_center().child(div().pl(px(22.)).child(text));
        } else {
            widget = widget.child(self.suggest_rows(height - 2., cx));
        }
        let mut layer = div().absolute().top_0().left_0().child(widget);
        let anchor = gpui::Bounds { origin: gpui::point(left, top), size: gpui::size(width, height) };
        if let Some(details) = self.suggest_details(anchor, editor_bounds, window_size, high_contrast, editor, char_width, cx) {
            layer = layer.child(details);
        }
        Some(deferred(layer).with_priority(1).into_any_element())
    }

    /// `suggestWidgetVisible`'s details: shown with the docs expanded and a focused item that has any.
    pub(super) fn suggest_details_visible(&self) -> bool {
        self.suggest.widget.details && matches!(self.suggest.widget.state, State::Open | State::Frozen) && self.focused_suggestion().is_some_and(CompletionItem::can_expand)
    }

    fn focused_suggestion(&self) -> Option<&CompletionItem> {
        self.suggest.widget.focused.and_then(|index| self.suggest_items().get(index))
    }

    /// suggestWidgetDetails.ts: the detail in the editor font and the documentation, 330px wide, beside the list
    /// (`_placeAtAnchor`: east if it fits, else west, else south) at its top. `anchor` is the list in text area
    /// coordinates. shortcut: no north placement or bottom alignment above the line, and no resize sash; port them
    /// with the details' measured height.
    fn suggest_details(&self, anchor: gpui::Bounds<f32>, editor_bounds: gpui::Bounds<f32>, window_size: gpui::Size<f32>, high_contrast: bool, editor: &OpenEditor, char_width: f32, cx: &mut Context<Self>) -> Option<gpui::Stateful<gpui::Div>> {
        if !self.suggest_details_visible() {
            return None;
        }
        let item = self.focused_suggestion()?;
        let border = if high_contrast { 2. } else { 1. };
        const WIDTH: f32 = 330.;
        const HORIZONTAL_PADDING: f32 = 14.;
        let page_left = editor_bounds.origin.x + anchor.origin.x;
        let east = window_size.width - (page_left + anchor.size.width + border + HORIZONTAL_PADDING);
        let west = page_left - border - HORIZONTAL_PADDING;
        let (left, top) = if east >= WIDTH {
            (anchor.origin.x + anchor.size.width - border, anchor.origin.y)
        } else if west >= WIDTH {
            ((page_left - WIDTH - border).max(HORIZONTAL_PADDING) - editor_bounds.origin.x, anchor.origin.y)
        } else {
            (anchor.origin.x, anchor.origin.y + anchor.size.height - border)
        };
        let detail = item.detail.as_deref().filter(|detail| !detail.is_empty() && *detail != item.label).map(|detail| detail.chars().take(100_000).collect::<String>());
        let has_docs = !item.documentation.is_null();
        let docs = match &item.documentation {
            Value::String(text) => Some(div().px(px(5.)).py(px(4.)).when(detail.is_none(), |docs| docs.mr(px(24.))).whitespace_normal().child(text.clone())),
            Value::Object(_) => {
                let style = super::markdown_view::MarkdownStyle { font_size: EDITOR_FONT_SIZE, line_height: LINE_HEIGHT, foreground: self.foreground_or("editorSuggestWidget.foreground") };
                let blocks = crate::markdown::parse(&crate::markdown::MarkdownString::from_value(&item.documentation));
                Some(div().px(px(5.)).py(px(4.)).child(self.markdown_blocks("suggest-details", &blocks, style, &editor.document.language_id, cx)))
            }
            _ => None,
        };
        let close = self.icon("close", LINE_HEIGHT, 14., self.foreground_or("editorSuggestWidget.foreground"));
        let details = div()
            .id("suggest-details")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(WIDTH))
            .max_h(px(LINE_HEIGHT * 12.))
            .overflow_hidden()
            .border(px(border))
            .border_color(self.color("editorSuggestWidget.border"))
            .rounded(px(RADIUS))
            .bg(self.color("editorSuggestWidget.background"))
            .text_color(self.foreground_or("editorSuggestWidget.foreground"))
            .font_family(".SystemUIFont")
            .text_size(px(EDITOR_FONT_SIZE))
            .cursor_default()
            .occlude()
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .relative()
                    .child(div().absolute().top(px(6.)).right(px(2.)).cursor_pointer().child(close).on_mouse_down(MouseButton::Left, cx.listener(|workbench, _, window, cx| {
                        drop(workbench.execute_command("toggleSuggestionDetails", vec![], window, cx));
                    })))
                    .children(detail.map(|detail| {
                        // suggest.css `.type.auto-wrap`: `word-break: break-all` in the monospace editor font, so a line
                        // holds as many characters as fit in the 299px left of the borders, padding and close button.
                        let columns = ((WIDTH - 2. * border - 5. - 24.) / char_width).floor().max(1.) as usize;
                        let lines = break_all(&detail, columns).into_iter().map(|line| div().whitespace_nowrap().child(line));
                        div().font_family(EDITOR_FONT).opacity(0.7).mr(px(24.)).pl(px(5.)).pt(px(4.)).pb(px(if has_docs { 12. } else { 4. })).children(lines)
                    }))
                    .children(docs),
            );
        Some(details)
    }

    fn suggest_rows(&self, height: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let item_height = LINE_HEIGHT;
        let visible = ((height / item_height).floor() as usize).max(1);
        let (top, focused) = (self.suggest.widget.top, self.suggest.widget.focused);
        let frozen = self.suggest_widget_frozen();
        let items: Vec<CompletionItem> = self.suggest_items().iter().skip(top).take(visible).cloned().collect();
        let count = self.suggest_items().len();
        let mut list = div()
            .id("suggest-list")
            .flex()
            .flex_col()
            .size_full()
            .on_scroll_wheel(cx.listener(move |workbench, event: &ScrollWheelEvent, window, cx| {
                let delta = event.delta.pixel_delta(window.line_height()).y.to_f64() as f32;
                let rows = (-delta / item_height).round() as isize;
                let top = (workbench.suggest.widget.top as isize + rows).clamp(0, count.saturating_sub(visible) as isize) as usize;
                workbench.suggest.widget.top = top;
                cx.stop_propagation();
                cx.notify();
            }));
        for (offset, item) in items.into_iter().enumerate() {
            let index = top + offset;
            list = list.child(self.suggest_row(&item, index, focused == Some(index), frozen, cx));
        }
        list
    }

    /// suggestWidgetRenderer.ts `renderElement`.
    fn suggest_row(&self, item: &CompletionItem, index: usize, focused: bool, frozen: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let foreground = if focused { self.foreground_or("editorSuggestWidget.selectedForeground") } else { self.foreground_or("editorSuggestWidget.foreground") };
        let (codicon, icon_color) = completion_kind_icon(item.kind);
        let icon_color = match (focused, icon_color) {
            (true, _) => self.theme.color("editorSuggestWidget.selectedIconForeground").unwrap_or(foreground),
            (false, Some(key)) => self.foreground_or(key),
            (false, None) => foreground,
        };
        let highlight = if focused { self.color("editorSuggestWidget.focusHighlightForeground") } else { self.color("editorSuggestWidget.highlightForeground") };
        let matches = if item.deprecated { Vec::new() } else { create_matches(Some(&item.score)) };
        let label = item.label.replace(['\r', '\n'], "");
        let mut styles = Vec::new();
        for range in matches {
            let start = label.char_indices().nth(range.start).map_or(label.len(), |(offset, _)| offset);
            let end = label.char_indices().nth(range.end).map_or(label.len(), |(offset, _)| offset);
            styles.push((start..end, HighlightStyle { color: Some(highlight.into()), font_weight: (!frozen).then_some(FontWeight::BOLD), ..Default::default() }));
        }
        let (signature, details) = match item.string_label {
            true => (None, item.detail.clone()),
            false => (item.label_detail.clone(), item.label_description.clone()),
        };
        let show_details = !item.string_label || (focused && !self.suggest_details_visible());
        let label = div().flex_none().when(item.deprecated, |label| label.opacity(0.66).line_through()).child(StyledText::new(label).with_highlights(styles));
        let left = div()
            .flex()
            .flex_grow(1.)
            .flex_shrink(1.)
            .overflow_hidden()
            .child(div().flex().items_center().mr(px(4.)).child(self.icon(codicon, 16., 16., icon_color)))
            .child(label)
            .children(signature.map(|signature| div().overflow_hidden().text_ellipsis().opacity(0.6).child(signature.replace(['\r', '\n'], ""))));
        let right = div()
            .flex()
            .flex_shrink(4.)
            .max_w(gpui::relative(0.7))
            .overflow_hidden()
            .children(details.filter(|_| show_details).map(|details| div().ml(px(EDITOR_FONT_SIZE * 0.85 * 1.1)).text_size(px(EDITOR_FONT_SIZE * 0.85)).overflow_hidden().text_ellipsis().whitespace_nowrap().text_color(foreground).child(details.replace(['\r', '\n'], ""))));
        div()
            .id(("suggest-row", index))
            .flex()
            .flex_none()
            .h(px(LINE_HEIGHT))
            .pr(px(10.))
            .whitespace_nowrap()
            .cursor_pointer()
            .text_color(foreground)
            .when(focused, |row| {
                let outline = self.theme.color("editorSuggestWidget.focusOutline");
                row.bg(self.color("editorSuggestWidget.selectedBackground")).when_some(outline, |row, outline| row.border_1().border_color(outline))
            })
            .when(!focused, |row| row.hover(|row| row.bg(self.color("list.hoverBackground"))))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _, window, cx| {
                // `onDidSelect` of a click: the item is focused and inserted.
                workbench.focus_suggestion(Some(index), cx);
                workbench.insert_suggestion(index, InsertFlags::default(), window, cx);
            }))
            .child(div().flex_1().h_full().overflow_hidden().pl(px(2.)).child(div().flex().justify_between().overflow_hidden().child(left).child(right)))
    }
}

/// suggestWidgetDetails.ts `auto-wrap`: a detail without indented lines is laid out as `white-space: normal` with
/// `word-break: break-all` (runs of whitespace collapse, lines break after `columns` characters); one with them keeps
/// its lines as `pre-wrap`.
fn break_all(detail: &str, columns: usize) -> Vec<String> {
    let lines: Vec<&str> = detail.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)).collect();
    if lines.iter().skip(1).any(|line| line.starts_with(char::is_whitespace)) {
        return lines.into_iter().map(String::from).collect();
    }
    let collapsed = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut result = Vec::new();
    let mut line = String::new();
    for character in collapsed.chars() {
        if line.chars().count() == columns {
            result.push(std::mem::take(&mut line));
            if character == ' ' {
                continue;
            }
        }
        line.push(character);
    }
    result.push(line);
    result
}

#[cfg(test)]
mod tests;
