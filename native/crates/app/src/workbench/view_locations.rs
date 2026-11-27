//! Where each movable view is (viewDescriptorService.ts): its default container, or the container `views.customizations`
//! moved it to, with the containers generated for views moved to a new entry (`workbench.views.service.<location>.<uuid>`),
//! and layoutActions.ts Move View, Move Focused View and Reset Focused View Location.

use gpui::{ClickEvent, Context, Div, InteractiveElement, ParentElement, PromptLevel, Rgba, Stateful, StatefulInteractiveElement, Styled, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::hover::HoverOptions;
use super::layout::Part;
use super::search_view::SideBarView;
use super::tree_view::static_id;
use super::debug_view::{BREAKPOINTS_VIEW, CALL_STACK_VIEW, DEBUG_CONTAINER, DEBUG_WELCOME_VIEW, LOADED_SCRIPTS_VIEW, VARIABLES_VIEW, WATCH_VIEW};
use super::view_panes::{EMPTY_VIEW, FILE_VIEW, OUTLINE_VIEW, PaneContainer, SCM_CHANGES_VIEW, SCM_HISTORY_VIEW, TIMELINE_VIEW, ViewPane};
use crate::commands::Outcome;
use crate::quick_input::{Entry, Item, Kind, QuickInput};

#[cfg(test)]
mod tests;

/// viewDescriptorService.ts `VIEWS_CUSTOMIZATIONS`, in profile storage.
pub(super) const VIEWS_CUSTOMIZATIONS_KEY: &str = "views.customizations";
pub(super) const EXPLORER_CONTAINER: &str = "workbench.view.explorer";
pub(super) const SCM_CONTAINER: &str = "workbench.view.scm";
/// search.ts `VIEWLET_ID` and `VIEW_ID`.
pub(super) const SEARCH_CONTAINER: &str = "workbench.view.search";
pub(super) const SEARCH_VIEW: &str = "workbench.view.search";
pub(super) const TESTING_CONTAINER: &str = "workbench.view.extension.test";
/// The built-in side bar containers views move between, as Move View lists them.
const SIDE_BAR_CONTAINERS: [&str; 4] = [EXPLORER_CONTAINER, SEARCH_CONTAINER, SCM_CONTAINER, DEBUG_CONTAINER];
/// viewDescriptorService.ts `COMMON_CONTAINER_ID_PREFIX`.
const GENERATED_PREFIX: &str = "workbench.views.service";

/// `ViewContainerLocation` as stored.
pub(super) fn location_code(part: Part) -> u64 {
    match part {
        Part::Panel => 1,
        Part::AuxiliaryBar => 2,
        _ => 0,
    }
}

fn location_of_code(code: u64) -> Part {
    match code {
        1 => Part::Panel,
        2 => Part::AuxiliaryBar,
        _ => Part::SideBar,
    }
}

/// `ViewContainerLocationToString`.
fn location_name(part: Part) -> &'static str {
    match part {
        Part::Panel => "panel",
        Part::AuxiliaryBar => "auxiliarybar",
        _ => "sidebar",
    }
}

pub(super) fn is_generated_container(id: &str) -> bool {
    id.starts_with(GENERATED_PREFIX)
}

/// A view that can move: its id, name, default container and `containerIcon`.
#[derive(Clone, Copy)]
pub(super) struct MovableView {
    pub id: &'static str,
    pub name: &'static str,
    pub default_container: &'static str,
    pub icon: &'static str,
}

/// What a Move View pick stands for.
#[derive(Clone, Copy)]
enum Destination {
    NewEntry(Part),
    Container(&'static str),
}

impl Workbench {
    /// `views.customizations` with its three maps, as `getStoredViewCustomizationsValue` parses it.
    pub(super) fn view_customizations(&self) -> Value {
        let stored: Option<Value> = self.profile_storage.lock().unwrap().get(VIEWS_CUSTOMIZATIONS_KEY).and_then(|stored| serde_json::from_str(stored).ok());
        let mut customizations = stored.filter(Value::is_object).unwrap_or_else(|| json!({}));
        for key in ["viewContainerLocations", "viewLocations", "viewContainerBadgeEnablementStates"] {
            if !customizations[key].is_object() {
                customizations[key] = json!({});
            }
        }
        customizations
    }

    pub(super) fn save_view_customizations(&self, customizations: &Value) {
        self.profile_storage.lock().unwrap().set(VIEWS_CUSTOMIZATIONS_KEY, customizations.to_string());
    }

    /// The views with `canMoveView`, in their containers' order: the side bar's, the panel's, then the extensions'
    /// tree views.
    pub(super) fn movable_views(&self) -> Vec<MovableView> {
        let view = |id, name, default_container, icon| MovableView { id, name, default_container, icon };
        let mut views = vec![
            view(EMPTY_VIEW, "No Folder Opened", EXPLORER_CONTAINER, "files"),
            view(FILE_VIEW, "Folders", EXPLORER_CONTAINER, "files"),
            view(OUTLINE_VIEW, "Outline", EXPLORER_CONTAINER, "symbol-class"),
            view(TIMELINE_VIEW, "Timeline", EXPLORER_CONTAINER, "history"),
            view(SEARCH_VIEW, "Search", SEARCH_CONTAINER, "search-large"),
            view(SCM_CHANGES_VIEW, "Changes", SCM_CONTAINER, "source-control"),
            view(SCM_HISTORY_VIEW, "Graph", SCM_CONTAINER, "source-control"),
            view(VARIABLES_VIEW, "Variables", DEBUG_CONTAINER, "debug-alt"),
            view(WATCH_VIEW, "Watch", DEBUG_CONTAINER, "debug-alt"),
            view(CALL_STACK_VIEW, "Call Stack", DEBUG_CONTAINER, "debug-alt"),
            view(LOADED_SCRIPTS_VIEW, "Loaded Scripts", DEBUG_CONTAINER, "debug-alt"),
            view(BREAKPOINTS_VIEW, "Breakpoints", DEBUG_CONTAINER, "debug-alt"),
        ];
        views.extend(self.panel.views.iter().map(|panel_view| view(panel_view.view_id, panel_view.title, panel_view.id, panel_view_icon(panel_view.id))));
        for tree_view in &self.tree_views.views {
            let Some((container, icon)) = self.tree_view_default_container(&tree_view.container) else { continue };
            views.push(view(tree_view.id, static_id(&tree_view.name), container, icon));
        }
        views
    }

    /// The container a `contributes.views` key names, with its codicon, which a contributed view takes as its
    /// `containerIcon`.
    // A view's own `icon` and a container's file icon draw as the window codicon (port/notes.md, milestone 11).
    fn tree_view_default_container(&self, key: &str) -> Option<(&'static str, &'static str)> {
        match key {
            "explorer" => Some((EXPLORER_CONTAINER, "files")),
            "scm" => Some((SCM_CONTAINER, "source-control")),
            "debug" => Some((DEBUG_CONTAINER, "debug-alt")),
            "test" => Some((TESTING_CONTAINER, "beaker")),
            key => self.tree_views.containers.iter().find(|container| container.key == key).map(|container| {
                let icon = match &container.icon {
                    super::view_containers::ContainerIcon::Codicon(name) => static_id(name),
                    super::view_containers::ContainerIcon::File(_) => "window",
                };
                (container.id, icon)
            }),
        }
    }

    /// Whether the view is in `container`: a view that cannot move always is in its own.
    pub(super) fn stays_in(&self, view: &str, container: &str) -> bool {
        self.movable_view(view).is_none_or(|view| self.view_container(&view) == container)
    }

    pub(super) fn movable_view(&self, id: &str) -> Option<MovableView> {
        self.movable_views().into_iter().find(|view| view.id == id)
    }

    /// `getViewContainerByViewId`: the stored container while it exists, else the default one.
    pub(super) fn view_container(&self, view: &MovableView) -> &'static str {
        let customizations = self.view_customizations();
        let Some(stored) = customizations["viewLocations"][view.id].as_str() else { return view.default_container };
        if is_generated_container(stored) {
            return match customizations["viewContainerLocations"][stored].is_u64() {
                true => static_id(stored),
                false => view.default_container,
            };
        }
        self.container_ids().into_iter().find(|id| *id == stored).unwrap_or(view.default_container)
    }

    /// The containers views can be in: the side bar's, the panel's and the generated ones.
    fn container_ids(&self) -> Vec<&'static str> {
        let mut ids = self.side_bar_container_ids();
        ids.extend(self.panel.views.iter().map(|view| view.id));
        ids.extend(self.generated_containers());
        ids
    }

    fn generated_containers(&self) -> Vec<&'static str> {
        let customizations = self.view_customizations();
        customizations["viewContainerLocations"].as_object().into_iter().flatten().map(|(id, _)| id.as_str()).filter(|id| is_generated_container(id)).map(static_id).collect()
    }

    /// The built-in side bar containers, Testing and the extensions' `viewsContainers.activitybar`, in the activity
    /// bar's order.
    fn side_bar_container_ids(&self) -> Vec<&'static str> {
        let mut ids = SIDE_BAR_CONTAINERS.to_vec();
        ids.push(TESTING_CONTAINER);
        ids.extend(self.tree_views.containers.iter().map(|container| container.id));
        ids
    }

    /// `getViewContainerLocation`.
    pub(super) fn container_location(&self, container: &str) -> Part {
        if self.side_bar_container_ids().contains(&container) {
            return Part::SideBar;
        }
        if is_generated_container(container) {
            return self.view_customizations()["viewContainerLocations"][container].as_u64().map_or(Part::SideBar, location_of_code);
        }
        self.view_container_part(container)
    }

    /// The movable views in the container, in the movable views' order.
    // Upstream sorts a container's views by descriptor `order` (none last); the movable views' order matches it for
    // the Explorer's views moved beside a panel view (move-view.json), not in general (port/notes.md).
    pub(super) fn views_in(&self, container: &str) -> Vec<MovableView> {
        self.movable_views().into_iter().filter(|view| self.view_container(view) == container).collect()
    }

    /// `hideIfEmpty`: the Explorer, Search and Source Control leave the activity bar while none of their views shows.
    pub(super) fn container_hidden_if_empty(&self, container: &str) -> bool {
        [EXPLORER_CONTAINER, SEARCH_CONTAINER, SCM_CONTAINER].contains(&container) && !self.views_in(container).iter().any(|view| self.movable_view_shown(view))
    }

    /// The generated containers in the part that hold a view.
    pub(super) fn generated_containers_in(&self, part: Part) -> Vec<&'static str> {
        self.generated_containers().into_iter().filter(|id| self.container_location(id) == part && !self.views_in(id).is_empty()).collect()
    }

    /// viewContainerModel.ts `updateContainerInfo`: a generated container takes its first view's name and icon.
    pub(super) fn generated_container_view(&self, container: &str) -> Option<MovableView> {
        self.views_in(container).into_iter().next()
    }

    /// `moveViewsToContainer` and `cleanUpGeneratedViewContainer`: the view's entry goes when it is back home, and a
    /// generated container left without views goes with its state.
    pub(super) fn move_view_to_container(&mut self, view: &str, container: &'static str, cx: &mut Context<Self>) {
        let Some(view) = self.movable_view(view) else { return };
        let from = self.view_container(&view);
        if from == container {
            return;
        }
        let mut customizations = self.view_customizations();
        let locations = customizations["viewLocations"].as_object_mut().expect("normalized above");
        match container == view.default_container {
            true => drop(locations.remove(view.id)),
            false => drop(locations.insert(view.id.to_owned(), json!(container))),
        }
        let still_used = locations.values().any(|stored| stored == from);
        if is_generated_container(from) && !still_used {
            customizations["viewContainerLocations"].as_object_mut().expect("normalized above").remove(from);
            self.profile_storage.lock().unwrap().delete(&format!("{from}.state.hidden"));
            self.generated_panes.remove(from);
            if self.side_bar_view == SideBarView::Generated(from) {
                self.side_bar_view = SideBarView::Explorer;
            }
        }
        self.save_view_customizations(&customizations);
        cx.notify();
    }

    /// `moveViewToLocation`: a new generated container in the part, the view moved into it.
    pub(super) fn move_view_to_location(&mut self, view: &str, part: Part, cx: &mut Context<Self>) -> &'static str {
        let id = static_id(&format!("{GENERATED_PREFIX}.{}.{}", location_name(part), crate::debug::generate_uuid()));
        let mut customizations = self.view_customizations();
        customizations["viewContainerLocations"][id] = json!(location_code(part));
        self.save_view_customizations(&customizations);
        self.move_view_to_container(view, id, cx);
        id
    }

    /// A view header dropped on a composite bar: the view moves to a new entry there and opens.
    pub(super) fn drop_view_on_location(&mut self, view: &'static str, part: Part, cx: &mut Context<Self>) {
        if !self.can_move_view(view) {
            return;
        }
        let container = self.move_view_to_location(view, part, cx);
        self.open_view_container_of(container, view, cx);
    }

    /// A view dropped on a container's activity bar item or panel tab moves into it and opens there.
    pub(super) fn drop_view_on_container(&mut self, view: &'static str, container: &'static str, cx: &mut Context<Self>) {
        let Some(movable) = self.movable_view(view) else { return };
        if self.view_container(&movable) == container {
            return;
        }
        self.move_view_to_container(view, container, cx);
        self.open_view_container_of(container, view, cx);
    }

    /// The container the side bar shows.
    pub(super) fn side_bar_container(&self) -> Option<&'static str> {
        match self.side_bar_view {
            SideBarView::Explorer => Some(EXPLORER_CONTAINER),
            SideBarView::Search => Some(SEARCH_CONTAINER),
            SideBarView::Scm => Some(SCM_CONTAINER),
            SideBarView::Debug => Some(DEBUG_CONTAINER),
            SideBarView::Testing => Some(TESTING_CONTAINER),
            SideBarView::Contributed(id) | SideBarView::Generated(id) => Some(id),
            SideBarView::Extensions => None,
        }
    }

    /// `viewsService.isViewVisible` of a movable view: its container shows and the view's pane is expanded.
    pub(super) fn is_view_visible(&self, id: &str) -> bool {
        let Some(view) = self.movable_view(id) else { return false };
        let container = self.view_container(&view);
        match self.container_location(container) {
            Part::SideBar => self.side_bar_visible_container() == Some(container) && self.view_pane_expanded(pane_container(container), id),
            part => self.is_part_visible(part) && self.active_container_in(part) == Some(container) && (self.views_in(container).len() == 1 || self.view_pane_expanded(PaneContainer::Generated(container), id)),
        }
    }

    fn side_bar_visible_container(&self) -> Option<&'static str> {
        self.side_bar_container().filter(|_| self.is_part_visible(Part::SideBar))
    }

    /// `viewsService.openView(id)`: shows the view's container and expands its pane; `true` if the view moved, so the
    /// caller's own opening does not apply.
    pub(super) fn open_moved_view(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        let Some(view) = self.movable_view(id) else { return false };
        let container = self.view_container(&view);
        if container == view.default_container {
            return false;
        }
        self.open_view_container_of(container, id, cx);
        true
    }

    fn open_view_container_of(&mut self, container: &'static str, view: &str, cx: &mut Context<Self>) {
        match self.container_location(container) {
            Part::SideBar => {
                self.side_bar_view = side_bar_view_of(container);
                self.set_part_hidden(Part::SideBar, false, cx);
                self.expand_pane(pane_container(container), view, cx);
            }
            _ => {
                self.show_panel_container(container, cx);
                if self.generated_panes.contains_key(container) {
                    self.expand_pane(PaneContainer::Generated(container), view, cx);
                }
            }
        }
        cx.notify();
    }

    /// The activity bar items of the generated side bar containers: their view's icon and name.
    pub(super) fn generated_activity_items(&self, hover: HoverOptions, inactive: Rgba, cx: &mut Context<Self>) -> Vec<Stateful<Div>> {
        let mut items = Vec::new();
        for id in self.generated_containers_in(Part::SideBar) {
            let view = self.generated_container_view(id).expect("a container with a view");
            let active = self.side_bar_view == SideBarView::Generated(id) && self.is_part_visible(Part::SideBar);
            let icon = if active { self.active_view_icon(view.icon) } else { self.icon(view.icon, 36., 24., inactive) };
            items.push(icon.id(id).relative().cursor_pointer().child(self.action_hover(view.name, id, hover)).on_click(cx.listener(move |workbench, event: &ClickEvent, _, cx| {
                if event.click_count() > 1 {
                    return;
                }
                let toggles = active && workbench.setting("workbench.activityBar.iconClickBehavior") != "focus";
                match toggles {
                    true => workbench.set_part_hidden(Part::SideBar, true, cx),
                    false => workbench.open_view_container_of(id, view.id, cx),
                }
            })));
        }
        items
    }

    /// The pane of a view drawn outside its own container's code.
    pub(super) fn movable_view_pane(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) -> ViewPane {
        match id {
            FILE_VIEW | EMPTY_VIEW => self.folder_view_pane(window, cx),
            OUTLINE_VIEW => self.outline_view_pane(window, cx),
            TIMELINE_VIEW => self.timeline_pane(window, cx),
            SEARCH_VIEW => self.search_view_pane(window, cx),
            SCM_CHANGES_VIEW => self.scm_changes_pane(window, cx),
            SCM_HISTORY_VIEW => {
                let provider = self.active_scm_provider().expect("Graph shows with a history provider");
                self.scm_graph_pane(provider, window, cx)
            }
            VARIABLES_VIEW | WATCH_VIEW | CALL_STACK_VIEW | LOADED_SCRIPTS_VIEW | BREAKPOINTS_VIEW | DEBUG_WELCOME_VIEW => self.debug_view_pane(id, window, cx),
            _ => match self.panel.views.iter().find(|view| view.view_id == id) {
                Some(view) => {
                    let (title, render, actions) = (view.title, view.render, view.actions);
                    ViewPane { id, title: title.into(), description: None, actions: Some(actions(self, window, cx)), show_actions: false, body: render(self, window, cx) }
                }
                None => self.tree_view_pane(id, window, cx),
            },
        }
    }

    /// The panes of the views moved into the side bar's container, built before the side bar draws them.
    pub(super) fn prepare_moved_view_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut panes = Vec::new();
        if let Some(container) = self.side_bar_container() {
            for view in self.views_in(container) {
                if view.default_container != container {
                    panes.push(self.movable_view_pane(view.id, window, cx));
                }
            }
        }
        *self.moved_view_panes.borrow_mut() = panes;
    }

    /// The view `focusedView` names: the focused movable view.
    pub(super) fn focused_movable_view(&self, window: &Window) -> Option<&'static str> {
        if self.outline_pane.focus.is_focused(window) {
            return Some(OUTLINE_VIEW);
        }
        if self.timeline_focus_within(window) {
            return Some(TIMELINE_VIEW);
        }
        if self.explorer.focus.is_focused(window) {
            return Some(FILE_VIEW);
        }
        if self.search.focus.is_focused(window) || super::search_view::SEARCH_FIELDS.iter().any(|&field| self.search.input(field).focus.is_focused(window)) {
            return Some(SEARCH_VIEW);
        }
        if self.scm.input_focus.is_focused(window) || self.scm.list_focus.is_focused(window) {
            return Some(SCM_CHANGES_VIEW);
        }
        let focused_container = if self.problems_focus.is_focused(window) || self.problems.filter_focus.is_focused(window) {
            Some(super::problems_view::MARKERS_CONTAINER_ID)
        } else if self.output_focused(window) {
            Some(super::output_view::OUTPUT_VIEW_ID)
        } else if self.terminal_view.focus.is_focused(window) {
            Some(super::terminal_view::TERMINAL_VIEW_ID)
        } else if self.debug_console.focus.is_focused(window) || self.debug_console.input_focus.is_focused(window) {
            Some(super::debug_console::DEBUG_PANEL_ID)
        } else {
            None
        };
        let focused = focused_container.and_then(|id| self.panel.views.iter().find(|view| view.id == id)).map(|view| view.view_id);
        focused.or_else(|| self.focused_pane_header(window))
    }

    pub(super) fn register_move_view_actions(&mut self) {
        self.commands.register("workbench.action.moveView", |workbench, _, window, cx| {
            workbench.pick_view_to_move(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.moveFocusedView", |workbench, args, window, cx| {
            let view = args.first().and_then(Value::as_str).map(static_id).or_else(|| workbench.focused_movable_view(window));
            workbench.move_focused_view(view, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.resetFocusedViewLocation", |workbench, _, window, cx| {
            let Some(view) = workbench.focused_movable_view(window).and_then(|id| workbench.movable_view(id)) else {
                drop(window.prompt(PromptLevel::Critical, "There is no view currently focused.", None, &["OK"], cx));
                return Outcome::done();
            };
            if workbench.view_container(&view) != view.default_container {
                workbench.move_view_to_container(view.id, view.default_container, cx);
                workbench.open_view_container_of(view.default_container, view.id, cx);
            }
            Outcome::done()
        });
    }

    /// The side bar, panel and secondary side bar containers with a view, in the order Move View lists them.
    fn containers_by_location(&self) -> Vec<(Part, &'static str, String)> {
        let mut containers = Vec::new();
        for part in [Part::SideBar, Part::Panel, Part::AuxiliaryBar] {
            let mut ids: Vec<&'static str> = match part {
                Part::SideBar => self.side_bar_container_ids(),
                _ => self.panel.views.iter().filter(|view| self.view_container_part(view.id) == part).map(|view| view.id).collect(),
            };
            ids.extend(self.generated_containers_in(part));
            for id in ids {
                containers.push((part, id, self.container_title(id)));
            }
        }
        containers
    }

    /// `ViewContainerModel.title`.
    fn container_title(&self, id: &str) -> String {
        if let Some(container) = self.tree_views.containers.iter().find(|container| container.id == id) {
            return container.title.clone();
        }
        match id {
            EXPLORER_CONTAINER => "Explorer".into(),
            SEARCH_CONTAINER => "Search".into(),
            SCM_CONTAINER => "Source Control".into(),
            DEBUG_CONTAINER => "Run and Debug".into(),
            TESTING_CONTAINER => "Testing".into(),
            _ if is_generated_container(id) => self.generated_container_view(id).map_or_else(String::new, |view| view.name.into()),
            _ => self.panel.views.iter().find(|view| view.id == id).map_or_else(String::new, |view| view.title.into()),
        }
    }

    /// MoveViewAction `getView`: the movable views grouped by container, the first one active (upstream selects the
    /// focused one, which a single-select pick does not show).
    fn pick_view_to_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut entries = Vec::new();
        let mut views = Vec::new();
        for (part, container, title) in self.containers_by_location() {
            let shown: Vec<MovableView> = self.views_in(container).into_iter().filter(|view| self.movable_view_shown(view)).collect();
            if shown.is_empty() {
                continue;
            }
            let location = match part {
                Part::SideBar => "Side Bar",
                Part::Panel => "Panel",
                _ => "Secondary Side Bar",
            };
            entries.push(Entry::Separator(Some(format!("{location} / {title}"))));
            for view in shown {
                entries.push(Entry::Item(Item { handle: views.len() as i64, label: view.name.into(), ..Item::default() }));
                views.push(view.id);
            }
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select a View to Move".into());
        input.set_items(entries);
        let picked = self.choose(input, window, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Some(index) = picked.await.ok().and_then(Result::ok).and_then(|handle| handle.as_u64()) else { return };
            let view = views[index as usize];
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.move_focused_view(Some(view), window, cx));
        })
        .detach();
    }

    /// `visibleViewDescriptors`: a panel view whose `when` does not hold (Test Results without results) is left out.
    pub(super) fn movable_view_shown(&self, view: &MovableView) -> bool {
        let has_folder = self.folder.as_ref().is_some_and(|tree| !tree.workspace.folders.is_empty());
        match view.id {
            FILE_VIEW => has_folder,
            EMPTY_VIEW => !has_folder,
            SCM_HISTORY_VIEW => self.active_scm_provider().is_some_and(|provider| provider.has_history_provider),
            VARIABLES_VIEW | WATCH_VIEW | CALL_STACK_VIEW | LOADED_SCRIPTS_VIEW | BREAKPOINTS_VIEW => self.debug_views().iter().any(|(id, _)| *id == view.id),
            id => match (self.panel.views.iter().find(|panel_view| panel_view.view_id == id), self.tree_views.views.iter().find(|tree_view| tree_view.id == id)) {
                (Some(panel_view), _) => (panel_view.visible)(self),
                (None, Some(tree_view)) => self.shown_views_of(&tree_view.container).contains(&id),
                (None, None) => true,
            },
        }
    }

    /// MoveFocusedViewAction: new entries in the locations the view is not alone in, then the other containers.
    fn move_focused_view(&mut self, view: Option<&'static str>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = view else {
            drop(window.prompt(PromptLevel::Critical, "There is no view currently focused.", None, &["OK"], cx));
            return;
        };
        let Some(view) = self.movable_view(view) else {
            drop(window.prompt(PromptLevel::Critical, "The currently focused view is not movable.", None, &["OK"], cx));
            return;
        };
        let current = self.view_container(&view);
        let destinations = self.move_destinations(&view, current);
        let mut entries = Vec::new();
        for (handle, (separator, label, _)) in destinations.iter().enumerate() {
            if let Some(separator) = separator {
                entries.push(Entry::Separator(Some(separator.to_string())));
            }
            entries.push(Entry::Item(Item { handle: handle as i64, label: label.clone(), ..Item::default() }));
        }
        let destinations: Vec<Destination> = destinations.into_iter().map(|(_, _, destination)| destination).collect();
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select a Destination for the View".into());
        input.title = Some(format!("View: Move {}", view.name));
        input.set_items(entries);
        let picked = self.choose(input, window, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Some(index) = picked.await.ok().and_then(Result::ok).and_then(|handle| handle.as_u64()) else { return };
            let destination = destinations[index as usize];
            let _ = workbench.update_in(cx, |workbench, _, cx| {
                let container = match destination {
                    Destination::NewEntry(part) => workbench.move_view_to_location(view.id, part, cx),
                    Destination::Container(container) => {
                        workbench.move_view_to_container(view.id, container, cx);
                        container
                    }
                };
                workbench.open_view_container_of(container, view.id, cx);
            });
        })
        .detach();
    }

    /// The picks of MoveFocusedViewAction with the separator before each: "New Panel Entry" unless the view is alone
    /// in the panel (likewise the side bars), then each location's containers but the view's own. Extensions rejects
    /// added views; Testing and the extensions' containers draw only their own (port/notes.md).
    fn move_destinations(&self, view: &MovableView, current: &'static str) -> Vec<(Option<&'static str>, String, Destination)> {
        let solo = self.views_in(current).len() == 1;
        let current_location = self.container_location(current);
        let mut picks = Vec::new();
        for (part, label) in [(Part::Panel, "New Panel Entry"), (Part::SideBar, "New Side Bar Entry"), (Part::AuxiliaryBar, "New Secondary Side Bar Entry")] {
            if !(solo && current_location == part) {
                picks.push((None, label.to_owned(), Destination::NewEntry(part)));
            }
        }
        let containers = self.containers_by_location();
        for (part, separator) in [(Part::SideBar, "Side Bar"), (Part::Panel, "Panel"), (Part::AuxiliaryBar, "Secondary Side Bar")] {
            let mut separator = Some(separator);
            for (_, id, title) in containers.iter().filter(|(location, id, _)| *location == part && *id != current) {
                let draws_only_own_views = *id == TESTING_CONTAINER || self.tree_views.containers.iter().any(|container| container.id == *id);
                let accepts = !draws_only_own_views || *id == view.default_container;
                if accepts {
                    picks.push((separator.take(), title.clone(), Destination::Container(id)));
                }
            }
        }
        picks
    }
}

/// The `containerIcon` of a panel container's view.
fn panel_view_icon(container: &str) -> &'static str {
    match container {
        super::problems_view::MARKERS_CONTAINER_ID => "warning",
        super::output_view::OUTPUT_VIEW_ID => "output",
        super::terminal_view::TERMINAL_VIEW_ID => "terminal",
        super::debug_console::DEBUG_PANEL_ID => "debug-console",
        super::comments_view::COMMENTS_VIEW_ID => "comment-discussion",
        super::testing_view::TEST_RESULTS_VIEW_ID => "checklist",
        super::refactor_preview::REFACTOR_PREVIEW_ID => "lightbulb",
        _ => "window",
    }
}

/// The pane container drawing a side bar container.
pub(super) fn pane_container(container: &'static str) -> PaneContainer {
    match container {
        EXPLORER_CONTAINER => PaneContainer::Explorer,
        SCM_CONTAINER => PaneContainer::Scm,
        SEARCH_CONTAINER => PaneContainer::Search,
        DEBUG_CONTAINER => PaneContainer::Debug,
        other => PaneContainer::Generated(other),
    }
}

fn side_bar_view_of(container: &'static str) -> SideBarView {
    match container {
        EXPLORER_CONTAINER => SideBarView::Explorer,
        SEARCH_CONTAINER => SideBarView::Search,
        SCM_CONTAINER => SideBarView::Scm,
        DEBUG_CONTAINER => SideBarView::Debug,
        TESTING_CONTAINER => SideBarView::Testing,
        other if is_generated_container(other) => SideBarView::Generated(other),
        other => SideBarView::Contributed(other),
    }
}
