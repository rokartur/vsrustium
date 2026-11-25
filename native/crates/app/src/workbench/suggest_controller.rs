//! suggestModel.ts, suggestController.ts and suggestCommitCharacters.ts: when completion runs, which providers it
//! asks, how the items follow the typing, and what accepting one writes. The widget is suggest_widget.rs.

use std::time::Duration;

use config::configuration::Overrides;
use futures::channel::oneshot;
use gpui::{Context, Task, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::bulk_text_edits::escape_snippet;
use super::host_requests::{range, uri_parts};
use super::snippet_completions::SNIPPET_PROVIDER;
use super::snippet_controller::{SNIPPET_CHOICE_PROVIDER, SnippetInsert};
use crate::commands::Outcome;
use crate::editor::{EditReason, OpenEditor, Position, Selection, utf16_column, word_at};
use crate::filters::FuzzyScoreOptions;
use crate::language_features::Candidate;
use crate::suggest::{CompletionItem, CompletionModel, INSERT_AS_SNIPPET, KIND_TEXT, LineContext, SnippetSort, compare_provided, select_first};

/// The handle of the main thread's word based provider (editorWorkerService.ts `WordBasedCompletionItemProvider`).
pub(super) const WORD_BASED_PROVIDER: u64 = u64::MAX;
/// `CompletionTriggerKind`.
const INVOKE: u64 = 0;
const TRIGGER_CHARACTER: u64 = 1;
const TRIGGER_FOR_INCOMPLETE: u64 = 2;
/// editorWebWorker.ts `_suggestionsLimit`.
const WORD_SUGGESTIONS_LIMIT: usize = 10000;

#[derive(Default)]
pub(super) struct Suggest {
    /// `_triggerState`, `_context` and `_completionModel`.
    pub(super) session: Option<Session>,
    /// `_triggerQuickSuggest`.
    quick_suggest: Option<Task<()>>,
    /// `_requestToken`: dropping the task drops the host calls, which cancels them.
    request: Option<Task<()>>,
    /// `_currentSelection` with its model.
    last_selection: Option<(u32, Selection)>,
    /// `_completionDisposables`: the result ids `$releaseCompletionItems` frees.
    to_release: Vec<(u64, u64)>,
    pub(super) widget: super::suggest_widget::WidgetState,
    /// The commit characters of the focused item (`CommitCharacterController._active`).
    commit_characters: Vec<char>,
}

#[derive(Clone)]
pub(super) struct TriggerOptions {
    pub auto: bool,
    pub retrigger: bool,
    pub refilter: bool,
    pub trigger_kind: u64,
    pub trigger_character: Option<String>,
    pub provider_filter: Vec<u64>,
    pub reuse: Vec<(u64, Vec<CompletionItem>)>,
    pub clipboard: Option<String>,
    /// `noFilter` of `triggerSuggest`: `editor.suggest.show*` hide nothing.
    pub no_kind_filter: bool,
}

impl TriggerOptions {
    pub(super) fn new(auto: bool) -> Self {
        Self { auto, retrigger: false, refilter: false, trigger_kind: INVOKE, trigger_character: None, provider_filter: Vec::new(), reuse: Vec::new(), clipboard: None, no_kind_filter: false }
    }
}

/// `LineContext` of the editor's cursor.
#[derive(Clone, Debug)]
struct Context_ {
    line: usize,
    column: usize,
    leading_line_content: String,
    /// `getWordUntilPosition`: the word's start and its text up to the cursor.
    leading_word_start: usize,
    leading_word: String,
}

pub(super) struct Session {
    pub model_id: u32,
    pub options: TriggerOptions,
    context: Context_,
    pub model: Option<CompletionModel>,
    /// `LineSuffix`: the chars right of the cursor when it triggered.
    line_suffix: usize,
}

/// One provider's answer: its items, whether the list is incomplete, and the id to release.
struct ProviderAnswer {
    provider: u64,
    supports_resolve: bool,
    result: Value,
}

impl Workbench {
    pub(super) fn suggest_setting(&self, editor: &OpenEditor, key: &str) -> Value {
        let settings = self.settings.lock().unwrap();
        let (resource, language) = (editor.document().path.clone(), editor.document().language_id.clone());
        let overrides = Overrides { resource: Some(&resource), override_identifier: Some(&language) };
        settings.get_value(Some(key), &overrides).unwrap_or(Value::Null)
    }

    pub(super) fn register_suggest(&mut self) {
        // editorWorkerService.ts registers the word based provider for every language ('*').
        self.language_features.register("$registerCompletionsProvider", vec![json!(WORD_BASED_PROVIDER), json!("*"), json!([]), json!(false)]);
        self.commands.register("editor.action.triggerSuggest", |workbench, _, _, cx| {
            workbench.trigger_suggest(TriggerOptions::new(false), cx);
            Outcome::done()
        });
        self.commands.register("acceptSelectedSuggestion", |workbench, _, window, cx| {
            workbench.accept_selected_suggestion(false, window, cx);
            Outcome::done()
        });
        self.commands.register("acceptAlternativeSelectedSuggestion", |workbench, _, window, cx| {
            workbench.accept_selected_suggestion(true, window, cx);
            Outcome::done()
        });
        self.commands.register("hideSuggestWidget", |workbench, _, _, cx| {
            workbench.cancel_suggest(false, cx);
            workbench.clear_suggest();
            Outcome::done()
        });
        self.register_suggest_widget_commands();
        self.commands.register("_executeCompletionItemProvider", |workbench, args, _, cx| workbench.execute_completion_item_provider(&args, cx));
    }

    /// `cancel`: the request and the timer stop, the session ends, the widget hides unless it retriggers.
    pub(super) fn cancel_suggest(&mut self, retrigger: bool, cx: &mut Context<Self>) {
        self.suggest.quick_suggest = None;
        if self.suggest.session.take().is_some() {
            self.suggest.request = None;
            self.suggest.commit_characters.clear();
            if !retrigger {
                self.hide_suggest_widget();
            }
            cx.notify();
        }
    }

    /// `clear`: the providers' results are released.
    pub(super) fn clear_suggest(&mut self) {
        for (handle, id) in std::mem::take(&mut self.suggest.to_release) {
            drop(self.host.call("ExtHostLanguageFeatures", "$releaseCompletionItems", vec![json!(handle), json!(id)]));
        }
    }

    /// The `onDidBlurEditorWidget` listener of the controller.
    pub(super) fn suggest_editor_blurred(&mut self, cx: &mut Context<Self>) {
        self.cancel_suggest(false, cx);
        self.clear_suggest();
    }

    /// `onDidChangeModelContent` and `onDidChangeCursorSelection` of the model: an edit refilters a running session;
    /// typing forward starts the quick suggest timer; a keyboard move refilters; anything else cancels.
    pub(super) fn suggest_editor_changed(&mut self, content_changed: bool, source: &str, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let selection = editor.selection();
        let model_id = editor.model_id;
        let previous = self.suggest.last_selection.replace((model_id, selection));
        if self.suggest.session.as_ref().is_some_and(|session| session.model_id != model_id) {
            self.cancel_suggest(false, cx);
        }
        let composing = self.marked_text.is_some();
        if content_changed && !composing && self.suggest.session.is_some() {
            self.refilter_suggest(cx);
        }
        if composing {
            return;
        }
        if !selection.is_empty() || source != "keyboard" {
            self.cancel_suggest(false, cx);
            return;
        }
        if self.suggest.session.is_none() && content_changed {
            let Some((previous_model, previous)) = previous else { return };
            let contains = previous.start() <= selection.start() && selection.end() <= previous.end();
            if previous_model == model_id && (contains || previous.end() <= selection.head) {
                self.trigger_quick_suggest(cx);
            }
        } else if self.suggest.session.is_some() && !content_changed {
            self.refilter_suggest(cx);
        }
    }

    /// `_doTriggerQuickSuggest`: after `editor.quickSuggestionsDelay`, a word at the cursor in a token kind
    /// `editor.quickSuggestions` turns on starts an automatic session.
    fn trigger_quick_suggest(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let config = self.suggest_setting(editor, "editor.quickSuggestions");
        if quick_suggestions_all_off(&config) {
            return;
        }
        let delay = self.suggest_setting(editor, "editor.quickSuggestionsDelay").as_u64().unwrap_or(10);
        self.cancel_suggest(false, cx);
        self.suggest.quick_suggest = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(Duration::from_millis(delay)).await;
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.suggest.quick_suggest = None;
                if workbench.suggest.session.is_some() || !workbench.should_auto_trigger() {
                    return;
                }
                let Some(editor) = workbench.editors.active() else { return };
                let config = workbench.suggest_setting(editor, "editor.quickSuggestions");
                if !quick_suggestions_all_on(&config) {
                    let head = editor.selection().head;
                    let text = &editor.document().lines[head.line];
                    let offset = text.char_indices().nth(head.column.saturating_sub(1)).map_or(text.len(), |(offset, _)| offset);
                    let token = editor.document().tokens.token_type_at(&editor.document().lines, head.line, offset);
                    let key = match token {
                        textmate::StandardTokenType::Comment => "comments",
                        textmate::StandardTokenType::String => "strings",
                        _ => "other",
                    };
                    if matches!(quick_suggestions_value(&config[key]), "off" | "inline") {
                        return;
                    }
                }
                workbench.trigger_suggest(TriggerOptions::new(true), cx);
            });
        }));
    }

    /// `LineContext.shouldAutoTrigger`: the cursor ends a word, or follows its first char, and the word is no number.
    fn should_auto_trigger(&self) -> bool {
        let Some(editor) = self.editors.active() else { return false };
        let head = editor.selection().head;
        let text = &editor.document().lines[head.line];
        let Some(word) = word_at(text, head.column) else { return false };
        if word.end != head.column && word.start + 1 != head.column {
            return false;
        }
        let word: String = text.chars().skip(word.start).take(word.len()).collect();
        word.trim().parse::<f64>().is_err()
    }

    /// The `onDidType` listener: a provider's trigger character just typed starts (or restarts) an automatic session.
    pub(super) fn suggest_typed(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        if self.suggest_setting(editor, "editor.suggestOnTriggerCharacters") == Value::Bool(false) || self.active_editor_readonly().is_some() || self.should_auto_trigger() {
            return;
        }
        let head = editor.selection().head;
        let Some(last) = editor.document().lines[head.line].chars().take(head.column).last() else { return };
        let last = last.to_string();
        let uri = editor.document().uri();
        let language = editor.document().language_id.clone();
        let candidate = Candidate { uri: &uri, language: &language };
        let supports: Vec<u64> = self.language_features.ordered("$registerCompletionsProvider", &candidate).into_iter().filter(|registration| registration.args.get(2).and_then(Value::as_array).is_some_and(|chars| chars.contains(&json!(last)))).map(|registration| registration.handle).collect();
        if supports.is_empty() {
            return;
        }
        let model = self.suggest.session.as_ref().and_then(|session| session.model.as_ref());
        let reuse = model.map(|model| model.provider_items().into_iter().filter(|(provider, _)| !supports.contains(provider)).collect()).unwrap_or_default();
        let options = TriggerOptions { trigger_kind: TRIGGER_CHARACTER, trigger_character: Some(last), retrigger: model.is_some(), provider_filter: supports, reuse, ..TriggerOptions::new(true) };
        self.trigger_suggest(options, cx);
    }

    /// The `onWillType` listener of `CommitCharacterController`: a commit character of the focused item accepts it
    /// before the character is typed.
    pub(super) fn suggest_will_type(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(last) = text.chars().last() else { return };
        let Some(editor) = self.editors.active() else { return };
        if self.suggest.commit_characters.contains(&last) && self.suggest.session.is_some() && !self.suggest_widget_frozen() && self.suggest_setting(editor, "editor.acceptSuggestionOnCommitCharacter") != Value::Bool(false) {
            if let Some(index) = self.suggest.widget.focused {
                self.insert_suggestion(index, InsertFlags { no_after_undo_stop: true, ..InsertFlags::default() }, window, cx);
            }
        }
    }

    fn line_context(editor: &OpenEditor) -> Context_ {
        let head = editor.selection().head;
        let text = &editor.document().lines[head.line];
        let leading_line_content: String = text.chars().take(head.column).collect();
        let (leading_word_start, leading_word) = match word_at(text, head.column) {
            Some(word) if word.start < head.column => (word.start, text.chars().skip(word.start).take(head.column - word.start).collect()),
            _ => (head.column, String::new()),
        };
        Context_ { line: head.line, column: head.column, leading_line_content, leading_word_start, leading_word }
    }

    /// `trigger`: asks the providers in `orderedGroups`, a group at a time until one gives items.
    pub(super) fn trigger_suggest(&mut self, options: TriggerOptions, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let context = Self::line_context(editor);
        let model_id = editor.model_id;
        let line_suffix = editor.document().lines[context.line].chars().count() - context.column;
        self.cancel_suggest(options.retrigger, cx);
        let auto = options.auto;
        self.suggest.session = Some(Session { model_id, options: options.clone(), context: context.clone(), model: None, line_suffix });
        self.show_suggest_triggered(auto, cx);
        // suggest.ts asks the snippet support beside the provider groups, whichever group answers.
        let snippets = match options.reuse.iter().find(|(provider, _)| *provider == SNIPPET_PROVIDER) {
            Some((_, items)) => Some(Call::Reuse(items.clone())),
            None if options.provider_filter.is_empty() || options.provider_filter.contains(&SNIPPET_PROVIDER) => {
                let result = self.snippet_suggestions(options.trigger_character.as_deref());
                Some(Call::Ready(ProviderAnswer { provider: SNIPPET_PROVIDER, supports_resolve: false, result }))
            }
            None => None,
        };
        let editor = self.editors.active().expect("checked above");
        let uri = editor.document().uri();
        let language = editor.document().language_id.clone();
        let candidate = Candidate { uri: &uri, language: &language };
        let position = json!({ "lineNumber": context.line + 1, "column": utf16_column(&editor.document().lines[context.line], context.column) + 1 });
        let completion_context = match &options.trigger_character {
            Some(character) => json!({ "triggerKind": TRIGGER_CHARACTER, "triggerCharacter": character }),
            None => json!({ "triggerKind": options.trigger_kind }),
        };
        let mut groups = Vec::new();
        // The controller registers the choice provider with an exclusive selector while a choice is active.
        let choice_active = editor.snippet_session.as_ref().is_some_and(|session| session.active_choice().is_some());
        if choice_active {
            let result = self.snippet_choice_completions(editor);
            groups.push(vec![Call::Ready(ProviderAnswer { provider: SNIPPET_CHOICE_PROVIDER, supports_resolve: false, result })]);
        }
        for group in self.language_features.ordered_groups("$registerCompletionsProvider", &candidate).into_iter().filter(|_| !choice_active) {
            let mut calls = Vec::new();
            for registration in group {
                let handle = registration.handle;
                if let Some((_, items)) = options.reuse.iter().find(|(provider, _)| *provider == handle) {
                    calls.push(Call::Reuse(items.clone()));
                    continue;
                }
                if !options.provider_filter.is_empty() && !options.provider_filter.contains(&handle) {
                    continue;
                }
                let supports_resolve = registration.args.get(3) == Some(&Value::Bool(true));
                if handle == WORD_BASED_PROVIDER {
                    let result = self.word_based_completions(editor, editor.selection().head);
                    calls.push(Call::Ready(ProviderAnswer { provider: handle, supports_resolve, result }));
                    continue;
                }
                let args = vec![json!(handle), uri.clone(), position.clone(), completion_context.clone()];
                calls.push(Call::Host(handle, supports_resolve, self.host.call_cancellable("ExtHostLanguageFeatures", "$provideCompletionItems", args)));
            }
            groups.push(calls);
        }
        self.suggest.request = Some(cx.spawn(async move |workbench, cx| {
            let mut items = Vec::new();
            let mut answers = Vec::new();
            for group in groups {
                let mut added = false;
                for call in group {
                    match call {
                        Call::Reuse(reused) => {
                            added |= !reused.is_empty();
                            items.extend(reused);
                        }
                        Call::Ready(answer) => {
                            added |= answer.result["b"].as_array().is_some_and(|items| !items.is_empty());
                            answers.push(answer);
                        }
                        Call::Host(provider, supports_resolve, receiver) => {
                            if let Ok(Ok(result)) = receiver.await.map(|answer| answer.map_err(drop)) {
                                added |= result["b"].as_array().is_some_and(|items| !items.is_empty());
                                answers.push(ProviderAnswer { provider, supports_resolve, result });
                            }
                        }
                    }
                }
                if added {
                    break;
                }
            }
            match snippets {
                Some(Call::Reuse(reused)) => items.extend(reused),
                Some(Call::Ready(answer)) => answers.push(answer),
                _ => {}
            }
            let _ = workbench.update(cx, |workbench, cx| workbench.suggest_items_arrived(model_id, items, answers, cx));
        }));
    }

    /// The end of `trigger`: the items become the session's model, filtered for the cursor as it is now.
    fn suggest_items_arrived(&mut self, model_id: u32, mut items: Vec<CompletionItem>, answers: Vec<ProviderAnswer>, cx: &mut Context<Self>) {
        self.suggest.request = None;
        for answer in &answers {
            if let Some(id) = answer.result["x"].as_u64() {
                self.suggest.to_release.push((answer.provider, id));
            }
        }
        let Some(editor) = self.editors.active().filter(|editor| editor.model_id == model_id) else { return };
        let Some(session) = self.suggest.session.as_ref() else { return };
        let position = Position::new(session.context.line, session.context.column);
        let kind_filter = if session.options.no_kind_filter { Vec::new() } else { self.suggest_kind_filter(editor) };
        let show_deprecated = self.suggest_setting(editor, "editor.suggest.showDeprecated") != Value::Bool(false);
        for answer in answers {
            let incomplete = answer.result["c"] == Value::Bool(true);
            for dto in answer.result["b"].as_array().into_iter().flatten() {
                let item = CompletionItem::from_dto(dto, answer.provider, answer.supports_resolve, incomplete, position, &answer.result["a"], |value| range(editor, value));
                if kind_filter.contains(&item.kind) || (!show_deprecated && item.deprecated) {
                    continue;
                }
                items.push(item);
            }
        }
        let snippet_sort = snippet_sort(&self.suggest_setting(editor, "editor.snippetSuggestions"));
        items.sort_by(|a, b| compare_provided(a, b, snippet_sort));
        let context = Self::line_context(editor);
        let filter_graceful = self.suggest_setting(editor, "editor.suggest.filterGraceful") != Value::Bool(false);
        let fuzzy_options = FuzzyScoreOptions { first_match_can_be_weak: self.suggest_setting(editor, "editor.suggest.matchOnWordStartOnly") == Value::Bool(false), ..FuzzyScoreOptions::default() };
        let session = self.suggest.session.as_mut().expect("checked above");
        let line_context = LineContext { leading_line_content: context.leading_line_content.clone(), character_count_delta: context.column as isize - session.context.column as isize };
        session.model = Some(CompletionModel::new(items, session.context.column, line_context, filter_graceful, snippet_sort, fuzzy_options));
        self.suggest_new_context(context, cx);
    }

    /// `_refilterCompletionItems`.
    fn refilter_suggest(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let context = Self::line_context(editor);
        if let Some(session) = self.suggest.session.as_mut() {
            session.options.refilter = true;
        }
        self.suggest_new_context(context, cx);
    }

    /// `_onNewContext`: the session follows the cursor on its line, retriggers for a new word or incomplete results,
    /// refilters otherwise, and ends when nothing is left to show.
    fn suggest_new_context(&mut self, context: Context_, cx: &mut Context<Self>) {
        let Some(session) = self.suggest.session.as_ref() else { return };
        let leading_whitespace = |text: &str| text.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        if context.line != session.context.line || leading_whitespace(&context.leading_line_content) != leading_whitespace(&session.context.leading_line_content) {
            self.cancel_suggest(false, cx);
            return;
        }
        let auto = session.options.auto;
        if context.column < session.context.column {
            match context.leading_word.is_empty() {
                false => self.trigger_suggest(TriggerOptions { retrigger: true, ..TriggerOptions::new(auto) }, cx),
                true => self.cancel_suggest(false, cx),
            }
            return;
        }
        let Some(model) = session.model.as_ref() else { return };
        if !context.leading_word.is_empty() && context.leading_word_start > session.context.leading_word_start {
            if self.should_auto_trigger() {
                let reuse = model.provider_items();
                self.trigger_suggest(TriggerOptions { retrigger: true, reuse, ..TriggerOptions::new(auto) }, cx);
            }
            return;
        }
        let incomplete = model.incomplete_providers();
        if context.column > session.context.column && !incomplete.is_empty() && !context.leading_word.is_empty() {
            let reuse = model.provider_items().into_iter().filter(|(provider, _)| !incomplete.contains(provider)).collect();
            self.trigger_suggest(TriggerOptions { trigger_kind: TRIGGER_FOR_INCOMPLETE, retrigger: true, provider_filter: incomplete, reuse, ..TriggerOptions::new(auto) }, cx);
            return;
        }
        let should_auto_trigger = self.should_auto_trigger();
        let session = self.suggest.session.as_mut().expect("checked above");
        let model = session.model.as_mut().expect("checked above");
        let old_line_context = model.line_context().clone();
        model.set_line_context(LineContext { leading_line_content: context.leading_line_content.clone(), character_count_delta: context.column as isize - session.context.column as isize });
        let mut frozen = false;
        if model.items().is_empty() {
            if should_auto_trigger && session.context.leading_word_start + session.context.leading_word.chars().count() < context.leading_word_start {
                self.trigger_suggest(TriggerOptions { retrigger: true, ..TriggerOptions::new(auto) }, cx);
                return;
            }
            if auto {
                self.cancel_suggest(false, cx);
                return;
            }
            model.set_line_context(old_line_context);
            frozen = !model.items().is_empty();
            if frozen && context.leading_word.is_empty() {
                self.cancel_suggest(false, cx);
                return;
            }
        }
        self.suggest_did_suggest(frozen, cx);
    }

    /// The controller's `onDidSuggest`: `editor.suggest.selectionMode` and the memory pick the focused item.
    fn suggest_did_suggest(&mut self, frozen: bool, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let selection_mode = self.suggest_setting(editor, "editor.suggest.selectionMode");
        let session = self.suggest.session.as_mut().expect("a session suggests");
        let options = session.options.clone();
        let index = select_first(session.model.as_mut().expect("a model suggests").items());
        let no_focus = options.auto
            && match selection_mode.as_str().unwrap_or("always") {
                "never" => true,
                "whenTriggerCharacter" => options.trigger_kind != TRIGGER_CHARACTER,
                "whenQuickSuggestion" => options.trigger_kind == TRIGGER_CHARACTER && !options.refilter,
                _ => false,
            };
        self.show_suggestions(index, frozen, options.auto, no_focus, cx);
    }

    /// The widget's `onDidFocus` for the controller: the commit characters of the focused item.
    pub(super) fn suggest_item_focused(&mut self, item: Option<&CompletionItem>) {
        self.suggest.commit_characters = item.map(|item| item.commit_characters.clone()).unwrap_or_default();
    }

    /// `SuggestModel.createSuggestFilter`: the kinds `editor.suggest.show*` and `editor.snippetSuggestions: none` hide.
    fn suggest_kind_filter(&self, editor: &OpenEditor) -> Vec<u64> {
        let shows = [
            ("showMethods", 0), ("showFunctions", 1), ("showConstructors", 2), ("showFields", 3), ("showVariables", 4), ("showClasses", 5),
            ("showStructs", 6), ("showInterfaces", 7), ("showModules", 8), ("showProperties", 9), ("showEvents", 10), ("showOperators", 11),
            ("showUnits", 12), ("showValues", 13), ("showConstants", 14), ("showEnums", 15), ("showEnumMembers", 16), ("showKeywords", 17),
            ("showWords", 18), ("showColors", 19), ("showFiles", 20), ("showReferences", 21), ("showColors", 22), ("showFolders", 23),
            ("showTypeParameters", 24), ("showSnippets", 28), ("showUsers", 25), ("showIssues", 26),
        ];
        let mut hidden: Vec<u64> = shows.iter().filter(|(key, _)| self.suggest_setting(editor, &format!("editor.suggest.{key}")) == Value::Bool(false)).map(|(_, kind)| *kind).collect();
        if self.suggest_setting(editor, "editor.snippetSuggestions") == "none" {
            hidden.push(crate::suggest::KIND_SNIPPET);
        }
        hidden
    }

    /// `WordBasedCompletionItemProvider` with `editorWebWorker.ts` `$textualSuggest`: the words of this model and,
    /// as `editor.wordBasedSuggestions` says, of the other models of its language or of all of them.
    fn word_based_completions(&self, editor: &OpenEditor, head: Position) -> Value {
        let mode = self.suggest_setting(editor, "editor.wordBasedSuggestions");
        let mode = mode.as_str().unwrap_or("matchingDocuments");
        if mode == "off" {
            return Value::Null;
        }
        let text = &editor.document().lines[head.line];
        let word = word_at(text, head.column);
        let current: Option<String> = word.clone().map(|word| text.chars().skip(word.start).take(word.len()).collect());
        let mut models = vec![editor];
        if mode != "currentDocument" {
            for other in self.editors.editors.iter().chain(&self.editors.models) {
                if other.model_id != editor.model_id && !models.iter().any(|model| model.model_id == other.model_id) && (mode == "allDocuments" || other.document().language_id == editor.document().language_id) {
                    models.push(other);
                }
            }
        }
        let mut seen: Vec<String> = Vec::new();
        let mut known = std::collections::HashSet::new();
        'models: for model in models {
            for line in &model.document().lines {
                for word in line_words(line) {
                    if current.as_deref() == Some(word.as_str()) || word.parse::<f64>().is_ok() || !known.insert(word.clone()) {
                        continue;
                    }
                    seen.push(word);
                    if seen.len() > WORD_SUGGESTIONS_LIMIT {
                        break 'models;
                    }
                }
            }
        }
        let line_number = head.line + 1;
        let column = |chars: usize| utf16_column(text, chars) + 1;
        let (start, end) = word.map_or((head.column, head.column), |word| (word.start, word.end));
        let replace = json!({ "startLineNumber": line_number, "startColumn": column(start), "endLineNumber": line_number, "endColumn": column(end) });
        let insert = json!({ "startLineNumber": line_number, "startColumn": column(start), "endLineNumber": line_number, "endColumn": column(head.column) });
        let suggestions: Vec<Value> = seen.into_iter().map(|word| json!({ "a": word, "b": KIND_TEXT, "j": { "insert": insert, "replace": replace } })).collect();
        json!({ "a": { "insert": insert, "replace": replace }, "b": suggestions })
    }

    fn accept_selected_suggestion(&mut self, alternative: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.suggest.widget.focused.filter(|_| self.suggest_widget_visible()) else { return };
        let flags = InsertFlags { alternative_overwrite: alternative, ..InsertFlags::default() };
        self.insert_suggestion(index, flags, window, cx);
    }

    /// `_insertSuggestion`: the item's text (a snippet, or escaped into one) replaces the overwritten chars around each
    /// cursor, its additional edits apply now or once resolved, and its command runs.
    pub(super) fn insert_suggestion(&mut self, index: usize, flags: InsertFlags, window: &mut Window, cx: &mut Context<Self>) {
        let insert_mode = self.editors.active().map(|editor| self.suggest_setting(editor, "editor.suggest.insertMode")).unwrap_or_default();
        let Some(session) = self.suggest.session.as_mut() else { return };
        let Some(item) = session.model.as_mut().and_then(|model| model.items().get(index).cloned()) else { return };
        let line_suffix = session.line_suffix;
        let clipboard = session.options.clipboard.clone().or_else(|| cx.read_from_clipboard().and_then(|clipboard| clipboard.text()));
        let Some(editor) = self.editors.active_mut() else { return };
        if !flags.no_before_undo_stop {
            editor.push_undo_stop();
        }
        // getOverwriteInfo
        let replace = (insert_mode == "replace") != flags.alternative_overwrite;
        let head = editor.selection().head;
        let overwrite_before = item.position.column as isize - item.edit_start.column as isize + head.column as isize - item.position.column as isize;
        let suffix_delta = if head.line == item.position.line { (editor.document().lines[head.line].chars().count() as isize - line_suffix as isize - head.column as isize).max(0) } else { 0 };
        let end = if replace { item.edit_replace_end } else { item.edit_insert_end };
        let overwrite_after = end.column as isize - item.position.column as isize + suffix_delta;
        let mut change = None;
        let sync_edits = item.additional_text_edits.as_array().cloned();
        if let Some(edits) = &sync_edits {
            let column_delta = head.column as isize - item.position.column as isize;
            let edits: Vec<_> = edits
                .iter()
                .map(|edit| {
                    let (mut start, mut end) = range(editor, &edit["range"]);
                    // Shift an edit after the completion's position by what was typed since.
                    if start.line == item.position.line && start.column > item.position.column {
                        start.column = (start.column as isize + column_delta) as usize;
                        if end.line == start.line {
                            end.column = (end.column as isize + column_delta) as usize;
                        }
                    }
                    (start, end, edit["text"].as_str().unwrap_or_default().to_owned(), true)
                })
                .collect();
            change = editor.execute_edits(edits, None, EditReason::Unknown("suggestController.additionalTextEdits.sync"));
        }
        let template = match item.insert_text_rules & INSERT_AS_SNIPPET != 0 {
            true => item.insert_text.clone(),
            false => escape_snippet(&item.insert_text),
        };
        let keep_whitespace = item.insert_text_rules & 1 != 0;
        // snippetSession.ts `adjustSelection`: each cursor overwrites the same chars around it, within its line.
        let ranges: Vec<(Position, Position)> = editor
            .selections
            .iter()
            .map(|selection| {
                let line_len = editor.document().lines[selection.start().line].chars().count();
                let start = Position::new(selection.start().line, (selection.start().column as isize - overwrite_before).clamp(0, line_len as isize) as usize);
                let end_line_len = editor.document().lines[selection.end().line].chars().count();
                let end = Position::new(selection.end().line, (selection.end().column as isize + overwrite_after).clamp(0, end_line_len as isize) as usize);
                (start, end)
            })
            .collect();
        let model_id = editor.model_id;
        if change.is_some() {
            self.editor_edited(change, true, "suggest", cx);
        }
        self.cancel_suggest(false, cx);
        let snippets: Vec<(Position, Position, &str, bool)> = ranges.into_iter().map(|(start, end)| (start, end, template.as_str(), keep_whitespace)).collect();
        let insert = SnippetInsert { snippets: &snippets, clipboard: clipboard.as_deref(), undo_stop_before: false, undo_stop_after: !flags.no_after_undo_stop, reason: "suggest" };
        self.insert_snippets(insert, cx);
        if sync_edits.is_none() && !item.resolved {
            self.resolve_additional_edits(item.clone(), model_id, cx);
        }
        if let Some(command) = &item.command {
            let id = command["id"].as_str().unwrap_or_default().to_owned();
            if id == "editor.action.triggerSuggest" {
                self.trigger_suggest(TriggerOptions { retrigger: true, ..TriggerOptions::new(true) }, cx);
            } else {
                let args = command["arguments"].as_array().cloned().unwrap_or_default();
                drop(self.execute_command(&id, args, window, cx));
            }
        }
        self.clear_suggest();
    }

    /// The async branch of `_insertSuggestion`: an unresolved item's additional edits apply once it resolves, unless
    /// an edit before them happened meanwhile.
    fn resolve_additional_edits(&mut self, item: CompletionItem, model_id: u32, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let version = editor.text_model().version_id;
        let resolve = self.host.call_cancellable("ExtHostLanguageFeatures", "$resolveCompletionItem", vec![json!(item.provider), item.id.clone()]);
        cx.spawn(async move |workbench, cx| {
            let Ok(Ok(resolved)) = resolve.await else { return };
            let _ = workbench.update(cx, |workbench, cx| {
                let Some(edits) = resolved["l"].as_array().filter(|edits| !edits.is_empty()) else { return };
                let Some(editor) = workbench.editors.active_mut().filter(|editor| editor.model_id == model_id) else { return };
                // shortcut: any edit since the insert drops the edits, where upstream keeps them when the edits were
                // after the earliest changed position; track change positions when an extension needs it.
                if editor.text_model().version_id != version + 1 {
                    return;
                }
                let edits = edits.iter().map(|edit| {
                    let (start, end) = range(editor, &edit["range"]);
                    (start, end, edit["text"].as_str().unwrap_or_default().to_owned(), true)
                });
                let change = editor.execute_edits(edits.collect(), None, EditReason::Unknown("suggestController.additionalTextEdits.async"));
                editor.push_undo_stop();
                workbench.editor_edited(change, true, "suggest", cx);
            });
        })
        .detach();
    }

    /// suggest.ts `_executeCompletionItemProvider`: every provider's items at a position of a model, resolved up to
    /// `maxItemsToResolve`, as a `CompletionList`.
    // shortcut: without the snippet support's items, which need the model active; add them when an API test needs it.
    fn execute_completion_item_provider(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let uri = args.first().cloned().unwrap_or_default();
        let Some(editor) = uri_parts(&uri).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path)) else {
            return Outcome::Ready(Err(crate::commands::error_value("model not found")));
        };
        let position = args.get(1).cloned().unwrap_or_default();
        let trigger_character = args.get(2).and_then(Value::as_str).map(String::from);
        let max_resolve = args.get(3).and_then(Value::as_u64).unwrap_or(0) as usize;
        let document_uri = editor.document().uri();
        let candidate = Candidate { uri: &document_uri, language: &editor.document().language_id };
        let context = match &trigger_character {
            Some(character) => json!({ "triggerKind": TRIGGER_CHARACTER, "triggerCharacter": character }),
            None => json!({ "triggerKind": INVOKE }),
        };
        let position_range = json!({ "startLineNumber": position["lineNumber"], "startColumn": position["column"], "endLineNumber": position["lineNumber"], "endColumn": position["column"] });
        let model_position = range(editor, &position_range).0;
        let mut groups = Vec::new();
        for group in self.language_features.ordered_groups("$registerCompletionsProvider", &candidate) {
            let mut calls = Vec::new();
            for registration in group {
                let supports_resolve = registration.args.get(3) == Some(&Value::Bool(true));
                if registration.handle == WORD_BASED_PROVIDER {
                    let (sender, receiver) = oneshot::channel();
                    let _ = sender.send(Ok(self.word_based_completions(editor, model_position)));
                    calls.push((registration.handle, false, receiver));
                    continue;
                }
                let call_args = vec![json!(registration.handle), document_uri.clone(), position.clone(), context.clone()];
                calls.push((registration.handle, supports_resolve, self.host.call_cancellable("ExtHostLanguageFeatures", "$provideCompletionItems", call_args)));
            }
            groups.push(calls);
        }
        let (sender, receiver) = oneshot::channel();
        let host_call = |workbench: &gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp, method: &'static str, args: Vec<Value>| workbench.update(cx, |workbench, _| workbench.host.call_cancellable("ExtHostLanguageFeatures", method, args)).ok();
        cx.spawn(async move |workbench, cx| {
                let mut suggestions = Vec::new();
                let mut incomplete = false;
                let mut releases = Vec::new();
                for group in groups {
                    let mut added = false;
                    for (handle, supports_resolve, call) in group {
                        let Ok(Ok(result)) = call.await else { continue };
                        incomplete |= result["c"] == Value::Bool(true);
                        if let Some(id) = result["x"].as_u64() {
                            releases.push((handle, id));
                        }
                        for dto in result["b"].as_array().into_iter().flatten() {
                            added = true;
                            let mut dto = dto.clone();
                            if supports_resolve && suggestions.len() < max_resolve
                                && let Some(call) = host_call(&workbench, cx, "$resolveCompletionItem", vec![json!(handle), dto["x"].clone()])
                                && let Ok(Ok(resolved)) = call.await
                            {
                                merge_dto(&mut dto, &resolved);
                            }
                            suggestions.push(completion_item_value(&dto, &result["a"]));
                        }
                    }
                    if added {
                        break;
                    }
                }
                for (handle, id) in releases {
                    drop(host_call(&workbench, cx, "$releaseCompletionItems", vec![json!(handle), json!(id)]));
                }
                let _ = sender.send(Ok(json!({ "incomplete": incomplete, "suggestions": suggestions })));
            })
            .detach();
        Outcome::Pending(receiver)
    }
}

enum Call {
    Reuse(Vec<CompletionItem>),
    Ready(ProviderAnswer),
    Host(u64, bool, oneshot::Receiver<Result<Value, Value>>),
}

/// `InsertFlags` of suggestController.ts.
#[derive(Clone, Copy, Default)]
pub(super) struct InsertFlags {
    pub no_before_undo_stop: bool,
    pub no_after_undo_stop: bool,
    pub alternative_overwrite: bool,
}

/// The words of a line as the default word pattern splits it.
fn line_words(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut words = Vec::new();
    let mut column = 0;
    while column < chars.len() {
        match word_at(line, column).filter(|word| word.start == column) {
            Some(word) => {
                words.push(chars[word.clone()].iter().collect());
                column = word.end;
            }
            None => column += 1,
        }
    }
    words
}

fn snippet_sort(value: &Value) -> SnippetSort {
    match value.as_str() {
        Some("top") => SnippetSort::Top,
        Some("bottom") => SnippetSort::Bottom,
        _ => SnippetSort::Inline,
    }
}

/// `QuickSuggestionsOptions`: a bool is every kind on or off; an object names `other`, `comments` and `strings`.
fn quick_suggestions_value(value: &Value) -> &str {
    match value {
        Value::Bool(true) => "on",
        Value::Bool(false) => "off",
        Value::String(text) => text,
        _ => "off",
    }
}

fn quick_suggestions_all_off(config: &Value) -> bool {
    match config {
        Value::Object(_) => ["other", "comments", "strings"].iter().all(|key| quick_suggestions_value(&config[*key]) == "off"),
        value => quick_suggestions_value(value) == "off",
    }
}

fn quick_suggestions_all_on(config: &Value) -> bool {
    match config {
        Value::Object(_) => ["other", "comments", "strings"].iter().all(|key| quick_suggestions_value(&config[*key]) == "on"),
        value => quick_suggestions_value(value) == "on",
    }
}

/// A resolved DTO's fields over the provided one's.
fn merge_dto(dto: &mut Value, resolved: &Value) {
    let (Some(dto), Some(resolved)) = (dto.as_object_mut(), resolved.as_object()) else { return };
    for (key, value) in resolved {
        if key != "j" && !value.is_null() {
            dto.insert(key.clone(), value.clone());
        }
    }
}

/// `_inflateSuggestDto` as the API converter reads it back (`typeConvert.CompletionItem.to` takes the main thread's
/// `languages.CompletionItem`).
fn completion_item_value(dto: &Value, default_ranges: &Value) -> Value {
    let label = dto["a"].clone();
    let insert_text = dto["h"].as_str().map(String::from).unwrap_or_else(|| label.as_str().map(String::from).unwrap_or_else(|| label["label"].as_str().unwrap_or_default().to_owned()));
    let mut item = json!({
        "label": label,
        "kind": dto["b"].as_u64().unwrap_or(crate::suggest::KIND_PROPERTY),
        "insertText": insert_text,
        "range": if dto["j"].is_object() { dto["j"].clone() } else { default_ranges.clone() },
    });
    let fields = [("c", "detail"), ("d", "documentation"), ("e", "sortText"), ("f", "filterText"), ("g", "preselect"), ("i", "insertTextRules"), ("l", "additionalTextEdits"), ("m", "tags")];
    for (short, name) in fields {
        if !dto[short].is_null() {
            item[name] = dto[short].clone();
        }
    }
    if let Some(chars) = dto["k"].as_str() {
        item["commitCharacters"] = json!(chars.chars().map(String::from).collect::<Vec<_>>());
    }
    if let Some(id) = dto["o"].as_str() {
        item["command"] = json!({ "id": id, "title": "", "arguments": dto["p"].clone() });
    }
    item
}

#[cfg(test)]
mod tests;
