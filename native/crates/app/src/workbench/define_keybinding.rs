//! keybindingsEditorContribution.ts and the Define Keybinding action of preferences.contribution.ts: in the user
//! keybindings.json, Define Keybinding (Cmd+K Cmd+K, the floating button) records keys over the editor and inserts a
//! rule where `SmartSnippetInserter` finds room; keys the layout cannot type, or types under another name, are marked.
//! The marks' hover messages wait for the editor hover (editor/contrib/hover).

use std::ops::Range;
use std::time::Duration;

use config::json::{self, NodeType, ParseOptions};
use gpui::{
    AnyElement, Context, Div, IntoElement, ParentElement, Rgba, Styled, Task, Window, div, px, rgb, rgba,
};
use serde_json::Value;

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use crate::commands::Outcome;
use crate::editor::{OpenEditor, VerticalReveal};
use crate::keybindings;
use crate::keyboard_mapper;
use crate::smart_snippet_inserter::{insert_snippet, offset_to_position};

pub const DEFINE_KEYBINDING_COMMAND: &str = "editor.action.defineKeybinding";
const MARKS_DELAY: Duration = Duration::from_millis(500);

/// `KeybindingEditorDecorationsRenderer`'s decorations of the open user keybindings.json.
#[derive(Default)]
pub struct KeybindingMarks {
    model_id: Option<u32>,
    marks: Vec<KeybindingMark>,
    update: Option<Task<()>>,
}

/// A `"key"` value the layout cannot type (`keybindingError`) or types under another name (`keybindingInfo`).
#[derive(Clone, Debug, PartialEq)]
pub struct KeybindingMark {
    pub line: usize,
    pub columns: Range<usize>,
    pub error: bool,
    pub message: String,
}

/// `_getDecorationForEntry` of one key string on the layout of `raw_mapping`: whether it is the error, and the hover
/// message; `None` unmarked. A layout without a key map dispatches on key codes, where every key resolves.
pub fn key_mark(key: &str, raw_mapping: &Value) -> Option<(bool, String)> {
    let chords = keybindings::parse_keybinding(key);
    let unproducible = Some((true, "You won't be able to produce this key combination under your current keyboard layout.".into()));
    if chords.is_empty() {
        return unproducible;
    }
    let (label, user_settings_label, wysiwyg) = match keyboard_mapper::for_layout(&Value::Null, raw_mapping, false, false) {
        Some(mapper) => {
            let resolved = mapper.resolve_keybinding(&chords);
            let Some(first) = resolved.first() else { return unproducible };
            (mapper.label(first), mapper.user_settings_label(first), mapper.is_wysiwyg_keybinding(first))
        }
        None => (Some(keybindings::label(&chords)), Some(keybindings::user_settings_label(&chords)), true),
    };
    // `**{0}** for your current keyboard layout.`, plain text until the editor hover renders markdown.
    let info = Some((false, format!("{} for your current keyboard layout.", label.clone().unwrap_or_default())));
    if !wysiwyg {
        // Coincidentally WYSIWYG: the key string is the label.
        if label.is_some_and(|label| label.to_lowercase() == key.to_lowercase()) {
            return None;
        }
        return info;
    }
    if key.contains("abnt_") || key.contains("oem_") {
        return info;
    }
    if user_settings_label.is_some_and(|expected| !keybindings::user_settings_fuzzy_equals(key, &expected)) {
        return info;
    }
    None
}

/// `_updateDecorationsNow`: each `"key"` string property of the top-level array's objects.
pub fn keybinding_marks(lines: &[String], eol: &str, raw_mapping: &Value) -> Vec<KeybindingMark> {
    let text = lines.join(eol);
    let tree = json::parse_tree(&text, &mut Vec::new(), ParseOptions::default());
    let Some(root) = tree.root else {
        return Vec::new();
    };
    let mut marks = Vec::new();
    for &entry in &tree.node(root).children {
        let properties = tree.node(entry).children.iter().map(|&property| tree.node(property));
        let key = properties
            .filter(|property| property.kind == NodeType::Property && property.children.len() == 2)
            .find(|property| tree.node(property.children[0]).value == "key");
        let Some(value) = key.map(|property| tree.node(property.children[1])).filter(|value| value.kind == NodeType::String) else {
            continue;
        };
        let Some((error, message)) = key_mark(value.value.as_str().expect("a string node"), raw_mapping) else {
            continue;
        };
        let start = offset_to_position(lines, eol, value.offset);
        let end = offset_to_position(lines, eol, value.offset + value.length);
        marks.push(KeybindingMark { line: start.line, columns: start.column..end.column, error, message });
    }
    marks
}

impl Workbench {
    pub(super) fn register_define_keybinding_command(&mut self) {
        self.commands.register(DEFINE_KEYBINDING_COMMAND, |workbench, _, window, cx| {
            workbench.show_define_keybinding_widget(window, cx);
            Outcome::done()
        });
    }

    /// `isInterestingEditorModel`.
    pub(super) fn is_user_keybindings(&self, editor: &OpenEditor) -> bool {
        editor.document().scheme == "file" && self.keybindings.user_file.as_ref() == Some(&editor.document().path)
    }

    /// `showDefineKeybindingWidget` and `DefineKeybindingOverlayWidget.start`.
    fn show_define_keybinding_widget(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editors.active().is_some_and(|editor| self.is_user_keybindings(editor)) {
            return;
        }
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("checked above");
        let position = editor.selections[0].head;
        editor.reveal_range(position, position, VerticalReveal::CenterIfOutsideViewport, viewport);
        self.show_define_widget(None, window, cx);
    }

    /// `_onAccepted`: the editor takes the focus back, and a recorded key inserts a rule snippet.
    pub(super) fn define_overlay_accepted(&mut self, key: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.editor_focus, cx);
        let Some(mut key) = key else { return };
        let Some(editor) = self.editors.active_mut() else {
            return;
        };
        if key.contains('\\') {
            key.pop();
            key.push_str("\\\\");
        }
        let template = [
            "{".to_string(),
            format!("\t\"key\": {},", Value::String(key)),
            "\t\"command\": \"${1:commandId}\",".into(),
            "\t\"when\": \"${2:editorTextFocus}\"".into(),
            "}$0".into(),
        ]
        .join("\n");
        let smart = insert_snippet(&editor.document().lines, editor.document().eol, editor.selections[0].head);
        let template = format!("{}{template}{}", smart.prepend, smart.append);
        let snippets = [(smart.position, smart.position, template.as_str(), false)];
        self.insert_snippets(super::snippet_controller::SnippetInsert { snippets: &snippets, clipboard: None, undo_stop_before: true, undo_stop_after: true, reason: "snippet" }, cx);
    }

    /// `_updateDecorations.schedule()`: the marks follow the text and the keybindings 500ms later.
    pub(super) fn schedule_keybinding_marks(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active().filter(|editor| self.is_user_keybindings(editor)) else {
            self.keybinding_marks = KeybindingMarks::default();
            return;
        };
        let model_id = editor.model_id;
        self.keybinding_marks.update = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(MARKS_DELAY).await;
            let _ = workbench.update(cx, |workbench, cx| {
                let Some(editor) = workbench.editors.active().filter(|editor| editor.model_id == model_id) else {
                    return;
                };
                let marks = keybinding_marks(&editor.document().lines, editor.document().eol, &crate::keyboard_layout::current_raw_mapping());
                workbench.keybinding_marks = KeybindingMarks { model_id: Some(model_id), marks, update: None };
                cx.notify();
            });
        }));
    }

    fn marks_of(&self, editor: &OpenEditor) -> &[KeybindingMark] {
        if self.keybinding_marks.model_id != Some(editor.model_id) {
            return &[];
        }
        &self.keybinding_marks.marks
    }

    /// keybindings.css `.keybindingInfo` and `.keybindingError` on line `index`: a 1px #B9B9B9 inset outline over
    /// a blue or red tint, with its hover message; `x` maps a char column to its left edge.
    /// shortcut: the message shows in the workbench hover until editor/contrib/hover is ported (milestone 11).
    pub(super) fn keybinding_mark_boxes(&self, editor: &OpenEditor, index: usize, x: impl Fn(usize) -> f32) -> Vec<Div> {
        let marks = self.marks_of(editor).iter().filter(|mark| mark.line == index);
        marks
            .map(|mark| {
                let background = if mark.error { rgba(0xFA646433) } else { rgba(0x6464FA33) };
                let (left, right) = (x(mark.columns.start), x(mark.columns.end));
                let hover = self.hover_target(
                    format!("keybinding-mark-{index}"),
                    HoverContent::Text(mark.message.clone()),
                    HoverOptions::instant(),
                );
                div()
                    .absolute()
                    .top_0()
                    .h_full()
                    .left(px(left))
                    .w(px(right - left))
                    .bg(background)
                    .border_1()
                    .border_color(rgb(0xB9B9B9))
                    .child(hover)
            })
            .collect()
    }

    /// The marks' line ranges in `OverviewRulerLane.Right`, the info ones then the error ones, each with its color.
    pub(super) fn keybinding_ruler_marks(&self, editor: &OpenEditor) -> [(Vec<(usize, usize)>, Rgba); 2] {
        let lines =
            |error: bool| self.marks_of(editor).iter().filter(|mark| mark.error == error).map(|mark| (mark.line, mark.line)).collect();
        [(lines(false), self.color("editorOverviewRuler.infoForeground")), (lines(true), self.color("editorOverviewRuler.errorForeground"))]
    }

    /// The `DefineKeybindingOverlayWidget` while it records keys in the user keybindings.json.
    pub(super) fn define_keybinding_overlay(&self, editor: &OpenEditor, width: f32, height: f32, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.is_user_keybindings(editor) || !self.define_widget_shown(true) {
            return None;
        }
        // The workbench's text style, not the editor's monospace lines it sits over.
        let widget = self.define_keybinding_widget(width, height, window, cx).font_family(".SystemUIFont").text_size(px(13.)).line_height(gpui::phi());
        Some(widget.into_any_element())
    }
}

#[cfg(test)]
mod tests;
