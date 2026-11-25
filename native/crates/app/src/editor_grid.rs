//! The editor part's grid of editor groups: the parts of base/browser/ui/grid/gridview.ts and grid.ts that
//! workbench/browser/parts/editor/editorPart.ts uses. A tree of split views, orthogonal at each depth, with
//! editor groups at the leaves.

use crate::split_view::{SashState, SplitView, View};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// Horizontal lays children side by side, as gridview's `Orientation.HORIZONTAL`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

/// `workbench.editor.splitSizing`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitSizing {
    Auto,
    Distribute,
    Split,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// The sash after child `index` of the branch at `path` (child indexes from the root).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SashId {
    pub path: Vec<usize>,
    pub index: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GridSash {
    pub id: SashId,
    pub orientation: Orientation,
    pub rect: Rect,
    pub state: SashState,
}

/// editorGroupsService `GroupLayoutArgument`: a leaf when `groups` is empty, `size` a proportion.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct GroupLayout {
    pub size: Option<f32>,
    pub groups: Vec<GroupLayout>,
}

/// gridview.ts `ISerializedNode`: `size` is the node's size along its parent's orientation.
#[derive(Clone, Debug, PartialEq)]
pub enum SerializedNode {
    Leaf { group: u32, size: f32 },
    Branch { children: Vec<SerializedNode>, size: f32 },
}

/// editor.ts `DEFAULT_EDITOR_MIN_DIMENSIONS`.
const MIN_WIDTH: f32 = 220.;
const MIN_HEIGHT: f32 = 70.;

enum Node {
    Leaf(u32),
    Branch(Branch),
}

struct Branch {
    split: SplitView,
    children: Vec<Node>,
}

/// splitview.ts sizing once `Auto` is resolved; `Last` is no sizing: the last view takes the change.
#[derive(Clone, Copy)]
enum Sizing {
    Distribute,
    Split(usize),
    Last,
}

pub struct EditorGrid {
    root: Branch,
    orientation: Orientation,
    width: f32,
    height: f32,
    maximized: Option<u32>,
    drag: Option<Vec<usize>>,
}

impl EditorGrid {
    /// gridview.ts starts with a vertical root.
    pub fn new(group: u32, width: f32, height: f32) -> Self {
        let mut grid = EditorGrid { root: Branch { split: SplitView::new(0., vec![]), children: vec![] }, orientation: Orientation::Vertical, width, height, maximized: None, drag: None };
        let (along, _) = grid.root_extents();
        let children = vec![Node::Leaf(group)];
        grid.root = Branch { split: new_split(&children, grid.orientation, along, vec![along]), children };
        grid
    }

    pub fn orientation(&self) -> Orientation {
        self.orientation
    }

    pub fn layout(&mut self, width: f32, height: f32) {
        self.width = width;
        self.height = height;
        let (along, across) = self.root_extents();
        layout_branch(&mut self.root, self.orientation, along, across);
    }

    /// grid.ts `addView` with a sizing: a split along the reference's parent inserts a sibling, a split across it
    /// replaces the reference with a branch of the two. A one-group grid first turns to the split's orientation.
    pub fn add_group(&mut self, reference: u32, direction: Direction, group: u32, sizing: SplitSizing) {
        self.restore();
        let direction_orientation = direction_orientation(direction);
        if self.root.children.len() == 1 && matches!(self.root.children[0], Node::Leaf(_)) && self.orientation != direction_orientation {
            self.set_orientation(direction_orientation);
        }
        let after = matches!(direction, Direction::Right | Direction::Down) as usize;
        let mut path = self.find(reference);
        let index = path.pop().expect("a group sits in a branch");
        let parent_orientation = self.orientation_at(path.len());
        let across = self.across(&path);
        let parent = self.branch_mut(&path);
        if parent_orientation == direction_orientation {
            let mut sizes = parent.split.sizes();
            let sizing = resolve(sizing, &sizes, index);
            parent.children.insert(index + after, Node::Leaf(group));
            insert_size(&mut sizes, &mins(&parent.children, parent_orientation), parent.split.size(), index + after, sizing);
            set_split(parent, parent_orientation, sizes);
        } else {
            // gridview addView turns `Split` into `Split(0)`; `Auto` over one view distributes.
            let sizing = if sizing == SplitSizing::Split { Sizing::Split(0) } else { Sizing::Distribute };
            let mut children = vec![Node::Leaf(reference)];
            children.insert(after, Node::Leaf(group));
            let orientation = orthogonal(parent_orientation);
            let mut sizes = vec![across];
            insert_size(&mut sizes, &mins(&children, orientation), across, after, sizing);
            let split = new_split(&children, orientation, across, sizes);
            parent.children[index] = Node::Branch(Branch { split, children });
        }
        self.relayout();
    }

    /// grid.ts `removeView`: `Split` passes no sizing (the last view takes the room), `Auto` splits into the view
    /// before. A branch left with one child folds into its parent, sizes kept (gridview.ts `removeView`).
    pub fn remove_group(&mut self, group: u32, sizing: SplitSizing) {
        self.restore();
        assert!(self.groups().len() > 1, "can't remove the last group");
        let mut path = self.find(group);
        let index = path.pop().expect("a group sits in a branch");
        let orientation = self.orientation_at(path.len());
        let parent = self.branch_mut(&path);
        let mut sizes = parent.split.sizes();
        let sizing = match sizing {
            SplitSizing::Distribute => Sizing::Distribute,
            SplitSizing::Auto => resolve(SplitSizing::Auto, &sizes, if index == 0 { 1 } else { index - 1 }),
            SplitSizing::Split => Sizing::Last,
        };
        parent.children.remove(index);
        remove_size(&mut sizes, &mins(&parent.children, orientation), parent.split.size(), index, sizing);
        set_split(parent, orientation, sizes);
        if parent.children.len() == 1 {
            self.fold(path);
        }
        self.relayout();
    }

    /// GRID_APPEARANCE order.
    pub fn groups(&self) -> Vec<u32> {
        let mut groups = vec![];
        collect_groups(&self.root, &mut groups);
        groups
    }

    /// The visible groups' bounds from the grid's top-left.
    pub fn bounds(&self) -> Vec<(u32, Rect)> {
        let mut bounds = vec![];
        collect_bounds(&self.root, self.orientation, self.rect(), &mut bounds);
        bounds
    }

    /// grid.ts `getNeighborViews`: the groups touching the group's edge in `direction`; `wrap` looks at the
    /// opposite edge of the grid when the group sits on the grid's edge.
    pub fn neighbors(&self, group: u32, direction: Direction, wrap: bool) -> Vec<u32> {
        let bounds = self.bounds();
        let Some(&(_, source)) = bounds.iter().find(|(id, _)| *id == group) else { return vec![] };
        let (mut offset, range) = boundary(source, direction);
        if wrap {
            let near = |a: f32, b: f32| (a - b).abs() < 0.5;
            match direction {
                Direction::Up if near(source.y, 0.) => offset = self.height,
                Direction::Right if near(source.x + source.width, self.width) => offset = 0.,
                Direction::Down if near(source.y + source.height, self.height) => offset = 0.,
                Direction::Left if near(source.x, 0.) => offset = self.width,
                _ => {}
            }
        }
        let mut neighbors = vec![];
        for (id, rect) in bounds {
            let (other_offset, other_range) = boundary(rect, opposite(direction));
            if (other_offset - offset).abs() < 0.5 && !(other_range.0 >= range.1 || range.0 >= other_range.1) {
                neighbors.push(id);
            }
        }
        neighbors
    }

    /// The neighbor whose edge starts nearest the group's (editorPart `findGroup` by direction, without MRU).
    pub fn neighbor(&self, group: u32, direction: Direction) -> Option<u32> {
        let bounds = self.bounds();
        let rect_of = |id: u32| bounds.iter().find(|(other, _)| *other == id).map(|(_, rect)| *rect).expect("a neighbor is visible");
        let start = |id: u32| boundary(rect_of(id), direction).1.0;
        let source = start(group);
        self.neighbors(group, direction, false).into_iter().min_by(|a, b| (start(*a) - source).abs().total_cmp(&(start(*b) - source).abs()))
    }

    pub fn sashes(&self, sash_size: f32) -> Vec<GridSash> {
        let mut sashes = vec![];
        if self.maximized.is_none() {
            collect_sashes(&self.root, self.orientation, self.rect(), &mut vec![], sash_size, &mut sashes);
        }
        sashes
    }

    /// `start` and later `current` are positions along the sash's branch orientation (x for horizontal).
    pub fn start_sash_drag(&mut self, sash: &SashId, start: f32, alt: bool) {
        self.branch_mut(&sash.path).split.start_sash_drag(sash.index, start, alt);
        self.drag = Some(sash.path.clone());
    }

    pub fn drag_sash(&mut self, current: f32, alt: bool) {
        let Some(path) = self.drag.clone() else { return };
        self.branch_mut(&path).split.drag_sash(current, alt);
        self.relayout();
    }

    /// The sash being dragged and its branch's orientation.
    pub fn dragged_sash(&self) -> Option<(SashId, Orientation)> {
        let path = self.drag.as_ref()?;
        let index = self.branch(path).split.dragged_sash()?;
        Some((SashId { path: path.clone(), index }, self.orientation_at(path.len())))
    }

    pub fn end_sash_drag(&mut self) {
        if let Some(path) = self.drag.take() {
            self.branch_mut(&path).split.end_sash_drag();
        }
    }

    /// grid.ts `onDidSashReset` without preferred sizes: the sash's branch distributes its children.
    pub fn reset_sash(&mut self, sash: &SashId) {
        self.restore();
        let orientation = self.orientation_at(sash.path.len());
        distribute_branch(self.branch_mut(&sash.path), orientation, false);
        self.relayout();
    }

    /// gridview.ts `orientation` setter: every branch turns, each child keeping its share of its branch.
    pub fn set_orientation(&mut self, orientation: Orientation) {
        if orientation == self.orientation {
            return;
        }
        self.restore();
        let (root, _, width, height) = self.serialize();
        *self = EditorGrid::deserialize(root, orientation, width, height);
    }

    /// arrangeGroups EVEN.
    pub fn distribute(&mut self) {
        self.restore();
        distribute_branch(&mut self.root, self.orientation, true);
        self.relayout();
    }

    /// gridview.ts `maximizeView`: every view off the group's path hides, in order, so `restore` can undo it in
    /// reverse order and get the cached sizes back.
    pub fn maximize(&mut self, group: u32) {
        if self.maximized == Some(group) {
            return;
        }
        self.restore();
        let path = self.find(group);
        for depth in 0..path.len() {
            let branch = self.branch_mut(&path[..depth]);
            for index in (0..branch.children.len()).filter(|&index| index != path[depth]) {
                branch.split.set_view_visible(index, false);
            }
        }
        self.maximized = Some(group);
        self.relayout();
    }

    /// gridview.ts `exitMaximizedView`.
    pub fn restore(&mut self) {
        let Some(group) = self.maximized.take() else { return };
        let path = self.find(group);
        for depth in (0..path.len()).rev() {
            let branch = self.branch_mut(&path[..depth]);
            for index in (0..branch.children.len()).rev().filter(|&index| index != path[depth]) {
                branch.split.set_view_visible(index, true);
            }
        }
        self.relayout();
    }

    pub fn is_maximized(&self) -> bool {
        self.maximized.is_some()
    }

    pub fn maximized(&self) -> Option<u32> {
        self.maximized
    }

    /// arrangeGroups EXPAND (gridview.ts `expandView`): each ancestor gives the group's side all it can.
    pub fn expand(&mut self, group: u32) {
        self.restore();
        let path = self.find(group);
        for depth in 0..path.len() {
            self.branch_mut(&path[..depth]).split.resize_view(path[depth], f32::INFINITY);
        }
        self.relayout();
    }

    /// gridview.ts `isViewExpanded`: every other view on the group's path sits at its minimum.
    pub fn is_expanded(&self, group: u32) -> bool {
        if self.maximized.is_some() {
            return false;
        }
        let path = self.find(group);
        (0..path.len()).all(|depth| {
            let branch = self.branch(&path[..depth]);
            let sizes = branch.split.sizes();
            let mins = mins(&branch.children, self.orientation_at(depth));
            (0..sizes.len()).all(|index| index == path[depth] || sizes[index] <= mins[index])
        })
    }

    /// editorPart.ts `applyLayout`: a new tree from `groups`, reusing the current groups in GRID_APPEARANCE order
    /// and taking `ids()` for each extra one. Returns the current groups left without a place.
    pub fn apply_layout(&mut self, orientation: Orientation, groups: &[GroupLayout], ids: &mut dyn FnMut() -> u32) -> Vec<u32> {
        self.restore();
        let one_group = [GroupLayout::default()];
        let groups = if groups.is_empty() { &one_group[..] } else { groups };
        let mut existing = self.groups().into_iter();
        let children = layout_children(groups, &mut || existing.next().unwrap_or_else(&mut *ids));
        *self = EditorGrid::deserialize(SerializedNode::Branch { children, size: 1. }, orientation, self.width, self.height);
        existing.collect()
    }

    /// grid.ts `SerializableGrid.serialize`; a hidden view keeps the size it comes back at.
    pub fn serialize(&self) -> (SerializedNode, Orientation, f32, f32) {
        let (_, across) = self.root_extents();
        (serialize_branch(&self.root, across), self.orientation, self.width, self.height)
    }

    /// gridview.ts `deserialize`: each branch keeps its children's shares, laid out at `width` x `height`.
    pub fn deserialize(root: SerializedNode, orientation: Orientation, width: f32, height: f32) -> Self {
        let children = match root {
            SerializedNode::Branch { children, .. } => children,
            leaf => vec![leaf],
        };
        let root = build_branch(&children, orientation);
        let mut grid = EditorGrid { root, orientation, width, height, maximized: None, drag: None };
        grid.relayout();
        grid
    }

    fn relayout(&mut self) {
        self.layout(self.width, self.height);
    }

    fn rect(&self) -> Rect {
        Rect { x: 0., y: 0., width: self.width, height: self.height }
    }

    fn root_extents(&self) -> (f32, f32) {
        match self.orientation {
            Orientation::Horizontal => (self.width, self.height),
            Orientation::Vertical => (self.height, self.width),
        }
    }

    fn orientation_at(&self, depth: usize) -> Orientation {
        if depth % 2 == 0 { self.orientation } else { orthogonal(self.orientation) }
    }

    /// The branch's extent across its orientation: its size in its parent.
    fn across(&self, path: &[usize]) -> f32 {
        match path.split_last() {
            None => self.root_extents().1,
            Some((&index, parent)) => self.branch(parent).split.visible_size(index),
        }
    }

    fn find(&self, group: u32) -> Vec<usize> {
        let mut path = vec![];
        assert!(find_path(&self.root, group, &mut path), "group {group} is in the grid");
        path
    }

    fn branch(&self, path: &[usize]) -> &Branch {
        let mut branch = &self.root;
        for &index in path {
            let Node::Branch(child) = &branch.children[index] else { panic!("path {path:?} leads to a branch") };
            branch = child;
        }
        branch
    }

    fn branch_mut(&mut self, path: &[usize]) -> &mut Branch {
        let mut branch = &mut self.root;
        for &index in path {
            let Node::Branch(child) = &mut branch.children[index] else { panic!("path {path:?} leads to a branch") };
            branch = child;
        }
        branch
    }

    /// The branch at `path` has one child left: a root keeps a leaf and gives way to a branch; any other branch
    /// hands its child (or the child's children) to its parent at the sizes they had.
    fn fold(&mut self, mut path: Vec<usize>) {
        let Some(index) = path.pop() else {
            if matches!(self.root.children[0], Node::Branch(_)) {
                let Some(Node::Branch(child)) = self.root.children.pop() else { unreachable!() };
                self.root = child;
                self.orientation = orthogonal(self.orientation);
            }
            return;
        };
        let orientation = self.orientation_at(path.len());
        let grandparent = self.branch_mut(&path);
        let mut sizes = grandparent.split.sizes();
        let Node::Branch(mut parent) = grandparent.children.remove(index) else { unreachable!() };
        match parent.children.pop().expect("one child left") {
            Node::Branch(child) => {
                sizes.splice(index..=index, child.split.sizes());
                grandparent.children.splice(index..index, child.children);
            }
            leaf => grandparent.children.insert(index, leaf),
        }
        set_split(grandparent, orientation, sizes);
    }
}

fn orthogonal(orientation: Orientation) -> Orientation {
    match orientation {
        Orientation::Horizontal => Orientation::Vertical,
        Orientation::Vertical => Orientation::Horizontal,
    }
}

fn direction_orientation(direction: Direction) -> Orientation {
    match direction {
        Direction::Up | Direction::Down => Orientation::Vertical,
        Direction::Left | Direction::Right => Orientation::Horizontal,
    }
}

fn opposite(direction: Direction) -> Direction {
    match direction {
        Direction::Up => Direction::Down,
        Direction::Down => Direction::Up,
        Direction::Left => Direction::Right,
        Direction::Right => Direction::Left,
    }
}

/// grid.ts `getBoxBoundary`: the edge's offset and its range along the edge.
fn boundary(rect: Rect, direction: Direction) -> (f32, (f32, f32)) {
    let (right, bottom) = (rect.x + rect.width, rect.y + rect.height);
    match direction {
        Direction::Up => (rect.y, (rect.x, right)),
        Direction::Down => (bottom, (rect.x, right)),
        Direction::Left => (rect.x, (rect.y, bottom)),
        Direction::Right => (right, (rect.y, bottom)),
    }
}

/// gridview.ts `minimumSize`: a node's minimum along `axis` (Horizontal is width); a branch sums its visible
/// children along its orientation and takes their largest across it.
fn minimum(node: &Node, orientation: Orientation, axis: Orientation) -> f32 {
    let Node::Branch(branch) = node else {
        return if axis == Orientation::Horizontal { MIN_WIDTH } else { MIN_HEIGHT };
    };
    let mut sum = 0.;
    let mut max = 0f32;
    for (index, child) in branch.children.iter().enumerate() {
        if branch.split.is_view_visible(index) {
            let child_minimum = minimum(child, orthogonal(orientation), axis);
            sum += child_minimum;
            max = max.max(child_minimum);
        }
    }
    if axis == orientation { sum } else { max }
}

fn mins(children: &[Node], orientation: Orientation) -> Vec<f32> {
    children.iter().map(|child| minimum(child, orthogonal(orientation), orientation)).collect()
}

fn new_split(children: &[Node], orientation: Orientation, size: f32, sizes: Vec<f32>) -> SplitView {
    let views = sizes.into_iter().zip(mins(children, orientation)).map(|(size, minimum)| View::new(size, minimum, f32::INFINITY)).collect();
    SplitView::new(size, views)
}

fn set_split(branch: &mut Branch, orientation: Orientation, sizes: Vec<f32>) {
    branch.split = new_split(&branch.children, orientation, branch.split.size(), sizes);
}

/// gridview.ts `BranchNode.layout`: a split view lays out again only when its size changed, so a sash drag in
/// progress keeps its sizes.
fn layout_branch(branch: &mut Branch, orientation: Orientation, along: f32, across: f32) {
    for (index, minimum) in mins(&branch.children, orientation).into_iter().enumerate() {
        branch.split.set_limits(index, minimum, f32::INFINITY);
    }
    if branch.split.size() != along {
        branch.split.layout(along);
    }
    let sizes = branch.split.sizes();
    for (index, child) in branch.children.iter_mut().enumerate() {
        if let Node::Branch(child) = child
            && branch.split.is_view_visible(index)
        {
            layout_branch(child, orthogonal(orientation), across, sizes[index]);
        }
    }
}

fn distribute_branch(branch: &mut Branch, orientation: Orientation, recursive: bool) {
    let mut sizes = branch.split.sizes();
    distribute(&mut sizes, &mins(&branch.children, orientation), branch.split.size());
    set_split(branch, orientation, sizes);
    if recursive {
        for child in &mut branch.children {
            if let Node::Branch(child) = child {
                distribute_branch(child, orthogonal(orientation), true);
            }
        }
    }
}

/// splitview.ts `addView`: `Auto` distributes when the views are within 2px of each other, else splits.
fn resolve(sizing: SplitSizing, sizes: &[f32], reference: usize) -> Sizing {
    let max = sizes.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let min = sizes.iter().copied().fold(f32::INFINITY, f32::min);
    match sizing {
        SplitSizing::Distribute => Sizing::Distribute,
        SplitSizing::Split => Sizing::Split(reference),
        SplitSizing::Auto if max - min <= 2. => Sizing::Distribute,
        SplitSizing::Auto => Sizing::Split(reference),
    }
}

/// splitview.ts `addView`: `Split` halves the reference, which gives the room first and the new view last.
fn insert_size(sizes: &mut Vec<f32>, mins: &[f32], total: f32, index: usize, sizing: Sizing) {
    match sizing {
        Sizing::Split(reference) => {
            sizes.insert(index, sizes[reference] / 2.);
            let reference = if reference >= index { reference + 1 } else { reference };
            let others = (0..sizes.len()).rev().filter(|&other| other != reference && other != index);
            let order: Vec<usize> = [reference].into_iter().chain(others).chain([index]).collect();
            fit(sizes, mins, total, order);
        }
        Sizing::Distribute | Sizing::Last => {
            sizes.insert(index, mins[index]);
            distribute(sizes, mins, total);
        }
    }
}

/// splitview.ts `removeView`: `Split` gives the room to the reference, the rest goes to the last view.
fn remove_size(sizes: &mut Vec<f32>, mins: &[f32], total: f32, index: usize, sizing: Sizing) {
    if let Sizing::Split(reference) = sizing {
        sizes[reference] += sizes[index];
    }
    sizes.remove(index);
    let last_first = (0..sizes.len()).rev().collect();
    fit(sizes, mins, total, last_first);
    if let Sizing::Distribute = sizing {
        distribute(sizes, mins, total);
    }
}

/// splitview.ts `distributeViewSizes`: equal floored sizes, the last view takes the rounding.
fn distribute(sizes: &mut [f32], mins: &[f32], total: f32) {
    let each = (total / sizes.len() as f32).floor();
    for (size, minimum) in sizes.iter_mut().zip(mins) {
        *size = each.max(*minimum);
    }
    let last_first = (0..sizes.len()).rev().collect();
    fit(sizes, mins, total, last_first);
}

/// splitview.ts `resize`: the views in `order` take what the sizes miss of `total`, each down to its minimum.
fn fit(sizes: &mut [f32], mins: &[f32], total: f32, order: Vec<usize>) {
    let mut empty = total - sizes.iter().sum::<f32>();
    for index in order {
        let size = (sizes[index] + empty).max(mins[index]);
        empty -= size - sizes[index];
        sizes[index] = size;
    }
}

fn find_path(branch: &Branch, group: u32, path: &mut Vec<usize>) -> bool {
    for (index, child) in branch.children.iter().enumerate() {
        path.push(index);
        let found = match child {
            Node::Leaf(id) => *id == group,
            Node::Branch(child) => find_path(child, group, path),
        };
        if found {
            return true;
        }
        path.pop();
    }
    false
}

fn collect_groups(branch: &Branch, groups: &mut Vec<u32>) {
    for child in &branch.children {
        match child {
            Node::Leaf(group) => groups.push(*group),
            Node::Branch(child) => collect_groups(child, groups),
        }
    }
}

fn child_rects(branch: &Branch, orientation: Orientation, rect: Rect) -> Vec<Rect> {
    let mut offset = 0.;
    let mut rects = vec![];
    for size in branch.split.sizes() {
        rects.push(match orientation {
            Orientation::Horizontal => Rect { x: rect.x + offset, width: size, ..rect },
            Orientation::Vertical => Rect { y: rect.y + offset, height: size, ..rect },
        });
        offset += size;
    }
    rects
}

fn collect_bounds(branch: &Branch, orientation: Orientation, rect: Rect, bounds: &mut Vec<(u32, Rect)>) {
    for (index, (child, rect)) in branch.children.iter().zip(child_rects(branch, orientation, rect)).enumerate() {
        if !branch.split.is_view_visible(index) {
            continue;
        }
        match child {
            Node::Leaf(group) => bounds.push((*group, rect)),
            Node::Branch(child) => collect_bounds(child, orthogonal(orientation), rect, bounds),
        }
    }
}

fn collect_sashes(branch: &Branch, orientation: Orientation, rect: Rect, path: &mut Vec<usize>, sash_size: f32, sashes: &mut Vec<GridSash>) {
    let rects = child_rects(branch, orientation, rect);
    for (index, state) in branch.split.sash_states().into_iter().enumerate() {
        let next = rects[index + 1];
        let rect = match orientation {
            Orientation::Horizontal => Rect { x: next.x - sash_size / 2., width: sash_size, ..rect },
            Orientation::Vertical => Rect { y: next.y - sash_size / 2., height: sash_size, ..rect },
        };
        sashes.push(GridSash { id: SashId { path: path.clone(), index }, orientation, rect, state });
    }
    for (index, (child, rect)) in branch.children.iter().zip(rects).enumerate() {
        if let Node::Branch(child) = child {
            path.push(index);
            collect_sashes(child, orthogonal(orientation), rect, path, sash_size, sashes);
            path.pop();
        }
    }
}

fn serialize_branch(branch: &Branch, size: f32) -> SerializedNode {
    let mut children = vec![];
    for (index, child) in branch.children.iter().enumerate() {
        let size = branch.split.visible_size(index);
        children.push(match child {
            Node::Leaf(group) => SerializedNode::Leaf { group: *group, size },
            Node::Branch(child) => serialize_branch(child, size),
        });
    }
    SerializedNode::Branch { children, size }
}

fn build_branch(serialized: &[SerializedNode], orientation: Orientation) -> Branch {
    let mut children = vec![];
    let mut sizes = vec![];
    for node in serialized {
        match node {
            SerializedNode::Leaf { group, size } => {
                children.push(Node::Leaf(*group));
                sizes.push(*size);
            }
            SerializedNode::Branch { children: grandchildren, size } => {
                children.push(Node::Branch(build_branch(grandchildren, orthogonal(orientation))));
                sizes.push(*size);
            }
        }
    }
    let size = sizes.iter().sum();
    Branch { split: new_split(&children, orientation, size, sizes), children }
}

/// grid.ts `sanitizeGridNodeDescriptor` and `createSerializedNode`: a nested node with one group is a leaf and
/// children without a size share what the sized ones leave (or 1).
fn layout_children(groups: &[GroupLayout], take: &mut dyn FnMut() -> u32) -> Vec<SerializedNode> {
    let defined: Vec<f32> = groups.iter().filter_map(|group| group.size.filter(|size| *size > 0.)).collect();
    let undefined_size = if defined.is_empty() { 1. } else { defined.iter().sum() } / (groups.len() - defined.len()) as f32;
    let mut nodes = vec![];
    for group in groups {
        let size = group.size.filter(|size| *size > 0.).unwrap_or(undefined_size);
        nodes.push(if group.groups.len() <= 1 {
            SerializedNode::Leaf { group: take(), size }
        } else {
            SerializedNode::Branch { children: layout_children(&group.groups, take), size }
        });
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect { x, y, width, height }
    }

    fn sizes(grid: &EditorGrid) -> Vec<(u32, f32, f32)> {
        grid.bounds().into_iter().map(|(group, rect)| (group, rect.width, rect.height)).collect()
    }

    fn three_columns() -> EditorGrid {
        let mut grid = EditorGrid::new(1, 1600., 1200.);
        grid.add_group(1, Direction::Right, 2, SplitSizing::Distribute);
        grid.add_group(2, Direction::Right, 3, SplitSizing::Distribute);
        grid
    }

    fn drag_first_sash_to(grid: &mut EditorGrid, to: f32) {
        let sash = grid.sashes(4.)[0].clone();
        let start = sash.rect.x + 2.;
        grid.start_sash_drag(&sash.id, start, false);
        grid.drag_sash(to, false);
        grid.end_sash_drag();
    }

    /// grid.test.ts 'another simple layout with automatic size distribution', at twice the size.
    #[test]
    fn adding_with_distribute_gives_siblings_equal_sizes() {
        let mut grid = EditorGrid::new(1, 1600., 1200.);
        grid.add_group(1, Direction::Left, 2, SplitSizing::Distribute);
        assert_eq!(grid.orientation(), Orientation::Horizontal);
        assert_eq!(sizes(&grid), [(2, 800., 1200.), (1, 800., 1200.)]);
        grid.add_group(1, Direction::Right, 3, SplitSizing::Distribute);
        assert_eq!(sizes(&grid), [(2, 533., 1200.), (1, 533., 1200.), (3, 534., 1200.)]);
        grid.add_group(2, Direction::Down, 4, SplitSizing::Distribute);
        grid.add_group(3, Direction::Up, 5, SplitSizing::Distribute);
        grid.add_group(3, Direction::Down, 6, SplitSizing::Distribute);
        assert_eq!(sizes(&grid), [(2, 533., 600.), (4, 533., 600.), (1, 533., 1200.), (5, 534., 400.), (3, 534., 400.), (6, 534., 400.)]);
        assert_eq!(grid.groups(), [2, 4, 1, 5, 3, 6]);
    }

    /// grid.test.ts 'another simple layout with split size distribution', at twice the size.
    #[test]
    fn adding_with_split_halves_the_reference() {
        let mut grid = EditorGrid::new(1, 1600., 1200.);
        grid.add_group(1, Direction::Left, 2, SplitSizing::Split);
        grid.add_group(1, Direction::Right, 3, SplitSizing::Split);
        assert_eq!(sizes(&grid), [(2, 800., 1200.), (1, 400., 1200.), (3, 400., 1200.)]);
        grid.add_group(2, Direction::Down, 4, SplitSizing::Split);
        grid.add_group(3, Direction::Up, 5, SplitSizing::Split);
        grid.add_group(3, Direction::Down, 6, SplitSizing::Split);
        assert_eq!(sizes(&grid), [(2, 800., 600.), (4, 800., 600.), (1, 400., 1200.), (5, 400., 600.), (3, 400., 300.), (6, 400., 300.)]);
    }

    #[test]
    fn adding_with_auto_distributes_equal_siblings_and_splits_unequal_ones() {
        let mut grid = EditorGrid::new(1, 1600., 1200.);
        grid.add_group(1, Direction::Right, 2, SplitSizing::Auto);
        grid.add_group(2, Direction::Right, 3, SplitSizing::Auto);
        assert_eq!(sizes(&grid), [(1, 533., 1200.), (2, 533., 1200.), (3, 534., 1200.)]);
        drag_first_sash_to(&mut grid, 800.);
        assert_eq!(sizes(&grid), [(1, 800., 1200.), (2, 266., 1200.), (3, 534., 1200.)]);
        grid.add_group(1, Direction::Right, 4, SplitSizing::Auto);
        assert_eq!(sizes(&grid), [(1, 400., 1200.), (4, 400., 1200.), (2, 266., 1200.), (3, 534., 1200.)]);
    }

    /// grid.test.ts 'sizing should be correct after branch demotion #50564', at twice the size.
    #[test]
    fn removing_a_group_folds_its_branch_into_the_parent() {
        let mut grid = EditorGrid::new(1, 1600., 1200.);
        grid.add_group(1, Direction::Right, 2, SplitSizing::Split);
        grid.add_group(2, Direction::Down, 3, SplitSizing::Split);
        grid.add_group(2, Direction::Right, 4, SplitSizing::Split);
        assert_eq!(sizes(&grid), [(1, 800., 1200.), (2, 400., 600.), (4, 400., 600.), (3, 800., 600.)]);
        grid.remove_group(3, SplitSizing::Split);
        assert_eq!(sizes(&grid), [(1, 800., 1200.), (2, 400., 1200.), (4, 400., 1200.)]);
        assert_eq!(grid.sashes(4.).len(), 2);
    }

    /// grid.test.ts 'sizing should be correct after branch demotion #50675', at twice the size.
    #[test]
    fn removing_with_distribute_keeps_the_parent_sizes() {
        let mut grid = EditorGrid::new(1, 1600., 1200.);
        grid.add_group(1, Direction::Down, 2, SplitSizing::Distribute);
        grid.add_group(2, Direction::Down, 3, SplitSizing::Distribute);
        grid.add_group(3, Direction::Right, 4, SplitSizing::Distribute);
        assert_eq!(sizes(&grid), [(1, 1600., 400.), (2, 1600., 400.), (3, 800., 400.), (4, 800., 400.)]);
        grid.remove_group(3, SplitSizing::Distribute);
        assert_eq!(sizes(&grid), [(1, 1600., 400.), (2, 1600., 400.), (4, 1600., 400.)]);
    }

    #[test]
    fn removing_from_a_two_level_root_promotes_the_remaining_branch() {
        let mut grid = EditorGrid::new(1, 1600., 1200.);
        grid.add_group(1, Direction::Right, 2, SplitSizing::Distribute);
        grid.add_group(2, Direction::Down, 3, SplitSizing::Distribute);
        grid.remove_group(1, SplitSizing::Distribute);
        assert_eq!(grid.orientation(), Orientation::Vertical);
        assert_eq!(grid.bounds(), [(2, rect(0., 0., 1600., 600.)), (3, rect(0., 600., 1600., 600.))]);
    }

    /// grid.test.ts 'getNeighborViews should work on simple layout' and '... on a complex layout'.
    #[test]
    fn neighbors_are_the_groups_across_each_edge() {
        let mut grid = EditorGrid::new(1, 1600., 1200.);
        grid.add_group(1, Direction::Down, 2, SplitSizing::Distribute);
        grid.add_group(2, Direction::Down, 3, SplitSizing::Distribute);
        assert_eq!(grid.neighbors(1, Direction::Down, false), [2]);
        assert_eq!(grid.neighbors(1, Direction::Up, false), [] as [u32; 0]);
        assert_eq!(grid.neighbors(1, Direction::Up, true), [3]);
        assert_eq!(grid.neighbors(1, Direction::Right, true), [1]);
        assert_eq!(grid.neighbors(3, Direction::Down, true), [1]);

        grid.add_group(2, Direction::Right, 4, SplitSizing::Distribute);
        grid.add_group(4, Direction::Down, 5, SplitSizing::Distribute);
        assert_eq!(grid.neighbors(1, Direction::Down, false), [2, 4]);
        assert_eq!(grid.neighbors(2, Direction::Right, false), [4, 5]);
        assert_eq!(grid.neighbors(4, Direction::Left, false), [2]);
        assert_eq!(grid.neighbors(5, Direction::Down, false), [3]);
        assert_eq!(grid.neighbors(3, Direction::Up, false), [2, 5]);
        assert_eq!(grid.neighbor(3, Direction::Up), Some(2));
        assert_eq!(grid.neighbor(1, Direction::Left), None);
    }

    /// grid.test.ts 'serialize simple layout' shape: sizes along the parent, the root's across the grid.
    #[test]
    fn serialize_round_trips_structure_and_sizes() {
        let mut grid = EditorGrid::new(1, 1600., 1200.);
        grid.add_group(1, Direction::Right, 2, SplitSizing::Split);
        grid.add_group(2, Direction::Down, 3, SplitSizing::Split);
        let (root, orientation, width, height) = grid.serialize();
        let leaf = |group, size| SerializedNode::Leaf { group, size };
        let expected = SerializedNode::Branch {
            children: vec![leaf(1, 800.), SerializedNode::Branch { children: vec![leaf(2, 600.), leaf(3, 600.)], size: 800. }],
            size: 1200.,
        };
        assert_eq!((&root, orientation, width, height), (&expected, Orientation::Horizontal, 1600., 1200.));

        let mut copy = EditorGrid::deserialize(root, orientation, width, height);
        assert_eq!(copy.bounds(), grid.bounds());
        copy.layout(800., 600.);
        assert_eq!(copy.bounds(), [(1, rect(0., 0., 400., 600.)), (2, rect(400., 0., 400., 300.)), (3, rect(400., 300., 400., 300.))]);
    }

    /// gridview.test.ts 'flipping orientation should preserve absolute offsets'.
    #[test]
    fn flipping_orientation_turns_every_branch() {
        let mut grid = EditorGrid::new(1, 1600., 1200.);
        grid.add_group(1, Direction::Down, 2, SplitSizing::Distribute);
        assert_eq!(grid.bounds(), [(1, rect(0., 0., 1600., 600.)), (2, rect(0., 600., 1600., 600.))]);
        grid.add_group(2, Direction::Right, 3, SplitSizing::Distribute);
        grid.set_orientation(Orientation::Horizontal);
        assert_eq!(grid.bounds(), [(1, rect(0., 0., 800., 1200.)), (2, rect(800., 0., 800., 600.)), (3, rect(800., 600., 800., 600.))]);
    }

    #[test]
    fn apply_layout_builds_a_two_by_two_grid_reusing_groups() {
        let mut grid = three_columns();
        let two = GroupLayout { size: None, groups: vec![GroupLayout::default(), GroupLayout::default()] };
        let mut next = 10;
        let mut ids = || {
            next += 1;
            next
        };
        let left = grid.apply_layout(Orientation::Horizontal, &[two.clone(), two], &mut ids);
        assert_eq!(left, [] as [u32; 0]);
        assert_eq!(grid.bounds(), [(1, rect(0., 0., 800., 600.)), (2, rect(0., 600., 800., 600.)), (3, rect(800., 0., 800., 600.)), (11, rect(800., 600., 800., 600.))]);

        let sized = |size| GroupLayout { size: Some(size), groups: vec![] };
        let left = grid.apply_layout(Orientation::Horizontal, &[sized(0.25), sized(0.75)], &mut ids);
        assert_eq!(left, [3, 11]);
        assert_eq!(sizes(&grid), [(1, 400., 1200.), (2, 1200., 1200.)]);
    }

    /// grid.test.ts 'hasMaximizedView'.
    #[test]
    fn maximize_hides_the_others_and_restore_brings_their_sizes_back() {
        let mut grid = three_columns();
        drag_first_sash_to(&mut grid, 800.);
        grid.add_group(3, Direction::Down, 4, SplitSizing::Distribute);
        let before = grid.bounds();
        grid.maximize(4);
        assert!(grid.is_maximized());
        assert_eq!(grid.bounds(), [(4, rect(0., 0., 1600., 1200.))]);
        assert!(grid.sashes(4.).is_empty());
        grid.restore();
        assert_eq!(grid.maximized(), None);
        assert_eq!(grid.bounds(), before);

        grid.maximize(2);
        grid.add_group(2, Direction::Right, 5, SplitSizing::Split);
        assert!(!grid.is_maximized());
        assert_eq!(grid.groups(), [1, 2, 5, 3, 4]);
    }

    #[test]
    fn reset_sash_distributes_its_branch() {
        let mut grid = three_columns();
        drag_first_sash_to(&mut grid, 800.);
        let sash = grid.sashes(4.)[0].id.clone();
        grid.reset_sash(&sash);
        assert_eq!(sizes(&grid), [(1, 533., 1200.), (2, 533., 1200.), (3, 534., 1200.)]);
    }

    #[test]
    fn expand_leaves_the_others_at_their_minimum_and_distribute_evens_them() {
        let mut grid = three_columns();
        grid.expand(2);
        assert_eq!(sizes(&grid), [(1, 220., 1200.), (2, 1160., 1200.), (3, 220., 1200.)]);
        assert!(grid.is_expanded(2));
        assert!(!grid.is_expanded(1));
        grid.distribute();
        assert_eq!(sizes(&grid), [(1, 533., 1200.), (2, 533., 1200.), (3, 534., 1200.)]);
    }

    #[test]
    fn sashes_are_strips_centered_on_each_boundary() {
        let mut grid = EditorGrid::new(1, 1600., 1200.);
        grid.add_group(1, Direction::Right, 2, SplitSizing::Distribute);
        grid.add_group(2, Direction::Down, 3, SplitSizing::Distribute);
        let sashes = grid.sashes(4.);
        assert_eq!(sashes[0], GridSash { id: SashId { path: vec![], index: 0 }, orientation: Orientation::Horizontal, rect: rect(798., 0., 4., 1200.), state: SashState::Enabled });
        assert_eq!(sashes[1], GridSash { id: SashId { path: vec![1], index: 0 }, orientation: Orientation::Vertical, rect: rect(800., 598., 800., 4.), state: SashState::Enabled });

        grid.start_sash_drag(&sashes[1].id, 600., false);
        grid.drag_sash(900., false);
        grid.end_sash_drag();
        assert_eq!(sizes(&grid), [(1, 800., 1200.), (2, 800., 900.), (3, 800., 300.)]);
    }
}
