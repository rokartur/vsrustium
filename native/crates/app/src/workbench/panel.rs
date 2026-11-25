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

/// A view container in the panel: its tab title and its content.
pub struct PanelView {
    pub id: &'static str,
    pub title: &'static str,
    pub render: fn(&Workbench, &mut Window, &mut Context<Workbench>) -> AnyElement,
    /// The view's title actions (`MenuId.ViewTitle`), drawn before the panel's own.
    pub actions: Option<fn(&Workbench, &mut Context<Workbench>) -> AnyElement>,
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
        cx.notify();
    }

    /// `setPanelHidden(false)`: opens the last active view, or the first registered one.
    fn show_panel(&mut self, cx: &mut Context<Self>) {
        self.set_part_hidden(Part::Panel, false, cx);
        if let Some(id) = self.active_panel_view().map(|view| view.id) {
            self.open_panel_view(id, cx);
        }
    }

    /// `setPanelHidden(true)`: a maximized panel gives the editor back first.
    pub(super) fn hide_panel(&mut self, cx: &mut Context<Self>) {
        if self.is_panel_maximized() {
            self.set_editor_hidden(false, cx);
        }
        self.set_part_hidden(Part::Panel, true, cx);
    }

    pub(super) fn active_panel_view(&self) -> Option<&PanelView> {
        let storage = self.workspace_storage.lock().unwrap();
        let id = self.panel.active.or(storage.get(ACTIVE_PANEL_KEY));
        self.panel.views.iter().find(|view| Some(view.id) == id).or(self.panel.views.first())
    }

    /// The panel card: the views' tabs, maximize and close, and the active view.
    pub(super) fn panel_part(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let active = self.active_panel_view().map(|view| view.id);
        let tabs: Vec<AnyElement> = self.panel.views.iter().map(|view| self.panel_tab(view, Some(view.id) == active, cx)).collect();
        let (maximize_icon, maximize_title) = match self.is_panel_maximized() {
            true => ("screen-normal", "Restore Panel Size"),
            false => ("screen-full", "Maximize Panel Size"),
        };
        let actions = vec![(maximize_icon, maximize_title, "workbench.action.toggleMaximizedPanel"), ("close", "Hide Panel", "workbench.action.closePanel")];
        let view_actions = self.active_panel_view().and_then(|view| view.actions).map(|actions| actions(self, cx));
        let content = match self.active_panel_view() {
            Some(view) => (view.render)(self, window, cx),
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

    /// `.composite.title`: the tab strip's background with its bottom line, tabs on the left, global actions on the right.
    /// The active view's actions come first, a separator between them and the part's (`.global-actions`).
    fn pane_composite_title(&self, tabs: Vec<AnyElement>, view_actions: Option<AnyElement>, actions: Vec<(&'static str, &'static str, &'static str)>, cx: &mut Context<Self>) -> Div {
        let separator = view_actions.is_some().then(|| div().w(px(1.)).h(px(16.)).mx(px(4.)).bg(self.color("panelTitle.border")));
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
            .child(div().ml_auto().h_full().flex().items_center().gap(px(4.)).children(view_actions).children(separator).children(actions.into_iter().map(|(icon, title, command)| {
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
            .on_click(cx.listener(move |workbench, _, _, cx| workbench.open_panel_view(id, cx)))
            .into_any_element()
    }

    /// paneCompositePart.ts `.empty-pane-message-area`.
    fn empty_pane_message(&self) -> Div {
        div().flex_1().flex().items_center().justify_center().text_color(self.foreground_or("foreground")).child("Drag a view here to display.")
    }
}
