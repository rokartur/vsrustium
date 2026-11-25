//! commentNode.ts and reactionsAction.ts: one comment of a thread widget. The author line carries the
//! `comments/comment/title` inline actions and a "More Actions..." menu of the others; a comment the extension
//! switches to `CommentMode.Editing` shows its text in the input with the `comments/comment/context` buttons; the
//! reactions with a count follow the body and toggle through the controller's reaction handler, whose picker lists
//! the comment's reactions as `reaction.command.<label>` commands.

use gpui::{AnyElement, Context, KeyDownEvent, MouseButton, Window, div, img, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::comment_thread_widget::{BODY_LINE, Reply, ThreadWidget};
use super::comments::Thread;
use super::hover::{HoverContent, HoverOptions};
use super::input_box_view::{InputId, InputLook};
use super::markdown_view::MarkdownStyle;
use crate::input_box::InputBox;
use crate::markdown::{MarkdownString, parse};
use crate::menus::{MenuAction, MenuId, menu_actions};

#[cfg(test)]
mod tests;

/// `languages.CommentMode.Editing`.
const EDITING: u64 = 0;
/// review.css `.comment-reactions`: 8px above the 25px row.
pub(super) const REACTIONS_ROW: f32 = 8. + 25.;

impl Workbench {
    /// `CommentNode.update` on a mode change: a comment switched to Editing opens its edit form with its text, one
    /// switched back closes it. A comment new to the thread keeps its body (`switchToEditMode` runs on changes only).
    pub(super) fn sync_comment_edits(&mut self, controller: u64, thread: u64, before: &[Value], window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.comment_thread(controller, thread) else { return };
        let mut opened = None;
        let mut closed = Vec::new();
        for comment in &state.comments {
            let Some(old) = before.iter().find(|old| old["uniqueIdInThread"] == comment["uniqueIdInThread"]) else { continue };
            if comment["mode"].is_null() || comment["mode"] == old["mode"] {
                continue;
            }
            let id = comment["uniqueIdInThread"].as_u64().unwrap_or_default();
            match comment["mode"].as_u64() == Some(EDITING) {
                true => opened = Some((id, comment_text(comment))),
                false => closed.push(id),
            }
        }
        let editing = |reply: &Reply| reply.controller == controller && reply.thread == thread && reply.comment.is_some_and(|id| closed.contains(&id));
        if self.comments.reply.as_ref().is_some_and(editing) {
            self.comments.reply = None;
        }
        if let Some((id, text)) = opened {
            let mut input = InputBox::default();
            input.set_value(text);
            self.comments.reply = Some(Reply { controller, thread, comment: Some(id), input });
            self.comments.reply_view.reset();
            window.focus(&self.comments.reply_focus, cx);
        }
    }

    /// The context of a comment's menus: the thread's keys and `comment`, its `contextValue`.
    fn comment_context(&self, controller: u64, thread: &Thread, comment: &Value, window: &Window) -> std::collections::HashMap<String, Value> {
        let mut context = self.comment_thread_context(controller, thread, window);
        if let Some(value) = comment["contextValue"].as_str() {
            context.insert("comment".into(), json!(value));
        }
        context
    }

    /// `commentNodeContext`: the `CommentNode` (`MarshalledId.CommentNode`, 10) and the thread.
    fn comment_node_args(controller: u64, thread: u64, comment: &Value) -> Vec<Value> {
        let thread = json!({ "$mid": 7, "commentControlHandle": controller, "commentThreadHandle": thread });
        vec![json!({ "thread": thread, "commentUniqueId": comment["uniqueIdInThread"], "$mid": 10 }), thread]
    }

    /// The edit form's `CommentFormActions` run: the command gets the `CommentThreadNode` (`MarshalledId`, 11) with
    /// the typed text, then the form closes (`removeCommentEditor`).
    fn run_comment_edit_action(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(reply) = self.comments.reply.take() else { return };
        let thread = json!({ "$mid": 7, "commentControlHandle": reply.controller, "commentThreadHandle": reply.thread });
        let argument = json!({ "thread": thread, "commentUniqueId": reply.comment, "text": reply.input.value, "$mid": 11 });
        self.run_code_lens_command(id, vec![argument], window, cx);
        self.sync_comment_widgets();
        window.focus(&self.editor_focus, cx);
        cx.notify();
    }

    /// `editor.action.submitComment` in an edit form: its first enabled action runs.
    pub(super) fn submit_comment_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(reply) = self.comments.reply.as_ref() else { return };
        let Some(thread) = self.comment_thread(reply.controller, reply.thread) else { return };
        let Some(comment) = thread.comments.iter().find(|comment| comment["uniqueIdInThread"].as_u64() == reply.comment) else { return };
        let context = self.comment_context(reply.controller, thread, comment, window);
        let first = menu_actions(&self.menus, MenuId::CommentActions, &context).into_iter().find_map(|action| match action {
            MenuAction::Command { id, enabled: true, .. } => Some(id),
            _ => None,
        });
        if let Some(id) = first {
            self.run_comment_edit_action(&id, window, cx);
        }
    }

    /// `commentService.toggleReaction`: the controller's reaction handler adds or removes the user's reaction.
    fn toggle_comment_reaction(&mut self, controller: u64, thread: u64, comment: &Value, reaction: &Value) {
        let Some(resource) = self.comment_thread(controller, thread).map(|thread| thread.resource.clone()) else { return };
        drop(self.host.call("ExtHostComments", "$toggleReaction", vec![json!(controller), json!(thread), resource, comment.clone(), reaction.clone()]));
    }

    /// `CommentNode`: the author, the comment's label and its title actions, then the body (plain text, rendered
    /// markdown, or the edit form), then the reactions.
    pub(super) fn comment_node(&self, widget: &ThreadWidget, thread: &Thread, index: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let comment = &thread.comments[index];
        let foreground = self.foreground_or("foreground");
        let header = div()
            .h(px(22.))
            .flex()
            .items_center()
            .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().font_weight(gpui::FontWeight::BOLD).child(comment["userName"].as_str().unwrap_or_default().to_owned()))
            .children(comment["label"].as_str().map(|label| div().mx(px(5.)).px(px(2.)).italic().child(label.to_owned())))
            .child(self.comment_title_actions(widget, thread, comment, window, cx));
        let editing = self.comments.reply.as_ref().is_some_and(|reply| reply.controller == widget.controller && reply.thread == widget.thread && reply.comment == comment["uniqueIdInThread"].as_u64());
        let body = match (&comment["body"], editing) {
            (_, true) => self.comment_edit_form(widget, thread, comment, window, cx),
            (Value::String(text), false) => div().whitespace_normal().child(text.clone()).into_any_element(),
            (markdown, false) => {
                let blocks = parse(&MarkdownString::from_value(markdown));
                let style = MarkdownStyle { font_size: 13., line_height: BODY_LINE, foreground };
                self.markdown_blocks(&format!("comment-{}-{}-{index}", widget.controller, widget.thread), &blocks, style, "", cx).into_any_element()
            }
        };
        // The edit container replaces the body and its 4px padding.
        let body = div().when(!editing, |body| body.pt(px(4.))).line_height(px(BODY_LINE)).child(body);
        let details = div().pl(px(20.)).flex_1().min_w_0().overflow_hidden().child(header).child(body);
        div()
            .pt(px(8.))
            .pb(px(8.))
            .pl(px(20.))
            .pr(px(16.))
            .flex()
            .child(details.children(self.comment_reactions(widget, comment, cx)))
            .into_any_element()
    }

    /// `createActionsToolbar`: the `inline` group as icons, the others behind "More Actions...".
    fn comment_title_actions(&self, widget: &ThreadWidget, thread: &Thread, comment: &Value, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let context = self.comment_context(widget.controller, thread, comment, window);
        let args = Self::comment_node_args(widget.controller, widget.thread, comment);
        let icon_color = self.foreground_or("icon.foreground");
        let mut actions = Vec::new();
        let inline: Vec<_> = self.menus.iter().filter(|item| item.menu == MenuId::CommentTitle && item.group.starts_with("inline")).cloned().collect();
        for action in menu_actions(&inline, MenuId::CommentTitle, &context) {
            let MenuAction::Command { id, title, enabled: true, .. } = action else { continue };
            actions.push(self.command_button(&id, title, args.clone(), icon_color, cx).into_any_element());
        }
        let others: Vec<_> = self.menus.iter().filter(|item| item.menu != MenuId::CommentTitle || !item.group.starts_with("inline")).cloned().collect();
        let secondary = menu_actions(&others, MenuId::CommentTitle, &context);
        if !secondary.is_empty() {
            let more = self
                .icon("toolbar-more", 22., 16., icon_color)
                .id(gpui::SharedString::from(format!("comment-more-{}-{}-{}", widget.controller, widget.thread, comment["uniqueIdInThread"])))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
                .child(self.hover_target("comment-more-hover", HoverContent::Text("More Actions...".into()), HoverOptions::mouse()))
                .on_click(cx.listener(move |workbench, event: &gpui::ClickEvent, window, cx| {
                    let anchor = gpui::Bounds::new(event.position(), Default::default());
                    workbench.show_context_menu_actions(secondary.clone(), &context, anchor, args.clone(), window, cx);
                }));
            actions.push(more.into_any_element());
        }
        div().ml_auto().h(px(22.)).flex().items_center().children(actions).into_any_element()
    }

    /// The edit container: the input with the text, then the `comments/comment/context` buttons, the first primary.
    fn comment_edit_form(&self, widget: &ThreadWidget, thread: &Thread, comment: &Value, window: &Window, cx: &mut Context<Self>) -> AnyElement {
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
                ("enter", true) => workbench.submit_comment_edit(window, cx),
                ("escape", _) => drop(workbench.execute_command("workbench.action.hideComment", Vec::new(), window, cx)),
                _ => {}
            }))
            .child(div().h(px(18.)).flex().child(self.input_text(InputId::CommentReply, InputLook { password: false, placeholder: None, focused }, cx)));
        let context = self.comment_context(widget.controller, thread, comment, window);
        let mut buttons = Vec::new();
        for (index, action) in menu_actions(&self.menus, MenuId::CommentActions, &context).into_iter().enumerate() {
            let MenuAction::Command { id, title, enabled, .. } = action else { continue };
            let (background, foreground, hover) = match index {
                0 => ("button.background", "button.foreground", "button.hoverBackground"),
                _ => ("button.secondaryBackground", "button.secondaryForeground", "button.secondaryHoverBackground"),
            };
            let button = div()
                .id(gpui::SharedString::from(format!("comment-edit-action-{id}")))
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
                    button
                        .cursor_pointer()
                        .hover(|button| button.bg(self.color(hover)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |workbench, _, window, cx| workbench.run_comment_edit_action(&id, window, cx)))
                })
                .child(title);
            buttons.push(button);
        }
        let actions = div().mt(px(10.)).pt(px(10.)).mb(px(10.)).flex().flex_row_reverse().children(buttons.into_iter().rev());
        div().mt(px(5.)).mr(px(12.)).child(input).child(actions).into_any_element()
    }

    /// `createReactionsContainer`: each reaction with a count, its icon or label and the count, active when the user
    /// reacted; a click toggles it when the controller has a reaction handler.
    fn comment_reactions(&self, widget: &ThreadWidget, comment: &Value, cx: &mut Context<Self>) -> Option<AnyElement> {
        let handler = self.comments.controllers.iter().any(|controller| controller.handle == widget.controller && controller.reaction_handler);
        let reactions: Vec<Value> = comment["commentReactions"].as_array().into_iter().flatten().filter(|reaction| reaction["count"].as_u64().unwrap_or_default() > 0).cloned().collect();
        if reactions.is_empty() && !handler {
            return None;
        }
        let mut row = div().mt(px(8.)).min_h(px(25.)).flex().items_center();
        for (index, reaction) in reactions.into_iter().enumerate() {
            let label = reaction["label"].as_str().unwrap_or_default().to_owned();
            let content = match reaction["iconPath"]["path"].as_str() {
                Some(path) => div().w(px(14.)).child(img(path.to_owned()).size(px(14.))).into_any_element(),
                None => div().mr(px(4.)).line_height(px(20.)).child(label.clone()).into_any_element(),
            };
            let active = handler && reaction["hasReacted"] == true;
            let (controller, thread, comment) = (widget.controller, widget.thread, comment.clone());
            let tooltip = reaction_tooltip(&reaction, handler);
            let count = reaction["count"].as_u64();
            let button = div()
                .id(gpui::SharedString::from(format!("comment-reaction-{controller}-{thread}-{}-{index}", comment["uniqueIdInThread"])))
                .mr(px(7.))
                .px(px(4.))
                .py(px(1.))
                .flex()
                .items_center()
                .text_size(px(12.))
                .whitespace_nowrap()
                .border_1()
                .border_color(self.color("panel.border"))
                .rounded(px(3.))
                .when(!handler, |button| button.opacity(0.6))
                .when(active, |button| button.hover(|button| button.bg(self.color("statusBarItem.hoverBackground"))))
                .when(handler, |button| {
                    button.cursor_pointer().on_click(cx.listener(move |workbench, _, _, _| workbench.toggle_comment_reaction(controller, thread, &comment, &reaction)))
                })
                .child(self.hover_target(gpui::SharedString::from(format!("comment-reaction-hover-{index}")), HoverContent::Text(tooltip), HoverOptions::mouse()))
                .child(content)
                .children(count.map(|count| div().child(count.to_string())));
            row = row.child(button);
        }
        if handler {
            let (controller, thread, comment) = (widget.controller, widget.thread, comment.clone());
            let picker = self
                .icon("reactions", 26., 16., self.foreground_or("icon.foreground"))
                .id(gpui::SharedString::from(format!("comment-reaction-picker-{controller}-{thread}-{}", comment["uniqueIdInThread"])))
                .h(px(16.))
                .mt(px(3.))
                .cursor_pointer()
                .child(self.hover_target("comment-reaction-picker-hover", HoverContent::Text("Pick Reactions...".into()), HoverOptions::mouse()))
                .on_click(cx.listener(move |workbench, event: &gpui::ClickEvent, window, cx| {
                    let anchor = gpui::Bounds::new(event.position(), Default::default());
                    workbench.show_reaction_picker(controller, thread, &comment, anchor, window, cx);
                }));
            row = row.child(picker);
        }
        Some(row.into_any_element())
    }

    /// `createReactionPicker`: the comment's reactions as `reaction.command.<label>` items that toggle them. The
    /// previous picker's commands are disposed first.
    fn show_reaction_picker(&mut self, controller: u64, thread: u64, comment: &Value, anchor: gpui::Bounds<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        for (id, registration) in std::mem::take(&mut self.comments.reaction_commands) {
            self.commands.dispose(&id, registration);
        }
        let mut actions = Vec::new();
        for reaction in comment["commentReactions"].as_array().into_iter().flatten() {
            let label = reaction["label"].as_str().unwrap_or_default().to_owned();
            let id = format!("reaction.command.{label}");
            let (comment, reaction) = (comment.clone(), reaction.clone());
            let registration = self.commands.register(id.clone(), move |workbench, _, _, _| {
                workbench.toggle_comment_reaction(controller, thread, &comment, &reaction);
                crate::commands::Outcome::done()
            });
            self.comments.reaction_commands.push((id.clone(), registration));
            actions.push(MenuAction::Command { id, title: label, enabled: true, checked: false, alt: None });
        }
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }
}

/// `ReactionActionViewItem.getTooltip`: the count, or who reacted, with "Toggle reaction, " when it toggles.
fn reaction_tooltip(reaction: &Value, enabled: bool) -> String {
    let toggle = if enabled { "Toggle reaction, " } else { "" };
    let label = reaction["label"].as_str().unwrap_or_default();
    let count = reaction["count"].as_u64().unwrap_or_default();
    let reactors: Vec<&str> = reaction["reactors"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    if reactors.is_empty() {
        return match count {
            1 => format!("{toggle}1 reaction with {label}"),
            _ => format!("{toggle}{count} reactions with {label}"),
        };
    }
    if reactors.len() <= 10 && reactors.len() as u64 == count {
        return format!("{toggle}{} reacted with {label}", reactors.join(", "));
    }
    let shown = &reactors[..reactors.len().min(10)];
    format!("{toggle}{} and {} more reacted with {label}", shown.join(", "), count.saturating_sub(shown.len() as u64))
}

/// `commentBodyValue`: the text of a string or markdown body.
pub(super) fn comment_text(comment: &Value) -> String {
    comment["body"].as_str().or(comment["body"]["value"].as_str()).unwrap_or_default().to_owned()
}
