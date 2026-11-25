//! commentGlyphWidget.ts: the gutter glyph of each comment thread on the last line of its range, a 3px
//! `editorGutter.commentRangeForeground` bar with an 18px box holding the comment icon (review.css
//! `.comment-range-glyph`). A click toggles the thread widget (commentsController.ts `onEditorMouseUp`).

use gpui::{Context, Div, MouseButton, div, prelude::*, px};

use super::EditorMetrics;
use crate::editor::OpenEditor;
use crate::workbench::Workbench;

/// commentsController.ts `getWithCommentsLineDecorationWidth`: `lineDecorationsWidth` 24px wider, less the 11px the
/// folding controls had.
pub(super) const RESERVED_WIDTH: f32 = 13.;

impl Workbench {
    /// `tryUpdateReservedSpace`: the model has threads or commenting ranges.
    pub(super) fn editor_has_comment_threads(&self, editor: &OpenEditor) -> bool {
        let uri = editor.document().uri();
        self.comment_threads_of(&uri).next().is_some() || self.editor_has_commenting_ranges(editor)
    }

    fn comment_threads_of<'a>(&'a self, uri: &'a serde_json::Value) -> impl Iterator<Item = &'a crate::workbench::comments::Thread> {
        let threads = self.comments.controllers.iter().flat_map(|controller| &controller.threads);
        threads.filter(move |thread| thread.resource["path"] == uri["path"] && thread.resource["scheme"] == uri["scheme"])
    }

    pub(in crate::workbench) fn comment_glyph(&self, editor: &OpenEditor, index: usize, metrics: EditorMetrics, cx: &mut Context<Self>) -> Option<Div> {
        let uri = editor.document().uri();
        let thread = self.comments.controllers.iter().flat_map(|controller| controller.threads.iter().map(move |thread| (controller.handle, thread))).find(|(_, thread)| {
            let end = thread.range.as_ref().and_then(|range| range["endLineNumber"].as_u64());
            end == Some(index as u64 + 1) && thread.resource["path"] == uri["path"] && thread.resource["scheme"] == uri["scheme"]
        });
        let Some((controller, thread)) = thread else { return self.commenting_range_glyph(editor, index, metrics, cx) };
        let thread_handle = thread.handle;
        // Priority: draft > unresolved > resolved.
        let draft = thread.comments.iter().any(|comment| comment["state"] == 1);
        let (icon, glyph_color) = match (draft, thread.state) {
            (true, _) => ("comment-draft", "editorGutter.commentDraftGlyphForeground"),
            (false, Some(0)) => ("comment-unresolved", "editorGutter.commentUnresolvedGlyphForeground"),
            _ => ("comment", "editorGutter.commentGlyphForeground"),
        };
        let range_color = self.color("editorGutter.commentRangeForeground");
        let left = metrics.glyph_margin_width() + metrics.line_numbers_width;
        let bar = div().absolute().top_0().left(px(13.)).w(px(3.)).h_full().bg(range_color);
        let glyph = self.icon(icon, 18., 13., self.color(glyph_color)).absolute().top_0().left(px(2.)).h_full().rounded(px(3.)).bg(range_color).cursor_pointer();
        let toggle = cx.listener(move |workbench, _: &gpui::MouseDownEvent, _, cx| {
            cx.stop_propagation();
            workbench.toggle_comment_thread(controller, thread_handle, cx);
        });
        Some(div().absolute().top_0().left(px(left)).h(px(metrics.line_height)).w(px(20.)).child(bar).child(glyph).on_mouse_down(MouseButton::Left, toggle))
    }

    /// review.css `.comment-diff-added`: a 3px bar 10px into the lane on a commenting range's lines, and on the
    /// hovered one (`line-hover`) the 18px `+` box, which a click or a drag along the bars turns into a new thread.
    fn commenting_range_glyph(&self, editor: &OpenEditor, index: usize, metrics: EditorMetrics, cx: &mut Context<Self>) -> Option<Div> {
        if !self.line_has_commenting_range(editor, index) {
            return None;
        }
        let range_color = self.color("editorGutter.commentRangeForeground");
        let left = metrics.glyph_margin_width() + metrics.line_numbers_width;
        let bar = div().absolute().top_0().left(px(10.)).w(px(3.)).h_full().bg(range_color);
        let hovered = self.comments.commenting.hover_line == Some(index);
        let plus = hovered.then(|| self.icon("plus", 18., 16., self.color("editorGutter.commentGlyphForeground")).absolute().top_0().left(px(2.)).h_full().rounded(px(3.)).bg(range_color));
        let glyph = div()
            .absolute()
            .top_0()
            .left(px(left))
            .h(px(metrics.line_height))
            .w(px(20.))
            .cursor_pointer()
            .child(bar)
            .children(plus)
            .on_mouse_move(cx.listener(move |workbench, _: &gpui::MouseMoveEvent, _, cx| workbench.commenting_hover_moved(Some(index), cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &gpui::MouseDownEvent, _, cx| {
                cx.stop_propagation();
                workbench.commenting_glyph_mouse_down(index);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(move |workbench, event: &gpui::MouseUpEvent, window, cx| {
                cx.stop_propagation();
                workbench.commenting_glyph_mouse_up(index, event.position, window, cx);
            }));
        Some(glyph)
    }
}
