//! bannerPart.ts: the banner under the title bar. It shows the item of the highest priority, the latest on a tie,
//! from the restricted mode banner (workspace_trust_view.rs) and the installation folder warning (window.ts).

use std::collections::HashMap;

use gpui::{ClickEvent, Context, Div, FocusHandle, KeyDownEvent, MouseButton, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use crate::commands::Outcome;

/// bannerPart.ts `height`.
pub(super) const BANNER_HEIGHT: f32 = 26.;

/// `IBannerItem`: actions are (label, href) links.
pub(super) struct BannerItem {
    pub id: &'static str,
    pub icon: &'static str,
    pub message: String,
    pub actions: Vec<(&'static str, &'static str)>,
    pub on_close: Option<fn(&mut Workbench, &mut Context<Workbench>)>,
    pub priority: i32,
}

pub(super) struct Banner {
    focus: FocusHandle,
    /// `focusedActionIndex`: an action link, or the close button at `actions.len()`.
    focused_action: Option<usize>,
    /// Items closed this session.
    hidden: Vec<&'static str>,
}

impl Banner {
    pub fn new(cx: &mut gpui::App) -> Banner {
        Banner { focus: cx.focus_handle(), focused_action: None, hidden: Vec::new() }
    }
}

impl Workbench {
    pub(super) fn register_banner_commands(&mut self) {
        self.commands.register("workbench.action.focusBanner", |workbench, _, window, cx| {
            workbench.focus_banner(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.banner.focusBanner", |workbench, _, window, cx| {
            workbench.focus_banner(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.banner.focusNextAction", |workbench, _, _, cx| {
            workbench.move_banner_focus(true, cx);
            Outcome::done()
        });
        self.commands.register("workbench.banner.focusPreviousAction", |workbench, _, _, cx| {
            workbench.move_banner_focus(false, cx);
            Outcome::done()
        });
    }

    pub(super) fn banner_context(&self, window: &Window, context: &mut HashMap<String, Value>) {
        context.insert("bannerFocused".into(), json!(self.banner.focus.is_focused(window)));
    }

    /// `renderActiveItem`'s choice.
    fn banner_item(&self) -> Option<BannerItem> {
        let mut item: Option<BannerItem> = None;
        for candidate in [self.app_root_banner(), self.restricted_mode_banner()].into_iter().flatten() {
            if self.banner.hidden.contains(&candidate.id) {
                continue;
            }
            if item.as_ref().is_none_or(|item| candidate.priority >= item.priority) {
                item = Some(candidate);
            }
        }
        item
    }

    /// window.ts: a folder inside the installation folder of the packaged app may be overwritten by an update.
    fn app_root_banner(&self) -> Option<BannerItem> {
        // appRoot = VSRustium.app/Contents/Resources/app; run from the checkout the app is not built.
        let install_location = self.app_root.ancestors().nth(3).filter(|_| self.app_root.ends_with("Contents/Resources/app"))?;
        if self.extension_development || !self.workspace()?.folders.iter().any(|folder| folder.path.starts_with(install_location)) {
            return None;
        }
        Some(BannerItem {
            id: "appRootWarning.banner",
            icon: "warning",
            message: format!("Files you store within the installation folder ('{}') may be OVERWRITTEN or DELETED IRREVERSIBLY without warning at update time.", crate::quick_input::path_label(install_location, None)),
            actions: Vec::new(),
            on_close: None,
            priority: 0,
        })
    }

    fn focus_banner(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.banner_item().is_none() {
            return;
        }
        self.banner.focused_action = None;
        window.focus(&self.banner.focus, cx);
        cx.notify();
    }

    /// `focusNextAction` and `focusPreviousAction`: through the action links and the close button, wrapping.
    fn move_banner_focus(&mut self, next: bool, cx: &mut Context<Self>) {
        let Some(item) = self.banner_item() else { return };
        let last = item.actions.len();
        self.banner.focused_action = Some(match (self.banner.focused_action, next) {
            (None, true) => 0,
            (Some(index), true) if index < last => index + 1,
            (_, true) => 0,
            (Some(index), false) if index > 0 => index - 1,
            (_, false) => last,
        });
        cx.notify();
    }

    /// `close`: hidden, then the item's `onClose`.
    fn close_banner(&mut self, id: &'static str, on_close: Option<fn(&mut Workbench, &mut Context<Workbench>)>, cx: &mut Context<Self>) {
        self.banner.hidden.push(id);
        self.banner.focused_action = None;
        if let Some(on_close) = on_close {
            on_close(self, cx);
        }
        cx.notify();
    }

    /// The grid row the banner takes: none while it is hidden.
    pub(super) fn banner_height(&self) -> f32 {
        if self.banner_item().is_some() { BANNER_HEIGHT } else { 0. }
    }

    /// The part: 26px, the icon, the message cut with an ellipsis, the action links and the close button.
    pub(super) fn banner_part(&self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let item = self.banner_item()?;
        let foreground = self.color("banner.foreground");
        let focus_border = self.color("focusBorder");
        let focused = self.banner.focus.is_focused(window).then_some(self.banner.focused_action).flatten();
        let mut actions = div().flex_grow(1.).flex_shrink_0().flex().items_center().whitespace_nowrap();
        for (index, (label, href)) in item.actions.iter().copied().enumerate() {
            actions = actions.child(
                div()
                    .id(label)
                    .ml(px(12.))
                    .p(px(3.))
                    .underline()
                    .cursor_pointer()
                    .when(focused == Some(index), |link| link.border_1().border_color(focus_border))
                    .child(label)
                    .on_click(cx.listener(move |workbench, _, window, cx| workbench.open_notification_link(href, window, cx))),
            );
        }
        let (id, on_close) = (item.id, item.on_close);
        let close = self
            .icon("close", 22., 16., foreground)
            .id("banner.close")
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
            .when(focused == Some(item.actions.len()), |button| button.border_1().border_color(focus_border))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| workbench.close_banner(id, on_close, cx)))
            .child(self.hover_target("banner.close", HoverContent::Text("Close Banner".into()), HoverOptions::mouse()));
        let action_hrefs: Vec<&'static str> = item.actions.iter().map(|(_, href)| *href).collect();
        Some(
            div()
                .track_focus(&self.banner.focus)
                .h(px(BANNER_HEIGHT))
                .flex_none()
                .flex()
                .cursor_default()
                .bg(self.color("banner.background"))
                .text_color(foreground)
                .text_size(px(12.))
                .line_height(px(BANNER_HEIGHT))
                // The links and the close button take Enter and Space as the focused DOM elements do.
                .on_key_down(cx.listener(move |workbench, event: &KeyDownEvent, window, cx| {
                    let Some(index) = workbench.banner.focused_action.filter(|_| matches!(event.keystroke.key.as_str(), "enter" | "space")) else { return };
                    match action_hrefs.get(index) {
                        Some(href) => workbench.open_notification_link(href, window, cx),
                        None => workbench.close_banner(id, on_close, cx),
                    }
                    cx.stop_propagation();
                }))
                .child(div().flex().flex_none().items_center().pl(px(10.)).pr(px(6.)).child(self.icon(item.icon, 16., 16., self.color("banner.iconForeground"))))
                .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(item.message))
                .child(actions)
                .child(div().flex().items_center().pl(px(6.)).pr(px(10.)).child(close)),
        )
    }
}

#[cfg(test)]
mod tests;
