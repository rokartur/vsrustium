//! snippetController2.ts: inserting snippets with a session, Tab and Shift+Tab through the placeholders, Escape to
//! leave, the choice list, the `inSnippetMode` keys and the placeholder decorations.

use gpui::{Context, Div, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::host_requests::snippet_edits;
use super::suggest_controller::TriggerOptions;
use crate::commands::Outcome;
use crate::editor::snippet_session::{Inserted, SnippetSession, starts_after_edits};
use crate::editor::{EditReason, OpenEditor, Position, visible_column};

/// The handle of the choice completions the controller registers while a choice is active.
pub(super) const SNIPPET_CHOICE_PROVIDER: u64 = u64::MAX - 1;

pub(super) struct SnippetInsert<'a> {
    pub snippets: &'a [(Position, Position, &'a str, bool)],
    pub clipboard: Option<&'a str>,
    pub undo_stop_before: bool,
    pub undo_stop_after: bool,
    pub reason: &'static str,
}

impl Workbench {
    pub(super) fn register_snippet_commands(&mut self) {
        self.commands.register("jumpToNextSnippetPlaceholder", |workbench, _, _, cx| {
            workbench.move_snippet(true, cx);
            Outcome::done()
        });
        self.commands.register("jumpToPrevSnippetPlaceholder", |workbench, _, _, cx| {
            workbench.move_snippet(false, cx);
            Outcome::done()
        });
        self.commands.register("leaveSnippet", |workbench, _, _, cx| {
            workbench.cancel_snippet(true, cx);
            Outcome::done()
        });
        self.commands.register("acceptSnippet", |workbench, _, _, cx| {
            // `finish`: next until the session ends.
            while workbench.editors.active().is_some_and(|editor| editor.snippet_session.is_some()) {
                workbench.move_snippet(true, cx);
            }
            Outcome::done()
        });
    }

    /// `_doInsert`: the templates replace their ranges in one edit and a session starts on their placeholders.
    /// Text without placeholders inside a running session is written into it, as `SnippetSession.merge` of a trivial
    /// snippet. shortcut: a snippet with placeholders ends the running session instead of merging into it; port the
    /// merge for nested snippet completions.
    pub(super) fn insert_snippets(&mut self, insert: SnippetInsert, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active_mut() else { return };
        if insert.undo_stop_before {
            editor.push_undo_stop();
        }
        let version = editor.alternative_version_id();
        let (edits, selections, texts) = snippet_edits(editor, insert.snippets, insert.clipboard);
        let has_placeholders = texts.iter().any(|text| !text.stops.is_empty());
        if has_placeholders {
            editor.snippet_session = None;
        }
        let change = editor.execute_edits(edits, Some(selections), EditReason::Unknown(insert.reason));
        if insert.undo_stop_after {
            editor.push_undo_stop();
        }
        let replaced: Vec<(Position, Position, &str)> = insert.snippets.iter().zip(&texts).map(|(snippet, text)| (snippet.0, snippet.1, text.text.as_str())).collect();
        let starts = starts_after_edits(&replaced);
        let inserted: Vec<Inserted> = texts.iter().zip(starts).map(|(text, start)| Inserted { start, text: &text.text, stops: &text.stops, choices: &text.choices }).collect();
        let mut session = SnippetSession::new(&inserted, version);
        if has_placeholders {
            let selections = session.move_to(true);
            editor.set_selections(selections);
            editor.snippet_session = Some(session);
        }
        self.editor_edited(change, true, "snippet", cx);
    }

    fn move_snippet(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active_mut() else { return };
        let Some(session) = editor.snippet_session.as_mut() else { return };
        let before = editor.selections.clone();
        let selections = session.move_to(forward);
        editor.set_selections(selections);
        self.cursors_changed(before, crate::editor::Reveal::All, "keyboard", cx);
        self.snippet_state_changed(cx);
    }

    /// `cancel`: the session ends; leaving with Escape keeps the primary selection only.
    pub(super) fn cancel_snippet(&mut self, reset_selection: bool, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active_mut() else { return };
        if editor.snippet_session.take().is_none() {
            return;
        }
        if reset_selection {
            let primary = editor.selection();
            editor.set_selections(vec![primary]);
        }
        cx.notify();
    }

    /// `_updateState`, after every cursor change: undoing the insert, reaching the last placeholder or leaving the
    /// placeholders ends the session; an active choice shows its options.
    pub(super) fn snippet_state_changed(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active_mut() else { return };
        let Some(session) = editor.snippet_session.as_ref() else { return };
        if editor.alternative_version_id() == session.version || !session.has_placeholder() {
            editor.snippet_session = None;
            return;
        }
        if session.is_at_last() || !session.selections_within(&editor.selections) {
            editor.push_undo_stop();
            editor.snippet_session = None;
            return;
        }
        let choice = session.active_choice().map(|(_, start, end)| (start, end));
        if choice.is_some() && choice != self.snippet_choice_shown {
            self.snippet_choice_shown = choice;
            // `queueMicrotask(showSimpleSuggestions)`: after the cursor events of this change, a manual trigger of the
            // choice provider alone, without the kind filter.
            cx.spawn(async |workbench, cx| {
                let options = TriggerOptions { provider_filter: vec![SNIPPET_CHOICE_PROVIDER], no_kind_filter: true, ..TriggerOptions::new(false) };
                let _ = workbench.update(cx, |workbench, cx| workbench.trigger_suggest(options, cx));
            })
            .detach();
        }
        if choice.is_none() {
            self.snippet_choice_shown = None;
        }
    }

    /// The choice provider of `_doInsert`: each option replaces the choice's range, sorted as given, then Tab moves on.
    pub(super) fn snippet_choice_completions(&self, editor: &OpenEditor) -> Value {
        let Some((options, start, end)) = editor.snippet_session.as_ref().and_then(SnippetSession::active_choice) else { return Value::Null };
        if editor.selection().head != end && editor.selection().head != start {
            return Value::Null;
        }
        let lines = &editor.document().lines;
        let word: String = lines[start.line].chars().skip(start.column).take(end.column.saturating_sub(start.column)).collect();
        let any_of_options = options.iter().any(|option| *option == word);
        let column = |position: Position| crate::editor::utf16_column(&lines[position.line], position.column) + 1;
        let range = json!({ "startLineNumber": start.line + 1, "startColumn": column(start), "endLineNumber": end.line + 1, "endColumn": column(end) });
        let suggestions: Vec<Value> = options
            .iter()
            .enumerate()
            .map(|(index, option)| {
                let mut item = json!({ "a": option, "b": 13, "h": option, "e": "a".repeat(index + 1), "j": range, "o": "jumpToNextSnippetPlaceholder" });
                if any_of_options {
                    item["f"] = json!(format!("{word}_{option}"));
                }
                item
            })
            .collect();
        json!({ "a": { "insert": range, "replace": range }, "b": suggestions })
    }

    /// The context keys of the controller.
    pub(super) fn snippet_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        let session = self.editors.active().and_then(|editor| editor.snippet_session.as_ref());
        context.insert("inSnippetMode".into(), json!(session.is_some()));
        context.insert("hasNextTabstop".into(), json!(session.is_some_and(|session| !session.is_at_last())));
        context.insert("hasPrevTabstop".into(), json!(session.is_some_and(|session| !session.is_at_first())));
        context.insert("inSnippetChoice".into(), json!(session.is_some_and(|session| session.active_choice().is_some())));
    }

    /// snippetSession.css `.snippet-placeholder` and `.finish-snippet-placeholder` on model line `index`: the
    /// background and the 1px outline; an empty placeholder is 2px wide.
    pub(super) fn snippet_placeholder_highlights(&self, editor: &OpenEditor, index: usize, char_width: f32) -> Vec<Div> {
        let Some(session) = editor.snippet_session.as_ref() else { return Vec::new() };
        let text = &editor.document().lines[index];
        let tab_size = editor.document().indentation.tab_size;
        let mut highlights = Vec::new();
        for (placeholder, _) in session.placeholders().filter(|(placeholder, _)| placeholder.start.line <= index && placeholder.end.line >= index) {
            let from = if placeholder.start.line == index { placeholder.start.column } else { 0 };
            let to = if placeholder.end.line == index { placeholder.end.column } else { text.chars().count() };
            let left = visible_column(text, from, tab_size) as f32 * char_width;
            let width = (visible_column(text, to, tab_size) as f32 * char_width - left).max(if placeholder.is_final { 0. } else { 2. });
            let (background, border) = match placeholder.is_final {
                true => ("editor.snippetFinalTabstopHighlightBackground", "editor.snippetFinalTabstopHighlightBorder"),
                false => ("editor.snippetTabstopHighlightBackground", "editor.snippetTabstopHighlightBorder"),
            };
            let mut highlight = div().absolute().top_0().h_full().left(px(left)).w(px(width));
            if let Some(background) = self.theme.color(background) {
                highlight = highlight.bg(background);
            }
            if let Some(border) = self.theme.color(border) {
                highlight = highlight.border_1().border_color(border);
            }
            highlights.push(highlight);
        }
        highlights
    }
}
