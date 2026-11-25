//! `editor.smoothScrolling` (scrollable.ts `setScrollPositionSmooth`, viewLayout.ts `SMOOTH_SCROLLING_TIME`): a
//! cursor reveal or a mouse wheel notch moves the scroll position over 125 ms. A reveal during the animation is
//! computed against its target (`getFutureViewport`); a scroll set at once (a scrollbar drag, a trackpad) ends it.

use std::time::Duration;

use gpui::{Context, Task};

use super::Workbench;
use crate::editor::smooth_scroll::SmoothScroll;
use crate::editor::{OpenEditor, Scroll, Viewport};

/// One animation frame at 120 Hz.
const FRAME: Duration = Duration::from_millis(8);

pub(super) struct SmoothScrolling {
    model_id: u32,
    animation: SmoothScroll,
    /// The position the last frame set; another one means a scroll set at once took over.
    applied: Scroll,
    _frames: Task<()>,
}

impl Workbench {
    /// Runs `scroll` on the active editor from the future scroll position and, with `editor.smoothScrolling`,
    /// animates from the current one to where it lands. Returns whether the editor scrolled.
    pub(super) fn scroll_smoothly(&mut self, cx: &mut Context<Self>, scroll: impl FnOnce(&mut OpenEditor, Viewport)) -> bool {
        let smooth = self.setting("editor.smoothScrolling").as_bool().unwrap_or(false);
        let viewport = self.editors.viewport;
        let running = self.smooth_scrolling.take();
        let editor = self.editors.active_mut().expect("an editor scrolls only while open");
        let current = editor.scroll;
        let running = running.filter(|running| running.model_id == editor.model_id && running.applied == current);
        if let Some(running) = &running {
            editor.scroll = Scroll { top: running.animation.to.0, left: running.animation.to.1 };
        }
        let future = editor.scroll;
        scroll(editor, viewport);
        let target = editor.scroll;
        if !smooth || target == current {
            return target != current;
        }
        editor.scroll = current;
        if let Some(running) = running.filter(|_| target == future) {
            self.smooth_scrolling = Some(running);
            return false;
        }
        let model_id = editor.model_id;
        let animation = SmoothScroll::start((current.top, current.left), (target.top, target.left), (viewport.height, viewport.width), cx.background_executor().now());
        let frames = cx.spawn(async move |workbench, cx| {
            loop {
                cx.background_executor().timer(FRAME).await;
                let Ok(true) = workbench.update(cx, |workbench, cx| workbench.smooth_scroll_frame(cx)) else { break };
            }
        });
        self.smooth_scrolling = Some(SmoothScrolling { model_id, animation, applied: current, _frames: frames });
        false
    }

    /// A scroll already made from `current` (a find reveal), animated as `scroll_smoothly` would.
    pub(super) fn animate_scroll_from(&mut self, current: Scroll, cx: &mut Context<Self>) -> bool {
        let editor = self.editors.active_mut().expect("an editor scrolls only while open");
        let target = std::mem::replace(&mut editor.scroll, current);
        self.scroll_smoothly(cx, |editor, _| editor.scroll = target)
    }

    /// Moves the editor to the animation's position now; false once it is done or another scroll took over.
    fn smooth_scroll_frame(&mut self, cx: &mut Context<Self>) -> bool {
        let viewport = self.editors.viewport;
        let Some(running) = &mut self.smooth_scrolling else { return false };
        let Some(editor) = self.editors.active_mut().filter(|editor| editor.model_id == running.model_id && editor.scroll == running.applied) else {
            self.smooth_scrolling = None;
            return false;
        };
        let (top, left, done) = running.animation.tick(cx.background_executor().now());
        editor.set_scroll(top, left, viewport);
        running.applied = editor.scroll;
        if done {
            self.smooth_scrolling = None;
        }
        self.editor_scrolled(cx);
        !done
    }
}

#[cfg(test)]
mod tests;
