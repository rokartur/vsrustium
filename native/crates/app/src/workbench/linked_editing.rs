//! linkedEditing.ts and linkedEditing.css: with `editor.linkedEditing` (or the deprecated `editor.renameOnType`) the
//! providers' linked ranges at the cursor get the `editor.linkedEditingBackground`; typing in the first (reference)
//! range copies its text into the others. Shift+Cmd+F2 starts it without the setting, Escape ends it.

use std::ops::Range;
use std::time::Duration;

use gpui::{Context, HighlightStyle, Hsla, Task};
use serde_json::{Value, json};

use super::Workbench;
use super::links::{end_after, tracked};
use crate::commands::Outcome;
use crate::editor::{ContentChanged, EditReason, OpenEditor, Position, char_column, expand_tabs, utf16_column};
use crate::editor::in_place_replace::DEFAULT_WORD_REGEXP;
use crate::language_features::Candidate;

const REGISTER: &str = "$registerLinkedEditingRangeProvider";
/// `languageFeatureDebounceService.for(providers, 'Linked Editing', { max: 200 })`: 1.5 times the default minimum.
// shortcut: the delay does not follow the measured request times, port it with the debounce service.
const DEBOUNCE: Duration = Duration::from_millis(75);

#[derive(Default)]
pub(super) struct LinkedEditing {
    /// `_enabled`: the setting is on and a provider serves the model.
    enabled: bool,
    model_id: Option<u32>,
    /// `_currentDecorations`: the reference range first, 1-based lines and UTF-16 columns, `AlwaysGrowsWhenTypingAtEdges`.
    ranges: Vec<((usize, usize), (usize, usize))>,
    /// `_currentRequestPosition` and `_currentRequestModelVersion`.
    request_position: Option<Position>,
    request_version: Option<u32>,
    /// Bumped when the request is cancelled (`_currentRequestCts`).
    request_id: u64,
    update: Option<Task<()>>,
    sync: Option<Task<()>>,
    sync_token: u64,
    /// `_currentWordPattern` when the provider gave one; without it the language's word definition applies.
    word_pattern: Option<onig::Regex>,
    ignore_change: bool,
}

impl Workbench {
    pub(super) fn register_linked_editing(&mut self) {
        self.commands.register("editor.action.linkedEditing", |workbench, _, _, cx| {
            workbench.update_linked_ranges(true, cx);
            Outcome::done()
        });
        self.commands.register("cancelLinkedEditingInput", |workbench, _, _, cx| {
            workbench.clear_linked_ranges();
            cx.notify();
            Outcome::done()
        });
        self.commands.register("_executeLinkedEditingProvider", |workbench, args, _, cx| workbench.execute_linked_editing_provider(&args, cx));
    }

    /// `LinkedEditingInputVisible`.
    pub(super) fn linked_editing_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        context.insert("LinkedEditingInputVisible".into(), json!(!self.linked_editing.ranges.is_empty()));
    }

    /// `reinitialize`: another model or language (`force`), a provider or the setting changed.
    pub(super) fn linked_editing_reinitialize(&mut self, force: bool, cx: &mut Context<Self>) {
        let model_id = self.editors.active().map(|editor| editor.model_id);
        let enabled = self.editors.active().is_some_and(|editor| self.linked_editing_enabled(editor));
        let force = force || model_id != self.linked_editing.model_id;
        if enabled == self.linked_editing.enabled && !force {
            return;
        }
        self.linked_editing.enabled = enabled;
        self.linked_editing.model_id = model_id;
        self.clear_linked_ranges();
        self.linked_editing.update = None;
        self.linked_editing.sync = None;
        if enabled {
            self.update_linked_ranges(false, cx);
        }
    }

    fn linked_editing_enabled(&self, editor: &OpenEditor) -> bool {
        let setting_on = ["editor.linkedEditing", "editor.renameOnType"].iter().any(|key| self.language_setting(editor, key) == json!(true));
        setting_on && !self.linked_editing_handles(editor).is_empty()
    }

    fn linked_editing_handles(&self, editor: &OpenEditor) -> Vec<u64> {
        let uri = editor.document().uri();
        let candidate = Candidate { uri: &uri, language: &editor.document().language_id };
        self.language_features.ordered(REGISTER, &candidate).iter().map(|registration| registration.handle).collect()
    }

    /// `onDidChangeModelContent` then `onDidChangeCursorPosition`: the ranges follow the edit; an edit inside the
    /// reference range syncs the others at once, any other edit or a cursor move asks for the ranges again.
    pub(super) fn linked_editing_editor_changed(&mut self, change: Option<&ContentChanged>, moved: bool, cx: &mut Context<Self>) {
        if !self.linked_editing.enabled && self.linked_editing.ranges.is_empty() {
            return;
        }
        if let Some(change) = change {
            let reference = self.linked_editing.ranges.first().copied();
            for content in &change.changes {
                let start = (content.start_line, content.start_column);
                let end = (content.end_line, content.end_column);
                let new_end = end_after(start, &content.text);
                for range in &mut self.linked_editing.ranges {
                    range.0 = tracked(range.0, start, end, new_end, true);
                    range.1 = tracked(range.1, start, end, new_end, false);
                }
            }
            // `Range.intersectRanges` before the edit: touching counts, as an empty intersection is still a range.
            let inside = |(start, end): ((usize, usize), (usize, usize))| {
                change.changes.iter().all(|content| (content.start_line, content.start_column) <= end && start <= (content.end_line, content.end_column))
            };
            if !self.linked_editing.ignore_change
                && let Some(reference) = reference
                && inside(reference)
            {
                self.linked_editing.request_version = Some(change.version_id);
                self.schedule_linked_sync(cx);
            } else {
                self.schedule_linked_update(cx);
            }
        }
        if moved {
            self.schedule_linked_update(cx);
        }
    }

    fn schedule_linked_update(&mut self, cx: &mut Context<Self>) {
        if !self.linked_editing.enabled {
            return;
        }
        self.linked_editing.update = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let _ = workbench.update(cx, |workbench, cx| workbench.update_linked_ranges(false, cx));
        }));
    }

    fn schedule_linked_sync(&mut self, cx: &mut Context<Self>) {
        let token = self.linked_editing.sync_token;
        self.linked_editing.sync = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(Duration::ZERO).await;
            let _ = workbench.update(cx, |workbench, cx| workbench.sync_linked_ranges(token, cx));
        }));
    }

    /// `_syncRanges`: the reference range's text replaces each mirror's, trimmed to what differs, in the typing's undo
    /// group; a reference spanning lines or not one word ends linked editing.
    fn sync_linked_ranges(&mut self, token: u64, cx: &mut Context<Self>) {
        if token != self.linked_editing.sync_token || self.linked_editing.ranges.is_empty() {
            return;
        }
        let Some(edits) = self.linked_range_edits() else {
            self.clear_linked_ranges();
            return cx.notify();
        };
        if edits.is_empty() {
            return;
        }
        let editor = self.editors.active_mut().expect("checked above");
        let change = editor.execute_edits_in_last_undo_group(edits, EditReason::Unknown("linkedEditing"));
        self.linked_editing.ignore_change = true;
        if let Some(change) = change {
            self.editor_edited(Some(change), false, "linkedEditing", cx);
        }
        self.linked_editing.ignore_change = false;
    }

    /// The mirror edits of `_syncRanges`, `None` when the reference spans lines or is not one word.
    fn linked_range_edits(&self) -> Option<Vec<(Position, Position, String, bool)>> {
        let document = self.editors.active()?.document();
        let (reference_start, reference_end) = self.linked_editing.ranges[0];
        if reference_start.0 != reference_end.0 {
            return None;
        }
        let line = &document.lines[reference_start.0 - 1];
        let reference: String = utf16_slice(line, reference_start.1, reference_end.1);
        let language_pattern = self.languages.rules(&document.language_id).and_then(|rules| rules.word_pattern.as_ref());
        let pattern = self.linked_editing.word_pattern.as_ref().or(language_pattern).unwrap_or(&DEFAULT_WORD_REGEXP);
        let match_length = pattern.find(&reference).map_or(0, |(start, end)| end - start);
        if match_length != reference.len() {
            return None;
        }
        let mut edits = Vec::new();
        for &(start, end) in &self.linked_editing.ranges[1..] {
            let start_position = Position::new(start.0 - 1, char_column(&document.lines[start.0 - 1], start.1 - 1));
            let end_position = Position::new(end.0 - 1, char_column(&document.lines[end.0 - 1], end.1 - 1));
            if start.0 != end.0 {
                edits.push((start_position, end_position, reference.clone(), true));
                continue;
            }
            let old: Vec<char> = document.lines[start.0 - 1].chars().skip(start_position.column).take(end_position.column - start_position.column).collect();
            let new: Vec<char> = reference.chars().collect();
            let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
            let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
            let replace_start = Position::new(start_position.line, start_position.column + prefix);
            let replace_end = Position::new(end_position.line, end_position.column - suffix);
            let text: String = new[prefix..new.len() - suffix].iter().collect();
            if replace_start != replace_end || !text.is_empty() {
                edits.push((replace_start, replace_end, text, true));
            }
        }
        Some(edits)
    }

    /// `clearRanges`.
    pub(super) fn clear_linked_ranges(&mut self) {
        self.linked_editing.ranges.clear();
        self.linked_editing.request_id += 1;
        self.linked_editing.request_position = None;
    }

    /// `updateRanges`: asks the providers in order for the ranges at the cursor; the range holding it becomes the
    /// reference. Several cursors end linked editing.
    fn update_linked_ranges(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else {
            self.clear_linked_ranges();
            return;
        };
        let position = editor.selection().head;
        if (!self.linked_editing.enabled && !force) || editor.selections.len() > 1 {
            self.clear_linked_ranges();
            return cx.notify();
        }
        let version_id = editor.text_model().version_id;
        let utf16_position = (position.line + 1, utf16_column(&editor.document().lines[position.line], position.column) + 1);
        let contains = |range: &((usize, usize), (usize, usize))| range.0 <= utf16_position && utf16_position <= range.1;
        if self.linked_editing.request_position.is_some() && self.linked_editing.request_version == Some(version_id) {
            if self.linked_editing.request_position == Some(position) {
                return;
            }
            if self.linked_editing.ranges.first().is_some_and(contains) {
                return;
            }
        }
        if self.linked_editing.request_position != Some(position) && !self.linked_editing.ranges.first().is_some_and(contains) {
            self.clear_linked_ranges();
            cx.notify();
        }
        let editor = self.editors.active().expect("checked above");
        let handles = self.linked_editing_handles(editor);
        let uri = editor.document().uri();
        let model_id = editor.model_id;
        self.linked_editing.request_position = Some(position);
        self.linked_editing.request_version = Some(version_id);
        self.linked_editing.request_id += 1;
        let request_id = self.linked_editing.request_id;
        let caller = self.host.caller();
        let position_value = json!({ "lineNumber": utf16_position.0, "column": utf16_position.1 });
        self.linked_editing.update = Some(cx.spawn(async move |workbench, cx| {
            let result = first_linked_ranges(&caller, &handles, &uri, &position_value).await;
            let _ = workbench.update(cx, |workbench, cx| {
                if workbench.linked_editing.request_id != request_id {
                    return;
                }
                let current = workbench.editors.active().is_some_and(|editor| (editor.model_id, editor.text_model().version_id) == (model_id, version_id));
                if !current {
                    return;
                }
                workbench.linked_editing.word_pattern = result.as_ref().and_then(|result| regex_dto(&result["wordPattern"]));
                let mut ranges: Vec<((usize, usize), (usize, usize))> = result.iter().flat_map(|result| result["ranges"].as_array().into_iter().flatten()).map(range_tuple).collect();
                let Some(reference) = ranges.iter().position(|range| range.0 <= utf16_position && utf16_position <= range.1) else {
                    workbench.clear_linked_ranges();
                    return cx.notify();
                };
                let reference = ranges.remove(reference);
                ranges.insert(0, reference);
                workbench.linked_editing.ranges = ranges;
                workbench.linked_editing.sync_token += 1;
                cx.notify();
            });
        }));
    }

    /// `.linked-editing-decoration`: the ranges' background on line `index` as runs over the tab-expanded text.
    // shortcut: an empty range draws nothing where upstream keeps `min-width: 1px`, needs a decoration box per range.
    pub(super) fn linked_editing_styles(&self, editor: &OpenEditor, index: usize, tab_size: usize) -> Vec<(Range<usize>, HighlightStyle)> {
        if self.linked_editing.model_id != Some(editor.model_id) || self.linked_editing.ranges.is_empty() {
            return Vec::new();
        }
        let Some(background) = self.theme.color("editor.linkedEditingBackground").map(Hsla::from) else { return Vec::new() };
        let text = &editor.document().lines[index];
        let line = index + 1;
        let expanded = |column: usize| expand_tabs(&text.chars().take(column).collect::<String>(), tab_size).len();
        let mut styles: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
        for &(start, end) in &self.linked_editing.ranges {
            if start.0 > line || end.0 < line {
                continue;
            }
            let start = if start.0 == line { char_column(text, start.1 - 1) } else { 0 };
            let end = if end.0 == line { char_column(text, end.1 - 1) } else { text.chars().count() };
            if start < end {
                styles.push((expanded(start)..expanded(end), HighlightStyle { background_color: Some(background), ..HighlightStyle::default() }));
            }
        }
        styles.sort_by_key(|(range, _)| range.start);
        styles
    }

    /// `_executeLinkedEditingProvider`: the first provider's ranges with a non-empty list.
    fn execute_linked_editing_provider(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let uri = args.first().cloned().unwrap_or(Value::Null);
        let position = args.get(1).cloned().unwrap_or(Value::Null);
        let language = self.editors.editors.iter().find(|editor| editor.document().uri() == uri).map(|editor| editor.document().language_id.clone()).unwrap_or_default();
        let candidate = Candidate { uri: &uri, language: &language };
        let handles: Vec<u64> = self.language_features.ordered(REGISTER, &candidate).iter().map(|registration| registration.handle).collect();
        let caller = self.host.caller();
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn(async move |_, _| {
            let result = first_linked_ranges(&caller, &handles, &uri, &position).await;
            let _ = sender.send(Ok(result.unwrap_or(Value::Null)));
        })
        .detach();
        Outcome::Pending(receiver)
    }
}

/// `getLinkedEditingRanges`: the providers in order until one answers with ranges; a failing one is skipped.
async fn first_linked_ranges(caller: &impl Fn(&'static str, &'static str, Vec<Value>) -> futures::channel::oneshot::Receiver<Result<Value, Value>>, handles: &[u64], uri: &Value, position: &Value) -> Option<Value> {
    for &handle in handles {
        let Ok(Ok(result)) = caller("ExtHostLanguageFeatures", "$provideLinkedEditingRanges", vec![json!(handle), uri.clone(), position.clone()]).await else { continue };
        if result["ranges"].as_array().is_some_and(|ranges| !ranges.is_empty()) {
            return Some(result);
        }
    }
    None
}

fn range_tuple(range: &Value) -> ((usize, usize), (usize, usize)) {
    let number = |key: &str| range[key].as_u64().unwrap_or(1) as usize;
    ((number("startLineNumber"), number("startColumn")), (number("endLineNumber"), number("endColumn")))
}

/// mainThreadLanguageFeatures.ts `_reviveRegExp` of an `IRegExpDto`.
fn regex_dto(value: &Value) -> Option<onig::Regex> {
    let pattern = value["pattern"].as_str()?;
    crate::editor::folding::marker_regex(pattern, value["flags"].as_str().unwrap_or("")).ok()
}

/// The text between two 1-based UTF-16 columns of `line`.
fn utf16_slice(line: &str, start: usize, end: usize) -> String {
    line.chars().skip(char_column(line, start - 1)).take(char_column(line, end - 1) - char_column(line, start - 1)).collect()
}

#[cfg(test)]
mod tests;
