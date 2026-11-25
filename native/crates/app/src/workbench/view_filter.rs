//! viewFilter.ts `FilterWidget` in a panel title (views.css): the filter input, the "Showing N of M" badge and the
//! More Filters button whose menu the view builds.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{Bounds, Context, Div, MouseButton, MouseDownEvent, Pixels, Window, div, prelude::*, px};

use super::Workbench;
use super::input_box_view::{InputId, InputLook};
use super::output_view::bounds_tracker;
use crate::theme::ThemeKind;

pub(super) struct ViewFilter {
    pub input: InputId,
    pub focused: bool,
    pub placeholder: &'static str,
    /// `checkMoreFilters`: a filter of the menu is not at its default.
    pub more_filters_checked: bool,
    /// `updateBadge`.
    pub badge: Option<String>,
    pub menu_bounds: Rc<Cell<Bounds<Pixels>>>,
    pub show_menu: fn(&mut Workbench, Bounds<Pixels>, &mut Window, &mut Context<Workbench>),
}

impl Workbench {
    /// 150px to 200px, 400px while focused, 10px right margin. The badge hides below 400px (`.small`), so in a
    /// panel title it shows while the box is focused.
    pub(super) fn view_filter_box(&self, filter: ViewFilter, cx: &mut Context<Self>) -> Div {
        let height = if self.theme.kind == ThemeKind::Light { 25. } else { 24. };
        let border = if filter.focused { self.color("focusBorder") } else { self.theme.color("input.border").unwrap_or(self.color("input.background")) };
        let checked = filter.more_filters_checked;
        let foreground = if checked { self.foreground_or("inputOption.activeForeground") } else { self.foreground_or("icon.foreground") };
        let bounds = filter.menu_bounds;
        let show_menu = filter.show_menu;
        let filter_button = self
            .icon("filter", 20., 16., foreground)
            .id("view-filter-menu")
            .relative()
            .rounded(px(4.))
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .when(checked, |button| button.bg(self.color("inputOption.activeBackground")).border_color(self.color("inputOption.activeBorder")))
            .when(!checked, |button| button.hover(|button| button.bg(self.color("toolbar.hoverBackground"))))
            .child(bounds_tracker(bounds.clone()))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                show_menu(workbench, bounds.get(), window, cx);
            }));
        let badge = filter.badge.filter(|_| filter.focused).map(|badge| {
            div()
                .flex_none()
                .my(px(4.))
                .mr(px(2.))
                .px(px(8.))
                .rounded(px(2.))
                .border_1()
                .border_color(self.theme.color("contrastBorder").unwrap_or(gpui::transparent_black().into()))
                .bg(self.color("badge.background"))
                .text_color(self.foreground_or("badge.foreground"))
                .child(badge)
        });
        let look = InputLook { password: false, placeholder: Some(filter.placeholder.to_owned()), focused: filter.focused };
        div()
            .flex_1()
            .min_w(px(150.))
            .max_w(px(if filter.focused { 400. } else { 200. }))
            .mr(px(10.))
            .h(px(height))
            .flex()
            .items_center()
            .pl(px(4.))
            .pr(px(2.))
            .border_1()
            .rounded(px(4.))
            .cursor_text()
            .text_size(px(12.))
            .bg(self.color("input.background"))
            .border_color(border)
            .text_color(self.foreground_or("input.foreground"))
            .child(div().flex_1().min_w_0().h_full().flex().child(self.input_text(filter.input, look, cx)))
            .children(badge)
            .child(filter_button)
    }
}
