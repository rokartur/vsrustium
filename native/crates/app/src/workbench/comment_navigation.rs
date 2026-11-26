//! commentsEditorContribution.ts: Collapse All, Expand All and Expand Unresolved Comments, Dismiss Comment, Toggle
//! Editor Commenting, Focus Comment on Current Line, and the controller's next and previous comment thread (Alt+F9,
//! Shift+Alt+F9), commented range (Alt+F10, Shift+Alt+F10) and commenting range. shortcut: the thread
//! reached expands and is selected, but focus stays in the editor (`CommentWidgetFocus.Widget` has no focusable
//! comment here); commenting ranges are whole lines, so a range is reached at its first or last line's start.

use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::{Position, Selection, VerticalReveal};

/// `CommentThreadCollapsibleState`.
const COLLAPSED: u64 = 0;
const EXPANDED: u64 = 1;
/// `CommentThreadState.Unresolved`.
const UNRESOLVED: u64 = 0;

#[cfg(test)]
mod tests;

impl Workbench {
    pub(super) fn register_comment_navigation_commands(&mut self) {
        self.commands.register("workbench.action.collapseAllComments", |workbench, _, _, cx| workbench.change_all_collapse_states(|_| COLLAPSED, cx));
        self.commands.register("workbench.action.expandAllComments", |workbench, _, _, cx| workbench.change_all_collapse_states(|_| EXPANDED, cx));
        self.commands.register("workbench.action.expandUnresolvedComments", |workbench, _, _, cx| {
            workbench.change_all_collapse_states(|state| if state == Some(UNRESOLVED) { EXPANDED } else { COLLAPSED }, cx)
        });
        self.commands.register("editor.action.nextCommentThreadAction", |workbench, _, window, cx| {
            workbench.reveal_nearest_comment_thread(false, window, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.previousCommentThreadAction", |workbench, _, window, cx| {
            workbench.reveal_nearest_comment_thread(true, window, cx);
            Outcome::done()
        });
        // `nextCommentThread(false)`: the same move without focusing the widget, which the port never focuses.
        self.commands.register("editor.action.nextCommentedRangeAction", |workbench, _, window, cx| {
            workbench.reveal_nearest_comment_thread(false, window, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.previousCommentedRangeAction", |workbench, _, window, cx| {
            workbench.reveal_nearest_comment_thread(true, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.toggleCommenting", |workbench, _, _, cx| {
            workbench.comments.commenting_disabled = !workbench.comments.commenting_disabled;
            workbench.sync_comment_widgets();
            cx.notify();
            Outcome::done()
        });
        self.commands.register("workbench.action.focusCommentOnCurrentLine", |workbench, _, _, cx| {
            if !workbench.expand_comment_thread_at_cursor() {
                workbench.notify_error("The cursor must be on a line with a comment to focus the comment");
            }
            cx.notify();
            Outcome::done()
        });
        self.commands.register("editor.action.nextCommentingRange", |workbench, _, window, cx| {
            workbench.reveal_nearest_commenting_range(false, window, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.previousCommentingRange", |workbench, _, window, cx| {
            workbench.reveal_nearest_commenting_range(true, window, cx);
            Outcome::done()
        });
        // `CommentCommandId.Hide` in a comment input: `getParentThread().collapse()` selects the thread's range.
        self.commands.register("workbench.action.hideComment", |workbench, _, window, cx| {
            let Some((controller, thread)) = workbench.comments.reply.as_ref().map(|reply| (reply.controller, reply.thread)) else { return Outcome::done() };
            let state = workbench.comments.controllers.iter_mut().find(|candidate| candidate.handle == controller).and_then(|owner| owner.threads.iter_mut().find(|candidate| candidate.handle == thread));
            // The reference leaves a revealed template open on Escape, text typed or not, without the confirmation
            // `collapse(true)` would ask for (ui-pair steps-comment-escape-typed.json): its collapse never runs.
            let Some(state) = state.filter(|state| !state.revealed) else { return Outcome::done() };
            let hides = state.set_collapse_state(COLLAPSED);
            let range = state.range.clone();
            if hides {
                workbench.hide_comment_reply(window, cx);
            }
            if let Some(range) = range {
                workbench.select_comment_range(&range);
            }
            workbench.sync_comment_widgets();
            cx.notify();
            Outcome::done()
        });
    }

    /// `changeAllCollapseState`: every thread takes the state `new_state` gives for its `CommentThreadState`.
    fn change_all_collapse_states(&mut self, new_state: impl Fn(Option<u64>) -> u64, cx: &mut Context<Self>) -> Outcome {
        for thread in self.comments.controllers.iter_mut().flat_map(|controller| &mut controller.threads) {
            thread.set_collapse_state(new_state(thread.state));
        }
        self.sync_comment_widgets();
        cx.notify();
        Outcome::done()
    }

    /// `_findNearestCommentThread`: the first thread of the active editor starting after the selection's end (before
    /// its start when `reverse`), selected and expanded.
    fn reveal_nearest_comment_thread(&mut self, reverse: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let uri = editor.document().uri();
        let selection = editor.selection();
        let after = if reverse { selection.start() } else { selection.end() };
        let mut threads: Vec<(u64, u64, Option<Value>)> = Vec::new();
        for controller in &self.comments.controllers {
            for thread in controller.threads.iter().filter(|thread| thread.resource["path"] == uri["path"] && thread.resource["scheme"] == uri["scheme"]) {
                threads.push((controller.handle, thread.handle, thread.range.clone()));
            }
        }
        let Some((controller, thread, range)) = nearest_thread(threads, after, reverse) else { return };
        self.select_comment_range(&range.unwrap_or_else(|| json!({})));
        if let Some(state) = self.comments.controllers.iter_mut().find(|candidate| candidate.handle == controller).and_then(|owner| owner.threads.iter_mut().find(|candidate| candidate.handle == thread)) {
            state.set_collapse_state(EXPANDED);
        }
        self.sync_comment_widgets();
        window.focus(&self.editor_focus, cx);
        cx.notify();
    }
}

impl Workbench {
    /// `_findNearestCommentingRange`: the cursor moves to the nearest commenting range's start (its end in reverse).
    fn reveal_nearest_commenting_range(&mut self, reverse: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let line = editor.selection().end().line;
        let ranges: Vec<(usize, usize)> = self.commenting_lines(editor);
        let Some((start, end)) = nearest_commenting_range(&ranges, line, reverse) else { return };
        let position = Position::new(if reverse { end } else { start }, 0);
        let viewport = self.editors.viewport;
        if let Some(editor) = self.editors.active_mut() {
            editor.set_selections(vec![Selection::caret(position)]);
            editor.reveal_range(position, position, VerticalReveal::CenterIfOutsideViewport, viewport);
        }
        window.focus(&self.editor_focus, cx);
        cx.notify();
    }

    /// `getCommentsAtLine` then `revealCommentThread`: the first thread whose range holds the cursor's line expands.
    fn expand_comment_thread_at_cursor(&mut self) -> bool {
        let Some(editor) = self.editors.active() else { return false };
        let uri = editor.document().uri();
        let line = editor.selection().head.line as u64 + 1;
        let holds = |range: &Value| range["startLineNumber"].as_u64().is_some_and(|start| start <= line) && range["endLineNumber"].as_u64().is_some_and(|end| line <= end);
        let threads = self.comments.controllers.iter_mut().flat_map(|controller| &mut controller.threads);
        let mut threads = threads.filter(|thread| thread.resource["path"] == uri["path"] && thread.resource["scheme"] == uri["scheme"]);
        let Some(thread) = threads.find(|thread| thread.range.as_ref().is_some_and(holds)) else { return false };
        thread.set_collapse_state(EXPANDED);
        self.sync_comment_widgets();
        true
    }

    /// The active editor selects `range` (an `IRange`, 1-based), revealed in the center when outside the viewport.
    fn select_comment_range(&mut self, range: &Value) {
        let (start, end) = (position(range, "startLineNumber", "startColumn"), position(range, "endLineNumber", "endColumn"));
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut() else { return };
        editor.set_selections(vec![Selection { anchor: start, head: end }]);
        editor.reveal_range(start, end, VerticalReveal::CenterIfOutsideViewport, viewport);
    }
}

/// `getNearestCommentingRange` over 0-based inclusive line ranges: the range containing `line` and the ranges
/// touching it are passed over as one; the first range after it (before it in reverse) wins, else the first range.
fn nearest_commenting_range(ranges: &[(usize, usize)], line: usize, reverse: bool) -> Option<(usize, usize)> {
    let ordered: Vec<(usize, usize)> = if reverse { ranges.iter().rev().copied().collect() } else { ranges.to_vec() };
    let mut containing: Option<(usize, usize)> = None;
    for &(start, end) in &ordered {
        if let Some((within_start, within_end)) = containing {
            if end + 1 >= within_start && within_end + 1 >= start {
                containing = Some((within_start.min(start), within_end.max(end)));
                continue;
            }
        }
        if start <= line && line <= end {
            containing = Some((start, end));
            continue;
        }
        if (!reverse && end < line) || (reverse && start > line) {
            continue;
        }
        return Some((start, end));
    }
    ordered.first().copied()
}

/// The sort and `findFirstIdxMonotonousOrArrLen` of `_findNearestCommentThread`: threads without a range sort first;
/// forward takes the first starting after `after`, reverse the last starting before it.
fn nearest_thread(mut threads: Vec<(u64, u64, Option<Value>)>, after: Position, reverse: bool) -> Option<(u64, u64, Option<Value>)> {
    let start = |range: &Option<Value>| range.as_ref().map(|range| position(range, "startLineNumber", "startColumn"));
    threads.sort_by_key(|(.., range)| start(range).map(|start| (start.line, start.column)));
    if reverse {
        threads.reverse();
        return threads.into_iter().find(|(.., range)| start(range).is_none_or(|start| (after.line, after.column) > (start.line, start.column)));
    }
    threads.into_iter().find(|(.., range)| start(range).is_some_and(|start| (start.line, start.column) > (after.line, after.column)))
}

fn position(range: &Value, line: &str, column: &str) -> Position {
    let number = |key: &str| (range[key].as_u64().unwrap_or(1) as usize).max(1) - 1;
    Position::new(number(line), number(column))
}
