//! The workbench grid of browser/layout.ts for the floating (modern UI) layout: `createGridDescriptor` and
//! `arrangeMiddleSectionNodes` build a tree of split views, orthogonal at each depth, with the parts at the leaves.
//! The side bar sits left or right, the panel at any edge of the editor with its alignment; moving a part builds the
//! tree again from the parts' sizes as `adjustPartPositions` and `setPanelPosition` move views.
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
    Editor,
    Panel,
    AuxiliaryBar,
}

/// layoutService.ts `Position`, stored by its number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Left = 0,
    Right = 1,
    Bottom = 2,
    Top = 3,
}

impl Position {
    pub fn is_horizontal(self) -> bool {
        matches!(self, Position::Bottom | Position::Top)
    }

    /// `positionFromString`.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "left" => Position::Left,
            "right" => Position::Right,
            "bottom" => Position::Bottom,
            "top" => Position::Top,
            _ => return None,
        })
    }

    pub fn from_number(number: i64) -> Option<Self> {
        [Position::Left, Position::Right, Position::Bottom, Position::Top].get(usize::try_from(number).ok()?).copied()
    }

    /// `positionToString`.
    pub fn name(self) -> &'static str {
        match self {
            Position::Left => "left",
            Position::Right => "right",
            Position::Bottom => "bottom",
            Position::Top => "top",
        }
    }
}

/// layoutService.ts `PanelAlignment`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alignment {
    Left,
    Right,
    Center,
    Justify,
}

impl Alignment {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "left" => Alignment::Left,
            "right" => Alignment::Right,
            "center" => Alignment::Center,
            "justify" => Alignment::Justify,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Alignment::Left => "left",
            Alignment::Right => "right",
            Alignment::Center => "center",
            Alignment::Justify => "justify",
        }
    }
}

/// Where the movable parts sit: the side bar's side, the panel's edge and, for a top or bottom panel, its alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrangement {
    pub side_bar: Position,
    pub panel: Position,
    pub alignment: Alignment,
}

impl Default for Arrangement {
    fn default() -> Self {
        Arrangement { side_bar: Position::Left, panel: Position::Bottom, alignment: Alignment::Center }
    }
}

impl Arrangement {
    /// layoutService.ts `getFloatingSidebarSiblingToEditorStatus`: whether the side bar and the secondary side bar
    /// share the editor's row above or below a top or bottom panel, rather than spanning the full height.
    pub fn next_to_editor(self) -> (bool, bool) {
        let left = self.side_bar == Position::Left;
        let side_bar = !(self.alignment == Alignment::Center || (left && self.alignment == Alignment::Right) || (!left && self.alignment == Alignment::Left));
        let auxiliary_bar = !(self.alignment == Alignment::Center || (!left && self.alignment == Alignment::Right) || (left && self.alignment == Alignment::Left));
        (side_bar, auxiliary_bar)
    }
}

/// A sash of the grid: the branch it belongs to, in depth-first order, and the view it follows there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sash {
    pub branch: usize,
    pub index: usize,
    /// Between two views side by side: it moves along x.
    pub vertical: bool,
}

/// `LayoutStateModel`'s part sizes (`InitializationStateKey`s). The panel's is its height at the top or bottom and its
/// width on the left or right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartSizes {
    pub side_bar: f32,
    pub panel: f32,
    pub auxiliary_bar: f32,
}

impl PartSizes {
    /// `LayoutStateModel.load`: the side bars take a quarter of the window up to 300, the panel a third of its height
    /// at the top or bottom and a quarter of its width on a side.
    pub fn defaults(window_width: f32, window_height: f32, panel: Position) -> Self {
        let panel = if panel.is_horizontal() { window_height / 3. } else { window_width / 4. };
        PartSizes { side_bar: (window_width / 4.).min(300.), panel, auxiliary_bar: (window_width / 4.).min(300.) }
    }
}

enum Node {
    Leaf(Part),
    Branch(Branch),
}

struct Branch {
    split: SplitView,
    children: Vec<Node>,
}

pub struct Grid {
    root: Branch,
    arrangement: Arrangement,
    width: f32,
    height: f32,
    dragged: Option<usize>,
}

impl Grid {
    /// `createGridDescriptor` for a middle section of `width` by `height`; `visible` says which parts show.
    pub fn new(width: f32, height: f32, sizes: PartSizes, arrangement: Arrangement, visible: impl Fn(Part) -> bool) -> Self {
        let leaf = |part: Part, size: f32, horizontal: bool| {
            let (minimum, maximum) = part_limits(part, horizontal);
            let view = View::new(size, minimum, maximum).visible(visible(part));
            let view = match part {
                Part::SideBar | Part::AuxiliaryBar => view.snap().priority(Priority::Low),
                Part::Panel => view.snap(),
                // editorPart.ts: the editor snaps only with the panel centered, at high priority.
                Part::Editor if arrangement.alignment == Alignment::Center => view.snap().priority(Priority::High),
                Part::Editor => view.priority(Priority::High),
                Part::ActivityBar => view,
            };
            (Node::Leaf(part), view)
        };
        let shown = |part: Part, size: f32| if visible(part) { size } else { 0. };
        let left = arrangement.side_bar == Position::Left;
        let mut row = Vec::new();
        if !arrangement.panel.is_horizontal() {
            let editor_width = width - shown(Part::ActivityBar, ACTIVITY_BAR_WIDTH) - shown(Part::SideBar, sizes.side_bar) - shown(Part::Panel, sizes.panel) - shown(Part::AuxiliaryBar, sizes.auxiliary_bar);
            row.push(leaf(Part::Editor, editor_width, true));
            let panel = leaf(Part::Panel, sizes.panel, true);
            if arrangement.panel == Position::Right { row.push(panel) } else { row.insert(0, panel) }
            let (outer, inner) = (leaf(Part::ActivityBar, ACTIVITY_BAR_WIDTH, true), leaf(Part::SideBar, sizes.side_bar, true));
            let auxiliary_bar = leaf(Part::AuxiliaryBar, sizes.auxiliary_bar, true);
            if left {
                row.push(auxiliary_bar);
                row.insert(0, inner);
                row.insert(0, outer);
            } else {
                row.insert(0, auxiliary_bar);
                row.push(inner);
                row.push(outer);
            }
        } else {
            let (side_bar_next, auxiliary_bar_next) = arrangement.next_to_editor();
            let section_width = width - shown(Part::ActivityBar, ACTIVITY_BAR_WIDTH) - if side_bar_next { 0. } else { shown(Part::SideBar, sizes.side_bar) } - if auxiliary_bar_next { 0. } else { shown(Part::AuxiliaryBar, sizes.auxiliary_bar) };
            let editor_height = height - shown(Part::Panel, sizes.panel);
            // `arrangeEditorNodes`: the editor alone, or a row with the side bars that sit next to it.
            let editor = if !side_bar_next && !auxiliary_bar_next {
                leaf(Part::Editor, editor_height, false)
            } else {
                let editor_width = section_width - if side_bar_next { shown(Part::SideBar, sizes.side_bar) } else { 0. } - if auxiliary_bar_next { shown(Part::AuxiliaryBar, sizes.auxiliary_bar) } else { 0. };
                let mut nodes = vec![leaf(Part::Editor, editor_width, true)];
                if side_bar_next {
                    let side_bar = leaf(Part::SideBar, sizes.side_bar, true);
                    if left { nodes.insert(0, side_bar) } else { nodes.push(side_bar) }
                }
                if auxiliary_bar_next {
                    let auxiliary_bar = leaf(Part::AuxiliaryBar, sizes.auxiliary_bar, true);
                    if left { nodes.push(auxiliary_bar) } else { nodes.insert(0, auxiliary_bar) }
                }
                branch(editor_height, section_width, nodes)
            };
            let panel = leaf(Part::Panel, sizes.panel, false);
            let column = if arrangement.panel == Position::Bottom { vec![editor, panel] } else { vec![panel, editor] };
            row.push(branch(section_width, height, column));
            if !side_bar_next {
                let side_bar = leaf(Part::SideBar, sizes.side_bar, true);
                if left { row.insert(0, side_bar) } else { row.push(side_bar) }
            }
            if !auxiliary_bar_next {
                let auxiliary_bar = leaf(Part::AuxiliaryBar, sizes.auxiliary_bar, true);
                if left { row.push(auxiliary_bar) } else { row.insert(0, auxiliary_bar) }
            }
            let activity_bar = leaf(Part::ActivityBar, ACTIVITY_BAR_WIDTH, true);
            if left { row.insert(0, activity_bar) } else { row.push(activity_bar) }
        }
        let (children, views) = row.into_iter().unzip();
        let root = Branch { split: SplitView::with_priorities(width, views), children };
        let mut grid = Grid { root, arrangement, width, height, dragged: None };
        grid.refresh();
        grid
    }

    pub fn dimensions(&self) -> [f32; 2] {
        [self.width, self.height]
    }

    pub fn arrangement(&self) -> Arrangement {
        self.arrangement
    }

    pub fn layout(&mut self, width: f32, height: f32) {
        if (width, height) == (self.width, self.height) {
            return;
        }
        (self.width, self.height) = (width, height);
        self.refresh();
    }

    pub fn is_visible(&self, part: Part) -> bool {
        let (branch, index) = self.root.find(part).expect("every part is in the grid");
        branch.split.is_view_visible(index)
    }

    /// `setViewVisible` on the part's view; a branch shows while one of its children does.
    pub fn set_visible(&mut self, part: Part, visible: bool) {
        if self.is_visible(part) == visible {
            return;
        }
        let (branch, index) = self.root.find_mut(part).expect("every part is in the grid");
        branch.split.set_view_visible(index, visible);
        self.refresh();
    }

    /// The part's size across its sash, or the size it comes back at while hidden (`getViewCachedVisibleSize`).
    pub fn size(&self, part: Part) -> f32 {
        let (branch, index) = self.root.find(part).expect("every part is in the grid");
        branch.split.visible_size(index)
    }

    /// `resizeView` along the part's sash.
    pub fn resize(&mut self, part: Part, size: f32) {
        let (branch, index) = self.root.find_mut(part).expect("every part is in the grid");
        branch.split.resize_view(index, size);
        self.refresh();
    }

    /// The visible parts with their bounds (x, y, width, height) within the middle section.
    pub fn bounds(&self) -> Vec<(Part, [f32; 4])> {
        let mut bounds = Vec::new();
        self.root.bounds(true, [0., 0., self.width, self.height], &mut bounds);
        bounds
    }

    pub fn part_bounds(&self, part: Part) -> Option<[f32; 4]> {
        self.bounds().into_iter().find(|(shown, _)| *shown == part).map(|(_, bounds)| bounds)
    }

    /// The sashes with their states and bounds within the middle section. In the floating layout a sash fills the
    /// 4px gap after its boundary (`workbench-sash-start-at-gap`); a horizontal one starts half a gap in.
    pub fn sashes(&self) -> Vec<(Sash, SashState, [f32; 4])> {
        let mut sashes = Vec::new();
        self.root.sashes(true, [0., 0., self.width, self.height], &mut 0, &mut sashes);
        sashes
    }

    pub fn start_sash_drag(&mut self, sash: Sash, position: f32, alt: bool) {
        self.dragged = Some(sash.branch);
        self.root.nth_branch(sash.branch, &mut 0).expect("the sash's branch").split.start_sash_drag(sash.index, position, alt);
    }

    pub fn drag_sash(&mut self, position: f32, alt: bool) {
        let Some(id) = self.dragged else { return };
        let branch = self.root.nth_branch(id, &mut 0).expect("the dragged branch");
        branch.split.drag_sash(position, alt);
        // The dragged split keeps its size; its children and the limits a snapped view changes follow.
        branch.layout_children();
        self.root.update_limits(true);
    }

    pub fn end_sash_drag(&mut self) {
        if let Some(id) = self.dragged.take() {
            self.root.nth_branch(id, &mut 0).expect("the dragged branch").split.end_sash_drag();
            self.refresh();
        }
    }

    pub fn dragged_sash(&self) -> Option<Sash> {
        let id = self.dragged?;
        let mut counter = 0;
        let branch = self.root.nth_branch_ref(id, &mut counter)?;
        let vertical = self.root.is_horizontal_branch(id, true, &mut 0)?;
        branch.split.dragged_sash().map(|index| Sash { branch: id, index, vertical })
    }

    fn refresh(&mut self) {
        self.root.update_limits(true);
        let rail = self.activity_bar_width();
        let (branch, index) = self.root.find_mut(Part::ActivityBar).expect("every part is in the grid");
        branch.split.set_limits(index, rail, rail);
        if branch.split.is_view_visible(index) && branch.split.sizes()[index] != rail {
            branch.split.resize_view(index, rail);
        }
        self.root.layout(self.width, self.height);
    }

    /// activitybarPart.ts `floatingHorizontalGutter`: a right rail with no side bar before it adds the gap it needs.
    fn activity_bar_width(&self) -> f32 {
        let leading_gap = self.arrangement.side_bar == Position::Right && !self.is_visible(Part::SideBar);
        ACTIVITY_BAR_WIDTH + if leading_gap { CARD_MARGIN } else { 0. }
    }
}

fn branch(size: f32, orthogonal: f32, nodes: Vec<(Node, View)>) -> (Node, View) {
    let (children, views): (Vec<Node>, Vec<View>) = nodes.into_iter().unzip();
    let visible = views.iter().any(|view| view.visible);
    let priority = if views.iter().any(|view| view.priority == Priority::High) {
        Priority::High
    } else if views.iter().any(|view| view.priority == Priority::Low) {
        Priority::Low
    } else {
        Priority::Normal
    };
    let view = View::new(size, 0., f32::INFINITY).priority(priority).visible(visible);
    (Node::Branch(Branch { split: SplitView::with_priorities(orthogonal, views), children }), view)
}

/// A part's minimum and maximum width (`horizontal`) or height.
fn part_limits(part: Part, horizontal: bool) -> (f32, f32) {
    match (part, horizontal) {
        (Part::ActivityBar, true) => (ACTIVITY_BAR_WIDTH, ACTIVITY_BAR_WIDTH),
        (Part::SideBar | Part::AuxiliaryBar, true) => (SIDE_BAR_MINIMUM_WIDTH, f32::INFINITY),
        (Part::Panel, true) => (PANEL_MINIMUM_WIDTH, f32::INFINITY),
        (Part::Panel, false) => (PANEL_MINIMUM_HEIGHT, f32::INFINITY),
        (Part::Editor, true) => (EDITOR_MINIMUM_WIDTH, f32::INFINITY),
        (Part::Editor, false) => (EDITOR_MINIMUM_HEIGHT, f32::INFINITY),
        (_, false) => (0., f32::INFINITY),
    }
}

impl Branch {
    fn find(&self, part: Part) -> Option<(&Branch, usize)> {
        for (index, child) in self.children.iter().enumerate() {
            match child {
                Node::Leaf(leaf) if *leaf == part => return Some((self, index)),
                Node::Leaf(_) => {}
                Node::Branch(branch) => {
                    if let Some(found) = branch.find(part) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }

    fn find_mut(&mut self, part: Part) -> Option<(&mut Branch, usize)> {
        let index = self.children.iter().position(|child| matches!(child, Node::Leaf(leaf) if *leaf == part));
        if let Some(index) = index {
            return Some((self, index));
        }
        self.children.iter_mut().find_map(|child| match child {
            Node::Branch(branch) => branch.find_mut(part),
            Node::Leaf(_) => None,
        })
    }

    /// The branch `id` in depth-first order, this one first.
    fn nth_branch(&mut self, id: usize, counter: &mut usize) -> Option<&mut Branch> {
        if *counter == id {
            return Some(self);
        }
        *counter += 1;
        self.children.iter_mut().find_map(|child| match child {
            Node::Branch(branch) => branch.nth_branch(id, counter),
            Node::Leaf(_) => None,
        })
    }

    fn nth_branch_ref(&self, id: usize, counter: &mut usize) -> Option<&Branch> {
        if *counter == id {
            return Some(self);
        }
        *counter += 1;
        self.children.iter().find_map(|child| match child {
            Node::Branch(branch) => branch.nth_branch_ref(id, counter),
            Node::Leaf(_) => None,
        })
    }

    fn is_horizontal_branch(&self, id: usize, horizontal: bool, counter: &mut usize) -> Option<bool> {
        if *counter == id {
            return Some(horizontal);
        }
        *counter += 1;
        self.children.iter().find_map(|child| match child {
            Node::Branch(branch) => branch.is_horizontal_branch(id, !horizontal, counter),
            Node::Leaf(_) => None,
        })
    }

    /// gridview.ts BranchNode limits: a child branch is as wide as its widest visible child needs across this split
    /// and hidden when none of its children shows. Returns this branch's minimum and maximum across its parent.
    fn update_limits(&mut self, horizontal: bool) -> (f32, f32) {
        for (index, child) in self.children.iter_mut().enumerate() {
            if let Node::Branch(branch) = child {
                let (minimum, maximum) = branch.update_limits(!horizontal);
                self.split.set_limits(index, minimum, maximum);
                let visible = (0..branch.children.len()).any(|child| branch.split.is_view_visible(child));
                if self.split.is_view_visible(index) != visible {
                    self.split.set_view_visible(index, visible);
                }
            }
        }
        // Across the parent's direction: the largest minimum and smallest maximum of the visible children.
        let mut minimum: f32 = 0.;
        let mut maximum = f32::INFINITY;
        for (index, child) in self.children.iter().enumerate() {
            if !self.split.is_view_visible(index) {
                continue;
            }
            let (child_minimum, child_maximum) = match child {
                Node::Leaf(part) => part_limits(*part, !horizontal),
                Node::Branch(branch) => branch.limits_along(!horizontal),
            };
            minimum = minimum.max(child_minimum);
            maximum = maximum.min(child_maximum);
        }
        (minimum, maximum.max(minimum))
    }

    /// A branch's limits along its own direction (`horizontal` is that direction): the sums of its visible children.
    fn limits_along(&self, horizontal: bool) -> (f32, f32) {
        let mut minimum = 0.;
        let mut maximum = 0.;
        for (index, child) in self.children.iter().enumerate() {
            if !self.split.is_view_visible(index) {
                continue;
            }
            let (child_minimum, child_maximum) = match child {
                Node::Leaf(part) => part_limits(*part, horizontal),
                Node::Branch(branch) => branch.limits_across(horizontal),
            };
            minimum += child_minimum;
            maximum += child_maximum;
        }
        (minimum, maximum)
    }

    /// A branch's limits across its own direction (`horizontal` is the other one).
    fn limits_across(&self, horizontal: bool) -> (f32, f32) {
        let mut minimum: f32 = 0.;
        let mut maximum = f32::INFINITY;
        for (index, child) in self.children.iter().enumerate() {
            if !self.split.is_view_visible(index) {
                continue;
            }
            let (child_minimum, child_maximum) = match child {
                Node::Leaf(part) => part_limits(*part, horizontal),
                Node::Branch(branch) => branch.limits_along(horizontal),
            };
            minimum = minimum.max(child_minimum);
            maximum = maximum.min(child_maximum);
        }
        (minimum, maximum.max(minimum))
    }

    /// Lays this branch out over `size` along its direction and its children across `orthogonal`.
    fn layout(&mut self, size: f32, orthogonal: f32) {
        self.split.layout(size);
        let sizes = self.split.sizes();
        for (child, child_size) in self.children.iter_mut().zip(sizes) {
            if let Node::Branch(branch) = child {
                branch.layout(orthogonal, child_size);
            }
        }
    }

    /// Lays the children out again after a sash in this branch moved.
    fn layout_children(&mut self) {
        let sizes = self.split.sizes();
        for (child, child_size) in self.children.iter_mut().zip(sizes) {
            if let Node::Branch(branch) = child {
                let size = branch.split.size();
                branch.layout(size, child_size);
            }
        }
    }

    fn bounds(&self, horizontal: bool, [x, y, width, height]: [f32; 4], out: &mut Vec<(Part, [f32; 4])>) {
        let mut offset = 0.;
        for (index, (child, size)) in self.children.iter().zip(self.split.sizes()).enumerate() {
            let rect = if horizontal { [x + offset, y, size, height] } else { [x, y + offset, width, size] };
            offset += size;
            if !self.split.is_view_visible(index) {
                continue;
            }
            match child {
                Node::Leaf(part) => out.push((*part, rect)),
                Node::Branch(branch) => branch.bounds(!horizontal, rect, out),
            }
        }
    }

    fn sashes(&self, horizontal: bool, [x, y, width, height]: [f32; 4], counter: &mut usize, out: &mut Vec<(Sash, SashState, [f32; 4])>) {
        let id = *counter;
        *counter += 1;
        let sizes = self.split.sizes();
        let mut offset = 0.;
        for (index, state) in self.split.sash_states().into_iter().enumerate() {
            offset += sizes[index];
            let rect = if horizontal { [x + offset, y, CARD_MARGIN, height] } else { [x + CARD_MARGIN / 2., y + offset, width - CARD_MARGIN / 2., CARD_MARGIN] };
            out.push((Sash { branch: id, index, vertical: horizontal }, state, rect));
        }
        let mut offset = 0.;
        for (child, size) in self.children.iter().zip(sizes) {
            let rect = if horizontal { [x + offset, y, size, height] } else { [x, y + offset, width, size] };
            offset += size;
            if let Node::Branch(branch) = child {
                branch.sashes(!horizontal, rect, counter, out);
            }
        }
    }
}

/// The floating card margins of a part at `rect` (left, top, right, bottom), floatingPanels.css and layoutService.ts
/// `getFloatingPaneCompositeHorizontalMargins`, `getFloatingPaneCompositeVerticalMargins` and
/// `getFloatingEditorVerticalMargins` for the default density: each card keeps the gap on its leading edge, the
/// outermost cards the outer gutter on the window's sides and bottom. The side bar meets the activity bar flush.
/// `top_exposed` is `isFloatingTopEdgeExposed`: no title bar and no banner above the grid.
pub fn card_margins(part: Part, rect: [f32; 4], parts: &[(Part, [f32; 4])], arrangement: Arrangement, width: f32, height: f32, top_exposed: bool) -> [f32; 4] {
    let [x, y, w, h] = rect;
    let activity_bar = parts.iter().find(|(shown, _)| *shown == Part::ActivityBar).map(|(_, rect)| rect);
    let side_bar_visible = parts.iter().any(|(shown, _)| *shown == Part::SideBar);
    let left_rail = arrangement.side_bar == Position::Left;
    let right_edge = x + w >= width - 0.5;
    let left = match part {
        _ if x <= 0.5 => CARD_MARGIN,
        Part::SideBar if left_rail && activity_bar.is_some() => 0.,
        // activitybarPart.ts `needsFloatingLeadingGap`: a right rail after the side bar meets it flush.
        Part::ActivityBar if side_bar_visible => 0.,
        _ => CARD_MARGIN,
    };
    // A top or bottom panel reaching under the side bar to a right rail keeps a gap before it.
    let meets_right_rail = !left_rail && part != Part::SideBar && side_bar_visible && activity_bar.is_some_and(|rail| (rail[0] - (x + w)).abs() < 0.5);
    let right = if right_edge || meets_right_rail { CARD_MARGIN } else { 0. };
    let top = if y > 0.5 || top_exposed { CARD_MARGIN } else { 0. };
    let bottom = if y + h >= height - 0.5 { CARD_MARGIN } else { 0. };
    [left, top, right, bottom]
}

#[cfg(test)]
mod tests;
