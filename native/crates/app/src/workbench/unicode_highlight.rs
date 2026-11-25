//! unicodeHighlighter.ts `UnicodeHighlighter` with bannerController.ts: the characters the active editor marks and
//! the banner that replaces the marks past `computeUnicodeHighlights`' cap.

use std::cell::RefCell;
use std::rc::Rc;

use config::configuration::Overrides;
use gpui::{Context, Div, InteractiveElement, MouseButton, ParentElement, Stateful, StatefulInteractiveElement, Styled, div, px};
use serde_json::Value;
use textmate::StandardTokenType;

use super::Workbench;
use crate::editor::{OpenEditor, visible_column};
use crate::editor::unicode_highlighter::{UnicodeHighlighterOptions, UnicodeHighlights, compute_unicode_highlights};

#[cfg(test)]
mod tests;

/// bannerController.ts `BANNER_ELEMENT_HEIGHT`.
const BANNER_HEIGHT: f32 = 26.;

#[derive(Default)]
pub(super) struct UnicodeHighlight {
    /// The highlights of a model version under some options.
    cached: RefCell<Option<((u32, u32), UnicodeHighlighterOptions, Rc<UnicodeHighlights>)>>,
}

impl Workbench {
    /// `_updateHighlighter`'s options for the editor's language: `None` when all three highlights are off.
    fn unicode_highlighter_options(&self, editor: &OpenEditor) -> Option<UnicodeHighlighterOptions> {
        let settings = self.settings.lock().unwrap();
        let overrides = Overrides { resource: Some(&editor.document.path), override_identifier: Some(&editor.document.language_id) };
        let value = |key: &str| settings.get_value(Some(&format!("editor.unicodeHighlight.{key}")), &overrides).unwrap_or(Value::Null);
        // shortcut: the port has no workspace trust yet, so `inUntrustedWorkspace` resolves as in a trusted workspace (false).
        let resolved = |key: &str| value(key) == Value::Bool(true);
        let boolean = |key: &str| value(key).as_bool().unwrap_or(true);
        let true_keys = |key: &str, default: &[&str]| match value(key).as_object() {
            Some(map) => map.iter().filter(|(_, enabled)| **enabled == Value::Bool(true)).map(|(key, _)| key.clone()).collect(),
            None => default.iter().map(|key| key.to_string()).collect::<Vec<_>>(),
        };
        let options = UnicodeHighlighterOptions {
            non_basic_ascii: resolved("nonBasicASCII"),
            ambiguous_characters: boolean("ambiguousCharacters"),
            invisible_characters: boolean("invisibleCharacters"),
            include_comments: resolved("includeComments"),
            include_strings: resolved("includeStrings"),
            allowed_code_points: true_keys("allowedCharacters", &[]).iter().filter_map(|key| key.chars().next()).collect(),
            // `_os` (Intl's locale) and `_vscode` (platform.language) are both English: the port has no language packs.
            allowed_locales: true_keys("allowedLocales", &["_os", "_vscode"]).into_iter().map(|locale| if locale.starts_with('_') { "en".into() } else { locale }).collect(),
        };
        (options.non_basic_ascii || options.ambiguous_characters || options.invisible_characters).then_some(options)
    }

    /// `DocumentUnicodeHighlighter`: the highlights over the whole model, with the options they were found with.
    // shortcut: found again on the UI thread for each new model version, where upstream asks the editor worker 250ms
    // after the last edit and moves the old marks with the text meanwhile; upgrade when large files lag on typing.
    pub(super) fn unicode_highlights(&self, editor: &OpenEditor) -> Option<(UnicodeHighlighterOptions, Rc<UnicodeHighlights>)> {
        let options = self.unicode_highlighter_options(editor)?;
        let key = (editor.model_id, editor.version_id);
        let mut cached = self.unicode_highlight.cached.borrow_mut();
        if let Some((cached_key, cached_options, highlights)) = cached.as_ref()
            && *cached_key == key
            && *cached_options == options
        {
            return Some((options, highlights.clone()));
        }
        let highlights = Rc::new(compute_unicode_highlights(&editor.document.lines, 0, &options));
        *cached = Some((key, options.clone(), highlights.clone()));
        Some((options, highlights))
    }

    /// unicodeHighlighter.css `.unicode-highlight` on the marks of model line `line`; `hideInCommentTokens` and
    /// `hideInStringTokens` drop a mark in a comment or a string unless `includeComments` or `includeStrings`.
    pub(super) fn unicode_highlight_boxes(&self, editor: &OpenEditor, line: usize, unicode: Option<&(UnicodeHighlighterOptions, Rc<UnicodeHighlights>)>, char_width: f32) -> Vec<Div> {
        let Some((options, highlights)) = unicode else { return Vec::new() };
        let document = &editor.document;
        let text = &document.lines[line];
        let tab_size = document.indentation.tab_size;
        let first = highlights.ranges.partition_point(|(start, _)| start.line < line);
        let mut boxes = Vec::new();
        for (start, end) in highlights.ranges[first..].iter().take_while(|(start, _)| start.line == line) {
            let offset = text.char_indices().nth(start.column).map_or(text.len(), |(offset, _)| offset);
            let hidden = match document.tokens.token_type_at(&document.lines, line, offset) {
                StandardTokenType::Comment => !options.include_comments,
                StandardTokenType::String => !options.include_strings,
                _ => false,
            };
            if hidden {
                continue;
            }
            let left = visible_column(text, start.column, tab_size) as f32 * char_width;
            let right = visible_column(text, end.column, tab_size) as f32 * char_width;
            let mut mark = div().absolute().top_0().h_full().left(px(left)).w(px(right - left)).border_1().border_color(self.color("editorUnicodeHighlight.border"));
            if let Some(background) = self.theme.color("editorUnicodeHighlight.background") {
                mark = mark.bg(background);
            }
            boxes.push(mark);
        }
        boxes
    }

    fn unicode_banner_highlights(&self, editor: &OpenEditor) -> Option<Rc<UnicodeHighlights>> {
        let (_, highlights) = self.unicode_highlights(editor)?;
        (highlights.has_more && !self.editors.unicode_banner_closed).then_some(highlights)
    }

    pub(super) fn unicode_banner_shown(&self) -> bool {
        self.editors.active().is_some_and(|editor| self.unicode_banner_highlights(editor).is_some())
    }

    /// `_updateState`: past the cap, a banner names the most frequent reason, until closed for this model.
    pub(super) fn unicode_banner(&self, editor: &OpenEditor, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let highlights = self.unicode_banner_highlights(editor)?;
        let max = highlights.ambiguous_character_count.max(highlights.non_basic_ascii_character_count).max(highlights.invisible_character_count);
        let (message, label, command) = if highlights.non_basic_ascii_character_count >= max {
            ("This document contains many non-basic ASCII unicode characters", "Disable Non ASCII Highlight", "editor.action.unicodeHighlight.disableHighlightingOfNonBasicAsciiCharacters")
        } else if highlights.ambiguous_character_count >= max {
            ("This document contains many ambiguous unicode characters", "Disable Ambiguous Highlight", "editor.action.unicodeHighlight.disableHighlightingOfAmbiguousCharacters")
        } else {
            ("This document contains many invisible unicode characters", "Disable Invisible Highlight", "editor.action.unicodeHighlight.disableHighlightingOfInvisibleCharacters")
        };
        let foreground = self.color("banner.foreground");
        let link = div()
            .id("unicode-banner-action")
            .debug_selector(|| "unicode-banner-action".into())
            .flex_grow(1.)
            .flex_shrink_0()
            .mx(px(4.))
            // The link is an inline `a`: its 3px padding leaves the line box's height alone.
            .child(div().ml(px(12.)).px(px(3.)).underline().cursor_pointer().child(label))
            .on_click(cx.listener(move |workbench, _, window, cx| {
                let _ = workbench.execute_command(command, Vec::new(), window, cx);
            }));
        let close = div()
            .id("unicode-banner-close")
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|close| close.bg(self.color("toolbar.hoverBackground")))
            .child(self.icon("close", 22., 16., foreground))
            .on_click(cx.listener(|workbench, _, _, cx| {
                workbench.editors.unicode_banner_closed = true;
                cx.notify();
            }));
        Some(
            div()
                .id("unicode-banner")
                .track_focus(&self.unicode_banner_focus)
                .on_mouse_down(MouseButton::Left, cx.listener(|workbench, _, window, cx| {
                    window.focus(&workbench.unicode_banner_focus, cx);
                    cx.stop_propagation();
                }))
                .h(px(BANNER_HEIGHT))
                .flex_none()
                .flex()
                .cursor_default()
                .text_size(px(12.))
                .line_height(px(BANNER_HEIGHT))
                .bg(self.color("banner.background"))
                .text_color(foreground)
                .child(div().flex().flex_none().items_center().pl(px(10.)).pr(px(6.)).child(self.icon("warning", 16., 16., self.color("banner.iconForeground"))))
                .child(div().flex().items_center().overflow_hidden().whitespace_nowrap().text_ellipsis().child(message))
                .child(link)
                .child(div().flex().items_center().pl(px(6.)).pr(px(10.)).child(close)),
        )
    }
}
