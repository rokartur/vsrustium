//! The workbench grid of browser/layout.ts for the floating (modern UI) layout with the panel at the bottom and
//! centered: a row of activity bar, primary side bar, the editor column (editor over panel) and secondary side bar.
//! Part views include the 4px floating card margins (floatingPanels.css), as the reference's grid views do.

use crate::split_view::{Priority, SashState, SplitView, View};

/// `--modern-ui-floating-card-margin` and `--modern-ui-floating-card-outer-margin`.
pub const CARD_MARGIN: f32 = 4.;
/// The activity bar's 44px card and its outer margin.
pub const ACTIVITY_BAR_WIDTH: f32 = 48.;
/// sidebarPart.ts and auxiliaryBarPart.ts `minimumWidth`.
const SIDE_BAR_MINIMUM_WIDTH: f32 = 170.;
/// panelPart.ts `minimumWidth` and `minimumHeight`.
const PANEL_MINIMUM_WIDTH: f32 = 300.;
const PANEL_MINIMUM_HEIGHT: f32 = 77.;
/// editor.ts `DEFAULT_EDITOR_MIN_DIMENSIONS`.
const EDITOR_MINIMUM_WIDTH: f32 = 220.;
const EDITOR_MINIMUM_HEIGHT: f32 = 70.;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    ActivityBar,
    SideBar,
    Panel,
    AuxiliaryBar,
}

/// The views of the middle row, in order.
const ROW_ACTIVITY_BAR: usize = 0;
const ROW_SIDE_BAR: usize = 1;
const ROW_EDITOR_COLUMN: usize = 2;
const ROW_AUXILIARY_BAR: usize = 3;
const COLUMN_EDITOR: usize = 0;
const COLUMN_PANEL: usize = 1;

/// A sash of the grid: the boundary it moves and its place in window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sash {
    /// Between the middle row's view `index` and the next.
    Row(usize),
    /// Between the editor and the panel.
    Column,
}

/// `LayoutStateModel`'s part sizes (`InitializationStateKey`s).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartSizes {
    pub side_bar: f32,
    pub panel: f32,
    pub auxiliary_bar: f32,
}

impl PartSizes {
    /// `LayoutStateModel.load`: the side bars take a quarter of the window up to 300, the panel a third of its height.
    pub fn defaults(window_width: f32, window_height: f32) -> Self {
        PartSizes { side_bar: (window_width / 4.).min(300.), panel: window_height / 3., auxiliary_bar: (window_width / 4.).min(300.) }
    }
}

pub struct Grid {
    row: SplitView,
    column: SplitView,
    width: f32,
    height: f32,
}

impl Grid {
    /// `createGridDescriptor` for a middle section of `width` by `height`; `visible` says which parts show.
    pub fn new(width: f32, height: f32, sizes: PartSizes, visible: impl Fn(Part) -> bool) -> Self {
        let panel = sizes.panel;
        let side = |size: f32, part| View::new(size, SIDE_BAR_MINIMUM_WIDTH, f32::INFINITY).snap().priority(Priority::Low).visible(visible(part));
        let activity_bar = View::new(ACTIVITY_BAR_WIDTH, ACTIVITY_BAR_WIDTH, ACTIVITY_BAR_WIDTH).visible(visible(Part::ActivityBar));
        let side_bar = side(sizes.side_bar, Part::SideBar);
        let auxiliary_bar = side(sizes.auxiliary_bar, Part::AuxiliaryBar);
        let editor_width = width - [&activity_bar, &side_bar, &auxiliary_bar].iter().filter(|view| view.visible).map(|view| view.size).sum::<f32>();
        // editorPart.ts: the editor snaps with the panel centered, at high priority.
        let editor = View::new(height - if visible(Part::Panel) { panel } else { 0. }, EDITOR_MINIMUM_HEIGHT, f32::INFINITY).snap().priority(Priority::High);
        let panel_view = View::new(panel, PANEL_MINIMUM_HEIGHT, f32::INFINITY).snap().visible(visible(Part::Panel));
        let mut column = SplitView::with_priorities(height, vec![editor, panel_view]);
        column.layout(height);
        let editor_column = View::new(editor_width, 0., f32::INFINITY).priority(Priority::High);
        let mut grid = Grid { row: SplitView::with_priorities(width, vec![activity_bar, side_bar, editor_column, auxiliary_bar]), column, width, height };
        grid.update_editor_column_limits();
        grid.row.layout(width);
        grid
    }

    pub fn layout(&mut self, width: f32, height: f32) {
        if (width, height) == (self.width, self.height) {
            return;
        }
        (self.width, self.height) = (width, height);
        self.update_editor_column_limits();
        self.row.layout(width);
        self.column.layout(height);
    }

    pub fn is_visible(&self, part: Part) -> bool {
        match part {
            Part::Panel => self.column.is_view_visible(COLUMN_PANEL),
            part => self.row.is_view_visible(row_index(part)),
        }
    }

    /// `setViewVisible` on the part's view.
    pub fn set_visible(&mut self, part: Part, visible: bool) {
        if self.is_visible(part) == visible {
            return;
        }
        match part {
            Part::Panel => self.column.set_view_visible(COLUMN_PANEL, visible),
            part => self.row.set_view_visible(row_index(part), visible),
        }
        self.update_editor_column_limits();
        self.row.layout(self.width);
    }

    pub fn is_editor_visible(&self) -> bool {
        self.column.is_view_visible(COLUMN_EDITOR)
    }

    /// `setEditorHidden`: a hidden editor maximizes the panel (`toggleMaximizedPanel`).
    pub fn set_editor_visible(&mut self, visible: bool) {
        self.column.set_view_visible(COLUMN_EDITOR, visible);
    }

    /// The part's size across its sash, or the size it comes back at while hidden (`getViewCachedVisibleSize`).
    pub fn size(&self, part: Part) -> f32 {
        match part {
            Part::Panel => self.column.visible_size(COLUMN_PANEL),
            part => self.row.visible_size(row_index(part)),
        }
    }

    /// The widths of the activity bar, side bar, editor column and secondary side bar.
    pub fn row_sizes(&self) -> Vec<f32> {
        self.row.sizes()
    }

    /// The heights of the editor and the panel.
    pub fn column_sizes(&self) -> Vec<f32> {
        self.column.sizes()
    }

    /// The sashes with their states and bounds (x, y, width, height) within the middle section. In the floating
    /// layout a sash fills the 4px gap after its boundary (`workbench-sash-start-at-gap`).
    pub fn sashes(&self) -> Vec<(Sash, SashState, [f32; 4])> {
        let mut sashes = Vec::new();
        let mut x = 0.;
        let row_sizes = self.row.sizes();
        for (index, state) in self.row.sash_states().into_iter().enumerate() {
            x += row_sizes[index];
            sashes.push((Sash::Row(index), state, [x, 0., CARD_MARGIN, self.height]));
        }
        let column_x = row_sizes[..ROW_EDITOR_COLUMN].iter().sum::<f32>();
        let editor_height = self.column.sizes()[COLUMN_EDITOR];
        let state = self.column.sash_states()[0];
        sashes.push((Sash::Column, state, [column_x + CARD_MARGIN / 2., editor_height, row_sizes[ROW_EDITOR_COLUMN] - CARD_MARGIN / 2., CARD_MARGIN]));
        sashes
    }

    pub fn start_sash_drag(&mut self, sash: Sash, position: f32, alt: bool) {
        match sash {
            Sash::Row(index) => self.row.start_sash_drag(index, position, alt),
            Sash::Column => self.column.start_sash_drag(0, position, alt),
        }
    }

    pub fn drag_sash(&mut self, position: f32, alt: bool) {
        if self.row.dragged_sash().is_some() {
            self.row.drag_sash(position, alt);
        } else {
            self.column.drag_sash(position, alt);
            self.update_editor_column_limits();
        }
    }

    pub fn end_sash_drag(&mut self) {
        self.row.end_sash_drag();
        self.column.end_sash_drag();
    }

    pub fn dragged_sash(&self) -> Option<Sash> {
        self.row.dragged_sash().map(Sash::Row).or(self.column.dragged_sash().map(|_| Sash::Column))
    }

    /// gridview.ts BranchNode: the editor column is as wide as its widest visible child needs; hidden with both.
    fn update_editor_column_limits(&mut self) {
        let panel_minimum = if self.column.is_view_visible(COLUMN_PANEL) { PANEL_MINIMUM_WIDTH } else { 0. };
        self.row.set_limits(ROW_EDITOR_COLUMN, EDITOR_MINIMUM_WIDTH.max(panel_minimum), f32::INFINITY);
    }
}

fn row_index(part: Part) -> usize {
    match part {
        Part::ActivityBar => ROW_ACTIVITY_BAR,
        Part::SideBar => ROW_SIDE_BAR,
        Part::AuxiliaryBar => ROW_AUXILIARY_BAR,
        Part::Panel => unreachable!("the panel is in the editor column"),
    }
}


#[cfg(test)]
mod tests;
