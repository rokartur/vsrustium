//! commentThreadZoneWidget.ts, commentThreadWidget.ts, commentNode.ts and commentReply.ts: an expanded thread as a
//! zone below its range's last line. The frame and arrow take the thread state's color, the head shows the label
//! and the `comments/commentThread/title` actions, then each comment, then the reply form whose buttons are the
//! `comments/commentThread/context` items; each comment is drawn by comment_node.rs. shortcut: the reply is a
//! single-line input, not an editor, one at a time with a comment's edit form, and the range is not highlighted;
//! upgrade with an editor input.

use std::collections::HashMap;

use gpui::{AnyElement, Context, KeyDownEvent, MouseButton, PathBuilder, Rgba, Window, canvas, div, point, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::comment_node::{REACTIONS_ROW, comment_text};
use super::comments::Thread;
use super::hover::{HoverContent, HoverOptions};
use super::input_box_view::{InputId, InputLook};
use super::peek_view::UI_FONT;
use crate::editor::{OpenEditor, Position, VERTICAL_SCROLLBAR_WIDTH, visible_column};
use crate::input_box::InputBox;
use crate::menus::{MenuAction, MenuId, menu_actions};

#[cfg(test)]
mod tests;

/// `CommentThreadCollapsibleState.Expanded`.
const EXPANDED: u64 = 1;
/// review.css `.review-comment`: 8px above and below, the 22px author line and the body's 4px top padding.
const COMMENT_CHROME: f32 = 8. + 22. + 4. + 8.;
/// shortcut: a body line is taken as the 18px the reference draws; measure it to size a wrapped body exactly.
pub(super) const BODY_LINE: f32 = 18.;
/// `.comment-form-container` 8px margins around the 32px "Reply..." button.
const REPLY_BUTTON_FORM: f32 = 8. + 32. + 8.;
/// The expanded form: the 90px input with its 12px padding, then the 26px buttons in 10px margins.
const REPLY_EDITOR_FORM: f32 = 8. + 102. + 10. + 26. + 10. + 8.;
/// `.edit-container`: 5px above the 102px input, then the buttons in 10px margins and 10px padding; it replaces the
/// body and its 4px padding.
const EDIT_FORM: f32 = 5. + 102. + 10. + 10. + 26. + 10. - 4.;

/// An expanded thread shown in an editor: its zone.
pub(super) struct ThreadWidget {
    pub controller: u64,
    pub thread: u64,
    pub model_id: u32,
    pub zone_id: u32,
}

/// The reply being written in a thread (`.comment-form-container.expand`), or the text of the comment being edited.
pub(super) struct Reply {
    pub controller: u64,
    pub thread: u64,
    /// The edited comment's `uniqueIdInThread`; None for a reply.
    pub comment: Option<u64>,
    pub input: InputBox,
}

impl Workbench {
    /// Every expanded thread gets a zone in each open editor of its resource, sized to its comments; a collapsed or
    /// gone thread loses its zones.
    pub(super) fn sync_comment_widgets(&mut self) {
        let line_height = self.editors.viewport.line_height;
        let mut wanted = Vec::new();
        for controller in &self.comments.controllers {
            for thread in controller.threads.iter().filter(|thread| thread.collapse_state == EXPANDED) {
                let Some(range) = thread.range.as_ref() else { continue };
                let reply = self.comments.reply.as_ref().filter(|reply| reply.controller == controller.handle && reply.thread == thread.handle);
                let height = widget_height(thread, reply.map(|reply| reply.comment), controller.reaction_handler, line_height);
                for editor in self.editors.editors.iter().filter(|editor| same_resource(&editor.document().uri(), &thread.resource)) {
                    wanted.push((controller.handle, thread.handle, editor.model_id, arrow_position(range), height));
                }
            }
        }
        let mut kept = Vec::new();
        for widget in std::mem::take(&mut self.comments.widgets) {
            let Some(editor) = self.editors.editors.iter_mut().find(|editor| editor.model_id == widget.model_id) else { continue };
            let want = wanted.iter().position(|(controller, thread, model_id, ..)| (*controller, *thread, *model_id) == (widget.controller, widget.thread, widget.model_id));
            let Some(index) = want else {
                editor.remove_view_zone(widget.zone_id);
                continue;
            };
            let (.., anchor, height) = wanted.remove(index);
            match editor.view_zones.iter_mut().find(|zone| zone.id == widget.zone_id) {
                Some(zone) => {
                    zone.anchor = Some(anchor);
                    zone.height = height;
                    kept.push(widget);
                }
                None => wanted.push((widget.controller, widget.thread, widget.model_id, anchor, height)),
            }
        }
        for (controller, thread, model_id, anchor, height) in wanted {
            let Some(editor) = self.editors.editors.iter_mut().find(|editor| editor.model_id == model_id) else { continue };
            let zone_id = editor.add_view_zone(Some(anchor), height);
            kept.push(ThreadWidget { controller, thread, model_id, zone_id });
        }
        self.comments.widgets = kept;
    }

    /// `toggleExpand` from the gutter glyph: an expanded thread collapses, a collapsed one expands.
    pub(super) fn toggle_comment_thread(&mut self, controller: u64, thread: u64, cx: &mut Context<Self>) {
        let Some(thread) = self.comment_thread_mut(controller, thread) else { return };
        thread.collapse_state = if thread.collapse_state == EXPANDED { 0 } else { EXPANDED };
        self.sync_comment_widgets();
        cx.notify();
    }

    fn comment_thread_mut(&mut self, controller: u64, thread: u64) -> Option<&mut Thread> {
        let controller = self.comments.controllers.iter_mut().find(|candidate| candidate.handle == controller)?;
        controller.threads.iter_mut().find(|candidate| candidate.handle == thread)
    }

    pub(super) fn comment_thread(&self, controller: u64, thread: u64) -> Option<&Thread> {
        let controller = self.comments.controllers.iter().find(|candidate| candidate.handle == controller)?;
        controller.threads.iter().find(|candidate| candidate.handle == thread)
    }

    /// `expandReplyArea`: the input shows, focused.
    pub(super) fn expand_comment_reply(&mut self, controller: u64, thread: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.comments.reply = Some(Reply { controller, thread, comment: None, input: InputBox::default() });
        self.comments.reply_view.reset();
        self.sync_comment_widgets();
        window.focus(&self.comments.reply_focus, cx);
        cx.notify();
    }

    /// `hideReplyArea`: the text is dropped and the button shows again.
    pub(super) fn hide_comment_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(reply) = self.comments.reply.take() else { return };
        // commentThreadZoneWidget.ts `collapse`: a thread without comments is deleted.
        if self.comment_thread(reply.controller, reply.thread).is_some_and(|thread| thread.comments.is_empty()) {
            self.delete_comment_thread(reply.controller, reply.thread, cx);
        }
        self.sync_comment_widgets();
        window.focus(&self.editor_focus, cx);
        cx.notify();
    }

    pub(super) fn edit_comment_reply_input(&mut self, edit: impl FnOnce(&mut InputBox), cx: &mut Context<Self>) {
        if let Some(reply) = self.comments.reply.as_mut() {
            edit(&mut reply.input);
        }
        cx.notify();
    }

    /// The context the thread's menus see: `commentController`, `commentThread`, `commentThreadIsEmpty` and, in the
    /// reply form, `commentIsEmpty`.
    pub(super) fn comment_thread_context(&self, controller: u64, thread: &Thread, window: &Window) -> HashMap<String, Value> {
        let mut context = self.key_context(window);
        let controller = self.comments.controllers.iter().find(|candidate| candidate.handle == controller);
        if let Some(value) = controller.and_then(|controller| controller.context_value.as_ref()) {
            context.insert("commentController".into(), json!(value));
        }
        if let Some(value) = &thread.context_value {
            context.insert("commentThread".into(), json!(value));
        }
        context.insert("commentThreadIsEmpty".into(), json!(thread.comments.is_empty()));
        let text = self.comments.reply.as_ref().map(|reply| reply.input.value.as_str()).unwrap_or_default();
        context.insert("commentIsEmpty".into(), json!(text.is_empty()));
        context
    }

    /// `CommentFormActions` run: the command gets the `CommentReply` (`MarshalledId.CommentThreadReply`, 9) with the
    /// thread (`MarshalledId.CommentThread`, 7) and the typed text, then the form hides.
    fn run_comment_reply_action(&mut self, id: &str, controller: u64, thread: u64, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.comments.reply.as_ref().map(|reply| reply.input.value.clone()).unwrap_or_default();
        let thread = json!({ "$mid": 7, "commentControlHandle": controller, "commentThreadHandle": thread });
        let argument = json!({ "thread": thread, "text": text, "$mid": 9 });
        self.run_code_lens_command(id, vec![argument], window, cx);
        self.hide_comment_reply(window, cx);
    }

    /// `editor.action.submitComment` (Cmd+Enter in the reply): the first form action runs.
    fn submit_comment_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((controller, thread)) = self.comments.reply.as_ref().filter(|reply| reply.comment.is_none()).map(|reply| (reply.controller, reply.thread)) else { return };
        let Some(state) = self.comment_thread(controller, thread) else { return };
        let context = self.comment_thread_context(controller, state, window);
        let first = menu_actions(&self.menus, MenuId::CommentThreadActions, &context).into_iter().find_map(|action| match action {
            MenuAction::Command { id, enabled: true, .. } => Some(id),
            _ => None,
        });
        if let Some(id) = first {
            self.run_comment_reply_action(&id, controller, thread, window, cx);
        }
    }

    pub(super) fn comment_thread_widgets(&self, editor: &OpenEditor, content_left: f32, char_width: f32, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let widgets = self.comments.widgets.iter().filter(|widget| widget.model_id == editor.model_id);
        widgets.filter_map(|widget| self.comment_thread_widget(widget, editor, content_left, char_width, window, cx)).collect()
    }

    fn comment_thread_widget(&self, widget: &ThreadWidget, editor: &OpenEditor, content_left: f32, char_width: f32, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let thread = self.comment_thread(widget.controller, widget.thread)?;
        let viewport = self.editors.viewport;
        let zone = editor.view_zone(widget.zone_id)?;
        let top = editor.view_zone_top(widget.zone_id, viewport)? - editor.scroll.top;
        let line_height = viewport.line_height;
        let arrow_height = (line_height / 3.).round();
        let frame_width = (line_height / 9.).round();
        let state_color = self.comment_thread_state_color(thread);
        let anchor = zone.anchor?;
        let line = &editor.document().lines[anchor.line];
        // zoneWidget.ts `show`: the arrow is not pretty at column 1 and moves to column 2.
        let arrow_column = anchor.column.max(1).min(line.chars().count().max(1));
        let arrow_x = content_left + visible_column(line, arrow_column, editor.document().indentation.tab_size) as f32 * char_width - editor.scroll.left;
        let arrow = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let (x, y) = (bounds.origin.x, bounds.origin.y);
                let mut path = PathBuilder::fill();
                path.move_to(point(x, y + px(arrow_height)));
                path.line_to(point(x + px(2. * arrow_height), y + px(arrow_height)));
                path.line_to(point(x + px(arrow_height), y));
                path.close();
                if let Ok(path) = path.build() {
                    window.paint_path(path, state_color);
                }
            },
        )
        .absolute()
        .top_0()
        .left(px(arrow_x - arrow_height))
        .w(px(2. * arrow_height))
        .h(px(arrow_height));
        let head_height = (line_height * 1.2).ceil();
        let body = self.comment_thread_body(widget, thread, window, cx);
        let container = div()
            .absolute()
            .top(px(arrow_height))
            .left_0()
            .right_0()
            .border_t(px(frame_width))
            .border_b(px(frame_width))
            .border_color(state_color)
            .bg(self.color("peekViewResult.background"))
            .child(self.comment_thread_head(widget, thread, head_height, state_color, window, cx))
            .child(div().border_t_1().border_color(state_color).child(body));
        let width = content_left + viewport.width - VERTICAL_SCROLLBAR_WIDTH;
        let element = div()
            .id(gpui::SharedString::from(format!("comment-thread-{}-{}", widget.controller, widget.thread)))
            .absolute()
            .left_0()
            .top(px(top))
            .w(px(width))
            .h(px(zone.height))
            .cursor_default()
            .occlude()
            .font_family(UI_FONT)
            .text_size(px(13.))
            .text_color(self.foreground_or("foreground"))
            .child(container)
            .child(arrow);
        Some(element.into_any_element())
    }

    /// commentColors.ts `getCommentThreadWidgetStateColor`.
    fn comment_thread_state_color(&self, thread: &Thread) -> Rgba {
        match thread.state {
            Some(0) => self.color("editorCommentsWidget.unresolvedBorder"),
            Some(1) => self.color("editorCommentsWidget.resolvedBorder"),
            _ => self.color("editorCommentsWidget.rangeBorder"),
        }
    }

    /// `CommentThreadHeader`: the label ("Start discussion" for a thread without comments), the title actions and
    /// Collapse (Delete for a thread without comments), on the state color at 10%.
    fn comment_thread_head(&self, widget: &ThreadWidget, thread: &Thread, height: f32, state_color: Rgba, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let label = match (&thread.label, thread.comments.is_empty()) {
            (Some(label), _) => label.clone(),
            (None, true) => "Start discussion".to_owned(),
            (None, false) => String::new(),
        };
        let icon_color = self.foreground_or("icon.foreground");
        let (controller, thread_handle) = (widget.controller, widget.thread);
        let context = self.comment_thread_context(controller, thread, window);
        let mut actions = Vec::new();
        for action in menu_actions(&self.menus, MenuId::CommentThreadTitle, &context) {
            let MenuAction::Command { id, title, enabled: true, .. } = action else { continue };
            let marshalled = json!({ "$mid": 7, "commentControlHandle": controller, "commentThreadHandle": thread_handle });
            actions.push(self.command_button(&id, title, vec![marshalled], icon_color, cx).ml(px(4.)).into_any_element());
        }
        let empty = thread.comments.is_empty();
        let (collapse_icon, collapse_title) = if empty { ("trash", "Delete") } else { ("chevron-up", "Collapse") };
        let collapse = self
            .icon(collapse_icon, 22., 16., icon_color)
            .id("comment-thread-collapse")
            .ml(px(4.))
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
            .child(self.hover_target("comment-thread-collapse-hover", HoverContent::Text(collapse_title.into()), HoverOptions::mouse()))
            .on_click(cx.listener(move |workbench, _, _, cx| match empty {
                true => workbench.delete_comment_thread(controller, thread_handle, cx),
                false => workbench.toggle_comment_thread(controller, thread_handle, cx),
            }));
        actions.push(collapse.into_any_element());
        let background = Rgba { a: state_color.a * 0.1, ..state_color };
        div()
            .h(px(height))
            .line_height(px(height))
            .flex()
            .items_center()
            .bg(background)
            .child(div().ml(px(20.)).min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(label))
            .child(div().flex_1().flex().justify_end().items_center().pr(px(2.)).children(actions))
            .into_any_element()
    }

    /// `CommentThreadBody` and the reply form.
    fn comment_thread_body(&self, widget: &ThreadWidget, thread: &Thread, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().flex().flex_col();
        for index in 0..thread.comments.len() {
            body = body.child(self.comment_node(widget, thread, index, window, cx));
        }
        if thread.can_reply != false {
            body = body.child(self.comment_reply_form(widget, thread, window, cx));
        }
        body.into_any_element()
    }

    /// `CommentReply`: the "Reply..." button, or the input with the `comments/commentThread/context` buttons, the
    /// first one primary.
    fn comment_reply_form(&self, widget: &ThreadWidget, thread: &Thread, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let (controller, thread_handle) = (widget.controller, widget.thread);
        let replying = self.comments.reply.as_ref().is_some_and(|reply| reply.controller == controller && reply.thread == thread_handle && reply.comment.is_none());
        let prompt = if thread.comments.is_empty() { "Type a new comment" } else { "Reply..." };
        let form = div().mx(px(20.)).my(px(8.));
        if !replying {
            // review.css: `outline-color: var(--vscode-contrastBorder)`, which falls back to the text color without one.
            let outline = self.theme.color("contrastBorder").unwrap_or(self.foreground_or("editor.foreground"));
            let button = div()
                .id("comment-reply-button")
                .h(px(32.))
                .border_1()
                .border_color(outline)
                .px(px(12.))
                .flex()
                .items_center()
                .font_family("Menlo")
                // review.css asks 600; the reference draws Menlo, which has no semibold, at its regular weight.
                .font_weight(gpui::FontWeight::MEDIUM)
                .whitespace_nowrap()
                .bg(self.color("editorCommentsWidget.replyInputBackground"))
                .text_color(self.foreground_or("editor.foreground"))
                .cursor_text()
                .on_click(cx.listener(move |workbench, _, window, cx| workbench.expand_comment_reply(controller, thread_handle, window, cx)))
                .child(prompt);
            return form.child(button).into_any_element();
        }
        let focused = self.comments.reply_focus.is_focused(window);
        let outline = if focused { self.color("focusBorder") } else { self.theme.color("contrastBorder").unwrap_or(self.foreground_or("editor.foreground")) };
        let input = div()
            .h(px(102.))
            .pl(px(12.))
            .py(px(6.))
            .rounded(px(3.))
            .border_1()
            .border_color(outline)
            .bg(self.color("editorCommentsWidget.replyInputBackground"))
            // simpleCommentEditor.ts: the editor's font family and size.
            .font_family("Menlo")
            .text_size(px(12.))
            .on_key_down(cx.listener(|workbench, event: &KeyDownEvent, window, cx| match (event.keystroke.key.as_str(), event.keystroke.modifiers.platform) {
                ("enter", true) => workbench.submit_comment_reply(window, cx),
                ("escape", _) => drop(workbench.execute_command("workbench.action.hideComment", Vec::new(), window, cx)),
                _ => {}
            }))
            .child(div().h(px(18.)).flex().child(self.input_text(InputId::CommentReply, InputLook { password: false, placeholder: Some(prompt.into()), focused }, cx)));
        let context = self.comment_thread_context(controller, thread, window);
        let mut buttons = Vec::new();
        for (index, action) in menu_actions(&self.menus, MenuId::CommentThreadActions, &context).into_iter().enumerate() {
            let MenuAction::Command { id, title, enabled, .. } = action else { continue };
            let (background, foreground, hover) = match index {
                0 => ("button.background", "button.foreground", "button.hoverBackground"),
                _ => ("button.secondaryBackground", "button.secondaryForeground", "button.secondaryHoverBackground"),
            };
            let button = div()
                .id(gpui::SharedString::from(format!("comment-reply-action-{id}")))
                .h(px(26.))
                .ml(px(5.))
                .px(px(8.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .bg(self.color(background))
                .text_color(self.color(foreground))
                .when(!enabled, |button| button.opacity(0.4))
                .when(enabled, |button| {
                    button.cursor_pointer().hover(|button| button.bg(self.color(hover))).on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).on_click(cx.listener(
                        move |workbench, _, window, cx| workbench.run_comment_reply_action(&id, controller, thread_handle, window, cx),
                    ))
                })
                .child(title);
            buttons.push(button);
        }
        form.child(input).child(div().my(px(10.)).flex().flex_row_reverse().children(buttons.into_iter().rev())).into_any_element()
    }
}

/// `arrowPosition`: the end line, at the range's middle for a one-line range, else at its start.
fn arrow_position(range: &Value) -> Position {
    let number = |key: &str| range[key].as_u64().unwrap_or(1) as usize;
    let end_line = number("endLineNumber");
    let column = if end_line == number("startLineNumber") { (number("startColumn") + number("endColumn") + 1) / 2 } else { 1 };
    Position::new(end_line.max(1) - 1, column - 1)
}

/// `_refresh`: the head (1.2 lines), the comments and the form, the arrow, the frame and 8px, in whole lines.
/// `reply` is the open form: Some(None) a reply, Some(Some(id)) the edit of comment `id`. With a reaction handler
/// every comment has a reactions row, for the picker.
fn widget_height(thread: &Thread, reply: Option<Option<u64>>, reaction_handler: bool, line_height: f32) -> f32 {
    let mut body = 1.;
    for comment in &thread.comments {
        let editing = reply.flatten().is_some_and(|id| comment["uniqueIdInThread"].as_u64() == Some(id));
        let lines = comment_text(comment).lines().filter(|line| !line.trim().is_empty()).count().max(1);
        body += COMMENT_CHROME + if editing { EDIT_FORM } else { lines as f32 * BODY_LINE };
        if reaction_handler || comment["commentReactions"].as_array().into_iter().flatten().any(|reaction| reaction["count"].as_u64().unwrap_or_default() > 0) {
            body += REACTIONS_ROW;
        }
    }
    if thread.can_reply != false {
        body += if reply == Some(None) { REPLY_EDITOR_FORM } else { REPLY_BUTTON_FORM };
    }
    let head = (line_height * 1.2).ceil();
    let arrow = (line_height / 3.).round();
    let frame = (line_height / 9.).round() * 2.;
    ((head + body + arrow + frame + 8.) / line_height).ceil() * line_height
}

fn same_resource(uri: &Value, resource: &Value) -> bool {
    uri["path"] == resource["path"] && uri["scheme"] == resource["scheme"]
}
