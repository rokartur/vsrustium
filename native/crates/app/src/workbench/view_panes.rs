//! The views of the Explorer and Source Control containers as collapsible panes (paneview.ts `PaneView`,
//! viewPaneContainer.ts `ViewPaneContainer`, viewContainerModel.ts state): headers that expand and collapse with the
//! mouse and keys, sashes between them, headers dragged onto another pane to reorder, a single view merged into the
//! container (`mergeViewWithContainerWhenSingleView`), and the state in the container's `storageId`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, AppContext, Bounds, Context, Div, DragMoveEvent, FocusHandle, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, Rgba,
    StatefulInteractiveElement, Styled, Task, Window, canvas, div, prelude::FluentBuilder, px, svg,
};
use serde_json::{Value, json};

use super::Workbench;
use super::view_locations::{EXPLORER_CONTAINER, SCM_CONTAINER};
use super::view_visibility::container_storage_id;
use crate::split_view::{SashState, SplitView, View};

/// `--vscode-workbench-paneHeaderHeight` of the modern layout.
pub const HEADER_SIZE: f32 = 28.;
/// paneview.ts: a vertical pane's default `minimumBodySize`.
const MINIMUM_BODY_SIZE: f32 = 120.;
/// viewPaneContainer.ts `layout`: the modern layout's gap below the last pane, `FLOATING_PANEL_MARGIN` and the
/// part's 1px border.
const BOTTOM_GAP: f32 = 5.;
pub const FILE_VIEW: &str = "workbench.explorer.fileView";
pub const EMPTY_VIEW: &str = "workbench.explorer.emptyView";
pub(super) const OUTLINE_VIEW: &str = "outline";
pub(super) const TIMELINE_VIEW: &str = "timeline";
pub(super) const OPEN_EDITORS_VIEW: &str = "workbench.explorer.openEditorsView";
/// scm.contribution.ts `VIEW_PANE_ID` and `HISTORY_VIEW_PANE_ID`.
pub(super) const SCM_CHANGES_VIEW: &str = "workbench.scm";
pub(super) const SCM_HISTORY_VIEW: &str = "workbench.scm.history";

/// A view container drawn as a `ViewPaneContainer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaneContainer {
    Explorer,
    Scm,
    /// A container generated for moved views (view_locations.rs), by its id.
    Generated(&'static str),
}

impl PaneContainer {
    /// The view container key of `contributes.views` and view_visibility.rs.
    pub(super) fn key(self) -> &'static str {
        match self {
            PaneContainer::Explorer => "explorer",
            PaneContainer::Scm => "scm",
            PaneContainer::Generated(id) => id,
        }
    }

    pub(super) fn container_id(self) -> &'static str {
        match self {
            PaneContainer::Explorer => EXPLORER_CONTAINER,
            PaneContainer::Scm => SCM_CONTAINER,
            PaneContainer::Generated(id) => id,
        }
    }
}

/// The panes of a `PaneView` in a split view: a collapsed pane is its header, an expanded one at least the header
/// and `MINIMUM_BODY_SIZE`.
pub struct PaneView {
    ids: Vec<&'static str>,
    expanded: Vec<bool>,
    /// `Pane.expandedSize`: the size an expanded pane last laid out at, which it comes back at.
    expanded_sizes: Vec<Option<f32>>,
    /// `minimumBodySize` and `maximumBodySize` per pane.
    body_limits: Vec<(f32, f32)>,
    split: SplitView,
}

const DEFAULT_BODY_LIMITS: (f32, f32) = (MINIMUM_BODY_SIZE, f32::INFINITY);

fn size_limits(expanded: bool, (minimum, maximum): (f32, f32)) -> (f32, f32) {
    match expanded {
        true => (HEADER_SIZE + minimum, HEADER_SIZE + maximum),
        false => (HEADER_SIZE, HEADER_SIZE),
    }
}

impl PaneView {
    /// `addPanes` at their minimum sizes, the first layout, then `restoreViewSizes`: each pane at its stored size,
    /// else an equal share (`computeInitialSizes` with the default weight of 20) of the container's height, which
    /// is the pane view's and its bottom gap.
    pub fn new(height: f32, panes: &[(&'static str, bool, Option<f32>)]) -> Self {
        let views = panes.iter().map(|&(_, expanded, _)| {
            let (minimum, maximum) = size_limits(expanded, DEFAULT_BODY_LIMITS);
            View::new(minimum, minimum, maximum)
        });
        let mut split = SplitView::new(height, views.collect());
        split.layout(height);
        let share = (height + BOTTOM_GAP) / panes.len() as f32;
        for (index, &(_, _, size)) in panes.iter().enumerate() {
            split.resize_view(index, size.unwrap_or(share));
        }
        let mut pane_view = PaneView {
            ids: panes.iter().map(|&(id, _, _)| id).collect(),
            expanded: panes.iter().map(|&(_, expanded, _)| expanded).collect(),
            expanded_sizes: vec![None; panes.len()],
            body_limits: vec![DEFAULT_BODY_LIMITS; panes.len()],
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

    pub fn height(&self) -> f32 {
        self.split.size()
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
        let (minimum, maximum) = size_limits(expanded, self.body_limits[index]);
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
        let limits = self.body_limits.remove(from);
        self.body_limits.insert(to, limits);
        self.split.move_view(from, to);
    }

    /// The `minimumBodySize` and `maximumBodySize` setters: `onDidChange`, then the split view's `onViewChange`
    /// clamps the pane's size to its new bounds.
    pub fn set_body_limits(&mut self, index: usize, minimum: f32, maximum: f32) {
        if self.body_limits[index] == (minimum, maximum) {
            return;
        }
        self.body_limits[index] = (minimum, maximum);
        let (pane_minimum, pane_maximum) = size_limits(self.expanded[index], (minimum, maximum));
        self.split.set_limits(index, pane_minimum, pane_maximum);
        self.split.resize_view(index, self.split.sizes()[index]);
        self.remember_expanded_sizes();
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
pub(super) struct ContainerPanes {
    pub(super) view: Option<PaneView>,
    focus: HashMap<&'static str, FocusHandle>,
    /// Sizes stored after a sash drag (`setSizes`); panes without one take an equal share when laid out.
    stored_sizes: HashMap<String, f32>,
    hovered_sash: Option<usize>,
    sash_hover: Option<(usize, Task<()>)>,
    /// The pane a header is dragged over, and whether over its lower half.
    drop: Option<(&'static str, bool)>,
    /// The header a pointer focused: paneHeaders.css `:focus:not(:focus-visible)` draws no outline until a key.
    pointer_focused: Option<&'static str>,
    /// Each pane's bounds in the last frame. `.pane:hover` tests the pointer against them: GPUI's `is_hovered` turns
    /// false after a key, a browser's `:hover` does not (the header actions under an open quick pick).
    bounds: Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>,
}

/// A view's body in the pane view: a header title and its content.
pub(super) struct ViewPane {
    pub id: &'static str,
    pub title: String,
    /// `ViewPane.titleDescription`, after the title.
    pub description: Option<String>,
    /// The `view/title` actions, shown while the expanded pane is hovered (paneview.css).
    pub actions: Option<AnyElement>,
    /// Whether the expanded pane's actions show without a hover: focus in its body (paneview.css `:focus-within`) or
    /// `ViewPaneShowActions.WhenExpanded`.
    pub show_actions: bool,
    pub body: AnyElement,
}

/// The header a pane drag carries, drawn as the drag image (`setDragImage(header)`).
#[derive(Clone)]
pub(super) struct DraggedPane {
    container: PaneContainer,
    pub(super) id: &'static str,
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
    pub(super) fn panes(&self, container: PaneContainer) -> &ContainerPanes {
        match container {
            PaneContainer::Explorer => &self.explorer_panes,
            PaneContainer::Scm => &self.scm_panes,
            PaneContainer::Generated(id) => self.generated_panes.get(id).expect("a generated container is laid out before use"),
        }
    }

    pub(super) fn panes_mut(&mut self, container: PaneContainer) -> &mut ContainerPanes {
        match container {
            PaneContainer::Explorer => &mut self.explorer_panes,
            PaneContainer::Scm => &mut self.scm_panes,
            PaneContainer::Generated(id) => self.generated_panes.entry(id).or_default(),
        }
    }

    /// The containers' pane views at the side bar's height: the card's borders, the composite title, the compact
    /// activity bar, and the pane view's 5px less.
    pub(super) fn layout_view_panes(&mut self, side_bar_height: f32, cx: &mut Context<Self>) {
        let compact = if self.compact_activity_bar_location().is_some() { 32. } else { 0. };
        let height = side_bar_height - 2. - 32. - compact - BOTTOM_GAP;
        self.layout_container_panes(height, cx);
    }

    /// The containers' pane views at the pane view's `height`, with the views shown now.
    pub(super) fn layout_container_panes(&mut self, height: f32, cx: &mut Context<Self>) {
        let first = if self.workspace().is_some_and(|workspace| !workspace.folders.is_empty()) { FILE_VIEW } else { EMPTY_VIEW };
        let built_in = [OPEN_EDITORS_VIEW, first, OUTLINE_VIEW, TIMELINE_VIEW].into_iter().filter(|&id| id == FILE_VIEW || !self.view_hidden("explorer", id));
        let built_in: Vec<_> = built_in.filter(|&id| id == first || id == OPEN_EDITORS_VIEW || self.views_in(EXPLORER_CONTAINER).iter().any(|view| view.id == id)).collect();
        // viewsExtensionPoint.ts `showCollapsed`: an extension's view in the Explorer or Source Control starts collapsed.
        let mut explorer: Vec<_> = built_in.into_iter().map(|id| (id, id != first && id != OPEN_EDITORS_VIEW)).chain(self.shown_tree_views().into_iter().map(|id| (id, true))).collect();
        explorer.extend(self.moved_views_in(EXPLORER_CONTAINER));
        self.layout_panes(PaneContainer::Explorer, height, explorer, cx);
        self.limit_open_editors_pane();
        if let Some(container) = self.side_bar_container().filter(|container| super::view_locations::is_generated_container(container)) {
            let views = self.moved_views_in(container);
            self.layout_panes(PaneContainer::Generated(container), height, views, cx);
        }
        let mut scm = Vec::new();
        if !self.view_hidden("scm", SCM_CHANGES_VIEW) {
            scm.push((SCM_CHANGES_VIEW, false));
        }
        // scm.contribution.ts: Graph shows while `scm.historyProviderCount` is not 0; the port draws the active repository's.
        if self.active_scm_provider().is_some_and(|provider| provider.has_history_provider) && !self.view_hidden("scm", SCM_HISTORY_VIEW) {
            scm.push((SCM_HISTORY_VIEW, false));
        }
        scm.extend(self.shown_views_of("scm").into_iter().map(|id| (id, true)));
        scm.extend(self.moved_views_in(SCM_CONTAINER));
        self.layout_panes(PaneContainer::Scm, height, scm, cx);
    }

    /// The shown views moved into the container from their own, expanded at first.
    pub(super) fn moved_views_in(&self, container: &str) -> Vec<(&'static str, bool)> {
        let views = self.views_in(container).into_iter().filter(|view| view.default_container != container && self.movable_view_shown(view));
        views.map(|view| (view.id, false)).collect()
    }

    /// The container's views by id with whether each starts collapsed: viewContainerModel.ts order (stored, else
    /// declared) and collapsed state (stored, else the descriptor's `collapsed`).
    pub(super) fn layout_panes(&mut self, container: PaneContainer, height: f32, mut descriptors: Vec<(&'static str, bool)>, cx: &mut Context<Self>) {
        let panes = self.panes_mut(container);
        for &(id, _) in &descriptors {
            panes.focus.entry(id).or_insert_with(|| cx.focus_handle());
        }
        let laid_out = panes.view.as_ref().is_some_and(|view| view.ids().len() == descriptors.len() && descriptors.iter().all(|(id, _)| view.ids().contains(id)));
        if laid_out {
            panes.view.as_mut().expect("checked above").layout(height);
            return;
        }
        let storage_id = container_storage_id(container.key());
        let workspace_state = self.workspace_storage.lock().unwrap().get(&storage_id).and_then(|raw| serde_json::from_str::<Value>(raw).ok()).unwrap_or_default();
        let global_state = self.views_visibility_state(container.key());
        let order = |id: &str| global_state.iter().find(|view| view["id"] == id).and_then(|view| view["order"].as_f64());
        descriptors.sort_by(|a, b| order(a.0).unwrap_or(f64::MAX).total_cmp(&order(b.0).unwrap_or(f64::MAX)));
        let panes = self.panes_mut(container);
        panes.stored_sizes.clear();
        let mut pane_states = Vec::new();
        for (id, collapsed_by_default) in descriptors {
            let state = &workspace_state[id];
            let size = state["size"].as_f64().map(|size| size as f32);
            if let Some(size) = size {
                panes.stored_sizes.insert(id.to_string(), size);
            }
            pane_states.push((id, !state["collapsed"].as_bool().unwrap_or(collapsed_by_default), size));
        }
        panes.view = Some(PaneView::new(height, &pane_states));
    }

    /// viewContainerModel.ts `updateState`: collapsed and size per view in the workspace, order in the profile.
    fn store_panes(&self, container: PaneContainer) {
        let panes = self.panes(container);
        let view = panes.view.as_ref().expect("stored after a layout");
        let mut workspace = serde_json::Map::new();
        // The hidden views keep their entries: `isHidden` is what keeps them out of the pane view.
        let mut global: Vec<Value> = self.views_visibility_state(container.key()).into_iter().filter(|entry| !view.ids().iter().any(|&id| entry["id"] == id)).collect();
        for (index, &id) in view.ids().iter().enumerate() {
            let mut state = json!({ "collapsed": !view.is_expanded(index), "isHidden": false });
            if let Some(size) = panes.stored_sizes.get(id) {
                state["size"] = json!(size);
            }
            workspace.insert(id.to_string(), state);
            global.push(json!({ "id": id, "isHidden": false, "order": index }));
        }
        let storage_id = container_storage_id(container.key());
        self.workspace_storage.lock().unwrap().set(&storage_id, Value::Object(workspace).to_string());
        self.profile_storage.lock().unwrap().set(&format!("{storage_id}.hidden"), Value::Array(global).to_string());
    }

    /// Whether the view's body shows: expanded, or merged into its container as its only view.
    pub(super) fn view_pane_expanded(&self, container: PaneContainer, id: &str) -> bool {
        if let PaneContainer::Generated(generated) = container && !self.generated_panes.contains_key(generated) {
            return false;
        }
        let Some(view) = self.panes(container).view.as_ref() else { return false };
        view.ids().iter().position(|&pane| pane == id).is_some_and(|index| view.ids().len() == 1 || view.is_expanded(index))
    }

    /// `viewsService.openView`: expands the view's pane once it is laid out.
    pub(super) fn expand_pane(&mut self, container: PaneContainer, id: &str, cx: &mut Context<Self>) {
        if let PaneContainer::Generated(generated) = container && !self.generated_panes.contains_key(generated) {
            return;
        }
        let index = self.panes(container).view.as_ref().and_then(|view| view.ids().iter().position(|&pane| pane == id));
        if let Some(index) = index {
            self.set_pane_expanded(container, index, true, cx);
        }
    }

    pub(super) fn set_pane_expanded(&mut self, container: PaneContainer, index: usize, expanded: bool, cx: &mut Context<Self>) {
        let view = self.panes_mut(container).view.as_mut().expect("panes are laid out before they draw");
        if view.set_expanded(index, expanded) {
            self.store_panes(container);
            self.sync_tree_views(cx);
            cx.notify();
        }
    }

    fn drop_pane(&mut self, container: PaneContainer, dragged: &'static str, target: &'static str, below: bool, cx: &mut Context<Self>) {
        let panes = self.panes_mut(container);
        panes.drop = None;
        let view = panes.view.as_mut().expect("panes are laid out before they draw");
        let (Some(from), Some(to)) = (view.ids().iter().position(|&id| id == dragged), view.ids().iter().position(|&id| id == target)) else { return };
        if let Some(to) = drop_index(from, to, below, view.ids().len()) {
            view.move_pane(from, to);
            self.store_panes(container);
        }
        cx.notify();
    }

    /// viewPaneContainer.ts `onDrop` of another container's view: only views the port can move.
    pub(super) fn can_move_view(&self, view: &str) -> bool {
        self.movable_views().iter().any(|movable| movable.id == view)
    }

    /// `moveViewsToContainer` at the drop position: the moved view takes the target's place in the stored order,
    /// before it or after it by the half it was dropped on.
    pub(super) fn drop_pane_from_container(&mut self, dragged: &'static str, container: PaneContainer, target: &'static str, below: bool, cx: &mut Context<Self>) {
        self.panes_mut(container).drop = None;
        let mut ids = self.panes(container).view.as_ref().expect("panes are laid out before they draw").ids().to_vec();
        let index = ids.iter().position(|&id| id == target).expect("the target is a pane") + usize::from(below);
        ids.insert(index, dragged);
        let key = format!("{}.hidden", container_storage_id(container.key()));
        let mut global: Vec<Value> = self.views_visibility_state(container.key()).into_iter().filter(|entry| !ids.iter().any(|&id| entry["id"] == id)).collect();
        global.extend(ids.iter().enumerate().map(|(order, id)| json!({ "id": id, "isHidden": false, "order": order })));
        self.profile_storage.lock().unwrap().set(&key, Value::Array(global).to_string());
        self.move_view_to_container(dragged, container.container_id(), cx);
    }

    /// A `ViewPaneContainer` in the side bar, laid out by `layout_view_panes`: the composite title, then the panes
    /// in their stored order; `panes` are the container's own views, a contributed one is a tree view. A single view
    /// merges into the container: no header, its title in the composite title (`getTitle`) beside its actions. The
    /// caller ends it with the pane view's 5px.
    pub(super) fn view_pane_container(&self, container: PaneContainer, title: &str, panes: Vec<ViewPane>, window: &Window, cx: &mut Context<Self>) -> Div {
        let ids = self.panes(container).view.as_ref().expect("panes are laid out before they draw").ids().to_vec();
        let mut ordered = self.ordered_panes(&ids, panes, window, cx);
        let frame = self.side_bar_frame().text_color(self.foreground_or("sideBar.foreground"));
        if let [id] = ids[..] {
            let pane = ordered.remove(0);
            let title = match pane.title.as_str() {
                _ if id == SCM_CHANGES_VIEW => "Source Control".to_string(),
                _ if id == SCM_HISTORY_VIEW => "Source Control Graph".to_string(),
                "" => title.to_string(),
                pane_title if pane_title == title => title.to_string(),
                pane_title => format!("{title}: {pane_title}"),
            };
            // shortcut: a view with its own actions keeps only its "..." (panecomposite.ts puts the Views submenu before
            // the view's secondary actions); merge the menus when a merged view's Views submenu has an enabled toggle.
            let more = pane.actions.is_none().then(|| self.view_container_more_actions(container.key(), cx));
            let title = self.side_bar_title_text(title).when(id == FILE_VIEW, |title| title.child(self.explorer_title_actions(window, cx))).children(pane.actions).children(more);
            return frame.child(title).child(div().flex_1().min_h_0().flex().flex_col().child(pane.body));
        }
        frame.child(self.side_bar_title_text(title.to_string()).child(self.view_container_more_actions(container.key(), cx))).child(self.pane_view_element(container, ordered, window, cx))
    }

    /// The panes in the pane view's order: the given ones, the moved views' (`prepare_moved_view_panes`), else a
    /// contributed tree view's.
    pub(super) fn ordered_panes(&self, ids: &[&'static str], mut panes: Vec<ViewPane>, window: &Window, cx: &mut Context<Self>) -> Vec<ViewPane> {
        let mut moved = self.moved_view_panes.borrow_mut();
        let mut ordered = Vec::new();
        for &id in ids {
            ordered.push(match (panes.iter().position(|pane| pane.id == id), moved.iter().position(|pane| pane.id == id)) {
                (Some(position), _) => panes.remove(position),
                (None, Some(position)) => moved.remove(position),
                (None, None) => self.tree_view_pane(id, window, cx),
            });
        }
        ordered
    }

    /// The panes with their headers and the sashes between them.
    pub(super) fn pane_view_element(&self, container: PaneContainer, ordered: Vec<ViewPane>, window: &Window, cx: &mut Context<Self>) -> Div {
        let view = self.panes(container).view.as_ref().expect("panes are laid out before they draw");
        let (ids, sizes, sash_states) = (view.ids().to_vec(), view.sizes(), view.sash_states());
        let expanded: Vec<bool> = (0..ids.len()).map(|index| view.is_expanded(index)).collect();
        let dragging_sash = view.dragged_sash().is_some();
        let mut elements = Vec::new();
        for (index, (&id, pane)) in ids.iter().zip(ordered).enumerate() {
            let header = self.pane_header(container, index, id, &pane.title, pane.description, pane.actions.filter(|_| expanded[index]), pane.show_actions, expanded[index], ids.len(), window, cx);
            elements.push(self.pane(container, id, sizes[index], header, expanded[index].then_some(pane.body), cx));
        }
        let mut top = 0.;
        let mut sashes = Vec::new();
        for (index, state) in sash_states.into_iter().enumerate() {
            top += sizes[index];
            sashes.push(self.pane_sash(container, index, state, top, cx));
        }
        div()
            .relative()
            .h(px(sizes.iter().sum()))
            .flex_none()
            .flex()
            .flex_col()
            .children(elements)
            .children(sashes)
            .when(dragging_sash, |view| view.child(self.pane_sash_drag(container, cx)))
    }

    /// `.pane` with its body under the header while expanded, and the drop overlay of a dragged header.
    fn pane(&self, container: PaneContainer, id: &'static str, size: f32, header: Div, body: Option<AnyElement>, cx: &mut Context<Self>) -> AnyElement {
        let drop = self.panes(container).drop.filter(|(target, _)| *target == id && cx.has_active_drag()).map(|(_, below)| below);
        let overlay = self.color("sideBar.dropBackground");
        let tracked = self.panes(container).bounds.clone();
        let outline = self.theme.color("contrastActiveBorder");
        div()
            .id(id)
            .relative()
            .h(px(size))
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(header)
            .when_some(body, |pane, body| pane.child(div().flex_1().min_h_0().flex().flex_col().child(body)))
            .child(canvas(move |bounds, _, _| _ = tracked.borrow_mut().insert(id, bounds), |_, _, _, _| ()).absolute().inset_0())
            .on_hover(cx.listener(|_, _: &bool, _, cx| cx.notify()))
            .on_drag_move(cx.listener(move |workbench, event: &DragMoveEvent<DraggedPane>, _, cx| {
                if !event.bounds.contains(&event.event.position) {
                    return;
                }
                let dragged = event.drag(cx);
                let below = event.event.position.y >= event.bounds.center().y;
                let accepts = dragged.container == container || workbench.can_move_view(dragged.id);
                let drop = (accepts && dragged.id != id).then_some((id, below));
                let panes = workbench.panes_mut(container);
                if panes.drop != drop {
                    panes.drop = drop;
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(move |workbench, dragged: &DraggedPane, _, cx| {
                let below = workbench.panes(container).drop.filter(|(target, _)| *target == id).map(|(_, below)| below);
                match below {
                    Some(below) if dragged.container != container => workbench.drop_pane_from_container(dragged.id, container, id, below, cx),
                    Some(below) => workbench.drop_pane(container, dragged.id, id, below, cx),
                    None => workbench.panes_mut(container).drop = None,
                }
            }))
            .when_some(drop, |pane, below| {
                let half = div().absolute().left_0().right_0().h(gpui::relative(0.5)).bg(overlay);
                let half = if below { half.bottom_0() } else { half.top_0() };
                pane.child(half.when_some(outline, |half, color| half.border_2().border_dashed().border_color(color)))
            })
            .into_any_element()
    }

    #[cfg(test)]
    pub(super) fn pane_header_focused(&self, id: &'static str, window: &Window) -> bool {
        self.explorer_panes.focus[id].is_focused(window)
    }

    /// The movable view whose pane header has focus (viewPane.ts sets `focusedView` for the whole pane).
    pub(super) fn focused_pane_header(&self, window: &Window) -> Option<&'static str> {
        let containers = [&self.explorer_panes, &self.scm_panes].into_iter().chain(self.generated_panes.values());
        let focused = containers.flat_map(|panes| panes.focus.iter()).find(|(_, focus)| focus.is_focused(window)).map(|(id, _)| *id);
        focused.filter(|id| self.can_move_view(id))
    }

    /// A pane header focused by the keyboard, so it draws its focus outline.
    pub(super) fn focus_pane_header(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.explorer_panes.pointer_focused = None;
        window.focus(&self.explorer_panes.focus[id].clone(), cx);
    }

    /// `.pane:hover`. The hover changes only on a mouse move, where a browser drops `:hover` once `.context-view-block`
    /// covers the pointer: the pane counts as not hovered while a context menu is open.
    pub(super) fn pane_hovered(&self, container: PaneContainer, id: &'static str, window: &Window) -> bool {
        self.context_menu.is_none() && self.panes(container).bounds.borrow().get(id).is_some_and(|bounds| bounds.contains(&window.mouse_position()))
    }

    /// `.pane-header`: a click, Enter or Space toggles, Left collapses, Right expands, Up and Down move to the
    /// neighbouring header; dragged onto another pane it moves this one.
    #[allow(clippy::too_many_arguments)]
    fn pane_header(&self, container: PaneContainer, index: usize, id: &'static str, title: &str, description: Option<String>, actions: Option<AnyElement>, show_actions: bool, expanded: bool, count: usize, window: &Window, cx: &mut Context<Self>) -> Div {
        let focus = &self.panes(container).focus[id];
        let focused = focus.is_focused(window);
        // paneview.css: the actions show on `.pane:focus-within` and with `workbench.view.alwaysShowHeaderActions`.
        let show_actions = show_actions
            || focused
            || (id == TIMELINE_VIEW && self.timeline_focus_within(window))
            || (id == OUTLINE_VIEW && self.outline_pane.focus.is_focused(window))
            || self.setting("workbench.view.alwaysShowHeaderActions") == json!(true);
        let foreground = self.foreground_or("sideBarSectionHeader.foreground");
        let background = self.color("sideBar.background");
        let border = self.theme.color("sideBarSectionHeader.border").or(self.theme.color("surface.border")).filter(|_| index > 0);
        let chevron = if expanded { "chevron-down" } else { "chevron-right" };
        let dragged = DraggedPane {
            container,
            id,
            title: title.to_string(),
            chevron: self.app_root.join("node_modules/@vscode/codicons/src/icons").join(format!("{chevron}.svg")).to_string_lossy().into_owned(),
            // The side bar part less the card's borders and the header's margins.
            width: self.part_grid.grid.as_ref().map_or(0., |grid| grid.size(super::layout::Part::SideBar)) - 10.,
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
                // paneviewlet.css: the title ellipsizes to make room for the actions ("Gr..." beside Graph's).
                .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
                // paneviewlet.css `.pane-header .description`: 10px after the title, normal weight, only while expanded.
                .when(id == OPEN_EDITORS_VIEW, |header| header.children(self.open_editors_dirty_badge(cx)))
                .children(description.filter(|_| expanded).map(|description| {
                    div().ml(px(10.)).text_size(px(12.)).text_color(self.color("panelTitle.inactiveForeground")).overflow_hidden().whitespace_nowrap().child(description)
                }))
                .children(actions.filter(|_| expanded).map(|actions| div().flex_none().ml_auto().mr(px(4.)).invisible().when(show_actions || self.pane_hovered(container, id, window), |actions| actions.visible()).child(actions)))
                .when(id == FILE_VIEW, |header| header.child(self.explorer_title_actions(window, cx)))
                .when(focused && self.panes(container).pointer_focused != Some(id), |header| header.child(div().absolute().inset_0().rounded(px(4.)).border_1().border_color(self.color("focusBorder"))))
                .on_click(cx.listener(move |workbench, event: &gpui::ClickEvent, window, cx| {
                    let panes = workbench.panes_mut(container);
                    panes.pointer_focused = (!event.is_keyboard()).then_some(id);
                    let focus = panes.focus[id].clone();
                    window.focus(&focus, cx);
                    workbench.set_pane_expanded(container, index, !expanded, cx);
                }))
                .on_key_down(cx.listener(move |workbench, event: &KeyDownEvent, window, cx| {
                    if workbench.panes_mut(container).pointer_focused.take().is_some() {
                        cx.notify();
                    }
                    let neighbour = match event.keystroke.key.as_str() {
                        // Enter and Space reach `on_click`: gpui clicks a focused element on its activation keys.
                        "left" => return workbench.set_pane_expanded(container, index, false, cx),
                        "right" => return workbench.set_pane_expanded(container, index, true, cx),
                        // The header's toolbar is the next tab stop.
                        "tab" if id == TIMELINE_VIEW && expanded && !event.keystroke.modifiers.shift => {
                            cx.stop_propagation();
                            return workbench.focus_timeline_toolbar(window, cx);
                        }
                        "up" if index > 0 => index - 1,
                        "down" if index + 1 < count => index + 1,
                        _ => return,
                    };
                    let panes = workbench.panes(container);
                    let ids = panes.view.as_ref().expect("panes are laid out before they draw").ids();
                    let focus = panes.focus.get(ids[neighbour]).cloned();
                    if let Some(focus) = focus {
                        window.focus(&focus, cx);
                        cx.notify();
                    }
                }))
                .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone())),
        )
    }

    /// A horizontal `.monaco-sash` between two panes (sash.css): 4px centered on the boundary.
    fn pane_sash(&self, container: PaneContainer, index: usize, state: SashState, boundary: f32, cx: &mut Context<Self>) -> AnyElement {
        if state == SashState::Disabled {
            return div().into_any_element();
        }
        let panes = self.panes(container);
        let dragged = panes.view.as_ref().and_then(PaneView::dragged_sash);
        let active = panes.hovered_sash == Some(index) || dragged == Some(index);
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
                let delay = workbench.sash_hover_delay();
                let panes = workbench.panes_mut(container);
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
                panes.sash_hover = Some((index, cx.spawn_in(window, async move |workbench, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = workbench.update(cx, |workbench, cx| {
                        workbench.panes_mut(container).hovered_sash = Some(index);
                        cx.notify();
                    });
                })));
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |workbench, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let view = workbench.panes_mut(container).view.as_mut().expect("sashes are drawn with the panes");
                    view.start_sash_drag(index, f32::from(event.position.y), event.modifiers.alt);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// Follows the pointer anywhere in the window while a pane sash drags; the sizes are stored when it ends.
    fn pane_sash_drag(&self, container: PaneContainer, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let workbench = cx.entity();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
                    moved.update(cx, |workbench, cx| {
                        let view = workbench.panes_mut(container).view.as_mut().expect("a sash drags in the panes");
                        view.drag_sash(f32::from(event.position.y), event.modifiers.alt);
                        cx.notify();
                    });
                });
                let released = workbench.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                    released.update(cx, |workbench, cx| {
                        let panes = workbench.panes_mut(container);
                        let view = panes.view.as_mut().expect("a sash drags in the panes");
                        view.end_sash_drag();
                        let sizes: Vec<(String, f32)> = view.ids().iter().map(|id| id.to_string()).zip(view.sizes()).collect();
                        panes.stored_sizes = sizes.into_iter().collect();
                        workbench.store_panes(container);
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
    fn body_limits_clamp_the_pane_and_the_others_take_the_change() {
        let mut view = PaneView::new(600., &[(OPEN_EDITORS_VIEW, true, None), (FILE_VIEW, true, None)]);
        view.set_body_limits(0, 44., 66.);
        assert_eq!(view.sizes(), [HEADER_SIZE + 66., 600. - HEADER_SIZE - 66.]);
        view.set_body_limits(0, 0., 0.);
        assert_eq!(view.sizes(), [HEADER_SIZE, 600. - HEADER_SIZE]);
        view.set_expanded(0, false);
        view.set_expanded(0, true);
        assert_eq!(view.sizes(), [HEADER_SIZE, 600. - HEADER_SIZE]);
    }

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

    // The reference's Graph header sits 3px above the middle: the share counts the container's 5px bottom gap.
    #[test]
    fn equal_shares_are_of_the_container_with_its_bottom_gap() {
        let view = PaneView::new(662., &[(SCM_CHANGES_VIEW, true, None), (SCM_HISTORY_VIEW, true, None)]);
        assert_eq!(view.sizes(), vec![328., 334.]);
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
