//! messageController.ts `MessageController`: a message above the cursor (the `messageoverlay` content widget, which
//! may overflow the editor), closed by a cursor move, the editor text losing the focus, another model, the mouse
//! leaving the lines around it, or Escape (`leaveEditorMessage`).

use std::ops::Range;
use std::time::Duration;

use gpui::{Animation, AnimationExt, Bounds, Context, Div, InteractiveElement, IntoElement, ParentElement, PathBuilder, Size, Stateful, StatefulInteractiveElement, Styled, Task, TextRun, Window, canvas, deferred, div, point, px};
use serde_json::Value;

use super::Workbench;
use crate::editor::{OpenEditor, Position, VerticalReveal, visible_column};

pub struct MessagePart {
    text: &'static str,
    command: Option<(&'static str, Vec<Value>)>,
}

impl MessagePart {
    pub fn text(text: &'static str) -> Self {
        MessagePart { text, command: None }
    }

    /// A `command:` link of trusted markdown: a click closes the message and runs the command.
    pub fn link(text: &'static str, command: &'static str, args: Vec<Value>) -> Self {
        MessagePart { text, command: Some((command, args)) }
    }
}

#[derive(Default)]
pub(super) struct EditorMessages {
    current: Option<EditorMessage>,
    /// `MessageWidget.fadeOut`: the closed message fades for 100ms before it goes.
    closing: Option<(EditorMessage, Task<()>)>,
    shown: usize,
}

struct EditorMessage {
    parts: Vec<MessagePart>,
    model_id: u32,
    position: Position,
    /// The cursor positions at show; a change closes the message (`onDidChangeCursorPosition`).
    cursors: Vec<Position>,
    /// The model lines from 3 above the message's to 3 below the first line the mouse moved over.
    mouse_bounds: Option<Range<usize>>,
    mouse_over: bool,
    serial: usize,
}

/// messageController.css: 8px for the anchor arrow beside a 1px bordered box with 2px by 4px padding.
const ANCHOR_HEIGHT: f32 = 8.;
const MESSAGE_LINE_HEIGHT: f32 = 18.;
const MESSAGE_HEIGHT: f32 = MESSAGE_LINE_HEIGHT + 2. * 2. + 2.;
const UI_FONT: &str = ".SystemUIFont";

impl Workbench {
    /// `showMessage` at the primary cursor, revealing its line in the center when it is outside the viewport.
    /// shortcut: GPUI has no screen reader announcements, so the message is not read out (`alert`); add one with them.
    pub(super) fn show_editor_message(&mut self, parts: Vec<MessagePart>, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut() else { return };
        let position = editor.selection().head;
        let cursors = editor.selections.iter().map(|selection| selection.head).collect();
        let scroll = editor.scroll;
        editor.reveal_range(position, position, VerticalReveal::CenterIfOutsideViewport, viewport);
        let (model_id, scrolled) = (editor.model_id, editor.scroll != scroll);
        self.editor_message.shown += 1;
        self.editor_message.closing = None;
        self.editor_message.current = Some(EditorMessage { parts, model_id, position, cursors, mouse_bounds: None, mouse_over: false, serial: self.editor_message.shown });
        if scrolled {
            self.editor_scrolled(cx);
        }
        cx.notify();
    }

    /// `messageVisible`.
    pub(super) fn editor_message_visible(&self) -> bool {
        self.editor_message.current.is_some()
    }

    /// `closeMessage`.
    pub(super) fn close_editor_message(&mut self, cx: &mut Context<Self>) {
        let Some(message) = self.editor_message.current.take() else { return };
        let fade = cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(Duration::from_millis(100)).await;
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.editor_message.closing = None;
                cx.notify();
            });
        });
        self.editor_message.closing = Some((message, fade));
        cx.notify();
    }

    /// Closes the message once a cursor moved or the active editor shows another model.
    pub(super) fn close_editor_message_on_cursor_change(&mut self, cx: &mut Context<Self>) {
        let Some(message) = &self.editor_message.current else { return };
        let moved = self.editors.active().is_none_or(|editor| editor.model_id != message.model_id || !editor.selections.iter().map(|selection| selection.head).eq(message.cursors.iter().copied()));
        if moved {
            self.close_editor_message(cx);
        }
    }

    /// `onDidBlurEditorText`, unless the mouse is over the message.
    pub(super) fn editor_message_text_blurred(&mut self, cx: &mut Context<Self>) {
        if self.editor_message.current.as_ref().is_some_and(|message| !message.mouse_over) {
            self.close_editor_message(cx);
        }
    }

    /// `onMouseMove` over a model line: the first move sets the bounds, a move outside them closes the message.
    pub(super) fn editor_message_mouse_moved(&mut self, line: usize, cx: &mut Context<Self>) {
        let Some(message) = self.editor_message.current.as_mut() else { return };
        match &message.mouse_bounds {
            None => message.mouse_bounds = Some(message.position.line.saturating_sub(3)..line + 3),
            Some(bounds) if !bounds.contains(&line) => self.close_editor_message(cx),
            Some(_) => {}
        }
    }

    pub(super) fn register_editor_message_commands(&mut self) {
        self.commands.register("leaveEditorMessage", |workbench, _, _, cx| {
            workbench.close_editor_message(cx);
            crate::commands::Outcome::done()
        });
    }

    /// The message widget of the text area: above the cursor, or below it when the window has no room above.
    pub(super) fn editor_message_widget(&self, editor: &OpenEditor, content_left: f32, char_width: f32, window: &Window, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let (message, closing) = match (&self.editor_message.current, &self.editor_message.closing) {
            (Some(message), _) => (message, false),
            (None, Some((message, _))) => (message, true),
            (None, None) => return None,
        };
        if message.model_id != editor.model_id || editor.folding.is_hidden(message.position.line) {
            return None;
        }
        let text = &editor.document.lines[message.position.line];
        let anchor_left = visible_column(text, message.position.column, editor.document.indentation.tab_size) as f32 * char_width;
        let anchor_top = editor.line_top(editor.view_line(message.position.line), self.editors.viewport) - editor.scroll.top;
        let text: String = message.parts.iter().map(|part| part.text).collect();
        let run = TextRun { len: text.len(), font: gpui::font(UI_FONT), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
        let text_width = f32::from(window.text_system().shape_line(text.into(), px(13.), &[run], None).width);
        let width = text_width + 2. * 4. + 2.;
        let layout = layout_in_page(anchor_left - editor.scroll.left + content_left, anchor_top, self.editors.viewport.line_height, width, self.text_area_bounds.get(), window.viewport_size().map(f32::from));
        let widget = self.message_box(message, layout.below, cx);
        // messageController.ts: `marginLeft = -6px` puts the anchor arrow's tip 4px right of the cursor.
        let widget = div().absolute().left(px(layout.left - 6.)).top(px(layout.top)).child(widget);
        let widget = match closing {
            false => widget.with_animation(("editor-message-in", message.serial), Animation::new(Duration::from_millis(150)).with_easing(ease_out), |widget, delta| widget.opacity(delta)).into_any_element(),
            true => widget.with_animation(("editor-message-out", message.serial), Animation::new(Duration::from_millis(100)).with_easing(ease_out), |widget, delta| widget.opacity(1. - delta)).into_any_element(),
        };
        Some(deferred(widget).with_priority(1))
    }

    fn message_box(&self, message: &EditorMessage, below: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let border = self.color("inputValidation.infoBorder");
        let link = self.color("textLink.foreground");
        let link_hover = self.color("textLink.activeForeground");
        let mut text = div().flex().whitespace_nowrap();
        for (index, part) in message.parts.iter().enumerate() {
            let Some((command, args)) = part.command.clone() else {
                text = text.child(part.text);
                continue;
            };
            text = text.child(div().id(("editor-message-link", index)).cursor_pointer().text_color(link).hover(move |style| style.text_color(link_hover)).child(part.text).on_click(cx.listener(
                move |workbench, _, window, cx| {
                    workbench.close_editor_message(cx);
                    workbench.execute_command(command, args.clone(), window, cx);
                },
            )));
        }
        let high_contrast = self.theme.kind.is_high_contrast();
        let boxed = div()
            .px(px(4.))
            .py(px(2.))
            .font_family(UI_FONT)
            .line_height(px(MESSAGE_LINE_HEIGHT))
            .text_size(px(13.))
            .rounded(px(3.))
            .bg(self.color("editorHoverWidget.background"))
            .text_color(self.foreground_or("editorHoverWidget.foreground"))
            .border_color(border)
            .border(px(if high_contrast { 2. } else { 1. }))
            .child(text);
        let arrow = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                // The 8px CSS border triangle of `.anchor`: pointing down under the box, up over it when below.
                let (x, y) = (bounds.origin.x, bounds.origin.y);
                let (tip, base) = if below { (y, y + px(ANCHOR_HEIGHT)) } else { (y + px(ANCHOR_HEIGHT), y) };
                let mut path = PathBuilder::fill();
                path.move_to(point(x, base));
                path.line_to(point(x + px(16.), base));
                path.line_to(point(x + px(8.), tip));
                path.close();
                if let Ok(path) = path.build() {
                    window.paint_path(path, border);
                }
            },
        )
        .ml(px(2.))
        .w(px(16.))
        .h(px(ANCHOR_HEIGHT));
        let widget = div().id("messageoverlay").flex().flex_col().on_hover(cx.listener(|workbench, hovered: &bool, _, _| {
            if let Some(message) = workbench.editor_message.current.as_mut() {
                message.mouse_over = *hovered;
            }
        }));
        match below {
            true => widget.child(arrow).child(boxed),
            false => widget.child(boxed).child(arrow),
        }
    }
}

fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(2)
}

#[derive(Debug, PartialEq)]
pub(super) struct MessageLayout {
    pub left: f32,
    pub top: f32,
    pub below: bool,
}

/// contentWidgets.ts `_layoutBoxInPage` and `_layoutHorizontalSegmentInPage` for the message, preferring ABOVE
/// then BELOW: the widget shifts to stay 15px inside the window and within its own width of the editor, and goes
/// below the line when its top would be less than 22px from the window's top. `left` is from the editor's left,
/// `anchor_top` the line's top in the editor and `line_height` its height, `editor` the editor's window bounds.
pub(super) fn layout_in_page(left: f32, anchor_top: f32, line_height: f32, width: f32, editor: Bounds<f32>, window: Size<f32>) -> MessageLayout {
    let height = MESSAGE_HEIGHT + ANCHOR_HEIGHT;
    let left = horizontal_in_page(left, width, editor, window);
    let above_top = anchor_top - height;
    let fits_above = editor.origin.y + above_top >= 22.;
    let below_top = anchor_top + line_height;
    let fits_below = editor.origin.y + below_top + height <= window.height - 22.;
    match fits_above || !fits_below {
        true => MessageLayout { left, top: above_top, below: false },
        false => MessageLayout { left, top: below_top, below: true },
    }
}

/// contentWidgets.ts `_layoutHorizontalSegmentInPage`: `left` from the editor's left, shifted to keep the widget
/// 15px inside the window and within its own width of the editor.
pub(super) fn horizontal_in_page(left: f32, width: f32, editor: Bounds<f32>, window: Size<f32>) -> f32 {
    let min_limit = 15f32.max(editor.origin.x - width);
    let max_limit = (editor.origin.x + editor.size.width + width).min(window.width - 15.);
    let mut absolute_left = editor.origin.x + left;
    let mut left = left;
    if absolute_left + width > max_limit {
        let delta = absolute_left - (max_limit - width);
        absolute_left -= delta;
        left -= delta;
    }
    if absolute_left < min_limit {
        left -= absolute_left - min_limit;
    }
    left
}
