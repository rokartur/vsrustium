//! The panel and the secondary side bar (panelPart.ts, auxiliaryBarPart.ts, paneCompositePart.ts): floating cards
//! with a composite title (the views' tabs, the active view's actions, maximize and close) over the active view.
//! Panel views register here with `register_panel_view`; the secondary side bar has no views yet and shows its
//! empty message.

use gpui::{AnyElement, Context, Div, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px};

use super::Workbench;
use super::hover::HoverOptions;
use super::layout::Part;
use crate::commands::Outcome;

/// paneCompositePart.ts `ACTIVE_PANEL_SETTINGS_KEY` of the panel, in workspace storage.
const ACTIVE_PANEL_KEY: &str = "workbench.panelpart.activepanelid";
/// The composite title's height (`--vscode-workbench-titleHeight` of a pane composite part).
const TITLE_HEIGHT: f32 = 32.;

/// A view container in the panel: its tab title, its content and its title actions (`MenuId.ViewTitle`).
pub struct PanelView {
    pub id: &'static str,
    pub title: &'static str,
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
}

impl Workbench {
    pub(super) fn register_panel_view(&mut self, view: PanelView) {
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

    /// The views with a tab in the panel's composite bar, by registration order.
    pub(super) fn panel_tabs(&self) -> Vec<&PanelView> {
        self.panel.views.iter().filter(|view| (view.visible)(self)).collect()
    }

    fn active_panel_view(&self) -> Option<&PanelView> {
        let storage = self.workspace_storage.lock().unwrap();
        let id = self.panel.active.or(storage.get(ACTIVE_PANEL_KEY));
        // terminal.contribution.ts registers the Terminal container with `isDefault`.
        let default = super::terminal_view::TERMINAL_VIEW_ID;
        self.panel.views.iter().find(|view| Some(view.id) == id).or(self.panel.views.iter().find(|view| view.id == default))
    }

    /// The panel card: the views' tabs, maximize and close, and the active view.
    pub(super) fn panel_part(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let active = self.active_panel_view().map(|view| view.id);
        let tabs: Vec<AnyElement> = self.panel_tabs().into_iter().map(|view| self.panel_tab(view, Some(view.id) == active, cx)).collect();
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
            .children(tabs)
            .child(div().ml_auto().h_full().flex().items_center().gap(px(4.)).when_some(view_actions, |bar, view_actions| {
                bar.child(view_actions).child(div().w(px(1.)).h(px(16.)).mx(px(4.)).bg(self.color("disabledForeground")))
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
                div()
                    .ml(px(3.))
                    .min_w(px(16.))
                    .h(px(16.))
                    .px(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.))
                    .text_size(px(11.))
                    .bg(self.color("activityBarBadge.background"))
                    .text_color(self.foreground_or("activityBarBadge.foreground"))
                    .child(count.to_string())
            }))
            .on_click(cx.listener(move |workbench, _, _, cx| workbench.open_panel_view(id, cx)))
            .into_any_element()
    }

    /// paneCompositePart.ts `.empty-pane-message-area`.
    fn empty_pane_message(&self) -> Div {
        div().flex_1().flex().items_center().justify_center().text_color(self.foreground_or("foreground")).child("Drag a view here to display.")
    }
}
