//! emptyTextEditorHint.ts without its inline chat parts: an empty plain text untitled editor shows "Select a
//! language (⌘K M) to get started. Start typing to dismiss or don't show this again." at its first line.

use gpui::{AnyElement, Context, InteractiveText, StyledText, div, prelude::*, px};
use serde_json::json;

use super::Workbench;
use crate::editor::OpenEditor;
use crate::keybindings;

const SETTING: &str = "workbench.editor.empty.hint";
const CHANGE_LANGUAGE: &str = "workbench.action.editor.changeLanguageMode";

impl Workbench {
    /// `shouldRenderHint` for the default hint: not `hidden`, a writable empty untitled model in plain text, no
    /// injected text on its first line.
    fn shows_empty_editor_hint(&self, editor: &OpenEditor) -> bool {
        let document = editor.document();
        self.setting(SETTING) != json!("hidden")
            && document.scheme == "untitled"
            && document.language_id == crate::languages::PLAINTEXT
            && document.lines.iter().all(String::is_empty)
            && document.lines.len() == 1
            && !editor.injected_text.iter().any(|text| text.position.line == 0)
            && self.active_editor_readonly().is_none()
    }

    /// `EmptyTextEditorHintContentWidget` at line 1, column 1: italic, 4px from the text, in
    /// `input.placeholderForeground` with `textLink.foreground` links.
    pub(super) fn empty_editor_hint(&self, editor: &OpenEditor, content_left: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.shows_empty_editor_hint(editor) {
            return None;
        }
        let context = std::collections::HashMap::new();
        let label = self.keybindings.resolver.lookup_primary_keybinding(CHANGE_LANGUAGE, &context).map(|item| keybindings::label(&item.keybinding));
        let after_link = match label {
            Some(label) => format!(" ({label}) to get started. Start typing to dismiss or "),
            None => " to get started. Start typing to dismiss or ".to_string(),
        };
        let link = gpui::HighlightStyle { color: Some(self.color("textLink.foreground").into()), ..Default::default() };
        let text = format!("Select a language{after_link}don't show this again.");
        let language = 0.."Select a language".len();
        let dont_show_start = "Select a language".len() + after_link.len();
        let dont_show = dont_show_start..dont_show_start + "don't show".len();
        let top = editor.line_top(0, self.editors.viewport) - editor.scroll.top;
        let weak = cx.entity().downgrade();
        let styled = StyledText::new(text).with_highlights([(language.clone(), link), (dont_show.clone(), link)]);
        let links = InteractiveText::new("empty-editor-hint", styled).on_click(vec![language, dont_show], move |index, window, cx| {
            let _ = weak.update(cx, |workbench, cx| {
                window.focus(&workbench.editor_focus, cx);
                match index {
                    0 => drop(workbench.execute_command(CHANGE_LANGUAGE, Vec::new(), window, cx)),
                    _ => workbench.update_setting(SETTING, json!("hidden")),
                }
                cx.notify();
            });
        });
        // `width: max-content` inside the content widgets layer: the text wraps at the editor's content width.
        let hint = div()
            .absolute()
            .left(px(content_left - editor.scroll.left + 4.))
            .top(px(top))
            .max_w(px(self.editors.viewport.width - 4.))
            .italic()
            .text_color(self.color("input.placeholderForeground"))
            .child(links);
        Some(hint.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use serde_json::json;

    use crate::workbench::goto_symbol::tests::{open, run};

    #[gpui::test]
    fn the_hint_shows_in_an_empty_plain_text_untitled_editor_only(cx: &mut TestAppContext) {
        let (workbench, cx, _, _) = open(cx, "empty-hint", "text");
        let shows = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.shows_empty_editor_hint(workbench.editors.active().unwrap()));
        assert!(!shows(cx), "a file editor");
        run(&workbench, cx, "workbench.action.files.newUntitledFile");
        assert!(shows(cx));
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("type", vec![json!({ "text": "a" })], window, cx)));
        assert!(!shows(cx), "typing dismisses it");
        run(&workbench, cx, "deleteLeft");
        assert!(shows(cx));
        workbench.update(cx, |workbench, _| workbench.update_setting(super::SETTING, json!("hidden")));
        assert!(!shows(cx), "don't show");
    }
}
