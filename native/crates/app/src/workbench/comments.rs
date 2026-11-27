//! mainThreadComments.ts: the comment controllers extensions register and their threads, kept for the Comments
//! view (workbench/comments_view.rs).

use gpui::{Context, FocusHandle, Window};
use serde_json::Value;

use super::Workbench;
use super::comment_thread_widget::{Reply, ThreadWidget};
use super::input_box_view::InputView;
use crate::commands::Outcome;

#[cfg(test)]
mod tests;

pub(super) struct Comments {
    pub(super) controllers: Vec<Controller>,
    pub(super) collapsed: std::collections::HashSet<String>,
    pub(super) widgets: Vec<ThreadWidget>,
    pub(super) reply: Option<Reply>,
    pub(super) reply_view: InputView,
    pub(super) reply_focus: FocusHandle,
    pub(super) view: super::comments_view::ViewState,
    pub(super) commenting: super::commenting_ranges::CommentingRanges,
    /// Toggle Editor Commenting turned commenting off (`commentingEnabled` false): no glyphs, ranges or threads show.
    pub(super) commenting_disabled: bool,
    /// The `reaction.command.<label>` commands of the last reaction picker.
    pub(super) reaction_commands: Vec<(String, crate::commands::Registration)>,
    pub(super) floating: super::comment_thread_floating::FloatingWidgets,
}

impl Comments {
    pub(super) fn new(cx: &mut Context<Workbench>) -> Comments {
        let reply_focus = cx.focus_handle();
        let view = super::comments_view::ViewState::new(cx);
        Comments { controllers: Vec::new(), collapsed: Default::default(), widgets: Vec::new(), reply: None, reply_view: InputView::default(), reply_focus, view, commenting: Default::default(), commenting_disabled: false, reaction_commands: Vec::new(), floating: Default::default() }
    }
}

/// `MainThreadCommentController`.
pub(super) struct Controller {
    pub(super) handle: u64,
    pub(super) label: String,
    /// `CommentControllerFeatures.contextValue`, the `commentController` context key.
    pub(super) context_value: Option<String>,
    /// `CommentControllerFeatures.reactionHandler`: reactions toggle.
    pub(super) reaction_handler: bool,
    /// `updateCommentingRanges` was called, which registers the view upstream.
    pub(super) has_commenting_ranges: bool,
    pub(super) threads: Vec<Thread>,
}

/// `MainThreadCommentThread`: the fields of `CommentThreadChanges`.
#[derive(Clone, Debug)]
pub(super) struct Thread {
    pub(super) handle: u64,
    pub(super) resource: Value,
    pub(super) range: Option<Value>,
    pub(super) label: Option<String>,
    pub(super) context_value: Option<String>,
    pub(super) comments: Vec<Value>,
    pub(super) collapse_state: u64,
    /// A new empty template shown by `shouldReveal` (ReviewZoneWidget `makeVisible`), until a collapse other than
    /// Dismiss Comment.
    pub(super) revealed: bool,
    pub(super) can_reply: Value,
    /// `CommentThreadState`: 0 unresolved, 1 resolved.
    pub(super) state: Option<u64>,
    pub(super) is_template: bool,
}

impl Workbench {
    pub(crate) fn handle_comments_request(&mut self, method: &str, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let arg = |index: usize| args.get(index).cloned().unwrap_or(Value::Null);
        let handle = arg(0).as_u64().unwrap_or_default();
        match method {
            "$registerCommentController" => {
                let text = |index: usize| arg(index).as_str().unwrap_or_default().to_owned();
                self.comments.controllers.push(Controller { handle, label: text(2), context_value: None, reaction_handler: false, has_commenting_ranges: false, threads: Vec::new() });
            }
            "$updateCommentControllerFeatures" => {
                if let Some(controller) = self.comment_controller_mut(handle) {
                    controller.context_value = arg(1)["contextValue"].as_str().map(str::to_owned);
                    controller.reaction_handler = arg(1)["reactionHandler"] == true;
                }
            }
            "$unregisterCommentController" => self.comments.controllers.retain(|controller| controller.handle != handle),
            "$createCommentThread" => {
                let Some(controller) = self.comment_controller_mut(handle) else { return Outcome::done() };
                let range = Some(arg(4)).filter(|range| !range.is_null());
                let comments = arg(5).as_array().cloned().unwrap_or_default();
                // `MainThreadCommentThread`: a new thread is collapsed and can be replied to.
                let mut thread = Thread { handle: arg(1).as_u64().unwrap_or_default(), resource: arg(3), range, label: None, context_value: None, comments, collapse_state: 0, revealed: false, can_reply: Value::Bool(true), state: None, is_template: arg(7) == true };
                // commentsController.ts `shouldReveal`: a new template without comments shows, its reply form focused,
                // without touching its Collapsed state, so a later collapse changes nothing and the zone stays.
                let reveal = thread.is_template && thread.comments.is_empty();
                thread.revealed = reveal;
                let thread_handle = thread.handle;
                controller.threads.push(thread);
                if reveal {
                    self.expand_comment_reply(handle, thread_handle, window, cx);
                }
            }
            "$updateCommentThread" => {
                let Some(controller) = self.comment_controller_mut(handle) else { return Outcome::done() };
                let thread_handle = arg(1).as_u64().unwrap_or_default();
                let Some(thread) = controller.threads.iter_mut().find(|thread| thread.handle == thread_handle) else { return Outcome::done() };
                let before = thread.comments.clone();
                thread.update(&arg(4));
                self.sync_comment_edits(handle, thread_handle, &before, window, cx);
            }
            "$deleteCommentThread" => {
                let thread_handle = arg(1).as_u64().unwrap_or_default();
                if let Some(controller) = self.comment_controller_mut(handle) {
                    controller.threads.retain(|thread| thread.handle != thread_handle);
                }
            }
            "$updateCommentingRanges" => {
                if let Some(controller) = self.comment_controller_mut(handle) {
                    controller.has_commenting_ranges = true;
                }
                self.refresh_commenting_ranges(false, cx);
            }
            "$revealCommentThread" => self.reveal_comment_thread(handle, arg(1).as_u64().unwrap_or_default(), window, cx),
            "$hideCommentThread" => {
                let thread_handle = arg(1).as_u64().unwrap_or_default();
                if let Some(thread) = self.comment_controller_mut(handle).and_then(|controller| controller.threads.iter_mut().find(|thread| thread.handle == thread_handle)) {
                    thread.set_collapse_state(0);
                }
            }
            _ => {}
        }
        self.sync_comment_widgets();
        self.select_first_comment_thread();
        cx.notify();
        Outcome::done()
    }

    fn comment_controller_mut(&mut self, handle: u64) -> Option<&mut Controller> {
        self.comments.controllers.iter_mut().find(|controller| controller.handle == handle)
    }

    /// `registerView` runs once a controller has threads or commenting ranges; the container `hideIfEmpty`.
    pub(super) fn comments_view_registered(&self) -> bool {
        self.comments.controllers.iter().any(|controller| controller.has_commenting_ranges || !controller.threads.is_empty())
    }

    /// `revealCommentThread` for a document thread: its file opens at the start of its range.
    pub(super) fn reveal_comment_thread(&mut self, controller: u64, thread: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.comments.commenting_disabled = false;
        let thread = self.comments.controllers.iter().find(|candidate| candidate.handle == controller).and_then(|controller| controller.threads.iter().find(|candidate| candidate.handle == thread));
        let Some(thread) = thread else { return };
        let mut target = thread.range.clone().unwrap_or_else(|| serde_json::json!({ "startLineNumber": 1, "startColumn": 1 }));
        target["resource"] = thread.resource.clone();
        self.open_marker_related(&target, window, cx);
    }
}

impl Thread {
    /// `MainThreadCommentThread.collapsibleState`'s setter: only a change reaches the zone, which then follows it.
    pub(super) fn set_collapse_state(&mut self, state: u64) -> bool {
        if state == self.collapse_state {
            return false;
        }
        self.collapse_state = state;
        // The host's Expanded echo of a revealed template keeps it revealed; a collapse closes it.
        self.revealed &= state == 1;
        true
    }

    /// The zone is shown: Expanded, or revealed and not collapsed since.
    pub(super) fn zone_shown(&self) -> bool {
        self.collapse_state == 1 || self.revealed
    }

    /// `MainThreadCommentThread.batchUpdate`: only the fields the changes carry.
    fn update(&mut self, changes: &Value) {
        let Some(changes) = changes.as_object() else { return };
        if let Some(range) = changes.get("range") {
            self.range = Some(range.clone()).filter(|range| !range.is_null());
        }
        if let Some(label) = changes.get("label") {
            self.label = label.as_str().map(str::to_owned);
        }
        if let Some(context_value) = changes.get("contextValue") {
            self.context_value = context_value.as_str().map(str::to_owned);
        }
        if let Some(comments) = changes.get("comments") {
            self.comments = comments.as_array().cloned().unwrap_or_default();
        }
        if let Some(collapse_state) = changes.get("collapseState").and_then(Value::as_u64) {
            self.set_collapse_state(collapse_state);
        }
        if let Some(can_reply) = changes.get("canReply") {
            self.can_reply = can_reply.clone();
        }
        if let Some(state) = changes.get("state") {
            self.state = state.as_u64();
        }
        if let Some(is_template) = changes.get("isTemplate") {
            self.is_template = is_template == true;
        }
    }
}
