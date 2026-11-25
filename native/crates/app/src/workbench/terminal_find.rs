//! terminalFindWidget.ts over the search addon: the find input with its case, whole word and regex toggles, the
//! result count, previous, next and close, drawn over the terminal's top right corner. The matches come from
//! `terminal/search.rs`; the grid paints them.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Point};
use alacritty_terminal::term::search::Match;
use gpui::{Context, Div, FocusHandle, MouseButton, MouseDownEvent, Window, div, prelude::*, px};

use super::Workbench;
use super::input_box_view::{InputId, InputLook, InputView};
use crate::commands::Outcome;
use crate::input_box::InputBox;
use crate::terminal::search::{Direction, HIGHLIGHT_LIMIT, SearchOptions, find_matches, next_index};

pub struct TerminalFind {
    pub input: InputBox,
    pub view: InputView,
    pub focus: FocusHandle,
    pub visible: bool,
    pub options: SearchOptions,
    /// The matches of the active terminal and the selected one.
    pub matches: Vec<Match>,
    pub current: Option<usize>,
}

impl TerminalFind {
    pub fn new(cx: &mut gpui::App) -> Self {
        TerminalFind { input: InputBox::new(String::new()), view: InputView::default(), focus: cx.focus_handle(), visible: false, options: SearchOptions::default(), matches: Vec::new(), current: None }
    }
}

impl Workbench {
    pub(super) fn register_terminal_find_actions(&mut self) {
        self.commands.register("workbench.action.terminal.focusFind", |workbench, _, window, cx| {
            // `TerminalFindContribution.reveal`: the selection seeds the query.
            let selection = workbench.terminal_view.active_instance_mut().and_then(|instance| instance.selection_text()).filter(|text| !text.contains('\n'));
            if let Some(selection) = selection {
                workbench.terminal_view.find.input.set_value(selection);
            }
            workbench.terminal_view.find.visible = true;
            window.focus(&workbench.terminal_view.find.focus, cx);
            workbench.terminal_find(None, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.hideFind", |workbench, _, window, cx| {
            workbench.hide_terminal_find(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.findNext", |workbench, _, _, cx| workbench.terminal_find_step(Direction::Next, cx));
        self.commands.register("workbench.action.terminal.findPrevious", |workbench, _, _, cx| workbench.terminal_find_step(Direction::Previous, cx));
        self.commands.register("workbench.action.terminal.toggleFindCaseSensitive", |workbench, _, _, cx| workbench.toggle_terminal_find_option(|options| options.case_sensitive = !options.case_sensitive, cx));
        self.commands.register("workbench.action.terminal.toggleFindWholeWord", |workbench, _, _, cx| workbench.toggle_terminal_find_option(|options| options.whole_word = !options.whole_word, cx));
        self.commands.register("workbench.action.terminal.toggleFindRegex", |workbench, _, _, cx| workbench.toggle_terminal_find_option(|options| options.regex = !options.regex, cx));
    }

    fn hide_terminal_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let find = &mut self.terminal_view.find;
        find.visible = false;
        find.matches.clear();
        find.current = None;
        window.focus(&self.terminal_view.focus, cx);
        cx.notify();
    }

    fn toggle_terminal_find_option(&mut self, toggle: impl FnOnce(&mut SearchOptions), cx: &mut Context<Self>) -> Outcome {
        toggle(&mut self.terminal_view.find.options);
        self.terminal_find(None, cx);
        Outcome::done()
    }

    fn terminal_find_step(&mut self, direction: Direction, cx: &mut Context<Self>) -> Outcome {
        self.terminal_find(Some(direction), cx);
        Outcome::done()
    }

    /// The query changed (`direction` None: `incremental` search upward from the bottom, as typing does) or a step
    /// to the next or previous match; the selected match scrolls into view.
    fn terminal_find(&mut self, direction: Option<Direction>, cx: &mut Context<Self>) {
        let query = self.terminal_view.find.input.value.clone();
        let options = self.terminal_view.find.options;
        let current = if direction.is_some() { self.terminal_view.find.current } else { None };
        let Some(instance) = self.terminal_view.active_instance_mut() else { return };
        let matches = if query.is_empty() { Vec::new() } else { find_matches(&instance.term, &query, options) };
        let bottom = Point::new(instance.term.bottommost_line() + 1, Column(0));
        let selected = next_index(&matches, current, bottom, direction.unwrap_or(Direction::Previous));
        if let Some(index) = selected {
            instance.term.scroll_to_point(*matches[index].start());
        }
        let find = &mut self.terminal_view.find;
        find.matches = matches;
        find.current = selected;
        cx.notify();
    }

    pub(super) fn edit_terminal_find_input(&mut self, edit: impl FnOnce(&mut InputBox), cx: &mut Context<Self>) {
        let before = self.terminal_view.find.input.value.clone();
        edit(&mut self.terminal_view.find.input);
        if self.terminal_view.find.input.value != before {
            self.terminal_find(None, cx);
        }
        cx.notify();
    }

    /// simpleFindWidget.css: the widget at the terminal's top right, 4px under its top edge.
    pub(super) fn terminal_find_widget(&self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let find = &self.terminal_view.find;
        if !find.visible {
            return None;
        }
        let focused = find.focus.is_focused(window);
        let count = match (find.matches.len(), find.current) {
            (0, _) if !find.input.value.is_empty() => "No results".to_owned(),
            (0, _) => String::new(),
            (total, current) => {
                let total = if total >= HIGHLIGHT_LIMIT { format!("{HIGHLIGHT_LIMIT}+") } else { total.to_string() };
                format!("{} of {total}", current.map_or("?".to_owned(), |index| (index + 1).to_string()))
            }
        };
        let options = find.options;
        let text = self.input_text(InputId::TerminalFind, InputLook { password: false, placeholder: Some("Find".into()), focused }, cx).into_any_element();
        let toggle = |icon: &'static str, checked: bool, command: &'static str| {
            let color = if checked { self.foreground_or("inputOption.activeForeground") } else { self.foreground_or("icon.foreground") };
            self.icon(icon, 20., 16., color)
                .id(command)
                .rounded(px(3.))
                .border_1()
                .border_color(gpui::transparent_black())
                .cursor_pointer()
                .when(checked, |button| button.bg(self.color("inputOption.activeBackground")).border_color(self.color("inputOption.activeBorder")))
                .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    workbench.execute_command(command, Vec::new(), window, cx);
                }))
        };
        let action = |icon: &'static str, command: &'static str| {
            self.icon(icon, 20., 16., self.foreground_or("icon.foreground"))
                .id(command)
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
                .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    workbench.execute_command(command, Vec::new(), window, cx);
                }))
        };
        let border = if focused { self.color("focusBorder") } else { self.theme.color("input.border").unwrap_or(self.color("input.background")) };
        let input = div()
            .w(px(180.))
            .h(px(24.))
            .flex()
            .items_center()
            .gap(px(2.))
            .pl(px(4.))
            .pr(px(2.))
            .border_1()
            .rounded(px(2.))
            .cursor_text()
            .bg(self.color("input.background"))
            .border_color(border)
            .text_color(self.foreground_or("input.foreground"))
            .child(div().flex_1().min_w_0().h_full().flex().child(text))
            .child(toggle("case-sensitive", options.case_sensitive, "workbench.action.terminal.toggleFindCaseSensitive"))
            .child(toggle("whole-word", options.whole_word, "workbench.action.terminal.toggleFindWholeWord"))
            .child(toggle("regex", options.regex, "workbench.action.terminal.toggleFindRegex"));
        Some(
            div()
                .absolute()
                .top(px(0.))
                .right(px(18.))
                .h(px(34.))
                .px(px(4.))
                .flex()
                .items_center()
                .gap(px(3.))
                .text_size(px(12.))
                .bg(self.color("editorWidget.background"))
                .text_color(self.foreground_or("editorWidget.foreground"))
                .shadow(vec![gpui::BoxShadow { color: self.color("widget.shadow").into(), offset: gpui::point(px(0.), px(0.)), blur_radius: px(8.), spread_radius: px(2.), inset: false }])
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(input)
                .child(div().min_w(px(69.)).px(px(3.)).child(count))
                .child(action("arrow-up", "workbench.action.terminal.findPrevious"))
                .child(action("arrow-down", "workbench.action.terminal.findNext"))
                .child(action("close", "workbench.action.terminal.hideFind")),
        )
    }
}
