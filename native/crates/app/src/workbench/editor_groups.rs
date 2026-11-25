//! The editor part's groups (editorPart.ts, editorGroupView.ts, editorCommands.ts, editorActions.ts): split,
//! focus, move and close groups, join them, the editor layouts, the grid's sashes and its state per group.
//! The active group draws as the editor always did; another group draws through the same code while it is
//! swapped in as the active one (`with_group`), with its focus, widgets and pointer input left out.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, Bounds, Context, CursorStyle, DispatchPhase, Div, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, ScrollWheelEvent, StatefulInteractiveElement, Styled, Task,
    Window, canvas, div, prelude::FluentBuilder, px,
};
use serde_json::{Value, json};

use super::Workbench;
use crate::editor::minimap::MinimapLayoutInfo;
use super::search_editor_view::SEARCH_EDITOR_SCHEME;
use super::terminal_view::TERMINAL_SCHEME;
use super::webview_panels::WEBVIEW_PANEL_SCHEME;
use crate::commands::Outcome;
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

#[derive(Default)]
pub(super) struct EditorGroups {
    /// Made at the first split, with the part's size the last frame measured.
    grid: Option<EditorGrid>,
    views: HashMap<u32, GroupView>,
    /// The editor part's content box, measured each frame.
    part_bounds: Rc<Cell<Bounds<f32>>>,
    /// Set while another group draws through the active group's code.
    pub(super) drawing_inactive: bool,
    hovered_sash: Option<SashId>,
    sash_hover: Option<(SashId, Task<()>)>,
}

/// Editors that cannot be copied into a new group (`EditorInputCapabilities.Singleton` or not serializable):
/// a split of one of them makes an empty group.
fn copies_into_split(scheme: &str) -> bool {
    !matches!(scheme, WEBVIEW_PANEL_SCHEME | TERMINAL_SCHEME | SEARCH_EDITOR_SCHEME | crate::output::OUTPUT_SCHEME)
}

impl Workbench {
    pub(super) fn register_editor_group_actions(&mut self) {
        let splits = [
            ("workbench.action.splitEditor", None),
            ("workbench.action.splitEditorRight", Some(Direction::Right)),
            ("workbench.action.splitEditorLeft", Some(Direction::Left)),
            ("workbench.action.splitEditorUp", Some(Direction::Up)),
            ("workbench.action.splitEditorDown", Some(Direction::Down)),
        ];
        for (id, direction) in splits {
            self.commands.register(id, move |workbench, _, window, cx| {
                let direction = direction.unwrap_or_else(|| workbench.side_by_side_direction());
                workbench.split_editor(direction, window, cx);
                Outcome::done()
            });
        }
        self.commands.register("workbench.action.splitEditorOrthogonal", |workbench, _, window, cx| {
            let direction = match workbench.side_by_side_direction() {
                Direction::Right => Direction::Down,
                _ => Direction::Right,
            };
            workbench.split_editor(direction, window, cx);
            Outcome::done()
        });
        let ordinals = ["First", "Second", "Third", "Fourth", "Fifth", "Sixth", "Seventh", "Eighth"];
        for (index, ordinal) in ordinals.into_iter().enumerate() {
            self.commands.register(&format!("workbench.action.focus{ordinal}EditorGroup"), move |workbench, _, window, cx| {
                workbench.focus_group_at(index, window, cx);
                Outcome::done()
            });
        }
        for (id, next) in [("workbench.action.focusNextGroup", true), ("workbench.action.focusPreviousGroup", false)] {
            self.commands.register(id, move |workbench, _, window, cx| {
                if let Some(target) = workbench.group_by_location(next, true) {
                    workbench.focus_group(target, window, cx);
                }
                Outcome::done()
            });
        }
        let directions = [("Left", Direction::Left), ("Right", Direction::Right), ("Above", Direction::Up), ("Below", Direction::Down)];
        for (name, direction) in directions {
            self.commands.register(&format!("workbench.action.focus{name}Group"), move |workbench, _, window, cx| {
                if let Some(target) = workbench.group_in_direction(direction) {
                    workbench.focus_group(target, window, cx);
                }
                Outcome::done()
            });
            self.commands.register(&format!("workbench.action.moveEditorTo{name}Group"), move |workbench, _, window, cx| {
                let target = workbench.group_in_direction(direction).unwrap_or_else(|| workbench.add_group_beside(direction));
                workbench.move_active_editor_to(target, window, cx);
                Outcome::done()
            });
        }
        for (name, direction) in [("Left", Direction::Left), ("Right", Direction::Right), ("Up", Direction::Up), ("Down", Direction::Down)] {
            self.commands.register(&format!("workbench.action.moveActiveEditorGroup{name}"), move |workbench, _, _, cx| {
                workbench.move_active_group(direction, cx);
                Outcome::done()
            });
        }
        for (id, next) in [("workbench.action.moveEditorToNextGroup", true), ("workbench.action.moveEditorToPreviousGroup", false)] {
            self.commands.register(id, move |workbench, _, window, cx| {
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
                let groups = workbench.groups_in_grid_order();
                let target = if last { groups[groups.len() - 1] } else { groups[0] };
                workbench.move_active_editor_to(target, window, cx);
                Outcome::done()
            });
        }
        self.commands.register("workbench.action.closeGroup", |workbench, _, window, cx| {
            workbench.close_group(workbench.editors.id, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.joinTwoGroups", |workbench, _, window, cx| {
            let source = workbench.editors.id;
            let target = [Direction::Right, Direction::Down, Direction::Left, Direction::Up].into_iter().find_map(|direction| workbench.group_in_direction(direction));
            if let Some(target) = target {
                workbench.merge_group(source, target, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.joinAllGroups", |workbench, _, window, cx| {
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
            self.commands.register(&format!("workbench.action.editorLayout{name}"), move |workbench, _, window, cx| {
                workbench.apply_editor_layout(orientation, &groups, window, cx);
                Outcome::done()
            });
        }
        self.commands.register("workbench.action.toggleEditorGroupLayout", |workbench, _, _, cx| {
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
            if let Some(grid) = workbench.editor_groups.grid.as_mut() {
                grid.distribute();
                cx.notify();
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.toggleEditorWidths", |workbench, _, _, cx| {
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
        let groups = self.groups_in_grid_order();
        let index = groups.iter().position(|group| *group == self.editors.id).expect("the active group is in the grid");
        let maximized = self.editor_groups.grid.as_ref().is_some_and(EditorGrid::is_maximized);
        context.insert("multipleEditorGroups".into(), json!(groups.len() > 1));
        context.insert("editorPartMultipleEditorGroups".into(), json!(groups.len() > 1));
        context.insert("editorPartMaximizedEditorGroup".into(), json!(maximized));
        context.insert("activeEditorGroupEmpty".into(), json!(self.editors.editors.is_empty()));
        context.insert("activeEditorGroupIndex".into(), json!(index + 1));
        context.insert("activeEditorGroupLast".into(), json!(index + 1 == groups.len()));
        context.insert("activeEditorGroupLocked".into(), json!(false));
        context.insert("groupEditorsCount".into(), json!(self.editors.editors.len()));
    }

    /// `getGroups(GroupsOrder.GRID_APPEARANCE)`.
    pub(super) fn groups_in_grid_order(&self) -> Vec<u32> {
        match &self.editor_groups.grid {
            Some(grid) => grid.groups(),
            None => vec![self.editors.id],
        }
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

    /// `addGroup` without an editor, beside the active group; answers the new group.
    pub(super) fn add_group_beside(&mut self, direction: Direction) -> u32 {
        let source = self.editors.id;
        let group = self.editors.add_group(false);
        self.add_to_grid(source, direction, group);
        group
    }

    fn add_to_grid(&mut self, reference: u32, direction: Direction, group: u32) {
        let sizing = self.split_sizing();
        let bounds = self.editor_groups.part_bounds.get();
        let grid = self.editor_groups.grid.get_or_insert_with(|| EditorGrid::new(reference, bounds.size.width, bounds.size.height));
        grid.add_group(reference, direction, group, sizing);
    }

    /// `activateGroup` and the focus of its editor (`IEditorGroup.focus`).
    pub(super) fn focus_group(&mut self, group: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_group(group);
        window.focus(&self.editor_focus, cx);
        self.editors_changed(cx);
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
    }

    /// Runs `run` with `group` as the active group, then brings the active group back.
    pub(super) fn with_group<R>(&mut self, group: u32, run: impl FnOnce(&mut Self) -> R) -> R {
        let active = self.editors.id;
        self.activate_group(group);
        let result = run(self);
        self.activate_group(active);
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
    fn group_in_direction(&self, direction: Direction) -> Option<u32> {
        self.editor_groups.grid.as_ref()?.neighbor(self.editors.id, direction)
    }

    /// editorCommands.ts `moveActiveEditorToGroup`: the editor leaves its group, the target shows and focuses it,
    /// and the source closes once empty when `workbench.editor.closeEmptyGroups` is on.
    fn move_active_editor_to(&mut self, target: u32, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.editors.id;
        let Some(index) = self.editors.active_index().filter(|_| target != source) else { return };
        let editor = self.editors.take_editor(index);
        self.activate_group(target);
        self.editors.insert_moved(editor);
        self.close_group_if_empty(source);
        self.focus_group(target, window, cx);
    }

    /// `mergeGroup`: every editor of `source` moves to the end of `target`, then `source` closes.
    fn merge_group(&mut self, source: u32, target: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.with_group(source, |workbench| {
            let moved: Vec<_> = (0..workbench.editors.editors.len()).map(|_| workbench.editors.take_editor(0)).collect();
            workbench.with_group(target, |workbench| {
                let active = workbench.editors.active_index();
                for editor in moved {
                    workbench.editors.insert_moved(editor);
                }
                if let Some(active) = active {
                    workbench.editors.activate(active);
                }
            });
        });
        self.remove_group(source);
        self.focus_group(target, window, cx);
    }

    /// `workbench.action.closeGroup` (`removeGroup`): a group with editors merges into the most recent other group.
    fn close_group(&mut self, group: u32, window: &mut Window, cx: &mut Context<Self>) {
        let mru = self.editors.group_ids_mru();
        let Some(&next) = mru.iter().find(|other| **other != group) else { return };
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
        if empty && self.editors.other_groups.len() > 0 && self.setting("workbench.editor.closeEmptyGroups") != json!(false) {
            self.remove_group(group);
        }
    }

    /// `removeGroup`: the group's editors close; the most recent other group becomes active when it was.
    fn remove_group(&mut self, group: u32) {
        if group == self.editors.id {
            let next = self.editors.group_ids_mru()[1];
            self.activate_group(next);
        }
        self.editors.remove_group(group);
        self.editor_groups.views.remove(&group);
        self.remove_from_navigation(|entry| entry.group == group);
        let sizing = self.split_sizing();
        let grid = self.editor_groups.grid.as_mut().expect("several groups have a grid");
        // `applyLayout` takes the groups it drops out of the grid before their editors merge.
        if grid.groups().contains(&group) {
            grid.remove_group(group, sizing);
        }
        if self.editors.other_groups.is_empty() {
            self.editor_groups.grid = None;
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
        cx.notify();
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
        if order.len() == 1 {
            self.editor_groups.grid = None;
        }
        let focus = if order.contains(&active) { active } else { last };
        self.focus_group(focus, window, cx);
    }

    /// The editor part: one card holding every group at its place in the grid, the separators and sashes between them.
    pub(super) fn editor_groups_part(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let part_bounds = self.editor_groups.part_bounds.clone();
        let measure = canvas(move |bounds, _, _| part_bounds.set(bounds.map(f32::from)), |_, _, _, _| ()).absolute().inset_0();
        let card = div()
            .flex_1()
            // A long breadcrumbs path must not widen the part past the window.
            .min_w_0()
            .relative()
            .border_1()
            .rounded(px(8.))
            .bg(self.color("editor.background"))
            .border_color(self.color("surface.border"))
            .child(measure);
        let sash_size = self.sash_size();
        let Some(grid) = self.editor_groups.grid.as_mut() else {
            if self.editors.active().is_none() {
                return card.flex().flex_col().items_center().justify_center().child(self.watermark(256.));
            }
            return card.flex().flex_col().child(self.text_editor_part(window, cx)).child(self.part_top_border());
        };
        let size = self.editor_groups.part_bounds.get().size;
        grid.layout(size.width, size.height);
        let places = grid.bounds();
        let sashes = self.editor_groups.grid.as_ref().expect("checked above").sashes(sash_size);
        let separator = self.theme.color("editorGroup.border").or(self.theme.color("contrastBorder")).unwrap_or(gpui::transparent_black().into());
        let active = self.editors.id;
        let mut groups = Vec::new();
        for (group, rect) in places {
            let body = match group == active {
                true => self.editor_group_body(window, cx),
                false => self.inactive_group_body(group, window, cx),
            };
            groups.push(div().absolute().left(px(rect.x)).top(px(rect.y)).w(px(rect.width)).h(px(rect.height)).overflow_hidden().flex().flex_col().child(body));
        }
        let separators = sashes.iter().map(|sash| {
            let line = div().absolute().bg(separator);
            match sash.orientation {
                Orientation::Horizontal => line.left(px(sash.rect.x + sash.rect.width / 2. - 0.5)).top(px(sash.rect.y)).w(px(1.)).h(px(sash.rect.height)),
                Orientation::Vertical => line.top(px(sash.rect.y + sash.rect.height / 2. - 0.5)).left(px(sash.rect.x)).h(px(1.)).w(px(sash.rect.width)),
            }
        });
        let separators: Vec<Div> = separators.collect();
        let sash_elements: Vec<AnyElement> = sashes.into_iter().map(|sash| self.group_sash(sash, cx)).collect();
        let dragging = self.editor_groups.grid.as_ref().is_some_and(|grid| grid.dragged_sash().is_some());
        card.children(groups).children(separators).children(sash_elements).when(dragging, |card| card.child(self.group_sash_drag(cx))).child(self.part_top_border())
    }

    /// A group's tabs and editor, or the watermark of an empty group (editorgroupview.css: at most 200px in a part of
    /// several groups, without the shortcuts).
    fn editor_group_body(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        match self.editors.active() {
            Some(_) => self.text_editor_part(window, cx),
            None => div().size_full().flex().flex_col().items_center().justify_center().child(self.watermark(200.)),
        }
    }

    /// Another group, drawn while swapped in. A press in it activates it before the press reaches its editor;
    /// moves and the wheel over it go to it alone (editorGroupView.ts focuses the group on mousedown).
    fn inactive_group_body(&mut self, group: u32, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let body = self.with_group(group, |workbench| {
            workbench.editor_groups.drawing_inactive = true;
            let body = workbench.editor_group_body(window, cx);
            workbench.editor_groups.drawing_inactive = false;
            body
        });
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

#[cfg(test)]
mod tests;
