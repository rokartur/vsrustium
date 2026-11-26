//! editorPlaceholder.ts: a file editor whose model did not resolve (textFileEditor.ts `handleSetInputError`) shows an
//! icon, a label and buttons where the text would be: binaryEditor.ts for a binary file, `createTooLargeFileError`
//! for a file over `workbench.editorLargeFileConfirmation`, `ErrorPlaceholderEditor` for a missing or unreadable one.

use std::path::Path;

use gpui::{ClickEvent, Context, Div, Stateful, FontWeight, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled, div, px};
use serde_json::json;

use super::Workbench;
use crate::editor::OpenError;

impl Workbench {
    /// `.monaco-editor-pane-placeholder` for the active editor's open error.
    pub(super) fn editor_placeholder(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let editor = self.editors.active().expect("drawn for the active editor");
        let error = editor.document().open_error.clone().expect("drawn for an editor with an open error");
        let (icon, color, label, buttons): (&str, &str, String, &[&'static str]) = match error {
            OpenError::Binary(_) => ("warning", "editorWarning.foreground", "The file is not displayed in the text editor because it is either binary or uses an unsupported text encoding.".into(), &["Open Anyway"]),
            OpenError::TooLarge(size) => (
                "warning",
                "editorWarning.foreground",
                format!("The file is not displayed in the text editor because it is very large ({}).", super::media_preview::format_size(size)),
                &["Open Anyway", "Configure Limit"],
            ),
            OpenError::NotFound => ("error", "editorError.foreground", "The editor could not be opened because the file was not found.".into(), &["Create File"]),
            OpenError::Failed => ("error", "editorError.foreground", "The editor could not be opened due to an unexpected error. Please consult the log for more details.".into(), &["Try Again", "Show Logs"]),
        };
        let mut bar = div().flex();
        for (index, label) in buttons.iter().copied().enumerate() {
            bar = bar.child(self.placeholder_button(index, label, cx));
        }
        div()
            .id("editor-placeholder")
            // editorPlaceholder.ts: the container takes the editor's focus (tabIndex 0), with no text input behind it.
            .track_focus(&self.editor_focus)
            .flex_1()
            .min_h_0()
            .px(px(16.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .bg(self.color("editor.background"))
            .text_color(self.color("foreground"))
            // The codicon's 48px font line box is 4px taller than the glyph (measured against the reference).
            .child(div().h(px(52.)).child(self.icon(icon, 48., 48., self.color(color))))
            // shortcut: max-width 450px plus a 14px space: GPUI's wrapper counts the space before a break, Chrome hangs it
            // ("binary or" fits the reference's first line); a line of 450 to 454px without a space there stays unwrapped.
            .child(div().max_w(px(454.)).text_size(px(14.)).line_height(px(18.)).text_center().child(label))
            .child(bar)
    }

    /// A `ButtonBar` button: the first one primary, the others secondary.
    fn placeholder_button(&self, index: usize, label: &'static str, cx: &mut Context<Self>) -> Stateful<Div> {
        let (background, foreground, hover) = match index {
            0 => ("button.background", "button.foreground", "button.hoverBackground"),
            _ => ("button.secondaryBackground", "button.secondaryForeground", "button.secondaryHoverBackground"),
        };
        let hover = self.color(hover);
        let border = self.theme.color(if index == 0 { "button.border" } else { "button.secondaryBorder" }).or(self.theme.color("button.border"));
        div()
            .id(("editor-placeholder-button", index))
            .mx(px(5.))
            .my(px(4.))
            .h(px(26.))
            .px(px(8.))
            .flex()
            .items_center()
            .rounded(px(4.))
            .border_1()
            .border_color(border.unwrap_or(gpui::transparent_black().into()))
            .cursor_pointer()
            .text_size(px(12.))
            .font_weight(FontWeight::NORMAL)
            .bg(self.color(background))
            .text_color(self.color(foreground))
            .hover(move |button| button.bg(hover))
            .child(label)
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| workbench.run_placeholder_action(label, window, cx)))
    }

    pub(super) fn run_placeholder_action(&mut self, label: &str, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let (error, path, uri) = (editor.document().open_error.clone(), editor.document().path.clone(), editor.document().uri());
        match (error, label) {
            // binaryFileEditor.ts `openInternal`: the Open With picker; its text editor reopens the file as text.
            (Some(OpenError::Binary(_)), _) => self.show_open_with_picker(uri, super::custom_editors::TEXT_EDITOR_ID.into(), false, window, cx),
            (Some(OpenError::TooLarge(_)), "Open Anyway") => self.reopen_file_editor(&path, true, false, cx),
            (Some(OpenError::TooLarge(_)), _) => drop(self.execute_command("workbench.action.openSettings", vec![json!({ "query": "workbench.editorLargeFileConfirmation" })], window, cx)),
            (Some(OpenError::NotFound), _) => {
                // textFileService.create, then the editor pinned.
                if std::fs::OpenOptions::new().write(true).create_new(true).open(&path).is_ok() {
                    self.reopen_file_editor(&path, false, false, cx);
                }
            }
            (Some(OpenError::Failed), "Try Again") => self.reopen_file_editor(&path, false, false, cx),
            (Some(OpenError::Failed), _) => drop(self.execute_command("workbench.action.showWindowLog", Vec::new(), window, cx)),
            (None, _) => {}
        }
    }

    /// `group.openEditor(input)` again: the active group's editor of `path` resolves its model anew.
    pub(super) fn reopen_file_editor(&mut self, path: &Path, unlimited: bool, as_text: bool, cx: &mut Context<Self>) {
        let Some(index) = self.editors.editors.iter().position(|editor| editor.document().is_file(path)) else { return };
        let document = self.read_file_editor_document(path, unlimited, as_text);
        self.editors.reload(index, document);
        self.editors_changed(cx);
        cx.notify();
    }
}
