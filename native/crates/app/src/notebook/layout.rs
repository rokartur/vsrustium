//! Cell geometry of the notebook list: notebookOptions.ts in its default compact view, with the layouts of
//! codeCellViewModel.ts and markupCellViewModel.ts. Tops are in list content coordinates, which are also the back
//! layer webview's document coordinates (`cellTop` of its `view-scroll` messages).

#[cfg(test)]
mod tests;

/// notebookOptions.ts `SCROLLABLE_ELEMENT_PADDING_TOP`.
pub const LIST_PADDING_TOP: f32 = 18.;
pub const CELL_TOP_MARGIN: f32 = 6.;
pub const CELL_BOTTOM_MARGIN: f32 = 6.;
pub const CELL_RIGHT_MARGIN: f32 = 16.;
/// `compactConfigConstants`.
pub const CODE_CELL_LEFT_MARGIN: f32 = 8.;
pub const CELL_RUN_GUTTER: f32 = 36.;
pub const MARKDOWN_CELL_TOP_MARGIN: f32 = 6.;
pub const MARKDOWN_CELL_BOTTOM_MARGIN: f32 = 6.;
pub const MARKDOWN_CELL_LEFT_MARGIN: f32 = 8.;
pub const MARKDOWN_CELL_GUTTER: f32 = 36.;
pub const FOCUS_INDICATOR_LEFT_MARGIN: f32 = 4.;
pub const STATUS_BAR_HEIGHT: f32 = 22.;
pub const OUTPUT_PADDING: f32 = 8.;
pub const MARKDOWN_PREVIEW_PADDING: f32 = 8.;
pub const EDITOR_TOP_PADDING: f32 = 12.;
pub const EDITOR_BOTTOM_PADDING: f32 = 4.;
/// `_computeBottomToolbarDimensions` with the cell toolbar on the right (`notebook.cellToolbarLocation`).
pub const BOTTOM_TOOLBAR_GAP: f32 = 18.;
pub const BOTTOM_TOOLBAR_HEIGHT: f32 = 18.;
/// markupCellViewModel.ts: a preview without its height from the webview yet.
const UNMEASURED_PREVIEW_HEIGHT: f32 = 100.;

/// What a cell's height depends on.
#[derive(Clone, Copy, Debug)]
pub enum CellInput {
    /// A code cell, or a markup cell being edited: its line count and its outputs' heights from the webview.
    Editor { lines: usize, outputs_height: f32, markup: bool },
    /// A markup cell's rendered preview, its height from the webview once measured.
    Preview { height: Option<f32> },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellLayout {
    pub top: f32,
    pub height: f32,
    /// The code editor's box, padding included, from the cell's top.
    pub editor_top: f32,
    pub editor_height: f32,
    /// The outputs, from the cell's top (`outputContainerOffset`).
    pub output_top: f32,
}

/// The cells one below the other after the list's top padding, and the list's content height.
pub fn layout(cells: &[CellInput], line_height: f32) -> (Vec<CellLayout>, f32) {
    let mut top = LIST_PADDING_TOP;
    let mut layouts = Vec::with_capacity(cells.len());
    for cell in cells {
        let cell_layout = match *cell {
            CellInput::Editor { lines, outputs_height, markup } => {
                let editor_height = lines.max(1) as f32 * line_height + EDITOR_TOP_PADDING + EDITOR_BOTTOM_PADDING;
                let (editor_top, margins) = match markup {
                    true => (MARKDOWN_CELL_TOP_MARGIN, MARKDOWN_CELL_TOP_MARGIN + MARKDOWN_CELL_BOTTOM_MARGIN),
                    false => (CELL_TOP_MARGIN, CELL_TOP_MARGIN + CELL_BOTTOM_MARGIN),
                };
                let output_top = editor_top + editor_height + STATUS_BAR_HEIGHT;
                let height = margins + editor_height + STATUS_BAR_HEIGHT + outputs_height + BOTTOM_TOOLBAR_GAP;
                CellLayout { top, height, editor_top, editor_height, output_top }
            }
            CellInput::Preview { height } => {
                let preview = height.unwrap_or(UNMEASURED_PREVIEW_HEIGHT);
                CellLayout { top, height: (preview + BOTTOM_TOOLBAR_GAP).max(1.), editor_top: 0., editor_height: 0., output_top: preview }
            }
        };
        top += cell_layout.height;
        layouts.push(cell_layout);
    }
    (layouts, top)
}

/// `computeBottomToolbarOffset`: where the add cell toolbar sits, from the cell's top.
pub fn bottom_toolbar_offset(cell: &CellLayout) -> f32 {
    cell.height - BOTTOM_TOOLBAR_GAP - BOTTOM_TOOLBAR_HEIGHT / 2.
}
