//! A `custom` dialog of dialogHandler.ts: base/browser/ui/dialog/dialog.ts in a dimmed modal block, with a codicon,
//! the message, markdown details, an optional checkbox and buttons with optional descriptions.

use futures::channel::oneshot;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, BoxShadow, ClickEvent, Context, FontStyle, FontWeight, HighlightStyle, InteractiveElement, InteractiveText, IntoElement, Keystroke, MouseButton, ParentElement, StatefulInteractiveElement, Styled,
    StyledText, TextRun, Window, div, point, px, rgba,
};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use crate::notifications::{Node, parse_linked_text};

/// notificationsDialogs.css (modern UI, on in 1.147.0): `min-width: 440px` on the content box, plus 4px padding and
/// the 1px border each side.
const MIN_WIDTH: f32 = 450.;
/// `.dialog-buttons`' margin-left: long button rows align with the message text.
const BUTTONS_MARGIN_LEFT: f32 = 67.;

pub struct DialogButton {
    pub label: String,
    /// `custom.buttonDetails`: a description under the button.
    pub detail: Option<String>,
}

pub struct CustomDialog {
    pub icon: &'static str,
    pub message: String,
    /// The plain `detail` under the message, its line breaks kept.
    pub detail: Option<String>,
    /// `custom.markdownDetails`: trusted markdown paragraphs, with `[label](href)` links and `` `code` `` spans.
    pub details: Vec<String>,
    pub checkbox: Option<String>,
    /// The first button is the primary one.
    pub buttons: Vec<DialogButton>,
    /// The button Escape picks; none with `disableCloseAction`.
    pub cancel: Option<usize>,
}

pub struct DialogResult {
    pub button: usize,
    pub checkbox_checked: bool,
}

pub(super) struct ShownDialog {
    dialog: CustomDialog,
    checked: bool,
    focused: usize,
    reply: oneshot::Sender<DialogResult>,
}

impl Workbench {
    /// `IDialogService.prompt` with `custom`: the chosen button, once picked. Dialogs queue as the reference's do.
    pub(crate) fn show_custom_dialog(&mut self, dialog: CustomDialog, cx: &mut Context<Self>) -> oneshot::Receiver<DialogResult> {
        let (reply, receiver) = oneshot::channel();
        self.dialogs.push(ShownDialog { dialog, checked: false, focused: 0, reply });
        cx.notify();
        receiver
    }

    fn close_custom_dialog(&mut self, button: usize, cx: &mut Context<Self>) {
        if self.dialogs.is_empty() {
            return;
        }
        let shown = self.dialogs.remove(0);
        let _ = shown.reply.send(DialogResult { button, checkbox_checked: shown.checked });
        cx.notify();
    }

    /// dialog.ts's keydown handler on the modal block: it takes every key while the dialog shows.
    pub(super) fn custom_dialog_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(shown) = self.dialogs.first_mut() else { return false };
        let count = shown.dialog.buttons.len();
        match (keystroke.key.as_str(), keystroke.modifiers.shift) {
            ("enter" | "space", _) => {
                let button = shown.focused;
                self.close_custom_dialog(button, cx);
            }
            ("escape", _) => {
                if let Some(cancel) = shown.dialog.cancel {
                    self.close_custom_dialog(cancel, cx);
                }
            }
            ("tab", false) | ("right", _) => shown.focused = (shown.focused + 1) % count,
            ("tab", true) | ("left", _) => shown.focused = (shown.focused + count - 1) % count,
            _ => {}
        }
        cx.notify();
        true
    }

    pub(super) fn custom_dialog_widget(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let shown = self.dialogs.first()?;
        let dialog = &shown.dialog;
        let foreground = self.color("editorWidget.foreground");
        let mut buttons = div().flex().justify_end().ml(px(BUTTONS_MARGIN_LEFT));
        let mut buttons_width = BUTTONS_MARGIN_LEFT;
        for index in button_order(dialog.buttons.len(), dialog.cancel) {
            let button = &dialog.buttons[index];
            let label_width = text_width(&button.label, 12., FontStyle::Normal, window) + 18.;
            let detail_width = button.detail.as_ref().map_or(0., |detail| text_width(detail, 11., FontStyle::Italic, window) + 40.);
            buttons_width += label_width.max(detail_width) + 8.;
            buttons = buttons.child(self.dialog_button(index, button, index == shown.focused, cx));
        }
        let width = (buttons_width + 10.).max(MIN_WIDTH).min(f32::from(window.viewport_size().width) * 0.9);
        // The details are markdown paragraphs, whose 13px margins also sit below the last one.
        let mut body = div().flex().flex_col().when(!dialog.details.is_empty(), |body| body.mb(px(13.)));
        if let Some(detail) = &dialog.detail {
            body = body.child(div().line_height(px(20.)).text_color(self.color("descriptionForeground")).child(detail.clone()));
        }
        for (index, detail) in dialog.details.iter().filter(|detail| !detail.is_empty()).enumerate() {
            body = body.child(div().mt(px(13.)).child(self.dialog_markdown(index, detail, cx)));
        }
        let checkbox = dialog.checkbox.as_ref().map(|label| {
            div()
                .id("dialog-checkbox")
                .pt(px(15.))
                .flex()
                .cursor_pointer()
                .child(self.checkbox(shown.checked, false).mr(px(9.)))
                .child(div().flex_1().child(label.clone()))
                .on_click(cx.listener(|workbench, _: &ClickEvent, _, cx| {
                    if let Some(shown) = workbench.dialogs.first_mut() {
                        shown.checked = !shown.checked;
                        cx.notify();
                    }
                }))
        });
        let message_row = div()
            .flex()
            .pt(px(16.))
            .px(px(8.))
            .child(self.icon(dialog.icon, 24., 24., self.color(icon_color(dialog.icon))))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .pl(px(8.))
                    .pr(px(20.))
                    .child(div().mt(px(2.)).mb(px(12.)).min_h(px(22.)).flex().items_center().font_weight(FontWeight::SEMIBOLD).child(dialog.message.clone()))
                    .child(body)
                    .children(checkbox),
            );
        // dialog.ts: the close action resolves with the cancel button; notificationsDialogs.css puts it 8px from the corner.
        let close = dialog.cancel.map(|cancel| {
            self.icon("close", 22., 16., foreground)
                .id("dialog.close")
                .absolute()
                .top(px(8.))
                .right(px(8.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
                .child(self.hover_target("dialog.close", HoverContent::Text("Close Dialog".into()), HoverOptions::mouse()))
                .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| workbench.close_custom_dialog(cancel, cx)))
        });
        let dialog_box = div()
            .id("custom-dialog")
            .relative()
            .w(px(width))
            .p(px(4.))
            .flex()
            .flex_col()
            .rounded(px(8.))
            .border_1()
            .border_color(self.color("widget.border"))
            .bg(self.color("editorWidget.background"))
            .text_color(foreground)
            .text_size(px(13.))
            .line_height(px(18.2))
            .shadow(vec![BoxShadow { color: rgba(0x00000026).into(), offset: point(px(0.), px(0.)), blur_radius: px(20.), spread_radius: px(0.), inset: false }])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(message_row)
            .child(div().pt(px(16.)).flex().child(buttons.w_full()))
            .children(close);
        Some(
            div()
                .id("dialog-modal-block")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000080))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(dialog_box)
                .into_any_element(),
        )
    }

    fn dialog_button(&self, index: usize, button: &DialogButton, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let primary = index == 0;
        let (background, foreground, hover) = match primary {
            true => ("button.background", "button.foreground", "button.hoverBackground"),
            false => ("button.secondaryBackground", "button.secondaryForeground", "button.secondaryHoverBackground"),
        };
        let hover_color = self.color(hover);
        let text_button = div()
            .h(px(26.))
            .px(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .text_size(px(12.))
            .line_height(px(16.))
            .bg(self.color(background))
            .text_color(self.color(foreground))
            .when_some(self.theme.color("button.border"), |button, border| button.border_1().border_color(border))
            .hover(move |button| button.bg(hover_color))
            .whitespace_nowrap()
            .child(button.label.clone());
        // button.css: the focus outline is 1px, offset 1px, around the text button only. It sits in 2px of the
        // button's 4px margin, so the ring and its gap are a 1px border and padding inside a 2px margin.
        let text_button = div().p(px(1.)).rounded(px(5.)).border_1().border_color(gpui::transparent_black()).when(focused, |ring| ring.border_color(self.color("focusBorder"))).child(text_button);
        div()
            .id(("dialog-button", index))
            .m(px(2.))
            .flex()
            .flex_col()
            .cursor_pointer()
            .child(text_button)
            .children(button.detail.clone().map(|detail| div().px(px(20.)).py(px(4.)).text_size(px(11.)).italic().flex().justify_center().whitespace_nowrap().child(detail)))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| workbench.close_custom_dialog(index, cx)))
            .into_any_element()
    }

    /// A rendered markdown paragraph: its links open through the opener, `` `code` `` shows in the editor font.
    fn dialog_markdown(&self, index: usize, markdown: &str, cx: &mut Context<Self>) -> AnyElement {
        let link_color = self.color("textLink.foreground");
        if let Some(code) = markdown.strip_prefix('`').and_then(|rest| rest.strip_suffix('`')) {
            return div().font_family("Menlo").child(code.to_owned()).into_any_element();
        }
        let mut text = String::new();
        let mut links = Vec::new();
        let mut hrefs = Vec::new();
        for node in parse_linked_text(markdown) {
            match node {
                Node::Text(value) => text.push_str(&value),
                Node::Link { label, href, .. } => {
                    links.push(text.len()..text.len() + label.len());
                    hrefs.push(href);
                    text.push_str(&label);
                }
            }
        }
        let style = HighlightStyle { color: Some(link_color.into()), underline: Some(gpui::UnderlineStyle { thickness: px(1.), color: Some(link_color.into()), wavy: false }), ..Default::default() };
        let highlights = links.iter().map(|range| (range.clone(), style)).collect::<Vec<_>>();
        let weak = cx.entity().downgrade();
        InteractiveText::new(("dialog-markdown", index), StyledText::new(text).with_highlights(highlights))
            .on_click(links, move |index, window, cx| {
                let href = hrefs[index].clone();
                let _ = weak.update(cx, |workbench, cx| workbench.open_notification_link(&href, window, cx));
            })
            .into_any_element()
    }
}

/// dialog.ts `rearrangeButtons` on macOS: the cancel button moves second, then the row reverses, so the primary
/// button is last and cancel next to it.
fn button_order(count: usize, cancel: Option<usize>) -> Vec<usize> {
    let mut order: Vec<usize> = (0..count).collect();
    if count < 2 {
        return order;
    }
    if let Some(cancel) = cancel.filter(|cancel| *cancel < count) {
        order.remove(cancel);
        order.insert(1, cancel);
    }
    order.reverse();
    order
}

#[cfg(test)]
mod tests {
    #[test]
    fn cancel_sits_next_to_the_primary_button_on_the_right() {
        assert_eq!(super::button_order(3, Some(2)), [1, 2, 0]);
        assert_eq!(super::button_order(2, Some(1)), [1, 0]);
        assert_eq!(super::button_order(3, None), [2, 1, 0]);
        assert_eq!(super::button_order(1, Some(0)), [0]);
    }
}

/// dialog.css: the error and warning codicons in the problems colors, every other icon in the info color.
fn icon_color(icon: &str) -> &'static str {
    match icon {
        "error" => "problemsErrorIcon.foreground",
        "warning" => "problemsWarningIcon.foreground",
        _ => "problemsInfoIcon.foreground",
    }
}

fn text_width(text: &str, size: f32, style: FontStyle, window: &Window) -> f32 {
    let mut font = window.text_style().font();
    font.style = style;
    let run = TextRun { len: text.len(), font, color: gpui::black(), background_color: None, underline: None, strikethrough: None };
    f32::from(window.text_system().shape_line(text.to_string().into(), px(size), &[run], None).width)
}
