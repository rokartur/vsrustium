//! The search box's suggest widget (suggestEnabledInput.ts with the `SettingsEditor2` provider): a typed
//! `@` or `:` opens the completions of the word before the caret.

use gpui::{Context, MouseButton, ScrollWheelEvent, Window, div, prelude::*, px};
use serde_json::{Value, json};
use std::collections::HashMap;

use super::super::Workbench;
use super::super::input_box_view::InputId;
use super::super::keybindings_editor_view::highlighted;
use super::transparent;
use crate::commands::Outcome;
use crate::filters::{FuzzyScoreOptions, Matches, create_matches, fuzzy_score};
use crate::settings_editor::search_suggestions;

const TRIGGER_CHARACTERS: [char; 2] = ['@', ':'];
/// The `lineHeight` of `getSuggestEnabledInputOptions`, which the rows take.
const ITEM_HEIGHT: f32 = 20.;
const MAX_VISIBLE: usize = 12;
/// suggest.css `.suggest-widget` width and the large corner radius.
const WIDTH: f32 = 430.;
const RADIUS: f32 = 8.;

pub(super) struct Suggest {
    /// The char offset where the completed word starts, the range a pick replaces up to the caret.
    word_start: usize,
    items: Vec<(String, Matches)>,
    selected: usize,
    top: usize,
}

impl Workbench {
    pub(super) fn register_settings_suggest_commands(&mut self) {
        let commands = &mut self.commands;
        commands.register("acceptSelectedSuggestion", |workbench, _, window, cx| {
            workbench.accept_settings_search_suggestion(None, window, cx);
            Outcome::done()
        });
        commands.register("hideSuggestWidget", |workbench, _, _, cx| {
            workbench.settings_editor.suggest = None;
            cx.notify();
            Outcome::done()
        });
        let page = MAX_VISIBLE as isize - 1;
        for (id, delta, wrap) in [("selectNextSuggestion", 1, true), ("selectPrevSuggestion", -1, true), ("selectNextPageSuggestion", page, false), ("selectPrevPageSuggestion", -page, false)] {
            commands.register(id, move |workbench, _, _, cx| {
                if let Some(suggest) = &mut workbench.settings_editor.suggest {
                    suggest.select(delta, wrap);
                    cx.notify();
                }
                Outcome::done()
            });
        }
    }

    /// The keys the suggest widget's keybindings need while it shows under the focused search box.
    pub(super) fn settings_suggest_context(&self, context: &mut HashMap<String, Value>) {
        let Some(suggest) = &self.settings_editor.suggest else { return };
        let word: String = self.settings_editor.search.value.chars().skip(suggest.word_start).take(self.settings_editor.search.selection.1 - suggest.word_start).collect();
        context.insert("textInputFocus".into(), json!(true));
        context.insert("suggestWidgetVisible".into(), json!(true));
        context.insert("suggestWidgetHasFocusedSuggestion".into(), json!(true));
        context.insert("suggestWidgetMultipleSuggestions".into(), json!(suggest.items.len() > 1));
        context.insert("suggestionMakesTextEdit".into(), json!(suggest.items[suggest.selected].0 != word));
        context.insert("acceptSuggestionOnEnter".into(), json!(true));
    }

    /// After an edit of the search box: a typed trigger character opens the widget, and an open one follows the word.
    pub(super) fn update_settings_search_suggest(&mut self, typed: Option<char>) {
        let open = self.settings_editor.suggest.is_some() || typed.is_some_and(|typed| TRIGGER_CHARACTERS.contains(&typed));
        self.settings_editor.suggest = if open { self.settings_search_completions() } else { None };
    }

    /// `provideCompletionItems`: the word runs from the last space before the caret; typed text without a trigger gets nothing.
    fn settings_search_completions(&self) -> Option<Suggest> {
        let search = &self.settings_editor.search;
        let chars: Vec<char> = search.value.chars().collect();
        let caret = search.selection.1.min(chars.len());
        let word_start = chars[..caret].iter().rposition(|&char| char == ' ').map_or(0, |space| space + 1);
        let word: String = chars[word_start..caret].iter().collect();
        if word.chars().next().is_some_and(|first| !TRIGGER_CHARACTERS.contains(&first)) {
            return None;
        }
        let languages: Vec<&str> = self.languages.sorted_names().into_iter().map(|(_, id)| id).collect();
        let mut extensions: Vec<&str> = self.settings_editor.groups.iter().filter_map(|group| group.extension_id.as_deref()).collect();
        extensions.dedup();
        // CompletionModel: `fuzzyScore` against the typed word, best score first, then the label as `sortText`.
        let word_low = word.to_lowercase();
        let mut scored = Vec::new();
        for label in search_suggestions(&search.value, &languages, &extensions) {
            let label_low = label.to_lowercase();
            let matched = match word.is_empty() {
                true => Some((0, Matches::new())),
                false => fuzzy_score(&word, &word_low, 0, &label, &label_low, 0, FuzzyScoreOptions::default()).map(|score| (score.score, create_matches(Some(&score)))),
            };
            let Some((score, matches)) = matched else { continue };
            scored.push((score, label_low, label, matches));
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let items: Vec<(String, Matches)> = scored.into_iter().map(|(_, _, label, matches)| (label, matches)).collect();
        (!items.is_empty()).then_some(Suggest { word_start, items, selected: 0, top: 0 })
    }

    /// `acceptSelectedSuggestion`: the label replaces the typed word; accepting never reopens the widget.
    fn accept_settings_search_suggestion(&mut self, item: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(suggest) = self.settings_editor.suggest.take() else { return };
        let label = suggest.items[item.unwrap_or(suggest.selected)].0.clone();
        let caret = self.settings_editor.search.selection.1;
        self.edit_input_box(InputId::SettingsSearch, window, cx, |input| input.insert(Some(suggest.word_start..caret), &label));
        self.settings_editor.suggest = None;
    }

    /// The widget under the word it completes, inside the search box's positioned container.
    pub(super) fn settings_suggest_widget(&self, text_left: f32, window: &Window, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let editor = &self.settings_editor;
        let suggest = editor.suggest.as_ref().filter(|_| editor.search_focus.is_focused(window))?;
        // suggestWidget.ts showSuggestions: the widget sits at the caret, not at the word start.
        let left = text_left + editor.search_view.x_of_char(&editor.search.value, editor.search.selection.1);
        let visible = suggest.items.len().min(MAX_VISIBLE);
        let rows = suggest.items.iter().enumerate().skip(suggest.top).take(visible).map(|(index, (label, matches))| {
            let selected = index == suggest.selected;
            let (foreground, highlight) = match selected {
                true => (self.color("editorSuggestWidget.selectedForeground"), self.color("editorSuggestWidget.focusHighlightForeground")),
                false => (self.color("editorSuggestWidget.foreground"), self.color("editorSuggestWidget.highlightForeground")),
            };
            div()
                .id(("settings-suggest", index))
                .h(px(ITEM_HEIGHT))
                .pl(px(2.))
                .pr(px(10.))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_color(foreground)
                .when(selected, |row| row.bg(self.color("editorSuggestWidget.selectedBackground")))
                .child(highlighted(label, matches, highlight))
                .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _, window, cx| {
                    cx.stop_propagation();
                    workbench.accept_settings_search_suggestion(Some(index), window, cx);
                }))
        });
        Some(
            div()
                .id("settings-suggest-widget")
                .absolute()
                .top(px(22.))
                .left(px(left))
                .w(px(WIDTH))
                .rounded(px(RADIUS))
                .overflow_hidden()
                .border_1()
                .border_color(self.theme.color("editorSuggestWidget.border").unwrap_or(transparent()))
                .bg(self.color("editorSuggestWidget.background"))
                .shadow(vec![gpui::BoxShadow {
                    color: self.color("widget.shadow").into(),
                    offset: gpui::point(px(0.), px(2.)),
                    blur_radius: px(8.),
                    spread_radius: px(0.),
                    inset: false,
                }])
                .text_size(px(13.))
                .on_scroll_wheel(cx.listener(|workbench, event: &ScrollWheelEvent, window, cx| {
                    let Some(suggest) = &mut workbench.settings_editor.suggest else { return };
                    let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
                    let rows = (-delta / ITEM_HEIGHT).round() as isize;
                    suggest.top = suggest.top.saturating_add_signed(rows).min(suggest.items.len().saturating_sub(MAX_VISIBLE));
                    cx.notify();
                }))
                .children(rows),
        )
    }
}

impl Suggest {
    /// `selectNext`/`selectPrevious` wrap around the ends; the page moves stop at them.
    fn select(&mut self, delta: isize, wrap: bool) {
        let count = self.items.len() as isize;
        let target = self.selected as isize + delta;
        self.selected = if wrap { target.rem_euclid(count) } else { target.clamp(0, count - 1) } as usize;
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + MAX_VISIBLE {
            self.top = self.selected + 1 - MAX_VISIBLE;
        }
    }
}
