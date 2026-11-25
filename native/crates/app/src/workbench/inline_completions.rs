//! inlineCompletionsController.ts, inlineCompletionsModel.ts, inlineCompletionsSource.ts, provideInlineCompletions.ts,
//! ghostTextView.ts and controller/commands.ts for the providers extensions register: the completions at the cursor
//! are asked after typing (or explicitly), the selected one shows as ghost text, Tab accepts it.
// The AI parts (inline edits, next edit suggestions, the chat agent's completions) are removed by decision; a list
// item with `isInlineEdit` is dropped.

use std::time::Duration;

use gpui::{AnyElement, Context, Task, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::editor_view::EDITOR_FONT;
use crate::commands::Outcome;
use crate::editor::injected_text::{InjectedText, InjectionOwner};
use crate::editor::{EditReason, OpenEditor, Position, char_column, utf16_column};
use crate::ghost_text::{GhostText, GhostTextMode, Replacement, compute_ghost_text, inline_completion_is_visible};
use crate::language_features::Candidate;

const REGISTER: &str = "$registerInlineCompletionsSupport";
/// `InlineCompletionsDebounce` of the feature debounce service (`min: 50, max: 50`).
const DEBOUNCE: Duration = Duration::from_millis(50);
/// `InlineCompletionEndOfLifeReasonKind`.
const ACCEPTED: u64 = 0;
const REJECTED: u64 = 1;
const IGNORED: u64 = 2;

#[derive(Default)]
pub(super) struct InlineCompletions {
    model_id: Option<u32>,
    /// `_isActive`: a trigger started the session; a stop ends it.
    active: bool,
    items: Vec<Item>,
    /// The (handle, pid) of the lists the items came from, freed when replaced (`$freeInlineCompletionsList`).
    lists: Vec<(u64, u64)>,
    /// `_selectedInlineCompletionId`: Alt+] and Alt+[ move it.
    selected: usize,
    /// The item whose show the provider heard (`handleItemDidShow`).
    shown: Option<(u64, u64, u64)>,
    request: Option<Task<()>>,
    /// The view zone of the ghost text's additional lines and the lines it shows.
    zone: Option<(u32, Vec<String>)>,
    /// The ghost text drawn now, by injection id.
    ghost: Option<GhostText>,
}

#[derive(Clone, Debug)]
struct Item {
    handle: u64,
    pid: u64,
    idx: u64,
    edit: Replacement,
    command: Value,
    /// `insertText.snippet`: the template the snippet controller inserts; `edit.text` is its plain text.
    snippet: Option<String>,
}

impl Workbench {
    pub(super) fn register_inline_completions(&mut self) {
        self.commands.register("editor.action.inlineSuggest.trigger", |workbench, _, _, cx| {
            workbench.trigger_inline_completions(true, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.inlineSuggest.commit", |workbench, _, window, cx| {
            workbench.accept_inline_completion(window, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.inlineSuggest.hide", |workbench, _, _, cx| {
            workbench.stop_inline_completions(true, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.inlineSuggest.showNext", |workbench, _, _, cx| {
            workbench.cycle_inline_completions(1, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.inlineSuggest.showPrevious", |workbench, _, _, cx| {
            workbench.cycle_inline_completions(-1, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.inlineSuggest.acceptNextWord", |workbench, _, window, cx| {
            workbench.accept_inline_completion_part(PartialKind::Word, window, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.inlineSuggest.acceptNextLine", |workbench, _, window, cx| {
            workbench.accept_inline_completion_part(PartialKind::Line, window, cx);
            Outcome::done()
        });
    }

    /// `inlineSuggestionVisible`, `inlineSuggestionHasIndentation`, `inlineSuggestionHasIndentationLessThanTabSize`,
    /// `editor.hasSelection` and `cursorBeforeGhostText`.
    pub(super) fn inline_completions_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        let ghost = self.inline_completions.ghost.as_ref().filter(|ghost| !ghost.is_empty());
        context.insert("inlineSuggestionVisible".into(), json!(ghost.is_some()));
        let Some(editor) = self.editors.active() else { return };
        context.insert("editor.hasSelection".into(), json!(editor.selections.iter().any(|selection| !selection.is_empty())));
        // `cursorBeforeGhostText`: the cursor sits where the first ghost text part starts.
        let head = editor.selection().head;
        context.insert("cursorBeforeGhostText".into(), json!(ghost.is_some_and(|ghost| ghost.line == head.line && ghost.parts[0].column == head.column)));
        // `getIndentationInfo`: the ghost text starts in the line's indentation, and how its indentation compares
        // to the tab size.
        let (starts_with_indentation, less_than_tab_size) = ghost.map_or((false, true), |ghost| {
            let line = &editor.document().lines[ghost.line];
            let first = line.chars().position(|c| c != ' ' && c != '\t').unwrap_or(line.chars().count());
            let part = &ghost.parts[0];
            let in_indentation = part.column <= first;
            let text: String = line.chars().take(part.column).collect::<String>() + &part.text;
            let indentation = text.chars().take_while(|c| *c == ' ' || *c == '\t').fold(0, |visible, c| if c == '\t' { visible + editor.document().indentation.tab_size - visible % editor.document().indentation.tab_size } else { visible + 1 });
            (in_indentation, !in_indentation || indentation < editor.document().indentation.tab_size || first < part.column)
        });
        context.insert("inlineSuggestionHasIndentation".into(), json!(starts_with_indentation));
        context.insert("inlineSuggestionHasIndentationLessThanTabSize".into(), json!(less_than_tab_size));
    }

    fn inline_completion_handles(&self, editor: &OpenEditor) -> Vec<(u64, Vec<Value>)> {
        let uri = editor.document().uri();
        let language = editor.document().language_id.clone();
        let candidate = Candidate { uri: &uri, language: &language };
        self.language_features.ordered(REGISTER, &candidate).iter().map(|registration| (registration.handle, registration.args.clone())).collect()
    }

    /// `onDidType` and the commands that do not type (Tab, Delete Left/Right, accept): a session starts or asks again.
    pub(super) fn inline_completions_typed(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        if self.suggest_setting(editor, "editor.inlineSuggest.enabled") == json!(false) {
            return;
        }
        self.trigger_inline_completions(false, cx);
    }

    /// `onDidChangeCursorPosition` by the user or the API: a shown ghost text stops; any edit asks again while
    /// a session runs.
    pub(super) fn inline_completions_cursor_changed(&mut self, edited: bool, cx: &mut Context<Self>) {
        if !self.inline_completions.active {
            return;
        }
        if edited {
            self.trigger_inline_completions(false, cx);
            return;
        }
        if self.inline_completions.ghost.is_some() {
            self.stop_inline_completions(false, cx);
        } else {
            self.update_ghost_text(cx);
        }
    }

    /// Another model: the session stops.
    pub(super) fn inline_completions_model_changed(&mut self, cx: &mut Context<Self>) {
        let model_id = self.editors.active().map(|editor| editor.model_id);
        if model_id != self.inline_completions.model_id {
            self.stop_inline_completions(false, cx);
            self.inline_completions.model_id = model_id;
        }
    }

    /// `trigger` and `_fetchInlineCompletionsPromise`: after the debounce (none for an explicit request) every
    /// provider is asked at the cursor.
    fn trigger_inline_completions(&mut self, explicit: bool, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let providers = self.inline_completion_handles(editor);
        self.inline_completions.active = true;
        self.inline_completions.model_id = Some(editor.model_id);
        if providers.is_empty() {
            return;
        }
        let model_id = editor.model_id;
        let version = editor.text_model().version_id;
        // `debounceDelayMs` of the providers, the feature's otherwise.
        let delay = match explicit {
            true => Duration::ZERO,
            false => providers.iter().filter_map(|(_, args)| args.get(8).and_then(Value::as_u64)).max().map_or(DEBOUNCE, Duration::from_millis),
        };
        self.inline_completions.request = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(delay).await;
            let Ok(calls) = workbench.update(cx, |workbench, _| workbench.request_inline_completions(model_id, explicit, &providers)) else { return };
            let mut lists = Vec::new();
            for (handle, call) in calls {
                if let Ok(Ok(list)) = call.await
                    && !list.is_null()
                {
                    lists.push((handle, list));
                }
            }
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.inline_completions.request = None;
                let current = workbench.editors.active().is_some_and(|editor| editor.model_id == model_id && editor.text_model().version_id == version);
                if !current || !workbench.inline_completions.active {
                    for (handle, list) in &lists {
                        workbench.free_inline_completion_list(*handle, list["pid"].as_u64().unwrap_or_default(), "lostRace");
                    }
                    return;
                }
                workbench.set_inline_completion_lists(lists, cx);
            });
        }));
    }

    fn request_inline_completions(&self, model_id: u32, explicit: bool, providers: &[(u64, Vec<Value>)]) -> Vec<(u64, futures::channel::oneshot::Receiver<Result<Value, Value>>)> {
        let Some(editor) = self.editors.active().filter(|editor| editor.model_id == model_id) else { return Vec::new() };
        let uri = editor.document().uri();
        let head = editor.selection().head;
        let position = json!({ "lineNumber": head.line + 1, "column": utf16_column(&editor.document().lines[head.line], head.column) + 1 });
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |time| time.as_millis() as u64);
        let min_show_delay = self.suggest_setting(editor, "editor.inlineSuggest.minShowDelay").as_u64().unwrap_or(0);
        let context = json!({
            "triggerKind": if explicit { 1 } else { 0 },
            "includeInlineCompletions": true,
            "includeInlineEdits": false,
            "requestIssuedDateTime": now,
            "earliestShownDateTime": now + if explicit { 0 } else { min_show_delay },
            "requestUuid": format!("icr-{now}"),
        });
        providers.iter().map(|(handle, _)| (*handle, self.host.call_cancellable("ExtHostLanguageFeatures", "$provideInlineCompletions", vec![json!(handle), uri.clone(), position.clone(), context.clone()]))).collect()
    }

    /// `InlineCompletionsSource` keeps the items of the new lists; the old lists are freed.
    fn set_inline_completion_lists(&mut self, lists: Vec<(u64, Value)>, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let lines = editor.document().lines.clone();
        let head = editor.selection().head;
        let mut items = Vec::new();
        let mut list_ids = Vec::new();
        for (handle, list) in &lists {
            let pid = list["pid"].as_u64().unwrap_or_default();
            list_ids.push((*handle, pid));
            for completion in list["items"].as_array().into_iter().flatten() {
                if completion["isInlineEdit"] == json!(true) {
                    continue;
                }
                if let Some(item) = completion_item(&lines, head, editor.document().indentation, *handle, pid, completion) {
                    items.push(item);
                }
            }
        }
        let previous = self.selected_item().map(|item| (item.handle, item.edit.text.clone()));
        let old = std::mem::replace(&mut self.inline_completions.lists, list_ids);
        self.inline_completions.items = items;
        // `itemToPreserve`: the selected completion stays selected while the new lists still hold it.
        self.inline_completions.selected = previous.and_then(|(handle, text)| self.visible_items().iter().position(|item| item.handle == handle && item.edit.text == text)).unwrap_or(0);
        for (handle, pid) in old {
            self.free_inline_completion_list(handle, pid, "tokenCancellation");
        }
        self.update_ghost_text(cx);
    }

    fn free_inline_completion_list(&self, handle: u64, pid: u64, reason: &str) {
        self.host.send("ExtHostLanguageFeatures", "$freeInlineCompletionsList", vec![json!(handle), json!(pid), json!({ "kind": reason })]);
    }

    /// The items whose edit shows at the cursor (`inlineCompletionIsVisible`), without duplicates.
    fn visible_items(&self) -> Vec<&Item> {
        let Some(editor) = self.editors.active() else { return Vec::new() };
        let lines = &editor.document().lines;
        let head = editor.selection().head;
        let mut visible: Vec<&Item> = Vec::new();
        for item in &self.inline_completions.items {
            if inline_completion_is_visible(&item.edit, None, lines, head) && !visible.iter().any(|other| other.edit == item.edit) {
                visible.push(item);
            }
        }
        visible
    }

    fn selected_item(&self) -> Option<Item> {
        let visible = self.visible_items();
        visible.get(self.inline_completions.selected.min(visible.len().saturating_sub(1))).map(|item| (*item).clone())
    }

    /// `state`: the selected item's ghost text in `editor.inlineSuggest` mode (`subwordSmart`), drawn as injected
    /// text on its line and a view zone for its other lines.
    fn update_ghost_text(&mut self, cx: &mut Context<Self>) {
        let ghost = self.current_ghost_text();
        if ghost == self.inline_completions.ghost {
            return;
        }
        self.inline_completions.ghost = ghost.clone();
        let line_height = self.editors.viewport.line_height;
        let Some(editor) = self.editors.active_mut() else { return };
        if let Some((zone, _)) = self.inline_completions.zone.take() {
            editor.remove_view_zone(zone);
        }
        let mut injections = Vec::new();
        if let Some(ghost) = &ghost {
            let mut extra_lines: Vec<String> = Vec::new();
            for (id, part) in ghost.parts.iter().enumerate() {
                let mut part_lines = part.text.split('\n');
                let first = part_lines.next().unwrap_or_default();
                injections.push(InjectedText { position: Position::new(ghost.line, part.column), columns: first.chars().count() as f32, owner: InjectionOwner::GhostText, id, at_range_start: false });
                extra_lines.extend(part_lines.map(String::from));
            }
            // `additionalLinesOriginalSuffix`: the rest of the line after a multi-line ghost text moves to its last line.
            if !extra_lines.is_empty() {
                let line_end = editor.document().lines[ghost.line].chars().count();
                let zone = editor.add_view_zone(Some(Position::new(ghost.line, line_end)), extra_lines.len() as f32 * line_height);
                self.inline_completions.zone = Some((zone, extra_lines));
            }
        }
        editor.set_injected_text(InjectionOwner::GhostText, injections);
        if let Some(item) = self.selected_item().filter(|_| ghost.is_some()) {
            let key = (item.handle, item.pid, item.idx);
            if self.inline_completions.shown != Some(key) {
                self.inline_completions.shown = Some(key);
                // `handleItemDidShow` with the text the ghost text shows.
                self.host.send("ExtHostLanguageFeatures", "$handleInlineCompletionDidShow", vec![json!(item.handle), json!(item.pid), json!(item.idx), json!(item.edit.text)]);
            }
        }
        cx.notify();
    }

    fn current_ghost_text(&self) -> Option<GhostText> {
        if !self.inline_completions.active {
            return None;
        }
        let editor = self.editors.active()?;
        if self.suggest_setting(editor, "editor.inlineSuggest.suppressInSnippetMode") != json!(false) && editor.snippet_session.is_some() {
            return None;
        }
        let item = self.selected_item()?;
        let mode = match self.suggest_setting(editor, "editor.inlineSuggest.mode").as_str() {
            Some("prefix") => GhostTextMode::Prefix,
            Some("subword") => GhostTextMode::Subword,
            _ => GhostTextMode::SubwordSmart,
        };
        compute_ghost_text(&item.edit, &editor.document().lines, mode, Some(editor.selection().head), 0).filter(|ghost| !ghost.parts.is_empty())
    }

    /// `stop`: the ghost text goes; an explicit cancel tells the provider the item was rejected.
    fn stop_inline_completions(&mut self, explicit: bool, cx: &mut Context<Self>) {
        if explicit && let Some(item) = self.selected_item().filter(|_| self.inline_completions.ghost.is_some()) {
            self.end_of_lifetime(&item, REJECTED);
        } else if let Some(item) = self.selected_item().filter(|_| self.inline_completions.ghost.is_some()) {
            self.end_of_lifetime(&item, IGNORED);
        }
        self.inline_completions.active = false;
        self.inline_completions.request = None;
        self.inline_completions.items.clear();
        self.inline_completions.shown = None;
        for (handle, pid) in std::mem::take(&mut self.inline_completions.lists) {
            self.free_inline_completion_list(handle, pid, "empty");
        }
        self.update_ghost_text(cx);
    }

    fn end_of_lifetime(&self, item: &Item, kind: u64) {
        self.host.send("ExtHostLanguageFeatures", "$handleInlineCompletionEndOfLifetime", vec![json!(item.handle), json!(item.pid), json!(item.idx), json!({ "kind": kind })]);
    }

    /// `next`/`previous`: the selection wraps around the visible items; an explicit request asks first when the
    /// session has only the automatic ones.
    fn cycle_inline_completions(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.visible_items().len();
        if count == 0 {
            return;
        }
        self.inline_completions.selected = (self.inline_completions.selected as isize + delta).rem_euclid(count as isize) as usize;
        self.update_ghost_text(cx);
    }

    /// `accept`: the item's edit (through the snippet controller for a snippet) and its command; then a new request
    /// without delay.
    fn accept_inline_completion(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let Some(item) = self.selected_item().filter(|_| self.inline_completions.ghost.is_some()) else { return };
        self.end_of_lifetime(&item, ACCEPTED);
        self.inline_completions.shown = None;
        self.inline_completions.items.clear();
        self.update_ghost_text(cx);
        match &item.snippet {
            Some(template) => {
                let snippets = [(item.edit.start, item.edit.end, template.as_str(), false)];
                self.insert_snippets(super::snippet_controller::SnippetInsert { snippets: &snippets, clipboard: None, undo_stop_before: true, undo_stop_after: true, reason: "inlineSuggestion.accept" }, cx);
            }
            None => {
                let Some(editor) = self.editors.active_mut() else { return };
                editor.push_undo_stop();
                let end = crate::editor::end_of(item.edit.start, &item.edit.text);
                let change = editor.execute_edits(vec![(item.edit.start, item.edit.end, item.edit.text.clone(), false)], Some(vec![crate::editor::Selection::caret(end)]), EditReason::Unknown("inlineSuggestion.accept"));
                editor.push_undo_stop();
                self.editor_edited(change, true, "inlineSuggestion.accept", cx);
            }
        }
        if let Some(id) = item.command["id"].as_str().filter(|id| !id.is_empty()) {
            let args = item.command["arguments"].as_array().cloned().unwrap_or_default();
            self.execute_command(id, args, window, cx);
        }
        self.trigger_inline_completions(false, cx);
    }

    /// `acceptNextWord`/`acceptNextLine`: the ghost text's first word (or line) is inserted and the session goes on.
    fn accept_inline_completion_part(&mut self, kind: PartialKind, _window: &mut gpui::Window, cx: &mut Context<Self>) {
        let (Some(item), Some(ghost)) = (self.selected_item(), self.inline_completions.ghost.clone()) else { return };
        let Some(part) = ghost.parts.first() else { return };
        let text = match kind {
            PartialKind::Word => next_word(&part.text),
            PartialKind::Line => part.text.split_inclusive('\n').next().unwrap_or_default().trim_end_matches('\n').to_owned(),
        };
        let text = if text.is_empty() { part.text.chars().take(1).collect() } else { text };
        let position = Position::new(ghost.line, part.column);
        let Some(editor) = self.editors.active_mut() else { return };
        let end = crate::editor::end_of(position, &text);
        let change = editor.execute_edits(vec![(position, position, text.clone(), false)], Some(vec![crate::editor::Selection::caret(end)]), EditReason::Unknown("inlineSuggestion.accept"));
        self.host.send("ExtHostLanguageFeatures", "$handleInlineCompletionPartialAccept", vec![json!(item.handle), json!(item.pid), json!(item.idx), json!(text.encode_utf16().count()), json!({ "kind": if kind == PartialKind::Word { 0 } else { 1 }, "acceptedLength": text.encode_utf16().count() })]);
        self.editor_edited(change, true, "inlineSuggestion.accept", cx);
    }

    /// ghostTextView.ts: a ghost text part drawn in the gap of its injected text, italic in `editorGhostText.foreground`.
    pub(super) fn ghost_text_element(&self, id: usize, left: f32, line_height: f32) -> Option<AnyElement> {
        let ghost = self.inline_completions.ghost.as_ref()?;
        let part = ghost.parts.get(id)?;
        let text: String = part.text.split('\n').next().unwrap_or_default().to_owned();
        Some(self.ghost_text_row(text).absolute().left(px(left)).top_0().h(px(line_height)).into_any_element())
    }

    fn ghost_text_row(&self, text: String) -> gpui::Div {
        let mut row = div().flex().items_center().whitespace_nowrap().italic().font_family(EDITOR_FONT).text_color(self.foreground_or("editorGhostText.foreground"));
        if let Some(background) = self.theme.color("editorGhostText.background") {
            row = row.bg(background);
        }
        if let Some(border) = self.theme.color("editorGhostText.border") {
            row = row.border_1().border_color(border);
        }
        row.child(text)
    }

    /// `AdditionalLinesWidget`: the ghost text's other lines in the view zone below its line.
    pub(super) fn ghost_text_lines(&self, editor: &OpenEditor, content_left: f32) -> Option<AnyElement> {
        let (zone, lines) = self.inline_completions.zone.as_ref()?;
        let top = editor.view_zone_top(*zone, self.editors.viewport)?;
        let line_height = self.editors.viewport.line_height;
        let mut column = div().absolute().left(px(content_left - editor.scroll.left)).top(px(top - editor.scroll.top)).flex().flex_col();
        for line in lines {
            column = column.child(self.ghost_text_row(crate::editor::expand_tabs(line, editor.document().indentation.tab_size)).h(px(line_height)));
        }
        Some(column.into_any_element())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum PartialKind {
    Word,
    Line,
}

/// `acceptNextWord`'s word: leading whitespace, then a word or one other char.
fn next_word(text: &str) -> String {
    let whitespace = text.len() - text.trim_start_matches([' ', '\t']).len();
    let rest = &text[whitespace..];
    let word = rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len());
    let word = if word == 0 { rest.chars().next().map_or(0, char::len_utf8) } else { word };
    text[..whitespace + word].to_owned()
}

/// One `InlineCompletion` DTO: its range (or the word before the cursor, `getDefaultRange`) and insert text.
fn completion_item(lines: &[String], head: Position, indentation: crate::editor::Indentation, handle: u64, pid: u64, completion: &Value) -> Option<Item> {
    let (text, snippet) = match &completion["insertText"] {
        Value::String(text) => (text.clone(), None),
        // `snippet.toString()`: the placeholders' text, unresolved variables empty.
        Value::Object(object) => {
            let template = object.get("snippet")?.as_str()?.to_owned();
            (crate::editor::expand_snippet(&template, "", false, indentation, |_| None).text, Some(template))
        }
        _ => return None,
    };
    let range = &completion["range"];
    let (start, end) = match range.is_object() {
        true => {
            let position = |line: &Value, column: &Value| {
                let line = (line.as_u64().unwrap_or(1).max(1) as usize - 1).min(lines.len() - 1);
                Position::new(line, char_column(&lines[line], column.as_u64().unwrap_or(1).max(1) as usize - 1))
            };
            (position(&range["startLineNumber"], &range["startColumn"]), position(&range["endLineNumber"], &range["endColumn"]))
        }
        // `getDefaultRange`: the word part before the cursor.
        false => {
            let start = crate::editor::word_at(&lines[head.line], head.column).filter(|word| word.start < head.column).map_or(head.column, |word| word.start);
            (Position::new(head.line, start), head)
        }
    };
    Some(Item { handle, pid, idx: completion["idx"].as_u64().unwrap_or_default(), edit: Replacement { start, end, text }, command: completion["command"].clone(), snippet })
}

#[cfg(test)]
mod tests;
