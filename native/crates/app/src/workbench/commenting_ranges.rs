//! commentsController.ts `CommentingRangeDecorator` and `addOrToggleCommentAtLine`: the active model's commenting ranges
//! as `$provideCommentingRanges` answers them, a 3px `editorGutter.commentRangeForeground` bar on their lines, the `+`
//! glyph on the hovered line, and a click on it (or a drag along it) that asks the controller for a new thread
//! (`$createCommentThreadTemplate`), which opens with its reply form focused.

use std::time::Duration;

use futures::future::join_all;
use gpui::{Bounds, Context, Pixels, Point, Task, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::OpenEditor;
use crate::menus::MenuAction;

/// `_computeCommentingRangeScheduler`: the ranges are asked again 200ms after the last edit.
const CONTENT_CHANGE_DELAY: Duration = Duration::from_millis(200);

/// commentsAccessibility.ts `CommentsAccessibilityHelpProvider.provideContent`, the help of a focused comment input.
pub(super) const COMMENTS_ACCESSIBILITY_HELP: [&str; 7] = [
    "This widget contains a text area, for composition of new comments, and actions, that can be tabbed to once tab moves focus mode has been enabled with the command Toggle Tab Key Moves Focus<keybinding:editor.action.toggleTabFocusMode>.",
    "Some useful comment commands include:",
    "- Dismiss Comment<keybinding:workbench.action.hideComment>.",
    "- Add Comment on Current Selection<keybinding:workbench.action.addComment>.",
    "- Submit Comment<keybinding:editor.action.submitComment>.",
    "- Go to Next Commenting Range<keybinding:editor.action.nextCommentingRange>.",
    "- Go to Previous Commenting Range<keybinding:editor.action.previousCommentingRange>.",
];

/// editorAccessibilityHelp.ts `getCommentCommandInfo`.
const COMMENT_COMMAND_INFO: &str = "The editor contains commentable range(s). Some useful commands include:
- Add Comment on Current Selection<keybinding:workbench.action.addComment>.
- Go to Next Comment Thread<keybinding:editor.action.nextCommentThreadAction>.
- Go to Previous Comment Thread<keybinding:editor.action.previousCommentThreadAction>.
- Go to Next Commenting Range<keybinding:editor.action.nextCommentingRange>.
- Go to Previous Commenting Range<keybinding:editor.action.previousCommentingRange>.";

#[derive(Default)]
pub(super) struct CommentingRanges {
    /// The active model's ranges per controller, 0-based inclusive lines.
    pub(super) ranges: Vec<ControllerRanges>,
    request: Option<Task<()>>,
    /// `updateHover`: the line whose `+` glyph shows.
    pub(super) hover_line: Option<usize>,
    /// `mouseDownInfo`: the line a press on the gutter glyph started on.
    mouse_down_line: Option<usize>,
}

pub(super) struct ControllerRanges {
    controller: u64,
    resource: Value,
    lines: Vec<(usize, usize)>,
}

impl Workbench {
    pub(super) fn register_commenting_commands(&mut self) {
        // commentsEditorContribution.ts `CommentCommandId.Add`: the selection, or the cursor's line.
        self.commands.register("workbench.action.addComment", |workbench, args, window, cx| {
            let Some(editor) = workbench.editors.active() else { return Outcome::done() };
            let range = match args.first().map(|arg| &arg["range"]) {
                Some(range) if range.is_object() => Some((line_of(range, "startLineNumber"), line_of(range, "endLineNumber"))),
                _ => Some((editor.selection().start().line, editor.selection().end().line)).filter(|_| args.first().is_none_or(|arg| arg["fileComment"] != true)),
            };
            workbench.add_or_toggle_comment_at(range, None, window, cx);
            Outcome::done()
        });
    }

    /// `workspaceHasCommenting`, `activeCursorHasCommentingRange`, `activeEditorHasCommentingRange` and
    /// `activeCursorHasComment`.
    pub(super) fn commenting_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        context.insert("workspaceHasCommenting".into(), json!(self.comments_view_registered()));
        let Some(editor) = self.editors.active() else { return };
        let line = editor.selection().head.line;
        let ranges = self.commenting_ranges_of(editor);
        context.insert("activeEditorHasCommentingRange".into(), json!(ranges.clone().next().is_some()));
        context.insert("activeCursorHasCommentingRange".into(), json!(ranges.flat_map(|ranges| &ranges.lines).any(|(start, end)| (*start..=*end).contains(&line))));
        let uri = editor.document().uri();
        let threads = self.comments.controllers.iter().flat_map(|controller| &controller.threads);
        let has_comment = threads.filter(|thread| thread.resource["path"] == uri["path"] && thread.resource["scheme"] == uri["scheme"]).any(|thread| {
            thread.range.as_ref().is_some_and(|range| (line_of(range, "startLineNumber")..=line_of(range, "endLineNumber")).contains(&line))
        });
        context.insert("activeCursorHasComment".into(), json!(has_comment));
    }

    fn commenting_ranges_of<'a>(&'a self, editor: &OpenEditor) -> impl Iterator<Item = &'a ControllerRanges> + Clone {
        let uri = editor.document().uri();
        self.comments.commenting.ranges.iter().filter(move |ranges| ranges.resource["path"] == uri["path"] && ranges.resource["scheme"] == uri["scheme"])
    }

    /// The commenting ranges of `editor`'s model by start line, each controller's together.
    pub(super) fn commenting_lines(&self, editor: &OpenEditor) -> Vec<(usize, usize)> {
        let mut lines: Vec<(usize, usize)> = self.commenting_ranges_of(editor).flat_map(|ranges| ranges.lines.iter().copied()).collect();
        lines.sort();
        lines
    }

    /// `getCommentCommandInfo`: the comment commands, when the active editor has commenting ranges.
    pub(super) fn comment_command_info(&self) -> Option<&'static str> {
        let editor = self.editors.active()?;
        self.commenting_ranges_of(editor).next().is_some().then_some(COMMENT_COMMAND_INFO)
    }

    /// Whether the editor's model has commenting ranges, which reserve the gutter space as threads do.
    pub(super) fn editor_has_commenting_ranges(&self, editor: &OpenEditor) -> bool {
        self.commenting_ranges_of(editor).any(|ranges| !ranges.lines.is_empty())
    }

    /// Whether line `index` is in one of the model's commenting ranges.
    pub(super) fn line_has_commenting_range(&self, editor: &OpenEditor, index: usize) -> bool {
        self.commenting_ranges_of(editor).flat_map(|ranges| &ranges.lines).any(|(start, end)| (*start..=*end).contains(&index))
    }

    /// `beginCompute`: every controller that set a commenting range provider answers for the active model; with
    /// `delay` (an edit) the request waits `CONTENT_CHANGE_DELAY`.
    pub(super) fn refresh_commenting_ranges(&mut self, delay: bool, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else {
            self.comments.commenting.ranges.clear();
            self.comments.commenting.request = None;
            return;
        };
        let uri = editor.document().uri();
        let cursor_line = editor.selection().head.line;
        let controllers: Vec<u64> = self.comments.controllers.iter().filter(|controller| controller.has_commenting_ranges).map(|controller| controller.handle).collect();
        if controllers.is_empty() {
            self.comments.commenting.ranges.clear();
            return;
        }
        self.comments.commenting.request = Some(cx.spawn(async move |workbench, cx| {
            if delay {
                cx.background_executor().timer(CONTENT_CHANGE_DELAY).await;
            }
            let Ok(calls) = workbench.update(cx, |workbench, _| {
                let ask = |handle: &u64| workbench.host.call_cancellable("ExtHostComments", "$provideCommentingRanges", vec![json!(handle), uri.clone()]);
                controllers.iter().map(ask).collect::<Vec<_>>()
            }) else {
                return;
            };
            let answers = join_all(calls).await;
            let _ = workbench.update(cx, |workbench, cx| {
                let mut ranges = Vec::new();
                for (controller, answer) in controllers.into_iter().zip(answers) {
                    let answer = answer.ok().and_then(Result::ok).unwrap_or(Value::Null);
                    let lines = answer["ranges"].as_array().into_iter().flatten().map(|range| (line_of(range, "startLineNumber"), line_of(range, "endLineNumber"))).collect();
                    ranges.push(ControllerRanges { controller, resource: uri.clone(), lines });
                }
                let commenting = &mut workbench.comments.commenting;
                commenting.ranges = ranges;
                // `update(editor, infos, cursorLine)`: the cursor's line is emphasized until the pointer moves.
                commenting.hover_line = commenting.hover_line.or(Some(cursor_line));
                cx.notify();
            });
        }));
    }

    /// `onEditorMouseMove` / `onEditorMouseLeave`: the `+` glyph follows the pointer's line.
    pub(super) fn commenting_hover_moved(&mut self, line: Option<usize>, cx: &mut Context<Self>) {
        if self.comments.commenting.hover_line != line {
            self.comments.commenting.hover_line = line;
            cx.notify();
        }
    }

    /// `onEditorMouseDown` on a `comment-range-glyph`.
    pub(super) fn commenting_glyph_mouse_down(&mut self, line: usize) {
        self.comments.commenting.mouse_down_line = Some(line);
    }

    /// `onEditorMouseUp` on a `comment-range-glyph`: from the press's line to this one, or this line alone.
    pub(super) fn commenting_glyph_mouse_up(&mut self, line: usize, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(down) = self.comments.commenting.mouse_down_line.take() else { return };
        self.add_or_toggle_comment_at(Some((down.min(line), down.max(line))), Some(position), window, cx);
    }

    /// `addOrToggleCommentAtLine`: the threads whose glyph is on the range's last line toggle; else the controllers
    /// with a commenting range there are asked for a new thread, through a menu when there are several.
    fn add_or_toggle_comment_at(&mut self, lines: Option<(usize, usize)>, anchor: Option<Point<Pixels>>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let uri = editor.document().uri();
        if let Some((_, end)) = lines {
            let threads: Vec<(u64, u64)> = self
                .comments
                .controllers
                .iter()
                .flat_map(|controller| controller.threads.iter().map(move |thread| (controller.handle, thread)))
                .filter(|(_, thread)| thread.resource["path"] == uri["path"] && thread.range.as_ref().is_some_and(|range| line_of(range, "endLineNumber") == end))
                .map(|(controller, thread)| (controller, thread.handle))
                .collect();
            if !threads.is_empty() {
                for (controller, thread) in threads {
                    self.toggle_comment_thread(controller, thread, cx);
                }
                return;
            }
        }
        let Some((start, end)) = lines else {
            return self.notify_error("File comments are not allowed on this file.");
        };
        let matched: Vec<(u64, (usize, usize))> = self
            .commenting_ranges_of(editor)
            .flat_map(|ranges| ranges.lines.iter().map(move |lines| (ranges.controller, *lines)))
            .filter(|(_, (range_start, range_end))| *range_start <= end && start <= *range_end)
            .collect();
        if matched.is_empty() {
            return self.notify_error("The cursor must be within a commenting range to add a comment.");
        }
        let document = editor.document();
        // `clipUserRangeToCommentRange`, then a one-line range at column 1 or the dragged lines in full.
        let range_for = |(range_start, range_end): (usize, usize)| {
            let (first, last) = (start.max(range_start), end.min(range_end));
            let end_column = if first == last { 1 } else { document.lines[last].encode_utf16().count() + 1 };
            json!({ "startLineNumber": first + 1, "startColumn": 1, "endLineNumber": last + 1, "endColumn": end_column })
        };
        let matched: Vec<(u64, Value)> = matched.into_iter().map(|(controller, range)| (controller, range_for(range))).collect();
        drop(document);
        if let [(controller, range)] = matched.as_slice() {
            return self.create_comment_thread_template(*controller, range.clone());
        }
        // `getContextMenuActions` with the mouse, else the "Select Comment Provider" pick; both list the labels.
        let mut actions = Vec::new();
        for (controller, range) in matched {
            let label = self.comments.controllers.iter().find(|candidate| candidate.handle == controller).map(|controller| controller.label.clone()).unwrap_or_default();
            let id = format!("comments.addCommentThread.{controller}");
            self.commands.register(id.clone(), move |workbench, _, _, _| {
                workbench.create_comment_thread_template(controller, range.clone());
                Outcome::done()
            });
            actions.push(MenuAction::Command { id, title: label, enabled: true, checked: false, alt: None });
        }
        let position = anchor.or_else(|| self.cursor_anchor(window)).unwrap_or_default();
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, Bounds { origin: position, size: gpui::Size::default() }, Vec::new(), window, cx);
    }

    /// `addCommentAtLine2`: the controller creates a template thread, which comes back through `$createCommentThread`.
    fn create_comment_thread_template(&mut self, controller: u64, range: Value) {
        let Some(editor) = self.editors.active() else { return };
        drop(self.host.call("ExtHostComments", "$createCommentThreadTemplate", vec![json!(controller), editor.document().uri(), range]));
    }

    /// `deleteCommentThread` of an empty thread's Delete (or its collapse): the controller disposes it.
    pub(super) fn delete_comment_thread(&mut self, controller: u64, thread: u64, cx: &mut Context<Self>) {
        drop(self.host.call("ExtHostComments", "$deleteCommentThread", vec![json!(controller), json!(thread)]));
        if let Some(owner) = self.comments.controllers.iter_mut().find(|candidate| candidate.handle == controller) {
            owner.threads.retain(|candidate| candidate.handle != thread);
        }
        if self.comments.reply.as_ref().is_some_and(|reply| reply.controller == controller && reply.thread == thread) {
            self.comments.reply = None;
        }
        self.sync_comment_widgets();
        cx.notify();
    }
}

fn line_of(range: &Value, key: &str) -> usize {
    (range[key].as_u64().unwrap_or(1) as usize).max(1) - 1
}

#[cfg(test)]
mod tests;
