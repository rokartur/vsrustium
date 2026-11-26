//! Where each movable view is (viewDescriptorService.ts): its default container, or the container `views.customizations`
//! moved it to, with the containers generated for views moved to a new entry (`workbench.views.service.<location>.<uuid>`),
//! and layoutActions.ts Move View, Move Focused View and Reset Focused View Location.
// shortcut: the movable views are the ones whose body any container can draw (Outline, Timeline and the panel's
// views); the folder view, Source Control, Search, Run and Debug and contributed tree views stay in their container
// until their bodies draw outside it. Built-in panel containers take no added views (panes side by side in the panel).

use gpui::{ClickEvent, Context, Div, InteractiveElement, IntoElement, ParentElement, PromptLevel, Rgba, Stateful, StatefulInteractiveElement, Styled, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::hover::HoverOptions;
use super::layout::Part;
use super::search_view::SideBarView;
use super::tree_view::static_id;
use super::view_panes::{OUTLINE_VIEW, PaneContainer, TIMELINE_VIEW, ViewPane};
use crate::commands::Outcome;
use crate::quick_input::{Entry, Item, Kind, QuickInput};

#[cfg(test)]
mod tests;

/// viewDescriptorService.ts `VIEWS_CUSTOMIZATIONS`, in profile storage.
pub(super) const VIEWS_CUSTOMIZATIONS_KEY: &str = "views.customizations";
pub(super) const EXPLORER_CONTAINER: &str = "workbench.view.explorer";
pub(super) const SCM_CONTAINER: &str = "workbench.view.scm";
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

    /// The views the port can move, in their containers' order: the Explorer's, then the panel's.
    pub(super) fn movable_views(&self) -> Vec<MovableView> {
        let mut views = vec![
            MovableView { id: OUTLINE_VIEW, name: "Outline", default_container: EXPLORER_CONTAINER, icon: "symbol-class" },
            MovableView { id: TIMELINE_VIEW, name: "Timeline", default_container: EXPLORER_CONTAINER, icon: "history" },
        ];
        views.extend(self.panel.views.iter().map(|view| MovableView { id: view.view_id, name: view.title, default_container: view.id, icon: panel_view_icon(view.id) }));
        views
    }

    fn movable_view(&self, id: &str) -> Option<MovableView> {
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

    /// The containers views can be in: the Explorer, Source Control, the panel's and the generated ones.
    fn container_ids(&self) -> Vec<&'static str> {
        let mut ids = vec![EXPLORER_CONTAINER, SCM_CONTAINER];
        ids.extend(self.panel.views.iter().map(|view| view.id));
        ids.extend(self.generated_containers());
        ids
    }

    fn generated_containers(&self) -> Vec<&'static str> {
        let customizations = self.view_customizations();
        customizations["viewContainerLocations"].as_object().into_iter().flatten().map(|(id, _)| id.as_str()).filter(|id| is_generated_container(id)).map(static_id).collect()
    }

    /// `getViewContainerLocation`.
    pub(super) fn container_location(&self, container: &str) -> Part {
        if container == EXPLORER_CONTAINER || container == SCM_CONTAINER {
            return Part::SideBar;
        }
        if is_generated_container(container) {
            return self.view_customizations()["viewContainerLocations"][container].as_u64().map_or(Part::SideBar, location_of_code);
        }
        self.view_container_part(container)
    }

    /// The movable views in the container, in the movable views' order.
    pub(super) fn views_in(&self, container: &str) -> Vec<MovableView> {
        self.movable_views().into_iter().filter(|view| self.view_container(view) == container).collect()
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

    /// The container the side bar shows.
    pub(super) fn side_bar_container(&self) -> Option<&'static str> {
        match self.side_bar_view {
            SideBarView::Explorer => Some(EXPLORER_CONTAINER),
            SideBarView::Scm => Some(SCM_CONTAINER),
            SideBarView::Generated(id) => Some(id),
            _ => None,
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
                self.side_bar_view = match container {
                    EXPLORER_CONTAINER => SideBarView::Explorer,
                    SCM_CONTAINER => SideBarView::Scm,
                    other => SideBarView::Generated(other),
                };
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
            OUTLINE_VIEW => {
                let show_actions = self.outline_pane.focus.is_focused(window);
                ViewPane { id, title: "Outline".into(), description: None, actions: Some(self.outline_title_actions(window, cx).into_any_element()), show_actions, body: self.outline_pane_body(window, cx) }
            }
            TIMELINE_VIEW => self.timeline_pane(window, cx),
            _ => {
                let view = self.panel.views.iter().find(|view| view.view_id == id).expect("a movable view");
                let (title, render, actions) = (view.title, view.render, view.actions);
                ViewPane { id, title: title.into(), description: None, actions: Some(actions(self, window, cx)), show_actions: false, body: render(self, window, cx) }
            }
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
                Part::SideBar => vec![EXPLORER_CONTAINER, SCM_CONTAINER],
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
        match id {
            EXPLORER_CONTAINER => "Explorer".into(),
            SCM_CONTAINER => "Source Control".into(),
            _ if is_generated_container(id) => self.generated_container_view(id).map_or_else(String::new, |view| view.name.into()),
            _ => self.panel.views.iter().find(|view| view.id == id).map_or_else(String::new, |view| view.title.into()),
        }
    }

    /// MoveViewAction `getView`: the movable views grouped by container, the focused one picked.
    fn pick_view_to_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.focused_movable_view(window);
        let mut entries = Vec::new();
        let mut views = Vec::new();
        let mut active = Vec::new();
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
                if Some(view.id) == focused {
                    active.push(views.len() as i64);
                }
                entries.push(Entry::Item(Item { handle: views.len() as i64, label: view.name.into(), ..Item::default() }));
                views.push(view.id);
            }
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select a View to Move".into());
        input.set_items(entries);
        input.set_active_items(&active);
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
        self.panel.views.iter().find(|panel_view| panel_view.view_id == view.id).is_none_or(|panel_view| (panel_view.visible)(self))
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
    /// in the panel (likewise the side bars), then each location's containers but the view's own. Containers that
    /// reject added views (Extensions) and the built-in panel ones (see the shortcut above) are not offered.
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
                let accepts = *id == EXPLORER_CONTAINER || *id == SCM_CONTAINER || is_generated_container(id) || *id == view.default_container;
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
        other => PaneContainer::Generated(other),
    }
}
