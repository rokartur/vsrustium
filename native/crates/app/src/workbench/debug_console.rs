//! The Debug Console (repl.ts, replViewer.ts): the REPL elements of the focused session's output and evaluations,
//! and the input that evaluates an expression in the focused frame.

use gpui::{ClickEvent, Context, Div, FocusHandle, Window, div, prelude::*, px};
use serde_json::json;

use super::Workbench;
use super::input_box_view::{InputId, InputLook, InputView};
use crate::commands::Outcome;
use crate::debug::session::{ReplElement, Severity, Variable};
use super::panel::PanelView;
use crate::input_box::InputBox;

/// debug.ts `DEBUG_PANEL_ID`, the panel container of the Debug Console view.
pub const DEBUG_PANEL_ID: &str = "workbench.panel.repl";

pub struct DebugConsole {
    pub focus: FocusHandle,
    pub input_focus: FocusHandle,
    pub input: InputBox,
    pub input_view: InputView,
    /// `HistoryNavigator` of the evaluated inputs, newest last, and the position Up and Down walk.
    pub history: Vec<String>,
    history_index: Option<usize>,
}

impl DebugConsole {
    pub fn new(cx: &mut gpui::App) -> Self {
        DebugConsole { focus: cx.focus_handle(), input_focus: cx.focus_handle(), input: InputBox::default(), input_view: InputView::default(), history: Vec::new(), history_index: None }
    }
}

impl Workbench {
    /// debug.contribution.ts registers the Debug Console container in the panel at order 2, after Output.
    // shortcut: the view's filter box and session picker are not ported; its title has Clear Console only.
    pub(super) fn register_debug_console_panel_view(&mut self) {
        self.register_panel_view(PanelView {
            id: DEBUG_PANEL_ID,
            title: "Debug Console",
            render: |workbench, window, cx| workbench.debug_console_body(window, cx).into_any_element(),
            actions: |workbench, _, cx| div().flex().child(workbench.search_action("clear-all", "workbench.debug.panel.action.clearReplAction", true, cx)).into_any_element(),
            visible: |_| true,
            badge: |_| None,
        });
    }

    pub(super) fn debug_console_visible(&self) -> bool {
        self.panel_view_visible(DEBUG_PANEL_ID)
    }

    /// `viewsService.openView(REPL_VIEW_ID, focus)`.
    pub(crate) fn open_debug_console(&mut self, focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.open_panel_view(DEBUG_PANEL_ID, cx);
        if focus {
            window.focus(&self.debug_console.input_focus, cx);
        }
    }

    pub(crate) fn close_debug_console(&mut self, cx: &mut Context<Self>) {
        self.close_panel_view(DEBUG_PANEL_ID, cx);
    }

    pub(super) fn register_debug_console_commands(&mut self) {
        self.commands.register("repl.action.acceptInput", |workbench, _, window, cx| {
            let expression = std::mem::take(&mut workbench.debug_console.input).value;
            if expression.trim().is_empty() {
                return Outcome::done();
            }
            workbench.debug_console.history.retain(|entry| *entry != expression);
            workbench.debug_console.history.push(expression.clone());
            workbench.debug_console.history_index = None;
            workbench.evaluate_in_repl(expression, window, cx);
            Outcome::done()
        });
    }

    /// repl.ts `navigateHistory`, through `history.showPrevious` and `history.showNext`.
    pub(super) fn walk_repl_history(&mut self, back: bool, cx: &mut Context<Self>) {
        let console = &mut self.debug_console;
        if console.history.is_empty() {
            return;
        }
        let last = console.history.len() - 1;
        let next = match (console.history_index, back) {
            (None, true) => Some(last),
            (None, false) => return,
            (Some(index), true) => Some(index.saturating_sub(1)),
            (Some(index), false) if index < last => Some(index + 1),
            (Some(_), false) => None,
        };
        console.history_index = next;
        let value = next.map(|index| console.history[index].clone()).unwrap_or_default();
        console.input = InputBox::default();
        console.input.set_value(value);
        cx.notify();
    }

    /// `debugService.addReplExpression`: the input as an element, then the `repl` evaluation's result below it.
    pub(crate) fn evaluate_in_repl(&mut self, expression: String, window: &mut Window, cx: &mut Context<Self>) {
        let session_id = self.debug.focus.session.clone().unwrap_or_default();
        let index = self.debug.repl.len();
        self.debug.repl.push(ReplElement::Evaluation { session_id: session_id.clone(), input: expression.clone(), result: None });
        cx.notify();
        if self.debug.session(&session_id).is_none() {
            return;
        }
        let reply = self.dap_request(&session_id, "evaluate", json!({ "expression": expression, "frameId": self.debug.focus.frame, "context": "repl" }));
        cx.spawn_in(window, async move |this, cx| {
            let result = match reply.await {
                Ok(Ok(body)) => Variable::from_dap(&body),
                Ok(Err(error)) => Variable { value: error["message"].as_str().unwrap_or_default().to_owned(), error: true, ..Variable::default() },
                Err(_) => return,
            };
            let _ = this.update_in(cx, |workbench, window, cx| {
                if let Some(ReplElement::Evaluation { result: slot, .. }) = workbench.debug.repl.get_mut(index) {
                    *slot = Some(result);
                }
                // repl.ts: evaluating may have side effects, so the variables and watches are fetched again.
                workbench.refresh_focus(window, cx);
            });
        })
        .detach();
    }

    pub(super) fn edit_debug_console_input(&mut self, edit: impl FnOnce(&mut InputBox), cx: &mut Context<Self>) {
        edit(&mut self.debug_console.input);
        cx.notify();
    }

    /// The console's body under the tab strip: the elements, then the input (repl.css).
    pub(super) fn debug_console_body(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let font_size = 12.;
        let line_height = (font_size * 1.4_f32).round();
        let mut rows = div().id("repl-tree").flex_1().min_h_0().overflow_y_scroll().px(px(10.)).py(px(4.)).flex().flex_col();
        let session = self.debug.focus.session.clone();
        for (index, element) in self.debug.repl.iter().enumerate() {
            rows = rows.child(self.repl_element(index, element, session.as_deref(), line_height, cx));
        }
        let focused = self.debug_console.input_focus.is_focused(window);
        let prompt_color = self.color("debugConsoleInputIcon.foreground");
        let input = div()
            .flex_none()
            .h(px(line_height + 8.))
            .flex()
            .items_center()
            .border_t_1()
            .border_color(self.color("panel.border"))
            .child(self.icon("chevron-right", 20., 16., prompt_color))
            .child(div().flex_1().h(px(line_height)).flex().child(self.input_text(InputId::DebugConsole, InputLook { password: false, placeholder: None, focused }, cx)));
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .track_focus(&self.debug_console.focus)
            .font_family("Menlo")
            .text_size(px(font_size))
            .line_height(px(line_height))
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|workbench, _, window, cx| window.focus(&workbench.debug_console.input_focus, cx)))
            .child(rows)
            .child(input)
    }

    fn repl_element(&self, index: usize, element: &ReplElement, _session: Option<&str>, line_height: f32, cx: &mut Context<Self>) -> gpui::AnyElement {
        let severity_color = |severity: Severity| match severity {
            Severity::Warning => self.color("debugConsole.warningForeground"),
            Severity::Error => self.color("debugConsole.errorForeground"),
            Severity::Info => self.color("debugConsole.infoForeground"),
            Severity::Ignore => self.foreground_or("foreground"),
        };
        match element {
            ReplElement::Output { value, severity, source, count, .. } => {
                let text = strip_ansi(value.strip_suffix('\n').unwrap_or(value));
                let mut row = div().flex().items_start().min_h(px(line_height));
                if *count > 1 {
                    row = row.child(div().flex_none().mr(px(6.)).px(px(4.)).rounded(px(8.)).bg(self.color("badge.background")).text_color(self.color("badge.foreground")).child(count.to_string()));
                }
                row = row.child(div().flex_1().min_w_0().text_color(severity_color(*severity)).whitespace_normal().child(text));
                if let Some((path, line)) = source {
                    let name = path.rsplit('/').next().unwrap_or_default().to_owned();
                    let (path, line) = (std::path::PathBuf::from(path), *line as usize);
                    row = row.child(
                        div()
                            .id(("repl-source", index))
                            .flex_none()
                            .ml(px(8.))
                            .text_color(self.color("debugConsole.sourceForeground"))
                            .cursor_pointer()
                            .hover(|source| source.underline())
                            .child(format!("{name}:{line}"))
                            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                                workbench.open_file(path.clone(), true, window, cx);
                                workbench.select_range(&path, crate::anything_quick_access::TextRange { start_line_number: line, start_column: 1, end_line_number: line, end_column: 1 }, cx);
                            })),
                    );
                }
                row.into_any_element()
            }
            ReplElement::Variable { variable, severity, .. } => {
                let mut label = self.expression_label(variable);
                if variable.name.is_empty() {
                    label = label.text_color(severity_color(*severity));
                }
                div().flex().min_h(px(line_height)).child(self.icon(if variable.has_children() { "chevron-right" } else { "blank" }, 16., 16., self.foreground_or("icon.foreground"))).child(label).into_any_element()
            }
            ReplElement::Evaluation { input, result, .. } => {
                let icon = self.foreground_or("icon.foreground");
                let mut rows = div().flex().flex_col().child(div().flex().items_center().min_h(px(line_height)).child(self.icon("arrow-small-right", 16., 16., icon)).child(input.clone()));
                if let Some(result) = result {
                    let shown = Variable { name: String::new(), ..result.clone() };
                    rows = rows.child(div().flex().items_center().min_h(px(line_height)).child(self.icon("chevron-left", 16., 16., icon)).child(self.expression_label(&shown)));
                }
                rows.into_any_element()
            }
        }
    }
}

/// debugANSIHandling.ts draws ANSI styles; the console shows the text without its escape sequences.
// shortcut: SGR colors and styles are dropped instead of drawn.
pub(super) fn strip_ansi(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        result.push(c);
    }
    result
}
