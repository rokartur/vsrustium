//! The Explorer's views as collapsible panes (paneview.ts `PaneView`, viewPaneContainer.ts `ViewPaneContainer`,
//! viewContainerModel.ts state): headers that expand and collapse with the mouse and keys, sashes between them,
//! headers dragged onto another pane to reorder, and the state in `workbench.explorer.views.state`.

use std::collections::HashMap;

use gpui::{
    AnyElement, AppContext, Context, Div, DragMoveEvent, FocusHandle, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Render, Rgba,
    StatefulInteractiveElement, Styled, Task, Window, canvas, div, prelude::FluentBuilder, px, svg,
};
use serde_json::{Value, json};

use super::Workbench;
use crate::split_view::{SashState, SplitView, View};

/// `--vscode-workbench-paneHeaderHeight` of the modern layout.
pub const HEADER_SIZE: f32 = 28.;
/// paneview.ts: a vertical pane's default `minimumBodySize`.
const MINIMUM_BODY_SIZE: f32 = 120.;
/// explorerViewlet.ts `storageId`: the views' collapsed state and sizes in workspace storage, their order with
/// `.hidden` in profile storage (viewContainerModel.ts `getViewsStateStorageId`).
const STATE_KEY: &str = "workbench.explorer.views.state";
pub const FILE_VIEW: &str = "workbench.explorer.fileView";
pub const EMPTY_VIEW: &str = "workbench.explorer.emptyView";
const OUTLINE_VIEW: &str = "outline";
const TIMELINE_VIEW: &str = "timeline";

/// The panes of a `PaneView` in a split view: a collapsed pane is its header, an expanded one at least the header
/// and `MINIMUM_BODY_SIZE`.
pub struct PaneView {
    ids: Vec<&'static str>,
    expanded: Vec<bool>,
    /// `Pane.expandedSize`: the size an expanded pane last laid out at, which it comes back at.
    expanded_sizes: Vec<Option<f32>>,
    split: SplitView,
}

fn size_limits(expanded: bool) -> (f32, f32) {
    match expanded {
        true => (HEADER_SIZE + MINIMUM_BODY_SIZE, f32::INFINITY),
        false => (HEADER_SIZE, HEADER_SIZE),
    }
}

impl PaneView {
    /// `addPanes` at their minimum sizes, the first layout, then `restoreViewSizes`: each pane at its stored size,
    /// else an equal share (`computeInitialSizes` with the default weight of 20).
    pub fn new(height: f32, panes: &[(&'static str, bool, Option<f32>)]) -> Self {
        let views = panes.iter().map(|&(_, expanded, _)| {
            let (minimum, maximum) = size_limits(expanded);
            View::new(minimum, minimum, maximum)
        });
        let mut split = SplitView::new(height, views.collect());
        split.layout(height);
        let share = height / panes.len() as f32;
        for (index, &(_, _, size)) in panes.iter().enumerate() {
            split.resize_view(index, size.unwrap_or(share));
        }
        let mut pane_view = PaneView {
            ids: panes.iter().map(|&(id, _, _)| id).collect(),
            expanded: panes.iter().map(|&(_, expanded, _)| expanded).collect(),
            expanded_sizes: vec![None; panes.len()],
            split,
        };
        pane_view.remember_expanded_sizes();
        pane_view
    }

    pub fn ids(&self) -> &[&'static str] {
        &self.ids
    }

    pub fn is_expanded(&self, index: usize) -> bool {
        self.expanded[index]
    }

    pub fn sizes(&self) -> Vec<f32> {
        self.split.sizes()
    }

    pub fn layout(&mut self, height: f32) {
        if height != self.split.size() {
            self.split.layout(height);
            self.remember_expanded_sizes();
        }
    }

    /// `Pane.setExpanded` and the split view's `onViewChange`: new bounds, then the expanded size or the current
    /// one clamped to them. Returns whether the state changed.
    pub fn set_expanded(&mut self, index: usize, expanded: bool) -> bool {
        if self.expanded[index] == expanded {
            return false;
        }
        self.expanded[index] = expanded;
        let (minimum, maximum) = size_limits(expanded);
        self.split.set_limits(index, minimum, maximum);
        let current = self.split.sizes()[index];
        let size = if expanded { self.expanded_sizes[index].unwrap_or(current) } else { current };
        self.split.resize_view(index, size);
        self.remember_expanded_sizes();
        true
    }

    /// `movePane`: the pane keeps its size at its new index.
    pub fn move_pane(&mut self, from: usize, to: usize) {
        let id = self.ids.remove(from);
        self.ids.insert(to, id);
        let expanded = self.expanded.remove(from);
        self.expanded.insert(to, expanded);
        let size = self.expanded_sizes.remove(from);
        self.expanded_sizes.insert(to, size);
        self.split.move_view(from, to);
    }

    pub fn sash_states(&self) -> Vec<SashState> {
        self.split.sash_states()
    }

    pub fn start_sash_drag(&mut self, index: usize, position: f32, alt: bool) {
        self.split.start_sash_drag(index, position, alt);
    }

    pub fn drag_sash(&mut self, position: f32, alt: bool) {
        self.split.drag_sash(position, alt);
        self.remember_expanded_sizes();
    }

    pub fn end_sash_drag(&mut self) {
        self.split.end_sash_drag();
    }

    pub fn dragged_sash(&self) -> Option<usize> {
        self.split.dragged_sash()
    }

    fn remember_expanded_sizes(&mut self) {
        let sizes = self.split.sizes();
        for (index, size) in sizes.into_iter().enumerate() {
            if self.expanded[index] {
                self.expanded_sizes[index] = Some(size);
            }
        }
    }
}

/// `ViewPaneDropOverlay.positionOverlay`: dropping on a pane's lower half puts the dragged pane after it.
fn drop_index(from: usize, to: usize, below: bool, count: usize) -> Option<usize> {
    let to = match below {
        true if from > to => to + 1,
        false if from < to => to - 1,
        _ => to,
    };
    (to < count && to != from).then_some(to)
}

#[derive(Default)]
pub(super) struct ExplorerPanes {
    view: Option<PaneView>,
    focus: HashMap<&'static str, FocusHandle>,
    /// Sizes stored after a sash drag (`setSizes`); panes without one take an equal share when laid out.
    stored_sizes: HashMap<String, f32>,
    hovered_sash: Option<usize>,
    sash_hover: Option<(usize, Task<()>)>,
    /// The pane a header is dragged over, and whether over its lower half.
    drop: Option<(&'static str, bool)>,
}

/// A view's body in the pane view: a header title and its content.
pub(super) struct ExplorerPane {
    pub id: &'static str,
    pub title: String,
    /// `ViewPane.titleDescription`, after the title.
    pub description: Option<String>,
    /// The `view/title` actions, shown while the expanded pane is hovered (paneview.css).
    pub actions: Option<AnyElement>,
    pub body: AnyElement,
}

/// The header a pane drag carries, drawn as the drag image (`setDragImage(header)`).
#[derive(Clone)]
pub(super) struct DraggedPane {
    id: &'static str,
    title: String,
    chevron: String,
    width: f32,
    background: Rgba,
    foreground: Rgba,
}

impl Render for DraggedPane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(self.width))
            .h(px(HEADER_SIZE))
            .pl(px(4.))
            .flex()
            .items_center()
            .rounded(px(4.))
            .bg(self.background)
            .text_color(self.foreground)
            .child(div().size(px(16.)).mx(px(2.)).flex().items_center().justify_center().child(svg().external_path(self.chevron.clone()).size(px(16.)).text_color(self.foreground)))
            .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).child(self.title.clone()))
    }
}

impl Workbench {
    /// The Explorer's views by id, the first one the folder's (or the empty window's) view: viewContainerModel.ts
    /// order (stored, else declared) and collapsed state (stored, else the descriptor's `collapsed`).
    pub(super) fn layout_explorer_panes(&mut self, side_bar_height: f32, cx: &mut Context<Self>) {
        // The card's borders, the composite title, the compact activity bar, and the pane view's 5px less.
        let compact = if self.compact_activity_bar_location().is_some() { 32. } else { 0. };
        let height = side_bar_height - 2. - 32. - compact - 5.;
        let first = if self.folder.is_some() { FILE_VIEW } else { EMPTY_VIEW };
        let contributed = self.shown_tree_views();
        let ids: Vec<&'static str> = [first, OUTLINE_VIEW, TIMELINE_VIEW].into_iter().chain(contributed.iter().copied()).collect();
        for &id in &ids {
            self.explorer_panes.focus.entry(id).or_insert_with(|| cx.focus_handle());
        }
        let laid_out = self.explorer_panes.view.as_ref().is_some_and(|view| view.ids().len() == ids.len() && ids.iter().all(|id| view.ids().contains(id)));
        if laid_out {
            self.explorer_panes.view.as_mut().expect("checked above").layout(height);
            return;
        }
        let workspace_state = self.workspace_storage.lock().unwrap().get(STATE_KEY).and_then(|raw| serde_json::from_str::<Value>(raw).ok()).unwrap_or_default();
        let global_state = self.profile_storage.lock().unwrap().get(&format!("{STATE_KEY}.hidden")).and_then(|raw| serde_json::from_str::<Value>(raw).ok()).unwrap_or_default();
        let order = |id: &str| global_state.as_array().and_then(|views| views.iter().find(|view| view["id"] == id)).and_then(|view| view["order"].as_f64());
        // viewsExtensionPoint.ts `showCollapsed`: an extension's view in the Explorer starts collapsed.
        let mut descriptors: Vec<(&'static str, bool)> = [(first, false), (OUTLINE_VIEW, true), (TIMELINE_VIEW, true)].into_iter().chain(contributed.into_iter().map(|id| (id, true))).collect();
        descriptors.sort_by(|a, b| order(a.0).unwrap_or(f64::MAX).total_cmp(&order(b.0).unwrap_or(f64::MAX)));
        self.explorer_panes.stored_sizes.clear();
        let panes: Vec<_> = descriptors
            .into_iter()
            .map(|(id, collapsed_by_default)| {
                let state = &workspace_state[id];
                let size = state["size"].as_f64().map(|size| size as f32);
                if let Some(size) = size {
                    self.explorer_panes.stored_sizes.insert(id.to_string(), size);
                }
                (id, !state["collapsed"].as_bool().unwrap_or(collapsed_by_default), size)
            })
            .collect();
        self.explorer_panes.view = Some(PaneView::new(height, &panes));
    }

    /// viewContainerModel.ts `updateState`: collapsed and size per view in the workspace, order in the profile.
    fn store_explorer_panes(&self) {
        let view = self.explorer_panes.view.as_ref().expect("stored after a layout");
        let mut workspace = serde_json::Map::new();
        let mut global = Vec::new();
        for (index, &id) in view.ids().iter().enumerate() {
            let mut state = json!({ "collapsed": !view.is_expanded(index), "isHidden": false });
            if let Some(size) = self.explorer_panes.stored_sizes.get(id) {
                state["size"] = json!(size);
            }
            workspace.insert(id.to_string(), state);
            global.push(json!({ "id": id, "isHidden": false, "order": index }));
        }
        self.workspace_storage.lock().unwrap().set(STATE_KEY, Value::Object(workspace).to_string());
        self.profile_storage.lock().unwrap().set(&format!("{STATE_KEY}.hidden"), Value::Array(global).to_string());
    }

    pub(super) fn explorer_pane_expanded(&self, id: &str) -> bool {
        let Some(view) = self.explorer_panes.view.as_ref() else { return false };
        view.ids().iter().position(|&pane| pane == id).is_some_and(|index| view.is_expanded(index))
    }

    /// `viewsService.openView`: expands the view's pane once it is laid out.
    pub(super) fn expand_explorer_pane(&mut self, id: &str, cx: &mut Context<Self>) {
        let index = self.explorer_panes.view.as_ref().and_then(|view| view.ids().iter().position(|&pane| pane == id));
        if let Some(index) = index {
            self.set_pane_expanded(index, true, cx);
        }
    }

    fn set_pane_expanded(&mut self, index: usize, expanded: bool, cx: &mut Context<Self>) {
        let view = self.explorer_panes.view.as_mut().expect("panes are laid out before they draw");
        if view.set_expanded(index, expanded) {
            self.store_explorer_panes();
            cx.notify();
        }
    }

    fn drop_pane(&mut self, dragged: &'static str, target: &'static str, below: bool, cx: &mut Context<Self>) {
        self.explorer_panes.drop = None;
        let view = self.explorer_panes.view.as_mut().expect("panes are laid out before they draw");
        let (Some(from), Some(to)) = (view.ids().iter().position(|&id| id == dragged), view.ids().iter().position(|&id| id == target)) else { return };
        if let Some(to) = drop_index(from, to, below, view.ids().len()) {
            view.move_pane(from, to);
            self.store_explorer_panes();
        }
        cx.notify();
    }

    /// The pane view of the Explorer, laid out by `layout_explorer_panes`: `first` (the folder's or the empty
    /// window's view), then Outline and Timeline in their stored order.
    pub(super) fn explorer_pane_view(&self, first: ExplorerPane, window: &Window, cx: &mut Context<Self>) -> Div {
        let mut panes = vec![
            first,
            ExplorerPane { id: OUTLINE_VIEW, title: "Outline".into(), description: None, actions: Some(self.outline_title_actions(window, cx).into_any_element()), body: self.outline_pane_body(window, cx) },
            self.timeline_pane(window, cx),
        ];
        let view = self.explorer_panes.view.as_ref().expect("panes are laid out before they draw");
        let (ids, sizes, sash_states) = (view.ids().to_vec(), view.sizes(), view.sash_states());
        let expanded: Vec<bool> = (0..ids.len()).map(|index| view.is_expanded(index)).collect();
        let dragging_sash = view.dragged_sash().is_some();
        let mut elements = Vec::new();
        for (index, &id) in ids.iter().enumerate() {
            let pane = match panes.iter().position(|pane| pane.id == id) {
                Some(position) => panes.remove(position),
                None => self.tree_view_pane(id, window, cx),
            };
            let header = self.pane_header(index, id, &pane.title, pane.description, pane.actions, expanded[index], ids.len(), window, cx);
            elements.push(self.pane(id, sizes[index], header, expanded[index].then_some(pane.body), cx));
        }
        let mut top = 0.;
        let mut sashes = Vec::new();
        for (index, state) in sash_states.into_iter().enumerate() {
            top += sizes[index];
            sashes.push(self.pane_sash(index, state, top, cx));
        }
        div()
            .relative()
            .h(px(sizes.iter().sum()))
            .flex_none()
            .flex()
            .flex_col()
            .children(elements)
            .children(sashes)
            .when(dragging_sash, |view| view.child(self.pane_sash_drag(cx)))
    }

    /// `.pane` with its body under the header while expanded, and the drop overlay of a dragged header.
    fn pane(&self, id: &'static str, size: f32, header: Div, body: Option<AnyElement>, cx: &mut Context<Self>) -> AnyElement {
        let drop = self.explorer_panes.drop.filter(|(target, _)| *target == id && cx.has_active_drag()).map(|(_, below)| below);
        let overlay = self.color("sideBar.dropBackground");
        let outline = self.theme.color("contrastActiveBorder");
        div()
            .id(id)
            .group(id)
            .relative()
            .h(px(size))
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(header)
            .when_some(body, |pane, body| pane.child(div().flex_1().min_h_0().flex().flex_col().child(body)))
            .on_drag_move(cx.listener(move |workbench, event: &DragMoveEvent<DraggedPane>, _, cx| {
                if !event.bounds.contains(&event.event.position) {
                    return;
                }
                let dragged = event.drag(cx).id;
                let below = event.event.position.y >= event.bounds.center().y;
                let drop = (dragged != id).then_some((id, below));
                if workbench.explorer_panes.drop != drop {
                    workbench.explorer_panes.drop = drop;
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(move |workbench, dragged: &DraggedPane, _, cx| {
                let below = workbench.explorer_panes.drop.filter(|(target, _)| *target == id).map(|(_, below)| below);
                match below {
                    Some(below) => workbench.drop_pane(dragged.id, id, below, cx),
                    None => workbench.explorer_panes.drop = None,
                }
            }))
            .when_some(drop, |pane, below| {
                let half = div().absolute().left_0().right_0().h(gpui::relative(0.5)).bg(overlay);
                let half = if below { half.bottom_0() } else { half.top_0() };
                pane.child(half.when_some(outline, |half, color| half.border_2().border_dashed().border_color(color)))
            })
            .into_any_element()
    }

    /// `.pane-header`: a click, Enter or Space toggles, Left collapses, Right expands, Up and Down move to the
    /// neighbouring header; dragged onto another pane it moves this one.
    #[allow(clippy::too_many_arguments)]
    fn pane_header(&self, index: usize, id: &'static str, title: &str, description: Option<String>, actions: Option<AnyElement>, expanded: bool, count: usize, window: &Window, cx: &mut Context<Self>) -> Div {
        let focus = &self.explorer_panes.focus[id];
        let focused = focus.is_focused(window);
        let foreground = self.foreground_or("sideBarSectionHeader.foreground");
        let background = self.color("sideBar.background");
        let border = self.theme.color("sideBarSectionHeader.border").or(self.theme.color("surface.border")).filter(|_| index > 0);
        let chevron = if expanded { "chevron-down" } else { "chevron-right" };
        let dragged = DraggedPane {
            id,
            title: title.to_string(),
            chevron: self.app_root.join("node_modules/@vscode/codicons/src/icons").join(format!("{chevron}.svg")).to_string_lossy().into_owned(),
            // The side bar part less the card's borders and the header's margins.
            width: self.part_grid.grid.as_ref().map_or(0., |grid| grid.row_sizes()[1]) - 10.,
            background,
            foreground,
        };
        div().flex_none().child(
            div()
                .id(("pane-header", index))
                .track_focus(focus)
                .relative()
                .h(px(HEADER_SIZE))
                // paneview.css: .pane-header is cursor: pointer.
                .cursor_pointer()
                .mx(px(4.))
                .pl(px(4.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .bg(background)
                .hover(|header| header.bg(self.color("list.hoverBackground")))
                .text_color(foreground)
                .when_some(border, |header, border| header.child(div().absolute().top_0().left(px(4.)).right(px(4.)).h(px(1.)).bg(border)))
                .child(self.icon(chevron, 16., 16., foreground).mx(px(2.)))
                .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
                // paneviewlet.css `.pane-header .description`: 10px after the title, normal weight, only while expanded.
                .children(description.filter(|_| expanded).map(|description| {
                    div().ml(px(10.)).text_size(px(12.)).text_color(self.color("panelTitle.inactiveForeground")).overflow_hidden().whitespace_nowrap().child(description)
                }))
                .children(actions.filter(|_| expanded).map(|actions| div().ml_auto().mr(px(4.)).invisible().group_hover(id, |actions| actions.visible()).when(focused, |actions| actions.visible()).child(actions)))
                .when(id == FILE_VIEW, |header| header.children(self.explorer_title_actions(window, cx)))
                .when(focused, |header| header.child(div().absolute().inset_0().rounded(px(4.)).border_1().border_color(self.color("focusBorder"))))
                .on_click(cx.listener(move |workbench, _, window, cx| {
                    let focus = workbench.explorer_panes.focus[id].clone();
                    window.focus(&focus, cx);
                    workbench.set_pane_expanded(index, !expanded, cx);
                }))
                .on_key_down(cx.listener(move |workbench, event: &KeyDownEvent, window, cx| {
                    let neighbour = match event.keystroke.key.as_str() {
                        // Enter and Space reach `on_click`: gpui clicks a focused element on its activation keys.
                        "left" => return workbench.set_pane_expanded(index, false, cx),
                        "right" => return workbench.set_pane_expanded(index, true, cx),
                        "up" if index > 0 => index - 1,
                        "down" if index + 1 < count => index + 1,
                        _ => return,
                    };
                    let ids = workbench.explorer_panes.view.as_ref().expect("panes are laid out before they draw").ids();
                    let focus = workbench.explorer_panes.focus.get(ids[neighbour]).cloned();
                    if let Some(focus) = focus {
                        window.focus(&focus, cx);
                        cx.notify();
                    }
                }))
                .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone())),
        )
    }

    /// A horizontal `.monaco-sash` between two panes (sash.css): 4px centered on the boundary.
    fn pane_sash(&self, index: usize, state: SashState, boundary: f32, cx: &mut Context<Self>) -> AnyElement {
        if state == SashState::Disabled {
            return div().into_any_element();
        }
        let dragged = self.explorer_panes.view.as_ref().and_then(PaneView::dragged_sash);
        let active = self.explorer_panes.hovered_sash == Some(index) || dragged == Some(index);
        let hover_border = self.theme.color("sash.hoverBorder").unwrap_or(self.color("focusBorder"));
        let cursor = match state {
            SashState::AtMinimum => gpui::CursorStyle::ResizeDown,
            SashState::AtMaximum => gpui::CursorStyle::ResizeUp,
            _ => gpui::CursorStyle::ResizeRow,
        };
        div()
            .id(("pane-sash", index))
            .absolute()
            .left_0()
            .right_0()
            .top(px(boundary - 2.))
            .h(px(4.))
            .cursor(cursor)
            .when(active, |sash| sash.bg(hover_border))
            .on_hover(cx.listener(move |workbench, hovered: &bool, window, cx| {
                let panes = &mut workbench.explorer_panes;
                if !*hovered {
                    if panes.hovered_sash == Some(index) {
                        panes.hovered_sash = None;
                    }
                    if panes.sash_hover.as_ref().is_some_and(|(pending, _)| *pending == index) {
                        panes.sash_hover = None;
                    }
                    cx.notify();
                    return;
                }
                panes.hovered_sash = None;
                let delay = workbench.sash_hover_delay();
                workbench.explorer_panes.sash_hover = Some((index, cx.spawn_in(window, async move |workbench, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = workbench.update(cx, |workbench, cx| {
                        workbench.explorer_panes.hovered_sash = Some(index);
                        cx.notify();
                    });
                })));
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |workbench, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let view = workbench.explorer_panes.view.as_mut().expect("sashes are drawn with the panes");
                    view.start_sash_drag(index, f32::from(event.position.y), event.modifiers.alt);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// Follows the pointer anywhere in the window while a pane sash drags; the sizes are stored when it ends.
    fn pane_sash_drag(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let workbench = cx.entity();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
                    moved.update(cx, |workbench, cx| {
                        let view = workbench.explorer_panes.view.as_mut().expect("a sash drags in the panes");
                        view.drag_sash(f32::from(event.position.y), event.modifiers.alt);
                        cx.notify();
                    });
                });
                let released = workbench.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                    released.update(cx, |workbench, cx| {
                        let view = workbench.explorer_panes.view.as_mut().expect("a sash drags in the panes");
                        view.end_sash_drag();
                        let sizes: Vec<(String, f32)> = view.ids().iter().map(|id| id.to_string()).zip(view.sizes()).collect();
                        workbench.explorer_panes.stored_sizes = sizes.into_iter().collect();
                        workbench.store_explorer_panes();
                        cx.notify();
                    });
                });
            },
        )
        .size_0()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPLORER: [(&str, bool, Option<f32>); 3] = [(FILE_VIEW, true, None), (OUTLINE_VIEW, false, None), (TIMELINE_VIEW, false, None)];

    #[test]
    fn collapsed_panes_are_their_headers_and_the_folder_takes_the_rest() {
        let view = PaneView::new(662., &EXPLORER);
        assert_eq!(view.sizes(), vec![606., 28., 28.]);
    }

    #[test]
    fn expanding_outline_and_timeline_gives_each_its_header_and_minimum_body_as_the_reference_does() {
        let mut view = PaneView::new(662., &EXPLORER);
        assert!(view.set_expanded(1, true));
        assert!(view.set_expanded(2, true));
        assert_eq!(view.sizes(), vec![366., 148., 148.]);
        assert!(!view.set_expanded(2, true));
    }

    #[test]
    fn a_collapsed_pane_comes_back_at_its_expanded_size() {
        let mut view = PaneView::new(662., &EXPLORER);
        view.set_expanded(1, true);
        view.start_sash_drag(0, 486., false);
        view.drag_sash(406., false);
        view.end_sash_drag();
        assert_eq!(view.sizes(), vec![406., 228., 28.]);
        view.set_expanded(1, false);
        assert_eq!(view.sizes().iter().sum::<f32>(), 662.);
        assert_eq!(view.sizes()[1], 28.);
        view.set_expanded(1, true);
        assert_eq!(view.sizes()[1], 228.);
    }

    #[test]
    fn stored_sizes_restore_and_moved_panes_keep_their_sizes() {
        let mut view = PaneView::new(662., &[(FILE_VIEW, true, Some(300.)), (OUTLINE_VIEW, true, Some(334.)), (TIMELINE_VIEW, false, Some(28.))]);
        assert_eq!(view.sizes(), vec![300., 334., 28.]);
        view.move_pane(2, 0);
        assert_eq!(view.ids(), [TIMELINE_VIEW, FILE_VIEW, OUTLINE_VIEW]);
        assert_eq!(view.sizes(), vec![28., 300., 334.]);
        assert!(!view.is_expanded(0));
    }

    #[test]
    fn a_drop_on_the_lower_half_puts_the_pane_after_the_target() {
        assert_eq!(drop_index(2, 0, true, 3), Some(1));
        assert_eq!(drop_index(2, 0, false, 3), Some(0));
        assert_eq!(drop_index(0, 2, true, 3), Some(2));
        assert_eq!(drop_index(0, 2, false, 3), Some(1));
        assert_eq!(drop_index(0, 1, false, 3), None);
        assert_eq!(drop_index(1, 0, true, 3), None);
    }
}
