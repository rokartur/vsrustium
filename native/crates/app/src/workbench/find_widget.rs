//! The editor find widget (findWidget.ts, findWidget.css, findInput.ts, replaceInput.ts, toggle.css): the find
//! and replace inputs with their option toggles, the match count, the buttons and the resize sash, drawn over the
//! editor's top right corner. The controller (editor/find/controller.rs) owns what it searches for.

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant};

use gpui::{
    BoxShadow, Context, Div, FocusHandle, HighlightStyle, KeyDownEvent, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Rgba, Stateful, Task, Window, canvas, div, point, prelude::*, px,
};
use serde_json::Value;

use super::Workbench;
use super::editor_view::selected_columns;
use super::input_box_view::{InputId, InputLook, InputView};
use crate::commands::Outcome;
use crate::editor::find::controller::{AutoFindInSelection, FindController, FindOptions, FocusAction, QueryOptions, SeedFromSelection};
use crate::editor::find::multicursor::{self, MultiCursorContext, SelectionHighlightOptions, SelectionHighlightQuery};
use crate::editor::find::state::{MATCHES_LIMIT, NewFindState};
use crate::editor::{ContentChanged, OpenEditor, Position, Selection, VERTICAL_SCROLLBAR_WIDTH, Viewport, expand_tabs, visible_column};
use crate::history::HistoryNavigator;
use crate::input_box::InputBox;
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Event, Kind, QuickInput};
use crate::theme::ThemeKind;

const FIND_WIDGET_INITIAL_WIDTH: f32 = 419.;
/// `new HistoryNavigator<string>(options.history, 100)` of `HistoryInputBox`.
pub(super) const HISTORY_LIMIT: usize = 100;
const FIND_HISTORY_KEY: &str = "workbench.find.history";
const REPLACE_HISTORY_KEY: &str = "workbench.replace.history";
/// The workspace storage keys of `saveQueryState`, in `QueryOptions` field order.
const QUERY_STATE_KEYS: [&str; 4] = ["editor.matchCase", "editor.wholeWord", "editor.isRegex", "editor.preserveCase"];
const MAX_MATCHES_COUNT_WIDTH: f32 = 69.;
/// `_getHeight`: 4 + the input's 23 + its 2px border + 4, plus 4 + 25 with the replace input.
const FIND_INPUT_AREA_HEIGHT: f32 = 33.;
const REPLACE_AREA_HEIGHT: f32 = 62.;
/// findWidget.css `transition: transform 200ms linear`.
const SLIDE: Duration = Duration::from_millis(200);
/// `--vscode-shadow-lg` (style.css) blurs 12px; CSS takes sigma as half the blur, GPUI's blur_radius is sigma.
const SHADOW_BLUR: f32 = 6.;
/// `workbench.sash.hoverDelay` default.
const SASH_HOVER_DELAY: Duration = Duration::from_millis(300);
const BUTTON_IDS: [&str; 7] = ["find-replace-toggle", "find-previous", "find-next", "find-selection", "find-close", "replace", "replace-all"];

/// A find match's overview ruler mark, also drawn in the minimap unless past 1000 matches.
pub(super) struct FindMark {
    pub range: Range<Position>,
    pub in_minimap: bool,
    pub current: bool,
}

pub struct FindWidget {
    pub controller: FindController,
    pub find_input: InputBox,
    pub replace_input: InputBox,
    pub find_view: InputView,
    pub replace_view: InputView,
    pub find_focus: FocusHandle,
    pub replace_focus: FocusHandle,
    /// The `SimpleButton`s and the Find in Selection toggle by id: a click focuses them (tabIndex 0).
    pub button_focus: HashMap<&'static str, FocusHandle>,
    /// The model the controller is bound to; another active model restarts the find there (`onDidChangeModel`).
    model_id: Option<u32>,
    /// The width the sash set (`_resized`).
    resized_width: Option<f32>,
    /// The widget width and pointer x when a sash drag started.
    sash_drag: Option<(f32, f32)>,
    /// The pointer over the sash, shown as hovered once `workbench.sash.hoverDelay` passed.
    sash_hover: Option<Task<()>>,
    sash_hovered: bool,
    /// When the widget last showed or hid, for its slide.
    pub(super) toggled: Option<Instant>,
    /// The widget's left edge as last drawn, from where the text's column 0 is.
    widget_left: Cell<f32>,
    /// The match Go to Match draws as a range highlight while its input is open (`addDecorations`).
    pub go_to_match_highlight: Option<Range<Position>>,
    /// The inputs' histories (`FindWidgetSearchHistory`, `ReplaceWidgetHistory`), stored per workspace.
    pub find_history: HistoryNavigator,
    pub replace_history: HistoryNavigator,
    /// `_updateHistoryDelayer`: the histories take the inputs' values 500 ms after the last state change.
    history_update: Option<Task<()>>,
    /// multicursor.ts `SelectionHighlighter`: the matches of the selected text and what they were found for.
    pub selection_highlights: Option<(SelectionHighlightKey, Vec<Range<Position>>)>,
    /// findOptionsWidget.ts while it shows, and findInput.ts `highlightFindOptions` flashing the input's options.
    pub(super) options_widget: super::find_options_widget::FindOptionsWidget,
}

/// The model, its version, the selections and the search the selection highlights were found for.
type SelectionHighlightKey = (u32, u32, Vec<Selection>, SelectionHighlightQuery);

impl FindWidget {
    pub fn new(cx: &mut Context<Workbench>) -> Self {
        FindWidget {
            controller: FindController::new(FindOptions::default(), QueryOptions::default()),
            find_input: InputBox::default(),
            replace_input: InputBox::default(),
            find_view: InputView::default(),
            replace_view: InputView::default(),
            find_focus: cx.focus_handle(),
            replace_focus: cx.focus_handle(),
            button_focus: BUTTON_IDS.into_iter().map(|id| (id, cx.focus_handle())).collect(),
            model_id: None,
            resized_width: None,
            sash_drag: None,
            sash_hover: None,
            sash_hovered: false,
            toggled: None,
            widget_left: Cell::new(f32::MAX),
            go_to_match_highlight: None,
            find_history: HistoryNavigator::new(Vec::new(), HISTORY_LIMIT),
            replace_history: HistoryNavigator::new(Vec::new(), HISTORY_LIMIT),
            history_update: None,
            selection_highlights: None,
            options_widget: Default::default(),
        }
    }
}

type FindAction = fn(&mut FindController, &mut OpenEditor, Viewport) -> (FocusAction, Option<ContentChanged>);

/// The find actions under their `FIND_IDS` (findModel.ts).
const FIND_COMMANDS: [(&str, FindAction); 16] = [
    ("actions.find", |find, editor, viewport| (find.start_find(editor, viewport), None)),
    ("editor.action.startFindReplaceAction", |find, editor, viewport| (find.start_find_replace(editor, viewport), None)),
    ("actions.findWithSelection", |find, editor, viewport| { find.start_find_with_selection(editor, viewport); (FocusAction::NoFocusChange, None) }),
    ("editor.action.nextMatchFindAction", |find, editor, viewport| { find.match_find_action(editor, viewport, true); (FocusAction::NoFocusChange, None) }),
    ("editor.action.previousMatchFindAction", |find, editor, viewport| { find.match_find_action(editor, viewport, false); (FocusAction::NoFocusChange, None) }),
    ("editor.action.nextSelectionMatchFindAction", |find, editor, viewport| { find.selection_match_find_action(editor, viewport, true); (FocusAction::NoFocusChange, None) }),
    ("editor.action.previousSelectionMatchFindAction", |find, editor, viewport| { find.selection_match_find_action(editor, viewport, false); (FocusAction::NoFocusChange, None) }),
    ("closeFindWidget", |find, editor, viewport| { find.close_find_widget(editor, viewport); (FocusAction::NoFocusChange, None) }),
    ("toggleFindCaseSensitive", |find, editor, viewport| { find.toggle_case_sensitive(editor, viewport); (FocusAction::NoFocusChange, None) }),
    ("toggleFindWholeWord", |find, editor, viewport| { find.toggle_whole_words(editor, viewport); (FocusAction::NoFocusChange, None) }),
    ("toggleFindRegex", |find, editor, viewport| { find.toggle_regex(editor, viewport); (FocusAction::NoFocusChange, None) }),
    ("toggleFindInSelection", |find, editor, viewport| { find.toggle_search_scope(editor, viewport); (FocusAction::NoFocusChange, None) }),
    ("togglePreserveCase", |find, editor, viewport| { find.toggle_preserve_case(editor, viewport); (FocusAction::NoFocusChange, None) }),
    ("editor.action.replaceOne", |find, editor, viewport| (FocusAction::NoFocusChange, find.replace(editor, viewport))),
    ("editor.action.replaceAll", |find, editor, viewport| (FocusAction::NoFocusChange, find.replace_all(editor, viewport))),
    ("editor.action.selectAllMatches", |find, editor, _| { find.select_all_matches(editor); (FocusAction::NoFocusChange, None) }),
];

impl Workbench {
    pub(super) fn register_find_commands(&mut self) {
        for (id, action) in FIND_COMMANDS {
            self.commands.register(id, move |workbench, _, window, cx| {
                // findController.ts: StartFindReplaceAction does not run on a read-only editor.
                if id == "editor.action.startFindReplaceAction" && workbench.active_editor_readonly().is_some() {
                    return Outcome::done();
                }
                workbench.run_find(window, cx, action);
                // `CommonFindController.selectAllMatches` focuses the editor.
                if id == "editor.action.selectAllMatches" {
                    window.focus(&workbench.editor_focus, cx);
                }
                Outcome::done()
            });
        }
        // multicursor.ts: (id, next, add).
        for (id, next, add) in [
            ("editor.action.addSelectionToNextFindMatch", true, true),
            ("editor.action.addSelectionToPreviousFindMatch", false, true),
            ("editor.action.moveSelectionToNextFindMatch", true, false),
            ("editor.action.moveSelectionToPreviousFindMatch", false, false),
        ] {
            self.commands.register(id, move |workbench, _, window, cx| {
                let context = workbench.multicursor_context(window);
                workbench.run_find(window, cx, |find, editor, viewport| {
                    multicursor::selection_to_find_match(find, editor, viewport, &context, next, add);
                    (FocusAction::NoFocusChange, None)
                });
                Outcome::done()
            });
        }
        for id in ["editor.action.selectHighlights", "editor.action.changeAll"] {
            self.commands.register(id, move |workbench, _, window, cx| {
                // `CompatChangeAll` precondition: a writable editor; its keybinding and palette entry check the text focus.
                if id == "editor.action.changeAll" && workbench.active_editor_readonly().is_some() {
                    return Outcome::done();
                }
                let context = workbench.multicursor_context(window);
                workbench.run_find(window, cx, |find, editor, viewport| {
                    multicursor::select_all(find, editor, viewport, &context);
                    (FocusAction::NoFocusChange, None)
                });
                Outcome::done()
            });
        }
        self.commands.register("editor.actions.findWithArgs", |workbench, args, window, cx| {
            let args = args.into_iter().next().filter(Value::is_object);
            let text = |key: &str| args.as_ref().and_then(|args| args[key].as_str().map(str::to_string));
            let flag = |key: &str| args.as_ref().and_then(|args| args[key].as_bool());
            let new = match &args {
                None => NewFindState::default(),
                Some(_) => NewFindState {
                    search_string: text("searchString"),
                    replace_string: text("replaceString"),
                    is_replace_revealed: Some(text("replaceString").is_some()),
                    is_regex: flag("isRegex"),
                    whole_word: flag("matchWholeWord"),
                    match_case: flag("isCaseSensitive"),
                    preserve_case: flag("preserveCase"),
                    ..NewFindState::default()
                },
            };
            let find_in_selection = flag("findInSelection").unwrap_or(false);
            workbench.run_find(window, cx, |find, editor, viewport| (find.start_find_with_args(editor, viewport, new, find_in_selection), None));
            Outcome::done()
        });
        self.commands.register("editor.action.goToMatchFindAction", |workbench, _, window, cx| workbench.show_go_to_match(window, cx));
        self.commands.register("history.showPrevious", |workbench, _, window, cx| workbench.show_history_value(true, window, cx));
        self.commands.register("history.showNext", |workbench, _, window, cx| workbench.show_history_value(false, window, cx));
    }

    /// `GoToMatchFindAction.run`: an input box that takes the match number, 1-based or negative from the end.
    fn show_go_to_match(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let matches_count = self.find.controller.state.matches_count;
        if matches_count < 1 {
            self.notify_warning("No matches. Try searching for something else.");
            cx.notify();
            return Outcome::done();
        }
        let mut input = QuickInput::new(0, Kind::InputBox);
        input.placeholder = Some(format!("Type a number to go to a specific match (between 1 and {matches_count})"));
        let id = self.quick_inputs.add(input, Owner::GoToMatch);
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
        Outcome::done()
    }

    pub(crate) fn go_to_match_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let matches_count = self.find.controller.state.matches_count;
        let invalid = format!("Please type a number between 1 and {matches_count}");
        match event {
            Event::ChangeValue(value) => {
                let index = find_match_index(&value, matches_count);
                if let Some(index) = index {
                    self.run_find(window, cx, |find, editor, viewport| { find.go_to_match(editor, viewport, index); (FocusAction::NoFocusChange, None) });
                }
                self.find.go_to_match_highlight = index.and_then(|_| self.find.controller.state.current_match.clone());
                let Some(input) = self.quick_inputs.get_mut(id) else { return };
                input.validation_message = index.is_none().then_some(invalid);
            }
            Event::Accept(_) => {
                let Some(input) = self.quick_inputs.get_mut(id) else { return };
                let Some(index) = find_match_index(&input.input_box.value, matches_count) else {
                    input.validation_message = Some(invalid);
                    return;
                };
                self.run_find(window, cx, |find, editor, viewport| { find.go_to_match(editor, viewport, index); (FocusAction::NoFocusChange, None) });
                self.hide_quick_input_by_id(id, window, cx);
            }
            Event::Hide => {
                self.find.go_to_match_highlight = None;
                self.quick_inputs.remove(id);
            }
            _ => {}
        }
        cx.notify();
    }

    /// Runs a controller action on the active editor, then tells the host what moved or changed, shows or hides
    /// the widget (`_onStateChanged` isRevealed) and moves the focus the action asks for.
    fn run_find(&mut self, window: &mut Window, cx: &mut Context<Self>, action: impl FnOnce(&mut FindController, &mut OpenEditor, Viewport) -> (FocusAction, Option<ContentChanged>)) {
        self.find.controller.options = self.find_options();
        self.find.controller.find_input_focused = self.find.find_focus.is_focused(window);
        self.find_editor_changed();
        let top_whitespace = self.editors.viewport.top_whitespace;
        let viewport = self.editors.viewport;
        let match_mode = self.selected_text_match_mode();
        let Some(editor) = self.editors.active_mut() else { return };
        let find = &mut self.find;
        let (selections, scroll, revealed) = (editor.selections.clone(), editor.scroll, find.controller.state.is_revealed);
        let options = (find.controller.state.is_regex, find.controller.state.whole_word, find.controller.state.match_case);
        let query = find.controller.query_options();
        // wordHighlighter.ts `_run`: without text focus the highlights keep the previous query's word.
        let highlighted_word = match self.editor_focus.is_focused(window) {
            true => None,
            false => Some(editor.highlights.first().map(|(line, columns)| editor.document.lines[*line].chars().skip(columns.start).take(columns.len()).collect::<String>())),
        };
        let (focus, change) = action(&mut find.controller, editor, viewport);
        multicursor::end_session_if_stale(&mut find.controller, editor, viewport, &match_mode);
        // An edit's cursor change stops the highlights; only a later find selection (source "api", which
        // refreshed them) runs the query again, so Replace All leaves none.
        let rerun = change.is_none() || !editor.highlights.is_empty();
        if let Some(word) = highlighted_word.filter(|_| rerun) {
            editor.highlights = word.map_or_else(Vec::new, |word| crate::editor::word_occurrences(&editor.document.lines, &word));
        }
        let adjust_scroll = !self.find.controller.state.is_revealed || revealed || !self.selection_under_find_widget(selections[0]);
        self.layout_find_zone(adjust_scroll);
        let moved = self.editors.active().expect("checked above").selections != selections;
        // findModel.ts reveals matches with `ScrollType.Smooth`.
        let scrolled = self.animate_scroll_from(scroll, cx) || self.editors.viewport.top_whitespace != top_whitespace;
        let editor = self.editors.active().expect("checked above");
        match &change {
            Some(change) => {
                self.host.send_content_change(&self.editors, self.editors.active_index().expect("checked above"), change);
                self.host.send_visible_ranges(&self.editors);
            }
            None if moved => self.host.send_selection(editor, "api"),
            None => {}
        }
        if scrolled {
            self.editor_scrolled(cx);
        }
        self.sync_find_inputs();
        let state = &self.find.controller.state;
        if state.is_revealed != revealed {
            self.find.toggled = Some(Instant::now());
            if state.is_revealed {
                self.blink_caret(InputId::Find, window, cx);
                self.blink_caret(InputId::Replace, window, cx);
            } else {
                self.find.find_view.reset();
                self.find.replace_view.reset();
                // `_hide(true)`; wordHighlighter.ts `onDidFocusEditorText` runs its query from the selection again.
                window.focus(&self.editor_focus, cx);
                if let Some(editor) = self.editors.active_mut() {
                    editor.refresh_highlights();
                }
            }
        }
        match focus {
            FocusAction::NoFocusChange => {}
            FocusAction::FocusFindInput => self.focus_find_input(InputId::Find, window, cx),
            FocusAction::FocusReplaceInput => self.focus_find_input(InputId::Replace, window, cx),
        }
        self.highlight_find_options(options, cx);
        self.save_query_state(query);
        self.schedule_find_history(window, cx);
        cx.notify();
    }

    /// `saveQueryState`: the options that changed, as workspace storage booleans.
    fn save_query_state(&self, before: QueryOptions) {
        let after = self.find.controller.query_options();
        let pairs = [(before.match_case, after.match_case), (before.whole_word, after.whole_word), (before.is_regex, after.is_regex), (before.preserve_case, after.preserve_case)];
        let mut storage = self.workspace_storage.lock().unwrap();
        for (key, (before, after)) in QUERY_STATE_KEYS.into_iter().zip(pairs) {
            if before != after {
                storage.set(key, after.to_string());
            }
        }
    }

    /// The options in workspace storage, each defaulting to `fallback`'s.
    fn stored_query_options(&self, fallback: QueryOptions) -> QueryOptions {
        let storage = self.workspace_storage.lock().unwrap();
        let stored = |key: &str, fallback: bool| storage.get(key).map_or(fallback, |value| value == "true");
        QueryOptions {
            match_case: stored(QUERY_STATE_KEYS[0], fallback.match_case),
            whole_word: stored(QUERY_STATE_KEYS[1], fallback.whole_word),
            is_regex: stored(QUERY_STATE_KEYS[2], fallback.is_regex),
            preserve_case: stored(QUERY_STATE_KEYS[3], fallback.preserve_case),
        }
    }

    /// The workspace's storage arrived: `loadQueryState` and the histories' `load` with `editor.find.history` and
    /// `editor.find.replaceHistory` at "workspace".
    pub(super) fn restore_find_state(&mut self) {
        let query = self.stored_query_options(self.find.controller.query_options());
        self.find.controller = FindController::new(self.find.controller.options, query);
        self.find.model_id = None;
        let stored_history = |workbench: &Self, setting: &str, key: &str| {
            let elements = match workbench.setting(setting).as_str() == Some("workspace") {
                true => workbench.workspace_storage.lock().unwrap().get(key).and_then(|raw| serde_json::from_str(raw).ok()),
                false => None,
            };
            HistoryNavigator::new(elements.unwrap_or_default(), HISTORY_LIMIT)
        };
        self.find.find_history = stored_history(self, "editor.find.history", FIND_HISTORY_KEY);
        self.find.replace_history = stored_history(self, "editor.find.replaceHistory", REPLACE_HISTORY_KEY);
    }

    fn schedule_find_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find.history_update = Some(cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            let _ = workbench.update(cx, |workbench, _| workbench.update_find_history());
        }));
    }

    /// `_updateHistory`: the inputs' values join their histories, saved for the workspace (`FindWidgetSearchHistory.save`).
    fn update_find_history(&mut self) {
        let find = &mut self.find;
        if !find.controller.state.search_string.is_empty() {
            find.find_history.add_to_history(&find.find_input.value, false);
        }
        if !find.controller.state.replace_string.is_empty() {
            find.replace_history.add_to_history(&find.replace_input.value, false);
        }
        self.save_find_histories();
    }

    fn save_find_histories(&self) {
        let mut storage = self.workspace_storage.lock().unwrap();
        for (setting, key, history) in [("editor.find.history", FIND_HISTORY_KEY, &self.find.find_history), ("editor.find.replaceHistory", REPLACE_HISTORY_KEY, &self.find.replace_history)] {
            if self.setting(setting).as_str() == Some("workspace") {
                storage.set(key, serde_json::to_string(history.elements()).expect("strings serialize"));
            }
        }
    }

    /// `history.showPrevious` and `history.showNext` on the focused find or replace input.
    fn show_history_value(&mut self, previous: bool, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(id) = self.focused_input(window) else { return Outcome::done() };
        if let InputId::Search(field) = id {
            self.show_search_history_value(field, previous, window, cx);
            return Outcome::done();
        }
        if let InputId::SearchEditor(field) = id {
            self.show_search_editor_history_value(field, previous, window, cx);
            return Outcome::done();
        }
        if !matches!(id, InputId::Find | InputId::Replace) {
            return Outcome::done();
        }
        let (input, history) = match id {
            InputId::Find => (&self.find.find_input, &mut self.find.find_history),
            _ => (&self.find.replace_input, &mut self.find.replace_history),
        };
        let value = match previous {
            true => history.show_previous_value(&input.value),
            false => Some(history.show_next_value(&input.value)),
        };
        self.save_find_histories();
        if let Some(value) = value {
            self.edit_find_input(id, |input| input.set_value(value), window, cx);
        }
        Outcome::done()
    }

    /// `focusFindInput` / `focusReplaceInput`: the value is selected.
    fn focus_find_input(&mut self, id: InputId, window: &mut Window, cx: &mut Context<Self>) {
        match id {
            InputId::Find => self.find.find_input.select_all(),
            _ => self.find.replace_input.select_all(),
        }
        window.focus(&self.input_focus(id).clone(), cx);
        self.blink_caret(id, window, cx);
    }

    /// The inputs show the state's strings when an action set them (`_onStateChanged` searchString, replaceString).
    fn sync_find_inputs(&mut self) {
        let find = &mut self.find;
        if find.find_input.value != find.controller.state.search_string {
            find.find_input.set_value(find.controller.state.search_string.clone());
        }
        if find.replace_input.value != find.controller.state.replace_string {
            find.replace_input.set_value(find.controller.state.replace_string.clone());
        }
    }

    /// `editor.find` settings the controller reads.
    fn find_options(&self) -> FindOptions {
        let seed = match self.setting("editor.find.seedSearchStringFromSelection") {
            Value::String(seed) if seed == "never" => SeedFromSelection::Never,
            Value::String(seed) if seed == "selection" => SeedFromSelection::Selection,
            _ => SeedFromSelection::Always,
        };
        let auto_find_in_selection = match self.setting("editor.find.autoFindInSelection") {
            Value::String(auto) if auto == "always" => AutoFindInSelection::Always,
            Value::String(auto) if auto == "multiline" => AutoFindInSelection::Multiline,
            _ => AutoFindInSelection::Never,
        };
        let is_loop = self.setting("editor.find.loop").as_bool().unwrap_or(true);
        let cursor_move_on_type = self.setting("editor.find.cursorMoveOnType").as_bool().unwrap_or(true);
        let global_find_clipboard = self.setting("editor.find.globalFindClipboard").as_bool().unwrap_or(false);
        FindOptions { seed_search_string_from_selection: seed, auto_find_in_selection, is_loop, global_find_clipboard, cursor_move_on_type, ..FindOptions::default() }
    }

    /// Typing in an input changes the state: the search string moves the cursor to the nearest match.
    pub(super) fn edit_find_input(&mut self, id: InputId, edit: impl FnOnce(&mut InputBox), window: &mut Window, cx: &mut Context<Self>) {
        let input = if id == InputId::Find { &mut self.find.find_input } else { &mut self.find.replace_input };
        let before = input.value.clone();
        edit(input);
        let value = input.value.clone();
        cx.notify();
        // `editor.find.findOnType` off: the search string waits for Enter (`find_input_key_down`).
        if value == before || (id == InputId::Find && self.setting("editor.find.findOnType") == Value::Bool(false)) {
            return;
        }
        let (new, move_cursor) = match id {
            InputId::Find => (NewFindState { search_string: Some(value), ..Default::default() }, true),
            _ => (NewFindState { replace_string: Some(value), ..Default::default() }, false),
        };
        self.run_find(window, cx, move |find, editor, viewport| {
            find.change(editor, viewport, new, move_cursor);
            (FocusAction::NoFocusChange, None)
        });
    }

    /// The find input's keydown, before the keybindings: with `editor.find.findOnType` off Enter searches.
    pub(super) fn find_input_key_down(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = keystroke.modifiers;
        if keystroke.key != "enter" || modifiers.modified() || !self.find.find_focus.is_focused(window) || self.setting("editor.find.findOnType") != Value::Bool(false) {
            return;
        }
        let new = NewFindState { search_string: Some(self.find.find_input.value.clone()), ..Default::default() };
        self.run_find(window, cx, move |find, editor, viewport| {
            find.change(editor, viewport, new, true);
            (FocusAction::NoFocusChange, None)
        });
    }

    /// `_onFindInputKeyDown` and `_onReplaceInputKeyDown`: Tab moves between the inputs, Cmd+Down to the editor.
    pub(super) fn find_input_key(&mut self, id: InputId, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let modifiers = keystroke.modifiers;
        match keystroke.key.as_str() {
            "down" if modifiers.platform && !modifiers.shift && !modifiers.alt => window.focus(&self.editor_focus, cx),
            "tab" if id == InputId::Find && !modifiers.shift && self.replace_visible() => self.focus_find_input(InputId::Replace, window, cx),
            "tab" if id == InputId::Replace && modifiers.shift => self.focus_find_input(InputId::Find, window, cx),
            _ => return false,
        }
        true
    }

    /// A newly active model rebinds the controller: a visible find searches it (`onDidChangeModel`).
    pub(super) fn find_editor_changed(&mut self) {
        let viewport = self.editors.viewport;
        let query = self.stored_query_options(QueryOptions::default());
        let Some(editor) = self.editors.active_mut() else {
            self.find.controller.model = None;
            self.find.model_id = None;
            return;
        };
        if self.find.model_id == Some(editor.model_id) {
            return;
        }
        self.find.model_id = Some(editor.model_id);
        self.find.controller.editor_changed(editor, viewport, query);
    }

    /// An edit the find did not make: its matches follow the text, and the next search starts at the cursor.
    /// The multi-cursor session ends unless it made the cursor change.
    pub(super) fn find_editor_edited(&mut self, change: Option<&ContentChanged>) {
        let viewport = self.editors.viewport;
        let match_mode = self.selected_text_match_mode();
        let Some(editor) = self.editors.active_mut() else { return };
        if let Some(change) = change {
            self.find.controller.content_changed(editor, viewport, change);
        }
        self.find.controller.cursor_moved(editor);
        multicursor::end_session_if_stale(&mut self.find.controller, editor, viewport, &match_mode);
    }

    fn selected_text_match_mode(&self) -> String {
        self.setting("editor.selectedTextMatchMode").as_str().unwrap_or("findOptions").to_string()
    }

    /// `SelectionHighlighter._update`, run on every render; the matches are searched again only when the query, the
    /// model version or the selections changed. Upstream waits 300 ms after an edit or an explicit empty cursor.
    pub(super) fn refresh_selection_highlights(&mut self, window: &Window) {
        let Some(editor) = self.editors.active() else {
            self.find.selection_highlights = None;
            return;
        };
        let options = SelectionHighlightOptions {
            enabled: self.setting("editor.selectionHighlight").as_bool().unwrap_or(true),
            multiline: self.setting("editor.selectionHighlightMultiline").as_bool().unwrap_or(false),
            max_length: self.setting("editor.selectionHighlightMaxLength").as_u64().unwrap_or(200) as usize,
        };
        let query = multicursor::selection_highlight_query(&self.find.controller, editor, &self.multicursor_context(window), options);
        let key = query.map(|query| (editor.model_id, editor.version_id, editor.selections.clone(), query));
        if self.find.selection_highlights.as_ref().map(|(key, _)| key) == key.as_ref() {
            return;
        }
        self.find.selection_highlights = key.map(|key| {
            let ranges = multicursor::selection_highlight_ranges(editor, &key.3);
            (key, ranges)
        });
    }

    fn multicursor_context(&self, window: &Window) -> MultiCursorContext {
        MultiCursorContext { text_focused: self.editor_focus.is_focused(window), match_mode: self.selected_text_match_mode() }
    }

    /// multicursor.ts: a text blur ends the session, and so does a text focus unless it matches with the find options.
    pub(super) fn multicursor_text_focus_changed(&mut self, focused: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.controller.multicursor.is_none() || (focused && self.selected_text_match_mode() == "findOptions") {
            return;
        }
        self.run_find(window, cx, |find, editor, viewport| {
            multicursor::end_session(find, editor, viewport);
            (FocusAction::NoFocusChange, None)
        });
    }

    /// `_showViewZone` and `_removeViewZone`: with `editor.find.addExtraSpaceOnTop`, whitespace as tall as the
    /// widget sits above line 1; the scroll moves with it, so the text stays put, unless `adjust` is false on show.
    fn layout_find_zone(&mut self, adjust: bool) {
        let state = &self.find.controller.state;
        let extra_space = self.setting("editor.find.addExtraSpaceOnTop").as_bool().unwrap_or(true);
        let height = match (state.is_revealed && extra_space, self.replace_visible()) {
            (false, _) => 0.,
            (true, false) => FIND_INPUT_AREA_HEIGHT,
            (true, true) => REPLACE_AREA_HEIGHT,
        };
        let delta = height - self.editors.viewport.top_whitespace;
        if delta == 0. {
            return;
        }
        self.editors.viewport.top_whitespace = height;
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut() else { return };
        let top = if adjust || height == 0. { editor.scroll.top + delta } else { editor.scroll.top };
        editor.set_scroll(top, editor.scroll.left, viewport);
    }

    /// `_reveal`'s `adjustEditorScrollTop`: a seeded selection in the zone's rows that spans lines or reaches the
    /// widget's left edge keeps the scroll, so the zone pushes it down from under the widget.
    fn selection_under_find_widget(&self, selection: Selection) -> bool {
        let Some(editor) = self.editors.active() else { return false };
        let seeding = self.find.controller.options.seed_search_string_from_selection != SeedFromSelection::Never;
        if selection.is_empty() || !seeding {
            return false;
        }
        let (start, end) = (selection.start(), selection.end());
        let start_top = editor.line_top(editor.position_view_line(start, false), self.editors.viewport) - editor.scroll.top;
        if start_top >= FIND_INPUT_AREA_HEIGHT {
            return false;
        }
        let tab_size = editor.document.indentation.tab_size;
        let left = |position: Position| visible_column(&editor.document.lines[position.line], position.column, tab_size) as f32 * self.editors.viewport.char_width - editor.scroll.left;
        let widget_left = self.find.widget_left.get();
        end.line > start.line || left(start) > widget_left || left(end) > widget_left
    }

    /// findDecorations.ts on one line, after Go to Line's range highlight: the find scopes and the range highlight across the line, then the matches,
    /// the current one on top. Returns the whole-line boxes, which decorations.ts draws under every other decoration,
    /// the match boxes (zIndex 10 and 13, over the zIndex 0 ones) and the text colors of the matches (`findMatchInline`).
    pub(super) fn find_line_decorations(&self, editor: &OpenEditor, index: usize, char_width: f32, scroll_left: f32) -> (Vec<Div>, Vec<Div>, Vec<(Range<usize>, HighlightStyle)>) {
        let (mut whole_lines, mut boxes, mut text_colors) = (Vec::new(), Vec::new(), Vec::new());
        let high_contrast = self.theme.kind.is_high_contrast();
        // shortcut: GPUI has no dotted border; high contrast dotted borders are dashed.
        let bordered = |mut decoration: Div, background: Option<Rgba>, border: Option<Rgba>, width: f32| {
            if let Some(background) = background {
                decoration = decoration.bg(background);
            }
            if let Some(border) = border {
                decoration = decoration.border(px(width)).border_color(border).when(high_contrast && width == 1., |decoration| decoration.border_dashed());
            }
            decoration
        };
        let whole_line = || div().absolute().inset_0().left(px(scroll_left));
        let range_highlight = || bordered(whole_line(), self.theme.color("editor.rangeHighlightBackground"), self.theme.color("editor.rangeHighlightBorder"), 1.);
        // editorNavigationQuickAccess.ts addDecorations: Go to Line's whole-line `rangeHighlight`.
        if self.quick_access.goto_line.as_ref().and_then(|state| state.highlight(editor.model_id)).is_some_and(|position| position.line == index) {
            whole_lines.push(range_highlight());
        }
        let Some(model) = self.find.controller.model.as_ref().filter(|_| self.find.model_id == Some(editor.model_id)) else {
            return (whole_lines, boxes, text_colors);
        };
        let on_line = |range: &Range<Position>| range.start.line <= index && index <= range.end.line;
        for _ in model.find_scopes(editor).unwrap_or_default().iter().filter(|scope| on_line(scope)) {
            whole_lines.push(bordered(whole_line(), self.theme.color("editor.findRangeHighlightBackground"), self.theme.color("editor.findRangeHighlightBorder"), 1.));
        }
        for range in [&model.range_highlight, &self.find.go_to_match_highlight] {
            if range.as_ref().is_some_and(on_line) {
                whole_lines.push(range_highlight());
            }
        }

        let text = &editor.document.lines[index];
        let tab_size = editor.document.indentation.tab_size;
        let x = |column: usize| visible_column(text, column, tab_size) as f32 * char_width;
        let byte = |column: usize| expand_tabs(&text.chars().take(column).collect::<String>(), tab_size).len();
        let first = model.matches.partition_point(|range| range.end.line < index);
        let mut current = None;
        for (match_index, range) in model.matches.iter().enumerate().skip(first).take_while(|(_, range)| range.start.line <= index) {
            let Some((start, end, line_break)) = selected_columns(Selection { anchor: range.start, head: range.end }, index, text) else { continue };
            let is_current = model.highlighted == Some(match_index);
            // `linesVisibleRangesForRange` includes the line break for `findMatch` only.
            let right = x(end) + if line_break && !is_current { char_width } else { 0. };
            let decoration = div().absolute().top_0().h_full().left(px(x(start))).w(px(right - x(start)));
            let (decoration, foreground) = match is_current {
                true => (bordered(decoration, self.theme.color("editor.findMatchBackground"), self.theme.color("editor.findMatchBorder"), 2.), "editor.findMatchForeground"),
                false => (bordered(decoration, self.theme.color("editor.findMatchHighlightBackground"), self.theme.color("editor.findMatchHighlightBorder"), 1.), "editor.findMatchHighlightForeground"),
            };
            if let Some(color) = self.theme.color(foreground) {
                text_colors.push((byte(start)..byte(end), HighlightStyle::from(color)));
            }
            match is_current {
                true => current = Some(decoration),
                false => boxes.push(decoration),
            }
        }
        boxes.extend(current);
        (whole_lines, boxes, text_colors)
    }

    /// findDecorations.ts's overview ruler and minimap marks in z-index order: the matches (10), or past 1000 matches
    /// their line ranges merged for the ruler only (0), then the current match (13).
    pub(super) fn find_marks(&self, editor: &OpenEditor, editor_height: f32) -> Vec<FindMark> {
        let Some(model) = self.find.controller.model.as_ref().filter(|_| self.find.model_id == Some(editor.model_id)) else { return Vec::new() };
        let mut marks = Vec::new();
        if model.matches.len() > 1000 {
            let approx_pixels_per_line = editor_height / editor.document.lines.len() as f32;
            let merge_lines_delta = ((3. / approx_pixels_per_line).ceil() as usize).max(2);
            for lines in merged_match_lines(&model.matches, merge_lines_delta) {
                marks.push(FindMark { range: Position::new(lines.start, 0)..Position::new(lines.end, 0), in_minimap: false, current: false });
            }
        } else {
            let others = model.matches.iter().enumerate().filter(|(index, _)| model.highlighted != Some(*index));
            marks.extend(others.map(|(_, range)| FindMark { range: range.clone(), in_minimap: true, current: false }));
        }
        if let Some(current) = model.highlighted {
            marks.push(FindMark { range: model.matches[current].clone(), in_minimap: true, current: true });
        }
        marks
    }

    /// The widget over the text area's top right corner (overlayWidgets.ts TOP_RIGHT_CORNER), `area_width` wide.
    pub(super) fn find_widget(&self, area_width: f32, content_left: f32, minimap_width: f32, window: &mut Window, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let state = &self.find.controller.state;
        let elapsed = self.find.toggled.map_or(SLIDE, |toggled| toggled.elapsed());
        if !state.is_revealed && elapsed >= SLIDE {
            return None;
        }
        if elapsed < SLIDE {
            window.request_animation_frame();
        }
        let replace = self.replace_visible();
        let height = if replace { REPLACE_AREA_HEIGHT } else { FIND_INPUT_AREA_HEIGHT };
        // `transform: translateY(calc(-100% - 10px))` when hidden, for the 10px shadow.
        let progress = (elapsed.as_secs_f32() / SLIDE.as_secs_f32()).min(1.);
        let hidden_fraction = if state.is_revealed { 1. - progress } else { progress };
        let top = 4. - (height + 10.) * hidden_fraction;

        // `_tryUpdateWidgetWidth`.
        let base = FIND_WIDGET_INITIAL_WIDTH + 28. + minimap_width;
        let reduced = base >= area_width;
        let narrow = base - MAX_MATCHES_COUNT_WIDTH >= area_width;
        let collapsed = base - MAX_MATCHES_COUNT_WIDTH >= area_width + 50.;
        let max_width = match (collapsed, narrow) {
            (true, _) => 170.,
            (false, true) => 257.,
            _ => area_width - 28. - minimap_width - 15.,
        };
        let width = self.find.resized_width.unwrap_or(FIND_WIDGET_INITIAL_WIDTH).min(max_width);
        let actions_width = if reduced { 0. } else { 3. + MAX_MATCHES_COUNT_WIDTH } + if collapsed { 0. } else { 50. } + 24.;
        // Border 2, padding 9 and 4, find part margins 17 and 25.
        let input_width = width - 57. - actions_width;

        let foreground = self.foreground_or("editorWidget.foreground");
        let disabled = self.color("disabledForeground");
        let searching = !state.search_string.is_empty();
        let has_matches = state.matches_count > 0;
        let selection = self.editors.active().map(|editor| editor.selection());
        let has_selection = selection.is_some_and(|selection| !selection.is_empty());
        let prev_enabled = searching && has_matches && state.can_navigate_back();
        let next_enabled = searching && has_matches && state.can_navigate_forward();
        let selection_checked = state.search_scope.is_some();

        let matches_label = match state.matches_count {
            0 => "No results".to_string(),
            count => {
                let position = if state.matches_position == 0 { "?".to_string() } else { state.matches_position.to_string() };
                let plus = if count >= MATCHES_LIMIT { "+" } else { "" };
                format!("{position} of {count}{plus}")
            }
        };
        let matches_color = if searching && !has_matches { self.color("errorForeground") } else { foreground };

        let find_part = div()
            .mt(px(3.))
            .ml(px(17.))
            .mr(px(25.))
            .h(px(25.))
            .flex()
            .child(
                self.find_input_box(InputId::Find, input_width, "Find", window, cx)
                    .when(!collapsed, |input| {
                        input.child(
                            div()
                                .absolute()
                                .top(px(2.))
                                .right(px(1.))
                                .flex()
                                .when_some(self.find_options_flash(window), |controls, color| controls.bg(color))
                                .child(self.option_toggle("case-sensitive", state.match_case, |state| NewFindState { match_case: Some(!state.match_case), ..Default::default() }, true, cx))
                                .child(self.option_toggle("whole-word", state.whole_word, |state| NewFindState { whole_word: Some(!state.whole_word), ..Default::default() }, true, cx))
                                .child(self.option_toggle("regex", state.is_regex, |state| NewFindState { is_regex: Some(!state.is_regex), ..Default::default() }, true, cx)),
                        )
                    }),
            )
            .child(
                div()
                    .h(px(25.))
                    .flex()
                    .items_center()
                    .when(!reduced, |actions| {
                        actions.child(
                            div()
                                .ml(px(3.))
                                .pl(px(2.))
                                .pt(px(2.))
                                .h(px(25.))
                                .min_w(px(MAX_MATCHES_COUNT_WIDTH))
                                .text_size(px(12.))
                                .line_height(px(23.))
                                .text_color(matches_color)
                                .whitespace_nowrap()
                                .child(matches_label),
                        )
                    })
                    .when(!collapsed, |actions| {
                        actions
                            .child(self.find_button("arrow-up", "find-previous", prev_enabled, foreground, disabled, cx, |workbench, window, cx| {
                                workbench.run_find(window, cx, |find, editor, viewport| {
                                    find.match_find_action(editor, viewport, false);
                                    (FocusAction::NoFocusChange, None)
                                })
                            }))
                            .child(self.find_button("arrow-down", "find-next", next_enabled, foreground, disabled, cx, |workbench, window, cx| {
                                workbench.run_find(window, cx, |find, editor, viewport| {
                                    find.match_find_action(editor, viewport, true);
                                    (FocusAction::NoFocusChange, None)
                                })
                            }))
                    })
                    .child(self.find_selection_toggle(selection_checked, selection_checked || has_selection, foreground, disabled, cx)),
            );

        let replace_part = replace.then(|| {
            div()
                .mt(px(3.))
                .ml(px(17.))
                .mr(px(25.))
                .h(px(25.))
                .flex()
                .child(
                    self.find_input_box(InputId::Replace, input_width, "Replace", window, cx).child(div().absolute().top(px(2.)).right(px(1.)).flex().child(self.option_toggle(
                        "preserve-case",
                        state.preserve_case,
                        |state| NewFindState { preserve_case: Some(!state.preserve_case), ..Default::default() },
                        true,
                        cx,
                    ))),
                )
                .when(!collapsed, |part| {
                    part.child(
                        div()
                            .h(px(25.))
                            .flex()
                            .items_center()
                            .child(self.find_button("replace", "replace", searching, foreground, disabled, cx, |workbench, window, cx| {
                                workbench.run_find(window, cx, |find, editor, viewport| (FocusAction::NoFocusChange, find.replace(editor, viewport)))
                            }))
                            .child(self.find_button("replace-all", "replace-all", searching, foreground, disabled, cx, |workbench, window, cx| {
                                workbench.run_find(window, cx, |find, editor, viewport| (FocusAction::NoFocusChange, find.replace_all(editor, viewport)))
                            })),
                    )
                })
        });

        let left = area_width - 2. * VERTICAL_SCROLLBAR_WIDTH - minimap_width - width;
        self.find.widget_left.set(left - content_left);
        let sash_color = self.theme.color("editorWidget.resizeBorder").unwrap_or(self.color("editorWidget.border"));
        Some(
            div()
                .id("find-widget")
                .debug_selector(|| "find-widget".into())
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(width))
                .h(px(height))
                .pl(px(9.))
                .pr(px(4.))
                .overflow_hidden()
                .occlude()
                .cursor_default()
                .font_family(".SystemUIFont")
                .text_size(px(13.))
                .line_height(px(19.))
                .text_color(foreground)
                .bg(self.color("editorWidget.background"))
                .border_1()
                .border_color(self.color("widget.border"))
                .rounded(px(8.))
                .shadow(vec![BoxShadow { color: gpui::rgba(0x00000024).into(), offset: point(px(0.), px(0.)), blur_radius: px(SHADOW_BLUR), spread_radius: px(0.), inset: false }])
                // A mouse down no focusable child took (no default prevented) leaves the focus on the document body;
                // wordHighlighter.ts `onDidBlurEditorWidget` stops the highlights without a focused code editor.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|workbench, _, window, cx| {
                        if window.default_prevented() {
                            return;
                        }
                        window.blur(cx);
                        if let Some(editor) = workbench.editors.active_mut() {
                            editor.highlights.clear();
                        }
                        cx.notify();
                    }),
                )
                .when(self.active_editor_readonly().is_none(), |widget| widget.child(self.replace_toggle(replace, height, cx)))
                .child(find_part)
                .children(replace_part)
                .child(
                    self.find_button("close", "find-close", true, foreground, disabled, cx, |workbench, window, cx| {
                        workbench.run_find(window, cx, |find, editor, viewport| {
                            find.close_find_widget(editor, viewport);
                            (FocusAction::NoFocusChange, None)
                        })
                    })
                    .absolute()
                    .top(px(5.))
                    .right(px(4.)),
                )
                .child(self.find_sash(width, height, max_width, sash_color, cx)),
        )
    }

    /// `ContextScopedFindInput`'s input box: 25px tall with a 1px border, the focused one outlined in `focusBorder`.
    fn find_input_box(&self, id: InputId, width: f32, placeholder: &str, window: &Window, cx: &mut Context<Self>) -> Div {
        let focused = self.input_focus(id).is_focused(window);
        let border = if focused { self.color("focusBorder") } else { self.color("input.border") };
        let controls_width = if id == InputId::Find { 66. } else { 22. };
        let history = if id == InputId::Find { &self.find.find_history } else { &self.find.replace_history };
        // HistoryInputBox `addSuffix`: a focused input with a history names the Up and Down keys.
        let placeholder = match focused && !history.elements().is_empty() {
            true => format!("{placeholder} (\u{21C5} for history)"),
            false => placeholder.to_string(),
        };
        div()
            .relative()
            .flex_none()
            .w(px(width))
            .h(px(25.))
            .flex()
            .bg(self.color("input.background"))
            .border_1()
            .border_color(border)
            .rounded(px(4.))
            .child(div().flex_1().h_full().ml(px(6.)).mr(px(controls_width)).flex().child(self.input_text(id, InputLook { password: false, placeholder: Some(placeholder), focused }, cx)))
    }

    /// A find input option (toggle.css `.monaco-custom-toggle`); a click puts the focus back in its input
    /// (`fixFocusOnOptionClickEnabled`) unless `focus_input` is false, as in the find options widget.
    pub(super) fn option_toggle(&self, icon: &'static str, checked: bool, toggled: fn(&crate::editor::find::state::FindState) -> NewFindState, focus_input: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let high_contrast = matches!(self.theme.kind, ThemeKind::HighContrastDark | ThemeKind::HighContrastLight);
        let color = if checked { self.foreground_or("inputOption.activeForeground") } else { self.foreground_or("editorWidget.foreground") };
        let input = if icon == "preserve-case" { InputId::Replace } else { InputId::Find };
        self.icon(icon, 20., 16., color)
            .id(icon)
            .ml(px(2.))
            .rounded(px(3.))
            .border_1()
            .border_color(if checked { self.color("inputOption.activeBorder") } else { gpui::transparent_black().into() })
            .cursor_pointer()
            .when(checked, |toggle| toggle.bg(self.color("inputOption.activeBackground")))
            .when(!checked && !high_contrast, |toggle| toggle.hover(|toggle| toggle.bg(self.color("inputOption.hoverBackground"))))
            .when(high_contrast, |toggle| toggle.hover(|toggle| toggle.border_dashed().border_color(self.color("focusBorder"))))
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_click(cx.listener(move |workbench, _, window, cx| {
                let new = toggled(&workbench.find.controller.state);
                workbench.run_find(window, cx, move |find, editor, viewport| {
                    find.change(editor, viewport, new, true);
                    (FocusAction::NoFocusChange, None)
                });
                if focus_input {
                    workbench.focus_input_keeping_selection(input, window, cx);
                }
            }))
    }

    fn focus_input_keeping_selection(&mut self, id: InputId, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.input_focus(id).clone(), cx);
        self.blink_caret(id, window, cx);
    }

    /// The Find in Selection toggle: a 22px toggle, disabled without a selection unless checked (`_updateToggleSelectionFindButton`).
    fn find_selection_toggle(&self, checked: bool, enabled: bool, foreground: Rgba, disabled: Rgba, cx: &mut Context<Self>) -> Stateful<Div> {
        let color = match (enabled, checked) {
            (false, _) => disabled,
            (true, true) => self.foreground_or("inputOption.activeForeground"),
            (true, false) => foreground,
        };
        self.icon("list-selection", 22., 16., color)
            .id("find-selection")
            .ml(px(2.))
            .rounded(px(5.))
            .border_1()
            .border_color(if checked { self.color("inputOption.activeBorder") } else { gpui::transparent_black().into() })
            .when(checked, |toggle| toggle.bg(self.color("inputOption.activeBackground")))
            .when(!enabled, |toggle| toggle.opacity(0.4))
            .when(enabled, |toggle| toggle.cursor_pointer().hover(|toggle| toggle.bg(self.color("toolbar.hoverBackground"))))
            .map(|toggle| {
                self.focusable_button(toggle, "find-selection", enabled, cx, |workbench, window, cx| {
                    workbench.run_find(window, cx, |find, editor, viewport| {
                        find.toggle_search_scope(editor, viewport);
                        (FocusAction::NoFocusChange, None)
                    })
                })
            })
    }

    /// A `SimpleButton`: 16px icon, 3px padding, 5px corners; a disabled one takes `disabledForeground`.
    #[expect(clippy::too_many_arguments, reason = "the colors are computed once per widget")]
    fn find_button(
        &self,
        icon: &'static str,
        id: &'static str,
        enabled: bool,
        foreground: Rgba,
        disabled: Rgba,
        cx: &mut Context<Self>,
        trigger: fn(&mut Workbench, &mut Window, &mut Context<Workbench>),
    ) -> Stateful<Div> {
        let button = self
            .icon(icon, 22., 16., if enabled { foreground } else { disabled })
            .id(id)
            .ml(px(3.))
            .rounded(px(5.))
            .border_1()
            .border_color(gpui::transparent_black())
            .when(enabled, |button| button.cursor_pointer().hover(|button| button.bg(self.color("toolbar.hoverBackground"))));
        self.focusable_button(button, id, enabled, cx, trigger)
    }

    /// A focusable button (tabIndex 0) with a 1px border: a mouse down focuses it, even disabled, and the border
    /// draws the workbench focus outline (`outline-offset: -1px`); a click, Enter or Space triggers it when enabled.
    fn focusable_button(&self, button: Stateful<Div>, id: &'static str, enabled: bool, cx: &mut Context<Self>, trigger: fn(&mut Workbench, &mut Window, &mut Context<Workbench>)) -> Stateful<Div> {
        let focus_border = self.color("focusBorder");
        button
            .track_focus(&self.find.button_focus[id])
            .focus(move |button| button.border_color(focus_border))
            .when(enabled, |button| {
                button
                    .on_click(cx.listener(move |workbench, _, window, cx| trigger(workbench, window, cx)))
                    .on_key_down(cx.listener(move |workbench, event: &KeyDownEvent, window, cx| {
                        let keystroke = &event.keystroke;
                        if matches!(keystroke.key.as_str(), "enter" | "space") && !keystroke.modifiers.modified() {
                            cx.stop_propagation();
                            trigger(workbench, window, cx);
                        }
                    }))
            })
    }

    /// The chevron at the left edge that shows and hides the replace input.
    /// `_isReplaceVisible`: a read-only editor keeps the replace part hidden and the toggle disabled (`canReplace`),
    /// which findWidget.css hides.
    fn replace_visible(&self) -> bool {
        self.find.controller.state.is_replace_revealed && self.active_editor_readonly().is_none()
    }

    fn replace_toggle(&self, replace: bool, height: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let icon = if replace { "chevron-down" } else { "chevron-right" };
        let toggle = self
            .icon(icon, 18., 16., self.foreground_or("editorWidget.foreground"))
            .id("find-replace-toggle")
            .absolute()
            .left(px(5.))
            .top(px(4.))
            .h(px(height - 10.))
            .rounded(px(4.))
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .hover(|button| button.bg(self.color("toolbar.hoverBackground")));
        self.focusable_button(toggle, "find-replace-toggle", true, cx, |workbench, window, cx| {
            workbench.run_find(window, cx, |find, editor, viewport| {
                let replace = !find.state.is_replace_revealed;
                find.change(editor, viewport, NewFindState { is_replace_revealed: Some(replace), ..Default::default() }, false);
                (FocusAction::NoFocusChange, None)
            })
        })
    }

    /// The 2px sash on the left edge: dragging it widens the widget past its initial width, a double click
    /// toggles between the initial width and the widest (`onDidChange`, `onDidReset`).
    fn find_sash(&self, width: f32, height: f32, max_width: f32, color: Rgba, cx: &mut Context<Self>) -> Stateful<Div> {
        let workbench = cx.entity();
        let dragging = self.find.sash_drag.is_some();
        // sash.css `::before`: 4px of `sash.hoverBorder` centered on the 2px sash, its left pixel and the ends in
        // the 8px corners clipped by the widget (GPUI clips to the rectangle).
        let hover_border = self.theme.color("sash.hoverBorder").unwrap_or(self.color("focusBorder"));
        let highlight = div().absolute().left_0().top(px(3.)).w(px(3.)).h(px(height - 8.)).bg(hover_border);
        div()
            .id("find-sash")
            .absolute()
            .left_0()
            .top_0()
            .w(px(2.))
            .h(px(height - 2.))
            .bg(color)
            .cursor_col_resize()
            .when(dragging || self.find.sash_hovered, |sash| sash.child(highlight))
            .on_hover(cx.listener(|workbench, hovered: &bool, window, cx| {
                workbench.find.sash_hovered = false;
                workbench.find.sash_hover = hovered.then(|| {
                    cx.spawn_in(window, async move |workbench, cx| {
                        cx.background_executor().timer(SASH_HOVER_DELAY).await;
                        let _ = workbench.update(cx, |workbench, cx| {
                            workbench.find.sash_hovered = true;
                            cx.notify();
                        });
                    })
                });
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |workbench, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if event.click_count == 2 {
                        let resized = workbench.find.resized_width.is_some_and(|resized| resized != FIND_WIDGET_INITIAL_WIDTH);
                        workbench.find.resized_width = Some(if resized { FIND_WIDGET_INITIAL_WIDTH } else { max_width });
                    } else {
                        workbench.find.sash_drag = Some((width, f32::from(event.position.x)));
                    }
                    cx.notify();
                }),
            )
            .when(dragging, |sash| {
                sash.child(
                    canvas(
                        |_, _, _| (),
                        move |_, _, window, _| {
                            let moved = workbench.clone();
                            window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
                                moved.update(cx, |workbench, cx| workbench.drag_find_sash(f32::from(event.position.x), max_width, cx));
                            });
                            let released = workbench.clone();
                            window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                                released.update(cx, |workbench, cx| {
                                    workbench.find.sash_drag = None;
                                    cx.notify();
                                });
                            });
                        },
                    )
                    .size_0(),
                )
            })
    }

    /// The width grows as the sash moves left; below the initial width or past the max it stays.
    fn drag_find_sash(&mut self, x: f32, max_width: f32, cx: &mut Context<Self>) {
        let Some((start_width, start_x)) = self.find.sash_drag else { return };
        let width = start_width + start_x - x;
        if (FIND_WIDGET_INITIAL_WIDTH..=max_width).contains(&width) {
            self.find.resized_width = Some(width);
            cx.notify();
        }
    }
}

/// `toFindMatchIndex`: `parseInt` of the value, 1 to the count or minus the count to -1, as a 0-based index.
/// The matches' line ranges, joined when at most `merge_lines_delta` lines apart; `end` is the last line.
fn merged_match_lines(matches: &[Range<Position>], merge_lines_delta: usize) -> Vec<Range<usize>> {
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in matches {
        match merged.last_mut() {
            Some(lines) if lines.end + merge_lines_delta >= range.start.line => lines.end = lines.end.max(range.end.line),
            _ => merged.push(range.start.line..range.end.line),
        }
    }
    merged
}

#[test]
fn merged_match_lines_join_matches_within_the_delta() {
    let on = |start: usize, end: usize| Position::new(start, 0)..Position::new(end, 1);
    assert_eq!(merged_match_lines(&[on(0, 0), on(2, 3), on(6, 6), on(9, 9)], 2), [0..3, 6..6, 9..9]);
}

fn find_match_index(value: &str, matches_count: usize) -> Option<usize> {
    let value = value.trim_start();
    let (negative, digits) = match value.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, value.strip_prefix('+').unwrap_or(value)),
    };
    let end = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
    let number: usize = digits[..end].parse().ok()?;
    match negative {
        false if (1..=matches_count).contains(&number) => Some(number - 1),
        true if (1..=matches_count).contains(&number) => Some(matches_count - number),
        _ => None,
    }
}

#[cfg(test)]
#[test]
fn go_to_match_parses_like_parse_int() {
    assert_eq!(find_match_index("2", 4), Some(1));
    assert_eq!(find_match_index(" 3abc", 4), Some(2));
    assert_eq!(find_match_index("-1", 4), Some(3));
    assert_eq!(find_match_index("-4", 4), Some(0));
    assert_eq!(find_match_index("5", 4), None);
    assert_eq!(find_match_index("0", 4), None);
    assert_eq!(find_match_index("-0", 4), None);
    assert_eq!(find_match_index("x", 4), None);
}

#[cfg(test)]
mod tests;
