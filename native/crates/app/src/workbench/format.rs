//! format.ts, formatActions.ts, formattingEdit.ts, formatActionsMultiple.ts, formatActionsNone.ts, formatModified.ts,
//! the minimal edits of editorWebWorker.ts and `FormatOnSaveParticipant` of saveParticipants.ts: the providers'
//! formatting edits for a document, its selections, its modified lines and the typed character.

use std::path::PathBuf;

use futures::channel::oneshot;
use gpui::{AnyWindowHandle, AsyncApp, Context, PromptLevel, Task, WeakEntity, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::auto_save::SaveReason;
use super::host_requests::range as model_range;
use super::text_files::SaveOptions;
use super::text_input::SavePhase;
use crate::commands::Outcome;
use crate::dialog;
use crate::editor::{ContentChanged, EditReason, OpenEditor, Position, utf16_column};
use crate::language_features::Candidate;
use crate::notifications::{Action, Notification, Priority, Run, Severity};
use crate::quick_input::{Entry, Item, Kind, QuickInput};

const DOCUMENT: &str = "$registerDocumentFormattingSupport";
const RANGE: &str = "$registerRangeFormattingSupport";
const ON_TYPE: &str = "$registerOnTypeFormattingSupport";
/// `EditorWorker._diffLimit`: longer texts are not diffed.
const DIFF_LIMIT: usize = 100000;

/// A `DocumentFormattingEditProvider` or `DocumentRangeFormattingEditProvider`; a range formatter asked for the
/// whole document is `whole_document` (`getRealAndSyntheticDocumentFormattersOrdered`).
#[derive(Clone, Debug)]
pub(super) struct Formatter {
    handle: u64,
    extension_id: String,
    display_name: String,
    supports_ranges: bool,
    whole_document: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Mode {
    Explicit,
    Silent,
}

#[derive(Clone, Copy, PartialEq)]
enum FormattingKind {
    File,
    Selection,
}

/// A model, by its resource.
#[derive(Clone)]
pub(super) struct ModelKey {
    scheme: String,
    path: PathBuf,
}

#[derive(Default)]
pub(super) struct Formatting {
    on_type: Option<OnType>,
}

/// A running format-on-type request; dropping it cancels the host call.
struct OnType {
    _task: Task<()>,
    model_id: u32,
    /// The 1-based line of the typed character.
    line: usize,
    /// The model version the request may apply to: the one it asked at, moved past edits below its line.
    version_id: u32,
}

impl Workbench {
    pub(super) fn register_format(&mut self) {
        self.commands.register("editor.action.formatDocument", |workbench, _, window, cx| workbench.format_document_command(window, cx));
        self.commands.register("editor.action.formatSelection", |workbench, _, window, cx| workbench.format_selection_command(window, cx));
        self.commands.register("editor.action.format", |workbench, _, window, cx| match workbench.editors.active().is_some_and(|editor| editor.selection().is_empty()) {
            true => workbench.format_document_command(window, cx),
            false => workbench.format_selection_command(window, cx),
        });
        self.commands.register("editor.action.formatDocument.multiple", |workbench, args, window, cx| workbench.format_document_with(args.first().cloned(), window, cx));
        self.commands.register("editor.action.formatSelection.multiple", |workbench, _, window, cx| workbench.format_selection_with(window, cx));
        self.commands.register("editor.action.formatDocument.none", |workbench, _, window, cx| workbench.format_document_none(window, cx));
        self.commands.register("editor.action.formatChanges", |workbench, _, window, cx| workbench.format_changes_command(window, cx));
        self.commands.register("_executeFormatDocumentProvider", |workbench, args, _, cx| workbench.execute_format_provider(&args, None, cx));
        self.commands.register("_executeFormatRangeProvider", |workbench, args, _, cx| {
            let range = args.get(1).cloned().unwrap_or_default();
            workbench.execute_format_provider(&args, Some(range), cx)
        });
        self.commands.register("_executeFormatOnTypeProvider", |workbench, args, _, cx| workbench.execute_format_on_type_provider(&args, cx));
    }

    /// `editorHasDocumentFormattingProvider` and its relatives (editorContextKeys.ts, `EditorContextKeysManager`).
    pub(super) fn format_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        let Some(editor) = self.editors.active() else { return };
        let document = self.document_formatters(editor).len();
        let range = self.range_formatters(editor).len();
        context.insert("editorHasDocumentFormattingProvider".into(), json!(document > 0));
        context.insert("editorHasDocumentSelectionFormattingProvider".into(), json!(range > 0));
        context.insert("editorHasMultipleDocumentFormattingProvider".into(), json!(document > 1));
        context.insert("editorHasMultipleDocumentSelectionFormattingProvider".into(), json!(range > 1));
    }

    /// `DefaultFormatter._updateConfigValues` on a formatter registration: the `editor.defaultFormatter` enum lists
    /// the extensions with a formatter first.
    pub(crate) fn formatters_changed(&mut self) {
        let mut ids: Vec<String> = Vec::new();
        for method in [DOCUMENT, RANGE] {
            ids.extend(self.language_features.all(method).map(|registration| extension_id(&registration.args[2])));
        }
        let extensions: Vec<(&str, &Value)> = self.extensions.iter().map(|extension| (extension.id.as_str(), &extension.description)).collect();
        self.settings.lock().unwrap().registry.list_default_formatters(&extensions, &ids);
    }

    fn formatter(&self, method: &str, handle: u64) -> Option<Formatter> {
        let args = &self.language_features.registration(handle)?.args;
        Some(Formatter {
            handle,
            extension_id: extension_id(&args[2]),
            display_name: args.get(3).and_then(Value::as_str).unwrap_or_default().to_owned(),
            supports_ranges: method == RANGE && args.get(4) == Some(&json!(true)),
            whole_document: false,
        })
    }

    fn ordered_formatters(&self, method: &str, editor: &OpenEditor) -> Vec<Formatter> {
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        let handles: Vec<u64> = self.language_features.ordered(method, &candidate).iter().map(|registration| registration.handle).collect();
        handles.into_iter().filter_map(|handle| self.formatter(method, handle)).collect()
    }

    fn range_formatters(&self, editor: &OpenEditor) -> Vec<Formatter> {
        self.ordered_formatters(RANGE, editor)
    }

    /// `getRealAndSyntheticDocumentFormattersOrdered`: the document formatters, then the range formatters of
    /// extensions without one, asked for the whole document.
    fn document_formatters(&self, editor: &OpenEditor) -> Vec<Formatter> {
        let mut formatters = self.ordered_formatters(DOCUMENT, editor);
        for formatter in self.range_formatters(editor) {
            if !formatter.extension_id.is_empty() && formatters.iter().any(|known| known.extension_id.eq_ignore_ascii_case(&formatter.extension_id)) {
                continue;
            }
            formatters.push(Formatter { whole_document: true, ..formatter });
        }
        formatters
    }

    fn active_model_key(&self) -> Option<ModelKey> {
        let editor = self.editors.active()?;
        Some(ModelKey { scheme: editor.document.scheme.clone(), path: editor.document.path.clone() })
    }

    /// `FormatDocumentAction.run`: the selected formatter for the document.
    fn format_document_command(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        if self.active_editor_readonly().is_some() {
            return Outcome::done();
        }
        let Some(key) = self.active_model_key() else { return Outcome::done() };
        let format = self.format_document_with_selected_provider(key, Mode::Explicit, window.window_handle(), cx);
        pending(cx, format)
    }

    /// `FormatSelectionAction.run`: each selection, an empty one as its whole line.
    fn format_selection_command(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        if self.active_editor_readonly().is_some() {
            return Outcome::done();
        }
        let Some(editor) = self.editors.active() else { return Outcome::done() };
        let ranges = editor.selections.iter().map(|selection| match selection.is_empty() {
            true => (Position::new(selection.head.line, 0), Position::new(selection.head.line, editor.document.lines[selection.head.line].chars().count())),
            false => (selection.start(), selection.end()),
        });
        let ranges = ranges.collect();
        let Some(key) = self.active_model_key() else { return Outcome::done() };
        let format = self.format_ranges_with_selected_provider(key, ranges, Mode::Explicit, window.window_handle(), cx);
        pending(cx, format)
    }

    /// `FormatModifiedAction.run`: the lines the quick diff marks as changed.
    fn format_changes_command(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        if self.active_editor_readonly().is_some() {
            return Outcome::done();
        }
        let Some(key) = self.active_model_key() else { return Outcome::done() };
        let Some(Some(ranges)) = self.modified_ranges(&key) else { return Outcome::done() };
        if ranges.is_empty() {
            return Outcome::done();
        }
        let format = self.format_ranges_with_selected_provider(key, ranges, Mode::Explicit, window.window_handle(), cx);
        pending(cx, format)
    }

    /// `getModifiedRanges`: `None` without a source control original, else the changed lines (deleted lines as the
    /// line before them).
    fn modified_ranges(&self, key: &ModelKey) -> Option<Option<Vec<(Position, Position)>>> {
        let editor = self.editors.model(&key.scheme, &key.path)?;
        let changes = self.quick_diff_line_changes(editor)?;
        let lines = &editor.document.lines;
        let ranges = changes
            .iter()
            .map(|change| {
                let end = if change.end == 0 { change.start } else { change.end };
                let start_line = change.start.clamp(1, lines.len()) - 1;
                let end_line = end.clamp(1, lines.len()) - 1;
                (Position::new(start_line, 0), Position::new(end_line, lines[end_line].chars().count()))
            })
            .collect();
        Some(Some(ranges))
    }

    /// `FormatDocumentMultipleAction.run` (Format Document With...): the formatter named by `args.formatter`, else the
    /// one picked.
    fn format_document_with(&mut self, args: Option<Value>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        if self.active_editor_readonly().is_some() {
            return Outcome::done();
        }
        let Some(editor) = self.editors.active() else { return Outcome::done() };
        let formatters = self.document_formatters(editor);
        let key = self.active_model_key().expect("an active editor");
        if let Some(formatter) = args.as_ref().and_then(|args| args["formatter"].as_str()) {
            let Some(formatter) = formatters.into_iter().find(|known| known.extension_id.eq_ignore_ascii_case(formatter)) else { return Outcome::done() };
            let format = self.format_document_with_provider(formatter, key, Mode::Explicit, cx);
            return pending(cx, format);
        }
        let pick = self.show_formatter_pick(&key, formatters.clone(), window, cx);
        let task = cx.spawn(async move |workbench, cx| {
            let Some(index) = pick.await else { return };
            let Ok(format) = workbench.update(cx, |workbench, cx| workbench.format_document_with_provider(formatters[index].clone(), key, Mode::Explicit, cx)) else { return };
            format.await;
        });
        pending(cx, task)
    }

    /// `FormatSelectionMultipleAction.run`: the primary selection, or its line, with the picked range formatter.
    fn format_selection_with(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        if self.active_editor_readonly().is_some() {
            return Outcome::done();
        }
        let Some(editor) = self.editors.active() else { return Outcome::done() };
        let selection = editor.selection();
        let range = match selection.is_empty() {
            true => (Position::new(selection.head.line, 0), Position::new(selection.head.line, editor.document.lines[selection.head.line].chars().count())),
            false => (selection.start(), selection.end()),
        };
        let formatters = self.range_formatters(editor);
        let key = self.active_model_key().expect("an active editor");
        let pick = self.show_formatter_pick(&key, formatters.clone(), window, cx);
        let task = cx.spawn(async move |workbench, cx| {
            let Some(index) = pick.await else { return };
            let Ok(format) = workbench.update(cx, |workbench, cx| workbench.format_ranges_with_provider(formatters[index].clone(), key, vec![range], cx)) else { return };
            format.await;
        });
        pending(cx, task)
    }

    /// formatActionsNone.ts: with no document formatter, a dialog offers to look for one in the Marketplace.
    fn format_document_none(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        if self.active_editor_readonly().is_some() {
            return Outcome::done();
        }
        let Some(editor) = self.editors.active() else { return Outcome::done() };
        let count = self.ordered_formatters(DOCUMENT, editor).len();
        if count > 1 {
            return self.execute_command("editor.action.formatDocument.multiple", vec![], window, cx);
        }
        if count == 1 {
            return self.execute_command("editor.action.formatDocument", vec![], window, cx);
        }
        let language = editor.document.language_id.clone();
        let message = format!("There is no formatter for '{language}' files installed.");
        let answer = dialog::show_message_box(window, PromptLevel::Info, &message, None, &["Install Formatter...", "Cancel"], None, cx);
        let workbench = cx.entity().downgrade();
        let task = window.spawn(cx, async move |cx| {
            if answer.await.is_ok_and(|(button, _)| button == 0) {
                let _ = workbench.update_in(cx, |workbench, window, cx| workbench.search_extensions(&format!("category:formatters {language}"), window, cx));
            }
        });
        pending(cx, task)
    }

    /// `formatDocumentWithSelectedProvider`.
    pub(super) fn format_document_with_selected_provider(&mut self, key: ModelKey, mode: Mode, window: AnyWindowHandle, cx: &mut Context<Self>) -> Task<()> {
        let Some(editor) = self.editors.model(&key.scheme, &key.path) else { return Task::ready(()) };
        let formatters = self.document_formatters(editor);
        let select = self.select_formatter(formatters, &key, mode, FormattingKind::File, window, cx);
        cx.spawn(async move |workbench, cx| {
            let Some(formatter) = select.await else { return };
            let Ok(format) = workbench.update(cx, |workbench, cx| workbench.format_document_with_provider(formatter, key, mode, cx)) else { return };
            format.await;
        })
    }

    /// `formatDocumentRangesWithSelectedProvider`.
    pub(super) fn format_ranges_with_selected_provider(&mut self, key: ModelKey, ranges: Vec<(Position, Position)>, mode: Mode, window: AnyWindowHandle, cx: &mut Context<Self>) -> Task<()> {
        let Some(editor) = self.editors.model(&key.scheme, &key.path) else { return Task::ready(()) };
        let formatters = self.range_formatters(editor);
        let select = self.select_formatter(formatters, &key, mode, FormattingKind::Selection, window, cx);
        cx.spawn(async move |workbench, cx| {
            let Some(formatter) = select.await else { return };
            let Ok(format) = workbench.update(cx, |workbench, cx| workbench.format_ranges_with_provider(formatter, key, ranges, cx)) else { return };
            format.await;
        })
    }

    /// `DefaultFormatter._selectFormatter`: the configured or only formatter; otherwise a dialog (explicit) or a
    /// silent notification offers to configure one.
    fn select_formatter(&mut self, formatters: Vec<Formatter>, key: &ModelKey, mode: Mode, kind: FormattingKind, window: AnyWindowHandle, cx: &mut Context<Self>) -> Task<Option<Formatter>> {
        if formatters.is_empty() {
            return Task::ready(None);
        }
        let message = match self.analyze_formatter(&formatters, key, kind) {
            Ok(formatter) => return Task::ready(Some(formatter)),
            Err(message) => message,
        };
        let key = key.clone();
        if mode == Mode::Explicit {
            return cx.spawn(async move |workbench, cx| {
                let answer = window.update(cx, |_, window, cx| dialog::show_message_box(window, PromptLevel::Info, "Configure Default Formatter", Some(&message), &["Configure...", "Cancel"], None, cx)).ok()?;
                if !answer.await.is_ok_and(|(button, _)| button == 0) {
                    return None;
                }
                pick_and_persist_default_formatter(workbench, window, formatters, key, cx).await
            });
        }
        let mut notification = Notification::new(Severity::Info, message);
        notification.priority = Priority::Silent;
        notification.primary = vec![Action { id: "workbench.notification.choice.0".into(), label: "Configure...".into(), run: Run::Reply(0) }];
        let (sender, receiver) = oneshot::channel();
        if let Some(notification_key) = self.notify(notification) {
            self.notifications.replies.insert(notification_key, sender);
        }
        cx.spawn(async move |workbench, cx| {
            if receiver.await.is_ok_and(|answer| answer == Ok(json!(0))) {
                pick_and_persist_default_formatter(workbench, window, formatters, key, cx).await;
            }
        })
        .detach();
        Task::ready(None)
    }

    /// `_analyzeFormatter`: the formatter to use, or why none is picked.
    fn analyze_formatter(&self, formatters: &[Formatter], key: &ModelKey, kind: FormattingKind) -> Result<Formatter, String> {
        let editor = self.editors.model(&key.scheme, &key.path).expect("a formatted model");
        let language = editor.document.language_id.as_str();
        let default_formatter = self.language_setting(editor, "editor.defaultFormatter");
        let language_name = self.languages.name(language);
        match default_formatter.as_str().filter(|id| !id.is_empty()) {
            Some(id) => {
                if let Some(formatter) = formatters.iter().find(|formatter| formatter.extension_id.eq_ignore_ascii_case(id)) {
                    return Ok(formatter.clone());
                }
                if let Some(extension) = self.extensions.iter().find(|extension| extension.id.eq_ignore_ascii_case(id)) {
                    let name = extension.description["displayName"].as_str().or(extension.description["name"].as_str()).unwrap_or(id);
                    return Err(match kind {
                        FormattingKind::File => format!("Extension '{name}' is configured as formatter but it cannot format '{language_name}'-files"),
                        FormattingKind::Selection => format!("Extension '{name}' is configured as formatter but it can only format '{language_name}'-files as a whole, not selections or parts of it."),
                    });
                }
                Err(format!("Extension '{id}' is configured as formatter but not available. Select a different default formatter to continue."))
            }
            None if formatters.len() == 1 => Ok(formatters[0].clone()),
            None => Err(format!("There are multiple formatters for '{}' files. One of them should be configured as default formatter.", maybe_quotes(language_name))),
        }
    }

    /// A setting for the model's resource and language.
    pub(super) fn language_setting(&self, editor: &OpenEditor, key: &str) -> Value {
        let overrides = config::configuration::Overrides { resource: Some(&editor.document.path), override_identifier: Some(&editor.document.language_id) };
        self.settings.lock().unwrap().get_value(Some(key), &overrides).unwrap_or(Value::Null)
    }

    /// `showFormatterPick`: the formatters with the default one focused, then Configure Default Formatter...; answers
    /// the picked formatter's index.
    fn show_formatter_pick(&mut self, key: &ModelKey, formatters: Vec<Formatter>, window: &mut Window, cx: &mut Context<Self>) -> Task<Option<usize>> {
        let editor = self.editors.model(&key.scheme, &key.path).expect("a formatted model");
        let default_formatter = self.language_setting(editor, "editor.defaultFormatter");
        let mut entries: Vec<Entry> = formatters
            .iter()
            .enumerate()
            .map(|(index, formatter)| {
                let is_default = default_formatter.as_str().is_some_and(|id| formatter.extension_id.eq_ignore_ascii_case(id));
                Entry::Item(Item { handle: index as i64, label: formatter.display_name.clone(), description: is_default.then(|| "(default)".to_owned()), ..Item::default() })
            })
            .collect();
        let default_index = formatters.iter().position(|formatter| default_formatter.as_str().is_some_and(|id| formatter.extension_id.eq_ignore_ascii_case(id)));
        let configure = formatters.len() as i64;
        entries.push(Entry::Separator(None));
        entries.push(Entry::Item(Item { handle: configure, label: "Configure Default Formatter...".into(), ..Item::default() }));
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select a formatter".into());
        input.set_items(entries);
        if let Some(index) = default_index {
            input.set_active_items(&[index as i64]);
        }
        let picked = self.choose(input, window, cx);
        let key = key.clone();
        let window = window.window_handle();
        cx.spawn(async move |workbench, cx| {
            let picked = picked.await.ok()?.ok()?.as_i64()?;
            if picked != configure {
                return Some(picked as usize);
            }
            pick_and_persist_default_formatter(workbench, window, formatters, key, cx).await;
            None
        })
    }

    /// `formatDocumentWithProvider`: the provider's edits, made minimal, applied unless the model or (in its editor)
    /// the cursor changed meanwhile.
    pub(super) fn format_document_with_provider(&mut self, formatter: Formatter, key: ModelKey, mode: Mode, cx: &mut Context<Self>) -> Task<bool> {
        let Some(editor) = self.editors.model(&key.scheme, &key.path) else { return Task::ready(false) };
        let uri = editor.document.uri();
        let options = formatting_options(editor);
        let call = match formatter.whole_document {
            false => self.host.call_cancellable("ExtHostLanguageFeatures", "$provideDocumentFormattingEdits", vec![json!(formatter.handle), uri, options]),
            true => {
                let lines = &editor.document.lines;
                let end = bulk_end(lines);
                self.host.call_cancellable("ExtHostLanguageFeatures", "$provideDocumentRangeFormattingEdits", vec![json!(formatter.handle), uri, range_value(lines, Position::new(0, 0), end), options])
            }
        };
        let state = self.edit_state(&key);
        cx.spawn(async move |workbench, cx| {
            let Ok(Ok(edits)) = call.await else { return false };
            workbench.update(cx, |workbench, cx| workbench.apply_formatting_edits(&key, state, vec![edits], mode, cx)).unwrap_or(false)
        })
    }

    /// `formatDocumentRangesWithProvider`: the ranges merged where they touch; a provider without
    /// `provideDocumentRangesFormattingEdits` is asked per range, and again for the union of two ranges whose edits
    /// intersect.
    pub(super) fn format_ranges_with_provider(&mut self, formatter: Formatter, key: ModelKey, ranges: Vec<(Position, Position)>, cx: &mut Context<Self>) -> Task<bool> {
        let Some(editor) = self.editors.model(&key.scheme, &key.path) else { return Task::ready(false) };
        let lines = editor.document.lines.clone();
        let uri = editor.document.uri();
        let options = formatting_options(editor);
        let ranges = merge_touching(ranges);
        let state = self.edit_state(&key);
        let call = self.host.caller();
        cx.spawn(async move |workbench, cx| {
            let ask = |range: (Position, Position)| call("ExtHostLanguageFeatures", "$provideDocumentRangeFormattingEdits", vec![json!(formatter.handle), uri.clone(), range_value(&lines, range.0, range.1), options.clone()]);
            let mut raw_edits: Vec<Value> = Vec::new();
            let mut ranges = ranges;
            if formatter.supports_ranges {
                let values: Vec<Value> = ranges.iter().map(|&(start, end)| range_value(&lines, start, end)).collect();
                let Ok(Ok(edits)) = call("ExtHostLanguageFeatures", "$provideDocumentRangesFormattingEdits", vec![json!(formatter.handle), uri.clone(), json!(values), options.clone()]).await else { return false };
                raw_edits.push(edits);
            } else {
                for &range in &ranges {
                    let Ok(Ok(edits)) = ask(range).await else { return false };
                    raw_edits.push(edits);
                }
                let mut i = 0;
                while i < ranges.len() {
                    let mut j = i + 1;
                    let mut merged = false;
                    while j < ranges.len() {
                        if edits_intersect(&raw_edits[i], &raw_edits[j]) {
                            let union = (ranges[i].0.min(ranges[j].0), ranges[i].1.max(ranges[j].1));
                            let Ok(Ok(edits)) = ask(union).await else { return false };
                            ranges.remove(j);
                            ranges.remove(i);
                            ranges.push(union);
                            raw_edits.remove(j);
                            raw_edits.remove(i);
                            raw_edits.push(edits);
                            merged = true;
                            break;
                        }
                        j += 1;
                    }
                    i = if merged { 0 } else { i + 1 };
                }
            }
            workbench.update(cx, |workbench, cx| workbench.apply_formatting_edits(&key, state, raw_edits, Mode::Explicit, cx)).unwrap_or(false)
        })
    }

    /// The model's version and, in the active editor, its cursor: what `EditorStateCancellationTokenSource(Value |
    /// Position)` and `TextModelCancellationTokenSource` watch.
    fn edit_state(&self, key: &ModelKey) -> (u32, Option<Position>) {
        let editor = self.editors.model(&key.scheme, &key.path).expect("a formatted model");
        let shown = self.editors.active().is_some_and(|active| active.document.is(&key.scheme, &key.path));
        (editor.version_id, shown.then(|| editor.selection().head))
    }

    /// `computeMoreMinimalEdits` of each list, then `FormattingEdit.execute` between undo stops (explicit) with the
    /// cursor's relative position kept, and the cursor revealed when outside the viewport.
    fn apply_formatting_edits(&mut self, key: &ModelKey, state: (u32, Option<Position>), lists: Vec<Value>, mode: Mode, cx: &mut Context<Self>) -> bool {
        if self.edit_state(key) != state {
            return false;
        }
        let viewport = self.editors.viewport;
        let shown = state.1.is_some();
        let editor = self.editors.model_mut(&key.scheme, &key.path).expect("a formatted model");
        let mut edits = Vec::new();
        let mut eol = None;
        for list in &lists {
            let raw: Vec<(Position, Position, String)> = list
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|edit| {
                    eol = edit["eol"].as_u64().or(eol);
                    let text = edit["text"].as_str()?;
                    let (start, end) = model_range(editor, &edit["range"]);
                    Some((start, end, text.to_owned()))
                })
                .collect();
            edits.extend(more_minimal_edits(&editor.document.lines, raw));
        }
        if edits.is_empty() && eol.is_none() {
            return false;
        }
        let explicit = mode == Mode::Explicit;
        if explicit {
            editor.push_undo_stop();
        }
        let scroll = StableScroll::capture(editor, viewport);
        let eol_change = eol.and_then(|sequence| editor.push_eol(sequence));
        // `_isFullModelReplaceEdit`: one edit over the whole model keeps the markers (`replace`), others move them.
        let whole = edits.len() == 1 && edits[0].0 == Position::new(0, 0) && edits[0].1 == bulk_end(&editor.document.lines);
        let edits = edits.into_iter().map(|(start, end, text)| (start, end, text, !whole)).collect();
        let change = editor.execute_edits(edits, None, EditReason::Unknown("formatEditsCommand"));
        if explicit {
            editor.push_undo_stop();
        }
        scroll.restore_relative_cursor(editor, viewport);
        let changes: Vec<ContentChanged> = [eol_change, change].into_iter().flatten().collect();
        if !shown {
            for change in &changes {
                self.send_model_edit(&key.scheme, &key.path, change);
            }
            self.host.send_visible_ranges(&self.editors);
            cx.notify();
            return true;
        }
        if explicit {
            let editor = self.editors.active_mut().expect("the shown editor");
            let head = editor.selection().head;
            editor.reveal_range(head, head, crate::editor::VerticalReveal::CenterIfOutsideViewport, viewport);
        }
        for change in changes {
            self.editor_edited(Some(change), true, "formatEditsCommand", cx);
        }
        true
    }

    /// `FormatOnType`: the typed text's last character, when the first on-type formatter of the model triggers on
    /// it, asks for edits at the cursor; an edit on or before the cursor's line cancels the request.
    pub(super) fn format_on_type(&mut self, typed: &str, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        if self.language_setting(editor, "editor.formatOnType") != json!(true) {
            return;
        }
        let Some(last) = typed.chars().last() else { return };
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        let Some(support) = self.language_features.ordered(ON_TYPE, &candidate).first().map(|registration| (registration.handle, registration.args[2].clone())) else { return };
        let triggers: Vec<char> = support.1.as_array().into_iter().flatten().filter_map(|trigger| trigger.as_str()?.chars().next()).collect();
        if !triggers.contains(&last) || editor.selections.len() > 1 || !editor.selection().is_empty() {
            return;
        }
        let position = editor.selection().head;
        let lines = &editor.document.lines;
        let position_value = json!({ "lineNumber": position.line + 1, "column": utf16_column(&lines[position.line], position.column) + 1 });
        let call = self.host.call_cancellable("ExtHostLanguageFeatures", "$provideOnTypeFormattingEdits", vec![json!(support.0), uri, position_value, json!(last.to_string()), formatting_options(editor)]);
        let key = self.active_model_key().expect("an active editor");
        let (model_id, version_id) = (editor.model_id, editor.version_id);
        let task = cx.spawn(async move |workbench, cx| {
            let Ok(Ok(edits)) = call.await else { return };
            let _ = workbench.update(cx, |workbench, cx| {
                let Some(on_type) = workbench.formatting.on_type.take() else { return };
                let Some(editor) = workbench.editors.active().filter(|editor| editor.model_id == on_type.model_id) else { return };
                if editor.version_id != on_type.version_id || workbench.language_setting(editor, "editor.formatOnType") != json!(true) {
                    return;
                }
                let state = workbench.edit_state(&key);
                workbench.apply_formatting_edits(&key, state, vec![edits], Mode::Explicit, cx);
            });
        });
        self.formatting.on_type = Some(OnType { _task: task, model_id, line: position.line + 1, version_id });
    }

    /// The active model changed: a change ending on or before the line of a running format-on-type request cancels
    /// it; one below it leaves it to apply.
    pub(super) fn format_on_type_content_changed(&mut self, change: &ContentChanged) {
        let Some(on_type) = &mut self.formatting.on_type else { return };
        if change.changes.iter().any(|content| content.end_line <= on_type.line) {
            self.formatting.on_type = None;
            return;
        }
        on_type.version_id = change.version_id;
    }

    /// `_executeFormatDocumentProvider` and `_executeFormatRangeProvider`: the first formatter with edits, made minimal.
    fn execute_format_provider(&mut self, args: &[Value], range: Option<Value>, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = super::host_requests::uri_parts(&args.first().cloned().unwrap_or_default()).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path)) else {
            return Outcome::Ready(Err(crate::commands::error_value("Illegal argument")));
        };
        let uri = editor.document.uri();
        let options = args.get(if range.is_some() { 2 } else { 1 }).cloned().unwrap_or_else(|| formatting_options(editor));
        let whole = range_value(&editor.document.lines, Position::new(0, 0), bulk_end(&editor.document.lines));
        let calls: Vec<(&'static str, Vec<Value>)> = match &range {
            None => self
                .document_formatters(editor)
                .into_iter()
                .map(|formatter| match formatter.whole_document {
                    false => ("$provideDocumentFormattingEdits", vec![json!(formatter.handle), uri.clone(), options.clone()]),
                    true => ("$provideDocumentRangeFormattingEdits", vec![json!(formatter.handle), uri.clone(), whole.clone(), options.clone()]),
                })
                .collect(),
            Some(range) => self.range_formatters(editor).into_iter().map(|formatter| ("$provideDocumentRangeFormattingEdits", vec![json!(formatter.handle), uri.clone(), range.clone(), options.clone()])).collect(),
        };
        let call = self.host.caller();
        let key = ModelKey { scheme: editor.document.scheme.clone(), path: editor.document.path.clone() };
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            for (method, args) in calls {
                let Ok(Ok(edits)) = call("ExtHostLanguageFeatures", method, args).await else { continue };
                if edits.as_array().is_some_and(|edits| !edits.is_empty()) {
                    let minimal = workbench.update(cx, |workbench, _| workbench.minimal_edit_values(&key, &edits)).unwrap_or(edits);
                    let _ = sender.send(Ok(minimal));
                    return;
                }
            }
            let _ = sender.send(Ok(Value::Null));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `_executeFormatOnTypeProvider`: the first on-type formatter's edits at the position, made minimal.
    fn execute_format_on_type_provider(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = super::host_requests::uri_parts(&args.first().cloned().unwrap_or_default()).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path)) else {
            return Outcome::Ready(Err(crate::commands::error_value("Illegal argument")));
        };
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        let Some(handle) = self.language_features.ordered(ON_TYPE, &candidate).first().map(|registration| registration.handle) else {
            return Outcome::Ready(Ok(Value::Null));
        };
        let options = args.get(3).cloned().unwrap_or_else(|| formatting_options(editor));
        let call = self.host.call_cancellable("ExtHostLanguageFeatures", "$provideOnTypeFormattingEdits", vec![json!(handle), uri, args.get(1).cloned().unwrap_or_default(), args.get(2).cloned().unwrap_or_default(), options]);
        let key = ModelKey { scheme: editor.document.scheme.clone(), path: editor.document.path.clone() };
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let edits = match call.await {
                Ok(Ok(edits)) => workbench.update(cx, |workbench, _| workbench.minimal_edit_values(&key, &edits)).unwrap_or(edits),
                _ => Value::Null,
            };
            let _ = sender.send(Ok(edits));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `computeMoreMinimalEdits` as `TextEdit` values.
    fn minimal_edit_values(&self, key: &ModelKey, edits: &Value) -> Value {
        let Some(editor) = self.editors.model(&key.scheme, &key.path) else { return edits.clone() };
        let lines = &editor.document.lines;
        let mut eol = None;
        let raw = edits
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|edit| {
                eol = edit["eol"].as_u64().or(eol);
                Some((model_range(editor, &edit["range"]), edit["text"].as_str()?.to_owned()))
            })
            .map(|((start, end), text)| (start, end, text))
            .collect();
        let mut values: Vec<Value> = more_minimal_edits(lines, raw).into_iter().map(|(start, end, text)| json!({ "range": range_value(lines, start, end), "text": text })).collect();
        if let Some(eol) = eol {
            values.push(json!({ "eol": eol, "text": "", "range": { "startLineNumber": 0, "startColumn": 0, "endLineNumber": 0, "endColumn": 0 } }));
        }
        Value::Array(values)
    }

    /// `FormatOnSaveParticipant.participate`: with `editor.formatOnSave` (not on auto save) the participants before it
    /// run, then the selected formatter formats the file, or its modified lines by `editor.formatOnSaveMode`; `None`
    /// when it does not run.
    pub(super) fn format_on_save(&mut self, scheme: &str, path: &std::path::Path, reason: SaveReason, cx: &mut Context<Self>) -> Option<Task<()>> {
        // A save on window change runs while no window is active; the port has one window per process.
        let window = cx.active_window().or_else(|| cx.windows().first().copied())?;
        let editor = self.editors.model(scheme, path)?;
        if reason == SaveReason::Auto || self.language_setting(editor, "editor.formatOnSave") != json!(true) {
            return None;
        }
        self.run_save_participants(scheme, path, reason, SavePhase::BeforeFormat);
        let editor = self.editors.model(scheme, path)?;
        let mode = self.language_setting(editor, "editor.formatOnSaveMode");
        let key = ModelKey { scheme: scheme.to_owned(), path: path.to_owned() };
        match mode.as_str() {
            Some("modifications") | Some("modificationsIfAvailable") => match self.modified_ranges(&key) {
                None if mode == "modificationsIfAvailable" => Some(self.format_document_with_selected_provider(key, Mode::Silent, window, cx)),
                Some(Some(ranges)) if !ranges.is_empty() => Some(self.format_ranges_with_selected_provider(key, ranges, Mode::Silent, window, cx)),
                _ => None,
            },
            _ => Some(self.format_document_with_selected_provider(key, Mode::Silent, window, cx)),
        }
    }
}

/// The rest of `doSave` once the formatter ran: the participants after it, then the write.
impl Workbench {
    pub(super) fn save_after_format(&mut self, scheme: String, path: PathBuf, reason: SaveReason, options: SaveOptions, format: Task<()>, cx: &mut Context<Self>) -> oneshot::Receiver<bool> {
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            format.await;
            let saved = workbench.update(cx, |workbench, cx| {
                if workbench.editors.model(&scheme, &path).is_some() {
                    workbench.run_save_participants(&scheme, &path, reason, SavePhase::AfterFormat);
                }
                workbench.save_model_with(scheme, path, reason, SaveOptions { skip_participants: true, ..options }, cx)
            });
            let saved = match saved {
                Ok(saved) => saved.await.unwrap_or(false),
                Err(_) => false,
            };
            let _ = sender.send(saved);
        })
        .detach();
        receiver
    }
}

/// `_pickAndPersistDefaultFormatter`: the picked formatter becomes `editor.defaultFormatter` for the model's language.
async fn pick_and_persist_default_formatter(workbench: WeakEntity<Workbench>, window: AnyWindowHandle, formatters: Vec<Formatter>, key: ModelKey, cx: &mut AsyncApp) -> Option<Formatter> {
    let picked = window
        .update(cx, |_, window, cx| workbench.update(cx, |workbench, cx| {
            let editor = workbench.editors.model(&key.scheme, &key.path)?;
            let language_name = workbench.languages.name(&editor.document.language_id).to_owned();
            let entries = formatters
                .iter()
                .enumerate()
                .map(|(index, formatter)| {
                    let label = if formatter.display_name.is_empty() { formatter.extension_id.clone() } else { formatter.display_name.clone() };
                    Entry::Item(Item { handle: index as i64, label, description: Some(formatter.extension_id.clone()), ..Item::default() })
                })
                .collect();
            let mut input = QuickInput::new(0, Kind::QuickPick);
            input.placeholder = Some(format!("Select a default formatter for '{}' files", maybe_quotes(&language_name)));
            input.set_items(entries);
            Some(workbench.choose(input, window, cx))
        }))
        .ok()?
        .ok()??;
    let index = picked.await.ok()?.ok()?.as_u64()? as usize;
    let formatter = formatters.get(index)?.clone();
    if formatter.extension_id.is_empty() {
        return None;
    }
    workbench
        .update(cx, |workbench, _| {
            let editor = workbench.editors.model(&key.scheme, &key.path)?;
            let language = editor.document.language_id.clone();
            let mut settings = workbench.settings.lock().unwrap();
            match settings.update_value("editor.defaultFormatter", Some(&json!(formatter.extension_id)), config::service::Target::User, Some(&key.path), &[language]) {
                Ok(change) if !change.is_empty() => workbench.host.send_configuration_change(&settings, &change),
                Ok(_) => {}
                Err(error) => eprintln!("writing editor.defaultFormatter: {error}"),
            }
            Some(())
        })
        .ok()??;
    Some(formatter)
}

/// A command's task as its pending outcome, answered once the task ends.
fn pending<T: 'static>(cx: &mut Context<Workbench>, task: Task<T>) -> Outcome {
    let (sender, receiver) = oneshot::channel();
    cx.spawn(async move |_, _| {
        task.await;
        let _ = sender.send(Ok(Value::Null));
    })
    .detach();
    Outcome::Pending(receiver)
}

/// `ExtensionIdentifier` as the host sends it: an object with `value`, or a string.
fn extension_id(value: &Value) -> String {
    value["value"].as_str().or(value.as_str()).unwrap_or_default().to_owned()
}

/// `_maybeQuotes`.
fn maybe_quotes(name: &str) -> String {
    if name.contains(char::is_whitespace) { format!("'{name}'") } else { name.to_owned() }
}

/// `model.getFormattingOptions()`.
fn formatting_options(editor: &OpenEditor) -> Value {
    let indentation = editor.document.indentation;
    json!({ "tabSize": indentation.tab_size, "indentSize": indentation.indent_size.unwrap_or(indentation.tab_size), "insertSpaces": indentation.insert_spaces })
}

fn bulk_end(lines: &[String]) -> Position {
    super::bulk_text_edits::document_end(lines)
}

fn range_value(lines: &[String], start: Position, end: Position) -> Value {
    json!({
        "startLineNumber": start.line + 1,
        "startColumn": utf16_column(&lines[start.line], start.column) + 1,
        "endLineNumber": end.line + 1,
        "endColumn": utf16_column(&lines[end.line], end.column) + 1,
    })
}

/// The ranges sorted, the ones intersecting or touching merged.
fn merge_touching(mut ranges: Vec<(Position, Position)>) -> Vec<(Position, Position)> {
    ranges.sort();
    let mut merged: Vec<(Position, Position)> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.0 <= last.1 => last.1 = last.1.max(range.1),
            _ => merged.push(range),
        }
    }
    merged
}

/// `hasIntersectingEdit` of two `TextEdit[]`: an edit of one shares a position with an edit of the other.
fn edits_intersect(a: &Value, b: &Value) -> bool {
    let ranges = |edits: &Value| -> Vec<((u64, u64), (u64, u64))> {
        edits
            .as_array()
            .into_iter()
            .flatten()
            .map(|edit| {
                let range = &edit["range"];
                let at = |line: &str, column: &str| (range[line].as_u64().unwrap_or(0), range[column].as_u64().unwrap_or(0));
                (at("startLineNumber", "startColumn"), at("endLineNumber", "endColumn"))
            })
            .collect()
    };
    let (a, b) = (ranges(a), ranges(b));
    // `Range.intersectRanges` is not null for ranges that touch.
    a.iter().any(|x| b.iter().any(|y| x.0.max(y.0) <= x.1.min(y.1)))
}

/// `EditorWorker.$computeMoreMinimalEdits`: the edits sorted and adjacent ones joined, each replaced by the char
/// changes between the text it replaces and its text (line breaks as `\n`); unchanged and empty edits drop.
pub fn more_minimal_edits(lines: &[String], mut edits: Vec<(Position, Position, String)>) -> Vec<(Position, Position, String)> {
    edits.sort_by_key(|edit| edit.0);
    let mut joined: Vec<(Position, Position, String)> = Vec::new();
    for edit in edits {
        match joined.last_mut() {
            Some(last) if last.1 == edit.0 => {
                last.1 = edit.1;
                last.2.push_str(&edit.2);
            }
            _ => joined.push(edit),
        }
    }
    let mut result = Vec::new();
    for (start, end, text) in joined {
        if start == end && text.is_empty() {
            continue;
        }
        let original = text_in_range(lines, start, end);
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if original == text {
            continue;
        }
        if text.chars().count().max(original.chars().count()) > DIFF_LIMIT {
            result.push((start, end, text));
            continue;
        }
        let modified: Vec<char> = text.chars().collect();
        for (original_range, modified_range) in diff::string_diff(&original, &text) {
            let change_start = position_after(start, &original, original_range.start);
            let change_end = position_after(start, &original, original_range.end);
            result.push((change_start, change_end, modified[modified_range].iter().collect()));
        }
    }
    result
}

/// The text from `start` to `end` with `\n` between lines.
fn text_in_range(lines: &[String], start: Position, end: Position) -> String {
    if start.line == end.line {
        return lines[start.line].chars().skip(start.column).take(end.column - start.column).collect();
    }
    let mut text: String = lines[start.line].chars().skip(start.column).collect();
    for line in &lines[start.line + 1..end.line] {
        text.push('\n');
        text.push_str(line);
    }
    text.push('\n');
    text.extend(lines[end.line].chars().take(end.column));
    text
}

/// The position `offset` chars into `text`, which starts at `start`.
fn position_after(start: Position, text: &str, offset: usize) -> Position {
    let mut position = start;
    for character in text.chars().take(offset) {
        if character == '\n' {
            position = Position::new(position.line + 1, 0);
        } else {
            position.column += 1;
        }
    }
    position
}

/// `StableEditorScrollState.capture` and `restoreRelativeVerticalPositionOfCursor`.
struct StableScroll {
    scroll_top: f32,
    content_height: f32,
    cursor: Position,
}

impl StableScroll {
    fn capture(editor: &OpenEditor, viewport: crate::editor::Viewport) -> StableScroll {
        StableScroll { scroll_top: editor.scroll.top, content_height: editor.scroll_height(viewport), cursor: editor.selection().head }
    }

    fn restore_relative_cursor(&self, editor: &mut OpenEditor, viewport: crate::editor::Viewport) {
        if self.scroll_top == 0. || (self.content_height == editor.scroll_height(viewport) && self.scroll_top == editor.scroll.top) {
            return;
        }
        let current = editor.selection().head;
        let offset = editor.line_top(editor.view_line(current.line), viewport) - editor.line_top(editor.view_line(self.cursor.line), viewport);
        editor.set_scroll(editor.scroll.top + offset, editor.scroll.left, viewport);
    }
}

#[cfg(test)]
mod tests;
