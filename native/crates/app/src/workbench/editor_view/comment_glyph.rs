//! commentGlyphWidget.ts: the gutter glyph of each comment thread on the last line of its range, a 3px
//! `editorGutter.commentRangeForeground` bar with an 18px box holding the comment icon (review.css
//! `.comment-range-glyph`). shortcut: a click does not open the thread widget, which is not ported yet.

use gpui::{Div, div, prelude::*, px};

use super::EditorMetrics;
use crate::editor::OpenEditor;
use crate::workbench::Workbench;

/// commentsController.ts `getWithCommentsLineDecorationWidth`: `lineDecorationsWidth` 24px wider, less the 11px the
/// folding controls had.
pub(super) const RESERVED_WIDTH: f32 = 13.;

impl Workbench {
    /// `tryUpdateReservedSpace` for the threads of the editor's model. shortcut: commenting ranges are not asked, so
    /// only threads reserve the space; upgrade with `$provideCommentingRanges`.
    pub(super) fn editor_has_comment_threads(&self, editor: &OpenEditor) -> bool {
        let uri = editor.document().uri();
        self.comment_threads_of(&uri).next().is_some()
    }

    fn comment_threads_of<'a>(&'a self, uri: &'a serde_json::Value) -> impl Iterator<Item = &'a crate::workbench::comments::Thread> {
        let threads = self.comments.controllers.iter().flat_map(|controller| &controller.threads);
        threads.filter(move |thread| thread.resource["path"] == uri["path"] && thread.resource["scheme"] == uri["scheme"])
    }

    pub(in crate::workbench) fn comment_glyph(&self, editor: &OpenEditor, index: usize, metrics: EditorMetrics) -> Option<Div> {
        let uri = editor.document().uri();
        let thread = self.comment_threads_of(&uri).find(|thread| {
            let end = thread.range.as_ref().and_then(|range| range["endLineNumber"].as_u64());
            end == Some(index as u64 + 1)
        })?;
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
        Some(div().absolute().top_0().left(px(left)).h(px(metrics.line_height)).w(px(20.)).child(bar).child(glyph))
    }
}
