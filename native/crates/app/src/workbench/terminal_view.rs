//! The Terminal view (terminalView.ts `TerminalViewPane` with terminalTabbedView.ts): the groups of terminals,
//! side by side when split, with the tabs list at their right once there are two terminals.
// shortcut: the view is an editor tab of the group, not a panel view; the panel part is milestone 11's.

mod grid;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::Side;
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{Bounds, ClipboardItem, Context, Div, FocusHandle, FontWeight, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollDelta, ScrollWheelEvent, SharedString, Window, canvas, div, prelude::*, px};
use serde_json::{Value, json};

use self::grid::{CellMetrics, GUTTER, Look, SCROLLBAR_WIDTH, SCROLLBAR_WIDTH_MODERN};
use super::Workbench;
use crate::commands::Outcome;
use crate::editor::TextDocument;
use crate::keybindings::Resolution;
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Entry, Event, Item, Kind, QuickInput};
use crate::terminal::colors::TerminalColors;
use crate::terminal::keys::{self, KeyResult};
use crate::terminal::mouse::{self, MouseAction};
use crate::terminal::profiles::{self, Profile};
use crate::terminal::pty::PtyEvent;
use crate::terminal::skip_shell::DEFAULT_COMMANDS_TO_SKIP_SHELL;
use crate::terminal::{ShellLaunchConfig, TerminalInstance, create_environment};

/// terminal.ts `TERMINAL_VIEW_ID`.
pub const TERMINAL_VIEW_ID: &str = "terminal";
/// The scheme of the editor tab that hosts the view (`Schemas.vscodeTerminal`).
pub const TERMINAL_SCHEME: &str = "vscode-terminal";

/// terminalInstance.ts `_setupTitlePolling`: node-pty's process title is read every 200 ms on macOS.
const TITLE_POLL_INTERVAL: Duration = Duration::from_millis(200);
/// terminalTabsList.ts `TerminalTabsListSizes`.
const TAB_HEIGHT: f32 = 22.;
const TABS_DEFAULT_WIDTH: f32 = 120.;
const TABS_ACTIONBAR_MINIMUM_WIDTH: f32 = 105.;
/// monaco's `ScrollableElement` wheel sensitivity: px per wheel unit.
const SCROLL_WHEEL_SENSITIVITY: f32 = 50.;

pub struct TerminalView {
    pub(super) focus: FocusHandle,
    instances: Vec<TerminalInstance>,
    /// terminalGroupService.ts groups: each holds instance ids, laid out side by side.
    groups: Vec<Group>,
    active_group: usize,
    next_id: u32,
    metrics: Option<CellMetrics>,
    /// Where each instance's pane painted, for its mouse events.
    pane_bounds: HashMap<u32, Bounds<Pixels>>,
    hovered: Option<u32>,
    /// The instance a left button drag selects in.
    selecting: Option<u32>,
    /// The wheel's fraction of a line not scrolled yet.
    scroll_remainder: f32,
    title_polling: bool,
    /// Set once a terminal was created: `terminalHasBeenCreated`.
    has_been_created: bool,
}

struct Group {
    instances: Vec<u32>,
    active: usize,
}

/// What a terminal quick input does when it is accepted.
pub enum TerminalInput {
    /// `TerminalProfileQuickpick`: create a terminal with the picked profile, or make it the default.
    Profiles { profiles: Vec<Profile>, set_default: bool },
    /// `renameWithQuickPick` for an instance id.
    Rename(u32),
}

impl TerminalView {
    pub fn new(cx: &mut gpui::App) -> Self {
        TerminalView {
            focus: cx.focus_handle(),
            instances: Vec::new(),
            groups: Vec::new(),
            active_group: 0,
            next_id: 1,
            metrics: None,
            pane_bounds: HashMap::new(),
            hovered: None,
            selecting: None,
            scroll_remainder: 0.,
            title_polling: false,
            has_been_created: false,
        }
    }

    fn active_instance_id(&self) -> Option<u32> {
        let group = self.groups.get(self.active_group)?;
        group.instances.get(group.active).copied()
    }

    fn instance(&self, id: u32) -> Option<&TerminalInstance> {
        self.instances.iter().find(|instance| instance.id == id)
    }

    fn instance_mut(&mut self, id: u32) -> Option<&mut TerminalInstance> {
        self.instances.iter_mut().find(|instance| instance.id == id)
    }

    fn active_instance_mut(&mut self) -> Option<&mut TerminalInstance> {
        let id = self.active_instance_id()?;
        self.instance_mut(id)
    }

    /// `setActiveInstance`: the group holding `id` and its pane become active.
    fn set_active(&mut self, id: u32) {
        for (group_index, group) in self.groups.iter_mut().enumerate() {
            if let Some(index) = group.instances.iter().position(|&instance| instance == id) {
                group.active = index;
                self.active_group = group_index;
            }
        }
    }

    /// terminalTabbedView.ts `_shouldShowTabs`.
    fn should_show_tabs(&self, enabled: bool, hide_condition: &str) -> bool {
        enabled
            && match hide_condition {
                "never" => true,
                "singleTerminal" => self.instances.len() > 1,
                "singleGroup" => self.groups.len() > 1,
                _ => false,
            }
    }
}

impl Workbench {
    pub(super) fn register_terminal_actions(&mut self) {
        self.commands.register("workbench.action.terminal.toggleTerminal", |workbench, _, window, cx| {
            if workbench.terminal_view_visible() && workbench.terminal_view.focus.is_focused(window) {
                workbench.close_terminal_view(cx);
            } else {
                workbench.open_terminal_view(window, cx);
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.focus", |workbench, _, window, cx| {
            workbench.open_terminal_view(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.new", |workbench, _, window, cx| {
            workbench.create_terminal(None, false, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.split", |workbench, _, window, cx| {
            workbench.create_terminal(None, true, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.newWithProfile", |workbench, _, window, cx| {
            workbench.show_terminal_profiles(false, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.selectDefaultShell", |workbench, _, window, cx| {
            workbench.show_terminal_profiles(true, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.kill", |workbench, _, window, cx| {
            if let Some(id) = workbench.terminal_view.active_instance_id() {
                workbench.kill_terminal(id, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.killAll", |workbench, _, window, cx| {
            let ids: Vec<u32> = workbench.terminal_view.instances.iter().map(|instance| instance.id).collect();
            for id in ids {
                workbench.kill_terminal(id, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.rename", |workbench, _, window, cx| {
            if let Some(id) = workbench.terminal_view.active_instance_id() {
                workbench.show_terminal_rename(id, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.clear", |workbench, _, _, cx| {
            if let Some(instance) = workbench.terminal_view.active_instance_mut() {
                instance.clear();
            }
            cx.notify();
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.copySelection", |workbench, _, _, cx| {
            if let Some(text) = workbench.terminal_view.active_instance_mut().and_then(|instance| instance.selection_text()) {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.paste", |workbench, _, _, cx| {
            let text = cx.read_from_clipboard().and_then(|item| item.text());
            if let (Some(text), Some(instance)) = (text, workbench.terminal_view.active_instance_mut()) {
                instance.paste(&text);
            }
            cx.notify();
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.selectAll", |workbench, _, _, cx| {
            if let Some(instance) = workbench.terminal_view.active_instance_mut() {
                instance.select_all();
            }
            cx.notify();
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.clearSelection", |workbench, _, _, cx| {
            if let Some(instance) = workbench.terminal_view.active_instance_mut() {
                instance.term.selection = None;
            }
            cx.notify();
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.sendSequence", |workbench, args, _, cx| {
            let text = args.first().and_then(|args| args["text"].as_str()).map(str::to_owned);
            if let (Some(text), Some(instance)) = (text, workbench.terminal_view.active_instance_mut()) {
                instance.input(&text);
            }
            cx.notify();
            Outcome::done()
        });
        for (command, scroll) in [
            ("workbench.action.terminal.scrollUp", Scroll::Delta(1)),
            ("workbench.action.terminal.scrollDown", Scroll::Delta(-1)),
            ("workbench.action.terminal.scrollUpPage", Scroll::PageUp),
            ("workbench.action.terminal.scrollDownPage", Scroll::PageDown),
            ("workbench.action.terminal.scrollToTop", Scroll::Top),
            ("workbench.action.terminal.scrollToBottom", Scroll::Bottom),
        ] {
            self.commands.register(command, move |workbench, _, _, cx| {
                if let Some(instance) = workbench.terminal_view.active_instance_mut() {
                    instance.scroll(scroll);
                }
                cx.notify();
                Outcome::done()
            });
        }
        for (command, step) in [("workbench.action.terminal.focusNext", 1), ("workbench.action.terminal.focusPrevious", -1)] {
            self.commands.register(command, move |workbench, _, window, cx| {
                let view = &mut workbench.terminal_view;
                if !view.groups.is_empty() {
                    view.active_group = (view.active_group as isize + step).rem_euclid(view.groups.len() as isize) as usize;
                }
                workbench.open_terminal_view(window, cx);
                Outcome::done()
            });
        }
        for (command, step) in [("workbench.action.terminal.focusNextPane", 1), ("workbench.action.terminal.focusPreviousPane", -1)] {
            self.commands.register(command, move |workbench, _, window, cx| {
                if let Some(group) = workbench.terminal_view.groups.get_mut(workbench.terminal_view.active_group) {
                    group.active = (group.active as isize + step).rem_euclid(group.instances.len() as isize) as usize;
                }
                workbench.open_terminal_view(window, cx);
                Outcome::done()
            });
        }
    }

    /// `isViewVisible(TERMINAL_VIEW_ID)`: the group shows the Terminal view's tab.
    pub(crate) fn terminal_view_visible(&self) -> bool {
        self.editors.active().is_some_and(|editor| editor.document().scheme == TERMINAL_SCHEME)
    }

    /// `terminalGroupService.showPanel(true)`: shows the view, creating a terminal when there is none.
    fn open_terminal_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_view.instances.is_empty() {
            self.create_terminal(None, false, window, cx);
            return;
        }
        self.show_terminal_tab(window, cx);
    }

    fn show_terminal_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.editors.editors.iter().position(|editor| editor.document().scheme == TERMINAL_SCHEME) {
            Some(index) => self.editors.activate(index),
            None => {
                let document = TextDocument::from_text(TERMINAL_SCHEME, Path::new(TERMINAL_VIEW_ID), "", "utf8".into(), None, &self.languages, &self.settings.lock().unwrap());
                self.editors.open(document, false);
            }
        }
        self.editors_changed(cx);
        window.focus(&self.terminal_view.focus, cx);
    }

    /// `viewsService.closeView(TERMINAL_VIEW_ID)`: the terminals keep running.
    fn close_terminal_view(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.editors.editors.iter().position(|editor| editor.document().scheme == TERMINAL_SCHEME) else { return };
        self.editors.close(index);
        self.editors_changed(cx);
    }

    /// `createTerminal`: a new group, or a pane after the active one with `split`; the split's cwd is its parent's
    /// (`terminal.integrated.splitCwd` inherited).
    fn create_terminal(&mut self, profile: Option<Profile>, split: bool, window: &mut Window, cx: &mut Context<Self>) {
        let profile = profile.unwrap_or_else(|| self.default_terminal_profile());
        let parent_cwd = match split {
            true if self.setting("terminal.integrated.splitCwd") == "inherited" => self.terminal_view.active_instance_id().and_then(|id| self.terminal_view.instance(id)).and_then(|parent| parent.cwd.clone()),
            _ => None,
        };
        let cwd = parent_cwd.map(PathBuf::from).unwrap_or_else(|| self.terminal_initial_cwd());
        let launch = self.shell_launch_config(&profile, cwd);
        let view = &mut self.terminal_view;
        let id = view.next_id;
        view.next_id += 1;
        let (cols, rows) = view.active_instance_id().and_then(|id| view.instance(id)).map_or((80, 30), |instance| (instance.term.columns() as u16, instance.term.screen_lines() as u16));
        let (mut instance, receiver) = TerminalInstance::new(id, launch, cols, rows, self.terminal_config());
        if let Some(message) = instance.spawn_error.clone() {
            let colors = TerminalColors::new(&self.theme);
            instance.write_message(&message, &colors);
        }
        let view = &mut self.terminal_view;
        view.instances.push(instance);
        view.has_been_created = true;
        match view.groups.get_mut(view.active_group).filter(|_| split) {
            Some(group) => {
                group.active += 1;
                group.instances.insert(group.active, id);
            }
            None => {
                view.groups.push(Group { instances: vec![id], active: 0 });
                view.active_group = view.groups.len() - 1;
            }
        }
        if let Some(receiver) = receiver {
            self.pump_terminal_events(id, receiver, window, cx);
        }
        self.start_title_poll(cx);
        self.show_terminal_tab(window, cx);
    }

    /// `getDefaultProfile` over the detected and configured profiles.
    fn default_terminal_profile(&self) -> Profile {
        let profiles = self.terminal_profiles();
        let default_name = self.setting("terminal.integrated.defaultProfile.osx");
        let env: HashMap<String, String> = std::env::vars().collect();
        profiles::default_profile(&profiles, default_name.as_str(), &profiles::system_shell(&env))
    }

    fn terminal_profiles(&self) -> Vec<Profile> {
        let shells = std::fs::read_to_string("/etc/shells").unwrap_or_default();
        profiles::detect(&self.setting("terminal.integrated.profiles.osx"), &shells, std::env::var("PATH").ok().as_deref())
    }

    /// `getCwd`: `terminal.integrated.cwd` against the workspace folder, else the folder, else the home directory.
    fn terminal_initial_cwd(&self) -> PathBuf {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
        let root = self.workspace().and_then(|workspace| workspace.folders.first()).map(|folder| folder.path.clone());
        let configured = self.setting("terminal.integrated.cwd").as_str().filter(|cwd| !cwd.is_empty()).map(PathBuf::from);
        match (configured, root) {
            (Some(cwd), Some(root)) if cwd.is_relative() => root.join(cwd),
            (Some(cwd), _) => cwd,
            (None, Some(root)) => root,
            (None, None) => home,
        }
    }

    fn shell_launch_config(&self, profile: &Profile, cwd: PathBuf) -> ShellLaunchConfig {
        let configured_env: HashMap<String, Option<String>> = match self.setting("terminal.integrated.env.osx") {
            Value::Object(map) => map.into_iter().map(|(key, value)| (key, value.as_str().map(str::to_owned))).collect(),
            _ => HashMap::new(),
        };
        let detect_locale = self.setting("terminal.integrated.detectLocale");
        let env = create_environment(std::env::vars(), &[configured_env, profile.env.clone()], crate::VERSION, "en", detect_locale.as_str().unwrap_or("auto"));
        ShellLaunchConfig {
            name: profile.override_name.then(|| profile.name.clone()),
            executable: profile.path.clone(),
            args: profile.args.clone(),
            cwd,
            env,
            icon: profile.icon.clone(),
            color: profile.color.clone(),
        }
    }

    /// The PTY's output reaches the instance in the order it was read; the exit follows the last read.
    fn pump_terminal_events(&mut self, id: u32, mut receiver: UnboundedReceiver<PtyEvent>, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |workbench, cx| {
            while let Some(event) = receiver.next().await {
                let mut events = vec![event];
                while let Ok(event) = receiver.try_recv() {
                    events.push(event);
                }
                let handled = workbench.update_in(cx, |workbench, window, cx| workbench.terminal_events(id, events, window, cx));
                if !matches!(handled, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Returns false once the instance is gone.
    fn terminal_events(&mut self, id: u32, events: Vec<PtyEvent>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let colors = TerminalColors::new(&self.theme);
        let Some(instance) = self.terminal_view.instance_mut(id) else { return false };
        for event in events {
            match event {
                PtyEvent::Data(bytes) => instance.process(&bytes, &colors),
                PtyEvent::Exit(code) => {
                    instance.exited(code);
                    self.terminal_process_exited(id, code, window, cx);
                    return false;
                }
            }
        }
        // xtermTerminal.ts loads ClipboardAddon: OSC 52 writes go to the clipboard.
        // shortcut: the OSC 52 `?` query that reads the clipboard back to the process is not answered.
        if let Some(text) = instance.clipboard_requests.drain(..).last() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        instance.bell = false;
        cx.notify();
        true
    }

    /// terminalInstance.ts `_onProcessExit`: a non-zero exit shows its message, then the instance goes.
    fn terminal_process_exited(&mut self, id: u32, code: Option<i32>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(instance) = self.terminal_view.instance(id) else { return };
        // node-pty reports exit code 0 for a process a signal ended.
        let message = instance.exit_message(code.unwrap_or(0));
        let failed_during_launch = instance.failed_during_launch();
        if let Some(message) = message {
            // shortcut: the Ctrl+D exception (`lastInputEvent !== '\x04'`) is not tracked; xterm's last input is not kept.
            if failed_during_launch || self.setting("terminal.integrated.showExitAlert") == true {
                self.notify_error(message);
            } else {
                eprintln!("[terminal] {message}");
            }
        }
        self.dispose_terminal(id, window, cx);
    }

    /// `killInstance`: the process gets SIGHUP and the instance goes without an exit message.
    fn kill_terminal(&mut self, id: u32, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(instance) = self.terminal_view.instance_mut(id) {
            instance.kill();
        }
        self.dispose_terminal(id, window, cx);
    }

    /// terminalGroup.ts `_onInstanceDisposed`: the next pane of the group becomes active; an empty group goes, and
    /// with it the view when it was the last one and `terminal.integrated.hideOnLastClosed` is on.
    fn dispose_terminal(&mut self, id: u32, window: &mut Window, cx: &mut Context<Self>) {
        let view = &mut self.terminal_view;
        let had_focus = view.focus.is_focused(window);
        view.instances.retain(|instance| instance.id != id);
        view.pane_bounds.remove(&id);
        for group in &mut view.groups {
            if let Some(index) = group.instances.iter().position(|&instance| instance == id) {
                group.instances.remove(index);
                group.active = group.active.min(group.instances.len().saturating_sub(1));
            }
        }
        if let Some(empty) = view.groups.iter().position(|group| group.instances.is_empty()) {
            view.groups.remove(empty);
            if view.active_group >= empty && view.active_group > 0 {
                view.active_group -= 1;
            }
        }
        if view.instances.is_empty() {
            if self.setting("terminal.integrated.hideOnLastClosed") == true {
                self.close_terminal_view(cx);
            }
        } else if had_focus {
            window.focus(&self.terminal_view.focus, cx);
        }
        cx.notify();
    }

    fn start_title_poll(&mut self, cx: &mut Context<Self>) {
        if self.terminal_view.title_polling {
            return;
        }
        self.terminal_view.title_polling = true;
        cx.spawn(async move |workbench, cx| {
            loop {
                cx.background_executor().timer(TITLE_POLL_INTERVAL).await;
                let running = workbench.update(cx, |workbench, cx| {
                    let view = &mut workbench.terminal_view;
                    let mut changed = false;
                    for instance in &mut view.instances {
                        changed |= instance.refresh_process();
                    }
                    if changed {
                        cx.notify();
                    }
                    view.title_polling = !view.instances.is_empty();
                    view.title_polling
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// `TerminalProfileQuickpick.showAndGetResult`: configured profiles, then the detected ones, each list with the
    /// default first.
    // shortcut: the "Configure Terminal Profile" item button and contributed profiles are not ported.
    fn show_terminal_profiles(&mut self, set_default: bool, window: &mut Window, cx: &mut Context<Self>) {
        let default_name = self.default_terminal_profile().name;
        let (detected, configured): (Vec<Profile>, Vec<Profile>) = self.terminal_profiles().into_iter().partition(|profile| profile.auto_detected);
        let sort = |mut profiles: Vec<Profile>| {
            profiles.sort_by(|a, b| (b.name == default_name).cmp(&(a.name == default_name)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
            profiles
        };
        let mut entries = Vec::new();
        let mut picks = Vec::new();
        for (separator, profiles) in [("profiles", sort(configured)), ("detected", sort(detected))] {
            if profiles.is_empty() {
                continue;
            }
            entries.push(Entry::Separator(Some(separator.into())));
            for profile in profiles {
                entries.push(Entry::Item(Item { handle: picks.len() as i64, label: format!("$({}) {}", profile.icon.as_deref().unwrap_or("terminal"), profile.name), description: Some(profile_description(&profile)), ..Item::default() }));
                picks.push(profile);
            }
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some(if set_default { "Select your default terminal profile" } else { "Select the terminal profile to create" }.into());
        input.set_items(entries);
        let id = self.quick_inputs.add(input, Owner::Terminal(TerminalInput::Profiles { profiles: picks, set_default }));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    fn show_terminal_rename(&mut self, id: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(instance) = self.terminal_view.instance(id) else { return };
        let mut input = QuickInput::new(0, Kind::InputBox);
        input.prompt = Some("Enter terminal name".into());
        input.set_value(instance.title());
        let id = self.quick_inputs.add(input, Owner::Terminal(TerminalInput::Rename(id)));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub fn terminal_input_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Accept(_) => {
                let Some(input) = self.quick_inputs.get(id) else { return };
                let (value, selected) = (input.input_box.value.clone(), input.selected_handles().first().copied());
                let Some(Owner::Terminal(owner)) = self.quick_inputs.owner(id) else { return };
                let accepted = match owner {
                    TerminalInput::Profiles { profiles, set_default } => selected.map(|handle| (Some((profiles[handle as usize].clone(), *set_default)), None)),
                    TerminalInput::Rename(instance) => Some((None, Some(*instance))),
                };
                self.hide_quick_input_by_id(id, window, cx);
                match accepted {
                    Some((Some((profile, true)), _)) => self.update_setting("terminal.integrated.defaultProfile.osx", json!(profile.name)),
                    Some((Some((profile, false)), _)) => self.create_terminal(Some(profile), false, window, cx),
                    // `if (title) instance.rename(title)`: an empty name keeps the title.
                    Some((None, Some(instance))) if !value.is_empty() => {
                        if let Some(instance) = self.terminal_view.instance_mut(instance) {
                            instance.user_title = Some(value);
                        }
                        cx.notify();
                    }
                    _ => {}
                }
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }

    /// terminalContextKey.ts keys.
    pub(super) fn terminal_context(&self, window: &Window, context: &mut HashMap<String, Value>) {
        let view = &self.terminal_view;
        let focused = view.focus.is_focused(window);
        let selected = view.active_instance_id().and_then(|id| view.instance(id)).is_some_and(|instance| instance.term.selection.as_ref().is_some_and(|selection| !selection.is_empty()));
        context.insert("terminalFocus".into(), json!(focused));
        context.insert("terminalFocusInAny".into(), json!(focused));
        context.insert("terminalHasBeenCreated".into(), json!(view.has_been_created));
        context.insert("terminalIsOpen".into(), json!(!view.instances.is_empty()));
        context.insert("terminalCount".into(), json!(view.instances.len()));
        context.insert("terminalGroupCount".into(), json!(view.groups.len()));
        context.insert("terminalProcessSupported".into(), json!(true));
        context.insert("terminalTextSelected".into(), json!(selected));
        context.insert("terminalTextSelectedInFocused".into(), json!(focused && selected));
        context.insert("terminalSplitPaneActive".into(), json!(focused && view.groups.get(view.active_group).is_some_and(|group| group.instances.len() > 1)));
        let alt_buffer = view.active_instance_id().and_then(|id| view.instance(id)).is_some_and(|instance| instance.term.mode().contains(TermMode::ALT_SCREEN));
        context.insert("terminalAltBufferActive".into(), json!(alt_buffer));
        context.insert("terminal.active".into(), json!(true));
        if self.terminal_view_visible() {
            context.insert("view".into(), json!(TERMINAL_VIEW_ID));
        }
    }

    /// terminalInstance.ts `attachCustomKeyEventHandler` and xterm.js `_keyDown`: a chord in progress and the
    /// commands to skip the shell go to the workbench, every other key the terminal turns into input. Returns
    /// whether the terminal took the key.
    pub(super) fn terminal_key(&mut self, keystroke: &Keystroke, resolution: &Resolution, window: &Window, cx: &mut Context<Self>) -> bool {
        if !self.terminal_view.focus.is_focused(window) {
            return false;
        }
        match resolution {
            Resolution::MoreChordsNeeded if self.setting("terminal.integrated.allowChords") == true && !keystroke.modifiers.alt => return false,
            Resolution::KbFound { command, .. } if self.setting("terminal.integrated.sendKeybindingsToShell") != true && self.skips_shell(command) => return false,
            _ => {}
        }
        let mac_option_is_meta = self.setting("terminal.integrated.macOptionIsMeta") == true;
        let Some(instance) = self.terminal_view.active_instance_mut() else { return false };
        let application_cursor = instance.term.mode().contains(TermMode::APP_CURSOR);
        match keys::evaluate(keystroke, application_cursor, mac_option_is_meta) {
            KeyResult::Send(data) => instance.input(&data),
            KeyResult::PageUp => instance.scroll(Scroll::PageUp),
            KeyResult::PageDown => instance.scroll(Scroll::PageDown),
            KeyResult::SelectAll => instance.select_all(),
            KeyResult::None => return false,
        }
        cx.notify();
        true
    }

    /// `_skipTerminalCommands`: the defaults without the `-` entries of `terminal.integrated.commandsToSkipShell`,
    /// plus its other entries.
    fn skips_shell(&self, command: &str) -> bool {
        let configured = self.setting("terminal.integrated.commandsToSkipShell");
        let configured: Vec<&str> = configured.as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        if configured.contains(&command) {
            return true;
        }
        DEFAULT_COMMANDS_TO_SKIP_SHELL.contains(&command) && !configured.iter().any(|entry| entry.strip_prefix('-') == Some(command))
    }

    /// The view's title actions where a text editor has its breadcrumbs (terminalMenus.ts `MenuId.ViewTitle`):
    /// the active terminal while the tabs list is hidden, New with the profile dropdown, Split and Kill while the
    /// tabs list is hidden, and Clear.
    pub(super) fn terminal_title_actions(&self, cx: &mut Context<Self>) -> Div {
        let show_tabs = self.terminal_tabs_shown();
        let title_border = ["editorGroupHeader.border", "contrastBorder"].into_iter().find_map(|key| self.theme.color(key));
        let active_title = self.terminal_view.active_instance_id().and_then(|id| self.terminal_view.instance(id)).map(TerminalInstance::title);
        let foreground = self.foreground_or("icon.foreground");
        div()
            .relative()
            .h(px(35.))
            .flex_none()
            .flex()
            .items_center()
            .justify_end()
            .px(px(8.))
            .gap(px(4.))
            .bg(self.color("breadcrumb.background"))
            .when_some(active_title.filter(|_| !show_tabs), |actions, title| {
                actions.child(
                    div()
                        .id("terminal-single-tab")
                        .h(px(22.))
                        .px(px(4.))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .rounded(px(4.))
                        .text_size(px(11.))
                        .cursor_pointer()
                        .text_color(self.foreground_or("foreground"))
                        .hover(|tab| tab.bg(self.color("toolbar.hoverBackground")))
                        .child(self.icon("terminal", 16., 16., foreground))
                        .child(title)
                        .on_click(cx.listener(|workbench, _, window, cx| {
                            workbench.execute_command("workbench.action.terminal.focus", Vec::new(), window, cx);
                        })),
                )
            })
            .child(self.search_action("add", "workbench.action.terminal.new", true, cx))
            .child(self.search_action("chevron-down", "workbench.action.terminal.newWithProfile", true, cx))
            .when(!show_tabs, |actions| {
                actions.child(self.search_action("split-horizontal", "workbench.action.terminal.split", true, cx)).child(self.search_action("trash", "workbench.action.terminal.kill", true, cx))
            })
            .when_some(title_border, |actions, color| actions.child(div().absolute().bottom_0().left_0().right_0().h(px(1.)).bg(color)))
    }

    fn terminal_tabs_shown(&self) -> bool {
        let enabled = self.setting("terminal.integrated.tabs.enabled") == true;
        let hide = self.setting("terminal.integrated.tabs.hideCondition");
        self.terminal_view.should_show_tabs(enabled, hide.as_str().unwrap_or("singleTerminal"))
    }

    /// The view below its title: the active group's panes and, at `terminal.integrated.tabs.location`, the tabs list.
    pub(super) fn terminal_body(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let view = &self.terminal_view;
        let background = self.theme.color("terminal.background").unwrap_or(self.color("panel.background"));
        let border = self.color("terminal.border");
        let mut panes = div().flex_1().min_w_0().h_full().flex();
        if let Some(group) = view.groups.get(view.active_group) {
            for (index, &id) in group.instances.iter().enumerate() {
                panes = panes.child(self.terminal_pane(id, index > 0, border, cx));
            }
        }
        let tabs_left = self.setting("terminal.integrated.tabs.location") == "left";
        div()
            .id("terminal-view")
            .track_focus(&view.focus)
            .flex_1()
            .min_h_0()
            .flex()
            .bg(background)
            .when(tabs_left, |body| body.flex_row_reverse())
            .child(panes)
            .when(self.terminal_tabs_shown(), |body| body.child(self.terminal_tabs_list(tabs_left, cx)))
    }

    fn terminal_pane(&self, id: u32, border_left: bool, border: gpui::Rgba, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        div()
            .id(("terminal-pane", id as usize))
            .flex_1()
            .min_w_0()
            .h_full()
            .cursor_text()
            .when(border_left, |pane| pane.border_l_1().border_color(border))
            .child(canvas(|_, _, _| {}, move |bounds, _, window, cx| entity.update(cx, |workbench, _| workbench.paint_terminal_pane(id, bounds, window))).size_full())
            .on_hover(cx.listener(move |workbench, hovered: &bool, _, cx| {
                workbench.terminal_view.hovered = hovered.then_some(id);
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| workbench.terminal_mouse_down(id, event, window, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| workbench.terminal_mouse_down(id, event, window, cx)))
            .on_mouse_down(MouseButton::Middle, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| workbench.terminal_mouse_down(id, event, window, cx)))
            .on_mouse_move(cx.listener(move |workbench, event: &MouseMoveEvent, _, cx| workbench.terminal_mouse_move(id, event, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(move |workbench, event: &MouseUpEvent, _, cx| workbench.terminal_mouse_up(id, event, cx)))
            .on_scroll_wheel(cx.listener(move |workbench, event: &ScrollWheelEvent, _, cx| workbench.terminal_wheel(id, event, cx)))
    }

    /// Lays out and paints one pane: the instance takes the columns and rows its pane fits (`_evaluateColsAndRows`).
    fn paint_terminal_pane(&mut self, id: u32, bounds: Bounds<Pixels>, window: &mut Window) {
        let metrics = self.terminal_metrics(window);
        let scrollbar_width = if self.setting("workbench.experimental.modernUI") == true { SCROLLBAR_WIDTH_MODERN } else { SCROLLBAR_WIDTH };
        let colors = TerminalColors::new(&self.theme);
        let cursor_inactive_style = self.setting("terminal.integrated.cursorStyleInactive");
        let look = Look {
            colors: &colors,
            metrics: &metrics,
            focused: self.terminal_view.focus.is_focused(window) && self.terminal_view.active_instance_id() == Some(id),
            cursor_inactive_style: cursor_inactive_style.as_str().unwrap_or("outline"),
            cursor_width: self.setting("terminal.integrated.cursorWidth").as_f64().unwrap_or(1.) as f32,
            minimum_contrast_ratio: self.setting("terminal.integrated.minimumContrastRatio").as_f64().unwrap_or(4.5),
            draw_bold_text_in_bright_colors: self.setting("terminal.integrated.drawBoldTextInBrightColors") == true,
            scrollbar_width,
            hovered: self.terminal_view.hovered == Some(id),
            scrollbar_slider: self.color("scrollbarSlider.background"),
        };
        let (cols, rows) = metrics.grid_size(f32::from(bounds.size.width) - GUTTER - scrollbar_width, f32::from(bounds.size.height));
        let config = self.terminal_config();
        self.terminal_view.pane_bounds.insert(id, bounds);
        let Some(instance) = self.terminal_view.instance_mut(id) else { return };
        instance.cell_size = (metrics.cell_width, metrics.cell_height);
        instance.resize(cols, rows);
        instance.configure(config);
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| grid::paint(bounds, instance, &look, window));
    }

    /// The xterm.js options the settings set: `scrollback`, `wordSeparator` and `cursorStyle`, which a program's
    /// DECSCUSR overrides.
    // shortcut: `terminal.integrated.cursorBlinking` is not applied, the cursor does not blink.
    fn terminal_config(&self) -> Config {
        let shape = match self.setting("terminal.integrated.cursorStyle").as_str() {
            Some("line") => CursorShape::Beam,
            Some("underline") => CursorShape::Underline,
            _ => CursorShape::Block,
        };
        Config {
            scrolling_history: self.setting("terminal.integrated.scrollback").as_u64().unwrap_or(1000) as usize,
            semantic_escape_chars: self.setting("terminal.integrated.wordSeparators").as_str().unwrap_or_default().to_owned(),
            default_cursor_style: CursorStyle { shape, blinking: false },
            ..Config::default()
        }
    }

    /// `TerminalConfigurationService.getFont`: `terminal.integrated.fontFamily`, else the editor's font.
    fn terminal_metrics(&mut self, window: &Window) -> CellMetrics {
        let family = self.setting("terminal.integrated.fontFamily");
        let family = family.as_str().and_then(|family| family.split(',').next()).map(|family| family.trim().trim_matches(['\'', '"']).to_owned()).filter(|family| !family.is_empty()).unwrap_or_else(|| "Menlo".into());
        let font_size = self.setting("terminal.integrated.fontSize").as_f64().unwrap_or(12.) as f32;
        let line_height = self.setting("terminal.integrated.lineHeight").as_f64().unwrap_or(1.) as f32;
        let letter_spacing = self.setting("terminal.integrated.letterSpacing").as_f64().unwrap_or(0.) as f32;
        let weight = |key: &str, fallback: FontWeight| match self.setting(key) {
            Value::String(name) if name == "normal" => FontWeight::NORMAL,
            Value::String(name) if name == "bold" => FontWeight::BOLD,
            Value::String(number) => number.parse().map(FontWeight).unwrap_or(fallback),
            Value::Number(number) => number.as_f64().map_or(fallback, |weight| FontWeight(weight as f32)),
            _ => fallback,
        };
        let (normal, bold) = (weight("terminal.integrated.fontWeight", FontWeight::NORMAL), weight("terminal.integrated.fontWeightBold", FontWeight::BOLD));
        let family = SharedString::from(family);
        let current = self.terminal_view.metrics.as_ref();
        let stale = current.is_none_or(|metrics| metrics.family != family || metrics.font_size != font_size || metrics.line_height != line_height || metrics.letter_spacing != letter_spacing || metrics.normal_weight != normal || metrics.bold_weight != bold || metrics.scale != window.scale_factor());
        if stale {
            self.terminal_view.metrics = Some(CellMetrics::measure(window, family, font_size, line_height, letter_spacing, normal, bold));
        }
        self.terminal_view.metrics.clone().expect("measured above")
    }

    fn terminal_mouse_down(&mut self, id: u32, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal_view.set_active(id);
        window.focus(&self.terminal_view.focus, cx);
        cx.stop_propagation();
        let force_selection = event.modifiers.alt && self.setting("terminal.integrated.macOptionClickForcesSelection") == true;
        let (Some(bounds), Some(metrics)) = (self.terminal_view.pane_bounds.get(&id).copied(), self.terminal_view.metrics.clone()) else { return };
        let Some(instance) = self.terminal_view.instance_mut(id) else { return };
        let (row, column, side) = grid::cell_at(bounds, event.position, instance, &metrics);
        let mode = *instance.term.mode();
        let button = match event.button {
            MouseButton::Left => mouse::MouseButton::Left,
            MouseButton::Middle => mouse::MouseButton::Middle,
            _ => mouse::MouseButton::Right,
        };
        if !force_selection && mouse::wants(mode, button, MouseAction::Down) {
            if let Some(report) = mouse::report(mode, button, MouseAction::Down, column, row.max(0) as usize, event.modifiers.shift, event.modifiers.alt, event.modifiers.control) {
                instance.input(&report);
            }
            cx.notify();
            return;
        }
        // shortcut: the right click selects the word (`rightClickBehavior` selectWord) but shows no context menu; the terminal context menu is not ported.
        let kind = match (event.button, event.click_count) {
            (MouseButton::Right, _) | (MouseButton::Left, 2) => SelectionType::Semantic,
            (MouseButton::Left, 3..) => SelectionType::Lines,
            (MouseButton::Left, _) => SelectionType::Simple,
            _ => return,
        };
        if event.button == MouseButton::Left && event.click_count == 1 && event.modifiers.shift && instance.term.selection.is_some() {
            instance.extend_selection(row, column, side);
        } else {
            instance.start_selection(kind, row, column, side);
        }
        if event.button == MouseButton::Left {
            self.terminal_view.selecting = Some(id);
        }
        cx.notify();
    }

    fn terminal_mouse_move(&mut self, id: u32, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let (Some(bounds), Some(metrics)) = (self.terminal_view.pane_bounds.get(&id).copied(), self.terminal_view.metrics.clone()) else { return };
        let selecting = self.terminal_view.selecting == Some(id) && event.pressed_button == Some(MouseButton::Left);
        let Some(instance) = self.terminal_view.instance_mut(id) else { return };
        let (row, column, side) = grid::cell_at(bounds, event.position, instance, &metrics);
        if selecting {
            instance.extend_selection(row, column, side);
            return cx.notify();
        }
        let mode = *instance.term.mode();
        let button = match event.pressed_button {
            Some(MouseButton::Left) => mouse::MouseButton::Left,
            Some(MouseButton::Middle) => mouse::MouseButton::Middle,
            Some(MouseButton::Right) => mouse::MouseButton::Right,
            _ => mouse::MouseButton::None,
        };
        if mouse::wants(mode, button, MouseAction::Move) && (0..instance.term.screen_lines() as i32).contains(&row) {
            if let Some(report) = mouse::report(mode, button, MouseAction::Move, column, row as usize, event.modifiers.shift, event.modifiers.alt, event.modifiers.control) {
                instance.input(&report);
            }
        }
    }

    fn terminal_mouse_up(&mut self, id: u32, event: &MouseUpEvent, cx: &mut Context<Self>) {
        let was_selecting = self.terminal_view.selecting.take() == Some(id);
        let copy_on_selection = self.setting("terminal.integrated.copyOnSelection") == true;
        let (Some(bounds), Some(metrics)) = (self.terminal_view.pane_bounds.get(&id).copied(), self.terminal_view.metrics.clone()) else { return };
        let Some(instance) = self.terminal_view.instance_mut(id) else { return };
        if was_selecting {
            if copy_on_selection && let Some(text) = instance.selection_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            return;
        }
        let mode = *instance.term.mode();
        let (row, column, _) = grid::cell_at(bounds, event.position, instance, &metrics);
        if mouse::wants(mode, mouse::MouseButton::Left, MouseAction::Up) && let Some(report) = mouse::report(mode, mouse::MouseButton::Left, MouseAction::Up, column, row.max(0) as usize, event.modifiers.shift, event.modifiers.alt, event.modifiers.control) {
            instance.input(&report);
        }
    }

    /// xterm.js `Viewport` wheel handling: a program that asked for the mouse gets wheel reports; in the alternate
    /// buffer the wheel sends arrow keys; otherwise the scrollback scrolls by `mouseWheelScrollSensitivity` lines,
    /// `fastScrollSensitivity` with Alt.
    fn terminal_wheel(&mut self, id: u32, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let Some(metrics) = self.terminal_view.metrics.clone() else { return };
        let units = match event.delta {
            ScrollDelta::Pixels(delta) => f32::from(delta.y) / 40.,
            ScrollDelta::Lines(delta) => delta.y,
        };
        let key = if event.modifiers.alt { "terminal.integrated.fastScrollSensitivity" } else { "terminal.integrated.mouseWheelScrollSensitivity" };
        let sensitivity = self.setting(key).as_f64().unwrap_or(1.) as f32;
        let lines = self.terminal_view.scroll_remainder + units * SCROLL_WHEEL_SENSITIVITY * sensitivity / metrics.cell_height;
        let whole = lines.trunc();
        self.terminal_view.scroll_remainder = lines - whole;
        if whole == 0. {
            return;
        }
        let bounds = self.terminal_view.pane_bounds.get(&id).copied();
        let Some(instance) = self.terminal_view.instance_mut(id) else { return };
        let mode = *instance.term.mode();
        let up = whole > 0.;
        let count = whole.abs() as usize;
        if mode.intersects(TermMode::MOUSE_MODE) {
            let (row, column, _) = bounds.map_or((0, 0, Side::Left), |bounds| grid::cell_at(bounds, event.position, instance, &metrics));
            let button = if up { mouse::MouseButton::WheelUp } else { mouse::MouseButton::WheelDown };
            if let Some(report) = mouse::report(mode, button, MouseAction::Down, column, row.max(0) as usize, event.modifiers.shift, event.modifiers.alt, event.modifiers.control) {
                instance.input(&report.repeat(count));
            }
        } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            let prefix = if mode.contains(TermMode::APP_CURSOR) { "\x1bO" } else { "\x1b[" };
            instance.input(&format!("{prefix}{}", if up { 'A' } else { 'B' }).repeat(count));
        } else {
            instance.scroll(Scroll::Delta(if up { count as i32 } else { -(count as i32) }));
        }
        cx.notify();
    }

    /// terminalTabsList.ts: one 22px row per terminal, `┌ ├ └` before the panes of a split group, the inline
    /// Split and Kill actions on hover once the list is wide enough.
    fn terminal_tabs_list(&self, at_left: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let view = &self.terminal_view;
        let foreground = self.foreground_or("foreground");
        let active_id = view.active_instance_id();
        let mut list = div().id("terminal-tabs").w(px(TABS_DEFAULT_WIDTH)).h_full().flex_none().flex().flex_col().overflow_y_scroll().border_color(self.color("panel.border"));
        list = if at_left { list.border_r_1() } else { list.border_l_1() };
        for group in &view.groups {
            for (index, &id) in group.instances.iter().enumerate() {
                let Some(instance) = view.instance(id) else { continue };
                let prefix = match (group.instances.len(), index) {
                    (1, _) => "",
                    (_, 0) => "┌",
                    (count, index) if index + 1 == count => "└",
                    _ => "├",
                };
                let active = Some(id) == active_id;
                let row = div()
                    .id(("terminal-tab", id as usize))
                    .group("terminal-tab")
                    .relative()
                    .h(px(TAB_HEIGHT))
                    .flex_none()
                    .flex()
                    .items_center()
                    .pl(px(8.))
                    .pr(px(4.))
                    .gap(px(6.))
                    .text_size(px(13.))
                    .cursor_pointer()
                    .text_color(foreground)
                    .when(active, |row| row.bg(self.color("list.inactiveSelectionBackground")).text_color(self.foreground_or("list.inactiveSelectionForeground")))
                    .when(!active, |row| row.hover(|row| row.bg(self.color("list.hoverBackground"))))
                    .when(active, |row| row.child(div().absolute().left_0().top_0().bottom_0().w(px(1.)).bg(self.color("terminal.tab.activeBorder"))))
                    .when(!prefix.is_empty(), |row| row.child(prefix))
                    .child(self.icon("terminal", 16., 16., foreground))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(instance.title()))
                    .when(TABS_DEFAULT_WIDTH >= TABS_ACTIONBAR_MINIMUM_WIDTH, |row| {
                        row.child(
                            div()
                                .flex()
                                .invisible()
                                .group_hover("terminal-tab", |actions| actions.visible())
                                .child(self.terminal_tab_action("split-horizontal", id, false, cx))
                                .child(self.terminal_tab_action("trash", id, true, cx)),
                        )
                    })
                    .on_click(cx.listener(move |workbench, event: &gpui::ClickEvent, window, cx| {
                        workbench.terminal_view.set_active(id);
                        if event.click_count() >= 2 {
                            workbench.show_terminal_rename(id, window, cx);
                            return;
                        }
                        window.focus(&workbench.terminal_view.focus, cx);
                        cx.notify();
                    }));
                list = list.child(row);
            }
        }
        // terminalTabsList.ts: a double click below the rows creates a terminal.
        list.child(div().id("terminal-tabs-empty").flex_1().min_h(px(TAB_HEIGHT)).on_click(cx.listener(|workbench, event: &gpui::ClickEvent, window, cx| {
            if event.click_count() >= 2 {
                workbench.create_terminal(None, false, window, cx);
            }
        })))
    }

    fn terminal_tab_action(&self, icon: &'static str, id: u32, kill: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let hover = self.color("toolbar.hoverBackground");
        self.icon(icon, 20., 16., self.foreground_or("icon.foreground")).id((icon, id as usize)).rounded(px(4.)).cursor_pointer().hover(|action| action.bg(hover)).on_click(cx.listener(move |workbench, _: &gpui::ClickEvent, window, cx| {
            cx.stop_propagation();
            if kill {
                workbench.kill_terminal(id, window, cx);
            } else {
                workbench.terminal_view.set_active(id);
                workbench.create_terminal(None, true, window, cx);
            }
        }))
    }
}

/// The profile's path, with its arguments when it has any (`_createProfileQuickPickItem`).
fn profile_description(profile: &Profile) -> String {
    let path = profile.path.to_string_lossy();
    if profile.args.is_empty() {
        return path.into_owned();
    }
    let args: Vec<String> = profile.args.iter().map(|arg| if arg.contains(' ') { format!("\"{}\"", arg.replace('"', "\\\"")) } else { arg.clone() }).collect();
    format!("{path} {}", args.join(" "))
}

