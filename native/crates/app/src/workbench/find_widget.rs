//! The editor find widget (findWidget.ts, findWidget.css, findInput.ts, replaceInput.ts, toggle.css): the find
//! and replace inputs with their option toggles, the match count, the buttons and the resize sash, drawn over the
//! editor's top right corner. The controller (editor/find/controller.rs) owns what it searches for.

use std::cell::Cell;
use std::time::{Duration, Instant};

use gpui::{
    BoxShadow, Context, Div, FocusHandle, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Rgba, Stateful, Window, canvas, div, point, prelude::*, px,
};
use serde_json::Value;

use super::Workbench;
use super::input_box_view::{InputId, InputLook, InputView};
use crate::commands::Outcome;
use crate::editor::find::controller::{AutoFindInSelection, FindController, FindOptions, FocusAction, QueryOptions, SeedFromSelection};
use crate::editor::find::state::{MATCHES_LIMIT, NewFindState};
use crate::editor::{ContentChanged, OpenEditor, Position, Selection, VERTICAL_SCROLLBAR_WIDTH, VerticalReveal, Viewport, visible_column};
use crate::input_box::InputBox;
use crate::theme::ThemeKind;

const FIND_WIDGET_INITIAL_WIDTH: f32 = 419.;
const MAX_MATCHES_COUNT_WIDTH: f32 = 69.;
/// `_getHeight`: 4 + the input's 23 + its 2px border + 4, plus 4 + 25 with the replace input.
const FIND_INPUT_AREA_HEIGHT: f32 = 33.;
const REPLACE_AREA_HEIGHT: f32 = 62.;
/// findWidget.css `transition: transform 200ms linear`.
const SLIDE: Duration = Duration::from_millis(200);
/// `--vscode-shadow-lg` (style.css).
const SHADOW_BLUR: f32 = 12.;

pub struct FindWidget {
    pub controller: FindController,
    pub find_input: InputBox,
    pub replace_input: InputBox,
    pub find_view: InputView,
    pub replace_view: InputView,
    pub find_focus: FocusHandle,
    pub replace_focus: FocusHandle,
    /// The model the controller is bound to; another active model restarts the find there (`onDidChangeModel`).
    model_id: Option<u32>,
    /// The width the sash set (`_resized`).
    resized_width: Option<f32>,
    /// The widget width and pointer x when a sash drag started.
    sash_drag: Option<(f32, f32)>,
    /// When the widget last showed or hid, for its slide.
    toggled: Option<Instant>,
    /// The widget's left edge as last drawn, from where the text's column 0 is.
    widget_left: Cell<f32>,
}

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
            model_id: None,
            resized_width: None,
            sash_drag: None,
            toggled: None,
            widget_left: Cell::new(f32::MAX),
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
                workbench.run_find(window, cx, action);
                // `CommonFindController.selectAllMatches` focuses the editor.
                if id == "editor.action.selectAllMatches" {
                    window.focus(&workbench.editor_focus, cx);
                }
                Outcome::done()
            });
        }
    }

    /// Runs a controller action on the active editor, then tells the host what moved or changed, shows or hides
    /// the widget (`_onStateChanged` isRevealed) and moves the focus the action asks for.
    fn run_find(&mut self, window: &mut Window, cx: &mut Context<Self>, action: impl FnOnce(&mut FindController, &mut OpenEditor, Viewport) -> (FocusAction, Option<ContentChanged>)) {
        self.find.controller.options = self.find_options();
        self.find.controller.find_input_focused = self.find.find_focus.is_focused(window);
        self.find_editor_changed();
        let top_whitespace = self.editors.viewport.top_whitespace;
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut() else { return };
        let find = &mut self.find;
        let (selections, scroll, revealed) = (editor.selections.clone(), editor.scroll, find.controller.state.is_revealed);
        let (focus, change) = action(&mut find.controller, editor, viewport);
        let adjust_scroll = !self.find.controller.state.is_revealed || revealed || !self.selection_under_find_widget(selections[0]);
        self.layout_find_zone(adjust_scroll);
        if self.find.controller.state.is_revealed && !revealed {
            self.reveal_current_match();
        }
        let editor = self.editors.active().expect("checked above");
        let (moved, scrolled) = (editor.selections != selections, editor.scroll != scroll || self.editors.viewport.top_whitespace != top_whitespace);
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
                // `_hide(true)`.
                window.focus(&self.editor_focus, cx);
            }
        }
        match focus {
            FocusAction::NoFocusChange => {}
            FocusAction::FocusFindInput => self.focus_find_input(InputId::Find, window, cx),
            FocusAction::FocusReplaceInput => self.focus_find_input(InputId::Replace, window, cx),
        }
        cx.notify();
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
        FindOptions { seed_search_string_from_selection: seed, auto_find_in_selection, is_loop, ..FindOptions::default() }
    }

    /// Typing in an input changes the state: the search string moves the cursor to the nearest match.
    pub(super) fn edit_find_input(&mut self, id: InputId, edit: impl FnOnce(&mut InputBox), window: &mut Window, cx: &mut Context<Self>) {
        let input = if id == InputId::Find { &mut self.find.find_input } else { &mut self.find.replace_input };
        let before = input.value.clone();
        edit(input);
        let value = input.value.clone();
        cx.notify();
        if value == before {
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

    /// `_onFindInputKeyDown` and `_onReplaceInputKeyDown`: Tab moves between the inputs, Cmd+Down to the editor.
    pub(super) fn find_input_key(&mut self, id: InputId, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let modifiers = keystroke.modifiers;
        match keystroke.key.as_str() {
            "down" if modifiers.platform && !modifiers.shift && !modifiers.alt => window.focus(&self.editor_focus, cx),
            "tab" if id == InputId::Find && !modifiers.shift && self.find.controller.state.is_replace_revealed => self.focus_find_input(InputId::Replace, window, cx),
            "tab" if id == InputId::Replace && modifiers.shift => self.focus_find_input(InputId::Find, window, cx),
            _ => return false,
        }
        true
    }

    /// A newly active model rebinds the controller: a visible find searches it (`onDidChangeModel`).
    pub(super) fn find_editor_changed(&mut self) {
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut() else {
            self.find.controller.model = None;
            self.find.model_id = None;
            return;
        };
        if self.find.model_id == Some(editor.model_id) {
            return;
        }
        self.find.model_id = Some(editor.model_id);
        self.find.controller.editor_changed(editor, viewport, self.find.controller.query_options());
    }

    /// An edit the find did not make: its matches follow the text, and the next search starts at the cursor.
    pub(super) fn find_editor_edited(&mut self, change: Option<&ContentChanged>) {
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut() else { return };
        if let Some(change) = change {
            self.find.controller.content_changed(editor, viewport, change);
        }
        self.find.controller.cursor_moved(editor);
    }

    /// `_showViewZone` and `_removeViewZone`: with `editor.find.addExtraSpaceOnTop`, whitespace as tall as the
    /// widget sits above line 1; the scroll moves with it, so the text stays put, unless `adjust` is false on show.
    fn layout_find_zone(&mut self, adjust: bool) {
        let state = &self.find.controller.state;
        let extra_space = self.setting("editor.find.addExtraSpaceOnTop").as_bool().unwrap_or(true);
        let height = match (state.is_revealed && extra_space, state.is_replace_revealed) {
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
        let start_top = self.editors.viewport.line_top(editor.view_line(start.line)) - editor.scroll.top;
        if start_top >= FIND_INPUT_AREA_HEIGHT {
            return false;
        }
        let tab_size = editor.document.indentation.tab_size;
        let left = |position: Position| visible_column(&editor.document.lines[position.line], position.column, tab_size) as f32 * self.editors.viewport.char_width - editor.scroll.left;
        let widget_left = self.find.widget_left.get();
        end.line > start.line || left(start) > widget_left || left(end) > widget_left
    }

    /// The research ran before the zone existed: its reveal of the current match runs again on the final layout.
    fn reveal_current_match(&mut self) {
        let viewport = self.editors.viewport;
        let Some(range) = self.find.controller.state.current_match.clone() else { return };
        let Some(editor) = self.editors.active_mut() else { return };
        editor.reveal_range(range.start, range.end, VerticalReveal::CenterIfOutsideViewport, viewport);
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
        let replace = state.is_replace_revealed;
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
                                .child(self.option_toggle("case-sensitive", state.match_case, |state| NewFindState { match_case: Some(!state.match_case), ..Default::default() }, cx))
                                .child(self.option_toggle("whole-word", state.whole_word, |state| NewFindState { whole_word: Some(!state.whole_word), ..Default::default() }, cx))
                                .child(self.option_toggle("regex", state.is_regex, |state| NewFindState { is_regex: Some(!state.is_regex), ..Default::default() }, cx)),
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
                .child(self.replace_toggle(replace, height, cx))
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
            .child(div().flex_1().h_full().ml(px(6.)).mr(px(controls_width)).flex().child(self.input_text(id, InputLook { password: false, placeholder: Some(placeholder.into()), focused }, cx)))
    }

    /// A find input option (toggle.css `.monaco-custom-toggle`); a click puts the focus back in its input
    /// (`fixFocusOnOptionClickEnabled`).
    fn option_toggle(&self, icon: &'static str, checked: bool, toggled: fn(&crate::editor::find::state::FindState) -> NewFindState, cx: &mut Context<Self>) -> Stateful<Div> {
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
                workbench.focus_input_keeping_selection(input, window, cx);
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
            .when(enabled, |toggle| {
                toggle.cursor_pointer().hover(|toggle| toggle.bg(self.color("toolbar.hoverBackground"))).on_click(cx.listener(|workbench, _, window, cx| {
                    workbench.run_find(window, cx, |find, editor, viewport| {
                        find.toggle_search_scope(editor, viewport);
                        (FocusAction::NoFocusChange, None)
                    })
                }))
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
        self.icon(icon, 22., 16., if enabled { foreground } else { disabled })
            .id(id)
            .ml(px(3.))
            .rounded(px(5.))
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
                    .on_click(cx.listener(move |workbench, _, window, cx| trigger(workbench, window, cx)))
            })
    }

    /// The chevron at the left edge that shows and hides the replace input.
    fn replace_toggle(&self, replace: bool, height: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let icon = if replace { "chevron-down" } else { "chevron-right" };
        self.icon(icon, 18., 16., self.foreground_or("editorWidget.foreground"))
            .id("find-replace-toggle")
            .absolute()
            .left(px(5.))
            .top(px(4.))
            .h(px(height - 10.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
            .on_click(cx.listener(|workbench, _, window, cx| {
                workbench.run_find(window, cx, |find, editor, viewport| {
                    let replace = !find.state.is_replace_revealed;
                    find.change(editor, viewport, NewFindState { is_replace_revealed: Some(replace), ..Default::default() }, false);
                    (FocusAction::NoFocusChange, None)
                })
            }))
    }

    /// The 2px sash on the left edge: dragging it widens the widget past its initial width, a double click
    /// toggles between the initial width and the widest (`onDidChange`, `onDidReset`).
    fn find_sash(&self, width: f32, height: f32, max_width: f32, color: Rgba, cx: &mut Context<Self>) -> Stateful<Div> {
        let workbench = cx.entity();
        let dragging = self.find.sash_drag.is_some();
        div()
            .id("find-sash")
            .absolute()
            .left_0()
            .top_0()
            .w(px(2.))
            .h(px(height - 2.))
            .bg(color)
            .cursor_col_resize()
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
