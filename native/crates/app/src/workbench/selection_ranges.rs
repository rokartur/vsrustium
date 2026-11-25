//! The extension `SelectionRangeProvider`s of smartSelect.ts: `provideSelectionRanges` asks every provider of the
//! model for the cursors' positions, then Expand and Shrink Selection run over their ranges and the word ones
//! (editor/smart_select.rs); `_executeSelectionRangeProvider` answers the same ranges.

use futures::future::join_all;
use gpui::Context;
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::smart_select::{Options, Span};
use crate::editor::{OpenEditor, Position, char_column, utf16_column};
use crate::language_features::Candidate;

#[cfg(test)]
mod tests;

const REGISTER: &str = "$registerSelectionRangeProvider";

impl Workbench {
    pub(super) fn register_selection_ranges(&mut self) {
        self.commands.register("_executeSelectionRangeProvider", |workbench, args, _, cx| workbench.execute_selection_range_provider(&args, cx));
    }

    /// `run` with providers: `None` when the model has none, so the bracket ranges apply at once.
    pub(super) fn smart_select_with_providers(&mut self, forward: bool, options: Options, cx: &mut Context<Self>) -> Option<Outcome> {
        let editor = self.editors.active()?;
        let positions: Vec<Position> = editor.selections.iter().map(|selection| selection.head).collect();
        let answers = self.ask_selection_range_providers(editor, &positions)?;
        let (model_id, selections_before) = (editor.model_id, editor.selections.clone());
        cx.spawn(async move |workbench, cx| {
            let answers = answers.await;
            let _ = workbench.update(cx, |workbench, cx| {
                let Some(editor) = workbench.editors.active().filter(|editor| editor.model_id == model_id) else { return };
                let provided = provided_ranges(&editor.document().lines, &answers, positions.len());
                workbench.finish_smart_select(forward, options, model_id, selections_before, &provided, cx);
            });
        })
        .detach();
        Some(Outcome::done())
    }

    /// `$provideSelectionRanges` of every provider of the editor's model at `positions` (0-based char columns).
    fn ask_selection_range_providers(&self, editor: &OpenEditor, positions: &[Position]) -> Option<impl Future<Output = Vec<Value>> + use<>> {
        let uri = editor.document().uri();
        let candidate = Candidate { uri: &uri, language: &editor.document().language_id };
        let handles: Vec<u64> = self.language_features.ordered(REGISTER, &candidate).iter().map(|registration| registration.handle).collect();
        if handles.is_empty() {
            return None;
        }
        let lines = &editor.document().lines;
        let dtos: Vec<Value> = positions.iter().map(|position| json!({ "lineNumber": position.line + 1, "column": utf16_column(&lines[position.line], position.column) + 1 })).collect();
        let calls: Vec<_> = handles.iter().map(|handle| self.host.call_cancellable("ExtHostLanguageFeatures", "$provideSelectionRanges", vec![json!(handle), uri.clone(), json!(dtos)])).collect();
        Some(async move { join_all(calls).await.into_iter().map(|answer| answer.ok().and_then(Result::ok).unwrap_or(Value::Null)).collect() })
    }

    /// `_executeSelectionRangeProvider(resource, positions)` with both smart select options on: the ranges per
    /// position as `IRange`s, inner first.
    fn execute_selection_range_provider(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = super::host_requests::uri_parts(args.first().unwrap_or(&Value::Null)).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path)) else {
            return Outcome::error("Illegal argument");
        };
        let Some(dtos) = args.get(1).and_then(Value::as_array) else { return Outcome::error("Illegal argument") };
        let lines = &editor.document().lines;
        let mut positions = Vec::new();
        for dto in dtos {
            let line = (dto["lineNumber"].as_u64().unwrap_or(1) as usize).clamp(1, lines.len()) - 1;
            let column = char_column(&lines[line], (dto["column"].as_u64().unwrap_or(1) as usize).saturating_sub(1));
            positions.push(Position::new(line, column));
        }
        let options = Options { select_leading_and_trailing_whitespace: true, select_subwords: true };
        let model_id = editor.model_id;
        let Some(answers) = self.ask_selection_range_providers(editor, &positions) else {
            return Outcome::Ready(Ok(self.selection_ranges_value(model_id, options, &positions, None)));
        };
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let answers = answers.await;
            let value = workbench.update(cx, |workbench, _| {
                let editor = workbench.editors.editors.iter().find(|editor| editor.model_id == model_id)?;
                let provided = provided_ranges(&editor.document().lines, &answers, positions.len());
                Some(workbench.selection_ranges_value(model_id, options, &positions, Some(&provided)))
            });
            let _ = sender.send(Ok(value.ok().flatten().unwrap_or(json!([]))));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    fn selection_ranges_value(&self, model_id: u32, options: Options, positions: &[Position], provided: Option<&[Vec<Span>]>) -> Value {
        let Some(editor) = self.editors.editors.iter().find(|editor| editor.model_id == model_id) else { return json!([]) };
        let one_based: Vec<Position> = positions.iter().map(|position| Position::new(position.line, position.column + 1)).collect();
        let lines = &editor.document().lines;
        let ranges = editor.provide_selection_ranges(&self.languages, options, &one_based, provided);
        let range = |(start, end): Span| {
            let column = |position: Position| utf16_column(&lines[position.line], position.column.saturating_sub(1)) + 1;
            json!({ "startLineNumber": start.line + 1, "startColumn": column(start), "endLineNumber": end.line + 1, "endColumn": column(end) })
        };
        Value::Array(ranges.into_iter().map(|ranges| Value::Array(ranges.into_iter().map(range).collect())).collect())
    }
}

/// The providers' answers as ranges per position: an answer counts only with one list per position
/// (`allProviderRanges.length === positions.length`); `range` is an `IRange` with UTF-16 columns.
fn provided_ranges(lines: &[String], answers: &[Value], position_count: usize) -> Vec<Vec<Span>> {
    let mut provided = vec![Vec::new(); position_count];
    for answer in answers {
        let Some(lists) = answer.as_array().filter(|lists| lists.len() == position_count) else { continue };
        for (ranges, list) in provided.iter_mut().zip(lists) {
            for item in list.as_array().into_iter().flatten() {
                if let Some(span) = span_of(lines, &item["range"]) {
                    ranges.push(span);
                }
            }
        }
    }
    provided
}

/// An `IRange` (1-based lines, UTF-16 columns) as a span of 0-based lines and 1-based char columns.
fn span_of(lines: &[String], range: &Value) -> Option<Span> {
    let number = |key: &str| range[key].as_u64().map(|value| value as usize);
    let position = |line: usize, column: usize| {
        let line = line.clamp(1, lines.len()) - 1;
        Position::new(line, char_column(&lines[line], column.saturating_sub(1)) + 1)
    };
    Some((position(number("startLineNumber")?, number("startColumn")?), position(number("endLineNumber")?, number("endColumn")?)))
}
