//! Breakpoints in the editor: the glyph margin decorations, hint, clicks and context menu of
//! breakpointEditorContribution.ts, the call stack line decorations of callStackEditorContribution.ts, and the
//! breakpoint widget (breakpointWidget.ts) that edits a condition, hit count or log message in a zone below the line.

use gpui::{AnyElement, ClickEvent, Context, Div, KeyDownEvent, MouseButton, MouseDownEvent, Rgba, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::input_box_view::{InputId, InputLook};
use crate::commands::Outcome;
use crate::debug::{Breakpoint, BreakpointData, DebugModel};
use crate::editor::{OpenEditor, Position};
use crate::input_box::InputBox;
use crate::menus::MenuAction;

/// breakpointWidget.ts `Context`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetContext {
    Condition,
    HitCount,
    LogMessage,
}

pub struct BreakpointWidget {
    pub model_uri: String,
    pub uri: Value,
    /// 1-based line and the zone that shows the widget below it.
    pub line: u32,
    pub zone_id: u32,
    pub breakpoint: Option<String>,
    pub context: WidgetContext,
    pub input: InputBox,
    /// The values typed for the other contexts before switching (`conditionInput`, `hitCountInput`, `logMessageInput`).
    pub condition: String,
    pub hit_count: String,
    pub log_message: String,
}

/// getBreakpointMessageAndIcon for a source breakpoint: the icon id and the hover message.
pub fn breakpoint_icon(breakpoint: &Breakpoint, model: &DebugModel, debug_active: bool, dirty: bool) -> (&'static str, String) {
    let log = breakpoint.base.log_message.is_some();
    if !breakpoint.base.enabled || !model.breakpoints_activated {
        let icon = if log { "debug-breakpoint-log-disabled" } else { "debug-breakpoint-disabled" };
        return (icon, if log { "Disabled Logpoint".into() } else { "Disabled Breakpoint".into() });
    }
    if debug_active && breakpoint.pending() {
        return ("debug-breakpoint-pending", String::new());
    }
    if debug_active && !breakpoint.verified(dirty) {
        let icon = if log { "debug-breakpoint-log-unverified" } else { "debug-breakpoint-unverified" };
        let message = breakpoint.message(dirty).unwrap_or_else(|| if log { "Unverified Logpoint".into() } else { "Unverified Breakpoint".into() });
        return (icon, message);
    }
    let triggering = breakpoint.triggered_by.as_deref().and_then(|id| model.breakpoint(id));
    let append = |text: String| match breakpoint.message(dirty) {
        Some(message) => format!("{text}, {message}"),
        None => text,
    };
    if log || breakpoint.base.condition.is_some() || breakpoint.base.hit_condition.is_some() || triggering.is_some() {
        let mut messages = Vec::new();
        let mut icon = if log { "debug-breakpoint-log" } else { "debug-breakpoint-conditional" };
        if !breakpoint.supported() {
            icon = "debug-breakpoint-unsupported";
            messages.push("Breakpoints of this type are not supported by the debugger".to_owned());
        }
        if let Some(message) = &breakpoint.base.log_message {
            messages.push(format!("Log Message: {message}"));
        }
        if let Some(condition) = &breakpoint.base.condition {
            messages.push(format!("Condition: {condition}"));
        }
        if let Some(hit_condition) = &breakpoint.base.hit_condition {
            messages.push(format!("Hit Count: {hit_condition}"));
        }
        if let Some(trigger) = triggering {
            let name = trigger.uri["path"].as_str().and_then(|path| path.rsplit('/').next()).unwrap_or_default();
            messages.push(format!("Hit after breakpoint: {name}: {}", trigger.line_number()));
        }
        return (icon, append(messages.join("\n")));
    }
    let message = breakpoint.message(dirty).unwrap_or_else(|| breakpoint.uri["path"].as_str().unwrap_or_default().to_owned());
    ("debug-breakpoint", message)
}

/// The codicon file of a debug icon id (codiconsLibrary.ts aliases) and the color debug.contribution's rules give it.
pub fn icon_file_and_color(icon: &str) -> (&'static str, &'static str) {
    match icon {
        "debug-breakpoint" => ("circle-filled", "debugIcon.breakpointForeground"),
        "debug-breakpoint-disabled" => ("circle-filled", "debugIcon.breakpointDisabledForeground"),
        "debug-breakpoint-unverified" => ("circle", "debugIcon.breakpointUnverifiedForeground"),
        "debug-breakpoint-pending" => ("circle-small-filled", "debugIcon.breakpointForeground"),
        "debug-hint" => ("circle-filled", "debugIcon.breakpointForeground"),
        "debug-breakpoint-conditional" => ("debug-breakpoint-conditional", "debugIcon.breakpointForeground"),
        "debug-breakpoint-conditional-disabled" => ("debug-breakpoint-conditional", "debugIcon.breakpointDisabledForeground"),
        "debug-breakpoint-conditional-unverified" => ("debug-breakpoint-conditional-unverified", "debugIcon.breakpointUnverifiedForeground"),
        "debug-breakpoint-log" => ("debug-breakpoint-log", "debugIcon.breakpointForeground"),
        "debug-breakpoint-log-disabled" => ("debug-breakpoint-log", "debugIcon.breakpointDisabledForeground"),
        "debug-breakpoint-log-unverified" => ("debug-breakpoint-log-unverified", "debugIcon.breakpointUnverifiedForeground"),
        "debug-breakpoint-function" => ("debug-breakpoint-function", "debugIcon.breakpointForeground"),
        "debug-breakpoint-function-disabled" => ("debug-breakpoint-function", "debugIcon.breakpointDisabledForeground"),
        "debug-breakpoint-function-unverified" => ("debug-breakpoint-function-unverified", "debugIcon.breakpointUnverifiedForeground"),
        "debug-breakpoint-unsupported" => ("debug-breakpoint-unsupported", "debugIcon.breakpointForeground"),
        "debug-stackframe" => ("debug-stackframe", "debugIcon.breakpointCurrentStackframeForeground"),
        "debug-stackframe-focused" => ("debug-stackframe", "debugIcon.breakpointStackframeForeground"),
        _ => ("circle-filled", "debugIcon.breakpointForeground"),
    }
}

/// breakpointWidget.ts placeholders, by context.
fn placeholder(context: WidgetContext) -> &'static str {
    match context {
        WidgetContext::Condition => "Break when expression evaluates to true. 'Enter' to accept, 'esc' to cancel.",
        WidgetContext::HitCount => "Break when hit count condition is met. 'Enter' to accept, 'esc' to cancel.",
        WidgetContext::LogMessage => "Message to log when breakpoint is hit. Expressions within {} are interpolated. 'Enter' to accept, 'esc' to cancel.",
    }
}

const CONTEXT_LABELS: [(WidgetContext, &str); 3] = [(WidgetContext::Condition, "Expression"), (WidgetContext::HitCount, "Hit Count"), (WidgetContext::LogMessage, "Log Message")];

impl Workbench {
    pub(super) fn register_breakpoint_widget_commands(&mut self) {
        self.commands.register("breakpointWidget.action.acceptInput", |workbench, _, window, cx| {
            workbench.accept_breakpoint_widget(window, cx);
            Outcome::done()
        });
        self.commands.register("closeBreakpointWidget", |workbench, _, window, cx| {
            workbench.close_breakpoint_widget(window, cx);
            Outcome::done()
        });
        // The glyph margin context menu's actions, with `[uri, line]` arguments.
        self.commands.register("_debug.gutter.addBreakpoint", |workbench, args, _, cx| {
            let (uri, line) = gutter_args(&args);
            let change = workbench.debug.model.add_breakpoints(&uri, vec![BreakpointData::at(line)]);
            workbench.breakpoints_changed(change, true, cx);
            Outcome::done()
        });
        for (id, context) in [("_debug.gutter.addConditionalBreakpoint", WidgetContext::Condition), ("_debug.gutter.addLogpoint", WidgetContext::LogMessage), ("_debug.gutter.editBreakpoint", WidgetContext::Condition)] {
            self.commands.register(id, move |workbench, args, window, cx| {
                let (uri, line) = gutter_args(&args);
                let uri = crate::debug::uri_string(&uri);
                let existing = workbench.debug.model.breakpoints_in(&uri).find(|breakpoint| breakpoint.line_number() == line).map(|breakpoint| breakpoint.id().to_owned());
                workbench.show_breakpoint_widget(line, existing, context, window, cx);
                Outcome::done()
            });
        }
        self.commands.register("_debug.gutter.removeBreakpoints", |workbench, args, _, cx| {
            let (uri, line) = gutter_args(&args);
            let ids = workbench.line_breakpoints(&uri, line);
            let change = workbench.debug.model.remove_breakpoints(&ids);
            workbench.breakpoints_changed(change, true, cx);
            Outcome::done()
        });
        self.commands.register("_debug.gutter.toggleEnablement", |workbench, args, _, cx| {
            let (uri, line) = gutter_args(&args);
            let ids = workbench.line_breakpoints(&uri, line);
            let enable = !ids.iter().any(|id| workbench.debug.model.breakpoint(id).is_some_and(|breakpoint| breakpoint.base.enabled));
            let mut change = crate::debug::BreakpointsChange::default();
            for id in ids {
                change.changed.extend(workbench.debug.model.set_enablement(&id, enable).changed);
            }
            workbench.breakpoints_changed(change, true, cx);
            Outcome::done()
        });
    }

    fn line_breakpoints(&self, uri: &Value, line: u32) -> Vec<String> {
        let uri = crate::debug::uri_string(uri);
        self.debug.model.breakpoints_in(&uri).filter(|breakpoint| breakpoint.line_number() == line).map(|breakpoint| breakpoint.id().to_owned()).collect()
    }

    /// The glyph margin of one line: its breakpoint, the stack frame arrow, or the hint under the pointer.
    pub(super) fn breakpoint_glyph_margin(&self, editor: &OpenEditor, index: usize, width: f32, line_height: f32, cx: &mut Context<Self>) -> AnyElement {
        let line = index as u32 + 1;
        let uri_string = editor.document().uri_string();
        let model = &self.debug.model;
        let debug_active = matches!(self.debug.state(), crate::debug::session::State::Running | crate::debug::session::State::Stopped);
        let dirty = editor.is_dirty();
        let breakpoint = model.breakpoints_in(&uri_string).find(|breakpoint| breakpoint.line_number() == line);
        let mut cell = div().id(("glyph-margin", index)).relative().w(px(width)).h(px(line_height)).flex_none().flex().items_center().justify_center();
        let frame = self.stack_frame_glyph(editor, line);
        match breakpoint {
            Some(breakpoint) => {
                let (icon, _) = breakpoint_icon(breakpoint, model, debug_active, dirty);
                cell = cell.child(div().absolute().inset_0().flex().items_center().justify_center().child(self.breakpoint_icon(icon, 16.)));
            }
            None if frame.is_none() && self.debug_view_hint_line() == Some((uri_string.clone(), index)) => {
                cell = cell.child(div().absolute().inset_0().flex().items_center().justify_center().opacity(0.4).child(self.breakpoint_icon("debug-hint", 16.)));
            }
            None if frame.is_none() => cell = cell.children(self.test_glyph(editor, line, cx)),
            None => {}
        }
        if let Some(icon) = frame {
            cell = cell.child(div().absolute().inset_0().flex().items_center().justify_center().child(self.breakpoint_icon(icon, 16.)));
        }
        let uri = editor.document().uri();
        let hover_uri = uri_string.clone();
        let menu_uri = uri.clone();
        cell.cursor_pointer()
            .on_hover(cx.listener(move |workbench, hovered: &bool, _, cx| {
                let hint = &mut workbench.debug_view.hint;
                match hovered {
                    true => *hint = Some((hover_uri.clone(), index)),
                    false if hint.as_ref() == Some(&(hover_uri.clone(), index)) => *hint = None,
                    false => return,
                }
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                workbench.glyph_margin_clicked(&uri, line, event.modifiers.shift, window, cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                workbench.show_glyph_margin_menu(&menu_uri, line, event.position, window, cx);
            }))
            .into_any_element()
    }

    fn debug_view_hint_line(&self) -> Option<(String, usize)> {
        self.debug_view.hint.clone()
    }

    /// callStackEditorContribution.ts: the top frame of each stopped thread in this file, and the focused frame.
    fn stack_frames_in(&self, editor: &OpenEditor) -> Vec<(u32, bool, bool)> {
        let path = editor.document().path.to_string_lossy().into_owned();
        let mut frames = Vec::new();
        for session in &self.debug.sessions {
            let session_focused = self.debug.focus.session.as_deref() == Some(session.id.as_str());
            for thread in session.threads.iter().filter(|thread| thread.stopped) {
                let Some(top) = thread.frames.first() else { continue };
                let focused = thread.frames.iter().find(|frame| session_focused && Some(frame.id) == self.debug.focus.frame && frame.id != top.id);
                for (frame, is_top) in focused.map(|frame| (frame, false)).into_iter().chain([(top, true)]) {
                    if frame.path() == Some(&*path) && editor.document().scheme == "file" {
                        frames.push((frame.line, is_top, session_focused));
                    }
                }
            }
        }
        frames
    }

    fn stack_frame_glyph(&self, editor: &OpenEditor, line: u32) -> Option<&'static str> {
        let frames = self.stack_frames_in(editor);
        let (_, is_top, _) = frames.into_iter().find(|(frame_line, _, session_focused)| *frame_line == line && *session_focused)?;
        Some(if is_top { "debug-stackframe" } else { "debug-stackframe-focused" })
    }

    /// The whole-line background of a top or focused stack frame (`debug-top-stack-frame-line`).
    pub(super) fn stack_frame_line(&self, editor: &OpenEditor, index: usize) -> Option<Div> {
        let (_, is_top, _) = self.stack_frames_in(editor).into_iter().find(|(line, ..)| *line == index as u32 + 1)?;
        let color = if is_top { "editor.stackFrameHighlightBackground" } else { "editor.focusedStackFrameHighlightBackground" };
        Some(div().absolute().inset_0().bg(self.color(color)))
    }

    /// breakpointEditorContribution.ts `onEditorMouseDown` on the glyph margin: Shift toggles enablement, a click
    /// removes (asking first when a condition would be lost) or adds a breakpoint.
    fn glyph_margin_clicked(&mut self, uri: &Value, line: u32, shift: bool, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.line_breakpoints(uri, line);
        if ids.is_empty() {
            if self.can_set_breakpoints_in_active_editor() {
                let change = self.debug.model.add_breakpoints(uri, vec![BreakpointData::at(line)]);
                self.breakpoints_changed(change, true, cx);
            }
            return;
        }
        let breakpoints: Vec<Breakpoint> = ids.iter().filter_map(|id| self.debug.model.breakpoint(id).cloned()).collect();
        let enabled = breakpoints.iter().any(|breakpoint| breakpoint.base.enabled);
        if shift {
            let mut change = crate::debug::BreakpointsChange::default();
            for id in &ids {
                change.changed.extend(self.debug.model.set_enablement(id, !enabled).changed);
            }
            self.breakpoints_changed(change, true, cx);
            return;
        }
        let has_condition = breakpoints.iter().any(|breakpoint| breakpoint.base.condition.is_some() || breakpoint.base.log_message.is_some() || breakpoint.base.hit_condition.is_some() || breakpoint.triggered_by.is_some());
        if !has_condition {
            let change = self.debug.model.remove_breakpoints(&ids);
            self.breakpoints_changed(change, true, cx);
            return;
        }
        let kind = if breakpoints.iter().all(|breakpoint| breakpoint.base.log_message.is_some()) { "Logpoint" } else { "Breakpoint" };
        let (message, toggle) = match enabled {
            true => {
                let detail = if breakpoints.iter().any(|breakpoint| breakpoint.base.log_message.is_some()) { "log message" } else if breakpoints.iter().any(|breakpoint| breakpoint.base.condition.is_some()) { "condition" } else { "hit count" };
                (format!("This {} has a {detail} that will get lost on remove. Consider disabling the {} instead.", kind.to_lowercase(), kind.to_lowercase()), format!("Disable {kind}"))
            }
            false => (format!("Removing a disabled {} will lose its condition. Consider enabling the {} instead.", kind.to_lowercase(), kind.to_lowercase()), format!("Enable {kind}")),
        };
        let remove = format!("Remove {kind}");
        let answer = crate::dialog::show_message_box(window, gpui::PromptLevel::Warning, &message, None, &[&remove, &toggle, "Cancel"], None, cx);
        cx.spawn_in(window, async move |this, cx| {
            let Ok((choice, _)) = answer.await else { return };
            let _ = this.update(cx, |workbench, cx| {
                let change = match choice {
                    0 => workbench.debug.model.remove_breakpoints(&ids),
                    1 => {
                        let mut change = crate::debug::BreakpointsChange::default();
                        for id in &ids {
                            change.changed.extend(workbench.debug.model.set_enablement(id, !enabled).changed);
                        }
                        change
                    }
                    _ => return,
                };
                workbench.breakpoints_changed(change, true, cx);
            });
        })
        .detach();
    }

    /// breakpointEditorContribution.ts `getContextMenuActions`.
    fn show_glyph_margin_menu(&mut self, uri: &Value, line: u32, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.line_breakpoints(uri, line);
        let command = |id: &str, title: &str| MenuAction::Command { id: id.into(), title: title.into(), enabled: true, checked: false, alt: None };
        let actions = match ids.first().and_then(|id| self.debug.model.breakpoint(id)) {
            Some(breakpoint) => {
                let kind = if breakpoint.base.log_message.is_some() { "Logpoint" } else { "Breakpoint" };
                let toggle = if breakpoint.base.enabled { format!("Disable {kind}") } else { format!("Enable {kind}") };
                vec![command("_debug.gutter.removeBreakpoints", &format!("Remove {kind}")), command("_debug.gutter.editBreakpoint", &format!("Edit {kind}...")), command("_debug.gutter.toggleEnablement", &toggle)]
            }
            None => vec![command("_debug.gutter.addBreakpoint", "Add Breakpoint"), command("_debug.gutter.addConditionalBreakpoint", "Add Conditional Breakpoint..."), command("_debug.gutter.addLogpoint", "Add Logpoint...")],
        };
        let anchor = gpui::Bounds { origin: position, size: gpui::size(px(0.), px(0.)) };
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, vec![uri.clone(), json!(line)], window, cx);
    }

    /// `BreakpointWidget.show`: a zone of two lines below the line, the input focused with the breakpoint's value.
    pub(crate) fn show_breakpoint_widget(&mut self, line: u32, breakpoint: Option<String>, context: WidgetContext, window: &mut Window, cx: &mut Context<Self>) {
        self.close_breakpoint_widget(window, cx);
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut() else { return };
        let existing = breakpoint.as_deref().and_then(|id| self.debug.model.breakpoint(id));
        let text = |value: Option<&String>| value.cloned().unwrap_or_default();
        let (condition, hit_count, log_message) = (text(existing.and_then(|bp| bp.base.condition.as_ref())), text(existing.and_then(|bp| bp.base.hit_condition.as_ref())), text(existing.and_then(|bp| bp.base.log_message.as_ref())));
        // breakpointWidget.ts: an existing logpoint opens on its message, a hit count without a condition on it.
        let context = match (existing.is_some(), context) {
            (true, _) if !log_message.is_empty() => WidgetContext::LogMessage,
            (true, _) if condition.is_empty() && !hit_count.is_empty() => WidgetContext::HitCount,
            (_, context) => context,
        };
        let zone_id = editor.add_view_zone(Some(Position::new(line as usize - 1, 0)), 2. * viewport.line_height);
        let mut input = InputBox::default();
        input.set_value(match context {
            WidgetContext::Condition => condition.clone(),
            WidgetContext::HitCount => hit_count.clone(),
            WidgetContext::LogMessage => log_message.clone(),
        });
        let uri = editor.document().uri();
        self.breakpoint_widget = Some(BreakpointWidget { model_uri: editor.document().uri_string(), uri, line, zone_id, breakpoint, context, input, condition, hit_count, log_message });
        self.debug_view.widget_view.reset();
        window.focus(&self.debug_view.widget_focus, cx);
        cx.notify();
    }

    pub(crate) fn close_breakpoint_widget(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(widget) = self.breakpoint_widget.take() else { return };
        if let Some(editor) = self.editors.editors.iter_mut().find(|editor| editor.document().uri_string() == widget.model_uri) {
            editor.remove_view_zone(widget.zone_id);
        }
        window.focus(&self.editor_focus, cx);
        cx.notify();
    }

    /// `BreakpointWidget.close(success: true)`: the typed values update the breakpoint or add one.
    fn accept_breakpoint_widget(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(widget) = self.breakpoint_widget.as_mut() else { return };
        let value = widget.input.value.clone();
        match widget.context {
            WidgetContext::Condition => widget.condition = value,
            WidgetContext::HitCount => widget.hit_count = value,
            WidgetContext::LogMessage => widget.log_message = value,
        }
        let some = |text: &String| (!text.is_empty()).then(|| text.clone());
        let (condition, hit_condition, log_message) = (some(&widget.condition), some(&widget.hit_count), some(&widget.log_message));
        let (uri, line, breakpoint) = (widget.uri.clone(), widget.line, widget.breakpoint.clone());
        let change = match breakpoint {
            Some(id) => {
                let mut update = serde_json::Map::new();
                update.insert("condition".into(), json!(condition));
                update.insert("hitCondition".into(), json!(hit_condition));
                update.insert("logMessage".into(), json!(log_message));
                self.debug.model.update_breakpoints(&[(id, update)])
            }
            None if condition.is_some() || hit_condition.is_some() || log_message.is_some() => self.debug.model.add_breakpoints(&uri, vec![BreakpointData { condition, hit_condition, log_message, ..BreakpointData::at(line) }]),
            None => crate::debug::BreakpointsChange::default(),
        };
        self.breakpoints_changed(change, true, cx);
        self.close_breakpoint_widget(window, cx);
    }

    fn switch_breakpoint_widget_context(&mut self, context: WidgetContext, window: &mut Window, cx: &mut Context<Self>) {
        let Some(widget) = self.breakpoint_widget.as_mut() else { return };
        let value = widget.input.value.clone();
        match widget.context {
            WidgetContext::Condition => widget.condition = value,
            WidgetContext::HitCount => widget.hit_count = value,
            WidgetContext::LogMessage => widget.log_message = value,
        }
        widget.context = context;
        let next = match context {
            WidgetContext::Condition => widget.condition.clone(),
            WidgetContext::HitCount => widget.hit_count.clone(),
            WidgetContext::LogMessage => widget.log_message.clone(),
        };
        widget.input = InputBox::default();
        widget.input.set_value(next);
        window.focus(&self.debug_view.widget_focus, cx);
        cx.notify();
    }

    /// The widget over its zone: the context select box, then the input (breakpointWidget.css, zone frame in
    /// `editor.foreground`-less `peekView.border`-style top and bottom borders of `debugIcon.breakpointForeground`).
    pub(super) fn breakpoint_widget_element(&self, editor: &OpenEditor, content_left: f32, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let widget = self.breakpoint_widget.as_ref().filter(|widget| widget.model_uri == editor.document().uri_string())?;
        let viewport = self.editors.viewport;
        let top = editor.view_zone_top(widget.zone_id, viewport)? - editor.scroll.top;
        let border: Rgba = self.color("debugIcon.breakpointForeground");
        let focused = self.debug_view.widget_focus.is_focused(window);
        let label = CONTEXT_LABELS.iter().find(|(context, _)| *context == widget.context).map(|(_, label)| *label).unwrap_or_default();
        let select = div()
            .id("breakpoint-widget-select")
            .flex_none()
            .h(px(22.))
            .ml(px(content_left.max(10.) - 10.))
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(4.))
            .rounded(px(4.))
            .border_1()
            .border_color(self.color("dropdown.border"))
            .bg(self.color("dropdown.background"))
            .text_color(self.color("dropdown.foreground"))
            .cursor_pointer()
            .child(label)
            .child(self.icon("chevron-down", 16., 14., self.color("dropdown.foreground")))
            .on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| {
                let Some(widget) = workbench.breakpoint_widget.as_ref() else { return };
                let next = match widget.context {
                    WidgetContext::Condition => WidgetContext::HitCount,
                    WidgetContext::HitCount => WidgetContext::LogMessage,
                    WidgetContext::LogMessage => WidgetContext::Condition,
                };
                workbench.switch_breakpoint_widget_context(next, window, cx);
            }));
        let input = div()
            .flex_1()
            .h(px(22.))
            .ml(px(8.))
            .mr(px(20.))
            .px(px(4.))
            .flex()
            .bg(self.color("input.background"))
            .on_key_down(cx.listener(|workbench, event: &KeyDownEvent, window, cx| match event.keystroke.key.as_str() {
                "enter" => workbench.accept_breakpoint_widget(window, cx),
                "escape" => workbench.close_breakpoint_widget(window, cx),
                _ => {}
            }))
            .child(self.input_text(InputId::BreakpointWidget, InputLook { password: false, placeholder: Some(placeholder(widget.context).into()), focused }, cx));
        Some(
            div()
                .absolute()
                .left_0()
                .right(px(14.))
                .top(px(top))
                .h(px(2. * viewport.line_height))
                .border_t_2()
                .border_b_2()
                .border_color(border)
                .bg(self.color("editor.background"))
                .flex()
                .items_center()
                .child(select)
                .child(input)
                .into_any_element(),
        )
    }

    pub(super) fn edit_breakpoint_widget_input(&mut self, edit: impl FnOnce(&mut InputBox), cx: &mut Context<Self>) {
        if let Some(widget) = self.breakpoint_widget.as_mut() {
            edit(&mut widget.input);
        }
        cx.notify();
    }
}

fn gutter_args(args: &[Value]) -> (Value, u32) {
    (args.first().cloned().unwrap_or_default(), args.get(1).and_then(Value::as_u64).unwrap_or(1) as u32)
}
