//! The Run and Debug view container (debugViewlet.ts) in the side bar: the title with the launch configuration
//! picker (debugActionViewItems.ts `StartDebugActionViewItem`), the welcome view (welcomeView.ts) and the Variables,
//! Watch, Call Stack, Breakpoints and Loaded Scripts views (variablesView.ts, watchExpressionsView.ts,
//! callStackView.ts, breakpointsView.ts, loadedScriptsView.ts).

use std::collections::HashSet;

use gpui::{ClickEvent, Context, Div, KeyDownEvent, MouseButton, MouseDownEvent, Rgba, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::input_box_view::{InputId, InputLook};
use crate::debug::session::{State, Variable};
use crate::input_box::InputBox;

/// The views of the container, in their order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Section {
    Variables,
    Watch,
    CallStack,
    Breakpoints,
    LoadedScripts,
}

/// The inline input of the Watch or Breakpoints view.
#[derive(Clone, Debug, PartialEq)]
pub enum Editing {
    NewWatch,
    Watch(String),
    NewFunctionBreakpoint,
}

pub struct DebugView {
    pub collapsed: HashSet<Section>,
    /// Tree rows the user expanded, by their path (`<scope reference>/<name>/...`), and the call stack's sessions.
    pub expanded: HashSet<String>,
    pub collapsed_rows: HashSet<String>,
    pub editing: Option<Editing>,
    pub input: InputBox,
    pub input_view: super::input_box_view::InputView,
    pub input_focus: gpui::FocusHandle,
    /// The breakpoint Run to Cursor added, removed at the next stop.
    pub run_to_cursor: Option<String>,
    pub selected: Option<String>,
    /// The glyph margin line under the pointer, which shows the breakpoint hint.
    pub hint: Option<(String, usize)>,
    /// The input element of the breakpoint widget, which shows one widget at a time.
    pub widget_view: super::input_box_view::InputView,
    pub widget_focus: gpui::FocusHandle,
    pub toolbar_drag: Option<super::debug_toolbar::ToolbarDrag>,
    pub exception: Option<super::exception_widget::ExceptionWidget>,
}

impl DebugView {
    pub fn new(cx: &mut gpui::App) -> Self {
        DebugView {
            // Loaded Scripts registers `collapsed: true`.
            collapsed: HashSet::from([Section::LoadedScripts]),
            expanded: HashSet::new(),
            collapsed_rows: HashSet::new(),
            editing: None,
            input: InputBox::default(),
            input_view: Default::default(),
            input_focus: cx.focus_handle(),
            run_to_cursor: None,
            selected: None,
            hint: None,
            widget_view: Default::default(),
            widget_focus: cx.focus_handle(),
            toolbar_drag: None,
            exception: None,
        }
    }
}

const ROW_HEIGHT: f32 = 22.;
const INDENT: f32 = 8.;

impl Workbench {
    /// The Run and Debug container below the side bar title.
    pub(super) fn debug_side_bar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let foreground = self.foreground_or("sideBar.foreground");
        let simple = self.debug.state() == State::Inactive && self.debug.sessions.is_empty() && !self.workspace_folders().iter().any(|folder| !self.folder_launch(folder).configurations.is_empty());
        let mut title = self.side_bar_title("Run and Debug");
        if !simple {
            title = title.child(self.start_debug_item(cx));
        }
        let mut body = div().flex_1().min_h_0().flex().flex_col();
        if simple {
            body = body.child(self.debug_welcome(window, cx));
            if !self.debug.model.breakpoints.is_empty() || !self.debug.model.function_breakpoints.is_empty() || !self.debug.model.exception_breakpoints.is_empty() {
                body = body.child(self.debug_section(Section::Breakpoints, "Breakpoints", false, window, cx));
            }
        } else {
            let loaded = self.debug.focused_session().is_some_and(|session| session.capabilities["supportsLoadedSourcesRequest"] == true);
            // debug.contribution.ts view `order`: Loaded Scripts (35) sits between Call Stack and Breakpoints.
            let mut sections = vec![(Section::Variables, "Variables"), (Section::Watch, "Watch"), (Section::CallStack, "Call Stack"), (Section::Breakpoints, "Breakpoints")];
            if loaded {
                sections.insert(3, (Section::LoadedScripts, "Loaded Scripts"));
            }
            for (index, (section, title)) in sections.into_iter().enumerate() {
                body = body.child(self.debug_section(section, title, index > 0, window, cx));
            }
        }
        self.side_bar_frame().text_color(foreground).child(title).child(body)
    }

    /// welcomeView.ts contents with the debuggers' `viewsWelcome` for the `debug` view.
    fn debug_welcome(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let mut content = div().px(px(20.)).py(px(13.)).flex().flex_col().gap(px(13.)).text_size(px(13.));
        let link_color = self.color("textLink.foreground");
        let context = self.key_context(window);
        let key = self.keybindings.resolver.lookup_primary_keybinding("workbench.action.debug.start", &context).map(|item| crate::keybindings::label(&item.keybinding));
        let label = match key {
            Some(key) => format!("Run and Debug ({key})"),
            None => "Run and Debug".into(),
        };
        if !self.debug.debuggers.is_empty() {
            content = content.child(self.debug_welcome_button(&label, "workbench.action.debug.start", cx));
            if self.folder.is_some() {
                content = content.child(div().child(self.welcome_paragraph(&[("To customize Run and Debug ", None), ("create a launch.json file", Some("workbench.action.debug.configure")), (".", None)], link_color, cx)));
            }
        }
        for extension in &self.extensions {
            for welcome in extension.description["contributes"]["viewsWelcome"].as_array().into_iter().flatten().filter(|welcome| welcome["view"] == "debug") {
                if !self.welcome_when_holds(welcome["when"].as_str()) {
                    continue;
                }
                for line in welcome["contents"].as_str().unwrap_or_default().split('\n').filter(|line| !line.trim().is_empty()) {
                    content = content.child(self.welcome_line(line, link_color, cx));
                }
            }
        }
        content
    }

    fn welcome_when_holds(&self, when: Option<&str>) -> bool {
        let Some(when) = when.and_then(crate::contextkey::Expr::deserialize) else { return true };
        let mut context = self.context_keys.clone();
        if let Some(editor) = self.editors.active() {
            context.insert("debugStartLanguage".into(), json!(editor.document.language_id));
        }
        context.insert("isWeb".into(), json!(false));
        when.evaluate(&context)
    }

    /// viewPane.ts `WelcomeView` rendering of one line: a lone link is a button, else a paragraph with links.
    fn welcome_line(&self, line: &str, link_color: Rgba, cx: &mut Context<Self>) -> Div {
        let parts = markdown_links(line);
        if let [(label, Some(command))] = parts.as_slice() {
            return self.debug_welcome_button(label, command, cx);
        }
        let parts: Vec<(&str, Option<&str>)> = parts.iter().map(|(text, command)| (text.as_str(), command.as_deref())).collect();
        div().child(self.welcome_paragraph(&parts, link_color, cx))
    }

    fn debug_welcome_button(&self, label: &str, command: &str, cx: &mut Context<Self>) -> Div {
        let command = command.to_owned();
        let background = self.color("button.background");
        let hover = self.color("button.hoverBackground");
        div().child(
            div()
                .id(gpui::SharedString::from(format!("welcome-{command}")))
                .h(px(26.))
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .bg(background)
                .text_color(self.color("button.foreground"))
                .cursor_pointer()
                .hover(move |button| button.bg(hover))
                .child(label.to_owned())
                .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                    let (id, args) = command_link(&command);
                    workbench.execute_command(&id, args, window, cx);
                })),
        )
    }

    fn welcome_paragraph(&self, parts: &[(&str, Option<&str>)], link_color: Rgba, cx: &mut Context<Self>) -> Div {
        let mut paragraph = div().flex().flex_wrap();
        for (index, (text, command)) in parts.iter().enumerate() {
            match command {
                None => paragraph = paragraph.child(text.to_string()),
                Some(command) => {
                    let command = command.to_string();
                    paragraph = paragraph.child(
                        div()
                            .id(("welcome-link", index))
                            .text_color(link_color)
                            .cursor_pointer()
                            .hover(|link| link.underline())
                            .child(text.to_string())
                            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                                let (id, args) = command_link(&command);
                                workbench.execute_command(&id, args, window, cx);
                            })),
                    );
                }
            }
        }
        paragraph
    }

    /// debugActionViewItems.ts `StartDebugActionViewItem`: the green start button, the configuration name, then
    /// the gear of Open launch.json.
    fn start_debug_item(&self, cx: &mut Context<Self>) -> Div {
        let (_, selected) = self.selected_launch();
        let name = selected.unwrap_or_else(|| "No Configurations".into());
        let start_color = self.color("debugIcon.startForeground");
        let hover = self.color("toolbar.hoverBackground");
        let foreground = self.foreground_or("icon.foreground");
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(22.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(self.color("dropdown.border"))
                    .bg(self.color("dropdown.background"))
                    .child(self.icon("debug-start", 22., 16., start_color).id("debug-start").cursor_pointer().hover(move |button| button.bg(hover)).on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| {
                        workbench.execute_command("workbench.action.debug.start", Vec::new(), window, cx);
                    })))
                    .child(
                        div()
                            .id("debug-configuration")
                            .max_w(px(120.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .pl(px(4.))
                            .pr(px(20.))
                            .text_size(px(12.))
                            .text_color(self.color("dropdown.foreground"))
                            .cursor_pointer()
                            .child(name)
                            .on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| {
                                workbench.execute_command("workbench.action.debug.selectandstart", Vec::new(), window, cx);
                            })),
                    ),
            )
            .child(self.search_action("gear", "workbench.action.debug.configure", self.folder.is_some(), cx).text_color(foreground))
    }

    /// A pane: its header with the view's actions while expanded, then its rows.
    fn debug_section(&self, section: Section, title: &str, separated: bool, window: &Window, cx: &mut Context<Self>) -> Div {
        let expanded = !self.debug_view.collapsed.contains(&section);
        let actions = match section {
            Section::Watch => vec![("add", "workbench.debug.viewlet.action.addWatchExpression", true), ("close-all", "workbench.debug.viewlet.action.removeAllWatchExpressions", !self.debug.model.watch_expressions.is_empty())],
            Section::Breakpoints => vec![
                ("add", "workbench.debug.viewlet.action.addFunctionBreakpointAction", true),
                ("activate-breakpoints", "workbench.debug.viewlet.action.toggleBreakpointsActivatedAction", true),
                ("close-all", "workbench.debug.viewlet.action.removeAllBreakpoints", !self.debug.model.breakpoints.is_empty() || !self.debug.model.function_breakpoints.is_empty()),
            ],
            _ => Vec::new(),
        };
        let mut header = self.section_header(title, expanded, separated).id(("debug-section", section as usize)).on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
            if !workbench.debug_view.collapsed.remove(&section) {
                workbench.debug_view.collapsed.insert(section);
            }
            cx.notify();
        }));
        if expanded && !actions.is_empty() {
            let mut bar = div().ml_auto().mr(px(4.)).flex().gap(px(2.));
            for (icon, command, enabled) in actions {
                bar = bar.child(self.search_action(icon, command, enabled, cx));
            }
            header = header.child(bar);
        }
        let mut pane = div().flex().flex_col().child(header.debug_selector(|| format!("debug-section-{}", section as usize)));
        if expanded {
            let rows = match section {
                Section::Variables => self.variables_rows(cx),
                Section::Watch => self.watch_rows(window, cx),
                Section::CallStack => self.call_stack_rows(cx),
                Section::Breakpoints => self.breakpoint_rows(window, cx),
                Section::LoadedScripts => self.loaded_script_rows(),
            };
            // debug.contribution.ts view `weight`s share the container's height.
            let weight = match section {
                Section::Variables => 40.,
                Section::Watch => 10.,
                Section::CallStack => 30.,
                Section::Breakpoints => 20.,
                Section::LoadedScripts => 5.,
            };
            pane = pane.flex_1().min_h_0();
            pane.style().flex_grow = Some(weight);
            pane = pane.child(div().id(("debug-rows", section as usize)).flex_1().min_h_0().overflow_y_scroll().children(rows));
        }
        pane
    }

    fn debug_row(&self, key: String, depth: usize, twistie: Option<bool>) -> gpui::Stateful<Div> {
        let hover = self.color("list.hoverBackground");
        let selected = self.debug_view.selected.as_deref() == Some(key.as_str());
        let mut row = div()
            .id(gpui::SharedString::from(key))
            .h(px(ROW_HEIGHT))
            .mx(px(4.))
            .rounded(px(4.))
            .flex()
            .items_center()
            .cursor_pointer()
            .hover(move |row| row.bg(hover))
            .when(selected, |row| row.bg(self.color("list.inactiveSelectionBackground")))
            .pl(px(INDENT * depth as f32 + 4.));
        let foreground = self.foreground_or("icon.foreground");
        row = match twistie {
            Some(true) => row.child(self.icon("chevron-down", 16., 16., foreground)),
            Some(false) => row.child(self.icon("chevron-right", 16., 16., foreground)),
            None => row.child(div().w(px(16.)).flex_none()),
        };
        row
    }

    /// debugExpressionRenderer.ts `renderExpressionValue`: numbers, strings and booleans in their token colors.
    fn value_color(&self, variable: &Variable) -> Rgba {
        let value = variable.value.as_str();
        if variable.error {
            return self.color("debugTokenExpression.error");
        }
        if value.parse::<f64>().is_ok() {
            return self.color("debugTokenExpression.number");
        }
        if value == "true" || value == "false" {
            return self.color("debugTokenExpression.boolean");
        }
        if value.starts_with('"') || value.starts_with('\'') || value.starts_with('`') {
            return self.color("debugTokenExpression.string");
        }
        self.color("debugTokenExpression.value")
    }

    /// `name: value` of a variable, watch or REPL result row.
    pub(super) fn expression_label(&self, variable: &Variable) -> Div {
        let mut label = div().flex().min_w_0().overflow_hidden().whitespace_nowrap();
        if !variable.name.is_empty() {
            label = label.child(div().text_color(self.color("debugTokenExpression.name")).child(variable.name.clone())).child(div().child(": ").mr(px(0.)));
        }
        label.child(div().text_color(self.value_color(variable)).overflow_hidden().text_ellipsis().child(variable.value.clone()))
    }

    fn variables_rows(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let Some(session) = self.debug.focus.session.clone() else { return Vec::new() };
        let mut rows = Vec::new();
        let first_cheap = self.debug.scopes.iter().position(|scope| !scope.expensive);
        for (index, scope) in self.debug.scopes.iter().enumerate() {
            let key = format!("scope/{}", scope.name);
            let expanded = self.row_expanded(&key, Some(index) == first_cheap);
            let reference = scope.variables_reference;
            let row_key = key.clone();
            let session_id = session.clone();
            rows.push(
                self.debug_row(key.clone(), 0, Some(expanded))
                    .child(div().child(scope.name.clone()))
                    .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| workbench.toggle_debug_row(&row_key, expanded, &session_id, reference, window, cx)))
                    .into_any_element(),
            );
            if expanded {
                self.variable_children(&session, reference, &key, 1, &mut rows, cx);
            }
        }
        rows
    }

    fn variable_children(&self, session: &str, reference: i64, parent: &str, depth: usize, rows: &mut Vec<gpui::AnyElement>, cx: &mut Context<Self>) {
        let Some(children) = self.debug.children.get(&(session.to_owned(), reference)) else { return };
        for variable in children {
            let key = format!("{parent}/{}", variable.name);
            let expandable = variable.has_children();
            let expanded = expandable && self.row_expanded(&key, false);
            let (row_key, session_id, child_reference) = (key.clone(), session.to_owned(), variable.variables_reference);
            let mut row = self.debug_row(key.clone(), depth, expandable.then_some(expanded)).child(self.expression_label(variable));
            if expandable {
                row = row.on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| workbench.toggle_debug_row(&row_key, expanded, &session_id, child_reference, window, cx)));
            }
            rows.push(row.into_any_element());
            if expanded {
                self.variable_children(session, variable.variables_reference, &key, depth + 1, rows, cx);
            }
        }
    }

    fn row_expanded(&self, key: &str, default: bool) -> bool {
        if self.debug_view.expanded.contains(key) {
            return true;
        }
        default && !self.debug_view.collapsed_rows.contains(key)
    }

    fn toggle_debug_row(&mut self, key: &str, expanded: bool, session: &str, reference: i64, window: &mut Window, cx: &mut Context<Self>) {
        if expanded {
            self.debug_view.expanded.remove(key);
            self.debug_view.collapsed_rows.insert(key.to_owned());
        } else {
            self.debug_view.collapsed_rows.remove(key);
            self.debug_view.expanded.insert(key.to_owned());
            self.fetch_children(session, reference, window, cx);
        }
        self.debug_view.selected = Some(key.to_owned());
        cx.notify();
    }

    fn watch_rows(&self, window: &Window, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let mut rows = Vec::new();
        let session = self.debug.focus.session.clone().unwrap_or_default();
        for expression in &self.debug.model.watch_expressions {
            let key = format!("watch/{}", expression.id);
            if self.debug_view.editing == Some(Editing::Watch(expression.id.clone())) {
                rows.push(self.debug_input_row(key, window, cx));
                continue;
            }
            let value = self.debug.watch_values.get(&expression.id).cloned().unwrap_or_else(|| Variable { name: expression.name.clone(), value: "not available".into(), ..Variable::default() });
            let expandable = value.has_children();
            let expanded = expandable && self.row_expanded(&key, false);
            let (row_key, session_id, reference, id) = (key.clone(), session.clone(), value.variables_reference, expression.id.clone());
            let mut row = self.debug_row(key.clone(), 0, expandable.then_some(expanded)).child(self.expression_label(&Variable { name: expression.name.clone(), ..value.clone() }));
            row = row.on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| {
                if event.click_count() == 2 {
                    workbench.start_debug_view_edit(Editing::Watch(id.clone()), window, cx);
                    return;
                }
                if expandable {
                    workbench.toggle_debug_row(&row_key, expanded, &session_id, reference, window, cx);
                }
            }));
            rows.push(row.into_any_element());
            if expanded {
                self.variable_children(&session, value.variables_reference, &key, 1, &mut rows, cx);
            }
        }
        if self.debug_view.editing == Some(Editing::NewWatch) {
            rows.push(self.debug_input_row("watch/new".into(), window, cx));
        }
        rows
    }

    fn call_stack_rows(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let mut rows = Vec::new();
        let several = self.debug.sessions.iter().filter(|session| session.parent.is_none() || session.options["compact"] != true).count() > 1;
        let label_background = self.color("debugView.stateLabelBackground");
        let label_foreground = self.color("debugView.stateLabelForeground");
        let description = self.color("descriptionForeground");
        let state_label = |text: String| div().ml_auto().mr(px(8.)).px(px(4.)).rounded(px(3.)).text_size(px(10.)).bg(label_background).text_color(label_foreground).child(text.to_uppercase());
        for session in &self.debug.sessions {
            let depth = usize::from(several);
            if several {
                let state = match session.state(None) {
                    State::Stopped => "Paused".to_owned(),
                    State::Running => "Running".into(),
                    State::Initializing => "Initializing".into(),
                    State::Inactive => String::new(),
                };
                rows.push(self.debug_row(format!("session/{}", session.id), 0, Some(true)).child(self.icon("debug-alt-small", 16., 16., self.foreground_or("icon.foreground"))).child(session.name.clone()).child(state_label(state)).into_any_element());
            }
            for thread in &session.threads {
                let thread_key = format!("thread/{}/{}", session.id, thread.id);
                let show_thread = session.threads.len() > 1 || several;
                if show_thread {
                    rows.push(self.debug_row(thread_key.clone(), depth, Some(true)).child(thread.name.clone()).child(state_label(thread.state_label())).into_any_element());
                }
                let frame_depth = depth + usize::from(show_thread);
                let frames = &thread.frames;
                let mut index = 0;
                while index < frames.len() {
                    let frame = &frames[index];
                    index += 1;
                    // callStackView.ts `getThreadChildren`: two or more deemphasized frames in a row fold into one row.
                    let shown_key = format!("deemphasized/{}/{}", session.id, frame.id);
                    let run = frames[index - 1..].iter().take_while(|frame| frame.deemphasized() && frame.source.is_object()).count();
                    if run > 1 && !self.debug_view.expanded.contains(&shown_key) {
                        let keys: Vec<String> = frames[index - 1..index - 1 + run].iter().map(|frame| format!("deemphasized/{}/{}", session.id, frame.id)).collect();
                        rows.push(
                            self.debug_row(format!("more-frames/{}/{}", session.id, frame.id), frame_depth, None)
                                .justify_center()
                                .text_color(self.color("descriptionForeground"))
                                .child(format!("Show {run} More Stack Frames"))
                                .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                                    workbench.debug_view.expanded.extend(keys.iter().cloned());
                                    cx.notify();
                                }))
                                .into_any_element(),
                        );
                        index += run - 1;
                        continue;
                    }
                    if frame.presentation_hint.as_deref() == Some("label") {
                        rows.push(self.debug_row(format!("frame/{}/{}", session.id, frame.id), frame_depth, None).justify_center().italic().text_color(self.color("descriptionForeground")).child(frame.name.clone()).into_any_element());
                        continue;
                    }
                    let focused = self.debug.focus.session.as_deref() == Some(session.id.as_str()) && self.debug.focus.frame == Some(frame.id);
                    let (session_id, thread_id, frame_id) = (session.id.clone(), thread.id, frame.id);
                    let file = format!("{} {}:{}", frame.source_name(), frame.line, frame.column);
                    let row = self
                        .debug_row(format!("frame/{}/{}", session.id, frame.id), frame_depth, None)
                        .when(focused, |row| row.bg(self.color("list.inactiveSelectionBackground")))
                        .when(frame.deemphasized(), |row| row.opacity(0.7))
                        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(frame.name.clone()))
                        .child(div().flex_none().mr(px(8.)).text_size(px(11.)).text_color(description).child(file))
                        .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                            let focus = super::debug_service::DebugFocus { session: Some(session_id.clone()), thread: Some(thread_id), frame: Some(frame_id) };
                            workbench.set_debug_focus(focus, true, window, cx);
                        }));
                    rows.push(row.into_any_element());
                }
                if thread.stopped && thread.total_frames.is_some() {
                    let (session_id, thread_id) = (session.id.clone(), thread.id);
                    rows.push(
                        self.debug_row(format!("more/{}/{}", session.id, thread.id), frame_depth, None)
                            .text_color(self.color("textLink.foreground"))
                            .child("Load More Stack Frames")
                            .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| workbench.fetch_more_frames(&session_id, thread_id, cx)))
                            .into_any_element(),
                    );
                }
            }
        }
        rows
    }

    fn breakpoint_rows(&self, window: &Window, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let mut rows = Vec::new();
        let model = &self.debug.model;
        let focused = self.debug.focus.session.as_deref();
        let description = self.color("descriptionForeground");
        let debug_active = matches!(self.debug.state(), State::Running | State::Stopped);
        for breakpoint in model.exception_breakpoints_for(focused).collect::<Vec<_>>() {
            let id = breakpoint.base.id.clone();
            let enabled = breakpoint.base.enabled;
            rows.push(
                self.debug_row(format!("exception/{id}"), 0, None)
                    .child(self.debug_checkbox(&id, enabled, cx))
                    .child(div().child(breakpoint.label.clone()))
                    .into_any_element(),
            );
        }
        for breakpoint in &model.function_breakpoints {
            let id = breakpoint.base.id.clone();
            let icon = if !breakpoint.base.enabled || !model.breakpoints_activated { "debug-breakpoint-function-disabled" } else { "debug-breakpoint-function" };
            rows.push(
                self.debug_row(format!("function/{id}"), 0, None)
                    .child(self.breakpoint_icon(icon, 16.))
                    .child(self.debug_checkbox(&id, breakpoint.base.enabled, cx))
                    .child(div().child(breakpoint.name.clone()))
                    .into_any_element(),
            );
        }
        if self.debug_view.editing == Some(Editing::NewFunctionBreakpoint) {
            rows.push(self.debug_input_row("function/new".into(), window, cx));
        }
        for breakpoint in &model.breakpoints {
            let id = breakpoint.id().to_owned();
            let dirty = self.editors.editors.iter().chain(&self.editors.models).any(|editor| editor.document.uri_string() == breakpoint.uri_string() && editor.is_dirty());
            let (icon, _) = super::breakpoint_widget::breakpoint_icon(breakpoint, model, debug_active, dirty);
            let path = breakpoint.uri["path"].as_str().unwrap_or_default();
            let name = path.rsplit('/').next().unwrap_or_default().to_owned();
            let folder = self.relative_folder_label(path);
            let line = breakpoint.line_number();
            let uri = breakpoint.uri.clone();
            rows.push(
                self.debug_row(format!("breakpoint/{id}"), 0, None)
                    .child(self.breakpoint_icon(icon, 16.))
                    .child(self.debug_checkbox(&id, breakpoint.base.enabled, cx))
                    .child(div().flex_none().child(name))
                    .child(div().flex_1().min_w_0().ml(px(6.)).overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(12.)).text_color(description).child(folder))
                    .child(div().flex_none().mr(px(8.)).px(px(4.)).rounded(px(11.)).text_size(px(11.)).bg(self.color("badge.background")).text_color(self.color("badge.foreground")).child(line.to_string()))
                    .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                        let Some(path) = uri["path"].as_str().map(std::path::PathBuf::from).filter(|_| uri["scheme"] == "file") else { return };
                        workbench.open_file(path.clone(), false, window, cx);
                        let line = line as usize;
                        workbench.select_range(&path, crate::anything_quick_access::TextRange { start_line_number: line, start_column: 1, end_line_number: line, end_column: 1 }, cx);
                    }))
                    .into_any_element(),
            );
        }
        rows
    }

    fn loaded_script_rows(&self) -> Vec<gpui::AnyElement> {
        let Some(session) = self.debug.focused_session() else { return Vec::new() };
        session
            .loaded_sources
            .iter()
            .map(|source| {
                let name = source["name"].as_str().or(source["path"].as_str()).unwrap_or_default().to_owned();
                self.debug_row(format!("source/{name}"), 0, None).child(name).into_any_element()
            })
            .collect()
    }

    /// The folder of a breakpoint's file relative to the workspace (`labelService.getUriLabel(dirname, { relative })`).
    fn relative_folder_label(&self, path: &str) -> String {
        let parent = std::path::Path::new(path).parent().unwrap_or(std::path::Path::new(""));
        for folder in self.workspace_folders() {
            if let Ok(relative) = parent.strip_prefix(&folder) {
                return relative.to_string_lossy().into_owned();
            }
        }
        parent.to_string_lossy().into_owned()
    }

    pub(super) fn breakpoint_icon(&self, icon: &str, size: f32) -> Div {
        let (file, color) = super::breakpoint_widget::icon_file_and_color(icon);
        self.icon(file, size, size, self.color(color))
    }

    /// The enablement checkbox of a breakpoint row (breakpointsView.ts `createCheckbox`).
    fn debug_checkbox(&self, id: &str, enabled: bool, cx: &mut Context<Self>) -> Div {
        let id = id.to_owned();
        let border = self.color("checkbox.border");
        let background = self.color("checkbox.background");
        let mut checkbox = div().id(gpui::SharedString::from(format!("check-{id}"))).size(px(16.)).mx(px(4.)).rounded(px(3.)).border_1().border_color(border).bg(background).flex().items_center().justify_center();
        if enabled {
            checkbox = checkbox.child(self.icon("check", 14., 14., self.color("checkbox.foreground")));
        }
        div().child(checkbox.on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, _, cx| {
            cx.stop_propagation();
            let change = workbench.debug.model.set_enablement(&id, !enabled);
            workbench.breakpoints_changed(change, true, cx);
        })))
    }

    fn start_debug_view_edit(&mut self, editing: Editing, window: &mut Window, cx: &mut Context<Self>) {
        let mut input = InputBox::default();
        if let Editing::Watch(id) = &editing
            && let Some(expression) = self.debug.model.watch_expressions.iter().find(|expression| expression.id == *id)
        {
            input.set_value(expression.name.clone());
            input.select_all();
        }
        self.debug_view.input = input;
        self.debug_view.editing = Some(editing);
        window.focus(&self.debug_view.input_focus, cx);
        cx.notify();
    }

    /// The inline input of the Watch and Breakpoints views: Enter accepts, Escape cancels, as the trees'
    /// `renderInputBox` does.
    fn debug_input_row(&self, key: String, window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let focused = self.debug_view.input_focus.is_focused(window);
        let border = self.color("focusBorder");
        self.debug_row(key, 0, None)
            .child(
                div()
                    .flex_1()
                    .h(px(20.))
                    .mr(px(4.))
                    .px(px(4.))
                    .flex()
                    .bg(self.color("input.background"))
                    .border_1()
                    .border_color(border)
                    .on_key_down(cx.listener(|workbench, event: &KeyDownEvent, window, cx| match event.keystroke.key.as_str() {
                        "enter" => workbench.accept_debug_view_input(window, cx),
                        "escape" => {
                            workbench.debug_view.editing = None;
                            cx.notify();
                        }
                        _ => {}
                    }))
                    .child(self.input_text(InputId::DebugView, InputLook { password: false, placeholder: None, focused }, cx)),
            )
            .into_any_element()
    }

    fn accept_debug_view_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.debug_view.input.value.trim().to_owned();
        match self.debug_view.editing.take() {
            Some(Editing::NewWatch) if !value.is_empty() => _ = self.debug.model.add_watch_expression(&value),
            Some(Editing::Watch(id)) if value.is_empty() => self.debug.model.remove_watch_expressions(Some(&id)),
            Some(Editing::Watch(id)) => self.debug.model.rename_watch_expression(&id, &value),
            Some(Editing::NewFunctionBreakpoint) if !value.is_empty() => {
                let change = self.debug.model.add_function_breakpoint(&value, &json!({}), None);
                self.breakpoints_changed(change, true, cx);
            }
            _ => {}
        }
        self.store_debug_model();
        self.refresh_focus(window, cx);
    }

    pub(super) fn edit_debug_view_input(&mut self, edit: impl FnOnce(&mut InputBox), cx: &mut Context<Self>) {
        edit(&mut self.debug_view.input);
        cx.notify();
    }
}

/// `command:` links of welcome content: the id and its JSON arguments.
fn command_link(link: &str) -> (String, Vec<Value>) {
    let link = link.strip_prefix("command:").unwrap_or(link);
    match link.split_once('?') {
        Some((id, query)) => {
            let decoded = crate::debug::session::percent_decode(query);
            let args = match serde_json::from_str::<Value>(&decoded) {
                Ok(Value::Array(args)) => args,
                Ok(arg) => vec![arg],
                Err(_) => Vec::new(),
            };
            (id.to_owned(), args)
        }
        None => (link.to_owned(), Vec::new()),
    }
}

/// `[label](target)` links of one welcome line, with the text between them.
fn markdown_links(line: &str) -> Vec<(String, Option<String>)> {
    let mut parts = Vec::new();
    let mut rest = line.trim();
    while let Some(start) = rest.find('[') {
        let Some(middle) = rest[start..].find("](") else { break };
        let Some(end) = rest[start + middle..].find(')') else { break };
        if start > 0 {
            parts.push((rest[..start].to_owned(), None));
        }
        parts.push((rest[start + 1..start + middle].to_owned(), Some(rest[start + middle + 2..start + middle + end].to_owned())));
        rest = &rest[start + middle + end + 1..];
    }
    if !rest.is_empty() {
        parts.push((rest.to_owned(), None));
    }
    parts
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gpui::TestAppContext;

    use super::*;
    use crate::host_sync::HostSync;
    use crate::languages::Languages;
    use crate::theme::load_theme;

    #[gpui::test]
    fn every_section_header_shows_while_a_session_runs(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let (_workbench, cx) = cx.add_window_view(|_, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
            let mut session = crate::debug::session::Session::new("s".into(), 1, json!({ "name": "Launch", "type": "node", "request": "launch" }), json!({}), None, json!({}));
            session.capabilities = json!({ "supportsLoadedSourcesRequest": true });
            session.set_threads(&[json!({ "id": 1, "name": "main" })]);
            for index in 0..40 {
                session.accept_loaded_source(&json!({ "reason": "new", "source": { "name": format!("s{index}.js"), "path": format!("/s{index}.js") } }));
            }
            workbench.debug.sessions.push(session);
            workbench.debug.sessions.push(crate::debug::session::Session::new("c".into(), 2, json!({ "name": "Child", "type": "pwa-node", "request": "launch" }), json!({}), None, json!({ "parentSessionId": "s" })));
            workbench.debug.focus.session = Some("s".into());
            workbench.show_debug_view(cx);
            workbench
        });
        cx.run_until_parked();
        for section in 0..4 {
            let bounds = cx.debug_bounds(["debug-section-0", "debug-section-1", "debug-section-2", "debug-section-3"][section]);
            assert!(bounds.is_some_and(|bounds| bounds.size.height > gpui::px(0.)), "section {section}: {bounds:?}");
        }
    }

    #[test]
    fn welcome_lines_split_into_text_and_links() {
        assert_eq!(markdown_links("[JavaScript Debug Terminal](command:extension.js-debug.createDebuggerTerminal)"), vec![("JavaScript Debug Terminal".into(), Some("command:extension.js-debug.createDebuggerTerminal".into()))]);
        assert_eq!(markdown_links("To customize, [open a folder](command:x) now."), vec![("To customize, ".into(), None), ("open a folder".into(), Some("command:x".into())), (" now.".into(), None)]);
        assert_eq!(command_link("command:a.b?%5B1%5D"), ("a.b".into(), vec![json!(1)]));
    }
}
