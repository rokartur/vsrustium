//! The exception widget (exceptionWidget.ts, debugEditorContribution.ts `toggleExceptionWidget`): a zone below
//! the line of the focused frame when its thread stopped on an exception and the frame is the thread's first
//! frame with a source.

use gpui::{AnyElement, ClickEvent, Context, FontWeight, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use crate::editor::{OpenEditor, Position};

pub struct ExceptionWidget {
    pub model_uri: String,
    pub zone_id: u32,
    /// `IExceptionInfo`: `id`, `description` and `details.stackTrace`.
    pub info: Value,
}

impl Workbench {
    /// Closes the widget, then shows it again when the focused frame threw.
    pub(super) fn toggle_exception_widget(&mut self, cx: &mut Context<Self>) {
        self.close_exception_widget(cx);
        let Some(session) = self.debug.focused_session() else { return };
        let Some(thread) = self.debug.focus.thread.and_then(|id| session.thread(id)) else { return };
        if thread.stopped_details["reason"] != "exception" {
            return;
        }
        let Some(thrower) = thread.frames.iter().find(|frame| frame.path().is_some() && !frame.deemphasized()) else { return };
        if Some(thrower.id) != self.debug.focus.frame {
            return;
        }
        let (session_id, thread_id, line) = (session.id.clone(), thread.id, thrower.line);
        let path = thrower.path().unwrap_or_default().to_owned();
        let supported = session.capabilities["supportsExceptionInfoRequest"] == true;
        let fallback = json!({ "description": thread.stopped_details["text"] });
        let reply = supported.then(|| self.dap_request(&session_id, "exceptionInfo", json!({ "threadId": thread_id })));
        cx.spawn(async move |this, cx| {
            let info = match reply {
                Some(reply) => match reply.await {
                    Ok(Ok(body)) => json!({ "id": body["exceptionId"], "description": body["description"], "details": body["details"] }),
                    _ => return,
                },
                None => fallback,
            };
            let _ = this.update(cx, |workbench, cx| workbench.show_exception_widget(&path, line, info, cx));
        })
        .detach();
    }

    fn show_exception_widget(&mut self, path: &str, line: u32, info: Value, cx: &mut Context<Self>) {
        self.close_exception_widget(cx);
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.editors.iter_mut().find(|editor| editor.document().scheme == "file" && editor.document().path.to_string_lossy() == path) else { return };
        let lines = 1 + info["description"].as_str().map_or(0, |text| text.lines().count()) + info["details"]["stackTrace"].as_str().map_or(0, |text| text.lines().count());
        // exceptionWidget.ts `_doLayout`: the content height plus the 6px paddings, in 11px lines (exceptionWidget.css on mac).
        let height = lines as f32 * (viewport.line_height * 11. / 12.).round() + 12. + 4.;
        let zone_id = editor.add_view_zone(Some(Position::new(line.max(1) as usize - 1, 0)), height);
        self.debug_view.exception = Some(ExceptionWidget { model_uri: editor.document().uri_string(), zone_id, info });
        cx.notify();
    }

    pub(super) fn close_exception_widget(&mut self, cx: &mut Context<Self>) {
        let Some(widget) = self.debug_view.exception.take() else { return };
        if let Some(editor) = self.editors.editors.iter_mut().find(|editor| editor.document().uri_string() == widget.model_uri) {
            editor.remove_view_zone(widget.zone_id);
        }
        cx.notify();
    }

    pub(super) fn exception_widget_element(&self, editor: &OpenEditor, content_left: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let widget = self.debug_view.exception.as_ref().filter(|widget| widget.model_uri == editor.document().uri_string())?;
        let viewport = self.editors.viewport;
        let top = editor.view_zone_top(widget.zone_id, viewport)? - editor.scroll.top;
        let height = editor.view_zones.iter().find(|zone| zone.id == widget.zone_id).map(|zone| zone.height)?;
        let info = &widget.info;
        let label = match info["id"].as_str() {
            Some(id) => format!("Exception has occurred: {id}"),
            None => "Exception has occurred.".to_owned(),
        };
        let icon_color = self.foreground_or("icon.foreground");
        let close = self.icon("close", 22., 16., icon_color).id("exception-widget-close").cursor_pointer().on_click(cx.listener(|workbench, _: &ClickEvent, _, cx| workbench.close_exception_widget(cx)));
        let copy_text = [info["id"].as_str(), info["description"].as_str(), info["details"]["stackTrace"].as_str()].into_iter().flatten().collect::<Vec<_>>().join("\n");
        let copy = self.icon("copy", 22., 16., icon_color).id("exception-widget-copy").cursor_pointer().on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_text.clone()));
        }));
        let mut body = div()
            .absolute()
            .left(px(content_left.max(10.) - 10.))
            .right(px(14.))
            .top(px(top))
            .h(px(height))
            .overflow_hidden()
            .border_t_2()
            .border_b_2()
            .border_color(self.color("debugExceptionWidget.border"))
            .bg(self.color("debugExceptionWidget.background"))
            .px(px(10.))
            .py(px(6.))
            .text_size(px(11.))
            .flex()
            .flex_col()
            .child(div().flex().items_center().child(div().font_weight(FontWeight::BOLD).child(label)).child(div().ml_auto().flex().child(copy).child(close)));
        for text in [info["description"].as_str(), info["details"]["stackTrace"].as_str()].into_iter().flatten() {
            body = body.child(div().font_family("Menlo").whitespace_normal().child(text.to_owned()));
        }
        Some(body.into_any_element())
    }
}
