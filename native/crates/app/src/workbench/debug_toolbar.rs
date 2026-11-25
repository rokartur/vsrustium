//! The floating debug toolbar (debugToolBar.ts): the `MenuId.DebugToolBar` actions over the window while a
//! session runs, dragged by its gripper and stored at `debug.actionswidgetposition`.

use gpui::{ClickEvent, Context, Div, MouseButton, MouseDownEvent, MouseMoveEvent, Window, div, prelude::*, px};

use super::Workbench;
use super::hover::HoverOptions;
use crate::debug::session::State;

const DEBUG_STATUS_ID: &str = "status.debug";
const POSITION_KEY: &str = "debug.actionswidgetposition";
const Y_KEY: &str = "debug.actionswidgety";
const HEIGHT: f32 = 28.;
const ACTION_WIDTH: f32 = 26.;
const DRAG_AREA_WIDTH: f32 = 20.;

/// The drag that moves the toolbar: the pointer and the toolbar's x fraction and y where it started, and where
/// it is now; the position is stored when the button is released.
#[derive(Clone, Copy, Default)]
pub struct ToolbarDrag {
    origin: (f32, f32),
    start: (f32, f32),
    current: (f32, f32),
}

/// One `registerDebugToolBarItem` action.
struct ToolbarAction {
    command: &'static str,
    title: &'static str,
    icon: &'static str,
    color: &'static str,
    enabled: bool,
}

impl Workbench {
    /// The toolbar's actions in their `order`, with their `when` and `precondition` evaluated.
    fn debug_toolbar_actions(&self) -> Vec<ToolbarAction> {
        let state = self.debug.state();
        let stopped = state == State::Stopped;
        let session = self.debug.focused_session();
        let attach = session.is_some_and(|session| session.configuration["request"] == "attach");
        let no_debug = session.is_some_and(|session| session.options["noDebug"] == true);
        let step_back = session.is_some_and(|session| session.capabilities["supportsStepBack"] == true);
        let thread = session.zip(self.debug.focus.thread).and_then(|(session, id)| session.thread(id));
        let multiple_frames = thread.is_some_and(|thread| thread.frames.len() > 1 || thread.total_frames.is_some());
        let action = |command, title, icon, color, enabled| ToolbarAction { command, title, icon, color, enabled };
        let mut actions = Vec::new();
        match stopped {
            true => actions.push(action("workbench.action.debug.continue", "Continue", "debug-continue", "debugIcon.continueForeground", true)),
            false => actions.push(action("workbench.action.debug.pause", "Pause", "debug-pause", "debugIcon.pauseForeground", state == State::Running && !no_debug)),
        }
        actions.push(action("workbench.action.debug.stepOver", "Step Over", "debug-step-over", "debugIcon.stepOverForeground", stopped));
        actions.push(action("workbench.action.debug.stepInto", "Step Into", "debug-step-into", "debugIcon.stepIntoForeground", stopped));
        actions.push(action("workbench.action.debug.stepOut", "Step Out", "debug-step-out", "debugIcon.stepOutForeground", stopped && multiple_frames));
        if step_back {
            actions.push(action("workbench.action.debug.stepBack", "Step Back", "debug-step-back", "debugIcon.stepBackForeground", stopped));
            actions.push(action("workbench.action.debug.reverseContinue", "Reverse", "debug-reverse-continue", "debugIcon.reverseContinueForeground", stopped));
        }
        actions.push(action("workbench.action.debug.restart", "Restart", "debug-restart", "debugIcon.restartForeground", true));
        match attach {
            true => actions.push(action("workbench.action.debug.disconnect", "Disconnect", "debug-disconnect", "debugIcon.disconnectForeground", true)),
            false => actions.push(action("workbench.action.debug.stop", "Stop", "debug-stop", "debugIcon.stopForeground", true)),
        }
        actions
    }

    fn debug_toolbar_position(&self) -> (f32, f32) {
        if let Some(drag) = self.debug_view.toolbar_drag {
            return drag.current;
        }
        let storage = self.profile_storage.lock().unwrap();
        let x = storage.get(POSITION_KEY).and_then(|value| value.parse::<f32>().ok()).filter(|x| !x.is_nan()).unwrap_or(0.5);
        let y = storage.get(Y_KEY).and_then(|value| value.parse::<f32>().ok()).unwrap_or(0.);
        (x, y)
    }

    fn store_debug_toolbar_position(&mut self, x: f32, y: f32) {
        let mut storage = self.profile_storage.lock().unwrap();
        storage.set(POSITION_KEY, x.to_string());
        storage.set(Y_KEY, y.to_string());
    }

    /// The toolbar while a session runs and `debug.toolBarLocation` is `floating`.
    pub(super) fn debug_toolbar(&self, window: &Window, cx: &mut Context<Self>) -> Option<gpui::Stateful<Div>> {
        if self.debug.state() == State::Inactive || self.setting("debug.toolBarLocation").as_str().unwrap_or("floating") != "floating" {
            return None;
        }
        if !self.debug.sessions.is_empty() && self.debug.sessions.iter().all(|session| session.options["suppressDebugToolbar"] == true) {
            return None;
        }
        let actions = self.debug_toolbar_actions();
        let width = 2. + DRAG_AREA_WIDTH + actions.len() as f32 * ACTION_WIDTH;
        let window_width = f32::from(window.viewport_size().width);
        let (x, y) = self.debug_toolbar_position();
        // debugToolBar.ts `yRange`: from the top to the bottom of the title bar and the editor tabs.
        let y = y.clamp(0., 28. + 35.);
        let left = (x * window_width - width / 2.).clamp(0., (window_width - width).max(0.));
        let disabled = self.color("disabledForeground");
        let mut bar = div()
            .id("debug-toolbar")
            .absolute()
            .left(px(left))
            .top(px(y))
            .h(px(HEIGHT))
            .pl(px(2.))
            .flex()
            .items_center()
            .rounded(px(8.))
            .bg(self.color("debugToolBar.background"))
            .shadow(vec![gpui::BoxShadow { color: self.color("widget.shadow").into(), offset: gpui::point(px(0.), px(2.)), blur_radius: px(8.), spread_radius: px(0.), inset: false }])
            .when_some(self.theme.color("contrastBorder"), |bar, border| bar.border_1().border_color(border))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                self.icon("gripper", DRAG_AREA_WIDTH, 16., self.foreground_or("icon.foreground"))
                    .id("debug-toolbar-drag-area")
                    .opacity(0.5)
                    .cursor_grab()
                    .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        // Double click on the gripper centers the toolbar again (#8250).
                        if event.click_count == 2 {
                            workbench.store_debug_toolbar_position(0.5, 0.);
                            cx.notify();
                            return;
                        }
                        let (x, y) = workbench.debug_toolbar_position();
                        workbench.debug_view.toolbar_drag = Some(ToolbarDrag { origin: (f32::from(event.position.x), f32::from(event.position.y)), start: (x, y), current: (x, y) });
                    })),
            );
        for action in actions {
            let color = if action.enabled { self.color(action.color) } else { disabled };
            let command = action.command;
            bar = bar.child(
                self.icon(action.icon, 22., 16., color)
                    .id(command)
                    .mr(px(4.))
                    .rounded(px(5.))
                    .when(action.enabled, |item| {
                        item.cursor_pointer().hover(|item| item.bg(self.color("toolbar.hoverBackground"))).on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                            workbench.execute_command(command, Vec::new(), window, cx);
                        }))
                    })
                    .child(self.action_hover(action.title, command, HoverOptions::mouse())),
            );
        }
        Some(bar)
    }

    /// statusbarColorProvider.ts `isStatusbarInDebugMode` with `debug.enableStatusBarColor`.
    pub(super) fn statusbar_in_debug_mode(&self) -> bool {
        let state = self.debug.state();
        if self.setting("debug.enableStatusBarColor") == false || state == State::Inactive || state == State::Initializing {
            return false;
        }
        !self.debug.sessions.iter().all(|session| session.options["suppressDebugStatusbar"] == true || session.configuration["noDebug"] == true)
    }

    /// debugStatus.ts: the `status.debug` entry with the selected configuration, by `debug.showInStatusBar`.
    pub(crate) fn update_debug_status(&mut self) {
        let shown = self.status_entries.iter().any(|entry| entry.entry_id == DEBUG_STATUS_ID);
        let show = match self.setting("debug.showInStatusBar").as_str() {
            Some("always") => true,
            Some("never") => false,
            _ => shown || self.debug.first_session_started,
        };
        if !show {
            self.status_entries.retain(|entry| entry.entry_id != DEBUG_STATUS_ID);
            return;
        }
        let (folder, name) = self.selected_launch();
        let mut text = name.unwrap_or_default();
        if let Some(folder) = folder.filter(|_| !text.is_empty() && self.workspace_folders().len() > 1) {
            text = format!("{text} ({})", folder.file_name().unwrap_or_default().to_string_lossy());
        }
        let entry = crate::main_thread_status_bar::StatusEntry {
            entry_id: DEBUG_STATUS_ID.into(),
            id: DEBUG_STATUS_ID.into(),
            name: "Debug".into(),
            text: format!("$(debug-alt-small) {text}"),
            tooltip: Some("Select and Start Debug Configuration".into()),
            command: Some(serde_json::json!("workbench.action.debug.selectandstart")),
            color: None,
            background_color: None,
            kind: None,
            align_left: true,
            priority: 30.,
            secondary: 0,
            show_progress: false,
        };
        crate::main_thread_status_bar::set_or_update_entry(&mut self.status_entries, entry);
    }

    /// The pointer moves over the window while the gripper is held.
    pub(super) fn drag_debug_toolbar(&mut self, event: &MouseMoveEvent, window: &Window, cx: &mut Context<Self>) {
        if self.debug_view.toolbar_drag.is_none() {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.end_debug_toolbar_drag();
            return;
        }
        let window_width = f32::from(window.viewport_size().width);
        let drag = self.debug_view.toolbar_drag.as_mut().expect("checked above");
        let x = drag.start.0 + (f32::from(event.position.x) - drag.origin.0) / window_width;
        let y = drag.start.1 + f32::from(event.position.y) - drag.origin.1;
        drag.current = (x.clamp(0., 1.), y.clamp(0., 28. + 35.));
        cx.notify();
    }

    pub(super) fn end_debug_toolbar_drag(&mut self) {
        let Some(drag) = self.debug_view.toolbar_drag.take() else { return };
        self.store_debug_toolbar_position(drag.current.0, drag.current.1);
    }
}
