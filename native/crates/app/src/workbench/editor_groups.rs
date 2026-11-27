//! The editor part's groups (editorPart.ts, editorGroupView.ts, editorCommands.ts, editorActions.ts): split,
//! focus, move and close groups, join them, the editor layouts, the grid's sashes and its state per group.
//! The active group draws as the editor always did; another group draws through the same code while it is
//! swapped in as the active one (`with_group`), with its focus, widgets and pointer input left out.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    AnyElement, Bounds, Context, CursorStyle, DispatchPhase, Div, DragMoveEvent, ExternalPaths, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, ScrollWheelEvent,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Task, Window, canvas, div, prelude::*, px,
};
use serde_json::{Value, json};

use super::Workbench;
use crate::editor::minimap::MinimapLayoutInfo;
use super::search_editor_view::SEARCH_EDITOR_SCHEME;
use super::webview_panels::WEBVIEW_PANEL_SCHEME;
use crate::commands::Outcome;
use crate::editor::{MODAL_GROUP, PANEL_GROUP};
use crate::editor_grid::{Direction, EditorGrid, GridSash, GroupLayout, Orientation, SashId, SplitSizing};
use crate::split_view::SashState;

/// What the code editor of a group that is not the active one keeps between frames; swapped with the
/// workbench's fields when the group becomes active.
#[derive(Default)]
struct GroupView {
    text_area_left: Rc<Cell<f32>>,
    text_area_bounds: Rc<Cell<Bounds<f32>>>,
    minimap_info: Option<(MinimapLayoutInfo, f64)>,
}

/// A tab being dragged (`DraggedEditorIdentifier`): its workbench, group and index, its file, the pointer's offset in
/// it, and its label for the drag image.
#[derive(Clone)]
pub(super) struct DraggedTab {
    workbench: gpui::EntityId,
    pub(super) group: u32,
    pub(super) index: usize,
    resource: Option<PathBuf>,
    pub(super) offset: gpui::Point<Pixels>,
    label: SharedString,
    foreground: gpui::Rgba,
    background: gpui::Rgba,
}

impl DraggedTab {
    pub(super) fn is_from(&self, workbench: gpui::EntityId) -> bool {
        self.workbench == workbench
    }
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().h(px(32.)).px(px(10.)).flex().items_center().text_size(px(13.)).text_color(self.foreground).bg(self.background).opacity(0.8).child(self.label.clone())
    }
}

/// The main editor part's id; auxiliary parts count up from 1 (auxiliaryEditorPart.ts `COUNTER`).
pub(super) const MAIN_PART: u32 = 0;

/// What `EditorGroups` keeps of each editor part: the part of the active group has it in the fields, the others here.
#[derive(Default)]
pub(super) struct PartLayout {
    pub(super) grid: Option<EditorGrid>,
    part_bounds: Rc<Cell<Bounds<f32>>>,
    hovered_sash: Option<SashId>,
    sash_hover: Option<(SashId, Task<()>)>,
}

impl PartLayout {
    /// An auxiliary part: always a grid, also of one group, which tells the part's groups.
    pub(super) fn auxiliary(group: u32, width: f32, height: f32) -> Self {
        PartLayout { grid: Some(EditorGrid::new(group, width, height)), ..PartLayout::default() }
    }
}

#[derive(Default)]
pub(super) struct EditorGroups {
    /// The part whose layout the next four fields hold: the part of the active group.
    part: u32,
    /// The other parts' layouts by part id.
    pub(super) parked: Vec<(u32, PartLayout)>,
    /// Made at the first split, with the part's size the last frame measured.
    grid: Option<EditorGrid>,
    /// The editor part's content box, measured each frame.
    part_bounds: Rc<Cell<Bounds<f32>>>,
    hovered_sash: Option<SashId>,
    sash_hover: Option<(SashId, Task<()>)>,
    views: HashMap<u32, GroupView>,
    /// Set while another group draws through the active group's code.
    pub(super) drawing_inactive: bool,
    /// The active group while groups draw swapped in: a side by side editor's secondary group, active, draws inside
    /// its owner's swapped body.
    really_active: Option<u32>,
    /// editorDropTarget.ts: the group a dragged tab is over and the side it would split, `None` to move into it.
    drop_target: Option<(u32, Option<Direction>)>,
    /// The tab being dragged, from the drag's start to its release (auxiliary_windows.rs `cross_window_drag`).
    pub(super) dragged_tab: Option<DraggedTab>,
}

impl EditorGroups {
    /// Makes `part`'s layout the one in the fields, parking the current part's.
    pub(super) fn switch_part(&mut self, part: u32) {
        if part == self.part {
            return;
        }
        let index = self.parked.iter().position(|(id, _)| *id == part).expect("an open part");
        let (_, layout) = self.parked.remove(index);
        let leaving = PartLayout {
            grid: std::mem::replace(&mut self.grid, layout.grid),
            part_bounds: std::mem::replace(&mut self.part_bounds, layout.part_bounds),
            hovered_sash: std::mem::replace(&mut self.hovered_sash, layout.hovered_sash),
            sash_hover: std::mem::replace(&mut self.sash_hover, layout.sash_hover),
        };
        self.parked.push((self.part, leaving));
        self.part = part;
    }

    pub(super) fn part_grid(&self, part: u32) -> Option<&EditorGrid> {
        if part == self.part {
            return self.grid.as_ref();
        }
        self.parked.iter().find(|(id, _)| *id == part).and_then(|(_, layout)| layout.grid.as_ref())
    }

    /// The auxiliary part whose grid holds `group`.
    fn auxiliary_part_of(&self, group: u32) -> Option<u32> {
        let current = std::iter::once((self.part, self.grid.as_ref()));
        let parked = self.parked.iter().map(|(id, layout)| (*id, layout.grid.as_ref()));
        current.chain(parked).find(|(id, grid)| *id != MAIN_PART && grid.is_some_and(|grid| grid.groups().contains(&group))).map(|(id, _)| id)
    }

    /// The part of the active group.
    pub(super) fn active_part(&self) -> u32 {
        self.part
    }
}

/// Editors that cannot be copied into a new group (`EditorInputCapabilities.Singleton` or not serializable):
/// a split of one of them makes an empty group.
fn copies_into_split(scheme: &str) -> bool {
    !matches!(scheme, WEBVIEW_PANEL_SCHEME | SEARCH_EDITOR_SCHEME)
}

impl Workbench {
    pub(super) fn register_editor_group_actions(&mut self) {
        self.commands.register("workbench.action.closeEditorsInOtherGroups", |workbench, _, window, cx| {
            workbench.leave_panel_group();
            let keep = workbench.editors.id;
            let others = workbench.editors.group_ids_mru().into_iter().filter(|group| *group != keep && *group != PANEL_GROUP).collect();
            workbench.close_editors_in_groups(others, keep, window, cx)
        });
        let splits = [
            ("workbench.action.splitEditor", None),
            ("workbench.action.splitEditorRight", Some(Direction::Right)),
            ("workbench.action.splitEditorLeft", Some(Direction::Left)),
            ("workbench.action.splitEditorUp", Some(Direction::Up)),
            ("workbench.action.splitEditorDown", Some(Direction::Down)),
        ];
        for (id, direction) in splits {
            self.commands.register(id, move |workbench, args, window, cx| {
                workbench.leave_panel_group();
                // `resolveCommandsContext`: the tab menu's editor is the one split.
                if let Some(index) = workbench.context_editor(&args) {
                    workbench.editors.activate(index);
                }
                let direction = direction.unwrap_or_else(|| workbench.side_by_side_direction());
                workbench.split_editor(direction, window, cx);
                Outcome::done()
            });
        }
        self.commands.register("workbench.action.splitEditorOrthogonal", |workbench, _, window, cx| {
            workbench.leave_panel_group();
            let direction = match workbench.side_by_side_direction() {
                Direction::Right => Direction::Down,
                _ => Direction::Right,
            };
            workbench.split_editor(direction, window, cx);
            Outcome::done()
        });
        let ordinals = ["First", "Second", "Third", "Fourth", "Fifth", "Sixth", "Seventh", "Eighth"];
        for (index, ordinal) in ordinals.into_iter().enumerate() {
            self.commands.register(format!("workbench.action.focus{ordinal}EditorGroup"), move |workbench, _, window, cx| {
                workbench.leave_panel_group();
                workbench.focus_group_at(index, window, cx);
                Outcome::done()
            });
        }
        for (id, next) in [("workbench.action.focusNextGroup", true), ("workbench.action.focusPreviousGroup", false)] {
            self.commands.register(id, move |workbench, _, window, cx| {
                workbench.leave_panel_group();
                if let Some(target) = workbench.group_by_location(next, true) {
                    workbench.focus_group(target, window, cx);
                }
                Outcome::done()
            });
        }
        let directions = [("Left", Direction::Left), ("Right", Direction::Right), ("Above", Direction::Up), ("Below", Direction::Down)];
        for (name, direction) in directions {
            self.commands.register(format!("workbench.action.focus{name}Group"), move |workbench, _, window, cx| {
                workbench.leave_panel_group();
                if let Some(target) = workbench.group_in_direction(direction) {
                    workbench.focus_group(target, window, cx);
                }
                Outcome::done()
            });
            self.commands.register(format!("workbench.action.moveEditorTo{name}Group"), move |workbench, _, window, cx| {
                workbench.leave_panel_group();
                let target = workbench.group_in_direction(direction).unwrap_or_else(|| workbench.add_group_beside(direction));
                workbench.move_active_editor_to(target, window, cx);
                Outcome::done()
            });
        }
        for (name, direction) in [("Left", Direction::Left), ("Right", Direction::Right), ("Up", Direction::Up), ("Down", Direction::Down)] {
            self.commands.register(format!("workbench.action.moveActiveEditorGroup{name}"), move |workbench, _, _, cx| {
                workbench.leave_panel_group();
                workbench.move_active_group(direction, cx);
                Outcome::done()
            });
        }
        for (id, next) in [("workbench.action.moveEditorToNextGroup", true), ("workbench.action.moveEditorToPreviousGroup", false)] {
            self.commands.register(id, move |workbench, _, window, cx| {
                workbench.leave_panel_group();
                let target = match workbench.group_by_location(next, false) {
                    Some(target) => target,
                    None if next => workbench.add_group_beside(workbench.side_by_side_direction()),
                    None => workbench.add_group_beside(opposite(workbench.side_by_side_direction())),
                };
                workbench.move_active_editor_to(target, window, cx);
                Outcome::done()
            });
        }
        for (id, last) in [("workbench.action.moveEditorToFirstGroup", false), ("workbench.action.moveEditorToLastGroup", true)] {
            self.commands.register(id, move |workbench, _, window, cx| {
                workbench.leave_panel_group();
                let groups = workbench.groups_in_grid_order();
                let target = if last { groups[groups.len() - 1] } else { groups[0] };
                workbench.move_active_editor_to(target, window, cx);
                Outcome::done()
            });
        }
        // editorCommands.ts `moveCopyEditorsToGroup` by group: the tab menu's editor (else the active one) moves to the
        // group in that direction, a new group there when there is none.
        let moves = [
            ("workbench.action.moveEditorToAboveGroup", Direction::Up),
            ("workbench.action.moveEditorToBelowGroup", Direction::Down),
            ("workbench.action.moveEditorToLeftGroup", Direction::Left),
            ("workbench.action.moveEditorToRightGroup", Direction::Right),
        ];
        for (id, direction) in moves {
            self.commands.register(id, move |workbench, args, window, cx| {
                workbench.leave_panel_group();
                let Some(index) = workbench.context_editor(&args) else { return Outcome::done() };
                workbench.editors.activate(index);
                let target = workbench.group_in_direction(direction).unwrap_or_else(|| workbench.add_group_beside(direction));
                workbench.move_active_editor_to(target, window, cx);
                Outcome::done()
            });
        }
        self.commands.register("workbench.action.closeGroup", |workbench, _, window, cx| {
            workbench.leave_panel_group();
            workbench.close_group(workbench.editors.id, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.joinTwoGroups", |workbench, _, window, cx| {
            workbench.leave_panel_group();
            let source = workbench.editors.id;
            let target = [Direction::Right, Direction::Down, Direction::Left, Direction::Up].into_iter().find_map(|direction| workbench.group_in_direction(direction));
            if let Some(target) = target {
                workbench.merge_group(source, target, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.joinAllGroups", |workbench, _, window, cx| {
            workbench.leave_panel_group();
            let target = workbench.editors.id;
            for source in workbench.groups_in_grid_order().into_iter().filter(|group| *group != target) {
                workbench.merge_group(source, target, window, cx);
            }
            Outcome::done()
        });
        let layouts: [(&str, Orientation, Vec<GroupLayout>); 8] = [
            ("Single", Orientation::Horizontal, leaves(1)),
            ("TwoColumns", Orientation::Horizontal, leaves(2)),
            ("ThreeColumns", Orientation::Horizontal, leaves(3)),
            ("TwoRows", Orientation::Vertical, leaves(2)),
            ("ThreeRows", Orientation::Vertical, leaves(3)),
            ("TwoByTwoGrid", Orientation::Horizontal, vec![GroupLayout { size: None, groups: leaves(2) }, GroupLayout { size: None, groups: leaves(2) }]),
            ("TwoColumnsBottom", Orientation::Vertical, vec![GroupLayout { size: None, groups: Vec::new() }, GroupLayout { size: None, groups: leaves(2) }]),
            ("TwoRowsRight", Orientation::Horizontal, vec![GroupLayout { size: None, groups: Vec::new() }, GroupLayout { size: None, groups: leaves(2) }]),
        ];
        for (name, orientation, groups) in layouts {
            self.commands.register(format!("workbench.action.editorLayout{name}"), move |workbench, _, window, cx| {
                workbench.leave_panel_group();
                workbench.apply_editor_layout(orientation, &groups, window, cx);
                Outcome::done()
            });
        }
        self.commands.register("workbench.action.toggleEditorGroupLayout", |workbench, _, _, cx| {
            workbench.leave_panel_group();
            if let Some(grid) = workbench.editor_groups.grid.as_mut() {
                let flipped = match grid.orientation() {
                    Orientation::Horizontal => Orientation::Vertical,
                    Orientation::Vertical => Orientation::Horizontal,
                };
                grid.set_orientation(flipped);
                cx.notify();
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.evenEditorWidths", |workbench, _, _, cx| {
            workbench.leave_panel_group();
            if let Some(grid) = workbench.editor_groups.grid.as_mut() {
                grid.distribute();
                cx.notify();
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.toggleEditorWidths", |workbench, _, _, cx| {
            workbench.leave_panel_group();
            let active = workbench.editors.id;
            if let Some(grid) = workbench.editor_groups.grid.as_mut() {
                match grid.is_expanded(active) {
                    true => grid.distribute(),
                    false => grid.expand(active),
                }
                cx.notify();
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.toggleMaximizeEditorGroup", |workbench, _, _, cx| {
            workbench.leave_panel_group();
            let active = workbench.editors.id;
            if let Some(grid) = workbench.editor_groups.grid.as_mut() {
                match grid.is_maximized() {
                    true => grid.restore(),
                    false => grid.maximize(active),
                }
                cx.notify();
            }
            Outcome::done()
        });
    }

    /// The editor part's context keys (editorPart.ts `handleContextKeys`, editorGroupView.ts).
    pub(super) fn editor_groups_context(&self, context: &mut HashMap<String, Value>) {
        let groups = self.part_groups(self.editor_groups.part);
        // The modal group is in no grid: its keys follow the part's group active before it.
        let active = self.editors.group_ids_mru().into_iter().find(|group| groups.contains(group)).expect("the part has a group");
        let index = groups.iter().position(|group| *group == active).expect("found in the grid above");
        let maximized = self.editor_groups.grid.as_ref().is_some_and(EditorGrid::is_maximized);
        context.insert("multipleEditorGroups".into(), json!(self.groups_in_grid_order().len() > 1));
        context.insert("editorPartMultipleEditorGroups".into(), json!(groups.len() > 1));
        context.insert("editorPartMaximizedEditorGroup".into(), json!(maximized));
        context.insert("activeEditorGroupEmpty".into(), json!(self.editors.group_tabs(self.editor_group_id()).0.is_empty()));
        context.insert("activeEditorGroupIndex".into(), json!(index + 1));
        context.insert("activeEditorGroupLast".into(), json!(index + 1 == groups.len()));
        context.insert("activeEditorGroupLocked".into(), json!(false));
        context.insert("groupEditorsCount".into(), json!(self.editors.group_tabs(self.editor_group_id()).0.len()));
    }

    /// editorParts.ts `getGroups(GroupsOrder.GRID_APPEARANCE)`: the main part's groups, then each auxiliary part's in
    /// the order the parts opened.
    pub(super) fn groups_in_grid_order(&self) -> Vec<u32> {
        let mut groups = self.part_groups(MAIN_PART);
        for part in self.auxiliary_windows.part_ids() {
            groups.extend(self.part_groups(part));
        }
        groups
    }

    /// A part's groups in grid order.
    pub(super) fn part_groups(&self, part: u32) -> Vec<u32> {
        match self.editor_groups.part_grid(part) {
            Some(grid) => grid.groups(),
            None => self.editors.group_ids_mru().into_iter().filter(|group| self.part_of_group(*group) == Some(part)).take(1).collect(),
        }
    }

    /// The part `group` is in; `None` for a group in no part's grid: the modal and panel groups and a side by side
    /// editor's secondary side.
    pub(super) fn part_of_group(&self, group: u32) -> Option<u32> {
        if group == MODAL_GROUP || group == PANEL_GROUP || self.editors.is_side_group(group) {
            return None;
        }
        Some(self.editor_groups.auxiliary_part_of(group).unwrap_or(MAIN_PART))
    }

    /// Runs `run` with `part`'s layout in the fields, then brings the active part's back.
    pub(super) fn in_part<R>(&mut self, part: u32, run: impl FnOnce(&mut Self) -> R) -> R {
        let active = self.editor_groups.part;
        self.editor_groups.switch_part(part);
        let result = run(self);
        self.editor_groups.switch_part(active);
        result
    }

    /// The active editor group: the active group, or while the panel's editor has focus the group active before it.
    pub(super) fn editor_group_id(&self) -> u32 {
        self.editors.group_ids_mru()[0]
    }

    /// Editor group commands act on the editor groups: focus in the panel's editor gives way to the last group first.
    pub(super) fn leave_panel_group(&mut self) {
        if self.editors.id == PANEL_GROUP {
            let group = self.editor_group_id();
            self.activate_group(group);
        }
        self.leave_side_group();
    }

    /// `preferredSideBySideGroupDirection`: `workbench.editor.openSideBySideDirection`.
    pub(super) fn side_by_side_direction(&self) -> Direction {
        match self.setting("workbench.editor.openSideBySideDirection").as_str() {
            Some("down") => Direction::Down,
            _ => Direction::Right,
        }
    }

    /// editorPart.ts `getSplitSizingStyle`.
    fn split_sizing(&self) -> SplitSizing {
        match self.setting("workbench.editor.splitSizing").as_str() {
            Some("distribute") => SplitSizing::Distribute,
            Some("split") => SplitSizing::Split,
            _ => SplitSizing::Auto,
        }
    }

    /// editorCommands.ts `splitEditor`: a new group beside the active one with a copy of its active editor, focused.
    pub(super) fn split_editor(&mut self, direction: Direction, window: &mut Window, cx: &mut Context<Self>) {
        let copy = self.editors.active_tab().is_some_and(|tab| copies_into_split(&tab.document().scheme));
        let source = self.editors.id;
        let group = self.editors.add_group(copy);
        self.add_to_grid(source, direction, group);
        self.focus_group(group, window, cx);
    }

    /// editorActions.ts `CloseEditorsInOtherGroupsAction`: each group closes all its editors in turn (a save prompt
    /// waits), then `keep` has the focus again.
    pub(super) fn close_editors_in_groups(&mut self, mut groups: Vec<u32>, keep: u32, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(group) = groups.pop() else {
            // An emptied group may have closed (`workbench.editor.closeEmptyGroups`).
            if self.groups_in_grid_order().contains(&keep) {
                self.focus_group(keep, window, cx);
            }
            return Outcome::done();
        };
        if !self.groups_in_grid_order().contains(&group) {
            return self.close_editors_in_groups(groups, keep, window, cx);
        }
        self.focus_group(group, window, cx);
        // `closeAllEditors({ excludeSticky: true })`.
        match self.close_editors((self.editors.sticky_count()..self.editors.editors.len()).collect(), window, cx) {
            Outcome::Ready(_) => self.close_editors_in_groups(groups, keep, window, cx),
            Outcome::Pending(closing) => {
                let (sender, receiver) = futures::channel::oneshot::channel();
                cx.spawn_in(window, async move |workbench, cx| {
                    let _ = closing.await;
                    let next = workbench.update_in(cx, |workbench, window, cx| workbench.close_editors_in_groups(groups, keep, window, cx));
                    if let Ok(Outcome::Pending(next)) = next {
                        let _ = next.await;
                    }
                    let _ = sender.send(Ok(Value::Null));
                })
                .detach();
                Outcome::Pending(receiver)
            }
        }
    }

    /// `addGroup` without an editor, beside the active group; answers the new group.
    pub(super) fn add_group_beside(&mut self, direction: Direction) -> u32 {
        let source = self.editors.id;
        let group = self.editors.add_group(false);
        self.add_to_grid(source, direction, group);
        group
    }

    /// editorGroupColumn.ts `columnToEditorGroup`: ACTIVE (-1) keeps the active group; column `n` is the nth group in
    /// grid order, adding groups in the side-by-side direction up to it below 9; else SIDE (-2), the group beside
    /// the active one, created when there is none.
    pub(super) fn activate_column(&mut self, column: &Value) {
        self.leave_panel_group();
        let Some(column) = column.as_i64().filter(|column| *column >= 0 || *column == -2) else { return };
        let direction = self.side_by_side_direction();
        if (0..9).contains(&column) {
            while self.groups_in_grid_order().len() <= column as usize {
                let last = *self.groups_in_grid_order().last().expect("one group at least");
                self.activate_group(last);
                self.add_group_beside(direction);
            }
        }
        let target = match self.groups_in_grid_order().get(column as usize) {
            Some(&group) if column >= 0 => group,
            _ => self.group_in_direction(direction).unwrap_or_else(|| self.add_group_beside(direction)),
        };
        self.activate_group(target);
    }

    fn add_to_grid(&mut self, reference: u32, direction: Direction, group: u32) {
        // auxiliaryEditorPart.ts `getAddGroupLocation`: a compact editor window adds groups to the main part.
        if self.part_of_group(reference).is_some_and(|part| self.is_compact_part(part)) {
            let main = self.part_active_group(MAIN_PART).expect("the main part has a group");
            return self.in_part(MAIN_PART, |workbench| workbench.add_to_grid(main, direction, group));
        }
        let sizing = self.split_sizing();
        let bounds = self.editor_groups.part_bounds.get();
        let grid = self.editor_groups.grid.get_or_insert_with(|| EditorGrid::new(reference, bounds.size.width, bounds.size.height));
        grid.add_group(reference, direction, group, sizing);
    }

    /// `activateGroup` and the focus of its editor (`IEditorGroup.focus`).
    pub(crate) fn focus_group(&mut self, group: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_group(group);
        self.editors_changed(cx);
        // editorGroupView.ts `focus` focuses the active pane: a webview editor focuses its webview.
        let webview = self.editors.active().and_then(|editor| self.webview_panels.panels.iter().find(|panel| panel.shows(editor)));
        match webview.map(|panel| panel.handle.clone()) {
            Some(handle) => self.focus_webview_panel(&handle, window, cx),
            None => self.focus_editor_in_its_window(window, cx),
        }
    }

    /// Focuses the active group's editor in the window that shows the group, which comes to the front when it is
    /// not `window`.
    pub(super) fn focus_editor_in_its_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.group_window(self.editors.id).filter(|target| *target != window.window_handle()) else {
            return window.focus(&self.editor_focus, cx);
        };
        let focus = self.editor_focus.clone();
        cx.defer(move |cx| {
            let _ = target.update(cx, |_, window, cx| {
                window.activate_window();
                window.focus(&focus, cx);
            });
        });
    }

    /// Makes `group` the active one: its code editor state comes back from `views`, the leaving one goes there.
    pub(super) fn activate_group(&mut self, group: u32) {
        let previous = self.editors.id;
        if group == previous {
            return;
        }
        let view = self.editor_groups.views.remove(&group).unwrap_or_default();
        let leaving = GroupView {
            text_area_left: std::mem::replace(&mut self.text_area_left, view.text_area_left),
            text_area_bounds: std::mem::replace(&mut self.text_area_bounds, view.text_area_bounds),
            minimap_info: std::mem::replace(&mut self.minimap.info, view.minimap_info),
        };
        self.editor_groups.views.insert(previous, leaving);
        self.editors.activate_group(group);
        if let Some(part) = self.part_of_group(group) {
            self.editor_groups.switch_part(part);
        }
    }

    /// Runs `run` with `group` as the active group, then brings the active group and part back.
    pub(super) fn with_group<R>(&mut self, group: u32, run: impl FnOnce(&mut Self) -> R) -> R {
        let (active, part) = (self.editors.id, self.editor_groups.part);
        self.activate_group(group);
        let result = run(self);
        self.activate_group(active);
        self.editor_groups.switch_part(part);
        result
    }

    /// editorCommands.ts `focusGroup` by index: the next index past the last group adds one beside it.
    fn focus_group_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let groups = self.groups_in_grid_order();
        let group = match groups.get(index) {
            Some(group) => *group,
            None if index == groups.len() => {
                let last = groups[groups.len() - 1];
                let direction = self.side_by_side_direction();
                let group = self.editors.add_group(false);
                self.add_to_grid(last, direction, group);
                group
            }
            None => return,
        };
        self.focus_group(group, window, cx);
    }

    /// `findGroup({ location: NEXT | PREVIOUS }, active, wrap)`.
    fn group_by_location(&self, next: bool, wrap: bool) -> Option<u32> {
        let groups = self.groups_in_grid_order();
        let index = groups.iter().position(|group| *group == self.editors.id)?;
        match next {
            true if index + 1 < groups.len() => Some(groups[index + 1]),
            false if index > 0 => Some(groups[index - 1]),
            true => wrap.then(|| groups[0]).filter(|group| *group != self.editors.id),
            false => wrap.then(|| groups[groups.len() - 1]).filter(|group| *group != self.editors.id),
        }
    }

    /// `findGroup({ direction }, active)`.
    pub(super) fn group_in_direction(&self, direction: Direction) -> Option<u32> {
        self.editor_groups.grid.as_ref()?.neighbor(self.editors.id, direction)
    }

    /// editorCommands.ts `moveActiveEditorToGroup`: the editor leaves its group, the target shows and focuses it,
    /// and the source closes once empty when `workbench.editor.closeEmptyGroups` is on.
    fn move_active_editor_to(&mut self, target: u32, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.editors.id;
        let Some(index) = self.editors.active_index().filter(|_| target != source) else { return };
        let editor = self.editors.take_editor(index);
        self.activate_group(target);
        self.insert_moved_editor(editor);
        self.close_group_if_empty(source);
        self.focus_group(target, window, cx);
    }

    /// `mergeGroup`: every editor of `source` moves to the end of `target`, then `source` closes.
    pub(super) fn merge_group(&mut self, source: u32, target: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.with_group(source, |workbench| {
            let moved: Vec<_> = (0..workbench.editors.editors.len()).map(|_| workbench.editors.take_editor(0)).collect();
            workbench.with_group(target, |workbench| {
                let active = workbench.editors.active_index();
                for editor in moved {
                    workbench.insert_moved_editor(editor);
                }
                if let Some(active) = active {
                    workbench.editors.activate(active);
                }
            });
        });
        self.remove_group(source);
        self.focus_group(target, window, cx);
    }

    /// `workbench.action.closeGroup` (`removeGroup`): a group with editors merges into the part's most recent other
    /// group; an editor window's last group closes the window (auxiliaryEditorPart.ts `removeGroup`).
    fn close_group(&mut self, group: u32, window: &mut Window, cx: &mut Context<Self>) {
        let part = self.part_of_group(group);
        let mru = self.editors.group_ids_mru();
        let Some(&next) = mru.iter().find(|other| **other != group && self.part_of_group(**other) == part) else {
            if let Some(part) = part.filter(|part| *part != MAIN_PART) {
                self.close_auxiliary_window(part, cx);
            }
            return;
        };
        let has_editors = if group == self.editors.id { !self.editors.editors.is_empty() } else { self.editors.group(group).is_some_and(|state| !state.editors.is_empty()) };
        if has_editors {
            return self.merge_group(group, next, window, cx);
        }
        self.remove_group(group);
        self.focus_group(self.editors.id, window, cx);
    }

    /// editorGroupView.ts `doCloseEditor`: the last editor of a group closed with `workbench.editor.closeEmptyGroups`.
    pub(super) fn close_group_if_empty(&mut self, group: u32) {
        let empty = if group == self.editors.id { self.editors.editors.is_empty() } else { self.editors.group(group).is_some_and(|state| state.editors.is_empty()) };
        // The main part keeps its last group; an auxiliary part's last group goes with its window
        // (auxiliaryEditorPart.ts `removeGroup`).
        let removable = match self.part_of_group(group) {
            Some(MAIN_PART) => self.part_groups(MAIN_PART).len() > 1,
            Some(_) => true,
            None => group != PANEL_GROUP,
        };
        // The modal part enforces `closeEmptyGroups`: its group goes with its last editor.
        if empty && (group == MODAL_GROUP || removable && self.setting("workbench.editor.closeEmptyGroups") != json!(false)) {
            self.remove_group(group);
        }
    }

    /// `removeGroup`: the group's editors close; the most recent other group becomes active when it was.
    pub(super) fn remove_group(&mut self, group: u32) {
        let part = self.part_of_group(group);
        if group == self.editors.id {
            let next = self.editors.group_ids_mru()[1];
            self.activate_group(next);
        }
        self.editors.remove_group(group);
        self.editor_groups.views.remove(&group);
        self.remove_from_navigation(|entry| entry.group == group);
        let sizing = self.split_sizing();
        let Some(part) = part else { return };
        let emptied = self.in_part(part, |workbench| {
            // `applyLayout` takes the groups it drops out of the grid before their editors merge.
            let grid = workbench.editor_groups.grid.as_mut().filter(|grid| grid.groups().contains(&group));
            // An editor window's last group: the part goes with it.
            if grid.as_ref().is_some_and(|grid| grid.groups().len() == 1) {
                return true;
            }
            if let Some(grid) = grid {
                grid.remove_group(group, sizing);
            }
            if part == MAIN_PART && workbench.editor_groups.grid.as_ref().is_some_and(|grid| grid.groups().len() < 2) {
                workbench.editor_groups.grid = None;
            }
            false
        });
        if emptied {
            self.close_auxiliary_part(part);
        }
    }

    /// `moveGroup`: the active group goes to the other side of its neighbor in `direction`.
    fn move_active_group(&mut self, direction: Direction, cx: &mut Context<Self>) {
        let group = self.editors.id;
        let Some(target) = self.group_in_direction(direction) else { return };
        let sizing = self.split_sizing();
        let grid = self.editor_groups.grid.as_mut().expect("a neighbor is in the grid");
        grid.remove_group(group, sizing);
        grid.add_group(target, direction, group, sizing);
        // The columns change: the host hears the new view columns and tab model.
        self.editors_changed(cx);
    }

    /// editorPart.ts `applyLayout`: the groups take the new layout in grid order; the editors of groups left
    /// without a place move to the last group.
    fn apply_editor_layout(&mut self, orientation: Orientation, groups: &[GroupLayout], window: &mut Window, cx: &mut Context<Self>) {
        let bounds = self.editor_groups.part_bounds.get();
        let active = self.editors.id;
        let mut grid = self.editor_groups.grid.take().unwrap_or_else(|| EditorGrid::new(active, bounds.size.width, bounds.size.height));
        let mut added = Vec::new();
        let editors = &mut self.editors;
        let dropped = grid.apply_layout(orientation, groups, &mut || {
            let group = editors.add_group(false);
            added.push(group);
            group
        });
        let order = grid.groups();
        self.editor_groups.grid = Some(grid);
        let last = order[order.len() - 1];
        for group in dropped {
            self.merge_group(group, last, window, cx);
        }
        if order.len() == 1 && self.editor_groups.part == MAIN_PART {
            self.editor_groups.grid = None;
        }
        let focus = if order.contains(&active) { active } else { last };
        self.focus_group(focus, window, cx);
    }

    /// layout.ts `hasMoreThanOneColumn` unless a group is maximized.
    pub(super) fn editor_groups_have_several_columns(&self) -> bool {
        let Some(grid) = self.editor_groups.grid.as_ref().filter(|grid| !grid.is_maximized()) else { return false };
        let bounds = grid.bounds();
        bounds.iter().any(|(_, rect)| rect.x != bounds[0].1.x)
    }

    /// The editor part: one card holding every group at its place in the grid, the separators and sashes between them.
    pub(super) fn editor_groups_part(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let part_bounds = self.editor_groups.part_bounds.clone();
        // The centered layout's margins beside the groups (centeredViewLayout.ts), measured out of the groups' room.
        let margins = self.centered_margins();
        let (left, right) = margins.unwrap_or_default();
        let measure = canvas(move |bounds, _, _| part_bounds.set(bounds.map(f32::from)), |_, _, _, _| ()).absolute().top_0().bottom_0().left(px(left)).right(px(right));
        let centered_chrome = self.centered_layout_chrome(margins, cx);
        // floatingPanels.css floats the main window's parts; an editor window's part fills it.
        let floating = self.editor_groups.part == MAIN_PART;
        let card = div()
            .flex_1()
            // A long breadcrumbs path must not widen the part past the window.
            .min_w_0()
            .min_h_0()
            .relative()
            .bg(self.color("editor.background"))
            .when(floating, |card| match self.compact_density() {
                // editorBorder.css: a compact editor has no left border unless it meets the window's left edge.
                true => card.border_t_1().border_r_1().border_b_1().when(self.part_at_left_edge(super::layout::Part::Editor), |card| card.border_l_1()).border_color(gpui::transparent_black()),
                false => card.border_1().rounded(px(8.)).border_color(self.color("surface.border")),
            })
            .pl(px(left))
            .pr(px(right))
            .child(measure)
            .children(centered_chrome)
            .children(self.centered_sash_drag(cx));
        let sash_size = self.sash_size();
        let Some(grid) = self.editor_groups.grid.as_mut() else {
            // The modal part's group draws in the modal; the part shows the group active before it.
            let group = self.part_groups(self.editor_groups.part)[0];
            if self.editors.group_tabs(group).0.is_empty() {
                let strokes = self.compact_density().then(|| self.part_top_border());
                // editorDropTarget.ts: the empty part takes drops too.
                let empty = div().flex_1().min_h_0().relative().flex().flex_col().items_center().justify_center().child(self.watermark(256.));
                return card.flex().flex_col().child(self.group_drop_area(group, empty, cx)).children(strokes);
            }
            let body = if group == self.editors.id { self.editor_group_body(window, cx) } else { self.inactive_group_body(group, window, cx) };
            let area = self.group_drop_area(group, div().flex_1().min_h_0().relative().flex().flex_col().child(body), cx);
            return card.flex().flex_col().child(area).when(floating, |card| card.child(self.part_top_border()));
        };
        let size = self.editor_groups.part_bounds.get().size;
        grid.layout(size.width, size.height);
        let places = grid.bounds();
        let sashes = self.editor_groups.grid.as_ref().expect("checked above").sashes(sash_size);
        let separator = self.theme.color("editorGroup.border").or(self.theme.color("contrastBorder")).unwrap_or(gpui::transparent_black().into());
        let active = self.editors.id;
        let mut groups: Vec<Stateful<Div>> = Vec::new();
        for (group, rect) in places {
            let body = match group == active {
                true => self.editor_group_body(window, cx),
                false => self.inactive_group_body(group, window, cx),
            };
            let area = div().absolute().left(px(rect.x)).top(px(rect.y)).w(px(rect.width)).h(px(rect.height)).overflow_hidden().flex().flex_col().child(body);
            groups.push(self.group_drop_area(group, area, cx));
        }
        // splitview.css: the separator is the 1px `::before` at the start of the view after the sash.
        let separators = sashes.iter().map(|sash| {
            let line = div().absolute().bg(separator);
            match sash.orientation {
                Orientation::Horizontal => line.left(px(sash.rect.x + sash.rect.width / 2.)).top(px(sash.rect.y)).w(px(1.)).h(px(sash.rect.height)),
                Orientation::Vertical => line.top(px(sash.rect.y + sash.rect.height / 2.)).left(px(sash.rect.x)).h(px(1.)).w(px(sash.rect.width)),
            }
        });
        let separators: Vec<Div> = separators.collect();
        let sash_elements: Vec<AnyElement> = sashes.into_iter().map(|sash| self.group_sash(sash, cx)).collect();
        let dragging = self.editor_groups.grid.as_ref().is_some_and(|grid| grid.dragged_sash().is_some());
        let groups_area = div().absolute().top_0().bottom_0().left(px(left)).right(px(right)).children(groups).children(separators).children(sash_elements);
        card.child(groups_area).when(dragging, |card| card.child(self.group_sash_drag(cx))).when(floating, |card| card.child(self.part_top_border()))
    }

    /// A group's tabs and editor, or the watermark of an empty group (editorgroupview.css: at most 200px in a part of
    /// several groups, without the shortcuts).
    fn editor_group_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        match self.editors.active() {
            Some(_) => {
                let secondary = self.side_by_side_secondary(window, cx);
                self.text_editor_part(secondary, window, cx)
            }
            None => div().size_full().flex().flex_col().items_center().justify_center().child(self.watermark(200.)),
        }
    }

    fn inactive_group_body(&mut self, group: u32, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.swapped_group_body(group, |workbench, window, cx| workbench.editor_group_body(window, cx), window, cx)
    }

    /// Another group drawn by `draw` while swapped in. A press in it activates it before the press reaches its
    /// editor; moves and the wheel over it go to it alone (editorGroupView.ts focuses the group on mousedown).
    pub(super) fn swapped_group_body(&mut self, group: u32, draw: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) -> Div, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let outermost = self.editor_groups.really_active.is_none();
        let really_active = *self.editor_groups.really_active.get_or_insert(self.editors.id);
        let was_inactive = self.editor_groups.drawing_inactive;
        let body = self.with_group(group, |workbench| {
            workbench.editor_groups.drawing_inactive = group != really_active;
            let body = draw(workbench, window, cx);
            workbench.editor_groups.drawing_inactive = was_inactive;
            body
        });
        if outermost {
            self.editor_groups.really_active = None;
        }
        let workbench = cx.entity();
        let guard = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let pressed = workbench.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && bounds.contains(&event.position) {
                        pressed.update(cx, |workbench, cx| workbench.focus_group(group, window, cx));
                    }
                });
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && event.pressed_button.is_none() && bounds.contains(&event.position) {
                        cx.stop_propagation();
                    }
                });
                let scrolled = workbench.clone();
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && bounds.contains(&event.position) {
                        cx.stop_propagation();
                        scrolled.update(cx, |workbench, cx| workbench.with_group(group, |workbench| workbench.editor_wheel(event, window, cx)));
                    }
                });
            },
        );
        body.child(guard.absolute().inset_0())
    }

    /// The text area of a group that is not active got its size (see `editor_laid_out`).
    pub(super) fn group_laid_out(&mut self, group: u32, laid_out: impl FnOnce(&mut Self)) {
        match group == self.editors.id {
            true => laid_out(self),
            false if self.editors.group(group).is_some() => self.with_group(group, laid_out),
            false => {}
        }
    }

    /// multieditortabscontrol.ts: a tab drags to another place in its group, another group's tabs, or a group's
    /// drop area; over a tab the `tab.dragAndDropBorder` line marks where it goes.
    /// A tab of `group` (a tab strip's, or an Open Editors row): dragged, and the drop before it.
    pub(super) fn tab_drag_and_drop(&self, tab: Stateful<Div>, group: u32, index: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let dragged = self.dragged_tab(group, index, cx);
        let border = self.theme.color("tab.dragAndDropBorder").unwrap_or(self.foreground_or("tab.activeForeground"));
        let workbench = cx.entity().downgrade();
        tab.on_drag(dragged, move |dragged, offset, _, cx| {
            let dragged = DraggedTab { offset, ..dragged.clone() };
            let _ = workbench.update(cx, |workbench, _| workbench.editor_groups.dragged_tab = Some(dragged.clone()));
            cx.new(|_| dragged)
        })
            .drag_over::<DraggedTab>(move |style, _, _, _| style.border_l_2().border_color(border))
            .on_drop(cx.listener(move |workbench, dragged: &DraggedTab, window, cx| {
                workbench.editor_groups.drop_target = None;
                workbench.move_dragged_tab(dragged, group, Some(index), window, cx);
            }))
    }

    pub(super) fn dragged_tab(&self, group: u32, index: usize, cx: &Context<Self>) -> DraggedTab {
        let tab = &self.editors.group_tabs(group).0[index];
        DraggedTab {
            workbench: cx.entity_id(),
            group,
            index,
            resource: Some(tab.document()).filter(|document| document.scheme == "file").map(|document| document.path.clone()),
            offset: gpui::Point::default(),
            label: self.editor_name(&self.editors.group_tabs(group).0[index]).into(),
            foreground: self.foreground_or("tab.activeForeground"),
            background: self.color("tab.activeBackground"),
        }
    }

    /// The drop area of a group (below its tabs): the half it would split into, or the whole of it.
    fn group_drop_area(&self, group: u32, body: Div, cx: &mut Context<Self>) -> Stateful<Div> {
        // editorDropTarget.ts `getDropAreaOffsetHeight`: below the tabs of a group with editors.
        let title_height = if self.editors.group_tabs(group).0.is_empty() { 0. } else { 32. };
        let target = self.editor_groups.drop_target.filter(|(over, _)| *over == group && cx.has_active_drag());
        let prefer_vertical = self.side_by_side_direction() == Direction::Right;
        let overlay = target.map(|(_, direction)| {
            let fill = div().absolute().bg(self.color("editorGroup.dropBackground"));
            let fill = match self.theme.color("activeContrastBorder") {
                Some(border) => fill.border_2().border_dashed().border_color(border),
                None => fill,
            };
            let area = div().absolute().left_0().right_0().bottom_0().top(px(title_height));
            area.child(match direction {
                None => fill.inset_0(),
                Some(Direction::Up) => fill.top_0().left_0().right_0().h_1_2(),
                Some(Direction::Down) => fill.bottom_0().left_0().right_0().h_1_2(),
                Some(Direction::Left) => fill.top_0().bottom_0().left_0().w_1_2(),
                Some(Direction::Right) => fill.top_0().bottom_0().right_0().w_1_2(),
            })
        });
        body.id(("group-drop", group as usize))
            .on_drag_move::<DraggedTab>(cx.listener(move |workbench, event: &DragMoveEvent<DraggedTab>, _, cx| {
                workbench.drag_over_group(group, event.bounds, event.event.position, title_height, prefer_vertical, cx);
            }))
            // editorDropTarget.ts: files from outside the window take the same drop overlay as a tab.
            .on_drag_move::<ExternalPaths>(cx.listener(move |workbench, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                workbench.drag_over_group(group, event.bounds, event.event.position, title_height, prefer_vertical, cx);
            }))
            .on_drop(cx.listener(move |workbench, dragged: &DraggedTab, window, cx| {
                let Some(target) = workbench.drop_target_group(group) else { return };
                workbench.move_dragged_tab(dragged, target, None, window, cx);
            }))
            .on_drop(cx.listener(move |workbench, paths: &ExternalPaths, window, cx| {
                let Some(target) = workbench.drop_target_group(group) else { return };
                workbench.drop_external_paths(paths.paths().to_vec(), target, window, cx);
            }))
            .children(overlay)
    }

    /// `getSplitDirection` for the pointer over a group's drop area (below its tabs).
    fn drag_over_group(&mut self, group: u32, bounds: Bounds<Pixels>, position: gpui::Point<Pixels>, title_height: f32, prefer_vertical: bool, cx: &mut Context<Self>) {
        let (x, y) = (f32::from(position.x - bounds.origin.x), f32::from(position.y - bounds.origin.y) - title_height);
        let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height) - title_height);
        let inside = x >= 0. && y >= 0. && x <= width && y <= height;
        let target = inside.then(|| (group, split_direction(x, y, width, height, prefer_vertical)));
        let over_this = self.editor_groups.drop_target.is_some_and(|(over, _)| over == group);
        if (target.is_some() || over_this) && self.editor_groups.drop_target != target {
            self.editor_groups.drop_target = target;
            cx.notify();
        }
    }

    /// The group a drop on `group` goes to: `group`, or a new group on the side its overlay showed.
    fn drop_target_group(&mut self, group: u32) -> Option<u32> {
        let (_, direction) = self.editor_groups.drop_target.take().filter(|(over, _)| *over == group)?;
        Some(match direction {
            Some(direction) => {
                self.activate_group(group);
                self.add_group_beside(direction)
            }
            None => group,
        })
    }

    /// dnd.ts `ResourcesDropHandler.handleDrop` for files from outside the window: a workspace file or a single folder
    /// opens in a new window (`handleWorkspaceDrop`), the files open pinned in the target group and join the recently
    /// opened list.
    pub(super) fn drop_external_paths(&mut self, paths: Vec<PathBuf>, target: u32, window: &mut Window, cx: &mut Context<Self>) {
        let workspaces: Vec<&PathBuf> = paths.iter().filter(|path| path.is_dir() || path.extension().is_some_and(|extension| extension == "code-workspace")).collect();
        if !workspaces.is_empty() {
            // shortcut: several folders open a window each; upstream enters an untitled multi-root workspace, which the port has not.
            for path in workspaces {
                super::host::open_new_window(Some(path.clone()), cx);
            }
            return;
        }
        let files: Vec<PathBuf> = paths.into_iter().filter(|path| path.is_file()).collect();
        if files.is_empty() {
            return;
        }
        let entries = files.iter().map(|path| crate::recently_opened::entry(crate::recently_opened::Kind::File, path, None)).collect();
        crate::recently_opened::add(&mut self.shared_storage.lock().unwrap(), entries);
        self.recently_opened_changed(window, cx);
        self.activate_group(target);
        for path in files {
            self.open_file(path, true, window, cx);
        }
        window.focus(&self.editor_focus.clone(), cx);
        cx.notify();
    }

    /// `moveEditor` / `doMoveEditorAcrossGroups` of a dropped tab, to `index` or after the target's active editor.
    pub(super) fn move_dragged_tab(&mut self, dragged: &DraggedTab, target: u32, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_groups.dragged_tab = None;
        // A tab of another window's workbench is a resource drop there (dnd.ts `ResourcesDropHandler`): its file opens.
        if !dragged.is_from(cx.entity_id()) {
            return self.drop_external_paths(dragged.resource.iter().cloned().collect(), target, window, cx);
        }
        if dragged.group == target {
            self.with_group(target, |workbench| workbench.editors.move_editor(dragged.index, index.unwrap_or(usize::MAX)));
        } else {
            let mut editor = self.with_group(dragged.group, |workbench| workbench.editors.take_editor(dragged.index));
            // multiEditorTabsControl.ts `onDrop` opens with `sticky: false`: only a drop into the sticky range sticks it.
            if index.is_some() {
                editor.sticky = false;
            }
            self.with_group(target, |workbench| {
                workbench.insert_moved_editor(editor);
                if let (Some(index), Some(from)) = (index, workbench.editors.active_index()) {
                    workbench.editors.move_editor(from, index);
                }
            });
            self.close_group_if_empty(dragged.group);
        }
        self.focus_group(target, window, cx);
    }

    /// `.monaco-sash` between groups: the grid's sash size, the hover border after the hover delay, double click
    /// resets the sizes of that branch (grid.ts `onDidSashReset`).
    fn group_sash(&self, sash: GridSash, cx: &mut Context<Self>) -> AnyElement {
        if sash.state == SashState::Disabled {
            return div().into_any_element();
        }
        let vertical = sash.orientation == Orientation::Horizontal;
        let grid = self.editor_groups.grid.as_ref().expect("sashes are drawn with the grid");
        let active = self.editor_groups.hovered_sash.as_ref() == Some(&sash.id) || grid.dragged_sash().is_some_and(|(dragged, _)| dragged == sash.id);
        let hover_border = self.theme.color("sash.hoverBorder").unwrap_or(self.color("focusBorder"));
        let cursor = match (vertical, sash.state) {
            (true, SashState::AtMinimum) => CursorStyle::ResizeRight,
            (true, SashState::AtMaximum) => CursorStyle::ResizeLeft,
            (true, _) => CursorStyle::ResizeColumn,
            (false, SashState::AtMinimum) => CursorStyle::ResizeDown,
            (false, SashState::AtMaximum) => CursorStyle::ResizeUp,
            (false, _) => CursorStyle::ResizeRow,
        };
        let (hovered, pressed) = (sash.id.clone(), sash.id.clone());
        div()
            .id(gpui::ElementId::Name(format!("group-sash-{:?}", sash.id).into()))
            .absolute()
            .left(px(sash.rect.x))
            .top(px(sash.rect.y))
            .w(px(sash.rect.width))
            .h(px(sash.rect.height))
            .cursor(cursor)
            .when(active, |element| {
                // `--vscode-sash-hover-size`, centered in the sash.
                let (size, hover) = (self.sash_size(), self.sash_hover_size());
                let highlight = div().absolute().bg(hover_border);
                element.child(match vertical {
                    true => highlight.top_0().bottom_0().left(px((size - hover) / 2.)).w(px(hover)),
                    false => highlight.left_0().right_0().top(px((size - hover) / 2.)).h(px(hover)),
                })
            })
            .on_hover(cx.listener(move |workbench, over: &bool, window, cx| {
                let sashes = &mut workbench.editor_groups;
                if !*over {
                    if sashes.hovered_sash.as_ref() == Some(&hovered) {
                        sashes.hovered_sash = None;
                    }
                    if sashes.sash_hover.as_ref().is_some_and(|(pending, _)| *pending == hovered) {
                        sashes.sash_hover = None;
                    }
                    cx.notify();
                    return;
                }
                let delay = workbench.sash_hover_delay();
                let id = hovered.clone();
                workbench.editor_groups.sash_hover = Some((
                    hovered.clone(),
                    cx.spawn_in(window, async move |workbench, cx| {
                        cx.background_executor().timer(delay).await;
                        let _ = workbench.update(cx, |workbench, cx| {
                            workbench.editor_groups.hovered_sash = Some(id);
                            cx.notify();
                        });
                    }),
                ));
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |workbench, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let grid = workbench.editor_groups.grid.as_mut().expect("sashes are drawn with the grid");
                    if event.click_count == 2 {
                        grid.reset_sash(&pressed);
                    } else {
                        let position = if vertical { event.position.x } else { event.position.y };
                        grid.start_sash_drag(&pressed, f32::from(position), event.modifiers.alt);
                    }
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// Follows the pointer anywhere in the window while a group sash drags.
    fn group_sash_drag(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let workbench = cx.entity();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
                    moved.update(cx, |workbench, cx| {
                        let grid = workbench.editor_groups.grid.as_mut().expect("a sash drags in the grid");
                        let Some((_, orientation)) = grid.dragged_sash() else { return };
                        let position = if orientation == Orientation::Horizontal { event.position.x } else { event.position.y };
                        grid.drag_sash(f32::from(position), event.modifiers.alt);
                        cx.notify();
                    });
                });
                let released = workbench.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                    released.update(cx, |workbench, cx| {
                        workbench.editor_groups.grid.as_mut().expect("a sash drags in the grid").end_sash_drag();
                        cx.notify();
                    });
                });
            },
        )
        .size_0()
    }
}

/// editorDropTarget.ts `getSplitDirection` for an editor drag: 10% edges split, then the preferred direction's
/// thirds decide the side; the center moves into the group.
fn split_direction(x: f32, y: f32, width: f32, height: f32, prefer_vertical: bool) -> Option<Direction> {
    let (edge_width, edge_height) = (width * 0.1, height * 0.1);
    if x > edge_width && x < width - edge_width && y > edge_height && y < height - edge_height {
        return None;
    }
    let (split_width, split_height) = (width / 3., height / 3.);
    Some(match prefer_vertical {
        true if x < split_width => Direction::Left,
        true if x > split_width * 2. => Direction::Right,
        true if y < height / 2. => Direction::Up,
        true => Direction::Down,
        false if y < split_height => Direction::Up,
        false if y > split_height * 2. => Direction::Down,
        false if x < width / 2. => Direction::Left,
        false => Direction::Right,
    })
}

fn leaves(count: usize) -> Vec<GroupLayout> {
    (0..count).map(|_| GroupLayout { size: None, groups: Vec::new() }).collect()
}

fn opposite(direction: Direction) -> Direction {
    match direction {
        Direction::Up => Direction::Down,
        Direction::Down => Direction::Up,
        Direction::Left => Direction::Right,
        Direction::Right => Direction::Left,
    }
}

mod part_state;

#[cfg(test)]
mod tests;
