//! The search editor's query widget (searchEditor.ts `createQueryEditor`, searchWidget.ts with `showContextToggle`):
//! one widget above the active search editor's results, loaded with that editor's configuration when it becomes
//! active (`setSearchConfig`) and read back when it searches (`readConfigFromWidget`).

use std::path::PathBuf;
use std::time::Duration;

use gpui::{ClickEvent, Context, Div, InteractiveElement, Keystroke, ParentElement, StatefulInteractiveElement, Styled, Window, div, px};
use serde_json::{Value, json};

use super::Workbench;
use super::input_box_view::InputId;
use super::search_view::{SearchField, SearchOption, SearchView};
use crate::commands::Outcome;
use crate::input_box::InputBox;
use crate::search_editor::SearchConfiguration;

/// The search editor's inputs in the order the focus walks them.
pub const SEARCH_EDITOR_FIELDS: [SearchField; 4] = [SearchField::Query, SearchField::ContextLines, SearchField::Include, SearchField::Exclude];

pub struct SearchEditorWidget {
    /// The model path of the search editor the widget holds the configuration of.
    pub path: Option<PathBuf>,
    pub view: SearchView,
}

impl SearchEditorWidget {
    pub fn new(cx: &mut Context<Workbench>) -> Self {
        SearchEditorWidget { path: None, view: SearchView::new(cx) }
    }
}

/// searchEditor.contribution.ts: the actions on the active search editor's widget.
const OPTION_COMMANDS: [(&str, SearchOption); 3] = [
    ("toggleSearchEditorCaseSensitive", SearchOption::MatchCase),
    ("toggleSearchEditorWholeWord", SearchOption::WholeWord),
    ("toggleSearchEditorRegex", SearchOption::Regex),
];

impl Workbench {
    pub(super) fn register_search_editor_widget_actions(&mut self) {
        for (command, field) in [("search.action.focusQueryEditorWidget", SearchField::Query), ("search.action.focusFilesToInclude", SearchField::Include), ("search.action.focusFilesToExclude", SearchField::Exclude)] {
            self.commands.register(command, move |workbench, _, window, cx| {
                if workbench.active_search_editor().is_some() {
                    if field != SearchField::Query {
                        workbench.toggle_search_editor_details(Some(true), cx);
                    }
                    workbench.focus_search_editor_input(field, false, window, cx);
                }
                Outcome::done()
            });
        }
        for (command, option) in OPTION_COMMANDS {
            self.commands.register(command, move |workbench, _, window, cx| {
                if workbench.active_search_editor().is_some() {
                    workbench.toggle_search_editor_option(option, window, cx);
                }
                Outcome::done()
            });
        }
        self.commands.register("toggleSearchEditorContextLines", |workbench, _, window, cx| {
            if workbench.active_search_editor().is_some() {
                workbench.toggle_search_editor_context_lines(window, cx);
            }
            Outcome::done()
        });
        for (command, increase) in [("increaseSearchEditorContextLines", true), ("decreaseSearchEditorContextLines", false)] {
            self.commands.register(command, move |workbench, _, window, cx| {
                if workbench.active_search_editor().is_some() {
                    workbench.modify_search_editor_context_lines(increase, window, cx);
                }
                Outcome::done()
            });
        }
    }

    /// `setInput`: a search editor that became active shows its configuration in the widget.
    pub(super) fn load_search_editor_widget(&mut self) {
        let Some(path) = self.active_search_editor() else { return };
        if self.search_editor_widget.path.as_ref() != Some(&path) {
            let config = self.search_editors[&path].config.clone();
            self.set_search_editor_widget(path, &config);
        }
    }

    /// `setSearchConfig`.
    pub(super) fn set_search_editor_widget(&mut self, path: PathBuf, config: &SearchConfiguration) {
        let widget = &mut self.search_editor_widget;
        widget.path = Some(path);
        let view = &mut widget.view;
        view.delayed = None;
        view.input_mut(SearchField::Query).value.set_value(config.query.clone());
        view.input_mut(SearchField::Include).value.set_value(config.files_to_include.clone());
        view.input_mut(SearchField::Exclude).value.set_value(config.files_to_exclude.clone());
        view.match_case = config.is_case_sensitive;
        view.regex = config.is_regexp;
        view.whole_word = config.match_whole_word;
        view.only_open_editors = config.only_open_editors;
        view.use_excludes_and_ignore_files = config.use_exclude_settings_and_ignore_files;
        view.details_visible = config.show_includes_excludes;
        // `setContextLines`: 0 unchecks the toggle and keeps the input's number.
        view.show_context = config.context_lines != 0;
        if config.context_lines != 0 {
            view.input_mut(SearchField::ContextLines).value.set_value(config.context_lines.to_string());
        }
        view.validation = Self::validate_search_input(view);
    }

    /// The search editor's context keys: `InSearchEditor` in its scoped context key service, and the keys of the
    /// widget's inputs (`SearchContext.InputBoxFocusedKey`, `SearchInputBoxFocusedKey`, the history widgets').
    pub(super) fn search_editor_context(&self, window: &Window, context: &mut std::collections::HashMap<String, Value>) {
        if self.active_search_editor().is_none() {
            return;
        }
        context.insert("activeEditor".into(), json!("workbench.editor.searchEditor"));
        let input = self.focused_input(window).and_then(|id| match id {
            InputId::SearchEditor(field) => Some(field),
            _ => None,
        });
        context.insert("inSearchEditor".into(), json!(input.is_some() || self.editor_focus.is_focused(window)));
        let Some(field) = input else { return };
        context.insert("inputFocus".into(), json!(true));
        if field == SearchField::ContextLines {
            return;
        }
        context.insert("inputBoxFocus".into(), json!(true));
        context.insert("searchInputBoxFocus".into(), json!(field == SearchField::Query));
        context.insert("historyNavigationWidgetFocus".into(), json!(true));
        context.insert("historyNavigationForwardsEnabled".into(), json!(true));
        context.insert("historyNavigationBackwardsEnabled".into(), json!(true));
    }

    /// The focus is in the active search editor, its widget or its results.
    pub(super) fn in_search_editor(&self, window: &Window) -> bool {
        self.active_search_editor().is_some() && (self.editor_focus.is_focused(window) || matches!(self.focused_input(window), Some(InputId::SearchEditor(_))))
    }

    /// searchEditor.css `.query-container`: the search widget with the context lines input and toggle, then the
    /// includes and excludes, in place of the breadcrumbs.
    pub(super) fn search_editor_query(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let view = &self.search_editor_widget.view;
        let foreground = self.foreground_or("editor.foreground");
        let option_toggle = |icon: &'static str, checked: bool, option: SearchOption| {
            self.search_toggle(icon, checked, true, cx.listener(move |workbench, _: &ClickEvent, window, cx| workbench.toggle_search_editor_option(option, window, cx)))
        };
        let query_toggles = div()
            .flex()
            .gap(px(2.))
            .child(option_toggle("case-sensitive", view.match_case, SearchOption::MatchCase))
            .child(option_toggle("whole-word", view.whole_word, SearchOption::WholeWord))
            .child(option_toggle("regex", view.regex, SearchOption::Regex));
        let open_editors = option_toggle("book", view.only_open_editors, SearchOption::OnlyOpenEditors);
        let excludes = option_toggle("exclude", view.use_excludes_and_ignore_files, SearchOption::UseExcludes);
        let query = self.search_input_box(InputId::SearchEditor(SearchField::Query), "Search", 26., query_toggles, window, cx);
        // searchEditor.css `.context-lines-input`: 5px left and 2px right of the 50px number input.
        let context_lines = div().ml(px(5.)).mr(px(2.)).w(px(50.)).flex_none().child(self.search_input_box(InputId::SearchEditor(SearchField::ContextLines), "", 26., div(), window, cx));
        let context_toggle = self.search_toggle("list-selection", view.show_context, true, cx.listener(|workbench, _: &ClickEvent, window, cx| workbench.toggle_search_editor_context_lines(window, cx)));
        let widget = div().flex().items_center().child(div().flex_1().min_w_0().child(query)).child(context_lines).child(context_toggle);
        let more = div()
            .id("search-editor-details")
            .absolute()
            .top_0()
            .right(px(-2.))
            .w(px(25.))
            .h(px(16.))
            .cursor_pointer()
            .flex()
            .justify_center()
            .child(self.icon("ellipsis", 16., 16., foreground))
            .on_click(cx.listener(|workbench, _: &ClickEvent, _, cx| workbench.toggle_search_editor_details(None, cx)));
        let details = view.details_visible.then(|| {
            // The changed files toggle needs source control, which the port does not have: disabled, as upstream draws
            // it without a repository.
            let changed_files = self.search_toggle("edit-code", false, false, |_: &ClickEvent, _: &mut Window, _: &mut gpui::App| {});
            let heading = |title: &'static str| div().pt(px(4.)).h(px(22.2)).text_size(px(11.)).line_height(px(18.2)).child(title);
            div()
                .pb(px(10.))
                .flex()
                .flex_col()
                .child(heading("files to include"))
                .child(self.search_input_box(InputId::SearchEditor(SearchField::Include), "e.g. *.ts, src/**/include", 25., div().flex().gap(px(2.)).child(changed_files).child(open_editors), window, cx))
                .child(heading("files to exclude"))
                .child(self.search_input_box(InputId::SearchEditor(SearchField::Exclude), "e.g. *.ts, src/**/exclude", 25., div().child(excludes), window, cx))
        });
        let includes_excludes = div().relative().min_h(px(13.)).child(more).children(details);
        // The empty `.messages` box after the details has `margin-top: -5px`, pulling the results up by 5px.
        div().flex_none().ml(px(19.)).mr(px(12.)).mb(px(12. - 5.)).pt(px(6.)).text_color(foreground).child(widget).child(includes_excludes)
    }

    /// `edit_input_box` for a search editor input: the value changes, then the widget reacts as its change events do.
    pub(super) fn edit_search_editor_input(&mut self, field: SearchField, edit: impl FnOnce(&mut InputBox), window: &mut Window, cx: &mut Context<Self>) {
        let view = &mut self.search_editor_widget.view;
        let before = view.value(field).to_string();
        edit(&mut view.input_mut(field).value);
        cx.notify();
        if view.value(field) == before {
            return;
        }
        match field {
            // searchWidget.ts `onSearchInputChanged`: typing searches only with `search.searchOnType`.
            SearchField::Query => {
                view.validation = Self::validate_search_input(view);
                if self.setting("search.searchOnType") == true
                    && let Some(delay) = self.search_on_type_delay(&self.search_editor_widget.view)
                {
                    self.trigger_search_editor(true, delay, None, window, cx);
                }
            }
            // patternInputWidget.ts: every change submits, searched after the debounce period.
            SearchField::Include | SearchField::Exclude => {
                let delay = self.debounce_period();
                self.trigger_search_editor(false, delay, None, window, cx);
            }
            // `contextLinesInput.onDidChange` and `onContextLinesChanged`.
            SearchField::ContextLines => {
                if view.value(field) != "0" {
                    view.show_context = true;
                }
                if view.value(field).contains('-') {
                    view.input_mut(field).value.set_value("0".into());
                }
                self.trigger_search_editor(false, Duration::ZERO, None, window, cx);
            }
            SearchField::Replace => {}
        }
    }

    /// `history.showPrevious` and `history.showNext` on a search editor input.
    pub(super) fn show_search_editor_history_value(&mut self, field: SearchField, previous: bool, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.search_editor_widget.view.input_mut(field);
        let value = match previous {
            true => input.history.show_previous_value(&input.value.value),
            false => Some(input.history.show_next_value(&input.value.value)),
        };
        if let Some(value) = value {
            self.edit_search_editor_input(field, |input| input.set_value(value), window, cx);
        }
    }

    /// The keys of the search editor's inputs (searchWidget.ts `onSearchInputKeyDown`, patternInputWidget.ts).
    pub(super) fn search_editor_input_key(&mut self, field: SearchField, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let modifiers = keystroke.modifiers;
        let plain = !modifiers.control && !modifiers.platform && !modifiers.alt && !modifiers.shift;
        match keystroke.key.as_str() {
            "enter" if modifiers.control && !modifiers.platform && !modifiers.alt && !modifiers.shift && field == SearchField::Query => {
                self.edit_input_box(InputId::SearchEditor(field), window, cx, |input| input.insert(None, "\n"));
            }
            "enter" if plain && field == SearchField::Query => self.trigger_search_editor(true, Duration::ZERO, None, window, cx),
            "enter" if plain && matches!(field, SearchField::Include | SearchField::Exclude) => self.trigger_search_editor(false, Duration::ZERO, None, window, cx),
            "tab" if !modifiers.control && !modifiers.platform && !modifiers.alt => self.tab_search_editor_input(field, !modifiers.shift, window, cx),
            _ => return false,
        }
        true
    }

    /// Tab walks the visible inputs in the widget's order, then the results.
    fn tab_search_editor_input(&mut self, field: SearchField, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let details = self.search_editor_widget.view.details_visible;
        let visible: Vec<SearchField> = SEARCH_EDITOR_FIELDS.into_iter().filter(|field| details || !matches!(field, SearchField::Include | SearchField::Exclude)).collect();
        let index = visible.iter().position(|known| *known == field).expect("a visible input has the focus");
        let next = match forward {
            true => visible.get(index + 1).copied(),
            false => index.checked_sub(1).map(|index| visible[index]),
        };
        match next {
            Some(field) => self.focus_search_editor_input(field, true, window, cx),
            None if forward => window.focus(&self.editor_focus, cx),
            None => {}
        }
    }

    pub(super) fn focus_search_editor_input(&mut self, field: SearchField, select: bool, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.search_editor_widget.view.input_mut(field);
        if select {
            input.value.select_all();
        }
        window.focus(&input.focus.clone(), cx);
        self.blink_caret(InputId::SearchEditor(field), window, cx);
        cx.notify();
    }

    /// `focusNextInput` and `focusPrevInput`: query, includes while shown, excludes, then the results; back from
    /// the query wraps to the results.
    pub(super) fn focus_next_search_editor_input(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let details = self.search_editor_widget.view.details_visible;
        let next = match (self.focused_input(window), forward) {
            (Some(InputId::SearchEditor(SearchField::Query)), true) if details => Some(SearchField::Include),
            (Some(InputId::SearchEditor(SearchField::Query)), true) => None,
            (Some(InputId::SearchEditor(SearchField::Include)), true) => Some(SearchField::Exclude),
            (Some(InputId::SearchEditor(SearchField::Exclude)), true) => None,
            (Some(InputId::SearchEditor(SearchField::Query)), false) => None,
            (Some(InputId::SearchEditor(SearchField::Include)), false) => Some(SearchField::Query),
            (Some(InputId::SearchEditor(SearchField::Exclude)), false) => Some(SearchField::Include),
            _ => return,
        };
        match next {
            Some(field) => self.focus_search_editor_input(field, false, window, cx),
            None => window.focus(&self.editor_focus, cx),
        }
    }

    /// `toggleIncludesExcludes`: the focus stays where it is.
    pub(super) fn toggle_search_editor_details(&mut self, show: Option<bool>, cx: &mut Context<Self>) {
        let view = &mut self.search_editor_widget.view;
        view.details_visible = show.unwrap_or(!view.details_visible);
        cx.notify();
    }

    /// The query toggles search again keeping the cursor (`onDidOptionChange`); the includes' open editors toggle and
    /// the excludes' ignore toggle search from the start (`onChangeSearchInEditorsBox`, `onChangeIgnoreBox`).
    pub(super) fn toggle_search_editor_option(&mut self, option: SearchOption, window: &mut Window, cx: &mut Context<Self>) {
        let view = &mut self.search_editor_widget.view;
        let reset_cursor = match option {
            SearchOption::MatchCase => {
                view.match_case = !view.match_case;
                false
            }
            SearchOption::WholeWord => {
                view.whole_word = !view.whole_word;
                false
            }
            SearchOption::Regex => {
                view.regex = !view.regex;
                false
            }
            SearchOption::OnlyOpenEditors => {
                view.only_open_editors = !view.only_open_editors;
                true
            }
            SearchOption::UseExcludes => {
                view.use_excludes_and_ignore_files = !view.use_excludes_and_ignore_files;
                true
            }
            SearchOption::PreserveCase => unreachable!("the search editor has no replace input"),
        };
        view.validation = Self::validate_search_input(view);
        cx.notify();
        self.trigger_search_editor(reset_cursor, Duration::ZERO, None, window, cx);
    }

    /// `toggleContextLines`.
    fn toggle_search_editor_context_lines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = &mut self.search_editor_widget.view;
        view.show_context = !view.show_context;
        if view.value(SearchField::ContextLines).contains('-') {
            view.input_mut(SearchField::ContextLines).value.set_value("0".into());
        }
        cx.notify();
        self.trigger_search_editor(false, Duration::ZERO, None, window, cx);
    }

    /// `modifyContextLines`: the number one up or down; the toggle is checked unless it reaches 0.
    fn modify_search_editor_context_lines(&mut self, increase: bool, window: &mut Window, cx: &mut Context<Self>) {
        let view = &mut self.search_editor_widget.view;
        let current: i64 = view.value(SearchField::ContextLines).trim().parse().unwrap_or(0);
        let modified = current + if increase { 1 } else { -1 };
        view.show_context = modified != 0;
        self.edit_search_editor_input(SearchField::ContextLines, |input| input.set_value(modified.to_string()), window, cx);
    }

    /// `triggerSearch`: an invalid query does not search; the search waits for `delay` (`triggerQueryDelayer`).
    /// `focus_results` defaults to `search.searchEditor.focusResultsOnSearch`.
    pub(super) fn trigger_search_editor(&mut self, reset_cursor: bool, delay: Duration, focus_results: Option<bool>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.search_editor_widget.path.clone() else { return };
        if self.search_editor_widget.view.validation.is_some() {
            return;
        }
        let focus_results = focus_results.unwrap_or_else(|| self.setting("search.searchEditor.focusResultsOnSearch") == true);
        if delay.is_zero() {
            self.search_editor_widget.view.delayed = None;
            self.run_search_editor(path, reset_cursor, focus_results, window, cx);
            return;
        }
        self.search_editor_widget.view.delayed = Some(cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(delay).await;
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.run_search_editor(path, reset_cursor, focus_results, window, cx));
        }));
    }

    /// `doRunSearch` reads the widget (`readConfigFromWidget`) and its inputs join their histories 2 seconds later
    /// (`searchHistoryDelayer`).
    pub(super) fn read_search_editor_widget(&mut self, cx: &mut Context<Self>) -> SearchConfiguration {
        self.search_editor_widget.view.history_update = Some(cx.spawn(async |workbench, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ = workbench.update(cx, |workbench, _| {
                for field in [SearchField::Query, SearchField::Exclude, SearchField::Include] {
                    let input = workbench.search_editor_widget.view.input_mut(field);
                    input.history.add_to_history(&input.value.value, false);
                }
            });
        }));
        self.search_editor_widget.view.configuration()
    }
}
