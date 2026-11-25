//! wordHighlighter.ts with extension `DocumentHighlightProvider`s: where a provider serves the model, its occurrences
//! (Text, Read, Write) replace the textual word highlights the editor computes on an explicit cursor move. The
//! textual provider (textualHighlightProvider.ts) is the editor's own `refresh_highlights`, the last in score order.

use std::ops::Range;

use futures::channel::oneshot;
use gpui::{Context, Rgba, Task};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::{OpenEditor, Position, char_column, utf16_column};
use crate::language_features::Candidate;

const REGISTER: &str = "$registerDocumentHighlightProvider";
const TEXT: u64 = 0;
const WRITE: u64 = 2;

type Answer = Result<Result<Value, Value>, oneshot::Canceled>;

#[derive(Default)]
pub(super) struct DocumentHighlights {
    /// The query the providers were asked for: the model, its version and the cursor.
    key: Option<(u32, u32, Position)>,
    /// The providers' occurrences of the model: line, char columns and `DocumentHighlightKind`.
    ranges: Vec<(usize, Range<usize>, u64)>,
    _request: Option<Task<()>>,
}

impl Workbench {
    pub(super) fn register_document_highlight_commands(&mut self) {
        // `_executeDocumentHighlights` (registerModelAndPositionCommand): the occurrences at a position of a model.
        self.commands.register("_executeDocumentHighlights", |workbench, args, _, cx| {
            let (Some(uri), Some(position)) = (args.first(), args.get(1)) else { return Outcome::error("Illegal argument") };
            let Ok((scheme, path)) = super::host_requests::uri_parts(uri) else { return Outcome::error("Illegal argument") };
            let Some(editor) = workbench.editors.editors.iter().chain(&workbench.editors.models).find(|editor| editor.document.is(scheme, &path)) else {
                return Outcome::error("Illegal argument");
            };
            let request = workbench.occurrences_at_position(editor, position.clone());
            let textual = textual_highlights(editor, position);
            let (sender, receiver) = oneshot::channel();
            cx.spawn(async move |_, _| {
                let answer = match request {
                    Some(request) => request.await,
                    None => None,
                };
                let _ = sender.send(Ok(Value::Array(answer.unwrap_or(textual))));
            })
            .detach();
            Outcome::Pending(receiver)
        });
    }

    /// `getOccurrencesAtPosition` over the extension providers: `None` when none serves the model; the future gives
    /// `None` when every provider declined (null), which leaves the textual provider to answer. With
    /// `editor.occurrencesHighlightFromAllProviders` every provider is asked and the answers merged without
    /// duplicate ranges; else the first provider in score order whose answer is an array wins, even an empty one.
    fn occurrences_at_position(&self, editor: &OpenEditor, position: Value) -> Option<impl Future<Output = Option<Vec<Value>>> + use<>> {
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        let handles: Vec<u64> = self.language_features.ordered(REGISTER, &candidate).iter().map(|registration| registration.handle).collect();
        if handles.is_empty() {
            return None;
        }
        let merge = self.setting("editor.occurrencesHighlightFromAllProviders") == json!(true);
        let calls: Vec<_> = handles.iter().map(|handle| self.host.call_cancellable("ExtHostLanguageFeatures", "$provideDocumentHighlights", vec![json!(handle), uri.clone(), position.clone()])).collect();
        Some(async move {
            match merge {
                true => Some(merge_highlights(futures::future::join_all(calls).await)),
                false => {
                    for call in calls {
                        if let Ok(Ok(Value::Array(highlights))) = call.await {
                            return Some(highlights);
                        }
                    }
                    None
                }
            }
        })
    }

    /// Run on every render after the editor's textual highlights: a new word query asks the providers; a cursor
    /// inside the shown occurrences keeps them (`_run`).
    pub(super) fn refresh_document_highlights(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let head = editor.selection().head;
        let key = Some((editor.model_id, editor.version_id, head));
        if self.document_highlights.key == key {
            return;
        }
        self.document_highlights.key = key;
        // No textual word at the cursor: the query is cleared (`_stopAll`).
        if editor.highlights.is_empty() {
            self.document_highlights.ranges.clear();
            self.document_highlights._request = None;
            return;
        }
        let inside = self.document_highlights.ranges.iter().any(|(line, columns, _)| *line == head.line && columns.start <= head.column && head.column <= columns.end);
        if inside {
            let ranges = self.document_highlights.ranges.iter().map(|(line, columns, _)| (*line, columns.clone())).collect();
            self.editors.active_mut().expect("checked above").highlights = ranges;
            return;
        }
        let position = json!({ "lineNumber": head.line + 1, "column": utf16_column(&editor.document.lines[head.line], head.column) + 1 });
        let Some(request) = self.occurrences_at_position(editor, position) else {
            self.document_highlights = DocumentHighlights::default();
            return;
        };
        let editor = self.editors.active_mut().expect("checked above");
        let textual = std::mem::take(&mut editor.highlights);
        self.document_highlights.ranges.clear();
        self.document_highlights._request = Some(cx.spawn(async move |workbench, cx| {
            let answer = request.await;
            let _ = workbench.update(cx, |workbench, cx| workbench.document_highlights_answered(key, answer, textual, cx));
        }));
    }

    fn document_highlights_answered(&mut self, key: Option<(u32, u32, Position)>, highlights: Option<Vec<Value>>, textual: Vec<(usize, Range<usize>)>, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active_mut().filter(|editor| Some((editor.model_id, editor.version_id, editor.selection().head)) == key) else { return };
        let Some(highlights) = highlights else {
            editor.highlights = textual;
            return cx.notify();
        };
        let lines = &editor.document.lines;
        let mut ranges = Vec::new();
        for highlight in &highlights {
            let range = &highlight["range"];
            let number = |key: &str| range[key].as_u64().map(|value| value as usize - 1);
            let (Some(start_line), Some(start), Some(end_line), Some(end)) = (number("startLineNumber"), number("startColumn"), number("endLineNumber"), number("endColumn")) else { continue };
            let kind = highlight["kind"].as_u64().unwrap_or(1);
            for line in start_line..=end_line.min(lines.len() - 1) {
                let from = if line == start_line { char_column(&lines[line], start) } else { 0 };
                let to = if line == end_line { char_column(&lines[line], end) } else { lines[line].chars().count() };
                ranges.push((line, from..to, kind));
            }
        }
        editor.highlights = ranges.iter().map(|(line, columns, _)| (*line, columns.clone())).collect();
        self.document_highlights.ranges = ranges;
        cx.notify();
    }

    /// The overview ruler lines of the word highlights by kind: Text, Read (and no kind), Write, with their colors.
    pub(super) fn word_highlight_ruler_lines(&self, editor: &OpenEditor) -> [(Vec<(usize, usize)>, Rgba); 3] {
        let mut lines = [Vec::new(), Vec::new(), Vec::new()];
        for (line, columns) in &editor.highlights {
            lines[self.word_highlight_kind(*line, columns).min(2) as usize].push((*line, *line));
        }
        let [text, read, write] = lines;
        [
            (text, self.color("editorOverviewRuler.wordHighlightTextForeground")),
            (read, self.color("editorOverviewRuler.wordHighlightForeground")),
            (write, self.color("editorOverviewRuler.wordHighlightStrongForeground")),
        ]
    }

    /// highlightDecorations.ts: `wordHighlightStrong` for Write, `wordHighlightText` for Text and the textual
    /// provider, `wordHighlight` for Read and no kind; background and border.
    pub(super) fn word_highlight_colors(&self, line: usize, columns: &Range<usize>) -> (Option<Rgba>, Option<Rgba>) {
        let (background, border) = match self.word_highlight_kind(line, columns) {
            TEXT => ("editor.wordHighlightTextBackground", "editor.wordHighlightTextBorder"),
            WRITE => ("editor.wordHighlightStrongBackground", "editor.wordHighlightStrongBorder"),
            _ => ("editor.wordHighlightBackground", "editor.wordHighlightBorder"),
        };
        (self.theme.color(background), self.theme.color(border))
    }

    fn word_highlight_kind(&self, line: usize, columns: &Range<usize>) -> u64 {
        self.document_highlights.ranges.iter().find(|(other, range, _)| *other == line && range == columns).map_or(TEXT, |(_, _, kind)| *kind)
    }
}

/// textualHighlightProvider.ts: the whole-word occurrences of the word at a 1-based UTF-16 position, kind Text.
fn textual_highlights(editor: &OpenEditor, position: &Value) -> Vec<Value> {
    let lines = &editor.document.lines;
    let line = (position["lineNumber"].as_u64().unwrap_or(1) as usize).saturating_sub(1).min(lines.len() - 1);
    let column = char_column(&lines[line], (position["column"].as_u64().unwrap_or(1) as usize).saturating_sub(1));
    let Some(word) = crate::editor::word_at(&lines[line], column) else { return Vec::new() };
    let word: String = lines[line].chars().skip(word.start).take(word.len()).collect();
    let mut highlights = Vec::new();
    for (line, columns) in crate::editor::word_occurrences(lines, &word) {
        let text = &lines[line];
        let range = json!({ "startLineNumber": line + 1, "startColumn": utf16_column(text, columns.start) + 1, "endLineNumber": line + 1, "endColumn": utf16_column(text, columns.end) + 1 });
        highlights.push(json!({ "range": range, "kind": TEXT }));
    }
    highlights
}

/// `coalesce` and `mergeHighlights`: the arrays answered, without a second highlight of the same range. Upstream
/// leaves the textual provider out of a merge when another provider matches, so no answer is no highlights.
fn merge_highlights(answers: Vec<Answer>) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::new();
    for answer in answers {
        let Ok(Ok(Value::Array(highlights))) = answer else { continue };
        for highlight in highlights {
            if !merged.iter().any(|other| other["range"] == highlight["range"]) {
                merged.push(highlight);
            }
        }
    }
    merged
}

#[cfg(test)]
mod tests;
