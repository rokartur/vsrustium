//! terminalSuggestAddon.ts with simpleSuggestWidget.ts: completions for the prompt input from the extension host's
//! terminal completion providers (`$registerCompletionProvider`), shown at the cursor with the suggest status bar.

#[cfg(test)]
mod tests;

use std::time::{Duration, Instant};

use futures::future::{Either, join_all};
use gpui::{AnyElement, Context, FontWeight, HighlightStyle, MouseButton, ScrollWheelEvent, StyledText, Task, Window, anchored, deferred, div, point, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::terminal_view::grid;
use crate::commands::Outcome;
use crate::filters::create_matches;
use crate::terminal::completion_item::{TerminalCompletion, TerminalCompletionItem, TerminalCompletionItemKind};
use crate::terminal::completion_model::{LineContext, TerminalCompletionModel};
use crate::terminal::completion_service::{self, CompletionProviders, TerminalCompletionList, TerminalCompletionProvider};

const UI_FONT: &str = ".SystemUIFont";
/// simpleSuggestWidget.ts `_getLayoutInfo`.
const DEFAULT_WIDTH: f32 = 430.;
const VISIBLE_ITEMS: usize = 12;
const VERTICAL_PADDING: f32 = 22.;
const HORIZONTAL_PADDING: f32 = 14.;
/// simpleSuggestWidgetDetails.ts `defaultMinSize` width.
const DETAILS_WIDTH: f32 = 220.;
/// `showTriggered`: "Loading..." after 250 ms of an explicit request.
const LOADING_DELAY: Duration = Duration::from_millis(250);
/// terminalSuggestShownTracker.ts.
const DISCOVERABILITY_KEY: &str = "terminal.suggest.increasedDiscoverability";
const DISCOVERABILITY_COUNT_KEY: &str = "terminal.suggest.increasedDiscoverabilityCount";
const DISCOVERABILITY_MAX_COUNT: u64 = 10;
const DISCOVERABILITY_MIN: Duration = Duration::from_secs(10);
const LEARN_MORE_URL: &str = "https://aka.ms/vscode-terminal-intellisense";

/// `IPromptInputModelState`, indices in chars.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct PromptState {
    pub value: String,
    pub prefix: String,
    pub cursor: usize,
    pub ghost: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum State {
    #[default]
    Hidden,
    Loading,
    Empty,
    Open,
}

#[derive(Default)]
pub(super) struct TerminalSuggest {
    pub providers: CompletionProviders,
    /// The instance the widget and the request belong to.
    instance: Option<u32>,
    state: State,
    model: Option<TerminalCompletionModel>,
    focused: Option<usize>,
    top: usize,
    explicitly_invoked: bool,
    has_navigated: bool,
    /// The list was navigated since it was last shown: the partial selection style is gone.
    partial_cleared: bool,
    details: bool,
    /// The provider requests in flight; dropping the task cancels them.
    request: Option<Task<()>>,
    loading: Option<Task<()>>,
    requested_index: usize,
    cursor_delta: isize,
    leading_line_content: Option<String>,
    current: Option<PromptState>,
    most_recent: Option<PromptState>,
    is_filtering_directories: bool,
    path_separator: Option<char>,
    last_user_data: String,
    last_user_data_at: Option<Instant>,
    last_accepted_at: Option<Instant>,
    request_on_next_sync: bool,
    /// terminalSuggestShownTracker.ts: the status bar's first action is highlighted until shown 10 times or for 10 s.
    discoverability_done: Option<bool>,
    discoverability_started: Option<Instant>,
}

impl TerminalSuggest {
    fn visible(&self) -> bool {
        self.state != State::Hidden
    }

    fn items_len(&mut self) -> usize {
        self.model.as_mut().map_or(0, |model| model.items().len())
    }

    /// `_onData`: what the user last typed, for the arrow key and Tab checks of `_sync`.
    pub fn user_data(&mut self, data: &str) {
        self.last_user_data = data.to_owned();
        self.last_user_data_at = Some(Instant::now());
    }

    fn last_input_arrow(&self) -> bool {
        // `_wasLastInputArrowKey`: xterm sends CSI A..D, or SS3 A..D in application cursor mode.
        matches!(self.last_user_data.as_str(), "\x1b[A" | "\x1b[B" | "\x1b[C" | "\x1b[D" | "\x1bOA" | "\x1bOB" | "\x1bOC" | "\x1bOD")
    }

    fn last_input_right_arrow(&self) -> bool {
        matches!(self.last_user_data.as_str(), "\x1b[C" | "\x1bOC")
    }
}

impl Workbench {
    pub(super) fn register_terminal_suggest_actions(&mut self) {
        self.commands.register("workbench.action.terminal.triggerSuggest", |workbench, _, window, cx| {
            workbench.request_terminal_completions(true, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.selectPrevSuggestion", |workbench, _, _, cx| workbench.move_terminal_suggestion(-1, false, cx));
        self.commands.register("workbench.action.terminal.selectNextSuggestion", |workbench, _, _, cx| workbench.move_terminal_suggestion(1, false, cx));
        self.commands.register("workbench.action.terminal.selectPrevPageSuggestion", |workbench, _, _, cx| workbench.move_terminal_suggestion(-1, true, cx));
        self.commands.register("workbench.action.terminal.selectNextPageSuggestion", |workbench, _, _, cx| workbench.move_terminal_suggestion(1, true, cx));
        self.commands.register("workbench.action.terminal.acceptSelectedSuggestion", |workbench, _, _, cx| {
            workbench.accept_terminal_suggestion(None, false, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.acceptSelectedSuggestionEnter", |workbench, _, _, cx| {
            workbench.accept_terminal_suggestion(None, true, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.hideSuggestWidget", |workbench, _, _, cx| {
            workbench.hide_terminal_suggest(true);
            cx.notify();
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.hideSuggestWidgetAndNavigateHistory", |workbench, _, _, cx| {
            workbench.hide_terminal_suggest(true);
            if let Some(instance) = workbench.terminal_view.active_instance_mut() {
                instance.send_text("\x1b[A", false);
            }
            cx.notify();
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.suggestToggleDetails", |workbench, _, _, cx| {
            let suggest = &mut workbench.terminal_view.suggest;
            if suggest.visible() {
                suggest.details = !suggest.details;
            }
            cx.notify();
            Outcome::done()
        });
        // The details panel takes no focus in the port; toggling focus shows or hides it as the toggle does.
        self.commands.register("workbench.action.terminal.suggestToggleDetailsFocus", |workbench, _, _, cx| {
            let suggest = &mut workbench.terminal_view.suggest;
            if suggest.visible() {
                suggest.details = !suggest.details;
            }
            cx.notify();
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.resetSuggestWidgetSize", |_, _, _, _| Outcome::done());
        self.commands.register("workbench.action.terminal.changeSelectionModeNever", |workbench, _, _, _| workbench.set_terminal_suggest_setting("selectionMode", json!("partial")));
        self.commands.register("workbench.action.terminal.changeSelectionModePartial", |workbench, _, _, _| workbench.set_terminal_suggest_setting("selectionMode", json!("always")));
        self.commands.register("workbench.action.terminal.changeSelectionModeAlways", |workbench, _, _, _| workbench.set_terminal_suggest_setting("selectionMode", json!("never")));
        self.commands.register("workbench.action.terminal.doNotShowSuggestOnType", |workbench, _, _, _| {
            workbench.update_setting("terminal.integrated.suggest.quickSuggestions", json!({ "commands": "off", "arguments": "off", "unknown": "off" }));
            workbench.set_terminal_suggest_setting("suggestOnTriggerCharacters", json!(false))
        });
        self.commands.register("workbench.action.terminal.showSuggestOnType", |workbench, _, _, _| {
            workbench.update_setting("terminal.integrated.suggest.quickSuggestions", json!({ "commands": "on", "arguments": "on", "unknown": "off" }));
            workbench.set_terminal_suggest_setting("suggestOnTriggerCharacters", json!(true))
        });
        self.commands.register("workbench.action.terminal.learnMore", |workbench, _, window, cx| {
            workbench.execute_command("vscode.open", vec![json!(LEARN_MORE_URL)], window, cx)
        });
        self.commands.register("workbench.action.terminal.configureSuggestSettings", |workbench, _, window, cx| {
            workbench.execute_command("workbench.action.openSettings", vec![json!("terminal.integrated.suggest")], window, cx)
        });
    }

    fn set_terminal_suggest_setting(&mut self, key: &str, value: Value) -> Outcome {
        self.update_setting(&format!("terminal.integrated.suggest.{key}"), value);
        Outcome::done()
    }

    /// mainThreadTerminalService.ts `$registerCompletionProvider` and `$unregisterCompletionProvider`.
    pub(super) fn register_terminal_completion_provider(&mut self, args: &[Value]) {
        let id = args.first().and_then(Value::as_str).unwrap_or_default().to_owned();
        let extension = args.get(1).and_then(Value::as_str).unwrap_or_default().to_owned();
        let trigger_characters = args.iter().skip(2).filter_map(Value::as_str).map(str::to_owned).collect();
        self.terminal_view.suggest.providers.register(&extension, TerminalCompletionProvider { id, trigger_characters, shell_types: None, is_builtin: false });
    }

    pub(super) fn unregister_terminal_completion_provider(&mut self, id: &str) {
        self.terminal_view.suggest.providers.unregister_provider(id);
    }

    /// The prompt input of an instance as `IPromptInputModelState`; `None` while a command runs.
    fn terminal_prompt_state(&self, id: u32) -> Option<PromptState> {
        let input = &self.terminal_view.instance(id)?.prompt_input;
        let cursor = input.cursor_index()?;
        Some(PromptState { value: input.value().to_owned(), prefix: input.prefix().to_owned(), cursor, ghost: input.ghost_text_index() })
    }

    /// `requestCompletions` and `_handleCompletionProviders`: ask every enabled provider, then show the model.
    pub(super) fn request_terminal_completions(&mut self, explicitly_invoked: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.setting("terminal.integrated.suggest.enabled") != true || !self.terminal_view.focus.is_focused(window) {
            return;
        }
        let Some(id) = self.terminal_view.active_instance_id() else { return };
        let Some(state) = self.terminal_prompt_state(id) else { return };
        let shell_type = self.terminal_view.instance(id).and_then(|instance| instance.shell_type);
        let provider_config = self.setting("terminal.integrated.suggest.providers");
        let cdpath = self.setting("terminal.integrated.suggest.cdPath").as_str().unwrap_or("absolute").to_owned();
        let suggest = &mut self.terminal_view.suggest;
        // A key must be pressed after an accepted completion before extensions are asked again.
        let skip_extensions = !state.value.is_empty() && suggest.last_user_data_at < suggest.last_accepted_at;
        suggest.instance = Some(id);
        suggest.requested_index = state.cursor;
        suggest.current = Some(state.clone());
        if explicitly_invoked && suggest.state == State::Hidden {
            suggest.explicitly_invoked = true;
            suggest.loading = Some(cx.spawn(async |workbench, cx| {
                cx.background_executor().timer(LOADING_DELAY).await;
                let _ = workbench.update(cx, |workbench, cx| {
                    workbench.terminal_view.suggest.state = State::Loading;
                    cx.notify();
                });
            }));
        }
        let ghost = suggest.most_recent.as_ref().and_then(|state| state.ghost);
        let prompt_value: String = match ghost {
            Some(ghost) => state.value.chars().take(ghost).collect(),
            None => state.value.clone(),
        };
        let providers: Vec<TerminalCompletionProvider> = suggest.providers.providers_to_request(&prompt_value, state.cursor, false, skip_extensions, shell_type, &provider_config).into_iter().cloned().collect();
        let context = json!({ "commandLine": prompt_value, "cursorIndex": utf16_len(&prompt_value, state.cursor) });
        let calls: Vec<_> = providers.iter().map(|provider| self.host.call_cancellable("ExtHostTerminalService", "$provideTerminalCompletions", vec![json!(provider.id), context.clone()])).collect();
        // `_collectCompletions`: a provider that takes longer than this gives nothing.
        let timeout = Duration::from_millis(if explicitly_invoked { 30_000 } else { 5_000 });
        self.terminal_view.suggest.request = Some(cx.spawn(async move |workbench, cx| {
            let timed = calls.into_iter().map(|call| {
                let timer = cx.background_executor().timer(timeout);
                async move {
                    match futures::future::select(call, timer).await {
                        Either::Left((Ok(Ok(dto)), _)) => Some(dto),
                        _ => None,
                    }
                }
            });
            let answers = join_all(timed).await;
            let _ = workbench.update(cx, |workbench, cx| {
                let mut results = Vec::new();
                for (provider, answer) in providers.iter().zip(answers) {
                    if let Some(dto) = answer.filter(Value::is_object) {
                        results.push((provider, TerminalCompletionList::from_dto(&dto, &provider.id, &prompt_value)));
                    }
                }
                let env = |name: &str| std::env::var(name).ok();
                let completions = completion_service::collect(results, &prompt_value, state.cursor, shell_type, &env, &cdpath);
                workbench.show_terminal_completions(id, completions, explicitly_invoked, cx);
                cx.notify();
            });
        }));
    }

    /// The rest of `_handleCompletionProviders` and `_showCompletions`.
    fn show_terminal_completions(&mut self, id: u32, completions: Vec<TerminalCompletion>, explicitly_invoked: bool, cx: &mut Context<Self>) {
        let Some(now) = self.terminal_prompt_state(id) else { return };
        let shell_type = self.terminal_view.instance(id).and_then(|instance| instance.shell_type);
        let inline_setting = self.setting("terminal.integrated.suggest.inlineSuggestion");
        let suggest = &mut self.terminal_view.suggest;
        suggest.request = None;
        suggest.loading = None;
        suggest.cursor_delta = now.cursor as isize - suggest.requested_index as isize;
        let end = (suggest.requested_index as isize + suggest.cursor_delta).max(0) as usize;
        let mut leading: String = now.prefix.chars().take(end).collect();
        if !explicitly_invoked && completions.is_empty() {
            self.hide_terminal_suggest(true);
            return;
        }
        // A TabExpansion2 result.
        if leading.contains(' ') || leading.starts_with('[') {
            leading = now.prefix.clone();
        }
        suggest.leading_line_content = Some(leading.clone());
        let first_folder = completions.iter().find(|completion| completion.kind == Some(TerminalCompletionItemKind::Folder));
        suggest.is_filtering_directories = first_folder.is_some();
        let mut normalized = leading;
        if let Some(folder) = first_folder {
            if let Some(separator) = folder.label.chars().find(|char| *char == '/' || *char == '\\') {
                suggest.path_separator = Some(separator);
            }
            if let Some(separator) = suggest.path_separator {
                normalized = normalize_path_separator(&normalized, separator);
            }
        }
        let path_separator = suggest.path_separator;
        let mut completions = completions;
        let inline = is_inline_completion_supported(shell_type).then(|| inline_completion_item(&now, suggest.cursor_delta, &inline_setting, path_separator, &mut completions));
        let items: Vec<TerminalCompletionItem> = completions.into_iter().filter(|completion| !completion.label.is_empty()).map(|completion| TerminalCompletionItem::new(completion, path_separator)).chain(inline).collect();
        let mut model = TerminalCompletionModel::new(items, LineContext::new(normalized, suggest.cursor_delta.max(0) as usize));
        suggest.current = Some(now);
        if !explicitly_invoked && model.items().is_empty() {
            return;
        }
        suggest.model = Some(model);
        self.show_terminal_suggestions(!explicitly_invoked, cx);
    }

    /// `showSuggestions(0, false, isAuto)`.
    fn show_terminal_suggestions(&mut self, auto: bool, cx: &mut Context<Self>) {
        let selection_mode = self.setting("terminal.integrated.suggest.selectionMode");
        let suggest = &mut self.terminal_view.suggest;
        suggest.loading = None;
        let explicitly = suggest.explicitly_invoked;
        if suggest.items_len() == 0 {
            suggest.state = if auto { State::Hidden } else { State::Empty };
            suggest.model = None;
            return;
        }
        let was_hidden = suggest.state == State::Hidden || suggest.state == State::Loading;
        suggest.state = State::Open;
        suggest.top = 0;
        // Explicitly invoked always focuses the first item, else `never` focuses none.
        suggest.focused = (explicitly || selection_mode != "never").then_some(0);
        suggest.partial_cleared = false;
        if was_hidden {
            suggest.has_navigated = false;
            self.update_terminal_suggest_discoverability(cx);
        }
    }

    /// terminalSuggestShownTracker.ts `update`.
    fn update_terminal_suggest_discoverability(&mut self, cx: &mut Context<Self>) {
        let done = match self.terminal_view.suggest.discoverability_done {
            Some(done) => done,
            None => self.profile_storage.lock().unwrap().get(DISCOVERABILITY_KEY) == Some("true"),
        };
        self.terminal_view.suggest.discoverability_done = Some(done);
        if done {
            return;
        }
        let mut storage = self.profile_storage.lock().unwrap();
        let count = storage.get(DISCOVERABILITY_COUNT_KEY).and_then(|count| count.parse::<u64>().ok()).unwrap_or(0) + 1;
        storage.set(DISCOVERABILITY_COUNT_KEY, count.to_string());
        if count >= DISCOVERABILITY_MAX_COUNT {
            storage.set(DISCOVERABILITY_KEY, "true".to_owned());
            self.terminal_view.suggest.discoverability_done = Some(true);
            return;
        }
        drop(storage);
        if self.terminal_view.suggest.discoverability_started.is_none() {
            self.terminal_view.suggest.discoverability_started = Some(Instant::now());
            cx.spawn(async |workbench, cx| {
                cx.background_executor().timer(DISCOVERABILITY_MIN).await;
                let _ = workbench.update(cx, |workbench, cx| {
                    workbench.profile_storage.lock().unwrap().set(DISCOVERABILITY_KEY, "true".to_owned());
                    workbench.terminal_view.suggest.discoverability_done = Some(true);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// `hideSuggestWidget`.
    pub(super) fn hide_terminal_suggest(&mut self, cancel_any_request: bool) {
        let suggest = &mut self.terminal_view.suggest;
        if cancel_any_request {
            suggest.request = None;
        }
        suggest.loading = None;
        suggest.discoverability_started = None;
        suggest.current = None;
        suggest.leading_line_content = None;
        suggest.state = State::Hidden;
        suggest.model = None;
        suggest.focused = None;
        suggest.explicitly_invoked = false;
        suggest.has_navigated = false;
        suggest.details = false;
    }

    /// `_sync` on `onDidChangeInput` of the instance's prompt input model.
    pub(super) fn sync_terminal_suggest(&mut self, id: u32, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_view.active_instance_id() != Some(id) {
            return;
        }
        let Some(state) = self.terminal_prompt_state(id) else {
            if self.terminal_view.suggest.visible() {
                self.hide_terminal_suggest(true);
            }
            return;
        };
        let quick = self.setting("terminal.integrated.suggest.quickSuggestions");
        let shell_type = self.terminal_view.instance(id).and_then(|instance| instance.shell_type);
        let inline_setting = self.setting("terminal.integrated.suggest.inlineSuggestion");
        let on_trigger_characters = self.setting("terminal.integrated.suggest.suggestOnTriggerCharacters") == true;
        let suggest = &mut self.terminal_view.suggest;
        let mut request = false;
        if suggest.request_on_next_sync {
            suggest.request_on_next_sync = false;
            request = true;
        }
        let previous = suggest.most_recent.clone();
        if previous.as_ref().is_none_or(|previous| state.cursor > previous.cursor) {
            let has_space = state.prefix.trim().contains(char::is_whitespace);
            let quick_on = if has_space { quick["arguments"] == "on" } else { quick["commands"] == "on" };
            if !suggest.visible() && quick_on && state.prefix.ends_with(|char: char| !char.is_whitespace()) {
                request = true;
            }
            if on_trigger_characters && !request {
                let prefix = &state.prefix;
                let dash_after_space = prefix.ends_with(" -") || prefix.ends_with("\t-");
                let directory = suggest.is_filtering_directories && (prefix.ends_with('/') || prefix.ends_with('\\'));
                request = dash_after_space || directory || suggest.providers.providers().any(|provider| provider.trigger_characters.iter().any(|char| prefix.ends_with(char.as_str())));
            }
        }
        if let Some(previous) = &previous
            && state.cursor < previous.cursor
            && state.cursor > 0
            && suggest.visible()
            && on_trigger_characters
            && !request
            && let Some(char) = previous.value.chars().nth(previous.cursor - 1)
        {
            let directory = suggest.is_filtering_directories && (char == '/' || char == '\\');
            request = directory || suggest.providers.providers().any(|provider| provider.trigger_characters.iter().any(|trigger| trigger.chars().eq([char])));
        }
        if suggest.last_input_right_arrow() && previous.as_ref().is_some_and(|previous| previous.ghost.is_some() && previous.value == state.value) && state.ghost.is_none() {
            self.hide_terminal_suggest(false);
        }
        let suggest = &mut self.terminal_view.suggest;
        suggest.most_recent = Some(state.clone());
        if request {
            self.request_terminal_completions(false, window, cx);
            return;
        }
        let suggest = &mut self.terminal_view.suggest;
        let Some(leading_length) = suggest.leading_line_content.as_ref().map(|leading| leading.chars().count()) else { return };
        if suggest.instance != Some(id) || !suggest.visible() && suggest.model.is_none() {
            return;
        }
        let previous_current = suggest.current.replace(state.clone());
        // The latest character is a space.
        if state.cursor > 1 && state.value.chars().nth(state.cursor - 1) == Some(' ') && !suggest.last_input_arrow() {
            self.hide_terminal_suggest(false);
            return;
        }
        // The cursor moved left over whitespace or a separator: the completions no longer apply.
        if state.cursor < leading_length {
            let separator = previous_current.and_then(|previous| previous.value.chars().nth(state.cursor)).is_some_and(|char| char == '/' || char == '\\' || char.is_whitespace());
            if state.cursor == 0 || separator {
                self.hide_terminal_suggest(false);
                return;
            }
        }
        if suggest.visible() {
            suggest.cursor_delta = state.cursor as isize - suggest.requested_index as isize;
            let end = (suggest.requested_index as isize + suggest.cursor_delta).max(0) as usize;
            let mut leading: String = state.value.chars().take(end).collect();
            if suggest.is_filtering_directories
                && let Some(separator) = suggest.path_separator
            {
                leading = normalize_path_separator(&leading, separator);
            }
            let delta = suggest.cursor_delta.max(0) as usize;
            if let Some(model) = &mut suggest.model {
                model.set_line_context(LineContext::new(leading, delta));
            }
        }
        if is_inline_completion_supported(shell_type)
            && let Some(model) = &mut suggest.model
        {
            let mut completions: Vec<TerminalCompletion> = model.items().iter().map(|item| item.completion.clone()).collect();
            let item = inline_completion_item(&state, suggest.cursor_delta, &inline_setting, suggest.path_separator, &mut completions);
            model.set_inline_item(item);
        }
        if suggest.items_len() == 0 {
            self.hide_terminal_suggest(false);
            return;
        }
        if suggest.visible() {
            self.show_terminal_suggestions(true, cx);
        }
    }

    fn move_terminal_suggestion(&mut self, direction: isize, page: bool, cx: &mut Context<Self>) -> Outcome {
        let suggest = &mut self.terminal_view.suggest;
        let count = suggest.items_len();
        if count == 0 {
            return Outcome::done();
        }
        let step = if page { VISIBLE_ITEMS as isize - 1 } else { 1 };
        let index = match (suggest.focused, page) {
            // `focusNext(1, loop)` wraps; a page move stops at the ends.
            (None, _) if direction > 0 => 0,
            (None, _) => count as isize - 1,
            (Some(index), false) => (index as isize + direction).rem_euclid(count as isize),
            (Some(index), true) => (index as isize + direction * step).clamp(0, count as isize - 1),
        } as usize;
        suggest.focused = Some(index);
        suggest.has_navigated |= index != 0;
        // `_clearPartialSelectionState`.
        suggest.partial_cleared = true;
        if index < suggest.top {
            suggest.top = index;
        } else if index >= suggest.top + VISIBLE_ITEMS {
            suggest.top = index + 1 - VISIBLE_ITEMS;
        }
        cx.notify();
        Outcome::done()
    }

    /// `acceptSelectedSuggestion`: the keys that turn the typed word into the completion.
    fn accept_terminal_suggestion(&mut self, index: Option<usize>, respect_run_on_enter: bool, cx: &mut Context<Self>) {
        let run_on_enter_setting = self.setting("terminal.integrated.suggest.runOnEnter");
        let insert_trailing_space = self.setting("terminal.integrated.suggest.insertTrailingSpace") == true;
        let suggest = &mut self.terminal_view.suggest;
        let Some(index) = index.or(suggest.focused) else { return };
        let Some(initial) = suggest.most_recent.clone() else { return };
        if suggest.leading_line_content.is_none() {
            return;
        }
        let Some(completion) = suggest.model.as_mut().and_then(|model| model.items().get(index)).map(|item| item.completion.clone()) else { return };
        let Some(id) = suggest.instance else { return };
        suggest.last_accepted_at = Some(Instant::now());
        let current = suggest.current.clone().unwrap_or(initial);
        let sequence = accept_sequence(&completion, &current, respect_run_on_enter.then(|| run_on_enter_setting.as_str().unwrap_or("never")), insert_trailing_space);
        if completion.kind == Some(TerminalCompletionItemKind::Folder) {
            suggest.last_accepted_at = None;
        }
        if insert_trailing_space && !matches!(completion.kind, Some(TerminalCompletionItemKind::Folder | TerminalCompletionItemKind::SymbolicLinkFolder)) {
            suggest.last_user_data_at = Some(Instant::now());
            suggest.request_on_next_sync = true;
        }
        self.hide_terminal_suggest(true);
        if let Some(instance) = self.terminal_view.instance_mut(id) {
            instance.send_text(&sequence, false);
        }
        cx.notify();
    }

    /// terminalSuggest and simpleSuggestWidget context keys.
    pub(super) fn terminal_suggest_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        let suggest = &self.terminal_view.suggest;
        context.insert("terminalSuggestWidgetVisible".into(), json!(suggest.visible()));
        context.insert("simpleSuggestWidgetHasFocusedSuggestion".into(), json!(suggest.visible() && suggest.focused.is_some()));
        context.insert("simpleSuggestWidgetHasNavigated".into(), json!(suggest.has_navigated));
        context.insert("simpleSuggestWidgetExplicitlyInvoked".into(), json!(suggest.explicitly_invoked));
        context.insert("simpleSuggestWidgetFirstSuggestionFocused".into(), json!(suggest.focused == Some(0)));
    }

    /// The widget at the cursor of its instance's pane: `_layout` places it below the cursor, above when there is
    /// more room there, and right-aligned to the cursor when it would leave the window on the right.
    pub(super) fn terminal_suggest_widget(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let suggest = &self.terminal_view.suggest;
        if suggest.state == State::Hidden {
            return None;
        }
        let id = suggest.instance?;
        let bounds = *self.terminal_view.pane_bounds.get(&id)?;
        let metrics = self.terminal_view.metrics.clone()?;
        let instance = self.terminal_view.instance(id)?;
        let cursor = instance.term.grid().cursor.point;
        let row = cursor.line.0 + instance.term.grid().display_offset() as i32;
        let rows = alacritty_terminal::grid::Dimensions::screen_lines(&instance.term);
        let origin = grid::grid_origin(bounds, rows, &metrics);
        let cursor_left = f32::from(origin.x) + cursor.column.0 as f32 * metrics.cell_width;
        let cursor_top = f32::from(origin.y) + row as f32 * metrics.cell_height;
        let item_height = metrics.cell_height.clamp(8., 1000.);
        let show_status_bar = self.setting("terminal.integrated.suggest.showStatusBar") == true;
        let state = suggest.state;
        let message = matches!(state, State::Loading | State::Empty);
        let status_height = if show_status_bar && !message { item_height } else { 0. };
        let count = suggest.model.as_ref().map_or(0, |model| model.filtered_items().len());
        let window_size = window.viewport_size();
        let (window_width, window_height) = (f32::from(window_size.width), f32::from(window_size.height));
        let list_height = if message { item_height } else { count.min(VISIBLE_ITEMS) as f32 * item_height };
        let full_height = status_height + list_height + 2.;
        let width = if message { DEFAULT_WIDTH / 2. } else { DEFAULT_WIDTH.min(window_width - 2. - 2. * HORIZONTAL_PADDING) };
        let below = (window_height - (cursor_top + item_height) - VERTICAL_PADDING).min(full_height);
        let above = (cursor_top - VERTICAL_PADDING).min(full_height);
        let height = full_height.min(below.max(above) + 2.).max(item_height + status_height);
        let place_above = height > below && above > below;
        let height = height.min(if place_above { above } else { below }.max(item_height + status_height));
        let left = if cursor_left + width > window_width { cursor_left - width } else { cursor_left };
        let top = if place_above { cursor_top - height } else { cursor_top + item_height };
        let widget = self.terminal_suggest_widget_body(width, height, status_height, item_height, &metrics.family, metrics.font_size, cx);
        let details = self.terminal_suggest_details(item_height, cx);
        let content = div().flex().items_start().child(widget).children(details);
        Some(deferred(anchored().position(point(px(left), px(top))).child(content)).with_priority(1).into_any_element())
    }

    #[allow(clippy::too_many_arguments)]
    fn terminal_suggest_widget_body(&self, width: f32, height: f32, status_height: f32, item_height: f32, family: &gpui::SharedString, font_size: f32, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let state = self.terminal_view.suggest.state;
        let mut widget = div()
            .id("terminal-suggest-widget")
            .w(px(width))
            .h(px(height))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(3.))
            .border_1()
            .border_color(self.color("editorSuggestWidget.border"))
            .bg(self.color("editorSuggestWidget.background"))
            .text_color(self.color("editorSuggestWidget.foreground"))
            .font_family(family.clone())
            .text_size(px(font_size))
            .line_height(px(item_height))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        match state {
            State::Loading => return widget.child(div().px(px(5.)).opacity(0.7).font_family(UI_FONT).child("Loading...")),
            State::Empty => return widget.child(div().px(px(5.)).opacity(0.7).font_family(UI_FONT).child("No suggestions.")),
            _ => {}
        }
        widget = widget.child(self.terminal_suggest_rows(height - status_height - 2., item_height, cx));
        if status_height > 0. {
            widget = widget.child(self.terminal_suggest_status_bar(status_height, cx));
        }
        widget
    }

    fn terminal_suggest_rows(&self, height: f32, item_height: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = ((height / item_height).floor() as usize).max(1);
        let selection_mode = self.setting("terminal.integrated.suggest.selectionMode");
        let suggest = &self.terminal_view.suggest;
        let partial = !suggest.explicitly_invoked && !suggest.partial_cleared && selection_mode == "partial";
        let (top, focused) = (suggest.top, suggest.focused);
        let Some(model) = suggest.model.as_ref() else { return div().id("terminal-suggest-list") };
        let items = model.filtered_items();
        let count = items.len();
        let mut list = div()
            .id("terminal-suggest-list")
            .flex()
            .flex_col()
            .h(px(height))
            .on_scroll_wheel(cx.listener(move |workbench, event: &ScrollWheelEvent, window, cx| {
                let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
                let rows = (-delta / item_height).round() as isize;
                let suggest = &mut workbench.terminal_view.suggest;
                suggest.top = (suggest.top as isize + rows).clamp(0, count.saturating_sub(visible) as isize) as usize;
                cx.stop_propagation();
                cx.notify();
            }));
        for (offset, item) in items.iter().skip(top).take(visible).enumerate() {
            let index = top + offset;
            list = list.child(self.terminal_suggest_row(item, index, focused == Some(index), partial, item_height, cx));
        }
        list
    }

    /// simpleSuggestWidgetRenderer.ts `renderElement`.
    fn terminal_suggest_row(&self, item: &TerminalCompletionItem, index: usize, focused: bool, partial: bool, item_height: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let completion = &item.completion;
        let selected = focused && !partial;
        let foreground = if selected { self.foreground_or("editorSuggestWidget.selectedForeground") } else { self.foreground_or("editorSuggestWidget.foreground") };
        let (codicon, color_key) = completion_icon(completion);
        let icon_color = match (selected, color_key) {
            (true, _) => self.theme.color("editorSuggestWidget.selectedIconForeground").unwrap_or(foreground),
            (false, Some(key)) => self.foreground_or(key),
            (false, None) => foreground,
        };
        let highlight = if selected { self.color("editorSuggestWidget.focusHighlightForeground") } else { self.color("editorSuggestWidget.highlightForeground") };
        let label = completion.label.replace(['\r', '\n'], "");
        let mut styles = Vec::new();
        for range in create_matches(Some(&item.score)) {
            let start = label.char_indices().nth(range.start).map_or(label.len(), |(offset, _)| offset);
            let end = label.char_indices().nth(range.end).map_or(label.len(), |(offset, _)| offset);
            styles.push((start..end, HighlightStyle { color: Some(highlight.into()), font_weight: Some(FontWeight::BOLD), ..Default::default() }));
        }
        let (parameters, details) = match completion.string_label {
            true => (None, completion.detail.clone()),
            false => (completion.label_detail.clone(), completion.label_description.clone()),
        };
        let left = div()
            .flex()
            .flex_grow(1.)
            .flex_shrink(1.)
            .overflow_hidden()
            .child(div().flex().items_center().mr(px(4.)).child(self.icon(codicon, 16., 16., icon_color)))
            .child(div().flex_none().child(StyledText::new(label).with_highlights(styles)))
            .children(parameters.filter(|parameters| !parameters.is_empty()).map(|parameters| div().ml(px(4.)).overflow_hidden().text_ellipsis().opacity(0.7).child(parameters.replace(['\r', '\n'], ""))));
        let right = div()
            .flex()
            .flex_shrink(4.)
            .max_w(gpui::relative(0.7))
            .overflow_hidden()
            .children(details.filter(|details| !details.is_empty()).map(|details| div().ml(px(12.)).opacity(0.4).text_size(gpui::rems(0.85)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(details.replace(['\r', '\n'], ""))));
        let focus_border = self.color("focusBorder");
        let outline = self.theme.color("contrastActiveBorder");
        div()
            .id(("terminal-suggest-row", index))
            .flex()
            .flex_none()
            .h(px(item_height))
            .pr(px(10.))
            .whitespace_nowrap()
            .cursor_pointer()
            .text_color(foreground)
            .when(selected, |row| row.bg(self.color("editorSuggestWidget.selectedBackground")).when_some(outline, |row, outline| row.border_1().border_color(outline)))
            .when(focused && partial, |row| row.border_1().border_color(focus_border))
            .when(!focused, |row| row.hover(|row| row.bg(self.color("list.hoverBackground"))))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _, _, cx| {
                cx.stop_propagation();
                workbench.terminal_view.suggest.focused = Some(index);
                workbench.accept_terminal_suggestion(Some(index), false, cx);
            }))
            .child(div().flex_1().h_full().overflow_hidden().pl(px(2.)).child(div().flex().items_center().h_full().justify_between().overflow_hidden().child(left).child(right)))
    }

    /// `SuggestWidgetStatus` over `MenubarTerminalSuggestStatusMenu`: the selection mode at the left while
    /// suggestions show on type, the on-type toggle, Learn More and Configure at the right.
    fn terminal_suggest_status_bar(&self, height: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let quick = self.setting("terminal.integrated.suggest.quickSuggestions");
        let on_trigger = self.setting("terminal.integrated.suggest.suggestOnTriggerCharacters");
        // A context key of an object setting is `true`.
        let shows_on_type = (!quick.is_null() && quick != false) || on_trigger == true;
        let mode = self.setting("terminal.integrated.suggest.selectionMode");
        let (mode_title, mode_command) = match mode.as_str() {
            Some("never") => ("Selection Mode: None", "workbench.action.terminal.changeSelectionModeNever"),
            Some("always") => ("Selection Mode: Always (Tab or Enter)", "workbench.action.terminal.changeSelectionModeAlways"),
            _ => ("Selection Mode: Partial (Tab)", "workbench.action.terminal.changeSelectionModePartial"),
        };
        let quick_on = quick["commands"] == "on" || quick["arguments"] == "on";
        let on_type_action = if quick_on && on_trigger == true {
            Some(("eye", "workbench.action.terminal.doNotShowSuggestOnType"))
        } else if (quick["commands"] == "off" && quick["arguments"] == "off") || on_trigger == false {
            Some(("eye-closed", "workbench.action.terminal.showSuggestOnType"))
        } else {
            None
        };
        let status = self.color("editorSuggestWidgetStatus.foreground");
        let discoverable = self.terminal_view.suggest.discoverability_done == Some(false);
        let mut right = div().flex().items_center().h_full();
        let actions = on_type_action.into_iter().chain([("question", "workbench.action.terminal.learnMore"), ("gear", "workbench.action.terminal.configureSuggestSettings")]);
        for (position, (icon, command)) in actions.enumerate() {
            let highlighted = discoverable && position == 0;
            let color = if highlighted { self.color("button.foreground") } else { status };
            right = right.child(
                div()
                    .id(command)
                    .flex()
                    .items_center()
                    .justify_center()
                    .h_full()
                    .px(px(3.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .when(highlighted, |action| action.bg(self.color("button.background")))
                    .when(!highlighted, |action| action.hover(|action| action.bg(self.color("toolbar.hoverBackground"))))
                    .child(self.icon(icon, 16., 16., color))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |workbench, _, window, cx| {
                        drop(workbench.execute_command(command, Vec::new(), window, cx));
                    })),
            );
        }
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .h(px(height))
            .px(px(4.))
            .border_t_1()
            .border_color(self.color("editorSuggestWidget.border"))
            .font_family(UI_FONT)
            .text_size(px(13. * 0.8))
            .text_color(status)
            .child(
                div()
                    .id("terminal-suggest-selection-mode")
                    .pr(px(8.))
                    .cursor_pointer()
                    .when(shows_on_type, |left| left.child(mode_title))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |workbench, _, window, cx| {
                        drop(workbench.execute_command(mode_command, Vec::new(), window, cx));
                    })),
            )
            .child(right)
    }

    /// simpleSuggestWidgetDetails.ts east of the list at its minimum size: the focused item's detail and documentation.
    fn terminal_suggest_details(&self, line_height: f32, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let suggest = &self.terminal_view.suggest;
        if !suggest.details || suggest.state != State::Open {
            return None;
        }
        let index = suggest.focused?;
        let completion = suggest.model.as_ref()?.filtered_items().get(index)?.completion.clone();
        let detail = completion.detail.filter(|detail| *detail != completion.label);
        if detail.is_none() && completion.documentation.is_none() {
            return None;
        }
        Some(
            div()
                .id("terminal-suggest-details")
                .relative()
                .w(px(DETAILS_WIDTH))
                .h(px(2. * line_height + 2.))
                .overflow_hidden()
                .border_1()
                .border_color(self.color("editorSuggestWidget.border"))
                .bg(self.color("editorSuggestWidget.background"))
                .text_color(self.color("editorSuggestWidget.foreground"))
                .font_family(UI_FONT)
                .text_size(px(13.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .pl(px(5.))
                        .pr(px(line_height + 4.))
                        .py(px(4.))
                        .children(detail.map(|detail| div().opacity(0.7).font_family(self.terminal_view.metrics.as_ref().map(|metrics| metrics.family.clone()).unwrap_or_default()).child(detail)))
                        .children(completion.documentation.map(|documentation| div().child(documentation))),
                )
                .child(
                    div()
                        .id("terminal-suggest-details-close")
                        .absolute()
                        .top_0()
                        .right_0()
                        .size(px(line_height))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .child(self.icon("close", line_height, 16., self.color("editorSuggestWidget.foreground")))
                        .on_click(cx.listener(|workbench, _, _, cx| {
                            workbench.terminal_view.suggest.details = false;
                            cx.notify();
                        })),
                ),
        )
    }
}

/// The codicon and its color key: the item's own icon, else its kind's (`_kindToIconMap`, terminalSymbolIcons.ts).
fn completion_icon(completion: &TerminalCompletion) -> (&str, Option<&'static str>) {
    use TerminalCompletionItemKind::*;
    if let Some(icon) = &completion.icon {
        return (icon.as_str(), None);
    }
    let Some(kind) = completion.kind else { return ("symbol-key", Some("terminalSymbolIcon.symbolText")) };
    match kind {
        File => ("symbol-file", Some("terminalSymbolIcon.fileForeground")),
        Folder => ("symbol-folder", Some("terminalSymbolIcon.folderForeground")),
        SymbolicLinkFile => ("file-symlink-file", Some("terminalSymbolIcon.symbolicLinkFileForeground")),
        SymbolicLinkFolder => ("file-symlink-directory", Some("terminalSymbolIcon.symbolicLinkFolderForeground")),
        Method => ("symbol-method", Some("terminalSymbolIcon.methodForeground")),
        Alias => ("symbol-method", Some("terminalSymbolIcon.aliasForeground")),
        Argument => ("symbol-variable", Some("terminalSymbolIcon.argumentForeground")),
        Option => ("symbol-enum", Some("terminalSymbolIcon.optionForeground")),
        OptionValue => ("symbol-enum-member", Some("terminalSymbolIcon.optionValueForeground")),
        Flag => ("flag", Some("terminalSymbolIcon.flagForeground")),
        Commit => ("git-commit", Some("terminalSymbolIcon.commitForeground")),
        Branch => ("git-branch", Some("terminalSymbolIcon.branchForeground")),
        Tag => ("tag", Some("terminalSymbolIcon.tagForeground")),
        Stash => ("git-stash", Some("terminalSymbolIcon.stashForeground")),
        Remote => ("remote", Some("terminalSymbolIcon.remoteForeground")),
        PullRequest => ("git-pull-request", Some("terminalSymbolIcon.pullRequestForeground")),
        PullRequestDone => ("git-pull-request-done", Some("terminalSymbolIcon.pullRequestDoneForeground")),
        InlineSuggestion | InlineSuggestionAlwaysOnTop => ("star", Some("terminalSymbolIcon.inlineSuggestionForeground")),
    }
}

/// terminalSuggestAddon.ts `isInlineCompletionSupported`.
pub(super) fn is_inline_completion_supported(shell_type: Option<&str>) -> bool {
    matches!(shell_type, Some("bash" | "zsh" | "fish" | "pwsh" | "gitbash"))
}

/// `_inlineCompletionItem` after `_refreshInlineCompletion`: the shell's ghost text from the word it completes,
/// accepted with a right arrow; invalid without ghost text or with `inlineSuggestion` off. A completion with the
/// same label lends it its label, detail and documentation and leaves `completions`.
fn inline_completion_item(state: &PromptState, cursor_delta: isize, setting: &Value, path_separator: Option<char>, completions: &mut Vec<TerminalCompletion>) -> TerminalCompletionItem {
    let kind = if setting == "alwaysOnTopExceptExactMatch" { TerminalCompletionItemKind::InlineSuggestion } else { TerminalCompletionItemKind::InlineSuggestionAlwaysOnTop };
    let mut completion = TerminalCompletion {
        input_data: Some("\x1b[C".into()),
        replacement_range: Some((0, 0)),
        detail: Some("Inline suggestion".into()),
        kind: Some(kind),
        ..TerminalCompletion::new("", "core:inlineSuggestion")
    };
    let Some(ghost) = state.ghost.filter(|_| setting != "off") else {
        let mut item = TerminalCompletionItem::new(completion, path_separator);
        item.is_invalid = true;
        return item;
    };
    let chars: Vec<char> = state.value.chars().collect();
    let replacement_index = chars[..ghost.min(chars.len())].iter().rposition(|char| *char == ' ').map_or(0, |space| space + 1);
    completion.label = chars[replacement_index..].iter().collect();
    completion.replacement_range = Some((replacement_index, (state.cursor as isize - cursor_delta).max(0) as usize));
    let label = completion.label.trim().to_owned();
    match completions.iter().position(|rich| rich.label == label) {
        Some(index) => {
            let rich = completions.remove(index);
            completion.label = rich.label;
            completion.string_label = rich.string_label;
            completion.label_description = rich.label_description;
            completion.label_detail = rich.label_detail;
            completion.detail = rich.detail;
            completion.documentation = rich.documentation;
        }
        None => {
            completion.detail = None;
            completion.documentation = None;
        }
    }
    TerminalCompletionItem::new(completion, path_separator)
}

/// The keys `acceptSelectedSuggestion` sends: backspaces over the typed part, deletes over the rest of the word,
/// then the rest of the completion (or right arrows over text that already matches), `\r` when `run_on_enter` says so.
pub(super) fn accept_sequence(completion: &TerminalCompletion, current: &PromptState, run_on_enter: Option<&str>, insert_trailing_space: bool) -> String {
    let chars: Vec<char> = current.value.chars().collect();
    let substring = |start: usize, end: usize| -> String { chars[start.min(chars.len())..end.min(chars.len()).max(start.min(chars.len()))].iter().collect() };
    let start = completion.replacement_range.map_or(current.cursor, |(start, _)| start);
    let replacement = substring(start, current.cursor);
    let mut right_side = String::new();
    let ghost_after_cursor = current.ghost.is_none_or(|ghost| ghost > current.cursor);
    if ghost_after_cursor && chars.len() > current.cursor + 1 && chars.get(current.cursor) != Some(&' ') {
        let end = current.ghost.unwrap_or(chars.len());
        let rest = substring(current.cursor, end);
        right_side = match rest.find(' ') {
            Some(space) => rest[..space].to_owned(),
            None => rest,
        };
    }
    let mut sequence = match &completion.input_data {
        Some(input) => input.clone(),
        None => {
            let mut text = completion.label.clone();
            let is_file = completion.is_file_override == Some(true);
            if (completion.kind == Some(TerminalCompletionItemKind::Folder) || is_file) && text.contains(' ') {
                text = text.replace(' ', "\\ ");
            }
            let lower = |text: &str| text.to_lowercase();
            let run = match run_on_enter {
                Some("always") => true,
                Some("exactMatch") => lower(&replacement) == lower(&text),
                Some("exactMatchIgnoreExtension") => {
                    let without_extension = regex::Regex::new(r"\.[^.]+$").unwrap().replace(&lower(&text), "").into_owned();
                    lower(&replacement) == lower(&text) || (is_file && lower(&replacement) == without_extension)
                }
                _ => false,
            };
            let common = common_prefix_length(&replacement, &text);
            let replacement_chars: Vec<char> = replacement.chars().collect();
            let common_end = replacement_chars.len().saturating_sub(1);
            let common_prefix: String = replacement_chars[common_end.saturating_sub(common)..common_end].iter().collect();
            let suffix: String = text.chars().skip(common).collect();
            let prompt_suffix = substring(current.cursor, current.ghost.unwrap_or(chars.len()));
            if !prompt_suffix.is_empty() && current.prefix.ends_with(&common_prefix) && prompt_suffix.starts_with(&suffix) {
                "\x1bOC".repeat(text.chars().count() - common)
            } else {
                format!("{}{}{}{}", "\x7f".repeat(replacement.chars().count() - common), "\x1b[3~".repeat(right_side.chars().count()), suffix, if run { "\r" } else { "" })
            }
        }
    };
    if insert_trailing_space && !matches!(completion.kind, Some(TerminalCompletionItemKind::Folder | TerminalCompletionItemKind::SymbolicLinkFolder)) {
        sequence.push(' ');
    }
    sequence
}

/// strings.ts `commonPrefixLength`, case-sensitive, in chars.
fn common_prefix_length(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(a, b)| a == b).count()
}

/// terminalSuggestAddon.ts `normalizePathSeparator`.
fn normalize_path_separator(path: &str, separator: char) -> String {
    match separator {
        '\\' => path.replace('/', "\\"),
        _ => path.replace('\\', "/"),
    }
}

/// The UTF-16 length of the first `chars` chars, the extension host's string index.
fn utf16_len(text: &str, chars: usize) -> usize {
    text.chars().take(chars).map(char::len_utf16).sum()
}
