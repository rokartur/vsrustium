//! unicodeHighlighter.ts `UnicodeHighlighter` with bannerController.ts: the characters the active editor marks and
//! the banner that replaces the marks past `computeUnicodeHighlights`' cap.

use std::cell::RefCell;
use std::rc::Rc;

use config::configuration::Overrides;
use gpui::{Context, Div, InteractiveElement, MouseButton, ParentElement, Stateful, StatefulInteractiveElement, Styled, Window, div, px};
use serde_json::{Value, json};
use textmate::StandardTokenType;

use super::Workbench;
use super::content_hover::{code_point_to_hex, format_code_point_markdown};
use super::editor_status::{StatusPick, label_item};
use crate::editor::OpenEditor;
use crate::editor::unicode_highlighter::{CodePointHighlighter, HighlightReason, UnicodeHighlighterOptions, UnicodeHighlights, compute_unicode_highlights, is_invisible_character};
use crate::quick_input::{Entry, Kind, QuickInput};

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
        let (resource, language) = (editor.document().path.clone(), editor.document().language_id.clone());
        let overrides = Overrides { resource: Some(&resource), override_identifier: Some(&language) };
        let value = |key: &str| settings.get_value(Some(&format!("editor.unicodeHighlight.{key}")), &overrides).unwrap_or(Value::Null);
        // `resolveOptions`: `inUntrustedWorkspace` is on only while the workspace is not trusted.
        let trusted = self.workspace_trust.is_trusted();
        let resolved = |key: &str| match value(key) {
            Value::String(text) if text == "inUntrustedWorkspace" => !trusted,
            value => value == Value::Bool(true),
        };
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
        let key = (editor.model_id, editor.text_model().version_id);
        let mut cached = self.unicode_highlight.cached.borrow_mut();
        if let Some((cached_key, cached_options, highlights)) = cached.as_ref()
            && *cached_key == key
            && *cached_options == options
        {
            return Some((options, highlights.clone()));
        }
        let highlights = Rc::new(compute_unicode_highlights(&editor.document().lines, 0, &options));
        *cached = Some((key, options.clone(), highlights.clone()));
        Some((options, highlights))
    }

    /// `ShowExcludeOptions`: the hover's "Adjust settings" picker for one character.
    pub(super) fn show_unicode_exclude_options(&mut self, args: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let user_locales = self.settings.lock().unwrap().configuration.inspect("editor.unicodeHighlight.allowedLocales", &Overrides::default()).user_local.value;
        let allowed_characters = self.setting("editor.unicodeHighlight.allowedCharacters");
        let Some(options) = unicode_exclude_options(args, user_locales, allowed_characters) else { return };
        let rows = options.into_iter().map(|(label, key, value)| (Entry::Item(label_item(&label)), StatusPick::UserSetting { key, value })).collect();
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.title = Some("Configure Unicode Highlight Options".into());
        self.show_status_picker(input, rows, window, cx);
    }

    /// unicodeHighlighter.css `.unicode-highlight` on the marks of model line `line`; `hideInCommentTokens` and
    /// `hideInStringTokens` drop a mark in a comment or a string unless `includeComments` or `includeStrings`.
    pub(super) fn unicode_highlight_boxes(&self, editor: &OpenEditor, line: usize, unicode: Option<&(UnicodeHighlighterOptions, Rc<UnicodeHighlights>)>, char_width: f32) -> Vec<Div> {
        let Some((options, highlights)) = unicode else { return Vec::new() };
        let document = &editor.document();
        let text = &document.lines[line];
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
            let (left, right) = editor.line_layout(line).range(start.column, end.column);
            let (left, right) = (left * char_width, right * char_width);
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

/// UnicodeHighlighterHoverParticipant `computeSync`'s reason text for one highlighted character.
pub(super) fn unicode_hover_reason(c: char, reason: HighlightReason, highlighter: &CodePointHighlighter) -> String {
    let code_point = format_code_point_markdown(c);
    match reason {
        HighlightReason::Ambiguous => {
            let confusable = highlighter.ambiguous.get(&(c as u32)).and_then(|code| char::from_u32(*code)).unwrap_or(c);
            match confusable.is_ascii() {
                true => format!("The character {code_point} could be confused with the ASCII character {}, which is more common in source code.", format_code_point_markdown(confusable)),
                false => format!("The character {code_point} could be confused with the character {}, which is more common in source code.", format_code_point_markdown(confusable)),
            }
        }
        HighlightReason::Invisible => format!("The character {code_point} is invisible."),
        HighlightReason::NonBasicAscii => format!("The character {code_point} is not a basic ASCII character."),
    }
}

/// `ShowExcludeOptions.run`'s picks for `ShowExcludeOptionsArgs`, in its order: label, user setting key, new value.
fn unicode_exclude_options(args: &Value, user_locales: Option<Value>, allowed_characters: Value) -> Option<Vec<(String, &'static str, Value)>> {
    let c = args["codePoint"].as_u64().and_then(|code| char::from_u32(code as u32))?;
    let reason = &args["reason"];
    let mut options = Vec::new();
    if reason["kind"] == 0 {
        for locale in reason["notAmbiguousInLocales"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            let mut value = user_locales.clone().filter(Value::is_object).unwrap_or_else(|| json!({}));
            value[locale] = json!(true);
            options.push((format!("Allow unicode characters that are more common in the language \"{locale}\"."), "editor.unicodeHighlight.allowedLocales", value));
        }
    }
    let mut allowed = Some(allowed_characters).filter(Value::is_object).unwrap_or_else(|| json!({}));
    allowed[c.to_string()] = json!(true);
    let hex = code_point_to_hex(c);
    let label = match is_invisible_character(c) {
        true => format!("Exclude {hex} (invisible character) from being highlighted"),
        false => format!("Exclude {hex} \"{c}\" from being highlighted"),
    };
    options.push((label, "editor.unicodeHighlight.allowedCharacters", allowed));
    if args["inComment"] == true {
        options.push(("Disable highlighting of characters in comments".into(), "editor.unicodeHighlight.includeComments", json!(false)));
    } else if args["inString"] == true {
        options.push(("Disable highlighting of characters in strings".into(), "editor.unicodeHighlight.includeStrings", json!(false)));
    }
    let (label, key) = match reason["kind"].as_u64() {
        Some(0) => ("Disable highlighting of ambiguous characters", "editor.unicodeHighlight.ambiguousCharacters"),
        Some(1) => ("Disable highlighting of invisible characters", "editor.unicodeHighlight.invisibleCharacters"),
        _ => ("Disable highlighting of non basic ASCII characters", "editor.unicodeHighlight.nonBasicASCII"),
    };
    options.push((label.into(), key, json!(false)));
    Some(options)
}
