//! The debug commands of debugCommands.ts, debugEditorActions.ts and the views' actions, with the context keys of
//! debug.ts that their keybindings and menus read.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::search_view::SideBarView;
use crate::commands::Outcome;
use crate::debug::BreakpointData;
use crate::debug::session::State;

/// debug.ts `REPL_VIEW_ID` and `VIEWLET_ID`.
pub const REPL_VIEW_ID: &str = "workbench.panel.repl.view";
pub const VIEWLET_ID: &str = "workbench.view.debug";

impl Workbench {
    pub(super) fn register_debug_commands(&mut self) {
        let start = |no_debug: bool| {
            move |workbench: &mut Workbench, _: Vec<Value>, window: &mut Window, cx: &mut Context<Workbench>| {
                let (folder, name) = workbench.selected_launch();
                let target = name.map_or(Value::Null, Value::String);
                Outcome::Pending(workbench.start_debugging(folder, target, json!({ "noDebug": no_debug, "startedByUser": true }), window, cx))
            }
        };
        self.commands.register("workbench.action.debug.start", start(false));
        self.commands.register("workbench.action.debug.run", start(true));
        self.commands.register("debug.openView", |workbench, _, _, cx| {
            workbench.show_debug_view(cx);
            Outcome::done()
        });
        self.commands.register(VIEWLET_ID, |workbench, _, _, cx| {
            workbench.show_debug_view(cx);
            Outcome::done()
        });
        // debugCommands.ts STOP_ID: the focused session, the root of a child its parent manages; with none, all.
        self.commands.register("workbench.action.debug.stop", |workbench, _, window, cx| {
            let session = workbench.debug.focus.session.clone();
            Outcome::Pending(workbench.stop_session(session, false, window, cx))
        });
        self.commands.register("workbench.action.debug.disconnect", |workbench, _, window, cx| {
            let session = workbench.debug.focus.session.clone();
            Outcome::Pending(workbench.stop_session(session, true, window, cx))
        });
        self.commands.register("workbench.action.debug.restart", |workbench, _, window, cx| {
            let Some(session) = workbench.debug.focus.session.clone() else { return Outcome::done() };
            let Some(found) = workbench.debug.session(&session) else { return Outcome::done() };
            let (folder, configuration, options) = (found.folder_uri.as_ref().and_then(crate::debug::configuration_folder), found.unresolved_configuration.clone(), found.options.clone());
            let stop = workbench.stop_session(Some(session), false, window, cx);
            cx.spawn_in(window, async move |this, cx| {
                let _ = stop.await;
                let _ = this.update_in(cx, |workbench, window, cx| drop(workbench.start_debugging(folder, configuration, options, window, cx)));
            })
            .detach();
            Outcome::done()
        });
        for (id, command, granularity) in [
            ("workbench.action.debug.continue", "continue", false),
            ("workbench.action.debug.stepOver", "next", true),
            ("workbench.action.debug.stepInto", "stepIn", true),
            ("workbench.action.debug.stepOut", "stepOut", true),
            ("workbench.action.debug.stepBack", "stepBack", true),
            ("workbench.action.debug.reverseContinue", "reverseContinue", false),
        ] {
            self.commands.register(id, move |workbench, _, window, cx| workbench.run_thread_command(command, granularity, window, cx));
        }
        self.commands.register("workbench.action.debug.pause", |workbench, _, window, cx| {
            let (Some(session), thread) = (workbench.debug.focus.session.clone(), workbench.debug.focus.thread) else { return Outcome::done() };
            let thread = thread.or_else(|| workbench.debug.session(&session).and_then(|session| session.threads.first()).map(|thread| thread.id));
            let reply = workbench.dap_request(&session, "pause", json!({ "threadId": thread }));
            let _ = window;
            cx.notify();
            Outcome::Pending(reply)
        });
        self.commands.register("editor.debug.action.toggleBreakpoint", |workbench, _, _, cx| {
            let Some(editor) = workbench.editors.active() else { return Outcome::done() };
            let mut lines: Vec<u32> = editor.selections.iter().map(|selection| selection.head.line as u32 + 1).collect();
            lines.dedup();
            let uri = editor.document().uri();
            for line in lines {
                workbench.toggle_breakpoint(&uri, line, None, cx);
            }
            Outcome::done()
        });
        // debugEditorActions.ts TOGGLE_INLINE_BREAKPOINT: a breakpoint at the cursor's column.
        self.commands.register("editor.debug.action.toggleInlineBreakpoint", |workbench, _, _, cx| {
            let Some(editor) = workbench.editors.active() else { return Outcome::done() };
            let head = editor.selections[0].head;
            let uri = editor.document().uri();
            workbench.toggle_breakpoint(&uri, head.line as u32 + 1, Some(head.column as u32 + 1), cx);
            Outcome::done()
        });
        for (id, kind) in [("editor.debug.action.conditionalBreakpoint", super::breakpoint_widget::WidgetContext::Condition), ("editor.debug.action.addLogPoint", super::breakpoint_widget::WidgetContext::LogMessage)] {
            self.commands.register(id, move |workbench, _, window, cx| {
                let Some(editor) = workbench.editors.active() else { return Outcome::done() };
                let line = editor.selections[0].head.line as u32 + 1;
                workbench.show_breakpoint_widget(line, None, kind, window, cx);
                Outcome::done()
            });
        }
        self.commands.register("editor.debug.action.editBreakpoint", |workbench, _, window, cx| {
            let Some(editor) = workbench.editors.active() else { return Outcome::done() };
            let line = editor.selections[0].head.line as u32 + 1;
            let uri = editor.document().uri_string();
            let id = workbench.debug.model.breakpoints_in(&uri).find(|breakpoint| breakpoint.line_number() == line).map(|breakpoint| breakpoint.id().to_owned());
            workbench.show_breakpoint_widget(line, id, super::breakpoint_widget::WidgetContext::Condition, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.debug.viewlet.action.removeAllBreakpoints", |workbench, _, _, cx| {
            let ids: Vec<String> = workbench.debug.model.breakpoints.iter().map(|breakpoint| breakpoint.id().to_owned()).collect();
            let mut change = workbench.debug.model.remove_breakpoints(&ids);
            change.removed.extend(workbench.debug.model.remove_function_breakpoints(None).removed);
            change.removed.extend(workbench.debug.model.remove_data_breakpoints(None).removed);
            workbench.breakpoints_changed(change, true, cx);
            Outcome::done()
        });
        for (id, enable) in [("workbench.debug.viewlet.action.enableAllBreakpoints", true), ("workbench.debug.viewlet.action.disableAllBreakpoints", false)] {
            self.commands.register(id, move |workbench, _, _, cx| {
                let change = workbench.debug.model.enable_or_disable_all(enable);
                workbench.breakpoints_changed(change, true, cx);
                Outcome::done()
            });
        }
        self.commands.register("workbench.debug.viewlet.action.toggleBreakpointsActivatedAction", |workbench, _, _, cx| {
            workbench.debug.model.breakpoints_activated = !workbench.debug.model.breakpoints_activated;
            workbench.breakpoints_changed(crate::debug::BreakpointsChange::default(), true, cx);
            let ids: Vec<String> = workbench.debug.sessions.iter().map(|session| session.id.clone()).collect();
            for id in ids {
                workbench.send_all_breakpoints(&id, cx);
            }
            Outcome::done()
        });
        self.commands.register("workbench.debug.viewlet.action.addFunctionBreakpointAction", |workbench, _, _, cx| {
            workbench.debug_view.editing = Some(super::debug_view::Editing::NewFunctionBreakpoint);
            workbench.debug_view.input = crate::input_box::InputBox::default();
            workbench.show_debug_view(cx);
            Outcome::done()
        });
        self.commands.register("workbench.debug.viewlet.action.addWatchExpression", |workbench, _, _, cx| {
            workbench.debug_view.editing = Some(super::debug_view::Editing::NewWatch);
            workbench.debug_view.input = crate::input_box::InputBox::default();
            workbench.show_debug_view(cx);
            Outcome::done()
        });
        self.commands.register("workbench.debug.viewlet.action.removeAllWatchExpressions", |workbench, _, window, cx| {
            workbench.debug.model.remove_watch_expressions(None);
            workbench.store_debug_model();
            workbench.refresh_focus(window, cx);
            Outcome::done()
        });
        // debugEditorActions.ts SELECTION_TO_WATCH: the selection, or the word at the cursor.
        self.commands.register("editor.debug.action.selectionToWatch", |workbench, _, window, cx| {
            let Some(expression) = workbench.selected_expression() else { return Outcome::done() };
            workbench.debug.model.add_watch_expression(&expression);
            workbench.store_debug_model();
            workbench.show_debug_view(cx);
            workbench.refresh_focus(window, cx);
            Outcome::done()
        });
        self.commands.register("editor.debug.action.selectionToRepl", |workbench, _, window, cx| {
            let Some(expression) = workbench.selected_expression() else { return Outcome::done() };
            workbench.open_debug_console(true, window, cx);
            workbench.evaluate_in_repl(expression, window, cx);
            Outcome::done()
        });
        self.commands.register("editor.debug.action.runToCursor", |workbench, _, window, cx| workbench.run_to_cursor(window, cx));
        self.commands.register("workbench.debug.action.toggleRepl", |workbench, _, window, cx| {
            match workbench.debug_console_visible() && workbench.debug_console.focus.is_focused(window) {
                true => workbench.close_debug_console(cx),
                false => workbench.open_debug_console(true, window, cx),
            }
            Outcome::done()
        });
        self.commands.register("workbench.debug.action.focusRepl", |workbench, _, window, cx| {
            workbench.open_debug_console(true, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.debug.panel.action.clearReplAction", |workbench, _, _, cx| {
            workbench.debug.repl.clear();
            cx.notify();
            Outcome::done()
        });
        // debugCommands.ts CONFIGURE / ADD_CONFIGURATION: launch.json of the selected folder, created when missing.
        self.commands.register("workbench.action.debug.configure", |workbench, _, window, cx| workbench.open_launch_json(window, cx));
        self.commands.register("debug.addConfiguration", |workbench, _, window, cx| workbench.open_launch_json(window, cx));
        self.commands.register("workbench.action.debug.selectandstart", |workbench, _, window, cx| {
            workbench.select_and_start(window, cx);
            Outcome::done()
        });
    }

    /// `debugService.getConfigurationManager().selectedConfiguration`: the stored choice while it exists, else the
    /// first folder's first configuration.
    pub(crate) fn selected_launch(&self) -> (Option<PathBuf>, Option<String>) {
        let folders = self.workspace_folders();
        if let Some((root, name)) = &self.debug.selected {
            let folder = root.clone().or_else(|| folders.first().cloned());
            if let Some(folder) = &folder
                && self.folder_launch(folder).configuration_names().contains(name)
            {
                return (Some(folder.clone()), Some(name.clone()));
            }
        }
        for folder in &folders {
            if let Some(name) = self.folder_launch(folder).configuration_names().into_iter().next() {
                return (Some(folder.clone()), Some(name));
            }
        }
        (folders.first().cloned(), None)
    }

    /// Continue and the step commands on the focused thread (debugSession.ts `next`, `stepIn`, `stepOut`):
    /// the thread runs again at once, before the adapter's `continued` event.
    fn run_thread_command(&mut self, command: &str, granularity: bool, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let (Some(session_id), Some(thread)) = (self.debug.focus.session.clone(), self.debug.focus.thread) else { return Outcome::done() };
        let mut arguments = json!({ "threadId": thread });
        if granularity && self.debug.session(&session_id).is_some_and(|session| session.capabilities["supportsSteppingGranularity"] == true) {
            arguments["granularity"] = json!("statement");
        }
        let reply = self.dap_request(&session_id, command, arguments);
        let all_threads = command == "continue";
        if let Some(session) = self.debug.session_mut(&session_id) {
            session.continue_threads(if all_threads { None } else { Some(thread) });
        }
        self.debug.focus.frame = None;
        self.debug.scopes.clear();
        self.debug.children.clear();
        let _ = window;
        cx.notify();
        Outcome::Pending(reply)
    }

    /// debugEditorActions.ts RunToCursorAction: a breakpoint only this session sees at the cursor, then continue.
    // shortcut: the temporary breakpoint is a normal one removed at the next stop, not a session-only breakpoint.
    fn run_to_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = self.editors.active() else { return Outcome::done() };
        let line = editor.selections[0].head.line as u32 + 1;
        let uri = editor.document().uri();
        let uri_string = editor.document().uri_string();
        if self.debug.model.breakpoints_in(&uri_string).any(|breakpoint| breakpoint.line_number() == line) {
            return self.run_thread_command("continue", false, window, cx);
        }
        let change = self.debug.model.add_breakpoints(&uri, vec![BreakpointData::at(line)]);
        let id = change.added[0].clone();
        self.debug_view.run_to_cursor = Some(id);
        self.breakpoints_changed(change, true, cx);
        self.run_thread_command("continue", false, window, cx)
    }

    /// TOGGLE_BREAKPOINT for one line (or one column): removes the line's breakpoints, else adds one when the
    /// language takes breakpoints.
    pub(crate) fn toggle_breakpoint(&mut self, uri: &Value, line: u32, column: Option<u32>, cx: &mut Context<Self>) {
        let uri_string = crate::debug::uri_string(uri);
        let existing: Vec<String> = self.debug.model.breakpoints_in(&uri_string).filter(|breakpoint| breakpoint.line_number() == line && (column.is_none() || breakpoint.column() == column)).map(|breakpoint| breakpoint.id().to_owned()).collect();
        let change = match existing.is_empty() {
            false => self.debug.model.remove_breakpoints(&existing),
            true if self.can_set_breakpoints_in_active_editor() => self.debug.model.add_breakpoints(uri, vec![BreakpointData { column, ..BreakpointData::at(line) }]),
            true => return,
        };
        self.breakpoints_changed(change, true, cx);
    }

    /// debugAdapterManager.ts `canSetBreakpointsIn`: languages of `contributes.breakpoints`, or everywhere with
    /// `debug.allowBreakpointsEverywhere`; never in jsonc or log files.
    pub(crate) fn can_set_breakpoints_in_active_editor(&self) -> bool {
        let Some(editor) = self.editors.active() else { return false };
        let language = editor.document().language_id.clone();
        let language = language.as_str();
        if language.is_empty() || language == "jsonc" || language == "log" {
            return false;
        }
        if self.setting("debug.allowBreakpointsEverywhere") == true {
            return true;
        }
        self.extensions.iter().any(|extension| extension.description["contributes"]["breakpoints"].as_array().is_some_and(|breakpoints| breakpoints.iter().any(|breakpoint| breakpoint["language"] == language)))
    }

    /// The selection of the active editor, else the word at its cursor (debugEditorActions.ts `getExpression`).
    fn selected_expression(&self) -> Option<String> {
        let editor = self.editors.active()?;
        let selection = editor.selections[0];
        if !selection.is_empty() {
            let (start, end) = if selection.anchor < selection.head { (selection.anchor, selection.head) } else { (selection.head, selection.anchor) };
            let lines = &editor.document().lines;
            let text: Vec<String> = (start.line..=end.line)
                .map(|line| {
                    let chars: Vec<char> = lines[line].chars().collect();
                    let from = if line == start.line { start.column } else { 0 };
                    let to = if line == end.line { end.column } else { chars.len() };
                    chars[from.min(chars.len())..to.min(chars.len())].iter().collect()
                })
                .collect();
            return Some(text.join("\n"));
        }
        let line: Vec<char> = editor.document().lines[selection.head.line].chars().collect();
        let is_word = |c: &char| c.is_alphanumeric() || *c == '_' || *c == '$';
        let mut start = selection.head.column.min(line.len());
        while start > 0 && is_word(&line[start - 1]) {
            start -= 1;
        }
        let mut end = selection.head.column.min(line.len());
        while end < line.len() && is_word(&line[end]) {
            end += 1;
        }
        (start < end).then(|| line[start..end].iter().collect())
    }

    /// debugCommands.ts CONFIGURE: the launch.json of the selected folder (`.vsrustium/` when the folder has one),
    /// created with the empty configuration list upstream writes without initial configurations.
    fn open_launch_json(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let (folder, _) = self.selected_launch();
        let Some(folder) = folder else { return Outcome::error("Open a folder or workspace to configure debugging.") };
        let file = config::service::folder_config_dir(&folder).join("launch.json");
        if !file.exists() {
            let content = "{\n\t// Use IntelliSense to learn about possible attributes.\n\t// Hover to view descriptions of existing attributes.\n\t// For more information, visit: https://go.microsoft.com/fwlink/?linkid=830387\n\t\"version\": \"0.2.0\",\n\t\"configurations\": []\n}";
            let written = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&file, content));
            if let Err(error) = written {
                return Outcome::error(format!("writing {}: {error}", file.display()));
            }
        }
        self.open_file(file, true, window, cx);
        Outcome::done()
    }

    /// debugCommands.ts SELECT_AND_START: pick a launch configuration, then start it.
    fn select_and_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut choices: Vec<(Option<PathBuf>, String)> = Vec::new();
        for folder in self.workspace_folders() {
            for name in self.folder_launch(&folder).configuration_names() {
                choices.push((Some(folder.clone()), name));
            }
        }
        let several = self.workspace_folders().len() > 1;
        let items = choices
            .iter()
            .map(|(folder, name)| (name.clone(), several.then(|| folder.as_ref().and_then(|folder| folder.file_name()).map(|name| name.to_string_lossy().into_owned())).flatten()))
            .collect();
        let picked = self.debug_pick("Select a launch configuration", items, window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let Some(index) = picked.await.ok().flatten().and_then(|index| index.parse::<usize>().ok()) else { return };
            let (folder, name) = choices[index].clone();
            let _ = this.update_in(cx, |workbench, window, cx| {
                workbench.debug.selected = Some((folder.clone(), name.clone()));
                workbench.store_selected_configuration();
                drop(workbench.start_debugging(folder, json!(name), json!({ "startedByUser": true }), window, cx));
            });
        })
        .detach();
    }

    /// debug.ts context keys.
    pub(super) fn debug_context(&self, window: &Window, context: &mut HashMap<String, Value>) {
        let state = self.debug.state();
        let focused = self.debug.focused_session();
        context.insert("debugState".into(), json!(state.context_value()));
        context.insert("inDebugMode".into(), json!(state != State::Inactive));
        context.insert("debuggersAvailable".into(), json!(!self.debug.debuggers.is_empty()));
        context.insert("debugType".into(), json!(focused.map(|session| session.session_type()).unwrap_or_default()));
        context.insert("focusedSessionIsAttach".into(), json!(focused.is_some_and(|session| session.configuration["request"] == "attach")));
        context.insert("breakpointsExist".into(), json!(!self.debug.model.breakpoints.is_empty()));
        context.insert("watchExpressionsExist".into(), json!(!self.debug.model.watch_expressions.is_empty()));
        context.insert("loadedScriptsSupported".into(), json!(focused.is_some_and(|session| session.capabilities["supportsLoadedSourcesRequest"] == true)));
        context.insert("stepBackSupported".into(), json!(focused.is_some_and(|session| session.capabilities["supportsStepBack"] == true)));
        context.insert("restartFrameSupported".into(), json!(focused.is_some_and(|session| session.capabilities["supportsRestartFrame"] == true)));
        let thread = focused.zip(self.debug.focus.thread).and_then(|(session, id)| session.thread(id));
        context.insert("threadHasMultipleStackFrames".into(), json!(thread.is_some_and(|thread| thread.frames.len() > 1 || thread.total_frames.is_some())));
        let simple = state == State::Inactive && !self.workspace_folders().iter().any(|folder| !self.folder_launch(folder).configurations.is_empty());
        context.insert("debugUx".into(), json!(if simple { "simple" } else { "default" }));
        context.insert(format!("viewContainer.{VIEWLET_ID}.enabled"), json!(true));
        context.insert(format!("{REPL_VIEW_ID}.active"), json!(true));
        context.insert("inDebugRepl".into(), json!(self.debug_console.focus.is_focused(window) || self.debug_console.input_focus.is_focused(window)));
        // repl.ts: the input is a code editor and a history navigation widget.
        if self.debug_console.input_focus.is_focused(window) {
            for key in ["textInputFocus", "inputFocus", "historyNavigationWidgetFocus", "historyNavigationForwardsEnabled", "historyNavigationBackwardsEnabled"] {
                context.insert(key.into(), json!(true));
            }
        }
        context.insert("breakpointWidgetVisible".into(), json!(self.breakpoint_widget.is_some()));
        context.insert("inBreakpointWidget".into(), json!(self.breakpoint_widget.is_some() && self.debug_view.widget_focus.is_focused(window)));
        if self.side_bar_view == SideBarView::Debug {
            context.insert("activeViewlet".into(), json!(VIEWLET_ID));
        }
    }
}
