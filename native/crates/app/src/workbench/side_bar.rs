//! The side bar card (sidebarPart.ts) and, with `workbench.activityBar.location` `top` or `bottom`, its compact
//! activity bar (`.header-or-footer` of paneCompositePart.ts).

use gpui::{ClickEvent, Context, Div, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px};

use super::Workbench;
use super::hover::{HoverOptions, HoverPosition};
use super::search_view::SideBarView;

/// The view containers of the activity bar: icon in the activity bar, icon in the compact bar, title, command, view.
pub(super) const VIEW_CONTAINERS: [(&str, &str, &str, &str, Option<SideBarView>); 5] = [
    ("files", "files", "Explorer", "workbench.view.explorer", Some(SideBarView::Explorer)),
    ("search-large", "search", "Search", "workbench.view.search", Some(SideBarView::Search)),
    ("source-control", "source-control", "Source Control", "workbench.view.scm", Some(SideBarView::Scm)),
    ("debug-alt", "debug-alt", "Run and Debug", "workbench.view.debug", Some(SideBarView::Debug)),
    ("extensions", "extensions", "Extensions", "workbench.view.extensions", Some(SideBarView::Extensions)),
];

impl Workbench {
    /// `workbench.activityBar.location` is `top` or `bottom`.
    pub(super) fn compact_activity_bar_location(&self) -> Option<bool> {
        match self.setting("workbench.activityBar.location").as_str() {
            Some("top") => Some(false),
            Some("bottom") => Some(true),
            _ => None,
        }
    }

    /// The side bar card: without the activity bar left of it, its own left border and corners.
    pub(super) fn side_bar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let activity_bar_visible = self.is_part_visible(super::layout::Part::ActivityBar);
        let left = self.arrangement().side_bar == super::layout::Position::Left;
        let compact = self.compact_activity_bar_location();
        div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .border_t_1()
            .border_b_1()
            .bg(self.color("sideBar.background"))
            .border_color(self.color("surface.border"))
            // The seam with the activity bar is the bar's border; the side bar keeps a transparent 1px edge there.
            .when(activity_bar_visible && left, |card| card.border_r_1().pl(px(1.)).rounded_r(px(8.)))
            .when(activity_bar_visible && !left, |card| card.border_l_1().pr(px(1.)).rounded_l(px(8.)))
            .when(!activity_bar_visible, |card| card.border_l_1().border_r_1().rounded(px(8.)))
            .when(compact == Some(false), |card| card.child(self.compact_activity_bar(false, cx)))
            .child(self.side_bar_view(window, cx))
            .when(compact == Some(true), |card| card.child(self.compact_activity_bar(true, cx)))
    }

    /// The composite bar in the side bar's header or footer: 36x32 tabs with 16px icons on the tab strip's
    /// background, the checked one in the side bar's color and connected to its content.
    fn compact_activity_bar(&self, footer: bool, cx: &mut Context<Self>) -> Div {
        let hover = HoverOptions::composite_bar(if footer { HoverPosition::Above } else { HoverPosition::Below });
        let containers = VIEW_CONTAINERS.into_iter().filter(|(_, _, _, command, _)| !self.container_hidden_if_empty(command));
        let tabs: Vec<_> = containers.map(|(_, icon, title, command, view)| {
            let active = view.is_some_and(|view| view == self.side_bar_view);
            let color = if active { self.color("activityBarTop.foreground") } else { self.color("activityBarTop.inactiveForeground") };
            self.icon(icon, 36., 16., color)
                .h(px(32.))
                .id(command)
                .cursor_pointer()
                .when(active, |tab| tab.bg(self.color("sideBar.background")).when(footer, |tab| tab.rounded_b(px(7.))).when(!footer, |tab| tab.rounded_t(px(7.))))
                .child(self.action_hover(title, command, hover))
                .when(view.is_some(), |tab| tab.on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| workbench.activity_item_clicked(active, command, event, window, cx))))
        }).collect();
        div().h(px(32.)).flex_none().flex().bg(self.tab_strip_background()).children(tabs)
    }

    /// paneCompositeBar.ts `ViewContainerActivityAction.run`: a double click runs once, and the active view's icon
    /// hides the side bar unless `workbench.activityBar.iconClickBehavior` is `focus`.
    pub(super) fn activity_item_clicked(&mut self, active: bool, command: &'static str, event: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.click_count() > 1 {
            return;
        }
        let toggles = active && self.setting("workbench.activityBar.iconClickBehavior") != "focus";
        let command = if toggles { "workbench.action.closeSidebar" } else { command };
        self.execute_command(command, Vec::new(), window, cx);
    }
}
