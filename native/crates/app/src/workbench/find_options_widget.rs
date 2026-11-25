//! findOptionsWidget.ts and findInput.ts `highlightFindOptions`: with the find widget hidden, an option change or a
//! multi-cursor search over the find options shows the case, whole word and regex toggles at the editor's top right
//! for 2 s (kept while hovered); with it shown, the multi-cursor search flashes the find input's options for 100 ms.

use std::time::{Duration, Instant};

use gpui::{AnyElement, BoxShadow, Context, InteractiveElement, IntoElement, ParentElement, Rgba, StatefulInteractiveElement, Styled, Task, Window, div, point, prelude::FluentBuilder, px, rgba};

use super::Workbench;
use crate::editor::VERTICAL_SCROLLBAR_WIDTH;
use crate::editor::find::state::NewFindState;
use crate::theme::ThemeKind;

/// findOptionsWidget.ts `_hideSoon`.
const HIDE_DELAY: Duration = Duration::from_millis(2000);
/// findInput.css `monaco-findInput-highlight-*`: 100ms linear.
const FLASH: Duration = Duration::from_millis(100);

#[derive(Default)]
pub(crate) struct FindOptionsWidget {
    shown: bool,
    hide: Option<Task<()>>,
    flash: Option<Instant>,
}

impl Workbench {
    /// After a find action: findController.ts `highlightFindOptions` for a multi-cursor search, and the options
    /// widget's listener for an option change while the find widget is hidden.
    pub(super) fn highlight_find_options(&mut self, before: (bool, bool, bool), cx: &mut Context<Self>) {
        let highlight = std::mem::take(&mut self.find.controller.highlight_find_options);
        let state = &self.find.controller.state;
        if state.is_revealed {
            if highlight {
                self.find.options_widget.flash = Some(Instant::now());
            }
            return;
        }
        // findState.ts fires the option changes of the effective values, a session's overrides included.
        if highlight || (state.is_regex, state.whole_word, state.match_case) != before {
            self.find.options_widget.shown = true;
            self.schedule_find_options_hide(cx);
        }
    }

    fn schedule_find_options_hide(&mut self, cx: &mut Context<Self>) {
        self.find.options_widget.hide = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(HIDE_DELAY).await;
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.find.options_widget.shown = false;
                cx.notify();
            });
        }));
    }

    /// The `.controls` background of findInput.css's highlight animation, from its start color to transparent.
    pub(super) fn find_options_flash(&self, window: &mut Window) -> Option<Rgba> {
        let elapsed = self.find.options_widget.flash?.elapsed();
        if elapsed >= FLASH {
            return None;
        }
        window.request_animation_frame();
        let mut color = match self.theme.kind {
            ThemeKind::Light | ThemeKind::HighContrastLight => rgba(0xFDFF00CC),
            ThemeKind::Dark | ThemeKind::HighContrastDark => rgba(0xFFFFFF70),
        };
        color.a *= 1. - elapsed.as_secs_f32() / FLASH.as_secs_f32();
        Some(color)
    }

    /// `TOP_RIGHT_CORNER` at `top: 10px`, right of the scrollbar and the minimap.
    pub(super) fn find_options_widget(&self, minimap_width: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.find.options_widget.shown {
            return None;
        }
        let state = &self.find.controller.state;
        let widget = div()
            .id("find-options-widget")
            .absolute()
            .top(px(10.))
            .right(px(VERTICAL_SCROLLBAR_WIDTH + minimap_width))
            .flex()
            .bg(self.color("editorWidget.background"))
            .text_color(self.foreground_or("editorWidget.foreground"))
            .when_some(self.theme.color("contrastBorder"), |widget, border| widget.border_2().border_color(border))
            // --vscode-shadow-lg.
            .shadow(vec![BoxShadow { color: rgba(0x00000024).into(), offset: point(px(0.), px(0.)), blur_radius: px(12.), spread_radius: px(0.), inset: false }])
            .occlude()
            .on_hover(cx.listener(|workbench, hovered: &bool, _, cx| match hovered {
                true => workbench.find.options_widget.hide = None,
                false => workbench.schedule_find_options_hide(cx),
            }))
            .child(self.option_toggle("case-sensitive", state.match_case, |state| NewFindState { match_case: Some(!state.match_case), ..Default::default() }, false, cx))
            .child(self.option_toggle("whole-word", state.whole_word, |state| NewFindState { whole_word: Some(!state.whole_word), ..Default::default() }, false, cx))
            .child(self.option_toggle("regex", state.is_regex, |state| NewFindState { is_regex: Some(!state.is_regex), ..Default::default() }, false, cx));
        Some(widget.into_any_element())
    }
}

#[cfg(test)]
mod tests;
