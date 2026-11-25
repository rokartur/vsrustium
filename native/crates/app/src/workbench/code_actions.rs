//! codeAction.ts, codeActionModel.ts, codeActionController.ts, codeActionCommands.ts, codeActionMenu.ts,
//! codeActionKeybindingResolver.ts and lightBulbWidget.ts: the providers' code actions for the selection, asked
//! 250ms after the cursor or the markers change and shown as a light bulb, or asked by Cmd+. and the code action
//! commands and shown in the action widget (`action_widget.rs`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::channel::oneshot;
use gpui::{AnyElement, AnyWindowHandle, Bounds, Context, Div, MouseButton, MouseDownEvent, Task, Window, div, point, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::action_widget::Anchor;
use super::editor_message::MessagePart;
use crate::action_list::{Item, ItemKind};
use crate::commands::Outcome;
use crate::editor::{OpenEditor, Position, Selection, char_column, utf16_column, visible_column};
use crate::language_features::Candidate;
use crate::markers::{Marker, ReadOptions};

const REGISTER: &str = "$registerCodeActionSupport";
/// `CodeActionTriggerType`.
const INVOKE: u64 = 1;
const AUTO: u64 = 2;
/// `CodeActionOracle._delay`.
const AUTO_TRIGGER_DELAY: Duration = Duration::from_millis(250);
/// codeActionModel.ts: an automatic request replacing a manual one at another position waits this long.
const MANUAL_TO_AUTO_DELAY: Duration = Duration::from_millis(500);
/// `APPLY_FIX_ALL_COMMAND_ID` of the TypeScript extension.
const APPLY_FIX_ALL_COMMAND_ID: &str = "_typescript.applyFixAllCodeAction";
const QUICK_FIX: &str = "quickfix";
const SOURCE: &str = "source";
/// `HierarchicalKind.None`, which contains no kind.
const NONE_KIND: &str = "@@none@@";

#[derive(Default)]
pub(super) struct CodeActions {
    auto_trigger: Option<Task<()>>,
    request: Option<Request>,
    /// A manual request an automatic one replaces stays until the automatic one applies.
    superseded: Option<Request>,
    requests: usize,
    /// `_activeCodeActions`.
    pub(super) active: Option<Rc<CodeActionSet>>,
    pub(super) lightbulb: Option<LightBulb>,
    pub(super) marker_fixes: MarkerQuickFixes,
}

/// markerHoverParticipant.ts: the quick fixes of the marker the hover shows, and whether the last marker asked
/// had any (`recentMarkerCodeActionsInfo`).
#[derive(Default)]
pub(super) struct MarkerQuickFixes {
    marker: Option<Arc<Marker>>,
    answered: bool,
    set: Option<Rc<CodeActionSet>>,
    /// "Checking for quick fixes..." shows after 200ms.
    checking: bool,
    recent: Option<(Arc<Marker>, bool)>,
    tasks: Vec<Task<()>>,
    /// The "Quick Fix..." label's window bounds, which the widget opens under.
    pub(super) label_bounds: Rc<Cell<Bounds<f32>>>,
}

/// What the marker hover's status bar shows for quick fixes.
pub(super) enum QuickFixStatus {
    Text(&'static str),
    QuickFix,
}

struct Request {
    id: usize,
    kind: u64,
    position: Position,
    _task: Task<()>,
    /// The lists the providers answered so far, released when the request is dropped unfinished.
    caches: Rc<RefCell<Vec<(u64, u64)>>>,
}

/// `CodeActionTriggerSource`, the ones that change what runs.
#[derive(Clone, Copy, PartialEq)]
enum Source {
    Default,
    QuickFix,
}

/// `CodeActionAutoApply`.
#[derive(Clone, Copy, PartialEq)]
enum AutoApply {
    IfSingle,
    First,
    Never,
}

/// `CodeActionFilter`.
#[derive(Clone, Default)]
struct Filter {
    include: Option<String>,
    excludes: Vec<String>,
    include_source_actions: bool,
    only_preferred: bool,
}

#[derive(Clone)]
struct Trigger {
    kind: u64,
    source: Source,
    filter: Filter,
    auto_apply: Option<AutoApply>,
    /// The message when nothing is available, for a manual trigger.
    not_available: Option<String>,
    /// The window a manual trigger came from, which applying an action runs its command in.
    window: Option<AnyWindowHandle>,
}

#[derive(Clone)]
pub(super) struct CodeActionItem {
    pub(super) action: Value,
    handle: u64,
}

/// `CodeActionSet`: the actions sorted, the valid ones without `disabled`, and the lists to release.
pub(super) struct CodeActionSet {
    pub(super) all: Vec<CodeActionItem>,
    pub(super) valid: Vec<CodeActionItem>,
    pub(super) documentation: Vec<Value>,
    pub(super) has_auto_fix: bool,
    has_ai_fix: bool,
    all_ai_fixes: bool,
    caches: Vec<(u64, u64)>,
}

/// The light bulb content widget at a line, or the gutter light bulb on one.
pub(super) struct LightBulb {
    model_id: u32,
    line: usize,
    /// The 0-based column the content widget sits at; `None` in the gutter.
    column: Option<usize>,
    set: Rc<CodeActionSet>,
    icon: &'static str,
    color: &'static str,
    title: String,
}

/// One `toMenuItems` row's look: its group's icon and color, and the keybinding of the action's kind.
pub(super) struct RowLook {
    pub(super) icon: &'static str,
    pub(super) color: Option<&'static str>,
    pub(super) keybinding: Option<String>,
}

impl Workbench {
    pub(super) fn register_code_actions(&mut self) {
        self.commands.register("editor.action.quickFix", |workbench, _, window, cx| {
            workbench.manual_trigger_code_actions("No code actions available", Source::QuickFix, None, None, window, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.codeAction", |workbench, args, window, cx| {
            let (kind, apply, preferred) = command_args(args.first(), "", AutoApply::IfSingle);
            let message = none_message(args.first(), preferred, "code actions");
            let filter = Filter { include: Some(kind), include_source_actions: true, only_preferred: preferred, ..Default::default() };
            workbench.manual_trigger_code_actions(&message, Source::Default, Some(filter), Some(apply), window, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.refactor", |workbench, args, window, cx| {
            let (kind, apply, preferred) = command_args(args.first(), "refactor", AutoApply::Never);
            let message = none_message(args.first(), preferred, "refactorings");
            let include = if contains("refactor", &kind) { kind } else { NONE_KIND.to_owned() };
            let filter = Filter { include: Some(include), only_preferred: preferred, ..Default::default() };
            workbench.manual_trigger_code_actions(&message, Source::Default, Some(filter), Some(apply), window, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.sourceAction", |workbench, args, window, cx| {
            let (kind, apply, preferred) = command_args(args.first(), SOURCE, AutoApply::Never);
            let message = none_message(args.first(), preferred, "source actions");
            let include = if contains(SOURCE, &kind) { kind } else { NONE_KIND.to_owned() };
            let filter = Filter { include: Some(include), include_source_actions: true, only_preferred: preferred, ..Default::default() };
            workbench.manual_trigger_code_actions(&message, Source::Default, Some(filter), Some(apply), window, cx);
            Outcome::done()
        });
        let fixed = [
            ("editor.action.organizeImports", "source.organizeImports", "No organize imports action available", true, false),
            ("editor.action.fixAll", "source.fixAll", "No fix all action available", true, false),
            ("editor.action.autoFix", QUICK_FIX, "No auto fixes available", false, true),
        ];
        for (id, kind, message, include_source_actions, only_preferred) in fixed {
            self.commands.register(id, move |workbench, _, window, cx| {
                let filter = Filter { include: Some(kind.to_owned()), include_source_actions, only_preferred, ..Default::default() };
                workbench.manual_trigger_code_actions(message, Source::Default, Some(filter), Some(AutoApply::IfSingle), window, cx);
                Outcome::done()
            });
        }
        self.commands.register("_executeCodeActionProvider", |workbench, args, _, cx| workbench.execute_code_action_provider(&args, cx));
        self.register_action_widget_commands();
        self.register_paste_edits();
    }

    /// `editorHasCodeActionsProvider` and `supportedCodeAction` of the active editor.
    pub(super) fn code_actions_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        let Some(editor) = self.editors.active() else { return };
        let uri = editor.document().uri();
        let language = editor.document().language_id.clone();
        let candidate = Candidate { uri: &uri, language: &language };
        let providers = self.language_features.ordered(REGISTER, &candidate);
        let snippets = self.snippet_code_actions_enabled(editor);
        context.insert("editorHasCodeActionsProvider".into(), json!(!providers.is_empty() || snippets));
        if self.active_editor_readonly().is_none() {
            let mut kinds: Vec<&str> = providers.iter().flat_map(|registration| provided_kinds(&registration.args)).collect();
            // `FileTemplateCodeActionProvider.providedCodeActionKinds`.
            if snippets {
                kinds.push("refactor.surround");
            }
            context.insert("supportedCodeAction".into(), json!(kinds.join(" ")));
        }
    }

    /// A registry's `onDidChange`: the code action model and the code lenses update when their providers change.
    pub(crate) fn language_providers_changed(&mut self, method: &str, cx: &mut Context<Self>) {
        match method {
            REGISTER => {
                self.code_actions_on_save_kinds_changed();
                self.code_actions_model_changed(cx);
            }
            "$registerCodeLensSupport" => self.code_lens_registrations_changed(cx),
            super::document_symbols::REGISTER => self.outline_providers_changed(cx),
            "$registerInlayHintsProvider" => self.inlay_hints_registrations_changed(cx),
            "$registerDocumentColorProvider" => self.color_providers_changed(cx),
            "$registerFoldingRangeProvider" => self.trigger_folding_model_changed(cx),
            "$registerDocumentLinkProvider" => self.links_model_changed(cx),
            "$registerLinkedEditingRangeProvider" => self.linked_editing_reinitialize(false, cx),
            "$registerDocumentSemanticTokensProvider" | "$registerDocumentRangeSemanticTokensProvider" => self.semantic_tokens_providers_changed(cx),
            "$registerDocumentFormattingSupport" | "$registerRangeFormattingSupport" => {
                self.formatters_changed();
                self.refresh_language_status(cx);
            }
            _ => {}
        }
    }

    /// `CodeActionsContribution.updateConfigurationSchema` on a provider change.
    fn code_actions_on_save_kinds_changed(&mut self) {
        let registrations: Vec<_> = self.language_features.all(REGISTER).collect();
        let source_kinds: Vec<&str> = registrations.iter().flat_map(|registration| provided_kinds(&registration.args)).filter(|kind| contains(SOURCE, kind)).collect();
        self.settings.lock().unwrap().registry.list_code_actions_on_save(&source_kinds);
    }

    /// `CodeActionModel._update` on another model: the state empties and an automatic request runs at once.
    pub(super) fn code_actions_model_changed(&mut self, cx: &mut Context<Self>) {
        self.set_code_actions_state_empty();
        self.hide_action_widget(cx);
        self.code_actions.auto_trigger = None;
        self.trigger_code_actions(auto_trigger(), cx);
    }

    /// `onDidChangeCursorPosition`: an automatic request 250ms after the last move. The light bulb goes when its line
    /// is removed (`onDidChangeModelContent`).
    // shortcut: the action widget does not take the focus, so a cursor move or an edit closes it; move the focus to
    // the widget's list once GPUI's editor input can be routed past a focused overlay.
    pub(super) fn code_actions_cursor_changed(&mut self, content_changed: bool, cx: &mut Context<Self>) {
        self.hide_action_widget(cx);
        if content_changed
            && let Some(lightbulb) = &self.code_actions.lightbulb
            && let Some(editor) = self.editors.active()
            && lightbulb.line + 1 >= editor.document().lines.len()
        {
            self.hide_lightbulb();
        }
        self.try_auto_trigger_code_actions(cx);
    }

    /// `_onMarkerChanges`: markers of the active model ask again.
    pub(super) fn code_actions_markers_changed(&mut self, resources: &[String], cx: &mut Context<Self>) {
        if self.editors.active().is_some_and(|editor| resources.contains(&editor.document().uri_string())) {
            self.try_auto_trigger_code_actions(cx);
        }
    }

    /// The editor lost the focus: the widget closes (`onDidBlur` of its list).
    pub(super) fn code_actions_editor_blurred(&mut self, cx: &mut Context<Self>) {
        self.hide_action_widget(cx);
    }

    fn try_auto_trigger_code_actions(&mut self, cx: &mut Context<Self>) {
        self.code_actions.auto_trigger = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(AUTO_TRIGGER_DELAY).await;
            let _ = workbench.update(cx, |workbench, cx| workbench.trigger_code_actions(auto_trigger(), cx));
        }));
    }

    /// `manualTriggerAtCurrentPosition`.
    fn manual_trigger_code_actions(&mut self, message: &str, source: Source, filter: Option<Filter>, auto_apply: Option<AutoApply>, window: &mut Window, cx: &mut Context<Self>) {
        if self.editors.active().is_none() {
            return;
        }
        self.close_editor_message(cx);
        let trigger = Trigger { kind: INVOKE, source, filter: filter.unwrap_or_default(), auto_apply, not_available: Some(message.to_owned()), window: Some(window.window_handle()) };
        self.trigger_code_actions(trigger, cx);
    }

    /// `CodeActionOracle.trigger` and the model's handler: the selection, unless the light bulb settings rule it
    /// out, goes to the providers.
    fn trigger_code_actions(&mut self, trigger: Trigger, cx: &mut Context<Self>) {
        self.refresh_snippet_files();
        let Some(editor) = self.editors.active() else { return };
        let uri = editor.document().uri();
        let language = editor.document().language_id.clone();
        let candidate = Candidate { uri: &uri, language: &language };
        let no_providers = self.language_features.ordered(REGISTER, &candidate).is_empty() && !self.snippet_code_actions_enabled(editor);
        if no_providers || self.active_editor_readonly().is_some() {
            return;
        }
        let lightbulb = self.suggest_setting(editor, "editor.lightbulb.enabled");
        let Some(selection) = selection_unless_whitespace_enclosed(editor, &trigger, lightbulb.as_str().unwrap_or("onCode")) else {
            return self.set_code_actions_state_empty();
        };
        let start = selection.start();
        let nearby = self.suggest_setting(editor, "editor.codeActionWidget.includeNearbyQuickFixes") == json!(true)
            && trigger.kind == INVOKE
            && (trigger.source == Source::QuickFix || trigger.filter.include.as_deref().is_some_and(|include| contains(include, QUICK_FIX)));
        let markers: Vec<Value> = match nearby {
            true => self.diagnostics.service.read(ReadOptions { resource: Some(&editor.document().uri_string()), ..Default::default() }).iter().map(|marker| marker.to_data()).collect(),
            false => Vec::new(),
        };
        let head = selection.head;
        let cursor = (head.line + 1, utf16_column(&editor.document().lines[head.line], head.column) + 1);
        let calls = self.code_action_calls(editor, selection_value(editor, selection), &trigger);
        let caches = Rc::new(RefCell::new(Vec::new()));
        // Do not replace a manual request's state with an automatic one at another position at once.
        let deferred = self.code_actions.request.as_ref().is_some_and(|current| current.kind == INVOKE && trigger.kind == AUTO && current.position != start);
        self.code_actions.requests += 1;
        let id = self.code_actions.requests;
        let started = Instant::now();
        let task = cx.spawn({
            let (caches, trigger) = (caches.clone(), trigger.clone());
            async move |workbench, cx| {
                let mut set = collect_code_actions(calls, &trigger.filter, &caches).await;
                if nearby {
                    set = nearby_quick_fixes(set, &workbench, cx, &trigger, cursor, &markers, &caches).await;
                }
                if deferred {
                    cx.background_executor().timer(MANUAL_TO_AUTO_DELAY.saturating_sub(started.elapsed())).await;
                }
                caches.borrow_mut().clear();
                let _ = workbench.update(cx, |workbench, cx| workbench.code_actions_ready(id, trigger, start, set, cx));
            }
        });
        let request = Request { id, kind: trigger.kind, position: start, _task: task, caches };
        match deferred {
            true => {
                if let Some(old) = self.code_actions.superseded.replace(self.code_actions.request.take().expect("deferred replaces a request")) {
                    self.drop_code_action_request(old);
                }
            }
            false => {
                if let Some(old) = self.code_actions.request.take() {
                    self.drop_code_action_request(old);
                }
            }
        }
        self.code_actions.request = Some(request);
    }

    /// `getCodeActionProviders` and the `$provideCodeActions` calls of the model's providers that may answer the
    /// filter's kinds; an automatic request leaves out notebook providers.
    fn code_action_calls(&self, editor: &OpenEditor, range: Value, trigger: &Trigger) -> Vec<(u64, oneshot::Receiver<Result<Value, Value>>, Value)> {
        let uri = editor.document().uri();
        let language = editor.document().language_id.clone();
        let candidate = Candidate { uri: &uri, language: &language };
        let mut provider_filter = trigger.filter.clone();
        if trigger.kind == AUTO {
            provider_filter.excludes.push("notebook".to_owned());
        }
        let context = json!({ "only": trigger.filter.include, "trigger": trigger.kind });
        let mut calls = Vec::new();
        for registration in self.language_features.ordered(REGISTER, &candidate) {
            let kinds = provided_kinds(&registration.args);
            if registration.args.get(2).is_some_and(|metadata| metadata["providedKinds"].is_array()) && !kinds.iter().any(|kind| may_include_kind(&provider_filter, kind)) {
                continue;
            }
            let args = vec![json!(registration.handle), uri.clone(), range.clone(), context.clone()];
            let call = self.host.call_cancellable("ExtHostLanguageFeatures", "$provideCodeActions", args);
            let documentation = registration.args.get(2).map(|metadata| metadata["documentation"].clone()).unwrap_or_default();
            calls.push((registration.handle, call, documentation));
        }
        // snippetCodeActionProvider.ts registers its two providers for `*` at startup, so they come last.
        for (handle, list) in self.snippet_code_actions(editor, &range) {
            let (sender, receiver) = oneshot::channel();
            let _ = sender.send(Ok(list));
            calls.push((handle, receiver, Value::Null));
        }
        calls
    }

    /// The controller's `update` of a finished request: the light bulb, then for a manual trigger the auto apply,
    /// the not available message or the widget.
    fn code_actions_ready(&mut self, id: usize, trigger: Trigger, position: Position, set: CodeActionSet, cx: &mut Context<Self>) {
        if self.code_actions.request.as_ref().is_some_and(|request| request.id == id) {
            self.code_actions.request = None;
            if let Some(old) = self.code_actions.superseded.take() {
                self.drop_code_action_request(old);
            }
        } else if self.code_actions.superseded.as_ref().is_some_and(|request| request.id == id) {
            self.code_actions.superseded = None;
        }
        let set = Rc::new(set);
        let Some(editor) = self.editors.active() else { return self.release_code_action_set(set) };
        if editor.selection().start().line != position.line {
            return self.release_code_action_set(set);
        }
        self.update_lightbulb(set.clone(), position);
        if trigger.kind != INVOKE {
            match self.action_widget_visible() {
                true => self.release_code_action_set(set),
                false => self.set_active_code_actions(Some(set)),
            }
            return cx.notify();
        }
        if trigger.filter.include.is_some() {
            if let Some(item) = valid_action_to_apply(&trigger, &set) {
                self.hide_lightbulb();
                let window = trigger.window.expect("manual triggers carry their window");
                self.apply_code_action(item, set, false, window, cx).detach();
                return cx.notify();
            }
            if trigger.not_available.is_some()
                && let Some(disabled) = invalid_action_that_would_have_been_applied(&trigger, &set)
            {
                self.show_editor_message(vec![MessagePart::text(disabled)], cx);
                return self.release_code_action_set(set);
            }
        }
        let include_disabled = trigger.filter.include.is_some();
        if let Some(message) = &trigger.not_available
            && (set.all.is_empty() || !include_disabled && set.valid.is_empty())
        {
            self.show_editor_message(vec![MessagePart::text(message.clone())], cx);
            self.set_active_code_actions(None);
            return self.release_code_action_set(set);
        }
        self.set_active_code_actions(Some(set.clone()));
        self.show_code_action_list(set, Anchor::Position(position), include_disabled, false, cx);
    }

    /// `applyCodeAction`: the action resolves when it has no edit, its text edits apply, then its command runs; a
    /// failed command is an error notification. With `retrigger` an automatic request follows.
    // shortcut: only the text edits of a code action's workspace edit apply; file and cell edits need the bulk
    // edit service on the workbench side (they apply through `$tryApplyWorkspaceEdit` on the host thread today).
    pub(super) fn apply_code_action(&mut self, item: CodeActionItem, set: Rc<CodeActionSet>, retrigger: bool, window: AnyWindowHandle, cx: &mut Context<Self>) -> Task<()> {
        let supports_resolve = self.language_features.registration(item.handle).is_some_and(|registration| registration.args.get(5) == Some(&json!(true)));
        let resolve = match supports_resolve && item.action["edit"].is_null() && item.action["cacheId"].is_array() {
            true => Some(self.host.call("ExtHostLanguageFeatures", "$resolveCodeAction", vec![json!(item.handle), item.action["cacheId"].clone()])),
            false => None,
        };
        cx.spawn(async move |workbench, cx| {
            let mut action = item.action;
            if let Some(resolve) = resolve
                && let Ok(Ok(resolved)) = resolve.await
            {
                if !resolved["edit"].is_null() {
                    action["edit"] = resolved["edit"].clone();
                }
                if !resolved["command"].is_null() {
                    action["command"] = resolved["command"].clone();
                }
            }
            let command = window.update(cx, |_, window, cx| {
                workbench.update(cx, |workbench, cx| {
                    let text_edits: Vec<Value> = action["edit"]["edits"].as_array().into_iter().flatten().filter(|edit| edit.get("textEdit").is_some()).cloned().collect();
                    if !text_edits.is_empty() && workbench.apply_workspace_text_edits(&Value::Array(text_edits), &Value::Null, cx).is_err() {
                        return None;
                    }
                    let command = action["command"]["id"].as_str()?;
                    let args = action["command"]["arguments"].as_array().cloned().unwrap_or_default();
                    Some(workbench.execute_command(command, args, window, cx))
                })
            });
            let result = match command {
                Ok(Ok(Some(Outcome::Ready(result)))) => result,
                Ok(Ok(Some(Outcome::Pending(pending)))) => pending.await.unwrap_or(Ok(Value::Null)),
                _ => Ok(Value::Null),
            };
            let _ = workbench.update(cx, |workbench, cx| {
                if let Err(error) = result {
                    let message = error["message"].as_str().or(error.as_str()).unwrap_or("An unknown error occurred while applying the code action");
                    workbench.notify_error(message.to_owned());
                }
                workbench.release_code_action_set(set);
                if retrigger {
                    let trigger = Trigger { source: Source::QuickFix, ..auto_trigger() };
                    workbench.trigger_code_actions(trigger, cx);
                }
                cx.notify();
            });
        })
    }

    /// An action picked in the widget: it applies and an automatic request follows.
    pub(super) fn accept_code_action(&mut self, index: usize, set: Rc<CodeActionSet>, window: &mut Window, cx: &mut Context<Self>) {
        let item = set.all.iter().chain(&set.valid).nth(index).cloned();
        let Some(item) = item else { return };
        window.focus(&self.editor_focus, cx);
        self.apply_code_action(item, set, true, window.window_handle(), cx).detach();
    }

    /// `_executeCodeActionProvider`: the valid actions for a range of a model, as an Invoke with source actions,
    /// the first `itemResolveCount` resolved.
    fn execute_code_action_provider(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = super::host_requests::uri_parts(&args.first().cloned().unwrap_or_default()).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path)) else {
            return Outcome::Ready(Err(crate::commands::error_value("Illegal argument")));
        };
        let range = args.get(1).cloned().unwrap_or_default();
        let filter = Filter { include: args.get(2).and_then(Value::as_str).map(String::from), include_source_actions: true, ..Default::default() };
        let trigger = Trigger { kind: INVOKE, source: Source::Default, filter: filter.clone(), auto_apply: None, not_available: None, window: None };
        let calls = self.code_action_calls(editor, range, &trigger);
        let resolve_count = args.get(3).and_then(Value::as_u64).unwrap_or(0) as usize;
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let caches = Rc::new(RefCell::new(Vec::new()));
            let set = collect_code_actions(calls, &filter, &caches).await;
            let resolving = workbench.update(cx, |workbench, _| {
                let mut resolving = Vec::new();
                for (index, item) in set.valid.iter().enumerate().take(resolve_count) {
                    let supports_resolve = workbench.language_features.registration(item.handle).is_some_and(|registration| registration.args.get(5) == Some(&json!(true)));
                    if supports_resolve && item.action["edit"].is_null() {
                        resolving.push((index, workbench.host.call("ExtHostLanguageFeatures", "$resolveCodeAction", vec![json!(item.handle), item.action["cacheId"].clone()])));
                    }
                }
                resolving
            });
            let mut actions: Vec<Value> = set.valid.iter().map(|item| item.action.clone()).collect();
            for (index, call) in resolving.unwrap_or_default() {
                if let Ok(Ok(resolved)) = call.await
                    && !resolved["edit"].is_null()
                {
                    actions[index]["edit"] = resolved["edit"].clone();
                }
            }
            let _ = sender.send(Ok(Value::Array(actions)));
            cx.background_executor().timer(Duration::from_millis(100)).await;
            let _ = workbench.update(cx, |workbench, _| workbench.release_code_action_set(Rc::new(set)));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    fn set_code_actions_state_empty(&mut self) {
        if let Some(request) = self.code_actions.request.take() {
            self.drop_code_action_request(request);
        }
        if let Some(request) = self.code_actions.superseded.take() {
            self.drop_code_action_request(request);
        }
        self.hide_lightbulb();
    }

    fn drop_code_action_request(&self, request: Request) {
        for (handle, cache_id) in request.caches.borrow_mut().drain(..) {
            self.host.send("ExtHostLanguageFeatures", "$releaseCodeActions", vec![json!(handle), json!(cache_id)]);
        }
    }

    fn set_active_code_actions(&mut self, set: Option<Rc<CodeActionSet>>) {
        if let Some(old) = std::mem::replace(&mut self.code_actions.active, set) {
            self.release_code_action_set(old);
        }
    }

    /// `dispose` of a set once nothing shows it: the providers' lists are released.
    pub(super) fn release_code_action_set(&self, set: Rc<CodeActionSet>) {
        if let Ok(set) = Rc::try_unwrap(set) {
            for (handle, cache_id) in set.caches {
                self.host.send("ExtHostLanguageFeatures", "$releaseCodeActions", vec![json!(handle), json!(cache_id)]);
            }
        }
    }

    fn hide_lightbulb(&mut self) {
        if let Some(lightbulb) = self.code_actions.lightbulb.take() {
            self.release_code_action_set(lightbulb.set);
        }
    }

    /// `LightBulbWidget.update`: the light bulb at the start of the line when its indentation leaves room, else on
    /// the line above or below with room, else in the gutter of the selection's line.
    fn update_lightbulb(&mut self, set: Rc<CodeActionSet>, position: Position) {
        self.hide_lightbulb();
        let editor = self.editors.active().expect("light bulbs show in the active editor");
        if set.valid.is_empty() || self.suggest_setting(editor, "editor.lightbulb.enabled") == json!("off") {
            return self.release_code_action_set(set);
        }
        let lines = &editor.document().lines;
        let tab_size = editor.document().indentation.tab_size;
        let space_width = self.editors.viewport.char_width;
        let line = position.line.min(lines.len() - 1);
        let indent = compute_indent_level(&lines[line], tab_size);
        let line_has_space = space_width * indent as f32 > 22.;
        // `isFolded` of a 1-based line number: it shares its top with the line above.
        let is_folded = |number: usize| number > 2 && editor.view_line(number - 1) == editor.view_line(number - 2);
        let empty_or_indented = |number: usize| {
            let text = &lines[number - 1];
            text.starts_with(char::is_whitespace) || text.encode_utf16().count() <= 1
        };
        let number = line + 1;
        let count = lines.len();
        let mut effective = number;
        let mut gutter = false;
        if !line_has_space {
            if number > 1 && !is_folded(number - 1) {
                let end_line = number == count;
                let previous = empty_or_indented(number - 1);
                let next = !end_line && empty_or_indented(number + 1);
                let current = empty_or_indented(number);
                if !next && !previous {
                    gutter = true;
                } else if previous || end_line {
                    effective -= 1;
                } else if next || current {
                    effective += 1;
                }
            } else if number == 1 && (number == count || !empty_or_indented(number + 1) && !empty_or_indented(number)) {
                gutter = true;
            } else if number < count && !is_folded(number + 1) {
                effective += 1;
            } else if (utf16_column(&lines[line], position.column) + 1) as f32 * space_width < 22. {
                return self.release_code_action_set(set);
            }
        }
        let (icon, color, title) = self.lightbulb_info(&set);
        let (line, column) = match gutter {
            true => (editor.selection().start().line, None),
            false => {
                let text = &lines[effective - 1];
                // `/^\S\s*$/`: a lone character with trailing whitespace gets the light bulb after it.
                let mut chars = text.chars();
                let lone = chars.next().is_some_and(|first| !first.is_whitespace()) && chars.all(char::is_whitespace);
                (effective - 1, Some(if line_has_space { 0 } else { usize::from(lone) }))
            }
        };
        self.code_actions.lightbulb = Some(LightBulb { model_id: editor.model_id, line, column, set, icon, color, title });
    }

    /// `computeLightBulbInfo`: the icon for AI and preferred quick fixes and the title with the keybinding.
    /// The gutter variants (`gutter-lightbulb*`) are the same codicons.
    fn lightbulb_info(&self, set: &CodeActionSet) -> (&'static str, &'static str, String) {
        let auto_run = set.all_ai_fixes && set.valid.len() == 1;
        let (icon, color) = if set.all_ai_fixes {
            ("sparkle-filled", "editorLightBulbAi.foreground")
        } else if set.has_auto_fix {
            ("lightbulb-autofix", "editorLightBulbAutoFix.foreground")
        } else if set.has_ai_fix {
            ("lightbulb-sparkle", "editorLightBulb.foreground")
        } else {
            ("lightbulb", "editorLightBulb.foreground")
        };
        let label = |command: &str| self.keybindings.resolver.lookup_primary_keybinding(command, &self.context_keys).map(|item| crate::keybindings::label(&item.keybinding));
        let title = if auto_run {
            format!("Run: {}", set.valid[0].action["title"].as_str().unwrap_or_default())
        } else if let Some(preferred) = label("editor.action.autoFix").filter(|_| set.has_auto_fix) {
            format!("Show Code Actions. Preferred Quick Fix Available ({preferred})")
        } else if let Some(quick_fix) = label("editor.action.quickFix").filter(|_| !set.has_auto_fix) {
            format!("Show Code Actions ({quick_fix})")
        } else {
            "Show Code Actions".to_owned()
        };
        (icon, color, title)
    }

    /// The content widget light bulb: a 16px icon at the top of its line and column. The content left is kept for
    /// the action widget's anchors at a model position.
    pub(super) fn lightbulb_widget(&self, editor: &OpenEditor, content_left: f32, char_width: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.action_widget.content_left.set(content_left);
        let lightbulb = self.code_actions.lightbulb.as_ref().filter(|lightbulb| lightbulb.model_id == editor.model_id)?;
        let column = lightbulb.column?;
        let text = &editor.document().lines[lightbulb.line];
        let left = visible_column(text, column, editor.document().indentation.tab_size) as f32 * char_width - editor.scroll.left + content_left;
        let top = editor.line_top(editor.view_line(lightbulb.line), self.editors.viewport) - editor.scroll.top;
        let line_height = self.editors.viewport.line_height;
        let icon = self.icon(lightbulb.icon, 16., 16., self.color(lightbulb.color));
        let widget = div()
            .id("lightbulb-widget")
            .absolute()
            .left(px(left))
            .top(px(top))
            .cursor_pointer()
            .child(icon)
            // The DOM `title`, shown as the workbench hover: GPUI has no native tooltip.
            .child(self.hover_target("lightbulb-title", super::hover::HoverContent::Text(lightbulb.title.clone()), super::hover::HoverOptions::mouse()))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                window.focus(&workbench.editor_focus, cx);
                // The menu opens a third of a line below the light bulb, a line lower when it sits above the cursor.
                let mut pad = (line_height / 3.).floor();
                if workbench.code_actions.lightbulb.as_ref().is_some_and(|lightbulb| editor_line_above_cursor(workbench, lightbulb.line)) {
                    pad += line_height;
                }
                let y = workbench.text_area_bounds.get().origin.y + top + 16. + pad;
                workbench.show_code_actions_from_lightbulb(point(f32::from(event.position.x), y), window, cx);
            }));
        Some(widget.into_any_element())
    }

    /// The gutter light bulb in the glyph margin of model line `index`.
    pub(super) fn lightbulb_glyph(&self, editor: &OpenEditor, index: usize, line_height: f32, cx: &mut Context<Self>) -> Option<Div> {
        let lightbulb = self.code_actions.lightbulb.as_ref().filter(|lightbulb| lightbulb.model_id == editor.model_id && lightbulb.column.is_none() && lightbulb.line == index)?;
        let icon = self.icon(lightbulb.icon, line_height, 16., self.color(lightbulb.color));
        Some(icon.cursor_pointer().on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
            cx.stop_propagation();
            window.focus(&workbench.editor_focus, cx);
            let pad = (line_height / 3.).floor();
            let editor = workbench.editors.active().expect("the gutter shows the active editor");
            let top = editor.line_top(editor.view_line(index), workbench.editors.viewport) - editor.scroll.top + workbench.text_area_bounds.get().origin.y;
            workbench.show_code_actions_from_lightbulb(point(f32::from(event.position.x), top + line_height + pad), window, cx);
        })))
    }

    /// `showCodeActionsFromLightbulb`: a lone AI fix runs at once, else the widget opens at the point.
    fn show_code_actions_from_lightbulb(&mut self, at: gpui::Point<f32>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lightbulb) = &self.code_actions.lightbulb else { return };
        let set = lightbulb.set.clone();
        if set.all_ai_fixes && set.valid.len() == 1 {
            let item = set.valid[0].clone();
            return self.apply_code_action(item, set, true, window.window_handle(), cx).detach();
        }
        self.show_code_action_list(set, Anchor::Point(at), false, true, cx);
    }

    /// `showCodeActionList`: the valid actions, or every action when disabled ones are asked for and shown.
    pub(super) fn show_code_action_list(&mut self, set: Rc<CodeActionSet>, anchor: Anchor, include_disabled: bool, from_lightbulb: bool, cx: &mut Context<Self>) {
        let show_disabled = self.action_widget_shows_disabled();
        let all = include_disabled && (show_disabled || set.valid.is_empty());
        let shown: Vec<(usize, &CodeActionItem)> = match all {
            true => set.all.iter().enumerate().collect(),
            false => set.valid.iter().enumerate().map(|(index, item)| (set.all.len() + index, item)).collect(),
        };
        if shown.is_empty() {
            return;
        }
        let editor = self.editors.active().expect("code actions show over the active editor");
        let show_headers = self.suggest_setting(editor, "editor.codeActionWidget.showHeaders") != json!(false);
        let (items, looks) = self.code_action_menu_items(&shown, show_headers);
        let mut bar = Vec::new();
        if !from_lightbulb {
            bar.extend(set.documentation.iter().map(|command| (command["title"].as_str().unwrap_or_default().to_owned(), Some(command.clone()))));
            if include_disabled && !set.valid.is_empty() && set.all.len() != set.valid.len() {
                bar.push((if show_disabled { "Hide Disabled" } else { "Show Disabled" }.to_owned(), None));
            }
        }
        self.show_action_widget(items, looks, anchor, bar, super::action_widget::Owner::CodeActions { set, include_disabled, from_lightbulb }, cx);
    }

    /// `toMenuItems`: the actions under their group's header, each group once in `codeActionGroups` order.
    fn code_action_menu_items(&self, shown: &[(usize, &CodeActionItem)], show_headers: bool) -> (Vec<Item<usize>>, Vec<RowLook>) {
        let resolver = self.code_action_keybindings();
        let look = |item: &CodeActionItem, group: &(&str, &str, Option<&'static str>)| RowLook {
            icon: if item.action["isAI"] == true { "sparkle" } else { group.2.unwrap_or("lightbulb") },
            color: (group.2.is_none() && item.action["isAI"] != true).then_some("editorLightBulb.foreground"),
            keybinding: keybinding_for_action(&resolver, &item.action),
        };
        let mut items = Vec::new();
        let mut looks = Vec::new();
        if !show_headers {
            for (index, item) in shown {
                let disabled = item.action["disabled"].as_str();
                let label = disabled.unwrap_or(item.action["title"].as_str().unwrap_or_default());
                items.push(Item { disabled: disabled.is_some(), ..Item::action(*index, label) });
                looks.push(RowLook { keybinding: None, ..look(item, &UNCATEGORIZED) });
            }
            return (items, looks);
        }
        let mut groups: Vec<Vec<&(usize, &CodeActionItem)>> = vec![Vec::new(); GROUPS.len()];
        for entry in shown {
            let kind = entry.1.action["kind"].as_str().unwrap_or(NONE_KIND);
            if let Some(group) = GROUPS.iter().position(|group| contains(group.0, kind)) {
                groups[group].push(entry);
            }
        }
        for (group, entries) in GROUPS.iter().zip(groups) {
            if entries.is_empty() {
                continue;
            }
            items.push(Item { label: Some(group.1.to_owned()), ..Item::new(ItemKind::Header) });
            looks.push(RowLook { icon: "", color: None, keybinding: None });
            for (index, item) in entries {
                let title = item.action["title"].as_str().unwrap_or_default();
                items.push(Item { disabled: !item.action["disabled"].is_null(), ..Item::action(*index, title) });
                looks.push(look(item, group));
            }
        }
        (items, looks)
    }

    /// `CodeActionKeybindingResolver`: the keybindings of the code action commands, with the kind they run.
    fn code_action_keybindings(&self) -> Vec<(String, bool, String)> {
        let mut bindings = Vec::new();
        for item in self.keybindings.resolver.keybindings() {
            let args = match item.command.as_str() {
                "editor.action.organizeImports" => Some(json!({ "kind": "source.organizeImports" })),
                "editor.action.fixAll" => Some(json!({ "kind": "source.fixAll" })),
                "editor.action.refactor" | "editor.action.codeAction" | "editor.action.sourceAction" => item.args.clone(),
                _ => continue,
            };
            if item.keybinding.is_empty() {
                continue;
            }
            let (kind, _, preferred) = command_args(args.as_ref(), NONE_KIND, AutoApply::Never);
            bindings.push((kind, preferred, crate::keybindings::label(&item.keybinding)));
        }
        bindings
    }

    /// The hover showed or hid: a new marker asks for its quick fixes (an Invoke for its range and `quickfix`).
    pub(super) fn code_actions_hover_changed(&mut self, cx: &mut Context<Self>) {
        let marker = self.content_hover.shown.as_ref().and_then(|shown| {
            let mut parts: Vec<_> = shown.parts.iter().collect();
            parts.sort_by_key(|part| (part.participant, part.ordinal));
            parts.into_iter().find_map(|part| match &part.content {
                super::content_hover::PartContent::Marker(marker) => Some(marker.clone()),
                super::content_hover::PartContent::Markdown(_) | super::content_hover::PartContent::ColorPicker => None,
            })
        });
        let fixes = &mut self.code_actions.marker_fixes;
        if fixes.marker.as_deref() == marker.as_deref() {
            return;
        }
        fixes.tasks.clear();
        fixes.answered = false;
        fixes.checking = false;
        fixes.marker = marker.clone();
        if let Some(set) = fixes.set.take() {
            self.release_code_action_set(set);
        }
        let Some(marker) = marker.filter(|_| self.active_editor_readonly().is_none()) else { return };
        let fixes = &mut self.code_actions.marker_fixes;
        if fixes.recent.as_ref().is_some_and(|(recent, _)| **recent != *marker) {
            fixes.recent = None;
        }
        if !fixes.recent.as_ref().is_some_and(|(_, has)| !has) {
            fixes.tasks.push(cx.spawn(async move |workbench, cx| {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                let _ = workbench.update(cx, |workbench, cx| {
                    workbench.code_actions.marker_fixes.checking = true;
                    cx.notify();
                });
            }));
        }
        let range = json!({ "startLineNumber": marker.start_line, "startColumn": marker.start_column, "endLineNumber": marker.end_line, "endColumn": marker.end_column });
        let filter = Filter { include: Some(QUICK_FIX.to_owned()), ..Default::default() };
        let trigger = Trigger { kind: INVOKE, source: Source::QuickFix, filter: filter.clone(), auto_apply: None, not_available: None, window: None };
        let Some(editor) = self.editors.active() else { return };
        let calls = self.code_action_calls(editor, range, &trigger);
        let caches = Rc::new(RefCell::new(Vec::new()));
        self.code_actions.marker_fixes.tasks.push(cx.spawn(async move |workbench, cx| {
            let set = collect_code_actions(calls, &filter, &caches).await;
            let _ = workbench.update(cx, |workbench, cx| {
                let fixes = &mut workbench.code_actions.marker_fixes;
                fixes.answered = true;
                fixes.recent = Some((marker, !set.valid.is_empty()));
                match set.valid.is_empty() {
                    true => workbench.release_code_action_set(Rc::new(set)),
                    false => fixes.set = Some(Rc::new(set)),
                }
                cx.notify();
            });
        }));
    }

    /// markersTreeViewer.ts `MarkerViewModel.getCodeActions`: the quick fixes of the marker's range, triggered from
    /// the Problems view.
    pub(super) fn request_marker_quick_fixes(&self, editor: &OpenEditor, marker: &Marker) -> impl std::future::Future<Output = CodeActionSet> + use<> {
        let range = json!({ "startLineNumber": marker.start_line, "startColumn": marker.start_column, "endLineNumber": marker.end_line, "endColumn": marker.end_column });
        let filter = Filter { include: Some(QUICK_FIX.to_owned()), ..Default::default() };
        let trigger = Trigger { kind: INVOKE, source: Source::QuickFix, filter: filter.clone(), auto_apply: None, not_available: None, window: None };
        let calls = self.code_action_calls(editor, range, &trigger);
        async move { collect_code_actions(calls, &filter, &Rc::new(RefCell::new(Vec::new()))).await }
    }

    /// The status bar's quick fix entry for `marker`: "Quick Fix..." once fixes came, else a placeholder.
    // shortcut: no AI code action entry and no menu-contributed hover actions; neither has a provider in the port.
    pub(super) fn marker_quick_fix_status(&self, marker: &Marker) -> QuickFixStatus {
        let fixes = &self.code_actions.marker_fixes;
        if fixes.marker.as_deref() != Some(marker) {
            return QuickFixStatus::Text("\u{a0}");
        }
        if fixes.set.is_some() {
            return QuickFixStatus::QuickFix;
        }
        let none_before = fixes.recent.as_ref().is_some_and(|(recent, has)| **recent == *marker && !has);
        if fixes.answered || none_before {
            QuickFixStatus::Text("No quick fixes available")
        } else if fixes.checking {
            QuickFixStatus::Text("Checking for quick fixes...")
        } else {
            QuickFixStatus::Text("\u{a0}")
        }
    }

    /// "Quick Fix..." in the hover: the widget opens under the label with the marker's fixes.
    pub(super) fn show_marker_quick_fixes(&mut self, cx: &mut Context<Self>) {
        let Some(set) = self.code_actions.marker_fixes.set.clone() else { return };
        let bounds = self.code_actions.marker_fixes.label_bounds.get();
        self.hide_content_hover(cx);
        self.show_code_action_list(set, Anchor::Point(point(bounds.origin.x, bounds.origin.y + bounds.size.height)), false, false, cx);
    }

    /// The `quickfix-edit-highlight` of the focused action's ranges, or its diagnostics, on model line `index`.
    pub(super) fn code_action_highlights(&self, editor: &OpenEditor, index: usize, char_width: f32) -> Vec<Div> {
        let Some(action) = self.action_widget_focused_action() else { return Vec::new() };
        let ranges = action["ranges"].as_array().filter(|ranges| !ranges.is_empty());
        let diagnostics = action["diagnostics"].as_array().filter(|diagnostics| !diagnostics.is_empty());
        let ranges = match (ranges, diagnostics) {
            (Some(_), Some(diagnostics)) if diagnostics.len() > 1 => diagnostics,
            (Some(ranges), _) => ranges,
            (None, Some(diagnostics)) => diagnostics,
            (None, None) => return Vec::new(),
        };
        let text = &editor.document().lines[index];
        let border = self.theme.color("editor.findMatchHighlightBorder");
        let mut highlights = Vec::new();
        for range in ranges {
            let number = |key: &str| range[key].as_u64().unwrap_or(1).max(1) as usize - 1;
            let (start_line, end_line) = (number("startLineNumber"), number("endLineNumber"));
            if index < start_line || index > end_line {
                continue;
            }
            let from = if index == start_line { char_column(text, number("startColumn")) } else { 0 };
            let to = if index == end_line { char_column(text, number("endColumn")) } else { text.chars().count() };
            let (left, right) = editor.line_layout(index).range(from, to);
            let (left, right) = (left * char_width, right * char_width);
            let mut highlight = div().absolute().top_0().h_full().left(px(left)).w(px(right - left)).bg(self.color("editor.findMatchHighlightBackground"));
            if let Some(border) = border {
                highlight = highlight.border_1().border_color(border);
            }
            highlights.push(highlight);
        }
        highlights
    }
}

/// `codeActionGroups`: the kind, the header and the icon of each group; the last takes every other kind.
const GROUPS: [(&str, &str, Option<&str>); 8] = [
    (QUICK_FIX, "Quick Fix", None),
    ("refactor.extract", "Extract", Some("wrench")),
    ("refactor.inline", "Inline", Some("wrench")),
    ("refactor.rewrite", "Rewrite", Some("wrench")),
    ("refactor.move", "Move", Some("wrench")),
    ("refactor.surround", "Surround With", Some("surround-with")),
    (SOURCE, "Source Action", Some("symbol-file")),
    UNCATEGORIZED,
];
const UNCATEGORIZED: (&str, &str, Option<&str>) = ("", "More Actions...", None);

fn auto_trigger() -> Trigger {
    Trigger { kind: AUTO, source: Source::Default, filter: Filter::default(), auto_apply: None, not_available: None, window: None }
}

fn editor_line_above_cursor(workbench: &Workbench, line: usize) -> bool {
    workbench.editors.active().is_some_and(|editor| line < editor.selection().head.line)
}

/// `CodeActionCommandArgs.fromUser`: the kind, the apply mode and `preferred` of a command's argument object.
fn command_args(arg: Option<&Value>, default_kind: &str, default_apply: AutoApply) -> (String, AutoApply, bool) {
    let Some(arg) = arg.filter(|arg| arg.is_object()) else { return (default_kind.to_owned(), default_apply, false) };
    let kind = arg["kind"].as_str().unwrap_or(default_kind).to_owned();
    let apply = match arg["apply"].as_str().map(str::to_lowercase).as_deref() {
        Some("first") => AutoApply::First,
        Some("never") => AutoApply::Never,
        Some("ifsingle") => AutoApply::IfSingle,
        _ => default_apply,
    };
    (kind, apply, arg["preferred"].as_bool().unwrap_or(false))
}

/// The commands' `notAvailableMessage`s: "No code actions available", with `preferred` and the kind asked for.
fn none_message(arg: Option<&Value>, preferred: bool, what: &str) -> String {
    let preferred_word = if preferred { "preferred " } else { "" };
    match arg.and_then(|arg| arg["kind"].as_str()) {
        Some(kind) => format!("No {preferred_word}{what} for '{kind}' available"),
        None => format!("No {preferred_word}{what} available"),
    }
}

/// `HierarchicalKind.contains`.
fn contains(kind: &str, other: &str) -> bool {
    kind == other || kind.is_empty() || other.strip_prefix(kind).is_some_and(|rest| rest.starts_with('.'))
}

fn intersects(a: &str, b: &str) -> bool {
    contains(a, b) || contains(b, a)
}

fn provided_kinds(args: &[Value]) -> Vec<&str> {
    args.get(2).and_then(|metadata| metadata["providedKinds"].as_array()).into_iter().flatten().filter_map(Value::as_str).collect()
}

/// `excludesAction`: an exclude that contains the kind, unless the include is more specific.
fn excludes_action(kind: &str, exclude: &str, include: Option<&str>) -> bool {
    contains(exclude, kind) && !include.is_some_and(|include| contains(exclude, include))
}

/// `mayIncludeActionsOfKind`.
fn may_include_kind(filter: &Filter, kind: &str) -> bool {
    if filter.include.as_deref().is_some_and(|include| !intersects(include, kind)) {
        return false;
    }
    if filter.excludes.iter().any(|exclude| excludes_action(kind, exclude, filter.include.as_deref())) {
        return false;
    }
    filter.include_source_actions || !contains(SOURCE, kind)
}

/// `filtersAction`.
fn filters_action(filter: &Filter, action: &Value) -> bool {
    let kind = action["kind"].as_str();
    if let Some(include) = &filter.include
        && !kind.is_some_and(|kind| contains(include, kind))
    {
        return false;
    }
    if let Some(kind) = kind {
        if filter.excludes.iter().any(|exclude| excludes_action(kind, exclude, filter.include.as_deref())) {
            return false;
        }
        if !filter.include_source_actions && contains(SOURCE, kind) {
            return false;
        }
    }
    !filter.only_preferred || action["isPreferred"] == true
}

/// `getDocumentationFromProvider`: the entry whose kind best contains the include, else the first whose kind
/// contains one of the actions' kinds.
fn documentation_from_provider(documentation: &Value, actions: &[Value], only: Option<&str>) -> Option<Value> {
    let entries: Vec<(&str, &Value)> = documentation.as_array()?.iter().map(|entry| (entry["kind"].as_str().unwrap_or_default(), &entry["command"])).collect();
    if let Some(only) = only {
        let mut best: Option<(&str, &Value)> = None;
        for &(kind, command) in &entries {
            if contains(kind, only) && best.is_none_or(|(best_kind, _)| contains(best_kind, kind)) {
                best = Some((kind, command));
            }
        }
        if let Some((_, command)) = best {
            return Some(command.clone());
        }
    }
    for action in actions {
        let Some(action_kind) = action["kind"].as_str() else { continue };
        if let Some((_, command)) = entries.iter().find(|(kind, _)| contains(kind, action_kind)) {
            return Some((*command).clone());
        }
    }
    None
}

/// `getCodeActions` after the calls went out: each provider's actions the filter keeps, its documentation, and
/// its list's cache id, kept in `caches` until the set takes them.
async fn collect_code_actions(calls: Vec<(u64, oneshot::Receiver<Result<Value, Value>>, Value)>, filter: &Filter, caches: &Rc<RefCell<Vec<(u64, u64)>>>) -> CodeActionSet {
    let mut items = Vec::new();
    let mut documentation = Vec::new();
    let mut taken = Vec::new();
    for (handle, call, provider_documentation) in calls {
        let Ok(Ok(list)) = call.await else { continue };
        if let Some(cache_id) = list["cacheId"].as_u64() {
            caches.borrow_mut().push((handle, cache_id));
            taken.push((handle, cache_id));
        }
        let actions: Vec<Value> = list["actions"].as_array().into_iter().flatten().filter(|action| action.is_object() && filters_action(filter, action)).cloned().collect();
        documentation.extend(documentation_from_provider(&provider_documentation, &actions, filter.include.as_deref()));
        items.extend(actions.into_iter().map(|action| CodeActionItem { action, handle }));
    }
    CodeActionSet::new(items, documentation, taken)
}

/// codeActionModel.ts `includeNearbyQuickFixes`: without a quick fix at the selection, the quick fixes at the end
/// of the markers on the cursor's line, the closest first, each title once.
async fn nearby_quick_fixes(set: CodeActionSet, workbench: &gpui::WeakEntity<Workbench>, cx: &mut gpui::AsyncApp, trigger: &Trigger, cursor: (usize, usize), markers: &[Value], caches: &Rc<RefCell<Vec<(u64, u64)>>>) -> CodeActionSet {
    let related: Vec<Value> = markers.iter().filter(|marker| !marker["relatedInformation"].is_null()).cloned().collect();
    let mark_fix_all = |items: &mut Vec<CodeActionItem>| {
        for item in items {
            if item.action["command"]["arguments"].as_array().into_iter().flatten().any(|arg| arg.as_str().is_some_and(|arg| arg.contains(APPLY_FIX_ALL_COMMAND_ID))) {
                item.action["diagnostics"] = Value::Array(related.clone());
            }
        }
    };
    let found_quick_fix = set.valid.iter().any(|item| item.action["kind"].as_str().is_some_and(|kind| contains(QUICK_FIX, kind)) && item.action["isAI"] != true);
    let mut set = set;
    if found_quick_fix {
        mark_fix_all(&mut set.valid);
        return set;
    }
    if markers.is_empty() {
        return set;
    }
    let mut all = set.all.clone();
    let mut current = set.valid.clone();
    let mut distance = usize::MAX;
    for marker in markers {
        let (row, column) = (marker["endLineNumber"].as_u64().unwrap_or(0) as usize, marker["endColumn"].as_u64().unwrap_or(0) as usize);
        let start_row = marker["startLineNumber"].as_u64().unwrap_or(0) as usize;
        if row != cursor.0 && start_row != cursor.0 {
            continue;
        }
        let include = trigger.filter.include.clone().unwrap_or_else(|| QUICK_FIX.to_owned());
        let at_marker = Trigger { filter: Filter { include: Some(include), ..Default::default() }, ..trigger.clone() };
        let caret = json!({ "selectionStartLineNumber": row, "selectionStartColumn": column, "positionLineNumber": row, "positionColumn": column, "startLineNumber": row, "startColumn": column, "endLineNumber": row, "endColumn": column });
        let Ok(calls) = workbench.update(cx, |workbench, _| workbench.editors.active().map(|editor| workbench.code_action_calls(editor, caret, &at_marker))) else { break };
        let Some(calls) = calls else { break };
        let mut found = collect_code_actions(calls, &at_marker.filter, caches).await;
        set.caches.append(&mut found.caches);
        if !found.valid.is_empty() {
            mark_fix_all(&mut found.valid);
            if set.all.is_empty() {
                all.extend(found.all.iter().cloned());
            }
            if cursor.1.abs_diff(column) < distance {
                current.splice(0..0, found.valid.iter().cloned());
            } else {
                current.extend(found.valid.iter().cloned());
            }
        }
        distance = cursor.1.abs_diff(column);
    }
    let mut seen = std::collections::HashSet::new();
    current.retain(|item| seen.insert(item.action["title"].as_str().unwrap_or_default().to_owned()));
    current.sort_by(|a, b| {
        let (a, b) = (&a.action, &b.action);
        (b["isPreferred"] == true).cmp(&(a["isPreferred"] == true)).then((a["isAI"] == true).cmp(&(b["isAI"] == true)))
    });
    CodeActionSet { all, valid: current, ..set }
}

impl CodeActionSet {
    /// `ManagedCodeActionSet`: AI actions last, then the ones fixing diagnostics first, then the preferred ones.
    fn new(mut all: Vec<CodeActionItem>, documentation: Vec<Value>, caches: Vec<(u64, u64)>) -> Self {
        let preferred = |action: &Value| action["isPreferred"] == true;
        let has_diagnostics = |action: &Value| action["diagnostics"].as_array().is_some_and(|diagnostics| !diagnostics.is_empty());
        all.sort_by(|a, b| {
            let (a, b) = (&a.action, &b.action);
            (a["isAI"] == true).cmp(&(b["isAI"] == true)).then_with(|| match (has_diagnostics(a), has_diagnostics(b)) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => preferred(b).cmp(&preferred(a)),
            })
        });
        let valid: Vec<CodeActionItem> = all.iter().filter(|item| item.action["disabled"].is_null()).cloned().collect();
        let has_auto_fix = valid.iter().any(|item| item.action["kind"].as_str().is_some_and(|kind| contains(QUICK_FIX, kind)) && preferred(&item.action));
        let has_ai_fix = valid.iter().any(|item| item.action["isAI"] == true);
        let all_ai_fixes = valid.iter().all(|item| item.action["isAI"] == true);
        CodeActionSet { all, valid, documentation, has_auto_fix, has_ai_fix, all_ai_fixes, caches }
    }
}

/// `_getRangeOfSelectionUnlessWhitespaceEnclosed`: a manual trigger takes the selection; an automatic one none with
/// the light bulb off, and with `onCode` none for an empty selection between whitespace.
fn selection_unless_whitespace_enclosed(editor: &OpenEditor, trigger: &Trigger, lightbulb: &str) -> Option<Selection> {
    let selection = editor.selection();
    if trigger.kind == INVOKE {
        return Some(selection);
    }
    if lightbulb == "off" {
        return None;
    }
    if lightbulb != "onCode" || !selection.is_empty() {
        return Some(selection);
    }
    let chars: Vec<char> = editor.document().lines[selection.head.line].chars().collect();
    let column = selection.head.column;
    let enclosed = match chars.len() {
        0 => true,
        _ if column == 0 => chars[0].is_whitespace(),
        length if column == length => chars[length - 1].is_whitespace(),
        _ => chars[column - 1].is_whitespace() && chars[column].is_whitespace(),
    };
    (!enclosed).then_some(selection)
}

/// `ISelection` with 1-based UTF-16 columns.
fn selection_value(editor: &OpenEditor, selection: Selection) -> Value {
    let lines = &editor.document().lines;
    let column = |position: Position| utf16_column(&lines[position.line], position.column) + 1;
    let (start, end) = (selection.start(), selection.end());
    json!({
        "selectionStartLineNumber": selection.anchor.line + 1,
        "selectionStartColumn": column(selection.anchor),
        "positionLineNumber": selection.head.line + 1,
        "positionColumn": column(selection.head),
        "startLineNumber": start.line + 1,
        "startColumn": column(start),
        "endLineNumber": end.line + 1,
        "endColumn": column(end),
    })
}

/// `computeIndentLevel`: the visible width of the leading whitespace, -1 for a whitespace-only line.
fn compute_indent_level(text: &str, tab_size: usize) -> isize {
    let mut indent = 0;
    for character in text.chars() {
        match character {
            ' ' => indent += 1,
            '\t' => indent = indent - indent % tab_size + tab_size,
            _ => return indent as isize,
        }
    }
    -1
}

/// `tryGetValidActionToApply`.
fn valid_action_to_apply(trigger: &Trigger, set: &CodeActionSet) -> Option<CodeActionItem> {
    match trigger.auto_apply {
        Some(AutoApply::First) => set.valid.first().cloned(),
        Some(AutoApply::IfSingle) if set.valid.len() == 1 => Some(set.valid[0].clone()),
        _ => None,
    }
}

/// `getInvalidActionThatWouldHaveBeenApplied`: the disabled reason of the action an auto apply would have taken.
fn invalid_action_that_would_have_been_applied(trigger: &Trigger, set: &CodeActionSet) -> Option<String> {
    let would_apply = match trigger.auto_apply {
        Some(AutoApply::First) => set.valid.is_empty(),
        Some(AutoApply::IfSingle) => set.all.len() == 1,
        _ => false,
    };
    if set.all.is_empty() || !would_apply {
        return None;
    }
    set.all.iter().find_map(|item| item.action["disabled"].as_str().map(String::from))
}

/// `bestKeybindingForCodeAction`: of the bindings whose kind contains the action's (preferred-only ones for
/// preferred actions), the most specific, the earliest on a tie.
fn keybinding_for_action(bindings: &[(String, bool, String)], action: &Value) -> Option<String> {
    let kind = action["kind"].as_str()?;
    let mut best: Option<&(String, bool, String)> = None;
    for binding in bindings.iter().rev() {
        if !contains(&binding.0, kind) || binding.1 && action["isPreferred"] != true {
            continue;
        }
        best = match best {
            None => Some(binding),
            Some(current) if contains(&current.0, &binding.0) => Some(binding),
            Some(current) => Some(current),
        };
    }
    best.map(|binding| binding.2.clone())
}

mod on_save;
#[cfg(test)]
mod tests;
