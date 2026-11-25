//! The Explorer of a window without a folder (emptyView.ts) with its welcome content (views_welcome.rs).

use gpui::{Context, Div, Window, div};

use super::Workbench;

impl Workbench {
    /// The body of the Explorer's first pane without a folder: its welcome content (viewPane.ts `WelcomeView`).
    pub(super) fn empty_view(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        self.view_welcome(super::views_welcome::EXPLORER_EMPTY_VIEW, window, cx).unwrap_or_else(div)
    }
}
