//! Word wrap settings for the active editor (editorOptions.ts `EditorLayoutInfoComputer` wrapping info) and
//! View: Toggle Word Wrap with its context keys (toggleWordWrap.ts).

use std::collections::HashMap;

use config::configuration::Overrides;
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use gpui::{Div, ParentElement, Styled, div, px};

use crate::editor::word_wrap::{Segment, WrapOptions, WrappingIndent};
use crate::editor::{OpenEditor, VERTICAL_SCROLLBAR_WIDTH, Viewport, visible_column};

impl Workbench {
    pub(super) fn register_word_wrap_commands(&mut self) {
        // `ToggleWordWrapAction`: the first toggle overrides the configured wrapping with its opposite, the
        // second drops the override.
        self.commands.register("editor.action.toggleWordWrap", |workbench, _, _, cx| {
            if workbench.editors.active_diff().is_some() {
                return Outcome::done();
            }
            let Some(editor) = workbench.editors.active() else { return Outcome::done() };
            let toggled = match editor.word_wrap.toggled {
                Some(_) => None,
                None => Some(!editor.wraps()),
            };
            workbench.editors.active_mut().expect("checked above").word_wrap.toggled = toggled;
            workbench.configure_word_wrap();
            cx.notify();
            Outcome::done()
        });
    }

    /// `canToggleWordWrap` and `editorWordWrap` of the focused or active code editor.
    pub(super) fn word_wrap_context(&self, context: &mut HashMap<String, Value>) {
        let editor = self.editors.active().filter(|_| self.editors.active_diff().is_none());
        context.insert("canToggleWordWrap".into(), json!(editor.is_some()));
        context.insert("editorWordWrap".into(), json!(editor.is_some_and(OpenEditor::wraps)));
        // `ToggleWordWrapController`: `isWordWrapMinified` needs accessibility mode, which the port does not have.
        context.insert("isWordWrapMinified".into(), json!(false));
        context.insert("isDominatedByLongLines".into(), json!(editor.is_some_and(|editor| editor.word_wrap.dominated_by_long_lines)));
    }

    /// A wrapped view line shows its segment of the model line's content: the content moves left by the
    /// segment's x and is clipped from the wrapped indent to the segment's end, 1px wider on each side for a
    /// cursor at either end (viewCursor.ts draws it 1px left of its column, 2px wide).
    pub(super) fn wrapped_segment_clip(&self, content: Div, segment: Segment, shift: f32, scroll_left: f32, char_width: f32) -> Div {
        if segment.first && segment.last {
            return content;
        }
        let clip_left = segment.indent as f32 * char_width - scroll_left - 1.;
        let clip = div().absolute().top_0().bottom_0().left(px(clip_left)).overflow_hidden().child(content.left(px(-scroll_left - shift - clip_left)));
        if segment.last {
            return clip.right_0();
        }
        let editor = self.editors.active().expect("a wrapped view line has an editor");
        let text = &editor.document.lines[segment.line];
        let tab_size = editor.document.indentation.tab_size;
        let width = (visible_column(text, segment.end, tab_size) - visible_column(text, segment.start, tab_size)) as f32 * char_width;
        clip.w(px(width + 2.))
    }

    /// wordWrapIndicator.ts: `editor.wordWrapIndicator` puts U+21A9 at the wrapping column of every view line
    /// that continues on the next one.
    pub(super) fn word_wrap_indicator(&self, editor: &OpenEditor, segment: Segment, scroll_left: f32, char_width: f32) -> Option<Div> {
        let options = editor.word_wrap.options()?;
        if segment.last || self.setting("editor.wordWrapIndicator") != json!(true) {
            return None;
        }
        let left = options.column as f32 * char_width - scroll_left;
        Some(div().absolute().top_0().left(px(left)).text_color(self.color("editorWordWrapIndicator.foreground")).child("\u{21A9}"))
    }

    /// mouseTarget.ts: a press past a wrapped view line's end puts the cursor at that view line's end, where
    /// its model position alone would show it at the next view line's start.
    pub(super) fn keep_click_at_row_end(&mut self, view_line: usize, visible: f32) {
        let Some(editor) = self.editors.active_mut().filter(|editor| editor.wraps()) else { return };
        let segment = editor.segment(view_line);
        let head = editor.selections.last().expect("an editor has a cursor").head;
        let text = &editor.document.lines[segment.line];
        let end_visible = visible_column(text, segment.end, editor.document.indentation.tab_size) as f32;
        if !segment.last && head == crate::editor::Position::new(segment.line, segment.end) && visible >= end_visible - 0.5 {
            editor.set_row_ends(vec![head]);
        }
    }

    /// `setWrappingSettings` on the active editor with the wrapping of the last layout.
    pub(super) fn configure_word_wrap(&mut self) -> bool {
        let viewport = self.editors.viewport;
        let in_diff = self.editors.active_diff().is_some();
        let Some(editor) = self.editors.active() else { return false };
        let options = if in_diff { None } else { self.wrap_options(editor, viewport) };
        let editor = self.editors.active_mut().expect("checked above");
        let lines = std::mem::take(&mut editor.document.lines);
        let changed = editor.word_wrap.configure(options, &lines);
        editor.document.lines = lines;
        changed
    }

    /// `EditorLayoutInfoComputer.computeLayout`'s wrapping column: `on` wraps at the viewport less the vertical
    /// scrollbar and 2px for the cursor, `wordWrapColumn` at that column, `bounded` at the smaller; a model too
    /// large for tokenization never wraps (viewModelImpl.ts `ViewModelLinesFromModelAsIs`).
    fn wrap_options(&self, editor: &OpenEditor, viewport: Viewport) -> Option<WrapOptions> {
        if editor.document.too_large_for_tokenization || viewport.char_width <= 0. {
            return None;
        }
        let overrides = Overrides { resource: Some(&editor.document.path), override_identifier: Some(&editor.document.language_id) };
        let settings = self.settings.lock().unwrap();
        let value = |key: &str| settings.get_value(Some(key), &overrides).unwrap_or(Value::Null);
        let word_wrap = match editor.word_wrap.toggled {
            Some(true) => "on".to_string(),
            Some(false) => "off".to_string(),
            None => value("editor.wordWrap").as_str().unwrap_or("off").to_string(),
        };
        let word_wrap_column = value("editor.wordWrapColumn").as_u64().unwrap_or(80).max(1) as usize;
        let indicator = value("editor.wordWrapIndicator") == json!(true);
        let viewport_column = (((viewport.width - VERTICAL_SCROLLBAR_WIDTH - 2.) / viewport.char_width).floor() as usize).max(1);
        let viewport_wrapping_column = viewport_column.saturating_sub(usize::from(indicator)).max(1);
        let (column, viewport_wrapping) = match word_wrap.as_str() {
            "on" => (viewport_wrapping_column, true),
            "bounded" => (viewport_wrapping_column.min(word_wrap_column), true),
            "wordWrapColumn" => (word_wrap_column, false),
            _ => return None,
        };
        let string = |key: &str| value(key).as_str().unwrap_or_default().to_string();
        Some(WrapOptions {
            column,
            viewport_wrapping,
            tab_size: editor.document.indentation.tab_size,
            indent: WrappingIndent::from_setting(&string("editor.wrappingIndent")),
            keep_all: string("editor.wordBreak") == "keepAll",
            wrap_on_escaped_line_feeds: value("editor.wrapOnEscapedLineFeeds") == json!(true),
            // shortcut: the font's full-width advance is not measured (fontInfo.ts `typicalFullwidthCharacterWidth`), CJK counts two columns; measure it when CJK text needs exact wrapping.
            columns_for_full_width_char: 2.,
            two_cell_full_width: string("editor.fullwidthCharacterWidth") == "twoCells",
            break_before: string("editor.wordWrapBreakBeforeCharacters"),
            break_after: string("editor.wordWrapBreakAfterCharacters"),
        })
    }
}

