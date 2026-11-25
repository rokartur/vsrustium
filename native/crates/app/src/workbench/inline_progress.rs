//! inlineProgress.ts `InlineProgressManager`: a spinner at a model position while an operation runs past 500 ms; a
//! click cancels it. shortcut: the widget draws over the text, where upstream's `after` decoration also makes 1em of
//! room for it; upgrade with an injected text owner.

use std::time::Duration;

use gpui::{Animation, AnimationExt, AnyElement, Context, MouseButton, Task, Transformation, div, percentage, prelude::*, px, svg};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use crate::editor::{OpenEditor, Position};

/// `_showDelay`.
const SHOW_DELAY: Duration = Duration::from_millis(500);

pub(super) struct InlineProgress {
    pub model_id: u32,
    pub position: Position,
    pub title: String,
    shown: bool,
    _timer: Task<()>,
}

impl Workbench {
    /// `showWhile`: the spinner shows after the delay until `clear_inline_progress`.
    pub(super) fn show_inline_progress(&mut self, model_id: u32, position: Position, title: String, cx: &mut Context<Self>) {
        let timer = cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(SHOW_DELAY).await;
            let _ = workbench.update(cx, |workbench, cx| {
                if let Some(progress) = workbench.paste_edits.progress.as_mut() {
                    progress.shown = true;
                    cx.notify();
                }
            });
        });
        self.paste_edits.progress = Some(InlineProgress { model_id, position, title, shown: false, _timer: timer });
    }

    pub(super) fn clear_inline_progress(&mut self, cx: &mut Context<Self>) {
        if self.paste_edits.progress.take().is_some_and(|progress| progress.shown) {
            cx.notify();
        }
    }

    /// `InlineProgressWidget`: the `loading~spin` codicon at 80% size in a box 0.8 lines wide, `x` while hovered.
    pub(super) fn inline_progress_widget(&self, editor: &OpenEditor, content_left: f32, char_width: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let progress = self.paste_edits.progress.as_ref().filter(|progress| progress.shown && progress.model_id == editor.model_id)?;
        let viewport = self.editors.viewport;
        let view_line = editor.position_view_line(progress.position, false);
        let segment = editor.segment(view_line);
        let left = editor.segment_visible_column(segment, progress.position.column) * char_width - editor.scroll.left + content_left;
        let top = editor.line_top(view_line, viewport) - editor.scroll.top;
        let line_height = viewport.line_height;
        // inlineProgressWidget.css: 80% of the editor font, 90% hovered; the macOS line height is 1.5 font sizes.
        let font_size = line_height / 1.5;
        let icon_size = (font_size * 0.8).round();
        let foreground = self.foreground_or("editor.foreground");
        let icons = self.app_root.join("node_modules/@vscode/codicons/src/icons");
        let spinner = svg()
            .external_path(icons.join("loading.svg").to_string_lossy().into_owned())
            .size(px(icon_size))
            .text_color(foreground)
            // codicon-animations.css: `codicon-modifier-spin` turns once in 1.5s in 30 steps.
            .with_animation("inline-progress-spin", Animation::new(Duration::from_millis(1500)).repeat(), |icon, delta| icon.with_transformation(Transformation::rotate(percentage((delta * 30.).floor() / 30.))));
        let close = svg().external_path(icons.join("close.svg").to_string_lossy().into_owned()).size(px((font_size * 0.9).round())).text_color(foreground);
        let widget = div()
            .id("inline-progress-widget")
            .group("inline-progress")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px((0.8 * line_height).ceil()))
            .h(px(line_height))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(div().group_hover("inline-progress", |icon| icon.invisible()).child(spinner))
            .child(div().absolute().invisible().group_hover("inline-progress", |icon| icon.visible()).child(close))
            .child(self.hover_target("inline-progress-hover", HoverContent::Text(progress.title.clone()), HoverOptions::mouse()))
            .on_mouse_down(MouseButton::Left, cx.listener(|workbench, _, _, cx| {
                cx.stop_propagation();
                workbench.cancel_paste(cx);
            }));
        Some(widget.into_any_element())
    }
}
