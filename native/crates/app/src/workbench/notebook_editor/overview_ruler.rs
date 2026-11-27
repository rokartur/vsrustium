//! notebookOverviewRuler.ts: the 10px canvas at the list's right edge, under the scrollbar, with the cells' overview
//! ruler decorations (only find's in the port) scaled from the scroll height to the list height.

use gpui::{Bounds, Div, Window, canvas, div, fill, point, prelude::*, px, size};

use super::super::Workbench;
use super::CellRow;

pub(super) const RULER_WIDTH: f32 = 10.;
/// `_lanes`: Left, Center and Right; find draws in the center one.
const LANES: f32 = 3.;

/// `INotebookCellDecorationOptions.overviewRuler` of one cell, in the center lane.
pub(in crate::workbench) struct RulerDecoration {
    pub cell: usize,
    pub color: &'static str,
    /// The start lines of `modelRanges`, from 0.
    pub lines: Vec<usize>,
    pub include_output: bool,
}

/// A cell's heights and line count as `_render` reads them.
#[derive(Clone, Copy)]
struct RulerCell {
    index: usize,
    height: f32,
    editor_height: f32,
    lines: usize,
}

impl Workbench {
    pub(super) fn notebook_overview_ruler(&self, tab: u32, rows: &[CellRow], window: &Window) -> Div {
        let decorations = self.notebook_find_ruler_decorations(tab);
        let scroller = self.notebook_list_scroller(tab);
        let bounds = scroller.bounds();
        let scroll_height = scroller.max_position().y + f32::from(bounds.size.height);
        let Some(model) = self.host.notebooks.editor(tab).and_then(|editor| self.host.notebooks.model(&editor.notebook)) else { return div() };
        let cells: Vec<RulerCell> = rows
            .iter()
            .map(|row| RulerCell { index: row.index, height: row.layout.height, editor_height: row.layout.editor_height, lines: self.cell_line_count(&model.cells[row.index].uri) })
            .collect();
        let marks = ruler_marks(&cells, &decorations, f32::from(bounds.size.height), scroll_height, self.notebook_line_height(), window.scale_factor());
        let marks: Vec<_> = marks.into_iter().map(|(bounds, color)| (bounds, self.color(color))).collect();
        div().absolute().top_0().right_0().bottom_0().w(px(RULER_WIDTH)).child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    for (mark, color) in &marks {
                        window.paint_quad(fill(Bounds::new(bounds.origin + mark.origin, mark.size), *color));
                    }
                },
            )
            .size_full(),
        )
    }
}

/// `_render` in logical pixels from the ruler's top left. The canvas is `ratio` times larger and the reference caps
/// a line's height at the editor line height in canvas pixels, so on a 2x display a line is at most half of it.
fn ruler_marks(cells: &[RulerCell], decorations: &[RulerDecoration], height: f32, scroll_height: f32, line_height: f32, ratio: f32) -> Vec<(Bounds<gpui::Pixels>, &'static str)> {
    let lane = RULER_WIDTH / LANES;
    let mut marks = Vec::new();
    if scroll_height <= 0. {
        return marks;
    }
    let scale = height / scroll_height;
    let mut current_from = 0.;
    for cell in cells {
        for decoration in decorations.iter().filter(|decoration| decoration.cell == cell.index) {
            let line = (line_height / ratio).min(cell.editor_height * scale / cell.lines.max(1) as f32);
            let mut previous = None;
            for &start in &decoration.lines {
                if previous == Some(start) {
                    continue;
                }
                previous = Some(start);
                marks.push((Bounds::new(point(px(lane), px(current_from + start as f32 * line)), size(px(lane), px(line))), decoration.color));
            }
            if decoration.include_output {
                let top = current_from + cell.editor_height * scale;
                marks.push((Bounds::new(point(px(lane), px(top)), size(px(lane), px(line_height * scale))), decoration.color));
            }
        }
        current_from += cell.height * scale;
    }
    marks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_scale_to_the_list_and_cap_lines_at_the_canvas_line_height() {
        let cells = [RulerCell { index: 0, height: 176., editor_height: 0., lines: 1 }, RulerCell { index: 1, height: 120., editor_height: 56., lines: 2 }];
        let decorations = [
            RulerDecoration { cell: 1, color: "find", lines: vec![0, 0, 1], include_output: true },
            RulerDecoration { cell: 1, color: "current", lines: vec![1], include_output: false },
        ];
        let marks = ruler_marks(&cells, &decorations, 600., 1200., 18., 2.);
        let tops: Vec<(f32, f32, &str)> = marks.iter().map(|(bounds, color)| (f32::from(bounds.origin.y), f32::from(bounds.size.height), *color)).collect();
        // Cell 1 starts at 176 / 2; a line is min(18 / 2, 56 / 2 / 2) = 9; the output at 56 / 2 below, 18 / 2 tall.
        assert_eq!(tops, vec![(88., 9., "find"), (97., 9., "find"), (116., 9., "find"), (97., 9., "current")]);
        assert!(marks.iter().all(|(bounds, _)| f32::from(bounds.origin.x) == RULER_WIDTH / 3. && f32::from(bounds.size.width) == RULER_WIDTH / 3.));
    }
}
