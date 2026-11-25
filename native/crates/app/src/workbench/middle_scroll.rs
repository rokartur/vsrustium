//! middleScrollController.ts (`editor.scrollOnMiddleClick`): a middle press in the editor starts a scrolling session
//! that scrolls every frame by how far the pointer is from the press, past a 5px dead zone.

use std::time::Instant;

use gpui::{Context, Div, Pixels, Point, Styled, div, px};
use serde_json::json;

use super::Workbench;

pub(super) struct MiddleScroll {
    origin: Point<Pixels>,
    mouse: Point<Pixels>,
    last_frame: Instant,
    did_scroll: bool,
}

impl Workbench {
    /// The editor's `onMouseDown`: any press ends a session, a middle press with the setting on starts one and
    /// returns true, the press then goes no further (`stopPropagation`).
    pub(super) fn middle_scroll_mouse_down(&mut self, middle: bool, position: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        if self.middle_scroll.take().is_some() {
            cx.notify();
            return false;
        }
        if !middle || self.setting("editor.scrollOnMiddleClick") != json!(true) {
            return false;
        }
        self.middle_scroll = Some(MiddleScroll { origin: position, mouse: position, last_frame: Instant::now(), did_scroll: false });
        cx.notify();
        true
    }

    /// The editor's `onMouseUp`: a release ends the session only once it scrolled.
    pub(super) fn middle_scroll_mouse_up(&mut self, cx: &mut Context<Self>) {
        if self.middle_scroll.as_ref().is_some_and(|session| session.did_scroll) {
            self.middle_scroll = None;
            cx.notify();
        }
    }

    pub(super) fn middle_scroll_mouse_moved(&mut self, position: Point<Pixels>) {
        if let Some(session) = &mut self.middle_scroll {
            session.mouse = position;
        }
    }

    pub(super) fn end_middle_scroll(&mut self, cx: &mut Context<Self>) {
        if self.middle_scroll.take().is_some() {
            cx.notify();
        }
    }

    /// One animation frame of the session: the pointer's offset past the dead zone, every 32ms. Returns whether a
    /// session runs, so the caller asks for the next frame.
    pub(super) fn middle_scroll_frame(&mut self, cx: &mut Context<Self>) -> bool {
        // The autorun drops the session when the setting turns off.
        if self.setting("editor.scrollOnMiddleClick") != json!(true) {
            self.middle_scroll = None;
        }
        let viewport = self.editors.viewport;
        let (Some(session), Some(editor)) = (self.middle_scroll.as_mut(), self.editors.active_mut()) else { return false };
        let now = Instant::now();
        let factor = now.duration_since(session.last_frame).as_secs_f32() * 1000. / 32.;
        session.last_frame = now;
        let (dx, dy) = scroll_delta(session.mouse - session.origin, factor);
        if dx != 0. || dy != 0. {
            session.did_scroll = true;
            if editor.set_scroll(editor.scroll.top + dy, editor.scroll.left + dx, viewport) {
                self.editor_scrolled(cx);
            }
        }
        true
    }

    /// middleScroll.css `.scroll-editor-on-middle-click-dot`: a 5px circle with a 1px border, centered on the press.
    /// shortcut: GPUI has no `all-scroll` cursor, so the editor keeps its cursors during a session; add one to gpui.
    pub(super) fn middle_scroll_dot(&self, editor_origin: Point<Pixels>) -> Option<Div> {
        let session = self.middle_scroll.as_ref()?;
        let center = session.origin - editor_origin;
        Some(
            div()
                .absolute()
                .left(center.x - px(3.5))
                .top(center.y - px(3.5))
                .size(px(7.))
                .rounded_full()
                .border_1()
                .border_color(self.color("editor.background"))
                .bg(self.color("editor.foreground"))
                .opacity(0.5),
        )
    }
}

/// `mouseDelta.withThreshold(5).scale(frameDurationMs / 32)`.
fn scroll_delta(offset: Point<Pixels>, factor: f32) -> (f32, f32) {
    let threshold = |value: f32| match value {
        value if value > 5. => value - 5.,
        value if value < -5. => value + 5.,
        _ => 0.,
    };
    (threshold(f32::from(offset.x)) * factor, threshold(f32::from(offset.y)) * factor)
}

#[cfg(test)]
mod tests {
    use gpui::{point, px};

    use super::scroll_delta;

    #[test]
    fn the_pointer_scrolls_past_a_5px_dead_zone_every_32ms() {
        assert_eq!(scroll_delta(point(px(3.), px(-5.)), 1.), (0., 0.));
        assert_eq!(scroll_delta(point(px(12.), px(-25.)), 1.), (7., -20.));
        // A 16ms frame scrolls half as far.
        assert_eq!(scroll_delta(point(px(12.), px(-25.)), 0.5), (3.5, -10.));
    }
}
