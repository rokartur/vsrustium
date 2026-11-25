//! documentSemanticTokens.ts, viewportSemanticTokens.ts and getSemanticTokens.ts: the semantic tokens of the active
//! model from the extensions' providers, styled by the theme over the TextMate tokens (sparseTokensStore.ts).

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::ops::Range;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use exthost::session::VSBUFFER_KEY;
use futures::channel::oneshot;
use gpui::{Context, FontStyle, FontWeight, HighlightStyle, Hsla, Rgba, StrikethroughStyle, Task, UnderlineStyle, px};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::{ContentChange, ContentChanged, OpenEditor, char_column, expand_tabs};
use crate::language_features::{Candidate, LanguageFeatures};
use crate::semantic_tokens::{
    SemanticTokensDto, SemanticTokensLegend, SemanticTokensProviderStyling, TokenStyle, apply_semantic_tokens_edits, decode_semantic_tokens_dto,
    is_semantic_coloring_enabled, to_multiline_tokens,
};

const DOCUMENT_REGISTER: &str = "$registerDocumentSemanticTokensProvider";
const RANGE_REGISTER: &str = "$registerDocumentRangeSemanticTokensProvider";
/// `ModelSemanticColoring.REQUEST_MIN_DELAY`
// shortcut: the debounce is the minimum, not `languageFeatureDebounceService`'s average of the request times; port
// that service once a provider is slow enough for 300ms to flood it.
const DOCUMENT_DELAY: Duration = Duration::from_millis(300);
/// The minimum of `ViewportSemanticTokensContribution`'s debounce.
const RANGE_DELAY: Duration = Duration::from_millis(100);

#[derive(Default)]
pub(super) struct SemanticTokens {
    model_id: Option<u32>,
    /// A `ModelSemanticColoring` watches the model: semantic coloring is enabled for it.
    watching: bool,
    /// `_currentDocumentResponse`
    response: Option<Response>,
    /// The model's semantic tokens (`tokenization.setSemanticTokens`), sorted by line, then start.
    tokens: Vec<Token>,
    /// `hasCompleteSemanticTokens`
    complete: bool,
    /// `_fetchDocumentSemanticTokens` while it is scheduled.
    fetch: Option<Task<()>>,
    /// The running document request (`_currentDocumentRequestCancellationTokenSource`).
    request: Option<Task<()>>,
    /// The model's changes since the document request went out.
    pending_changes: Vec<ContentChange>,
    providers_changed_during_request: bool,
    /// `SemanticTokensStylingService.getStyling` per provider handle; a theme change drops them.
    stylings: HashMap<u64, SemanticTokensProviderStyling>,
    /// `_tokenizeViewport` while it is scheduled.
    viewport: Option<Task<()>>,
    /// `_outstandingRequests`, by request id.
    range_requests: Vec<(u64, Task<()>)>,
    next_range_request: u64,
}

struct Response {
    handle: u64,
    result_id: u32,
    data: Vec<u32>,
}

/// A styled token: 0-based line, UTF-16 columns.
#[derive(Clone, Debug, PartialEq)]
struct Token {
    line: usize,
    start: u32,
    end: u32,
    style: TokenStyle,
}

type Call = (u64, oneshot::Receiver<Result<Value, Value>>);

impl Workbench {
    pub(super) fn register_semantic_tokens(&mut self) {
        self.commands.register("_provideDocumentSemanticTokensLegend", |workbench, args, _, cx| workbench.provide_document_semantic_tokens_legend(&args, cx));
        self.commands.register("_provideDocumentSemanticTokens", |workbench, args, _, cx| workbench.provide_document_semantic_tokens(&args, cx));
        self.commands.register("_provideDocumentRangeSemanticTokensLegend", |workbench, args, _, cx| workbench.provide_document_range_semantic_tokens_legend(&args, cx));
        self.commands.register("_provideDocumentRangeSemanticTokens", |workbench, args, _, cx| workbench.provide_document_range_semantic_tokens(&args, cx));
    }

    /// `DocumentSemanticTokensFeature`'s watcher moves to the active model; the viewport asks again (`onDidChangeModel`).
    // shortcut: only the active model keeps semantic tokens, so a model shown again waits for its provider; keep a
    // `ModelSemanticColoring` per open model once background models are rendered.
    // shortcut: no `onDidChangeLanguage` reset; a language change keeps the tokens until the next request, add a
    // language hook once the port changes a model's language in place.
    pub(super) fn semantic_tokens_model_changed(&mut self, cx: &mut Context<Self>) {
        let model_id = self.editors.active().map(|editor| editor.model_id);
        if model_id == self.semantic_tokens.model_id {
            return;
        }
        self.dispose_semantic_coloring();
        self.semantic_tokens.model_id = model_id;
        if self.editors.active().is_some_and(|editor| self.semantic_coloring_enabled(editor)) {
            self.register_semantic_coloring(cx);
        }
        self.cancel_range_requests();
        self.schedule_viewport_tokens(cx);
        cx.notify();
    }

    /// `onDidChangeContent`: the tokens follow the edit (`SparseMultilineTokens.acceptEdit`), the document asks again
    /// after the debounce, a running request records the change, the viewport drops its requests.
    pub(super) fn semantic_tokens_content_changed(&mut self, change: &ContentChanged, cx: &mut Context<Self>) {
        for content in &change.changes {
            apply_content_change(&mut self.semantic_tokens.tokens, content);
        }
        let state = &mut self.semantic_tokens;
        if state.watching {
            if state.request.is_some() {
                state.pending_changes.extend(change.changes.iter().cloned());
            }
            if state.fetch.is_none() {
                self.schedule_document_tokens(DOCUMENT_DELAY, cx);
            }
        }
        self.cancel_range_requests();
        self.schedule_viewport_tokens(cx);
    }

    /// The registries' `onDidChange`: `handleRegistryChange`, and the viewport asks again.
    pub(super) fn semantic_tokens_providers_changed(&mut self, cx: &mut Context<Self>) {
        if self.semantic_tokens.watching {
            self.schedule_document_tokens(DOCUMENT_DELAY, cx);
        }
        self.cancel_range_requests();
        self.schedule_viewport_tokens(cx);
    }

    /// `handleSettingOrThemeChange`: the watcher goes when coloring turns off, comes when it turns on, and else
    /// drops the tokens and asks again (`handleThemeChange`).
    // shortcut: a theme change and an `editor.semanticHighlighting.enabled` change arrive here alike, so a setting
    // change that keeps coloring on also asks again; split the hook when that refetch costs too much.
    pub(super) fn semantic_tokens_settings_changed(&mut self, cx: &mut Context<Self>) {
        self.semantic_tokens.stylings.clear();
        let enabled = self.editors.active().is_some_and(|editor| self.semantic_coloring_enabled(editor));
        match (self.semantic_tokens.watching, enabled) {
            (true, false) => self.dispose_semantic_coloring(),
            (false, true) => self.register_semantic_coloring(cx),
            (true, true) => {
                self.release_document_response();
                self.set_semantic_tokens(Vec::new(), false);
                self.schedule_document_tokens(DOCUMENT_DELAY, cx);
            }
            (false, false) => {}
        }
        self.cancel_range_requests();
        self.schedule_viewport_tokens(cx);
        cx.notify();
    }

    /// `$emitDocumentSemanticTokensEvent` and `$emitDocumentRangeSemanticTokensEvent`: a provider of the model
    /// changed its tokens (`handleProviderDidChange`, the range providers' `onDidChange`).
    pub(crate) fn semantic_tokens_event(&mut self, event_handle: u64, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        let fired = |method: &str| self.language_features.ordered(method, &candidate).iter().any(|registration| registration.args.get(3) == Some(&json!(event_handle)));
        let (document, range) = (fired(DOCUMENT_REGISTER), fired(RANGE_REGISTER));
        if document && self.semantic_tokens.watching {
            match self.semantic_tokens.request.is_some() {
                true => self.semantic_tokens.providers_changed_during_request = true,
                false => self.schedule_document_tokens(Duration::ZERO, cx),
            }
        }
        if range {
            self.cancel_range_requests();
            self.schedule_viewport_tokens(cx);
        }
    }

    /// The semantic styles of line `index` over its tab-expanded text, to lay over the TextMate styles with
    /// `overlay_semantic_styles`.
    pub(super) fn semantic_token_styles(&self, editor: &OpenEditor, index: usize, tab_size: usize) -> Vec<(Range<usize>, HighlightStyle)> {
        if self.semantic_tokens.model_id != Some(editor.model_id) {
            return Vec::new();
        }
        let tokens = &self.semantic_tokens.tokens;
        let first = tokens.partition_point(|token| token.line < index);
        let text = &editor.document.lines[index];
        let expanded = |utf16: u32| expand_tabs(&text.chars().take(char_column(text, utf16 as usize)).collect::<String>(), tab_size).len();
        let mut styles = Vec::new();
        for token in tokens[first..].iter().take_while(|token| token.line == index) {
            let (start, end) = (expanded(token.start), expanded(token.end));
            if start < end {
                styles.push((start..end, highlight(&token.style)));
            }
        }
        styles
    }

    /// `isSemanticColoringEnabled` for the model.
    fn semantic_coloring_enabled(&self, editor: &OpenEditor) -> bool {
        is_semantic_coloring_enabled(&self.suggest_setting(editor, "editor.semanticHighlighting.enabled"), self.theme.semantic_highlighting)
    }

    /// `new ModelSemanticColoring`: asks at once.
    fn register_semantic_coloring(&mut self, cx: &mut Context<Self>) {
        self.semantic_tokens.watching = true;
        self.schedule_document_tokens(Duration::ZERO, cx);
    }

    /// `ModelSemanticColoring.dispose`
    fn dispose_semantic_coloring(&mut self) {
        self.release_document_response();
        let state = &mut self.semantic_tokens;
        state.watching = false;
        state.request = None;
        state.fetch = None;
        self.set_semantic_tokens(Vec::new(), false);
    }

    /// `tokenization.setSemanticTokens`
    fn set_semantic_tokens(&mut self, tokens: Vec<Token>, complete: bool) {
        self.semantic_tokens.tokens = tokens;
        self.semantic_tokens.complete = complete;
    }

    /// `SemanticTokensResponse.dispose`
    fn release_document_response(&mut self) {
        if let Some(response) = self.semantic_tokens.response.take() {
            self.release_semantic_tokens(response.handle, response.result_id);
        }
    }

    fn release_semantic_tokens(&self, handle: u64, result_id: u32) {
        self.host.send("ExtHostLanguageFeatures", "$releaseDocumentSemanticTokens", vec![json!(handle), json!(result_id)]);
    }

    /// `_fetchDocumentSemanticTokens.schedule(delay)`
    fn schedule_document_tokens(&mut self, delay: Duration, cx: &mut Context<Self>) {
        self.semantic_tokens.fetch = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(delay).await;
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.semantic_tokens.fetch = None;
                workbench.fetch_document_semantic_tokens_now(cx);
            });
        }));
    }

    /// `_fetchDocumentSemanticTokensNow`: the highest scoring providers, the last result's id to its provider.
    fn fetch_document_semantic_tokens_now(&mut self, cx: &mut Context<Self>) {
        if self.semantic_tokens.request.is_some() {
            return;
        }
        let Some(editor) = self.editors.active().filter(|editor| Some(editor.model_id) == self.semantic_tokens.model_id) else { return };
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        let handles = highest_group(&self.language_features, DOCUMENT_REGISTER, &candidate);
        if handles.is_empty() {
            if self.semantic_tokens.response.is_some() {
                self.set_semantic_tokens(Vec::new(), false);
                cx.notify();
            }
            return;
        }
        let last = self.semantic_tokens.response.as_ref().map(|response| (response.handle, response.result_id));
        let calls: Vec<Call> = handles
            .into_iter()
            .map(|handle| {
                let previous_result_id = last.filter(|(last_handle, _)| *last_handle == handle).map_or(0, |(_, result_id)| result_id);
                (handle, self.host.call_cancellable("ExtHostLanguageFeatures", "$provideDocumentSemanticTokens", vec![json!(handle), uri.clone(), json!(previous_result_id)]))
            })
            .collect();
        let line_count = editor.document.lines.len();
        self.semantic_tokens.pending_changes.clear();
        self.semantic_tokens.providers_changed_during_request = false;
        self.semantic_tokens.request = Some(cx.spawn(async move |workbench, cx| {
            let result = document_semantic_tokens(calls).await;
            let _ = workbench.update(cx, |workbench, cx| workbench.document_semantic_tokens_answered(result, line_count, cx));
        }));
    }

    fn document_semantic_tokens_answered(&mut self, result: Result<Option<(u64, Option<SemanticTokensDto>)>, Value>, line_count: usize, cx: &mut Context<Self>) {
        self.semantic_tokens.request = None;
        let pending_changes = std::mem::take(&mut self.semantic_tokens.pending_changes);
        match result {
            Ok(Some((handle, tokens))) => self.set_document_semantic_tokens(Some(handle), tokens, line_count, &pending_changes, cx),
            Ok(None) => self.set_document_semantic_tokens(None, None, line_count, &pending_changes, cx),
            // Upstream reads an error as tokens not available yet.
            Err(_) => self.reschedule_document_tokens_if_needed(&pending_changes, cx),
        }
        cx.notify();
    }

    /// `_setDocumentSemanticTokens`: a delta applies to the last result's data; the changes made while the request
    /// ran move the new tokens.
    fn set_document_semantic_tokens(&mut self, provider: Option<u64>, tokens: Option<SemanticTokensDto>, line_count: usize, pending_changes: &[ContentChange], cx: &mut Context<Self>) {
        let current = self.semantic_tokens.response.take();
        if let Some(current) = &current {
            self.release_semantic_tokens(current.handle, current.result_id);
        }
        let Some(handle) = provider.filter(|handle| self.language_features.registration(*handle).is_some()) else {
            self.set_semantic_tokens(Vec::new(), false);
            return;
        };
        let Some(tokens) = tokens else {
            self.set_semantic_tokens(Vec::new(), true);
            self.reschedule_document_tokens_if_needed(pending_changes, cx);
            return;
        };
        let (result_id, data) = match tokens {
            SemanticTokensDto::Full { id, data } => (id, data),
            SemanticTokensDto::Delta { id, deltas } => {
                let Some(current) = current else {
                    self.release_semantic_tokens(handle, id);
                    self.set_semantic_tokens(Vec::new(), true);
                    return;
                };
                match apply_semantic_tokens_edits(&current.data, &deltas) {
                    Ok(data) => (id, data),
                    Err(_) => {
                        self.release_semantic_tokens(handle, id);
                        self.set_semantic_tokens(Vec::new(), true);
                        self.schedule_document_tokens(Duration::ZERO, cx);
                        return;
                    }
                }
            }
        };
        let mut styled = self.style_semantic_tokens(handle, &data, line_count);
        for change in pending_changes {
            apply_content_change(&mut styled, change);
        }
        self.semantic_tokens.response = Some(Response { handle, result_id, data });
        self.set_semantic_tokens(styled, true);
        self.reschedule_document_tokens_if_needed(pending_changes, cx);
    }

    fn reschedule_document_tokens_if_needed(&mut self, pending_changes: &[ContentChange], cx: &mut Context<Self>) {
        let changed = !pending_changes.is_empty() || self.semantic_tokens.providers_changed_during_request;
        if changed && self.semantic_tokens.fetch.is_none() {
            self.schedule_document_tokens(DOCUMENT_DELAY, cx);
        }
    }

    /// `toMultilineTokens2` with the provider's styling (`getStyling`).
    fn style_semantic_tokens(&mut self, handle: u64, data: &[u32], line_count: usize) -> Vec<Token> {
        let Some(editor) = self.editors.active() else { return Vec::new() };
        let styling = match self.semantic_tokens.stylings.entry(handle) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let Some(registration) = self.language_features.registration(handle) else { return Vec::new() };
                entry.insert(SemanticTokensProviderStyling::new(legend(&registration.args[2])))
            }
        };
        let theme = &self.theme.semantic;
        let style_of = |token_type: &str, modifiers: &[&str], language: &str| theme.get_token_style_metadata(token_type, modifiers, language);
        let lines = to_multiline_tokens(data, styling, &editor.document.language_id, line_count, &style_of);
        let mut tokens = Vec::new();
        for (line, ranges) in lines.into_iter().enumerate() {
            tokens.extend(ranges.into_iter().map(|range| Token { line, start: range.start, end: range.end, style: range.style }));
        }
        tokens
    }

    fn schedule_viewport_tokens(&mut self, cx: &mut Context<Self>) {
        self.semantic_tokens.viewport = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(RANGE_DELAY).await;
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.semantic_tokens.viewport = None;
                workbench.tokenize_viewport_now(cx);
            });
        }));
    }

    /// `_cancelAll`
    fn cancel_range_requests(&mut self) {
        self.semantic_tokens.range_requests.clear();
    }

    /// `_tokenizeViewportNow`: without complete document tokens, the range providers are asked for the visible
    /// ranges and a viewport above and below them.
    fn tokenize_viewport_now(&mut self, cx: &mut Context<Self>) {
        if self.semantic_tokens.complete {
            return;
        }
        let Some(editor) = self.editors.active().filter(|editor| Some(editor.model_id) == self.semantic_tokens.model_id) else { return };
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        let handles = highest_group(&self.language_features, RANGE_REGISTER, &candidate);
        if !self.semantic_coloring_enabled(editor) || handles.is_empty() {
            if !self.semantic_tokens.tokens.is_empty() {
                self.set_semantic_tokens(Vec::new(), false);
                cx.notify();
            }
            return;
        }
        let (model_id, version_id, line_count) = (editor.model_id, editor.version_id, editor.document.lines.len());
        for (first, last) in visible_ranges_plus_viewport_above_below(editor, self.editors.viewport) {
            let range = json!({ "startLineNumber": first + 1, "startColumn": 1, "endLineNumber": last + 1, "endColumn": editor.document.lines[last].encode_utf16().count() + 1 });
            let calls: Vec<Call> = handles
                .iter()
                .map(|&handle| (handle, self.host.call_cancellable("ExtHostLanguageFeatures", "$provideDocumentRangeSemanticTokens", vec![json!(handle), uri.clone(), range.clone()])))
                .collect();
            let id = self.semantic_tokens.next_range_request;
            self.semantic_tokens.next_range_request += 1;
            let request = cx.spawn(async move |workbench, cx| {
                let result = range_semantic_tokens(calls).await;
                let _ = workbench.update(cx, |workbench, cx| {
                    workbench.semantic_tokens.range_requests.retain(|(request, _)| *request != id);
                    workbench.range_semantic_tokens_answered(model_id, version_id, line_count, (first, last), result, cx);
                });
            });
            self.semantic_tokens.range_requests.push((id, request));
        }
    }

    /// `_requestRange`'s answer: dropped when the model changed since, else `setPartialSemanticTokens`.
    fn range_semantic_tokens_answered(&mut self, model_id: u32, version_id: u32, line_count: usize, (first, last): (usize, usize), result: Option<(u64, Option<Vec<u32>>)>, cx: &mut Context<Self>) {
        let Some((handle, Some(data))) = result else { return };
        let current = self.editors.active().is_some_and(|editor| (editor.model_id, editor.version_id) == (model_id, version_id));
        if !current || self.semantic_tokens.complete {
            return;
        }
        let new_tokens = self.style_semantic_tokens(handle, &data, line_count);
        let first = new_tokens.first().map_or(first, |token| token.line.min(first));
        let last = new_tokens.last().map_or(last, |token| token.line.max(last));
        let tokens = &mut self.semantic_tokens.tokens;
        let start = tokens.partition_point(|token| token.line < first);
        let end = tokens.partition_point(|token| token.line <= last);
        tokens.splice(start..end, new_tokens);
        cx.notify();
    }

    /// `_provideDocumentSemanticTokensLegend`: the first provider's legend, else the range providers'.
    fn provide_document_semantic_tokens_legend(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let editor = match self.command_model(args) {
            Ok(Some(editor)) => editor,
            Ok(None) => return Outcome::Ready(Ok(Value::Null)),
            Err(error) => return Outcome::Ready(Err(error)),
        };
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        let Some(&handle) = highest_group(&self.language_features, DOCUMENT_REGISTER, &candidate).first() else {
            return self.provide_document_range_semantic_tokens_legend(&[uri], cx);
        };
        Outcome::Ready(Ok(self.legend_value(handle)))
    }

    /// `_provideDocumentSemanticTokens`: the full tokens as an encoded `VSBuffer`, else the range providers' for the
    /// whole model.
    fn provide_document_semantic_tokens(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let editor = match self.command_model(args) {
            Ok(Some(editor)) => editor,
            Ok(None) => return Outcome::Ready(Ok(Value::Null)),
            Err(error) => return Outcome::Ready(Err(error)),
        };
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        let handles = highest_group(&self.language_features, DOCUMENT_REGISTER, &candidate);
        if handles.is_empty() {
            let lines = &editor.document.lines;
            let full_range = json!({ "startLineNumber": 1, "startColumn": 1, "endLineNumber": lines.len(), "endColumn": lines[lines.len() - 1].encode_utf16().count() + 1 });
            return self.provide_document_range_semantic_tokens(&[uri, full_range], cx);
        }
        let calls: Vec<Call> = handles.into_iter().map(|handle| (handle, self.host.call_cancellable("ExtHostLanguageFeatures", "$provideDocumentSemanticTokens", vec![json!(handle), uri.clone(), json!(0)]))).collect();
        let call = self.host.caller();
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |_, _| {
            let reply = match document_semantic_tokens(calls).await {
                Err(error) => Err(error),
                Ok(Some((handle, Some(SemanticTokensDto::Full { id, data })))) => {
                    let buffer = encode_full_tokens(&data);
                    drop(call("ExtHostLanguageFeatures", "$releaseDocumentSemanticTokens", vec![json!(handle), json!(id)]));
                    Ok(buffer)
                }
                Ok(_) => Ok(Value::Null),
            };
            let _ = sender.send(reply);
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `_provideDocumentRangeSemanticTokensLegend`: one provider's legend; with several, the legend of the one whose
    /// tokens for the range win.
    fn provide_document_range_semantic_tokens_legend(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let editor = match self.command_model(args) {
            Ok(Some(editor)) => editor,
            Ok(None) => return Outcome::Ready(Ok(Value::Null)),
            Err(error) => return Outcome::Ready(Err(error)),
        };
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        let handles = highest_group(&self.language_features, RANGE_REGISTER, &candidate);
        let range = args.get(1).filter(|range| is_range(range));
        let range = match (handles.len(), range) {
            (0, _) => return Outcome::Ready(Ok(Value::Null)),
            (1, _) | (_, None) => return Outcome::Ready(Ok(self.legend_value(handles[0]))),
            (_, Some(range)) => range.clone(),
        };
        let legends: Vec<(u64, Value)> = handles.iter().map(|&handle| (handle, self.legend_value(handle))).collect();
        let calls: Vec<Call> = handles.into_iter().map(|handle| (handle, self.host.call_cancellable("ExtHostLanguageFeatures", "$provideDocumentRangeSemanticTokens", vec![json!(handle), uri.clone(), range.clone()]))).collect();
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |_, _| {
            let legend = match range_semantic_tokens(calls).await {
                Some((handle, _)) => legends.into_iter().find(|(legend_handle, _)| *legend_handle == handle).map_or(Value::Null, |(_, legend)| legend),
                None => Value::Null,
            };
            let _ = sender.send(Ok(legend));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `_provideDocumentRangeSemanticTokens`: the range's tokens as an encoded `VSBuffer`.
    fn provide_document_range_semantic_tokens(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let model = match self.command_model(args) {
            Ok(model) => model,
            Err(error) => return Outcome::Ready(Err(error)),
        };
        let Some(range) = args.get(1).filter(|range| is_range(range)).cloned() else {
            return Outcome::Ready(Err(exthost::main_thread::serialized_error("Unexpected type")));
        };
        let Some(editor) = model else { return Outcome::Ready(Ok(Value::Null)) };
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        let handles = highest_group(&self.language_features, RANGE_REGISTER, &candidate);
        let calls: Vec<Call> = handles.into_iter().map(|handle| (handle, self.host.call_cancellable("ExtHostLanguageFeatures", "$provideDocumentRangeSemanticTokens", vec![json!(handle), uri.clone(), range.clone()]))).collect();
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |_, _| {
            let buffer = match range_semantic_tokens(calls).await {
                Some((_, Some(data))) => encode_full_tokens(&data),
                _ => Value::Null,
            };
            let _ = sender.send(Ok(buffer));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// The model of a command's `uri` argument (`assertType(uri instanceof URI)`, `getModel`).
    fn command_model(&self, args: &[Value]) -> Result<Option<&OpenEditor>, Value> {
        let (scheme, path) = super::host_requests::uri_parts(args.first().unwrap_or(&Value::Null))?;
        Ok(self.editors.model(scheme, &path))
    }

    /// `getLegend()`: the legend the provider registered with.
    fn legend_value(&self, handle: u64) -> Value {
        self.language_features.registration(handle).map_or(Value::Null, |registration| registration.args[2].clone())
    }
}

/// `orderedGroups(model)[0]`: the handles of the best scoring providers.
fn highest_group(language_features: &LanguageFeatures, method: &str, candidate: &Candidate) -> Vec<u64> {
    let groups = language_features.ordered_groups(method, candidate);
    groups.first().map_or(Vec::new(), |group| group.iter().map(|registration| registration.handle).collect())
}

fn legend(value: &Value) -> SemanticTokensLegend {
    let strings = |key: &str| value[key].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
    SemanticTokensLegend { token_types: strings("tokenTypes"), token_modifiers: strings("tokenModifiers") }
}

/// `Range.isIRange`
fn is_range(value: &Value) -> bool {
    ["startLineNumber", "startColumn", "endLineNumber", "endColumn"].iter().all(|key| value[key].is_number())
}

/// `getDocumentSemanticTokens` with `MainThreadDocumentSemanticTokensProvider`: the first provider that threw, else
/// the first with tokens, else the first.
async fn document_semantic_tokens(calls: Vec<Call>) -> Result<Option<(u64, Option<SemanticTokensDto>)>, Value> {
    let mut results = Vec::new();
    for (handle, call) in calls {
        let result = match call.await {
            Ok(Ok(reply)) => Ok(decode_reply(&reply)),
            Ok(Err(error)) => Err(error),
            Err(oneshot::Canceled) => Err(Value::Null),
        };
        results.push((handle, result));
    }
    let first = results.first().map(|(handle, _)| *handle);
    for (handle, result) in results {
        if let Some(tokens) = result? {
            return Ok(Some((handle, Some(tokens))));
        }
    }
    Ok(first.map(|handle| (handle, None)))
}

/// `getDocumentRangeSemanticTokens` with `MainThreadDocumentRangeSemanticTokensProvider`: a provider that threw or
/// answered a delta counts as no tokens.
async fn range_semantic_tokens(calls: Vec<Call>) -> Option<(u64, Option<Vec<u32>>)> {
    let mut results = Vec::new();
    for (handle, call) in calls {
        let data = match call.await {
            Ok(Ok(reply)) => match decode_reply(&reply) {
                Some(SemanticTokensDto::Full { data, .. }) => Some(data),
                _ => None,
            },
            _ => None,
        };
        results.push((handle, data));
    }
    let first = results.first().map(|(handle, _)| *handle);
    results.into_iter().find(|(_, data)| data.is_some()).or(first.map(|handle| (handle, None)))
}

/// A `VSBuffer` reply arrives as the array of its bytes; `null` is no tokens.
fn decode_reply(reply: &Value) -> Option<SemanticTokensDto> {
    let bytes: Vec<u8> = reply.as_array()?.iter().map(|byte| byte.as_u64().and_then(|byte| u8::try_from(byte).ok())).collect::<Option<_>>()?;
    decode_semantic_tokens_dto(&bytes)
}

/// `encodeSemanticTokensDto({ id: 0, type: 'full', data })` as the `VSBuffer` a command answers.
fn encode_full_tokens(data: &[u32]) -> Value {
    let mut bytes = Vec::with_capacity(4 * (data.len() + 3));
    for value in [0, 1, data.len() as u32].iter().chain(data) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    json!({ VSBUFFER_KEY: STANDARD.encode(bytes) })
}

/// `getVisibleRangesPlusViewportAboveBelow`: the completely visible view lines and a viewport (20 lines at least)
/// above and below, as model line ranges around the folded lines.
fn visible_ranges_plus_viewport_above_below(editor: &OpenEditor, viewport: crate::editor::Viewport) -> Vec<(usize, usize)> {
    let lines = editor.completely_visible_lines(viewport);
    let around = ((viewport.height / viewport.line_height).round() as usize).max(20);
    let start = lines.start.saturating_sub(around);
    let end = (lines.end - 1).saturating_add(around).min(editor.view_line_count() - 1);
    editor.folding.shown_ranges(editor.model_line(start), editor.model_line(end))
}

/// `SparseMultilineTokens.acceptEdit`: the delete, then the insert of one change, on 0-based lines.
fn apply_content_change(tokens: &mut Vec<Token>, change: &ContentChange) {
    let (start_line, end_line) = (change.start_line - 1, change.end_line - 1);
    let (start, end) = (change.start_column as i64 - 1, change.end_column as i64 - 1);
    if start_line != end_line || start != end {
        accept_delete_range(tokens, start_line, start, end_line, end);
    }
    let text = change.text.replace("\r\n", "\n").replace('\r', "\n");
    let eol_count = text.matches('\n').count();
    let first_line_length = text.split('\n').next().map_or(0, |line| line.encode_utf16().count()) as i64;
    let last_line_length = text.rsplit('\n').next().map_or(0, |line| line.encode_utf16().count()) as i64;
    if eol_count == 0 && first_line_length == 0 {
        return;
    }
    let one_word_character = eol_count == 0 && first_line_length == 1 && text.starts_with(|c: char| c.is_ascii_alphanumeric());
    accept_insert_text(tokens, start_line, start, eol_count, first_line_length, last_line_length, one_word_character);
}

/// `SparseMultilineTokensStorage.acceptDeleteRange`
fn accept_delete_range(tokens: &mut Vec<Token>, start_line: usize, start: i64, end_line: usize, end: i64) {
    let deleted_lines = end_line - start_line;
    tokens.retain_mut(|token| {
        let (token_start, token_end) = (token.start as i64, token.end as i64);
        if token.line < start_line || (token.line == start_line && token_end <= start) {
            return true;
        }
        let ends_after = token.line == end_line && token_end > end;
        if token.line == start_line && token_start < start {
            token.end = if ends_after { (token_end - (end - start)) as u32 } else { start as u32 };
        } else if token.line == start_line && token_start == start {
            if !ends_after {
                return false;
            }
            token.end = (token_end - (end - start)) as u32;
        } else if token.line < end_line || (token.line == end_line && token_start < end) {
            if !ends_after {
                return false;
            }
            (token.line, token.start, token.end) = (start_line, start as u32, (start + token_end - end) as u32);
        } else if token.line > end_line {
            token.line -= deleted_lines;
        } else {
            token.line -= deleted_lines;
            (token.start, token.end) = ((token_start - (end - start)) as u32, (token_end - (end - start)) as u32);
        }
        true
    });
}

/// `SparseMultilineTokensStorage.acceptInsertText`: a token grows by one typed word character at its edges.
fn accept_insert_text(tokens: &mut [Token], line: usize, character: i64, eol_count: usize, first_line_length: i64, last_line_length: i64, one_word_character: bool) {
    for token in tokens {
        let (start, end) = (token.start as i64, token.end as i64);
        if token.line < line || (token.line == line && end < character) {
            continue;
        }
        if token.line == line && end == character {
            if one_word_character {
                token.end += 1;
            }
            continue;
        }
        if token.line == line && start < character && character < end {
            token.end = if eol_count == 0 { (end + first_line_length) as u32 } else { character as u32 };
            continue;
        }
        if token.line == line && start == character && one_word_character {
            continue;
        }
        if token.line != line {
            token.line += eol_count;
            continue;
        }
        token.line += eol_count;
        if eol_count == 0 {
            (token.start, token.end) = ((start + first_line_length) as u32, (end + first_line_length) as u32);
        } else {
            let new_start = last_line_length + (start - character);
            (token.start, token.end) = (new_start as u32, (new_start + end - start) as u32);
        }
    }
}

/// A token style as a highlight; a property the style leaves unset stays `None`.
fn highlight(style: &TokenStyle) -> HighlightStyle {
    // A zero thickness is an explicit `false`, which removes the TextMate line in `overlay_semantic_styles`.
    let line_thickness = |on: bool| if on { px(1.) } else { px(0.) };
    HighlightStyle {
        color: style.foreground.as_deref().and_then(|hex| Rgba::try_from(hex).ok()).map(Hsla::from),
        font_weight: style.bold.map(|bold| if bold { FontWeight::BOLD } else { FontWeight::NORMAL }),
        font_style: style.italic.map(|italic| if italic { FontStyle::Italic } else { FontStyle::Normal }),
        underline: style.underline.map(|on| UnderlineStyle { thickness: line_thickness(on), color: None, wavy: false }),
        strikethrough: style.strikethrough.map(|on| StrikethroughStyle { thickness: line_thickness(on), color: None }),
        ..HighlightStyle::default()
    }
}

/// The semantic styles over the TextMate ones (`SparseTokensStore.addSparseTokens`): each property a semantic token
/// sets replaces the TextMate token's, the others stay. Both lists are sorted and do not overlap themselves.
pub(super) fn overlay_semantic_styles(base: Vec<(Range<usize>, HighlightStyle)>, semantic: &[(Range<usize>, HighlightStyle)]) -> Vec<(Range<usize>, HighlightStyle)> {
    if semantic.is_empty() {
        return base;
    }
    let mut points: Vec<usize> = base.iter().chain(semantic).flat_map(|(range, _)| [range.start, range.end]).collect();
    points.sort_unstable();
    points.dedup();
    let (mut base_index, mut semantic_index) = (0, 0);
    let mut result: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    for pair in points.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        while base_index < base.len() && base[base_index].0.end <= start {
            base_index += 1;
        }
        while semantic_index < semantic.len() && semantic[semantic_index].0.end <= start {
            semantic_index += 1;
        }
        let base_style = base.get(base_index).filter(|(range, _)| range.start <= start).map(|(_, style)| *style);
        let semantic_style = semantic.get(semantic_index).filter(|(range, _)| range.start <= start).map(|(_, style)| *style);
        let style = match (base_style, semantic_style) {
            (None, None) => continue,
            (Some(base_style), None) => base_style,
            (base_style, Some(semantic_style)) => merge_styles(base_style.unwrap_or_default(), semantic_style),
        };
        match result.last_mut() {
            Some((last, last_style)) if last.end == start && *last_style == style => last.end = end,
            _ => result.push((start..end, style)),
        }
    }
    result
}

/// A line follows the token's color, as CSS `currentColor` does.
fn merge_styles(base: HighlightStyle, semantic: HighlightStyle) -> HighlightStyle {
    let color = semantic.color.or(base.color);
    let line_color = |base_color: Option<Hsla>| if semantic.color.is_some() { color } else { base_color };
    let underline = match semantic.underline {
        Some(underline) if underline.thickness == px(0.) => None,
        Some(underline) => Some(UnderlineStyle { color, ..underline }),
        None => base.underline.map(|underline| UnderlineStyle { color: line_color(underline.color), ..underline }),
    };
    let strikethrough = match semantic.strikethrough {
        Some(strikethrough) if strikethrough.thickness == px(0.) => None,
        Some(strikethrough) => Some(StrikethroughStyle { color, ..strikethrough }),
        None => base.strikethrough.map(|strikethrough| StrikethroughStyle { color: line_color(strikethrough.color), ..strikethrough }),
    };
    HighlightStyle {
        color,
        font_weight: semantic.font_weight.or(base.font_weight),
        font_style: semantic.font_style.or(base.font_style),
        underline,
        strikethrough,
        ..base
    }
}

#[cfg(test)]
mod tests;
