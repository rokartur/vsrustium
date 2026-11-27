//! The panel and the secondary side bar (panelPart.ts, auxiliaryBarPart.ts, paneCompositePart.ts): floating cards
//! with a composite title (the views' tabs, the active view's actions, maximize and close) over the active view.
//! Panel views register here with `register_panel_view`; a view container dragged onto the other part moves there
//! (viewDescriptorService.ts `moveViewContainerToLocation`, stored in `views.customizations`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    AnyElement, AppContext, Bounds, Context, Div, DragMoveEvent, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Render, Rgba, SharedString, StatefulInteractiveElement, Styled, TextRun, Window, div,
    prelude::FluentBuilder, px,
};
use serde_json::json;

use super::Workbench;
use super::hover::HoverOptions;
use super::layout::{Alignment, Part, Position};
use super::view_locations::{is_generated_container, location_code};
use crate::commands::Outcome;
use crate::menus::MenuAction;

/// paneCompositePart.ts `ACTIVE_PANEL_SETTINGS_KEY` of the panel and of the secondary side bar, in workspace storage.
const ACTIVE_PANEL_KEY: &str = "workbench.panelpart.activepanelid";
const ACTIVE_AUXILIARY_BAR_KEY: &str = "workbench.auxiliarybar.activepanelid";
/// The composite title's height (`--vscode-workbench-titleHeight` of a pane composite part).
const TITLE_HEIGHT: f32 = 32.;
/// views.css `.viewpane-filter-container` `max-width` in a panel title.
const FILTER_MAX_WIDTH: f32 = 200.;
const FILTER_MIN_WIDTH: f32 = 150.;
/// The "..." item as drawn: 6px padding each side of its 16px codicon.
const OVERFLOW_ITEM_WIDTH: f32 = 28.;
/// paneCompositePart.ts `MIN_COMPOSITE_BAR_WIDTH`.
const MIN_COMPOSITE_BAR_WIDTH: f32 = 50.;

/// A view container in the panel: its tab title, its content and its title actions (`MenuId.ViewTitle`).
pub struct PanelView {
    pub id: &'static str,
    /// The container's single view, whose `<view id>.focus` command opens it (viewsService.ts `registerFocusViewAction`).
    pub view_id: &'static str,
    pub title: &'static str,
    /// `openPaneComposite(id, true)`: shows the view and focuses its content.
    pub open: fn(&mut Workbench, &mut Window, &mut Context<Workbench>),
    pub render: fn(&mut Workbench, &mut Window, &mut Context<Workbench>) -> AnyElement,
    pub actions: fn(&Workbench, &mut Window, &mut Context<Workbench>) -> AnyElement,
    /// A `hideIfEmpty` container shows its tab only while one of its views' `when` holds.
    pub visible: fn(&Workbench) -> bool,
    /// The activity count the view's tab shows (`IActivity.badge`).
    pub badge: fn(&Workbench) -> Option<usize>,
}

/// A pane composite part's tab strip, title actions and content as last painted.
#[derive(Default)]
struct CompositeBar {
    title_bounds: Rc<Cell<Bounds<Pixels>>>,
    actions_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The active view whose title actions `actions_bounds` measured: a layout is kept only once they are its own.
    actions_view: Cell<Option<&'static str>>,
    overflow_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The active view's filter box in the title, empty without one.
    filter_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The width the tabs leave free beside a 200px filter box.
    filter_room: Cell<f32>,
    /// The shown tabs, what they were laid out for and the title actions' width then.
    tab_layout: RefCell<Option<(TabLayoutKey, f32, Vec<&'static str>)>>,
    /// The view's area under the title, the pane body size `FilterViewPane` places its filter by.
    content_bounds: Rc<Cell<Bounds<Pixels>>>,
}

#[derive(Default)]
pub(super) struct Panel {
    pub(super) views: Vec<PanelView>,
    active: Option<&'static str>,
    auxiliary_active: Option<&'static str>,
    panel_bar: CompositeBar,
    auxiliary_bar: CompositeBar,
    /// The part a dragged view container is over (`CompositeDragAndDropObserver` `onDragEnter`).
    drop_target: Option<Part>,
}

/// A global title action: its codicon, tooltip, command and whether it is toggled.
type GlobalAction = (&'static str, &'static str, &'static str, bool);

/// The title width, the tabs, the active view and whether it has a filter box.
type TabLayoutKey = (Pixels, Vec<&'static str>, Option<&'static str>, bool);

/// A composite bar tab being dragged (`CompositeDragAndDropData` of type `composite`), drawn as its label.
#[derive(Clone)]
pub(super) struct DraggedComposite {
    id: &'static str,
    label: SharedString,
    foreground: Rgba,
    background: Rgba,
}

impl Render for DraggedComposite {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().h(px(TITLE_HEIGHT)).px(px(16.)).flex().items_center().text_size(px(13.)).text_color(self.foreground).bg(self.background).opacity(0.8).child(self.label.clone())
    }
}

impl Workbench {
    pub(super) fn register_panel_view(&mut self, view: PanelView) {
        let open = view.open;
        self.commands.register(format!("{}.focus", view.view_id), move |workbench, _, window, cx| {
            open(workbench, window, cx);
            Outcome::done()
        });
        self.panel.views.push(view);
    }

    /// panelActions.ts and auxiliaryBarActions.ts.
    pub(super) fn register_panel_actions(&mut self) {
        self.commands.register("workbench.action.togglePanel", |workbench, _, _, cx| {
            match workbench.is_part_visible(Part::Panel) {
                true => workbench.hide_panel(cx),
                false => workbench.show_panel(cx),
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.closePanel", |workbench, _, _, cx| {
            workbench.hide_panel(cx);
            Outcome::done()
        });
        for (command, source, destination) in [("workbench.action.movePanelToSecondarySideBar", Part::Panel, Part::AuxiliaryBar), ("workbench.action.moveSecondarySideBarToPanel", Part::AuxiliaryBar, Part::Panel)] {
            self.commands.register(command, move |workbench, _, _, cx| {
                workbench.move_views_between_panels(source, destination, cx);
                Outcome::done()
            });
        }
        // panelActions.ts: maximizing needs a centered panel or one on a side.
        self.commands.register("workbench.action.toggleMaximizedPanel", |workbench, _, _, cx| {
            let arrangement = workbench.arrangement();
            if arrangement.alignment != Alignment::Center && arrangement.panel.is_horizontal() {
                workbench.notify_warning("Maximizing the panel is only supported when it is center aligned.");
                return Outcome::done();
            }
            if !workbench.is_part_visible(Part::Panel) {
                workbench.show_panel(cx);
                if !workbench.is_panel_maximized() {
                    workbench.toggle_maximized_panel(cx);
                }
            } else {
                workbench.toggle_maximized_panel(cx);
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.toggleAuxiliaryBar", |workbench, _, _, cx| {
            let visible = workbench.is_part_visible(Part::AuxiliaryBar);
            workbench.set_part_hidden(Part::AuxiliaryBar, visible, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.closeAuxiliaryBar", |workbench, _, _, cx| {
            workbench.set_part_hidden(Part::AuxiliaryBar, true, cx);
            Outcome::done()
        });
        // The preconditions of MaximizeAuxiliaryBar and RestoreAuxiliaryBar are the setter's early return.
        self.commands.register("workbench.action.maximizeAuxiliaryBar", |workbench, _, _, cx| {
            workbench.set_auxiliary_bar_maximized(true, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.restoreAuxiliaryBar", |workbench, _, _, cx| {
            workbench.set_auxiliary_bar_maximized(false, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.toggleMaximizedAuxiliaryBar", |workbench, _, _, cx| {
            let maximized = workbench.is_auxiliary_bar_maximized();
            workbench.set_auxiliary_bar_maximized(!maximized, cx);
            Outcome::done()
        });
    }

    /// `getViewContainerLocation`: the panel, unless `views.customizations` moved the container to the secondary side bar.
    pub(super) fn view_container_part(&self, id: &str) -> Part {
        match self.view_customizations()["viewContainerLocations"][id].as_u64() {
            Some(2) => Part::AuxiliaryBar,
            _ => Part::Panel,
        }
    }

    /// `moveViewContainerToLocation` and `openPaneComposite(id, true)` of a container dropped on `part`. Only moved
    /// containers keep an entry (`saveViewCustomizations` skips the default location).
    pub(super) fn move_view_container(&mut self, id: &'static str, part: Part, window: &mut Window, cx: &mut Context<Self>) {
        let mut customizations = self.view_customizations();
        let generated = is_generated_container(id);
        let locations = customizations["viewContainerLocations"].as_object_mut().expect("normalized");
        match part {
            Part::AuxiliaryBar => drop(locations.insert(id.to_owned(), json!(location_code(part)))),
            _ if generated => drop(locations.insert(id.to_owned(), json!(location_code(part)))),
            _ => drop(locations.remove(id)),
        }
        self.save_view_customizations(&customizations);
        match self.panel.views.iter().find(|view| view.id == id) {
            Some(view) => (view.open)(self, window, cx),
            None => self.show_panel_container(id, cx),
        }
    }

    /// `openPaneComposite` where the container is: shows its part with the view and stores it as the last active one.
    /// A panel container's view moved to another container opens there (`viewsService.openView`).
    pub(super) fn open_panel_view(&mut self, id: &'static str, cx: &mut Context<Self>) {
        if let Some(view) = self.moved_panel_view(id) {
            self.open_moved_view(view, cx);
            return;
        }
        self.show_panel_container(id, cx);
    }

    /// The view of a built-in panel container when it is no longer in it.
    fn moved_panel_view(&self, container: &str) -> Option<&'static str> {
        let view = self.panel.views.iter().find(|view| view.id == container)?;
        let moved = self.views_in(container).iter().all(|in_container| in_container.id != view.view_id);
        moved.then_some(view.view_id)
    }

    pub(super) fn show_panel_container(&mut self, id: &'static str, cx: &mut Context<Self>) {
        let part = self.container_location(id);
        let key = match part {
            Part::AuxiliaryBar => {
                self.panel.auxiliary_active = Some(id);
                ACTIVE_AUXILIARY_BAR_KEY
            }
            _ => {
                self.panel.active = Some(id);
                ACTIVE_PANEL_KEY
            }
        };
        self.workspace_storage.lock().unwrap().set(key, id.to_string());
        if !self.is_part_visible(part) {
            self.set_part_hidden(part, false, cx);
        }
        self.editors_changed(cx);
    }

    /// `setPanelHidden(false)`: opens the last active view, or the default one.
    fn show_panel(&mut self, cx: &mut Context<Self>) {
        self.set_part_hidden(Part::Panel, false, cx);
        if let Some(id) = self.active_container_in(Part::Panel) {
            self.show_panel_container(id, cx);
        }
    }

    /// `isViewVisible`: the view's part shows and `id` is its active view.
    pub(super) fn panel_view_visible(&self, id: &str) -> bool {
        if let Some(view) = self.moved_panel_view(id) {
            return self.is_view_visible(view);
        }
        let part = self.view_container_part(id);
        self.is_part_visible(part) && self.active_container_in(part) == Some(id)
    }

    /// `viewsService.closeView`: the view's part hides when the view is its active one.
    pub(super) fn close_panel_view(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.panel_view_visible(id) {
            return;
        }
        let container = match self.moved_panel_view(id).and_then(|view| self.movable_views().into_iter().find(|movable| movable.id == view)) {
            Some(view) => self.view_container(&view),
            None => id,
        };
        match self.container_location(container) {
            Part::SideBar => self.set_part_hidden(Part::SideBar, true, cx),
            Part::AuxiliaryBar => self.set_part_hidden(Part::AuxiliaryBar, true, cx),
            _ => self.hide_panel(cx),
        }
    }

    /// `setPanelHidden(true)`: a maximized panel gives the editor back first.
    pub(super) fn hide_panel(&mut self, cx: &mut Context<Self>) {
        self.set_part_hidden(Part::Panel, true, cx);
        self.editors_changed(cx);
    }

    /// panelActions.ts `MoveViewsBetweenPanelsAction`: every container of `source` moves to `destination`, which shows,
    /// and opens `source`'s active one when `destination` had none.
    fn move_views_between_panels(&mut self, source: Part, destination: Part, cx: &mut Context<Self>) {
        let moving = self.part_containers(source);
        if moving.is_empty() {
            return;
        }
        let active = self.is_part_visible(source).then(|| self.active_container_in(source)).flatten();
        let destination_was_empty = self.part_containers(destination).is_empty();
        let mut customizations = self.view_customizations();
        let locations = customizations["viewContainerLocations"].as_object_mut().expect("normalized");
        for tab in moving {
            // Only moved containers keep an entry (`saveViewCustomizations` skips the default location).
            match destination == Part::Panel && !is_generated_container(tab.id) {
                true => drop(locations.remove(tab.id)),
                false => drop(locations.insert(tab.id.to_owned(), json!(location_code(destination)))),
            }
        }
        self.save_view_customizations(&customizations);
        // paneCompositePart.ts: a part left without containers hides.
        self.set_part_hidden(source, true, cx);
        self.set_part_hidden(destination, false, cx);
        match active.filter(|_| destination_was_empty) {
            Some(id) => self.show_panel_container(id, cx),
            None => self.editors_changed(cx),
        }
    }

    /// `getActivePaneComposite(ViewContainerLocation.Panel)`, while the panel shows.
    pub(super) fn active_panel_id(&self) -> Option<&'static str> {
        self.is_part_visible(Part::Panel).then(|| self.active_container_in(Part::Panel)).flatten()
    }

    /// The containers with a tab in the part's composite bar: the built-in ones by registration order while their
    /// view is in them, then the generated ones.
    pub(super) fn part_tabs(&self, part: Part) -> Vec<CompositeTab> {
        self.part_containers(part).into_iter().filter(|tab| self.panel.views.iter().find(|view| view.view_id == tab.view).is_none_or(|view| (view.visible)(self))).collect()
    }

    /// The part's containers that hold a view, shown or not.
    fn part_containers(&self, part: Part) -> Vec<CompositeTab> {
        let mut tabs = Vec::new();
        for view in &self.panel.views {
            if self.view_container_part(view.id) == part && self.moved_panel_view(view.id).is_none() {
                tabs.push(CompositeTab { id: view.id, title: view.title, view: view.view_id });
            }
        }
        for id in self.generated_containers_in(part) {
            let view = self.generated_container_view(id).expect("a container with a view");
            tabs.push(CompositeTab { id, title: view.name, view: view.id });
        }
        tabs
    }

    /// The part's last active view, else its default: Terminal in the panel (terminal.contribution.ts registers it
    /// with `isDefault`), the first container in the secondary side bar.
    pub(super) fn active_container_in(&self, part: Part) -> Option<&'static str> {
        let (active, key) = match part {
            Part::AuxiliaryBar => (self.panel.auxiliary_active, ACTIVE_AUXILIARY_BAR_KEY),
            _ => (self.panel.active, ACTIVE_PANEL_KEY),
        };
        let stored = self.workspace_storage.lock().unwrap().get(key).map(str::to_owned);
        let id = active.map(str::to_owned).or(stored);
        let containers = self.part_containers(part);
        let default = match part {
            Part::AuxiliaryBar => containers.first(),
            _ => containers.iter().find(|tab| tab.id == super::terminal_view::TERMINAL_VIEW_ID),
        };
        containers.iter().find(|tab| Some(tab.id) == id.as_deref()).or(default).map(|tab| tab.id)
    }

    /// panelpart.css: a left, right or top panel turns its split and maximize icons to face the editor.
    pub(super) fn panel_title_icon_turns(&self) -> f32 {
        match self.arrangement().panel {
            Position::Bottom => 0.,
            Position::Right => -0.25,
            Position::Left => 0.25,
            Position::Top => 0.5,
        }
    }

    /// The panel card: the views' tabs, maximize and close, and the active view.
    pub(super) fn panel_part(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let (maximize_icon, maximize_title) = match self.is_panel_maximized() {
            true => ("screen-normal", "Restore Panel Size"),
            false => ("screen-full", "Maximize Panel Size"),
        };
        let arrangement = self.arrangement();
        // panelActions.ts `panelMaximizationSupportedWhen`.
        let maximizes = arrangement.alignment == Alignment::Center || !arrangement.panel.is_horizontal();
        let mut actions = vec![(maximize_icon, maximize_title, "workbench.action.toggleMaximizedPanel", false), ("close", "Hide Panel", "workbench.action.closePanel", false)];
        if !maximizes {
            actions.remove(0);
        }
        self.pane_composite_part(Part::Panel, actions, window, cx)
    }

    /// The secondary side bar card: `MenuId.AuxiliaryBarTitle`'s Toggle Maximized Secondary Side Bar (toggled: the
    /// other tooltip and the toolbar's `highlightToggledItems` background), then Hide Secondary Side Bar.
    pub(super) fn auxiliary_bar_part(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let maximized = self.is_auxiliary_bar_maximized();
        let maximize_title = if maximized { "Restore Secondary Side Bar" } else { "Maximize Secondary Side Bar" };
        let actions = vec![("screen-full", maximize_title, "workbench.action.toggleMaximizedAuxiliaryBar", maximized), ("close", "Hide Secondary Side Bar", "workbench.action.closeAuxiliaryBar", false)];
        self.pane_composite_part(Part::AuxiliaryBar, actions, window, cx)
    }

    /// paneCompositePart.ts: the tabs that fit and the "..." item, the active view's actions and the part's global
    /// actions over the active view, or the empty message. Dropping a dragged tab anywhere on the part moves its
    /// container here; over a part without an active view the title and the message take `editorGroup.dropBackground`.
    fn pane_composite_part(&mut self, part: Part, actions: Vec<GlobalAction>, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let active = self.active_container_in(part);
        let shown = self.tabs_that_fit(part, active, window);
        self.composite_bar(part).actions_view.set(active);
        let background = if part == Part::AuxiliaryBar { "sideBar.background" } else { "panel.background" };
        let mut tabs: Vec<AnyElement> = Vec::new();
        let mut overflowing = Vec::new();
        for tab in self.part_tabs(part) {
            match shown.contains(&tab.id) {
                true => tabs.push(self.panel_tab(&tab, Some(tab.id) == active, background, window, cx)),
                false => overflowing.push(MenuAction::Command { id: format!("{}.focus", tab.view), title: panel_tab_label(&tab, self), enabled: true, checked: Some(tab.id) == active, alt: None }),
            }
        }
        if !overflowing.is_empty() {
            tabs.push(self.panel_overflow_action(part, overflowing, cx));
        }
        let drop_feedback = active.is_none() && self.panel.drop_target == Some(part);
        let (content, view_actions) = match active {
            Some(container) => self.panel_container_content(container, window, cx),
            None => (self.empty_pane_message(drop_feedback).into_any_element(), None),
        };
        let content_bounds = self.composite_bar(part).content_bounds.clone();
        self.pane_composite_card(background)
            .on_drag_move(cx.listener(move |workbench, event: &DragMoveEvent<DraggedComposite>, _, cx| {
                let over = event.bounds.contains(&event.event.position);
                let target = if over { Some(part) } else { workbench.panel.drop_target.filter(|target| *target != part) };
                if workbench.panel.drop_target != target {
                    workbench.panel.drop_target = target;
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(move |workbench, dragged: &DraggedComposite, window, cx| {
                workbench.panel.drop_target = None;
                workbench.move_view_container(dragged.id, part, window, cx);
            }))
            .child(self.pane_composite_title(part, tabs, view_actions, actions, drop_feedback, cx).on_drop(cx.listener(move |workbench, dragged: &super::view_panes::DraggedPane, _, cx| {
                workbench.drop_view_on_location(dragged.id, part, cx);
            })))
            .child(div().relative().flex_1().min_h_0().flex().flex_col().child(size_tracker(content_bounds)).child(content))
    }

    /// The container's content and title actions: a single view merged into it (its body, its actions in the
    /// composite title), several as panes.
    fn panel_container_content(&mut self, container: &'static str, window: &mut Window, cx: &mut Context<Self>) -> (AnyElement, Option<AnyElement>) {
        let views: Vec<&'static str> = self.views_in(container).into_iter().filter(|view| self.movable_view_shown(view)).map(|view| view.id).collect();
        if let [view] = views[..] {
            let pane = self.movable_view_pane(view, window, cx);
            return (pane.body, pane.actions);
        }
        // viewPaneContainer.ts `orientation`: a top or bottom panel lays its panes side by side.
        let part = self.container_location(container);
        let horizontal = part == Part::Panel && self.arrangement().panel.is_horizontal();
        let content = self.composite_bar(part).content_bounds.get().size;
        let size = f32::from(if horizontal { content.width } else { content.height });
        let generated = super::view_panes::PaneContainer::Generated(container);
        self.layout_panes(generated, size, views.iter().map(|&view| (view, false)).collect(), horizontal, cx);
        let panes = views.into_iter().map(|view| self.movable_view_pane(view, window, cx)).collect();
        let ordered = self.ordered_panes(&self.panes(generated).view.as_ref().expect("laid out above").ids(), panes, window, cx);
        (self.pane_view_element(generated, ordered, window, cx).into_any_element(), None)
    }

    fn composite_bar(&self, part: Part) -> &CompositeBar {
        match part {
            Part::AuxiliaryBar => &self.panel.auxiliary_bar,
            _ => &self.panel.panel_bar,
        }
    }

    fn pane_composite_card(&self, background: &str) -> Div {
        div().flex_1().min_w_0().flex().flex_col().overflow_hidden().border_1().rounded(px(8.)).bg(self.color(background)).border_color(self.color("surface.border"))
    }

    /// `.composite.title`: the tab strip's background with its bottom line, tabs on the left, then the active view's
    /// title actions, a separator (actionbar.css `.separator`) and the global actions on the right.
    fn pane_composite_title(&self, part: Part, tabs: Vec<AnyElement>, view_actions: Option<AnyElement>, actions: Vec<GlobalAction>, drop_feedback: bool, cx: &mut Context<Self>) -> Div {
        let foreground = self.color("icon.foreground");
        let bottom_line = div().absolute().bottom_0().left_0().right_0().h(px(1.)).bg(self.color("surface.border"));
        let bar = self.composite_bar(part);
        let background = if drop_feedback { self.color("editorGroup.dropBackground") } else { self.tab_strip_background() };
        div()
            .relative()
            .h(px(TITLE_HEIGHT))
            .flex_none()
            .flex()
            .pr(px(4.))
            .bg(background)
            .child(bottom_line)
            .child(size_tracker(bar.title_bounds.clone()))
            .children(tabs)
            // The actions keep their width: the tab layout measures them, so they must not shrink for tabs that overflow.
            .child(div().relative().flex_none().ml_auto().h_full().flex().items_center().gap(px(4.)).child(size_tracker(bar.actions_bounds.clone())).when_some(view_actions, |bar, view_actions| {
                // The bar's 4px gap is the separator's right margin.
                bar.child(view_actions).child(div().w(px(1.)).h(px(16.)).ml(px(4.)).bg(self.color("disabledForeground")))
            }).children(actions.into_iter().map(|(icon, title, command, toggled)| {
                let toggled_background = self.theme.color("actionBar.toggledBackground").unwrap_or(self.color("inputOption.activeBackground"));
                self.icon(icon, 22., 16., foreground).id(command).rounded(px(4.)).cursor_pointer().when(toggled, |action| action.bg(toggled_background)).child(self.action_hover(title, command, HoverOptions::managed())).on_click(cx.listener(
                    move |workbench, _, window, cx| {
                        workbench.execute_command(command, Vec::new(), window, cx);
                    },
                ))
            })))
    }

    /// A composite bar tab (`.action-item` with 16px padding); the checked one is the part's own fill, connected to
    /// the content. Its width is the fractional one Chromium lays out: GPUI rounds a text element's width up, which
    /// made the Problems tab with its badge 2px wider than the reference's.
    fn panel_tab(&self, view: &CompositeTab, active: bool, background: &str, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let foreground = if active { self.foreground_or("panelTitle.activeForeground") } else { self.foreground_or("panelTitle.inactiveForeground") };
        let id = view.id;
        let dragged = DraggedComposite { id, label: view.title.into(), foreground, background: self.color(background) };
        let badge = self.tab_badge(view).map(|count| {
            let text = count.to_string();
            let text_width = text_width(&text, 10., window);
            let width = badge_width(text_width);
            // paneCompositePart.css `.badge` 2px from the label, `.badge-content` 10px text centered with 3px 5px padding.
            div()
                .ml(px(2.))
                .flex_none()
                .w(px(width))
                .h(px(16.))
                .pl(px((width - text_width) / 2.))
                .flex()
                .items_center()
                .rounded(px(8.))
                .text_size(px(10.))
                .whitespace_nowrap()
                .bg(self.color("activityBarBadge.background"))
                .text_color(self.foreground_or("activityBarBadge.foreground"))
                .child(text)
        });
        div()
            .id(id)
            .relative()
            .h_full()
            .flex_none()
            .w(px(panel_tab_width(view, self, window)))
            .pl(px(16.))
            .flex()
            .items_center()
            .cursor_pointer()
            .text_color(foreground)
            .when(active, |tab| tab.bg(self.color(background)).rounded_t(px(7.)).border_t_1().border_r_1().border_color(self.color("surface.border")))
            .child(div().flex_none().w(px(text_width(view.title, 13., window))).whitespace_nowrap().line_height(px(24.)).child(view.title))
            .children(badge)
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            // compositeBar.ts: a view dropped on a tab moves into that container.
            .on_drop(cx.listener(move |workbench, dragged: &super::view_panes::DraggedPane, _, cx| {
                cx.stop_propagation();
                workbench.drop_view_on_container(dragged.id, id, cx);
            }))
            .on_click(cx.listener(move |workbench, _, window, cx| match workbench.panel.views.iter().find(|view| view.id == id) {
                Some(view) => (view.open)(workbench, window, cx),
                None => workbench.show_panel_container(id, cx),
            }))
            .into_any_element()
    }

    /// compositeBar.ts `updateCompositeSwitcher`: the tabs that fit before the title actions (the active one kept,
    /// room made for the overflow action), on the sizes painted last frame; every tab before the first paint. The tabs
    /// are laid out again when the title's width, the tabs or the active view change, and when the title actions grow
    /// (a started terminal's actions); not when they shrink, as the reference (View as Table drops Collapse All without
    /// bringing Terminal back).
    fn tabs_that_fit(&self, part: Part, active: Option<&'static str>, window: &Window) -> Vec<&'static str> {
        let tabs: Vec<(&'static str, f32)> = self.part_tabs(part).into_iter().map(|view| (view.id, panel_tab_width(&view, self, window))).collect();
        let bar = self.composite_bar(part);
        let title = bar.title_bounds.get().size.width;
        if title <= px(0.) {
            return tabs.into_iter().map(|(id, _)| id).collect();
        }
        // paneCompositePart.ts `layoutCompositeBar`: 16px title padding, the toolbars and their 8px padding. The filter
        // box counts at its 200px resting width: the reference lays the tabs out on resize, not when the box grows on focus.
        let filter = f32::from(bar.filter_bounds.replace(Bounds::default()).size.width);
        let filter_basis = if filter > 0. { FILTER_MAX_WIDTH } else { 0. };
        let other_actions = f32::from(bar.actions_bounds.get().size.width) - filter;
        let key = (title, tabs.iter().map(|(id, _)| *id).collect(), active, filter > 0.);
        let cached = bar.tab_layout.borrow().as_ref().filter(|(cached, actions, _)| *cached == key && other_actions <= *actions + 0.5).map(|(_, _, shown)| shown.clone());
        let shown = match cached {
            Some(shown) => shown,
            None => {
                let limit = f32::from(title) - 16. - (other_actions + filter_basis) - 8.;
                let shown = fitting_tabs(&tabs, active, limit.max(MIN_COMPOSITE_BAR_WIDTH), overflow_action_size(part));
                // The first frames paint the actions before they are measured, and a newly active view's actions a frame later.
                if other_actions > 0. && bar.actions_view.get() == active {
                    *bar.tab_layout.borrow_mut() = Some((key, other_actions, shown.clone()));
                }
                shown
            }
        };
        let tabs_width: f32 = tabs.iter().filter(|(id, _)| shown.contains(id)).map(|(_, width)| width).sum();
        bar.filter_room.set(filter_room(f32::from(title), tabs_width, shown.len() < tabs.len(), other_actions));
        shown
    }

    /// viewPane.ts `FilterViewPane.shouldShowFilterInHeader`: the filter leaves the title for the top of the view
    /// when the view is narrower than 600px and taller than 100px; before the first paint it is in the title.
    pub(super) fn view_filter_in_header(&self, container: &str) -> bool {
        let size = self.filter_view_size(container);
        !(size.width < px(600.) && size.height > px(100.))
    }

    /// viewFilter.ts `layout`: below 400px of view width the filter is `.small` and hides its badge.
    pub(super) fn view_filter_small(&self, container: &str) -> bool {
        let width = self.filter_view_size(container).width;
        width > px(0.) && width < px(400.)
    }

    /// The size of the panel container's view: the part's content, or its pane's share of it when the view lies
    /// among other views of a panel container.
    fn filter_view_size(&self, container: &str) -> gpui::Size<Pixels> {
        let mut size = self.composite_bar(self.view_container_part(container)).content_bounds.get().size;
        let Some(view) = self.panel.views.iter().find(|view| view.id == container).map(|view| view.view_id) else { return size };
        let Some(holder) = self.movable_view(view).map(|movable| self.view_container(&movable)) else { return size };
        if self.views_in(holder).iter().filter(|view| self.movable_view_shown(view)).count() < 2 {
            return size;
        }
        let Some(panes) = self.generated_panes.get(holder).and_then(|panes| panes.view.as_ref()) else { return size };
        let Some(index) = panes.ids().iter().position(|id| *id == view) else { return size };
        let pane = panes.sizes()[index];
        match panes.is_horizontal() {
            true => size.width = px(pane),
            false => size.height = px(pane - super::view_panes::HEADER_SIZE),
        }
        size
    }

    /// views.css `.viewpane-filter-container` in a panel title: 200px, and while focused up to 400px of the room the
    /// tabs leave.
    pub(super) fn panel_filter_width(&self, container: &str, focused: bool) -> f32 {
        match focused {
            true => self.composite_bar(self.view_container_part(container)).filter_room.get().clamp(FILTER_MIN_WIDTH, 400.),
            false => FILTER_MAX_WIDTH,
        }
    }

    /// Measures a view's filter box in the title for the tab layout.
    pub(super) fn panel_filter_tracker(&self, container: &str) -> impl IntoElement {
        super::output_view::bounds_tracker(self.composite_bar(self.view_container_part(container)).filter_bounds.clone())
    }

    /// compositeBarActions.ts `CompositeOverflowActivityActionViewItem`: the "..." item listing the tabs that do not fit.
    fn panel_overflow_action(&self, part: Part, actions: Vec<MenuAction>, cx: &mut Context<Self>) -> AnyElement {
        let bounds = self.composite_bar(part).overflow_bounds.clone();
        let foreground = self.foreground_or("panelTitle.inactiveForeground");
        div()
            .id("additionalComposites.action")
            .relative()
            .h_full()
            .px(px(3.))
            .flex()
            .items_center()
            .cursor_pointer()
            .child(self.icon("ellipsis", 22., 16., foreground))
            .child(self.action_hover("Additional Views", "additionalComposites.action", HoverOptions::instant()))
            .child(super::output_view::bounds_tracker(bounds.clone()))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                workbench.show_context_menu_actions(actions.clone(), &Default::default(), bounds.get(), Vec::new(), window, cx);
            }))
            .into_any_element()
    }

    /// paneCompositePart.ts `.empty-pane-message-area`, in `editorGroup.dropBackground` while a tab is dragged over the part.
    fn empty_pane_message(&self, drop_feedback: bool) -> Div {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(self.foreground_or("foreground"))
            .when(drop_feedback, |message| message.bg(self.color("editorGroup.dropBackground")))
            .child("Drag a view here to display.")
    }
}

/// A container's tab: its id, title and the view whose name and badge it shows.
pub(super) struct CompositeTab {
    pub id: &'static str,
    pub title: &'static str,
    view: &'static str,
}

impl Workbench {
    /// The activity count of the tab's view (`IActivity.badge`).
    fn tab_badge(&self, tab: &CompositeTab) -> Option<usize> {
        let view = self.panel.views.iter().find(|view| view.view_id == tab.view)?;
        (view.badge)(self).filter(|count| *count > 0)
    }
}

/// compositeBarActions.ts `getActions`: a tab's name with its number badge, "{0} ({1})".
fn panel_tab_label(view: &CompositeTab, workbench: &Workbench) -> String {
    match workbench.tab_badge(view) {
        Some(count) => format!("{} ({count})", view.title),
        None => view.title.to_string(),
    }
}

/// The width `panel_tab` paints: 16px padding each side, the title, and the badge 2px after it.
fn panel_tab_width(view: &CompositeTab, workbench: &Workbench, window: &Window) -> f32 {
    let badge = workbench.tab_badge(view).map_or(0., |count| 2. + badge_width(text_width(&count.to_string(), 10., window)));
    32. + text_width(view.title, 13., window) + badge
}

/// `.badge-content`: `min-width: 16px` around the text and its 5px side padding.
fn badge_width(text_width: f32) -> f32 {
    (text_width + 10.).max(16.)
}

/// panelPart.ts and auxiliaryBarPart.ts `overflowActionSize` with the composite bar in the title.
fn overflow_action_size(part: Part) -> f32 {
    match part {
        Part::AuxiliaryBar => 40.,
        _ => 44.,
    }
}

fn text_width(text: &str, size: f32, window: &Window) -> f32 {
    let run = TextRun { len: text.len(), font: window.text_style().font(), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
    f32::from(window.text_system().shape_line(text.to_string().into(), px(size), &[run], None).width)
}

/// compositeBar.ts `updateCompositeSwitcher` on `(id, width)` tabs in order.
fn fitting_tabs(tabs: &[(&'static str, f32)], active: Option<&'static str>, limit: f32, overflow_action_size: f32) -> Vec<&'static str> {
    let width = |id: &str| tabs.iter().find(|(tab, _)| *tab == id).map_or(0., |(_, width)| *width);
    let mut size = 0.;
    let mut shown = Vec::new();
    for (id, width) in tabs {
        if size + width > limit {
            break;
        }
        size += width;
        shown.push(*id);
    }
    if let Some(active) = active.filter(|active| tabs.iter().any(|(id, _)| id == active) && !shown.contains(active)) {
        size += width(active);
        shown.push(active);
    }
    while size > limit && !shown.is_empty() {
        let removed = if shown.len() > 1 { shown.remove(shown.len() - 2) } else { shown.pop().unwrap() };
        size -= width(removed);
    }
    if tabs.len() > shown.len() {
        size += overflow_action_size;
    }
    while size > limit && !shown.is_empty() {
        let removed = if shown.len() > 1 && shown.last().copied() == active { shown.remove(shown.len() - 2) } else { shown.pop().unwrap() };
        size -= width(removed);
    }
    shown
}

/// Records the painted bounds and draws again when the size changed, so a layout reading them catches up at once.
fn size_tracker(bounds: Rc<Cell<Bounds<Pixels>>>) -> impl IntoElement {
    gpui::canvas(
        move |painted, window, _| {
            if bounds.get().size != painted.size {
                window.refresh();
            }
            bounds.set(painted);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

/// The room of a focused filter box (`.grow`) once the DOM laid out the shown tabs, the 28px "..." item,
/// `.title-actions`' 4px left padding, the other title actions and the title's 4px right padding.
fn filter_room(title: f32, tabs: f32, overflow: bool, other_actions: f32) -> f32 {
    let overflow = if overflow { OVERFLOW_ITEM_WIDTH } else { 0. };
    title - tabs - overflow - 4. - other_actions - 4.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_focused_filter_takes_the_room_the_tabs_leave() {
        // The reference at 1024x768: a 710px title, Problems, Output and Debug Console, the "..." item, the filter's
        // 4px margin, two actions, the separator and the global actions (port/results/problems.md).
        assert_eq!(filter_room(710., 306., true, 113.), 255.);
    }

    const TABS: [(&str, f32); 4] = [("problems", 100.), ("output", 80.), ("debug", 120.), ("terminal", 90.)];

    #[test]
    fn every_tab_shows_when_they_fit() {
        assert_eq!(fitting_tabs(&TABS, Some("problems"), 390., 44.), ["problems", "output", "debug", "terminal"]);
    }

    #[test]
    fn the_tail_overflows_with_room_for_the_overflow_action() {
        // 300 fits three tabs, the overflow action's 44px pushes the third out.
        assert_eq!(fitting_tabs(&TABS, Some("problems"), 300., 44.), ["problems", "output"]);
    }

    #[test]
    fn the_active_tab_stays_and_the_one_before_it_goes() {
        assert_eq!(fitting_tabs(&TABS, Some("terminal"), 300., 44.), ["problems", "terminal"]);
    }
}
