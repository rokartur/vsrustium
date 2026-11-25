//! The sticky scroll widget (stickyScrollWidget.ts, stickyScroll.css) with its controller's mouse and keyboard
//! handling (stickyScrollController.ts) and actions (stickyScrollActions.ts).

use gpui::{BoxShadow, ClickEvent, Context, Div, FocusHandle, ModifiersChangedEvent, MouseButton, MouseMoveEvent, Rgba, Stateful, StyledText, Window, div, point, prelude::*, px};
use serde_json::json;

use super::{DECORATIONS_WIDTH, EditorMetrics, FoldControls, Workbench};
use crate::commands::Outcome;
use crate::editor::{OpenEditor, Position, Reveal, Selection, VERTICAL_SCROLLBAR_WIDTH, expand_tabs, visible_column};

const ENABLED: &str = "editor.stickyScroll.enabled";

/// The controller's state: keyboard focus on a line, the line that shows its scope's end, the folding icons' fade.
pub(in crate::workbench) struct StickyScroll {
    focus: FocusHandle,
    /// `_focusedStickyElementIndex`.
    focused_index: usize,
    /// The sticky line under the pointer, for Shift pressed without a move.
    hovered_index: Option<usize>,
    /// `_showEndForLine`, with the scroll top it was set at: a scroll resets it.
    show_end_for_line: Option<(usize, f32)>,
    /// `_isOnGlyphMargin`: the expanded folding icons show while the pointer is over the line numbers.
    fold_icons: FoldControls,
}

impl StickyScroll {
    pub(in crate::workbench) fn new(cx: &mut Context<Workbench>) -> Self {
        StickyScroll { focus: cx.focus_handle(), focused_index: 0, hovered_index: None, show_end_for_line: None, fold_icons: FoldControls::default() }
    }

    pub(in crate::workbench) fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }
}

/// A rendered sticky line: its 1-based model line and the end line a Shift click reveals.
struct StickyLine {
    line: usize,
    end_line: usize,
}

impl Workbench {
    /// `editor.stickyScroll.maxLineCount` (1 to 20) with sticky scroll on, else 0.
    pub(super) fn sticky_scroll_lines(&self) -> usize {
        if self.setting(ENABLED) != json!(true) {
            return 0;
        }
        self.setting("editor.stickyScroll.maxLineCount").as_u64().map_or(5, |count| count.clamp(1, 20) as usize)
    }

    /// `_findRenderingData`: the lines the widget shows, with the one under Shift showing its scope's end, and
    /// how far the last one is pushed up.
    fn sticky_lines(&self, editor: &OpenEditor) -> (Vec<StickyLine>, f32) {
        let state = editor.sticky_scroll_state(self.editors.viewport);
        let show_end = self.sticky_scroll.show_end_for_line.filter(|(_, top)| *top == editor.scroll.top).map(|(index, _)| index);
        let mut lines = Vec::new();
        for (index, (&start, &end)) in state.start_line_numbers.iter().zip(&state.end_line_numbers).enumerate() {
            let line = if show_end == Some(index) { end } else { start };
            if line <= editor.document().lines.len() {
                lines.push(StickyLine { line, end_line: end });
            }
        }
        (lines, state.last_line_relative_position)
    }

    /// `stickyScrollVisible` and `stickyScrollFocused`.
    pub(in crate::workbench) fn sticky_scroll_context(&self, window: &Window, context: &mut std::collections::HashMap<String, serde_json::Value>) {
        let visible = self.editors.active().is_some_and(|editor| !self.sticky_lines(editor).0.is_empty());
        context.insert(format!("config.{ENABLED}"), self.setting(ENABLED));
        context.insert("stickyScrollVisible".into(), json!(visible));
        context.insert("stickyScrollFocused".into(), json!(self.sticky_scroll.is_focused(window)));
    }

    /// The widget over the top of the text area: line numbers in the gutter color with the folding icons, then
    /// each line's text; the last line slides up under the others as its scope ends.
    pub(super) fn sticky_scroll_widget(&self, editor: &OpenEditor, metrics: EditorMetrics, window: &mut Window, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let (lines, last_line_relative_position) = self.sticky_lines(editor);
        if lines.is_empty() {
            return None;
        }
        let height = lines.len() as f32 * metrics.line_height + last_line_relative_position;
        let now = std::time::Instant::now();
        if self.sticky_scroll.fold_icons.in_transition(now) {
            window.request_animation_frame();
        }
        let fold_icon_opacity = self.sticky_scroll.fold_icons.opacity(now);
        let focused_index = self.sticky_scroll.is_focused(window).then_some(self.sticky_scroll.focused_index);
        let scroll_left = if self.setting("editor.stickyScroll.scrollWithEditor") == json!(false) { 0. } else { editor.scroll.left };
        let gutter = self.color("editorStickyScrollGutter.background");
        let background = self.color("editorStickyScroll.background");
        let hover = self.color("editorStickyScrollHover.background");
        let number_color = self.color("editorLineNumber.foreground");
        let text_color = self.color("editor.foreground");
        let fold_color = self.color("editorGutter.foldingControlForeground");
        let focus_border = self.color("focusBorder");
        let tab_size = editor.document().indentation.tab_size;
        let (mut numbers, mut contents) = (Vec::new(), Vec::new());
        // The last line has z-index 0 under the others' 1, so it goes first.
        let last = lines.len() - 1;
        for index in std::iter::once(last).chain(0..last) {
            let StickyLine { line, end_line } = lines[index];
            let region = editor.folding.region_at_line(line).filter(|&region| editor.folding.regions.get(region).start == line);
            let collapsed = region.map(|region| editor.folding.regions.is_collapsed(region));
            let mut top = index as f32 * metrics.line_height;
            if index == last {
                top += last_line_relative_position + if collapsed == Some(true) { 1. } else { 0. };
            }
            let hovered = move |workbench: &mut Workbench, hovered: &bool, cx: &mut Context<Workbench>| {
                let hovered_index = &mut workbench.sticky_scroll.hovered_index;
                match hovered {
                    true => *hovered_index = Some(index),
                    false if *hovered_index == Some(index) => *hovered_index = None,
                    false => {}
                }
                cx.notify();
            };
            let mut number = div()
                .id(("sticky-number", index))
                .absolute()
                .top(px(top))
                .left_0()
                .w(px(metrics.content_left()))
                .h(px(metrics.line_height))
                .bg(gutter)
                .text_color(number_color)
                .on_hover(cx.listener(move |workbench, is_hovered: &bool, _, cx| hovered(workbench, is_hovered, cx)))
                .on_mouse_move(cx.listener(move |workbench, event: &MouseMoveEvent, _, cx| workbench.show_sticky_end(index, event.modifiers.shift, cx)))
                .on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| workbench.click_sticky_line(end_line, None, event, window, cx)))
                .child(div().absolute().left(px(metrics.glyph_margin_width())).w(px(metrics.line_numbers_width)).text_right().child(line.to_string()));
            if let Some(collapsed) = collapsed {
                // StickyFoldingIcon.setVisible: a collapsed icon always shows, an expanded one while over the gutter.
                let opacity = if collapsed { 1. } else { fold_icon_opacity };
                let name = if collapsed { "chevron-right" } else { "chevron-down" };
                let icon = self.icon(name, metrics.line_height, 1.4 * metrics.font_size, Rgba { a: fold_color.a * opacity, ..fold_color });
                let visible = collapsed || self.sticky_scroll.fold_icons.shown;
                number = number.child(
                    div()
                        .id(("sticky-fold", index))
                        .absolute()
                        .left(px(metrics.glyph_margin_width() + metrics.line_numbers_width + 2.))
                        .w(px(DECORATIONS_WIDTH))
                        .h(px(metrics.line_height))
                        .flex()
                        .justify_center()
                        .when(visible, |icon| icon.cursor_pointer())
                        .child(icon)
                        .on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            workbench.click_sticky_fold_icon(line, index, end_line, event, window, cx);
                        })),
                );
            }
            numbers.push(number);
            let text = &editor.document().lines[line - 1];
            let highlights = editor.document().tokens.highlights(&self.theme, &self.languages, &editor.document().lines, line - 1, tab_size);
            let span_starts = editor.document().tokens.span_starts(&self.theme, &self.languages, &editor.document().lines, line - 1);
            let column_at = move |workbench: &Workbench, event: &ClickEvent| {
                let x = f32::from(event.position().x) - workbench.text_area_left.get() - metrics.content_left() + scroll_left;
                let editor = workbench.editors.active().expect("sticky lines show only with an editor open");
                // An edit from the host can shrink the document between this frame and the click.
                let document = editor.document();
                let Some(text) = document.lines.get(line - 1) else { return 0 };
                span_column(text, &span_starts, x / metrics.char_width, tab_size)
            };
            let mut content = div()
                .id(("sticky-line", index))
                .absolute()
                .top(px(top))
                .left_0()
                .right_0()
                .h(px(metrics.line_height))
                .overflow_hidden()
                .bg(background)
                .hover(|content| content.bg(hover))
                .cursor_pointer()
                .text_color(text_color)
                .on_hover(cx.listener(move |workbench, is_hovered: &bool, _, cx| hovered(workbench, is_hovered, cx)))
                .on_mouse_move(cx.listener(move |workbench, event: &MouseMoveEvent, _, cx| workbench.show_sticky_end(index, event.modifiers.shift, cx)))
                .on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| {
                    let column = column_at(workbench, event);
                    workbench.click_sticky_line(end_line, Some(Position::new(line - 1, column)), event, window, cx);
                }))
                .child(div().absolute().top_0().left(px(-scroll_left)).whitespace_nowrap().child(StyledText::new(expand_tabs(text, tab_size)).with_highlights(highlights)));
            // workbench.css `[tabindex="0"]:focus`: a 1px focusBorder outline inside the focused line.
            if focused_index == Some(index) {
                content = content.child(div().absolute().inset_0().border_1().border_color(focus_border));
            }
            contents.push(content);
        }
        let border = self.theme.color("editorStickyScroll.border");
        let shadow = self.theme.color("editorStickyScroll.shadow");
        let widget = div()
            .id("sticky-widget")
            .absolute()
            .top_0()
            .left_0()
            .right(px(VERTICAL_SCROLLBAR_WIDTH))
            .h(px(height + if border.is_some() { 1. } else { 0. }))
            .overflow_hidden()
            .track_focus(&self.sticky_scroll.focus)
            .block_mouse_except_scroll()
            // stickyScroll.css: `box-shadow: var(--vscode-editorStickyScroll-shadow) 0 4px 2px -2px`; CSS blur 2px is sigma 1px.
            .when_some(shadow, |widget, shadow| {
                widget.shadow(vec![BoxShadow { color: shadow.into(), offset: point(px(0.), px(4.)), blur_radius: px(1.), spread_radius: px(-2.), inset: false }])
            })
            // _onMouseDown: a press keeps the focus in the editor, as the reveal that follows does.
            .on_mouse_down(MouseButton::Left, cx.listener(|workbench, _, window, cx| window.focus(&workbench.editor_focus, cx)))
            .child(
                div()
                    .id("sticky-line-numbers")
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(px(metrics.content_left()))
                    .h(px(height))
                    .bg(gutter)
                    .on_hover(cx.listener(|workbench, hovered: &bool, _, cx| {
                        workbench.sticky_scroll.fold_icons.set_shown(*hovered, std::time::Instant::now());
                        cx.notify();
                    }))
                    .children(numbers),
            )
            .child(div().absolute().top_0().left(px(metrics.content_left())).right_0().h(px(height)).bg(background).children(contents))
            .when_some(border, |widget, border| widget.child(div().absolute().left_0().right_0().top(px(height)).h(px(1.)).bg(border)));
        Some(widget)
    }

    /// `_onMouseMoveOrKeyDown`: with Shift held over a sticky line, the line shows its scope's end.
    fn show_sticky_end(&mut self, index: usize, shift: bool, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let show_end = Some((index, editor.scroll.top));
        if shift && self.sticky_scroll.show_end_for_line != show_end {
            self.sticky_scroll.show_end_for_line = show_end;
            cx.notify();
        }
    }

    /// Shift pressed over a sticky line shows its end; any release goes back to the starts (the KEY_UP listener).
    pub(super) fn sticky_modifiers_changed(&mut self, event: &ModifiersChangedEvent, _: &mut Window, cx: &mut Context<Self>) {
        match (event.modifiers.shift, self.sticky_scroll.hovered_index) {
            (true, Some(index)) => self.show_sticky_end(index, true, cx),
            (false, _) if self.sticky_scroll.show_end_for_line.is_some() => {
                self.sticky_scroll.show_end_for_line = None;
                cx.notify();
            }
            _ => {}
        }
    }

    /// The widget's CLICK listener: only a plain left click acts; with Shift it reveals the scope's end in the
    /// center, on text it goes to the clicked span's start, elsewhere on the line to the line's start.
    fn click_sticky_line(&mut self, end_line: usize, position: Option<Position>, event: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.modifiers();
        if modifiers.control || modifiers.alt || modifiers.platform || !event.standard_click() {
            return;
        }
        if modifiers.shift {
            let end = Position::new(end_line - 1, 0);
            self.reveal_from_sticky(end, Reveal::CenterIfOutside(end), window, cx);
            return;
        }
        if let Some(position) = position {
            self.reveal_from_sticky(position, Reveal::Position(position), window, cx);
        }
    }

    /// `_toggleFoldingRegionForLine`, or the Shift click of the line the icon is on.
    fn click_sticky_fold_icon(&mut self, line: usize, index: usize, end_line: usize, event: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.modifiers();
        if modifiers.shift || modifiers.control || modifiers.alt || modifiers.platform || !event.standard_click() {
            return self.click_sticky_line(end_line, None, event, window, cx);
        }
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("sticky lines show only with an editor open");
        let before = (editor.selections.clone(), editor.folding.hidden().to_vec());
        if editor.toggle_sticky_fold(line, index, viewport) {
            self.cursors_changed(before.0, Reveal::Nothing, "api", cx);
            self.folds_changed(&before.1);
            self.editor_scrolled(cx);
        }
    }

    /// `_reveaInEditor`: the sticky focus ends, the position is revealed and selected, the editor focused.
    fn reveal_from_sticky(&mut self, position: Position, reveal: Reveal, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.editors.active_mut().expect("sticky lines show only with an editor open");
        let before = editor.selections.clone();
        editor.set_selections(vec![Selection::caret(position)]);
        self.cursors_changed(before, reveal, "api", cx);
        window.focus(&self.editor_focus, cx);
    }

    pub(in crate::workbench) fn register_sticky_scroll_commands(&mut self) {
        self.commands.register("editor.action.toggleStickyScroll", |workbench, _, window, cx| {
            if workbench.editors.active().is_none() {
                return Outcome::done();
            }
            let enabled = workbench.setting(ENABLED) == json!(true);
            let focused = workbench.sticky_scroll.is_focused(window);
            let mut settings = workbench.settings.lock().unwrap();
            match settings.update_value_derived(ENABLED, Some(&json!(!enabled))) {
                Ok(change) if !change.is_empty() => workbench.host.send_configuration_change(&settings, &change),
                Ok(_) => {}
                Err(error) => return Outcome::error(format!("writing {ENABLED}: {error}")),
            }
            drop(settings);
            if focused {
                window.focus(&workbench.editor_focus, cx);
            }
            cx.notify();
            Outcome::done()
        });
        self.commands.register("editor.action.focusStickyScroll", |workbench, _, window, cx| {
            let Some(editor) = workbench.editors.active() else { return Outcome::done() };
            let count = workbench.sticky_lines(editor).0.len();
            // focus(): an already focused widget keeps its line.
            if count > 0 && !workbench.sticky_scroll.is_focused(window) {
                workbench.sticky_scroll.focused_index = count - 1;
                window.focus(&workbench.sticky_scroll.focus, cx);
                cx.notify();
            }
            Outcome::done()
        });
        self.commands.register("editor.action.selectNextStickyScrollLine", |workbench, _, window, cx| {
            let Some(editor) = workbench.editors.active() else { return Outcome::done() };
            let count = workbench.sticky_lines(editor).0.len();
            if workbench.sticky_scroll.is_focused(window) && workbench.sticky_scroll.focused_index + 1 < count {
                workbench.sticky_scroll.focused_index += 1;
                cx.notify();
            }
            Outcome::done()
        });
        self.commands.register("editor.action.selectPreviousStickyScrollLine", |workbench, _, window, cx| {
            if workbench.sticky_scroll.is_focused(window) && workbench.sticky_scroll.focused_index > 0 {
                workbench.sticky_scroll.focused_index -= 1;
                cx.notify();
            }
            Outcome::done()
        });
        self.commands.register("editor.action.goToFocusedStickyScrollLine", |workbench, _, window, cx| {
            let Some(editor) = workbench.editors.active() else { return Outcome::done() };
            if !workbench.sticky_scroll.is_focused(window) {
                return Outcome::done();
            }
            let lines = workbench.sticky_lines(editor).0;
            if let Some(sticky) = lines.get(workbench.sticky_scroll.focused_index) {
                let position = Position::new(sticky.line - 1, 0);
                workbench.reveal_from_sticky(position, Reveal::Position(position), window, cx);
            }
            Outcome::done()
        });
        self.commands.register("editor.action.selectEditor", |workbench, _, window, cx| {
            if workbench.sticky_scroll.is_focused(window) {
                window.focus(&workbench.editor_focus, cx);
                cx.notify();
            }
            Outcome::done()
        });
    }
}

/// `getColumnOfNodeOffset` at the span's start: the 0-based column where the span under `visible` starts, or 0
/// past the text, where the click hits the line's element instead of a span.
fn span_column(text: &str, span_starts: &[usize], visible: f32, tab_size: usize) -> usize {
    let mut byte_under = None;
    for (char_index, (byte, _)) in text.char_indices().enumerate() {
        if visible < visible_column(text, char_index + 1, tab_size) as f32 {
            byte_under = Some(byte);
            break;
        }
    }
    let Some(byte) = byte_under.filter(|_| visible >= 0.) else { return 0 };
    let start = span_starts.iter().rev().find(|&&start| start <= byte).copied().unwrap_or(0);
    text.get(..start).map_or(0, |before| before.chars().count())
}

#[cfg(test)]
mod tests {
    use super::span_column;

    #[test]
    fn a_click_goes_to_the_start_of_the_span_under_it() {
        let text = "\tfn merge(a) {";
        let starts = [0, 1, 3, 4, 9, 10, 11, 12, 13];
        assert_eq!(span_column(text, &starts, 0.5, 4), 0);
        assert_eq!(span_column(text, &starts, 9.5, 4), 4);
        assert_eq!(span_column(text, &starts, 4.2, 4), 1);
        assert_eq!(span_column(text, &starts, 40., 4), 0);
    }
}
