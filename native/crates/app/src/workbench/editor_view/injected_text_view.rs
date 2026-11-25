//! viewLineRenderer.ts with injected text: the line's text drawn in pieces between the injected texts, each piece at
//! its x from the line's `LineLayout`, and each injected text drawn by its owner in the gap left for it.

use std::ops::Range;

use gpui::{AnyElement, Context, Div, HighlightStyle, StyledText, div, prelude::*, px};

use super::{EditorMetrics, Workbench};
use crate::editor::injected_text::{InjectionOwner, LineLayout};
use crate::editor::{OpenEditor, expand_tabs};

/// The drawn line `rendered` (tabs expanded, `runs` by its bytes) cut at the injected texts' columns of `text`.
pub(super) fn line_text_pieces(rendered: String, runs: Vec<(Range<usize>, HighlightStyle)>, text: &str, layout: &LineLayout, tab_size: usize, char_width: f32) -> Vec<Div> {
    let mut cuts: Vec<usize> = layout.injection_x().map(|(index, _)| layout.injection_column(index)).filter(|column| *column > 0).collect();
    cuts.dedup();
    let byte = |column: usize| expand_tabs(&text.chars().take(column).collect::<String>(), tab_size).len().min(rendered.len());
    let mut pieces = Vec::new();
    let mut start_column = 0;
    for end_column in cuts.into_iter().chain([usize::MAX]) {
        let (start, end) = (byte(start_column), if end_column == usize::MAX { rendered.len() } else { byte(end_column) });
        if start < end || start_column == 0 {
            let piece_runs: Vec<_> = runs.iter().filter(|(range, _)| range.end > start && range.start < end).map(|(range, style)| (range.start.max(start) - start..range.end.min(end) - start, *style)).collect();
            let left = layout.char_x(start_column) * char_width;
            pieces.push(div().absolute().top_0().left(px(left)).whitespace_nowrap().child(StyledText::new(rendered[start..end].to_string()).with_highlights(piece_runs)));
        }
        start_column = end_column;
    }
    pieces
}

impl Workbench {
    /// Each injected text of model line `line`, drawn by its owner at its x.
    pub(super) fn injected_text_elements(&self, editor: &OpenEditor, line: usize, layout: &LineLayout, metrics: &EditorMetrics, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let injections = editor.injected_text_on(line);
        let mut elements = Vec::new();
        for (index, x) in layout.injection_x() {
            let injection = &injections[index];
            let left = x * metrics.char_width;
            let element = match injection.owner {
                InjectionOwner::InlayHint => self.inlay_hint_element(editor, injection.id, left, metrics.line_height, cx),
                InjectionOwner::Color => self.color_decorator_element(injection.id, left, metrics.line_height, cx),
                InjectionOwner::GhostText => self.ghost_text_element(injection.id, left, metrics.line_height),
            };
            elements.extend(element);
        }
        elements
    }
}
