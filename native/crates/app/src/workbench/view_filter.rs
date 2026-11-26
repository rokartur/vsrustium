//! viewFilter.ts `FilterWidget` in a panel title or at the top of the view (views.css): the filter input, the
//! "Showing N of M" badge and the More Filters button whose menu the view builds.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{Bounds, Context, Div, MouseButton, MouseDownEvent, Pixels, Window, div, prelude::*, px};

use super::Workbench;
use super::input_box_view::{InputId, InputLook};
use super::output_view::bounds_tracker;
use crate::theme::ThemeKind;

pub(super) struct ViewFilter {
    /// The view container, whose part and size place the filter.
    pub container: &'static str,
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
    /// In the title (`view_filter_in_header`): 200px, while focused up to 400px of the room the tabs leave, 10px right
    /// margin. At the top of the view: the view's width less 20px margins each side (`.pane-body .viewpane-filter-container`).
    /// The badge hides in a view below 400px (`.small`), and in a panel title shows while the box is focused.
    pub(super) fn view_filter_box(&self, filter: ViewFilter, cx: &mut Context<Self>) -> Div {
        let header = self.view_filter_in_header(filter.container);
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
        let small = self.view_filter_small(filter.container);
        let badge = filter.badge.filter(|_| filter.focused && !small).map(|badge| {
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
        let filter_box = div()
            .relative()
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
            .child(filter_button);
        match header {
            // paneCompositePart.css `.action-item { margin-right: 4px }` outranks views.css's 10px.
            true => filter_box.w(px(self.panel_filter_width(filter.container, filter.focused))).flex_shrink(1.).min_w(px(150.)).mr(px(4.)).child(self.panel_filter_tracker(filter.container)),
            false => div().flex_none().mx(px(20.)).my(px(10.)).flex().child(filter_box.flex_1()),
        }
    }
}
