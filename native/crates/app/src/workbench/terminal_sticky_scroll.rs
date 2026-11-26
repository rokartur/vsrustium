//! terminal.stickyScroll.contribution.ts and terminalStickyScrollOverlay.ts: the current command's prompt and
//! command line kept over the top of the terminal while its output scrolls, Toggle Sticky Scroll, and the
//! overlay's click (reveal the command), context menu and hover.

use alacritty_terminal::grid::Dimensions;
use gpui::{Bounds, Context, MouseButton, MouseDownEvent, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::terminal_view::grid::{self, StickyLook};
use crate::commands::Outcome;
use crate::menus::MenuId;
use crate::terminal::sticky_scroll::StickyScroll;

/// terminalStickyScrollConfiguration.ts `TerminalStickyScrollSettingId`.
const ENABLED: &str = "terminal.integrated.stickyScroll.enabled";
const MAX_LINE_COUNT: &str = "terminal.integrated.stickyScroll.maxLineCount";
const IGNORED_COMMANDS: &str = "terminal.integrated.stickyScroll.ignoredCommands";

impl Workbench {
    pub(super) fn register_terminal_sticky_scroll_actions(&mut self) {
        self.commands.register("workbench.action.terminal.toggleStickyScroll", |workbench, _, _, cx| {
            let enabled = workbench.setting(ENABLED) == true;
            workbench.update_user_setting(ENABLED, json!(!enabled));
            cx.notify();
            Outcome::done()
        });
        // terminal.contribution.ts `ScrollToPreviousCommand` and `ScrollToNextCommand` (markNavigationAddon.ts).
        for (id, previous) in [("workbench.action.terminal.scrollToPreviousCommand", true), ("workbench.action.terminal.scrollToNextCommand", false)] {
            self.commands.register(id, move |workbench, _, _, cx| {
                if let Some(instance) = workbench.terminal_view.active_instance_mut() {
                    if previous { instance.scroll_to_previous_mark() } else { instance.scroll_to_next_mark() }
                    workbench.end_navigation_outline_later(cx);
                    cx.notify();
                }
                Outcome::done()
            });
        }
    }

    /// The overlay of terminal `id`, `None` while it is hidden or the setting is off.
    pub(super) fn terminal_sticky_scroll(&self, id: u32) -> Option<StickyScroll> {
        if self.setting(ENABLED) != true {
            return None;
        }
        let max_line_count = self.setting(MAX_LINE_COUNT).as_u64().unwrap_or(5) as usize;
        let ignored: Vec<String> = match self.setting(IGNORED_COMMANDS) {
            Value::Array(commands) => commands.iter().filter_map(|command| command.as_str().map(str::to_owned)).collect(),
            _ => Vec::new(),
        };
        self.terminal_view.instance(id)?.sticky_scroll(max_line_count, &ignored)
    }

    /// terminalStickyScroll.css: `terminalStickyScroll.background`, else the terminal's, else the panel's.
    pub(super) fn terminal_sticky_look(&self, id: u32) -> StickyLook {
        let background = self.theme.color("terminalStickyScroll.background").or(self.theme.color("terminal.background")).unwrap_or(self.color("panel.background"));
        StickyLook {
            background,
            hover: self.terminal_view.sticky_hovered.filter(|hovered| *hovered == id).and_then(|_| self.theme.color("terminalStickyScrollHover.background")),
            border: self.theme.color("terminalStickyScroll.border"),
            shadow: self.theme.color("scrollbar.shadow"),
        }
    }

    /// The overlay's `.hover-overlay`, where the grid painted it last: a click reveals the command and focuses
    /// the terminal, a right click opens `TerminalStickyScrollContext`, the wheel passes through to the terminal.
    pub(super) fn terminal_sticky_scroll_hit_area(&self, id: u32, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let sticky = self.terminal_sticky_scroll(id)?;
        let pane = *self.terminal_view.pane_bounds.get(&id)?;
        let metrics = self.terminal_view.metrics.as_ref()?;
        let rows = self.terminal_view.instance(id)?.term.screen_lines();
        let overlay = grid::sticky_scroll_bounds(pane, rows, &sticky, metrics, self.terminal_scrollbar_width());
        let local = Bounds::new(overlay.origin - pane.origin, overlay.size);
        let (reveal_start, reveal_end) = sticky.reveal;
        Some(
            div()
                .id(("terminal-sticky-scroll", id as usize))
                .absolute()
                .left(local.origin.x)
                .top(local.origin.y)
                .w(local.size.width)
                .h(local.size.height.max(px(0.)))
                .cursor_pointer()
                .on_hover(cx.listener(move |workbench, hovered: &bool, _, cx| {
                    workbench.terminal_view.sticky_hovered = hovered.then_some(id);
                    cx.notify();
                }))
                .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    workbench.terminal_view.set_active(id);
                    if let Some(instance) = workbench.terminal_view.instance_mut(id) {
                        instance.reveal_rows(reveal_start, reveal_end);
                    }
                    workbench.end_navigation_outline_later(cx);
                    window.focus(&workbench.terminal_view.focus, cx);
                    cx.notify();
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    let context = workbench.key_context(window);
                    let anchor = Bounds { origin: event.position, size: Default::default() };
                    workbench.show_context_menu(MenuId::TerminalStickyScrollContext, &context, anchor, Vec::new(), window, cx);
                })),
        )
    }
}
