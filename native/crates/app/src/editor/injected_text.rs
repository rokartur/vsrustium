//! Injected text (model.ts `InjectedTextOptions`, modelLineProjectionData.ts): text an editor shows between a model
//! line's chars without it being in the model (inlay hints, color boxes, ghost text). It moves every later x of
//! its line right, so every x of a model column goes through `LineLayout`.
// shortcut: the cursor keeps model positions, so a cursor at an injection always draws before it, where upstream's
// view positions also stop after it (`InjectedTextCursorStops`); needs view positions in the cursor moves.

use super::cursor::Position;
use super::folding::tracked;
use super::{OpenEditor, char_columns, edit::end_of};

/// Who injected the text; each owner draws its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InjectionOwner {
    InlayHint,
    Color,
    GhostText,
}

/// One injected text before the char at `position` (after the line's last char at its length).
#[derive(Clone, Debug, PartialEq)]
pub struct InjectedText {
    pub position: Position,
    /// The drawn width in editor columns (fractional when the text uses another font size or has padding).
    pub columns: f32,
    pub owner: InjectionOwner,
    /// The owner's index of what it drew here.
    pub id: usize,
    /// `AlwaysGrowsWhenTypingAtEdges` on the owner's range: an injection at the range's start keeps to the
    /// char before it when text is typed at it, one at the end moves with the typed text.
    pub at_range_start: bool,
}

/// The x of every model column of one line, in editor columns from the line's start: tabs expanded as
/// `visible_column`, the injected texts' widths added (`ModelLineProjectionData.translateToOutputPosition`).
// shortcut: tab stops after an injected text ignore its width, where upstream counts its chars; a tab right after
// an inlay hint draws up to one tab stop off. Count the injected chars in `visible_column` when that shows up.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineLayout {
    /// The visible column of each char boundary, `len + 1` entries.
    visible: Vec<usize>,
    /// (char column, width) of each injection, by column.
    injections: Vec<(usize, f32)>,
}

impl LineLayout {
    pub fn new(text: &str, tab_size: usize, injections: impl IntoIterator<Item = (usize, f32)>) -> Self {
        let mut visible = Vec::with_capacity(text.len() + 1);
        let mut column = 0;
        visible.push(0);
        for c in text.chars() {
            column += char_columns(c, column, tab_size);
            visible.push(column);
        }
        LineLayout { visible, injections: injections.into_iter().collect() }
    }

    fn len(&self) -> usize {
        self.visible.len() - 1
    }

    fn injected_before(&self, column: usize, including_column: bool) -> f32 {
        self.injections.iter().take_while(|(at, _)| *at < column || (including_column && *at == column)).map(|(_, width)| width).sum()
    }

    /// `PositionAffinity.None`: a position at an injection is before it, as the cursor draws.
    pub fn x(&self, column: usize) -> f32 {
        let column = column.min(self.len());
        self.visible[column] as f32 + self.injected_before(column, false)
    }

    /// `PositionAffinity.Right`: where the char at `column` starts, after the injections at it.
    pub fn char_x(&self, column: usize) -> f32 {
        let column = column.min(self.len());
        self.visible[column] as f32 + self.injected_before(column, true)
    }

    /// `convertModelRangeToViewRange`: an empty range sits before the injections at it; a range starts after
    /// the injections at its start and ends before the ones at its end.
    pub fn range(&self, start: usize, end: usize) -> (f32, f32) {
        match start == end {
            true => (self.x(start), self.x(start)),
            false => (self.char_x(start), self.x(end)),
        }
    }

    /// The line's width with the injections after its last char.
    pub fn width(&self) -> f32 {
        self.char_x(self.len())
    }

    /// The model column nearest to `x` for a click: a char takes the clicks up to its middle, a click on an
    /// injection goes to its column.
    pub fn column_at(&self, x: f32) -> usize {
        for column in 0..self.len() {
            let start = self.char_x(column);
            if x < start {
                return column;
            }
            // `columnFromVisibleColumn`: the left column on a tie.
            if x <= (start + self.x(column + 1)) / 2. {
                return column;
            }
        }
        self.len()
    }

    /// `column_at` as the visible column of the model text alone, which the click handlers take.
    pub fn model_visible(&self, x: f32) -> f32 {
        self.visible[self.column_at(x)] as f32
    }

    pub fn injection_column(&self, index: usize) -> usize {
        self.injections[index].0
    }

    /// The injections of the line with their x, for drawing.
    pub fn injection_x(&self) -> impl Iterator<Item = (usize, f32)> + '_ {
        self.injections.iter().enumerate().map(|(index, &(column, _))| (index, self.visible[column.min(self.len())] as f32 + self.injections[..index].iter().map(|(_, width)| width).sum::<f32>()))
    }
}

impl OpenEditor {
    /// The injected texts on model line `line`, by column.
    pub fn injected_text_on(&self, line: usize) -> &[InjectedText] {
        let first = self.injected_text.partition_point(|injection| injection.position.line < line);
        let count = self.injected_text[first..].partition_point(|injection| injection.position.line == line);
        &self.injected_text[first..first + count]
    }

    /// The x of every column of model line `line`.
    pub fn line_layout(&self, line: usize) -> LineLayout {
        let document = self.document();
        LineLayout::new(&document.lines[line], document.indentation.tab_size, self.injected_text_on(line).iter().map(|injection| (injection.position.column, injection.columns)))
    }

    /// `changeDecorations` of one owner's injected text: replaces what it injected before; the lines with
    /// changed injections break again.
    pub fn set_injected_text(&mut self, owner: InjectionOwner, mut injections: Vec<InjectedText>) {
        let mut lines: Vec<usize> = self.injected_text.iter().filter(|injection| injection.owner == owner).map(|injection| injection.position.line).collect();
        lines.extend(injections.iter().map(|injection| injection.position.line));
        self.injected_text.retain(|injection| injection.owner != owner);
        self.injected_text.append(&mut injections);
        // Stable, so the owner's order holds among the injections at one position.
        self.injected_text.sort_by_key(|injection| injection.position);
        lines.sort_unstable();
        lines.dedup();
        let document = self.model.borrow();
        for line in lines.into_iter().filter(|line| *line < document.document.lines.len()) {
            self.word_wrap.line_injections_changed(&document.document.lines, line, &self.injected_text);
        }
    }

    /// The injections follow an edit like their owner's decorations (intervalTree.ts `nodeAcceptEdit`).
    pub(super) fn track_injected_text(&mut self, start: Position, end: Position, text: &str) {
        let new_end = end_of(start, text);
        for injection in &mut self.injected_text {
            injection.position = tracked(injection.position, start, end, new_end, injection.at_range_start);
        }
        self.injected_text.sort_by_key(|injection| injection.position);
    }
}

#[cfg(test)]
mod tests;
