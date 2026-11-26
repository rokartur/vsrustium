//! The panel and the secondary side bar (panelPart.ts, auxiliaryBarPart.ts, paneCompositePart.ts): floating cards
//! with a composite title (the views' tabs, the active view's actions, maximize and close) over the active view.
//! Panel views register here with `register_panel_view`; the secondary side bar has no views yet and shows its
//! empty message.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{AnyElement, Bounds, Context, Div, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, StatefulInteractiveElement, Styled, TextRun, Window, div, prelude::FluentBuilder, px};

use super::Workbench;
use super::hover::HoverOptions;
use super::layout::Part;
use crate::commands::Outcome;
use crate::menus::MenuAction;

/// paneCompositePart.ts `ACTIVE_PANEL_SETTINGS_KEY` of the panel, in workspace storage.
const ACTIVE_PANEL_KEY: &str = "workbench.panelpart.activepanelid";
/// The composite title's height (`--vscode-workbench-titleHeight` of a pane composite part).
const TITLE_HEIGHT: f32 = 32.;
/// panelPart.ts `overflowActionSize`.
const OVERFLOW_ACTION_SIZE: f32 = 44.;
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

#[derive(Default)]
pub(super) struct Panel {
    views: Vec<PanelView>,
    active: Option<&'static str>,
    /// The tab strip and its right-hand actions as last painted, for the overflow layout.
    title_bounds: Rc<Cell<Bounds<Pixels>>>,
    actions_bounds: Rc<Cell<Bounds<Pixels>>>,
    overflow_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The active view's filter box as last painted, empty without one.
    filter_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The width the tabs leave free beside a 200px filter box.
    filter_room: Cell<f32>,
    /// The shown tabs and what they were laid out for.
    tab_layout: RefCell<Option<(TabLayoutKey, Vec<&'static str>)>>,
}

/// The title width, the tabs, the active view and whether it has a filter box.
type TabLayoutKey = (Pixels, Vec<&'static str>, Option<&'static str>, bool);

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
        self.commands.register("workbench.action.toggleMaximizedPanel", |workbench, _, _, cx| {
            let maximize = !workbench.is_panel_maximized();
            if maximize && !workbench.is_part_visible(Part::Panel) {
                workbench.show_panel(cx);
            }
            workbench.set_editor_hidden(maximize, cx);
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
    }

    /// `openPaneComposite` in the panel: shows the panel with the view and stores it as the last active one.
    pub(super) fn open_panel_view(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.panel.active = Some(id);
        self.workspace_storage.lock().unwrap().set(ACTIVE_PANEL_KEY, id.to_string());
        if !self.is_part_visible(Part::Panel) {
            self.set_part_hidden(Part::Panel, false, cx);
        }
        self.editors_changed(cx);
    }

    /// `setPanelHidden(false)`: opens the last active view, or the first registered one.
    fn show_panel(&mut self, cx: &mut Context<Self>) {
        self.set_part_hidden(Part::Panel, false, cx);
        if let Some(id) = self.active_panel_view().map(|view| view.id) {
            self.open_panel_view(id, cx);
        }
    }

    /// `isViewVisible`: the panel shows and `id` is its active view.
    pub(super) fn panel_view_visible(&self, id: &str) -> bool {
        self.is_part_visible(Part::Panel) && self.active_panel_view().is_some_and(|view| view.id == id)
    }

    /// `viewsService.closeView`: the panel hides when the view is its active one.
    pub(super) fn close_panel_view(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.panel_view_visible(id) {
            self.hide_panel(cx);
        }
    }

    /// `setPanelHidden(true)`: a maximized panel gives the editor back first.
    pub(super) fn hide_panel(&mut self, cx: &mut Context<Self>) {
        if self.is_panel_maximized() {
            self.set_editor_hidden(false, cx);
        }
        self.set_part_hidden(Part::Panel, true, cx);
        self.editors_changed(cx);
    }

    /// `getActivePaneComposite(ViewContainerLocation.Panel)`, while the panel shows.
    pub(super) fn active_panel_id(&self) -> Option<&'static str> {
        self.is_part_visible(Part::Panel).then(|| self.active_panel_view().map(|view| view.id)).flatten()
    }

    /// The views with a tab in the panel's composite bar, by registration order.
    pub(super) fn panel_tabs(&self) -> Vec<&PanelView> {
        self.panel.views.iter().filter(|view| (view.visible)(self)).collect()
    }

    pub(super) fn active_panel_view(&self) -> Option<&PanelView> {
        let storage = self.workspace_storage.lock().unwrap();
        let id = self.panel.active.or(storage.get(ACTIVE_PANEL_KEY));
        // terminal.contribution.ts registers the Terminal container with `isDefault`.
        let default = super::terminal_view::TERMINAL_VIEW_ID;
        self.panel.views.iter().find(|view| Some(view.id) == id).or(self.panel.views.iter().find(|view| view.id == default))
    }

    /// The panel card: the views' tabs, maximize and close, and the active view.
    pub(super) fn panel_part(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let active = self.active_panel_view().map(|view| view.id);
        let shown = self.panel_tabs_that_fit(active, window);
        let mut tabs: Vec<AnyElement> = Vec::new();
        let mut overflowing = Vec::new();
        for view in self.panel_tabs() {
            match shown.contains(&view.id) {
                true => tabs.push(self.panel_tab(view, Some(view.id) == active, cx)),
                false => overflowing.push(MenuAction::Command { id: format!("{}.focus", view.view_id), title: panel_tab_label(view, self), enabled: true, checked: Some(view.id) == active, alt: None }),
            }
        }
        if !overflowing.is_empty() {
            tabs.push(self.panel_overflow_action(overflowing, cx));
        }
        let (maximize_icon, maximize_title) = match self.is_panel_maximized() {
            true => ("screen-normal", "Restore Panel Size"),
            false => ("screen-full", "Maximize Panel Size"),
        };
        let actions = vec![(maximize_icon, maximize_title, "workbench.action.toggleMaximizedPanel"), ("close", "Hide Panel", "workbench.action.closePanel")];
        let view = self.active_panel_view().map(|view| (view.render, view.actions));
        let view_actions = view.map(|(_, actions)| actions(self, window, cx));
        let content = match view {
            Some((render, _)) => render(self, window, cx),
            None => self.empty_pane_message().into_any_element(),
        };
        self.pane_composite_card("panel.background")
            .child(self.pane_composite_title(tabs, view_actions, actions, cx))
            .child(div().flex_1().min_h_0().flex().flex_col().child(content))
    }

    /// The secondary side bar card with no views. shortcut: its Maximize action waits for `toggleMaximizedAuxiliaryBar`, item 2.
    pub(super) fn auxiliary_bar_part(&self, cx: &mut Context<Self>) -> Div {
        let actions = vec![("close", "Hide Secondary Side Bar", "workbench.action.closeAuxiliaryBar")];
        self.pane_composite_card("sideBar.background").child(self.pane_composite_title(Vec::new(), None, actions, cx)).child(self.empty_pane_message())
    }

    fn pane_composite_card(&self, background: &str) -> Div {
        div().flex_1().min_w_0().flex().flex_col().overflow_hidden().border_1().rounded(px(8.)).bg(self.color(background)).border_color(self.color("surface.border"))
    }

    /// `.composite.title`: the tab strip's background with its bottom line, tabs on the left, then the active view's
    /// title actions, a separator (actionbar.css `.separator`) and the global actions on the right.
    fn pane_composite_title(&self, tabs: Vec<AnyElement>, view_actions: Option<AnyElement>, actions: Vec<(&'static str, &'static str, &'static str)>, cx: &mut Context<Self>) -> Div {
        let foreground = self.color("icon.foreground");
        let bottom_line = div().absolute().bottom_0().left_0().right_0().h(px(1.)).bg(self.color("surface.border"));
        div()
            .relative()
            .h(px(TITLE_HEIGHT))
            .flex_none()
            .flex()
            .pr(px(4.))
            .bg(self.tab_strip_background())
            .child(bottom_line)
            .child(size_tracker(self.panel.title_bounds.clone()))
            .children(tabs)
            .child(div().relative().ml_auto().h_full().flex().items_center().gap(px(4.)).child(size_tracker(self.panel.actions_bounds.clone())).when_some(view_actions, |bar, view_actions| {
                // The bar's 4px gap is the separator's right margin.
                bar.child(view_actions).child(div().w(px(1.)).h(px(16.)).ml(px(4.)).bg(self.color("disabledForeground")))
            }).children(actions.into_iter().map(|(icon, title, command)| {
                self.icon(icon, 22., 16., foreground).id(command).rounded(px(4.)).cursor_pointer().child(self.action_hover(title, command, HoverOptions::instant())).on_click(cx.listener(
                    move |workbench, _, window, cx| {
                        workbench.execute_command(command, Vec::new(), window, cx);
                    },
                ))
            })))
    }

    /// A composite bar tab (`.action-item` with 16px padding); the checked one is the panel's own fill, connected to the content.
    fn panel_tab(&self, view: &PanelView, active: bool, cx: &mut Context<Self>) -> AnyElement {
        let foreground = if active { self.foreground_or("panelTitle.activeForeground") } else { self.foreground_or("panelTitle.inactiveForeground") };
        let id = view.id;
        div()
            .id(id)
            .relative()
            .h_full()
            .px(px(16.))
            .flex()
            .items_center()
            .cursor_pointer()
            .text_color(foreground)
            .when(active, |tab| tab.bg(self.color("panel.background")).rounded_t(px(7.)).border_t_1().border_r_1().border_color(self.color("surface.border")))
            .child(div().line_height(px(24.)).child(view.title))
            .children((view.badge)(self).filter(|count| *count > 0).map(|count| {
                // paneCompositePart.css `.badge` 2px from the label, `.badge-content` 10px text with 3px 5px padding.
                div()
                    .ml(px(2.))
                    .min_w(px(16.))
                    .h(px(16.))
                    .px(px(5.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.))
                    .text_size(px(10.))
                    .bg(self.color("activityBarBadge.background"))
                    .text_color(self.foreground_or("activityBarBadge.foreground"))
                    .child(count.to_string())
            }))
            .on_click(cx.listener(move |workbench, _, window, cx| {
                let view = workbench.panel.views.iter().find(|view| view.id == id).expect("a registered panel view");
                (view.open)(workbench, window, cx);
            }))
            .into_any_element()
    }

    /// compositeBar.ts `updateCompositeSwitcher`: the tabs that fit before the title actions (the active one kept,
    /// room made for the overflow action), on the sizes painted last frame; every tab before the first paint. As the
    /// reference, the tabs are laid out again only when the title's width, the tabs or the active view change, not
    /// when the title actions do (View as Table drops Collapse All without bringing Terminal back).
    fn panel_tabs_that_fit(&self, active: Option<&'static str>, window: &Window) -> Vec<&'static str> {
        let tabs: Vec<(&'static str, f32)> = self.panel_tabs().into_iter().map(|view| (view.id, panel_tab_width(view, self, window))).collect();
        let title = self.panel.title_bounds.get().size.width;
        if title <= px(0.) {
            return tabs.into_iter().map(|(id, _)| id).collect();
        }
        // paneCompositePart.ts `layoutCompositeBar`: 16px title padding, the toolbars and their 8px padding. The filter
        // box counts at its 200px resting width: the reference lays the tabs out on resize, not when the box grows on focus.
        let filter = f32::from(self.panel.filter_bounds.replace(Bounds::default()).size.width);
        let filter_basis = if filter > 0. { FILTER_MAX_WIDTH } else { 0. };
        let other_actions = f32::from(self.panel.actions_bounds.get().size.width) - filter;
        let key = (title, tabs.iter().map(|(id, _)| *id).collect(), active, filter > 0.);
        let cached = self.panel.tab_layout.borrow().as_ref().filter(|(cached, _)| *cached == key).map(|(_, shown)| shown.clone());
        let shown = match cached {
            Some(shown) => shown,
            None => {
                let limit = f32::from(title) - 16. - (other_actions + filter_basis) - 8.;
                let shown = fitting_tabs(&tabs, active, limit.max(MIN_COMPOSITE_BAR_WIDTH));
                // The first frames paint the actions before they are measured.
                if other_actions > 0. {
                    *self.panel.tab_layout.borrow_mut() = Some((key, shown.clone()));
                }
                shown
            }
        };
        let tabs_width: f32 = tabs.iter().filter(|(id, _)| shown.contains(id)).map(|(_, width)| width).sum();
        self.panel.filter_room.set(filter_room(f32::from(title), tabs_width, shown.len() < tabs.len(), other_actions));
        shown
    }

    /// views.css `.viewpane-filter-container` in a panel title: 200px, and while focused up to 400px of the room the
    /// tabs leave.
    pub(super) fn panel_filter_width(&self, focused: bool) -> f32 {
        match focused {
            true => self.panel.filter_room.get().clamp(FILTER_MIN_WIDTH, 400.),
            false => FILTER_MAX_WIDTH,
        }
    }

    /// Measures a view's filter box in the title for the tab layout.
    pub(super) fn panel_filter_tracker(&self) -> impl IntoElement {
        super::output_view::bounds_tracker(self.panel.filter_bounds.clone())
    }

    /// compositeBarActions.ts `CompositeOverflowActivityActionViewItem`: the "..." item listing the tabs that do not fit.
    fn panel_overflow_action(&self, actions: Vec<MenuAction>, cx: &mut Context<Self>) -> AnyElement {
        let bounds = self.panel.overflow_bounds.clone();
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

    /// paneCompositePart.ts `.empty-pane-message-area`.
    fn empty_pane_message(&self) -> Div {
        div().flex_1().flex().items_center().justify_center().text_color(self.foreground_or("foreground")).child("Drag a view here to display.")
    }
}

/// compositeBarActions.ts `getActions`: a tab's name with its number badge, "{0} ({1})".
fn panel_tab_label(view: &PanelView, workbench: &Workbench) -> String {
    match (view.badge)(workbench).filter(|count| *count > 0) {
        Some(count) => format!("{} ({count})", view.title),
        None => view.title.to_string(),
    }
}

/// The width `panel_tab` paints: 16px padding each side, the title, and the badge 3px after it.
fn panel_tab_width(view: &PanelView, workbench: &Workbench, window: &Window) -> f32 {
    let badge = (view.badge)(workbench).filter(|count| *count > 0).map_or(0., |count| 2. + (text_width(&count.to_string(), 10., window) + 10.).max(16.));
    32. + text_width(view.title, 13., window) + badge
}

fn text_width(text: &str, size: f32, window: &Window) -> f32 {
    let run = TextRun { len: text.len(), font: window.text_style().font(), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
    f32::from(window.text_system().shape_line(text.to_string().into(), px(size), &[run], None).width)
}

/// compositeBar.ts `updateCompositeSwitcher` on `(id, width)` tabs in order.
fn fitting_tabs(tabs: &[(&'static str, f32)], active: Option<&'static str>, limit: f32) -> Vec<&'static str> {
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
        size += OVERFLOW_ACTION_SIZE;
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
        assert_eq!(fitting_tabs(&TABS, Some("problems"), 390.), ["problems", "output", "debug", "terminal"]);
    }

    #[test]
    fn the_tail_overflows_with_room_for_the_overflow_action() {
        // 300 fits three tabs, the overflow action's 44px pushes the third out.
        assert_eq!(fitting_tabs(&TABS, Some("problems"), 300.), ["problems", "output"]);
    }

    #[test]
    fn the_active_tab_stays_and_the_one_before_it_goes() {
        assert_eq!(fitting_tabs(&TABS, Some("terminal"), 300.), ["problems", "terminal"]);
    }
}
