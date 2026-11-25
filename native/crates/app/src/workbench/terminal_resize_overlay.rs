//! terminalResizeDimensionsOverlay.ts: `cols x rows` in the middle of a terminal for 500 ms after it resizes,
//! from a second after it started (`terminal.integrated.resizeDimensionsOverlay.enabled`).

use std::time::{Duration, Instant};

use alacritty_terminal::grid::Dimensions;
use gpui::{BoxShadow, Window, div, hsla, point, prelude::*, px};

use super::Workbench;

/// `Constants.ResizeOverlayHideDelay`.
const HIDE_DELAY: Duration = Duration::from_millis(500);
/// The contribution waits a second after the process is ready, to skip the resizes of startup.
const STARTUP_DELAY: Duration = Duration::from_secs(1);

impl Workbench {
    /// Called on every resize the pane's layout makes.
    pub(super) fn show_resize_overlay(&mut self, id: u32) {
        if self.setting("terminal.integrated.resizeDimensionsOverlay.enabled") == false {
            return;
        }
        let Some(instance) = self.terminal_view.instance(id) else { return };
        if instance.started.elapsed() < STARTUP_DELAY {
            return;
        }
        let label = format!("{} x {}", instance.term.columns(), instance.term.screen_lines());
        self.terminal_view.resize_overlay = Some((id, label, Instant::now()));
    }

    /// The pane repaints each frame while its overlay shows, so it hides on time.
    pub(super) fn repaint_while_resize_overlay_shows(&self, id: u32, window: &mut Window) {
        if self.terminal_view.resize_overlay.as_ref().is_some_and(|(shown, _, at)| *shown == id && at.elapsed() < HIDE_DELAY) {
            window.request_animation_frame();
        }
    }

    /// `.terminal-resize-overlay`.
    pub(super) fn terminal_resize_overlay(&self, id: u32) -> Option<impl IntoElement + use<>> {
        let (shown, label, at) = self.terminal_view.resize_overlay.as_ref()?;
        if *shown != id || at.elapsed() >= HIDE_DELAY {
            return None;
        }
        let shadow = BoxShadow { color: hsla(0., 0., 0., 0.5), offset: point(px(0.), px(2.)), blur_radius: px(8.), spread_radius: px(0.), inset: false };
        let high_contrast = self.theme.kind.is_high_contrast();
        Some(
            div().absolute().inset_0().flex().items_center().justify_center().child(
                div()
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(4.))
                    .bg(self.color("editorWidget.background"))
                    .text_color(self.color("editorWidget.foreground"))
                    .border_1()
                    .border_color(self.color(if high_contrast { "contrastBorder" } else { "editorWidget.border" }))
                    .when(!high_contrast, |overlay| overlay.shadow(vec![shadow]))
                    .text_size(px(11.))
                    .child(label.clone()),
            ),
        )
    }
}
