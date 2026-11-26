//! The Terminal view (terminalView.ts `TerminalViewPane` with terminalTabbedView.ts): the groups of terminals,
//! side by side when split, with the tabs list at their right once there are two terminals.

pub(super) mod grid;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::Side;
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{Bounds, ClipboardItem, Context, Div, FocusHandle, FontWeight, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent, SharedString, Size, Window, canvas, div, prelude::*, px};
use config::configuration::Overrides;
use config::service::Target;
use serde_json::{Value, json};

use self::grid::{CellMetrics, GUTTER, Look, SCROLLBAR_WIDTH, SCROLLBAR_WIDTH_MODERN};
use super::Workbench;
use super::panel::PanelView;
use crate::commands::Outcome;
use crate::keybindings::Resolution;
use crate::main_thread_quick_open::Owner;
use crate::menus::MenuId;
use crate::quick_input::{Button, Entry, Event, Item, Kind, QuickInput, Severity};
use crate::terminal::colors::TerminalColors;
use crate::terminal::environment_collection::EnvironmentCollections;
use crate::terminal::keys::{self, KeyResult};
use crate::terminal::mouse::{self, MouseAction};
use crate::terminal::profiles::{self, Profile};
use crate::terminal::pty::PtyEvent;
use crate::terminal::skip_shell::DEFAULT_COMMANDS_TO_SKIP_SHELL;
use crate::terminal::{ShellLaunchConfig, TerminalInstance, create_environment};

/// `TerminalExitReason`.
pub(super) const EXIT_REASON_PROCESS: u8 = 2;
pub(super) const EXIT_REASON_USER: u8 = 3;
pub(super) const EXIT_REASON_EXTENSION: u8 = 4;

/// terminal.ts `TERMINAL_VIEW_ID`.
pub const TERMINAL_VIEW_ID: &str = "terminal";

/// terminalInstance.ts `_setupTitlePolling`: node-pty's process title is read every 200 ms on macOS.
const TITLE_POLL_INTERVAL: Duration = Duration::from_millis(200);
/// xterm.js CursorBlinkStateManager `BLINK_INTERVAL`.
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(600);
/// terminalTabsList.ts `TerminalTabsListSizes`.
const TAB_HEIGHT: f32 = 22.;
const TABS_DEFAULT_WIDTH: f32 = 120.;
const TABS_ACTIONBAR_MINIMUM_WIDTH: f32 = 105.;
/// monaco's `ScrollableElement` wheel sensitivity: px per wheel unit.
const SCROLL_WHEEL_SENSITIVITY: f32 = 50.;

pub struct TerminalView {
    pub(super) focus: FocusHandle,
    pub(super) instances: Vec<TerminalInstance>,
    /// terminalGroupService.ts groups: each holds instance ids, laid out side by side.
    groups: Vec<Group>,
    active_group: usize,
    next_id: u32,
    pub(super) metrics: Option<CellMetrics>,
    /// Where each instance's pane painted, for its mouse events.
    pub(super) pane_bounds: HashMap<u32, Bounds<Pixels>>,
    hovered: Option<u32>,
    /// The instance a left button drag selects in.
    selecting: Option<u32>,
    /// The wheel's fraction of a line not scrolled yet.
    scroll_remainder: f32,
    title_polling: bool,
    /// The focused cursor blinks (`cursorBlinking` or a program's DECSCUSR) and is in its hidden phase.
    cursor_blinking: bool,
    cursor_blink_off: bool,
    cursor_blink_running: bool,
    /// The Cmd+trackpad zoom's fraction of a step not applied yet.
    pub(super) zoom_remainder: f32,
    /// Set once a terminal was created: `terminalHasBeenCreated`.
    has_been_created: bool,
    pub(super) find: super::terminal_find::TerminalFind,
    /// What the extension host was told (mainThreadTerminalService.ts): the active terminal and the events it asked for.
    pub(super) host: HostTerminals,
    pub(super) suggest: super::terminal_suggest::TerminalSuggest,
    pub(super) history: super::terminal_history::TerminalHistories,
    /// The instance whose sticky scroll overlay the pointer is over.
    pub(super) sticky_hovered: Option<u32>,
    /// The command guide under the pointer: its instance and buffer rows.
    pub(super) command_guide: Option<(u32, std::ops::Range<i64>)>,
    /// The shown OSC 99 notifications: instance, the handler's key, the notification's key.
    pub(super) osc99_notifications: Vec<(u32, u64, u64)>,
    /// The last resize: its instance, `cols x rows` and when.
    pub(super) resize_overlay: Option<(u32, String, std::time::Instant)>,
    pub(super) quick_fixes: super::terminal_quick_fix::TerminalQuickFixes,
    /// The link under the pointer and whether the link modifier is down.
    pub(super) hovered_link: Option<super::terminal_links::HoveredLink>,
    pub(super) link_modifier: bool,
    /// The instances whose initial hint a key or a click dismissed for good.
    pub(super) initial_hint_dismissed: std::collections::HashSet<u32>,
}

#[derive(Default)]
pub(super) struct HostTerminals {
    pub active: Option<u32>,
    /// `$startSendingDataEvents`: process data goes to the host (`window.onDidWriteTerminalData`).
    pub data_events: bool,
    /// `$startSendingCommandEvents`: finished commands go to the host (`onDidExecuteTerminalCommand`).
    pub command_events: bool,
    pub environment_collections: EnvironmentCollections,
    pub reported: HashMap<u32, super::terminal_host::Reported>,
}

struct Group {
    instances: Vec<u32>,
    active: usize,
}

/// What a terminal quick input does when it is accepted.
#[derive(Clone)]
pub enum TerminalInput {
    /// `TerminalProfileQuickpick`: create a terminal with the picked profile, or make it the default.
    Profiles { profiles: Vec<Profile>, set_default: bool },
    /// `renameWithQuickPick` for an instance id.
    Rename(u32),
    /// The "Configure Terminal Profile" button's name input: the profile to copy and the names already configured.
    ProfileName { profile: Profile, existing: Vec<String> },
    /// Send Sequence's input box.
    SendSequence,
    /// Write Data to Terminal's input box.
    WriteData,
    /// Send Signal's picker, then its manual entry.
    Signal,
    ManualSignal,
    /// Run Recent Command and Go to Recent Directory: what each pick runs, in which terminal.
    RunRecent { kind: super::terminal_history::RecentKind, picks: Vec<super::terminal_history::RecentPick>, fuzzy: bool, instance: u32 },
    /// The command decorations' Toggle Visibility picker.
    DecorationVisibility,
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
            cursor_blinking: false,
            cursor_blink_off: false,
            cursor_blink_running: false,
            zoom_remainder: 0.,
            find: super::terminal_find::TerminalFind::new(cx),
            has_been_created: false,
            host: HostTerminals::default(),
            suggest: Default::default(),
            history: Default::default(),
            sticky_hovered: None,
            command_guide: None,
            osc99_notifications: Vec::new(),
            resize_overlay: None,
            quick_fixes: Default::default(),
            hovered_link: None,
            link_modifier: false,
            initial_hint_dismissed: Default::default(),
        }
    }

    pub(super) fn active_instance_id(&self) -> Option<u32> {
        let group = self.groups.get(self.active_group)?;
        group.instances.get(group.active).copied()
    }

    pub(super) fn instance(&self, id: u32) -> Option<&TerminalInstance> {
        self.instances.iter().find(|instance| instance.id == id)
    }

    pub(super) fn instance_mut(&mut self, id: u32) -> Option<&mut TerminalInstance> {
        self.instances.iter_mut().find(|instance| instance.id == id)
    }

    pub(super) fn active_instance_mut(&mut self) -> Option<&mut TerminalInstance> {
        let id = self.active_instance_id()?;
        self.instance_mut(id)
    }

    /// The instance ids of each group, in order.
    pub(super) fn group_instances(&self) -> Vec<Vec<u32>> {
        self.groups.iter().map(|group| group.instances.clone()).collect()
    }

    /// `showBackgroundTerminal`: a terminal hidden from the user joins the view as its own group.
    pub(super) fn show_background(&mut self, id: u32) {
        if self.instance(id).is_some() && !self.groups.iter().any(|group| group.instances.contains(&id)) {
            self.groups.push(Group { instances: vec![id], active: 0 });
        }
    }

    /// `setActiveInstance`: the group holding `id` and its pane become active.
    pub(super) fn set_active(&mut self, id: u32) {
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
        self.commands.register("workbench.action.terminal.killViewOrEditor", |workbench, _, window, cx| {
            if let Some(id) = workbench.terminal_view.active_instance_id() {
                workbench.kill_terminal(id, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.killEditor", |workbench, _, window, cx| {
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
            workbench.sync_terminals_to_host();
            cx.notify();
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.clearSelection", |workbench, _, _, cx| {
            if let Some(instance) = workbench.terminal_view.active_instance_mut() {
                instance.term.selection = None;
            }
            workbench.sync_terminals_to_host();
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
        self.register_external_terminal_actions();
        self.register_terminal_contrib_actions();
        self.register_terminal_suggest_actions();
        self.register_terminal_quick_fix_actions();
        self.register_terminal_initial_hint_actions();
        self.register_terminal_history_actions();
        self.register_terminal_sticky_scroll_actions();
        self.register_terminal_environment_actions();
        self.register_terminal_decoration_actions();
        self.register_terminal_find_actions();
    }

    /// `isViewVisible(TERMINAL_VIEW_ID)`: the panel shows the Terminal view.
    pub(crate) fn terminal_view_visible(&self) -> bool {
        self.panel_view_visible(TERMINAL_VIEW_ID)
    }

    /// terminal.contribution.ts registers the Terminal view container in the panel.
    pub(super) fn register_terminal_panel_view(&mut self) {
        self.register_panel_view(PanelView {
            id: TERMINAL_VIEW_ID,
            view_id: "terminal",
            title: "Terminal",
            open: |workbench, window, cx| drop(workbench.execute_command("workbench.action.terminal.focus", Vec::new(), window, cx)),
            render: |workbench, window, cx| {
                // terminalView.ts `_initializeTerminal`: the view made visible without terminals creates one.
                if workbench.terminal_view.instances.is_empty() {
                    workbench.create_terminal(None, false, window, cx);
                }
                workbench.terminal_body(window, cx).into_any_element()
            },
            actions: |workbench, _, cx| workbench.terminal_title_actions(cx).into_any_element(),
            visible: |_| true,
            badge: |_| None,
        });
    }

    /// `terminalGroupService.showPanel(true)`: shows the view, creating a terminal when there is none.
    pub(super) fn open_terminal_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_view.instances.is_empty() {
            self.create_terminal(None, false, window, cx);
            return;
        }
        self.show_terminal_tab(window, cx);
    }

    pub(super) fn show_terminal_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_panel_view(TERMINAL_VIEW_ID, cx);
        window.focus(&self.terminal_view.focus, cx);
        self.sync_terminals_to_host();
    }

    /// `viewsService.closeView(TERMINAL_VIEW_ID)`: the terminals keep running.
    pub(super) fn close_terminal_view(&mut self, cx: &mut Context<Self>) {
        self.close_panel_view(TERMINAL_VIEW_ID, cx);
    }

    /// `createTerminal`: a new group, or a pane after the active one with `split`; the split's cwd is its parent's
    /// (`terminal.integrated.splitCwd` inherited).
    pub(super) fn create_terminal(&mut self, profile: Option<Profile>, split: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.create_terminal_with_cwd(profile, split, None, window, cx);
    }

    /// `createTerminal({ config: { cwd } })` of externalTerminal.contribution.ts `openInIntegratedTerminal`.
    pub(super) fn create_terminal_in(&mut self, cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.create_terminal_with_cwd(None, false, Some(cwd), window, cx);
    }

    fn create_terminal_with_cwd(&mut self, profile: Option<Profile>, split: bool, cwd: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let profile = profile.unwrap_or_else(|| self.default_terminal_profile());
        let parent = self.terminal_view.active_instance_id().filter(|_| split);
        let parent_cwd = match parent {
            Some(id) if self.setting("terminal.integrated.splitCwd") == "inherited" => self.terminal_view.instance(id).and_then(|parent| parent.cwd.clone()),
            _ => None,
        };
        let cwd = cwd.or(parent_cwd.map(PathBuf::from)).unwrap_or_else(|| self.terminal_initial_cwd());
        let launch = self.shell_launch_config(&profile, cwd);
        self.add_terminal(launch, parent, window, cx);
        self.show_terminal_tab(window, cx);
    }

    /// terminalService.ts `createTerminal` and terminalInstance.ts's process start: the instance joins a new group,
    /// or `split_from`'s group after it, unless it is hidden from the user; the extension host hears of it.
    pub(super) fn add_terminal(&mut self, mut launch: ShellLaunchConfig, split_from: Option<u32>, window: &mut Window, cx: &mut Context<Self>) -> u32 {
        let id = self.terminal_view.next_id;
        self.terminal_view.next_id += 1;
        self.prepare_terminal_launch(&mut launch);
        let view = &self.terminal_view;
        let (cols, rows) = view.active_instance_id().and_then(|id| view.instance(id)).map_or((80, 30), |instance| (instance.term.columns() as u16, instance.term.screen_lines() as u16));
        let hidden = launch.hide_from_user;
        let (mut instance, receiver) = TerminalInstance::new(id, launch, cols, rows, self.terminal_config());
        if let Some(message) = instance.spawn_error.clone() {
            let colors = TerminalColors::new(&self.theme);
            instance.write_message(&message, &colors);
        }
        self.connect_terminal_to_host(&mut instance);
        let view = &mut self.terminal_view;
        view.instances.push(instance);
        view.has_been_created = true;
        if !hidden {
            let group = split_from.and_then(|parent| view.groups.iter().position(|group| group.instances.contains(&parent)));
            match group {
                Some(index) => {
                    let group = &mut view.groups[index];
                    let parent = group.instances.iter().position(|&instance| Some(instance) == split_from).expect("the group holds the parent");
                    group.instances.insert(parent + 1, id);
                    group.active = parent + 1;
                    view.active_group = index;
                }
                None => {
                    view.groups.push(Group { instances: vec![id], active: 0 });
                    view.active_group = view.groups.len() - 1;
                }
            }
        }
        if let Some(receiver) = receiver {
            self.pump_terminal_events(id, receiver, window, cx);
        }
        self.terminal_opened(id, cx);
        self.start_title_poll(cx);
        self.start_cursor_blink(cx);
        cx.notify();
        id
    }

    /// `getDefaultProfile` over the detected and configured profiles.
    pub(super) fn default_terminal_profile(&self) -> Profile {
        let profiles = self.terminal_profiles();
        let default_name = self.setting("terminal.integrated.defaultProfile.osx");
        let env: HashMap<String, String> = std::env::vars().collect();
        profiles::default_profile(&profiles, default_name.as_str(), &profiles::system_shell(&env))
    }

    fn terminal_profiles(&self) -> Vec<Profile> {
        let shells = std::fs::read_to_string("/etc/shells").unwrap_or_default();
        profiles::detect(&self.setting("terminal.integrated.profiles.osx"), &shells, std::env::var("PATH").ok().as_deref())
    }

    /// `getCwd`: `terminal.integrated.cwd` with its variables resolved, against the workspace folder, else the
    /// folder, else the home directory; one that fails to resolve is dropped (`_resolveCwd`).
    pub(super) fn terminal_initial_cwd(&self) -> PathBuf {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
        let root = self.workspace().and_then(|workspace| workspace.folders.first()).map(|folder| folder.path.clone());
        let configured = self.setting("terminal.integrated.cwd").as_str().filter(|cwd| !cwd.is_empty()).map(str::to_owned);
        let resolved = configured.and_then(|cwd| match self.resolve_variables(&Value::String(cwd), root.clone()) {
            Ok(Value::String(cwd)) if !cwd.is_empty() => Some(PathBuf::from(cwd)),
            Ok(_) => None,
            Err(error) => {
                eprintln!("Could not resolve terminal cwd {error}");
                None
            }
        });
        match (resolved, root) {
            (Some(cwd), _) if cwd.is_absolute() => cwd,
            (Some(cwd), Some(root)) => root.join(cwd),
            (_, Some(root)) => root,
            (_, None) => home,
        }
    }

    fn shell_launch_config(&self, profile: &Profile, cwd: PathBuf) -> ShellLaunchConfig {
        ShellLaunchConfig {
            name: profile.override_name.then(|| profile.name.clone()),
            executable: profile.path.clone(),
            args: profile.args.clone(),
            cwd,
            env: self.terminal_environment(&profile.env),
            icon: profile.icon.clone(),
            color: profile.color.clone(),
            ..ShellLaunchConfig::default()
        }
    }

    /// `createTerminalEnvironment` with `terminal.integrated.env.osx`, then the launch's own variables.
    pub(super) fn terminal_environment(&self, launch_env: &HashMap<String, Option<String>>) -> HashMap<String, String> {
        let configured_env: HashMap<String, Option<String>> = match self.setting("terminal.integrated.env.osx") {
            Value::Object(map) => map.into_iter().map(|(key, value)| (key, value.as_str().map(str::to_owned))).collect(),
            _ => HashMap::new(),
        };
        let detect_locale = self.setting("terminal.integrated.detectLocale");
        create_environment(std::env::vars(), &[configured_env, launch_env.clone()], crate::VERSION, "en", detect_locale.as_str().unwrap_or("auto"))
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
        for event in events {
            match event {
                PtyEvent::Data(bytes) => {
                    if self.terminal_data(id, &bytes, window, cx) {
                        self.sync_terminal_suggest(id, window, cx);
                    }
                }
                PtyEvent::Exit(code) => {
                    if let Some(instance) = self.terminal_view.instance_mut(id) {
                        instance.exited(code);
                    }
                    self.terminal_process_exited(id, code, window, cx);
                    return false;
                }
            }
        }
        self.terminal_view.instance(id).is_some()
    }

    /// Output of the process, a PTY's or an extension pty's: the screen, the clipboard and what the host listens to.
    /// Returns whether the prompt input changed.
    pub(super) fn terminal_data(&mut self, id: u32, bytes: &[u8], window: &mut Window, cx: &mut Context<Self>) -> bool {
        let colors = TerminalColors::new(&self.theme);
        let auto_replies = self.setting("terminal.integrated.autoReplies");
        let Some(instance) = self.terminal_view.instance_mut(id) else { return false };
        let processed = instance.process(bytes, &colors);
        let input_changed = processed.input_changed;
        instance.answer_auto_replies(&auto_replies, &processed.text);
        // xtermTerminal.ts loads ClipboardAddon: OSC 52 writes go to the clipboard and `?` reads it back.
        if let Some(text) = instance.clipboard_requests.drain(..).last() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        if !instance.clipboard_loads.is_empty() {
            let text = cx.read_from_clipboard().and_then(|item| item.text()).unwrap_or_default();
            instance.answer_clipboard_loads(&text);
        }
        instance.bell = false;
        self.task_output(id, &processed.text, cx);
        self.record_terminal_history(id, &processed.shell_events);
        let finished = processed.shell_events.iter().any(|event| matches!(event, crate::terminal::shell_integration::ShellEvent::CommandFinished(_)));
        self.terminal_output_to_host(id, processed);
        if finished {
            self.terminal_command_finished_quick_fixes(id, cx);
        }
        self.terminal_osc99(id, window, cx);
        cx.notify();
        input_changed
    }

    /// terminalInstance.ts `_onProcessExit`: a non-zero exit shows its message, then the instance goes.
    pub(super) fn terminal_process_exited(&mut self, id: u32, code: Option<i32>, window: &mut Window, cx: &mut Context<Self>) {
        if self.task_process_exited(id, code, window, cx) {
            // terminalTaskSystem.ts keeps the terminal for reuse, with `waitOnExit`'s message.
            let colors = TerminalColors::new(&self.theme);
            if let Some(instance) = self.terminal_view.instance_mut(id) {
                instance.write_message("\r\n\x1b[0m\x1b[7m * \x1b[0m Terminal will be reused by tasks, press any key to close it. \x1b[0m", &colors);
            }
            return cx.notify();
        }
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
        self.dispose_terminal(id, EXIT_REASON_PROCESS, window, cx);
    }

    /// `killInstance`: the process gets SIGHUP and the instance goes without an exit message.
    pub(super) fn kill_terminal(&mut self, id: u32, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(instance) = self.terminal_view.instance_mut(id) {
            instance.kill();
        }
        self.dispose_terminal(id, EXIT_REASON_USER, window, cx);
    }

    /// terminalGroup.ts `_onInstanceDisposed`: the next pane of the group becomes active; an empty group goes, and
    /// with it the view when it was the last one and `terminal.integrated.hideOnLastClosed` is on.
    pub(super) fn dispose_terminal(&mut self, id: u32, exit_reason: u8, window: &mut Window, cx: &mut Context<Self>) {
        let view = &mut self.terminal_view;
        let had_focus = view.focus.is_focused(window);
        let Some(index) = view.instances.iter().position(|instance| instance.id == id) else { return };
        let instance = view.instances.remove(index);
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
        self.terminal_closed(instance, exit_reason);
        // terminalTaskSystem.ts `onDisposed`: a task whose terminal goes ends with it.
        self.task_process_exited(id, None, window, cx);
        if self.terminal_view.instances.is_empty() {
            if self.setting("terminal.integrated.hideOnLastClosed") == true {
                self.close_terminal_view(cx);
            }
        } else if had_focus {
            window.focus(&self.terminal_view.focus, cx);
        }
        cx.notify();
    }

    /// xterm.js `CursorBlinkStateManager`: the cursor toggles every 600 ms while the focused terminal blinks.
    fn start_cursor_blink(&mut self, cx: &mut Context<Self>) {
        if self.terminal_view.cursor_blink_running {
            return;
        }
        self.terminal_view.cursor_blink_running = true;
        cx.spawn(async move |workbench, cx| {
            loop {
                cx.background_executor().timer(CURSOR_BLINK_INTERVAL).await;
                let running = workbench.update(cx, |workbench, cx| {
                    let view = &mut workbench.terminal_view;
                    view.cursor_blink_off = view.cursor_blinking && !view.cursor_blink_off;
                    if view.cursor_blinking {
                        cx.notify();
                    }
                    view.cursor_blink_running = !view.instances.is_empty();
                    view.cursor_blink_running
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
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
                    view.title_polling = !view.instances.is_empty();
                    let polling = view.title_polling;
                    if changed {
                        workbench.sync_terminals_to_host();
                        cx.notify();
                    }
                    polling
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// `TerminalProfileQuickpick.showAndGetResult`: configured profiles, then the detected ones, each list with the
    /// profile `terminal.integrated.defaultProfile.osx` names first (`getDefaultProfileName`), then by name.
    // shortcut: contributed profiles (`contributes.terminal.profiles`) are not listed; no built-in of the port has one.
    fn show_terminal_profiles(&mut self, set_default: bool, window: &mut Window, cx: &mut Context<Self>) {
        let default_name = self.setting("terminal.integrated.defaultProfile.osx").as_str().unwrap_or_default().to_owned();
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
                let configure = Button { handle: 0, codicon: Some("gear".into()), tooltip: Some("Configure Terminal Profile".into()), ..Button::default() };
                entries.push(Entry::Item(Item {
                    handle: picks.len() as i64,
                    label: format!("$({}) {}", profile.icon.as_deref().unwrap_or("terminal"), profile.name),
                    description: Some(profile_description(&profile)),
                    buttons: vec![configure],
                    ..Item::default()
                }));
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
        let Some(Owner::Terminal(owner)) = self.quick_inputs.owner(id) else { return };
        let owner = owner.clone();
        match (event, owner) {
            (Event::Accept(_), TerminalInput::Profiles { profiles, set_default }) => {
                let selected = self.quick_inputs.get(id).and_then(|input| input.selected_handles().first().copied());
                let picked = selected.map(|handle| profiles[handle as usize].clone());
                self.hide_quick_input_by_id(id, window, cx);
                match (picked, set_default) {
                    (Some(profile), true) => self.update_setting("terminal.integrated.defaultProfile.osx", json!(profile.name)),
                    (Some(profile), false) => self.create_terminal(Some(profile), false, window, cx),
                    (None, _) => {}
                }
            }
            (Event::TriggerItemButton(handle, _, _), TerminalInput::Profiles { profiles, .. }) => {
                let profile = profiles[handle as usize].clone();
                self.hide_quick_input_by_id(id, window, cx);
                self.show_terminal_profile_name(profile, window, cx);
            }
            (Event::Accept(_), TerminalInput::Rename(instance)) => {
                let value = self.quick_inputs.get(id).map(|input| input.input_box.value.clone()).unwrap_or_default();
                self.hide_quick_input_by_id(id, window, cx);
                // `if (title) instance.rename(title)`: an empty name keeps the title.
                if !value.is_empty() {
                    self.rename_terminal(instance, value);
                    cx.notify();
                }
            }
            (Event::ChangeValue(value), TerminalInput::ProfileName { existing, .. }) => {
                let message = existing.contains(&value).then(|| "A terminal profile already exists with that name".to_owned());
                let Some(input) = self.quick_inputs.get_mut(id) else { return };
                input.severity = if message.is_some() { Severity::Error } else { Severity::Ignore };
                input.validation_message = message;
                cx.notify();
            }
            (Event::Accept(_), TerminalInput::ProfileName { profile, existing }) => {
                let Some(name) = self.quick_inputs.get(id).map(|input| input.input_box.value.clone()) else { return };
                if name.is_empty() || existing.contains(&name) {
                    return;
                }
                // `_createNewProfileConfig`: the path, and args and env when the profile has them.
                let mut config = json!({ "path": profile.path });
                if !profile.args.is_empty() {
                    config["args"] = json!(profile.args);
                }
                if !profile.env.is_empty() {
                    config["env"] = json!(profile.env);
                }
                self.hide_quick_input_by_id(id, window, cx);
                let key = "terminal.integrated.profiles.osx";
                let mut settings = self.settings.lock().unwrap();
                let mut profiles = settings.configuration.inspect(key, &Overrides::default()).user_local.value.filter(Value::is_object).unwrap_or_else(|| json!({}));
                profiles[name.as_str()] = config;
                match settings.update_value(key, Some(&profiles), Target::User, None, &[]) {
                    Ok(change) if !change.is_empty() => self.host.send_configuration_change(&settings, &change),
                    Ok(_) => {}
                    Err(error) => eprintln!("writing {key}: {error}"),
                }
            }
            (event, TerminalInput::RunRecent { kind, picks, fuzzy, instance }) => self.run_recent_input_event(id, event, kind, picks, fuzzy, instance, window, cx),
            (Event::Hide, _) => self.quick_inputs.remove(id),
            (Event::Accept(_), TerminalInput::DecorationVisibility) => self.hide_quick_input_by_id(id, window, cx),
            (event, TerminalInput::DecorationVisibility) => self.decoration_visibility_event(event),
            (event, input @ (TerminalInput::SendSequence | TerminalInput::WriteData | TerminalInput::Signal | TerminalInput::ManualSignal)) => self.terminal_contrib_input_event(id, event, input, window, cx),
            _ => {}
        }
    }

    /// `onDidTriggerItemButton` of the profile picker: name a copy of the profile in the user's profiles.
    fn show_terminal_profile_name(&mut self, profile: Profile, window: &mut Window, cx: &mut Context<Self>) {
        let configured = self.setting("terminal.integrated.profiles.osx");
        let existing: Vec<String> = configured.as_object().map(|profiles| profiles.keys().cloned().collect()).unwrap_or_default();
        let mut input = QuickInput::new(0, Kind::InputBox);
        input.prompt = Some("Enter terminal profile name".into());
        input.set_value(profile.name.clone());
        let id = self.quick_inputs.add(input, Owner::Terminal(TerminalInput::ProfileName { profile, existing }));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    /// terminalContextKey.ts keys.
    pub(super) fn terminal_context(&self, window: &Window, context: &mut HashMap<String, Value>) {
        let view = &self.terminal_view;
        let focused = view.focus.is_focused(window);
        let find_focused = view.find.focus.is_focused(window);
        context.insert("terminalFindVisible".into(), json!(view.find.visible));
        context.insert("terminalFindFocused".into(), json!(find_focused));
        context.insert("terminalFindInputFocused".into(), json!(find_focused));
        let selected = view.active_instance_id().and_then(|id| view.instance(id)).is_some_and(|instance| instance.term.selection.as_ref().is_some_and(|selection| !selection.is_empty()));
        context.insert("terminalFocus".into(), json!(focused));
        context.insert("terminalFocusInAny".into(), json!(focused || find_focused));
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
        self.terminal_suggest_context(context);
        if self.terminal_view_visible() {
            context.insert("view".into(), json!(TERMINAL_VIEW_ID));
        }
    }

    /// terminalInstance.ts `attachCustomKeyEventHandler` and xterm.js `_keyDown`: a chord in progress and the
    /// commands to skip the shell go to the workbench, every other key the terminal turns into input. Returns
    /// whether the terminal took the key.
    pub(super) fn terminal_key(&mut self, keystroke: &Keystroke, resolution: &Resolution, window: &Window, cx: &mut Context<Self>) -> bool {
        // The quick fix widget takes the keys while it shows, as its list has the focus upstream.
        if !self.terminal_view.focus.is_focused(window) || self.action_widget_visible() {
            return false;
        }
        // `restartBlinkAnimation` on a key: the cursor shows.
        self.terminal_view.cursor_blink_off = false;
        match resolution {
            Resolution::MoreChordsNeeded if self.setting("terminal.integrated.allowChords") == true && keystroke.key != "escape" => return false,
            Resolution::KbFound { command, .. } if self.setting("terminal.integrated.sendKeybindingsToShell") != true && (keystroke.modifiers.platform || self.skips_shell(command)) => return false,
            _ => {}
        }
        let mac_option_is_meta = self.setting("terminal.integrated.macOptionIsMeta") == true;
        let Some(instance) = self.terminal_view.active_instance_mut() else { return false };
        let application_cursor = instance.term.mode().contains(TermMode::APP_CURSOR);
        match keys::evaluate(keystroke, application_cursor, mac_option_is_meta) {
            // A task terminal whose process ended closes on any key (`waitOnExit`).
            KeyResult::Send(_) if instance.exit_code.is_some() => {
                let id = instance.id;
                cx.defer_in(window, move |workbench, window, cx| workbench.dispose_terminal(id, EXIT_REASON_USER, window, cx));
            }
            KeyResult::Send(data) => {
                instance.input(&data);
                let id = instance.id;
                self.terminal_view.initial_hint_dismissed.insert(id);
                self.terminal_view.suggest.user_data(&data);
                self.terminal_view.command_guide = None;
            }
            KeyResult::PageUp => instance.scroll(Scroll::PageUp),
            KeyResult::PageDown => instance.scroll(Scroll::PageDown),
            KeyResult::SelectAll => instance.select_all(),
            KeyResult::None => return false,
        }
        self.sync_terminals_to_host();
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

    /// The view's title actions in the panel title (terminalMenus.ts `MenuId.ViewTitle`): the active terminal while
    /// the tabs list is hidden, New with the profile dropdown, Split and Kill while the tabs list is hidden.
    pub(super) fn terminal_title_actions(&self, cx: &mut Context<Self>) -> Div {
        // terminalMenus.ts: the active terminal entry and the inline Split and Kill actions follow
        // `tabs.showActiveTerminal` and `tabs.showActions` (`shouldShowViewInlineActions`) over the group count.
        let tabs_enabled = self.setting("terminal.integrated.tabs.enabled") == true;
        let single_group = self.terminal_view.groups.len() == 1;
        let shown_for = |key: &str| match self.setting(key).as_str() {
            Some("always") => true,
            Some("singleTerminal" | "singleTerminalOrNarrow" | "singleGroup") => single_group,
            _ => false,
        };
        let show_active_terminal = tabs_enabled && shown_for("terminal.integrated.tabs.showActiveTerminal");
        let show_inline_actions = self.setting("terminal.integrated.tabs.hideCondition") != "never" && (!tabs_enabled || shown_for("terminal.integrated.tabs.showActions"));
        let active_title = self.terminal_view.active_instance_id().and_then(|id| self.terminal_view.instance(id)).map(|instance| self.single_tab_title(instance));
        let foreground = self.foreground_or("icon.foreground");
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .when_some(active_title.filter(|_| show_active_terminal), |actions, title| {
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
            // dropdown.css `.monaco-dropdown-with-primary`: New Terminal and its 12px profile chevron side by side.
            .child(div().flex().child(self.search_action("add", "workbench.action.terminal.new", true, cx)).child(self.terminal_profile_chevron(foreground, cx)))
            .when(show_inline_actions, |actions| {
                actions.child(self.turned_action("split-horizontal", self.panel_title_icon_turns(), "workbench.action.terminal.split", true, cx)).child(self.search_action("trash", "workbench.action.terminal.kill", true, cx))
            })
    }

    /// terminalView.ts `getSingleTabTitle`: `<title> <separator> <description>`, its whitespace collapsed as HTML does.
    fn single_tab_title(&self, instance: &TerminalInstance) -> String {
        let description = self.terminal_description(instance);
        if description.is_empty() {
            return instance.title();
        }
        let separator = self.setting("terminal.integrated.tabs.separator");
        let title = format!("{} {} {description}", instance.title(), separator.as_str().unwrap_or(" - "));
        title.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// `TerminalInstance.description`, `terminal.integrated.tabs.description` resolved.
    pub(super) fn terminal_description(&self, instance: &TerminalInstance) -> String {
        let template = self.setting("terminal.integrated.tabs.description");
        let separator = self.setting("terminal.integrated.tabs.separator");
        let workspace = self.workspace();
        let launch_folder = workspace.and_then(|workspace| workspace.folders.iter().filter(|folder| instance.launch.cwd.starts_with(&folder.path)).max_by_key(|folder| folder.path.as_os_str().len()));
        let workspace_folder = launch_folder.map(|folder| folder.path.clone()).or_else(|| self.last_active_workspace_root().map(std::path::PathBuf::from));
        let user_home = std::env::var("HOME").unwrap_or_default();
        let input = crate::terminal::label::LabelInput {
            process: &instance.process_title,
            sequence: instance.sequence_title.as_deref(),
            cwd: instance.cwd.as_deref(),
            user_home: &user_home,
            workspace_folder: workspace_folder.as_deref(),
            multi_root: workspace.is_some_and(|workspace| workspace.folders.len() > 1),
            task: instance.launch.is_feature_terminal,
            shell_type: instance.shell_type,
        };
        crate::terminal::label::description(template.as_str().unwrap_or_default(), separator.as_str().unwrap_or(" - "), &input)
    }

    fn terminal_profile_chevron(&self, foreground: gpui::Rgba, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let hover = self.color("toolbar.hoverBackground");
        self.icon("chevron-down", 16., 12., foreground).id("workbench.action.terminal.newWithProfile").rounded(px(4.)).cursor_pointer().hover(|action| action.bg(hover)).on_click(cx.listener(|workbench, _: &gpui::ClickEvent, window, cx| {
            cx.stop_propagation();
            workbench.execute_command("workbench.action.terminal.newWithProfile", Vec::new(), window, cx);
        }))
    }

    fn terminal_tabs_shown(&self) -> bool {
        let enabled = self.setting("terminal.integrated.tabs.enabled") == true;
        let hide = self.setting("terminal.integrated.tabs.hideCondition");
        self.terminal_view.should_show_tabs(enabled, hide.as_str().unwrap_or("singleTerminal"))
    }

    /// The view below its title: the active group's panes and, at `terminal.integrated.tabs.location`, the tabs list.
    pub(super) fn terminal_body(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let view = &self.terminal_view;
        let background = self.theme.color("terminal.background").unwrap_or(self.color("panel.background"));
        let border = self.color("terminal.border");
        let mut panes = div().relative().flex_1().min_w_0().h_full().flex();
        if let Some(group) = view.groups.get(view.active_group) {
            for (index, &id) in group.instances.iter().enumerate() {
                let focused = view.focus.is_focused(window) && view.active_instance_id() == Some(id);
                panes = panes.child(self.terminal_pane(id, index > 0, border, focused, cx));
            }
        }
        if let Some(find) = self.terminal_find_widget(window, cx) {
            panes = panes.child(find);
        }
        if let Some(suggest) = self.terminal_suggest_widget(window, cx) {
            panes = panes.child(suggest);
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

    fn terminal_pane(&self, id: u32, border_left: bool, border: gpui::Rgba, focused: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        div()
            .id(("terminal-pane", id as usize))
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .cursor_text()
            .when(border_left, |pane| pane.border_l_1().border_color(border))
            .child(canvas(|_, _, _| {}, move |bounds, _, window, cx| entity.update(cx, |workbench, _| workbench.paint_terminal_pane(id, bounds, window))).size_full())
            .children(self.terminal_sticky_scroll_hit_area(id, cx))
            .children(self.terminal_resize_overlay(id))
            .children(self.terminal_command_guide(id))
            .children(self.terminal_navigation_highlight(id))
            .children(self.terminal_decorations(id, cx))
            .children(self.terminal_initial_hint(id, focused, cx))
            .on_hover(cx.listener(move |workbench, hovered: &bool, _, cx| {
                workbench.terminal_view.hovered = hovered.then_some(id);
                if !hovered {
                    workbench.terminal_view.command_guide = None;
                }
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| workbench.terminal_mouse_down(id, event, window, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| workbench.terminal_mouse_down(id, event, window, cx)))
            .on_mouse_down(MouseButton::Middle, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| workbench.terminal_mouse_down(id, event, window, cx)))
            .on_mouse_move(cx.listener(move |workbench, event: &MouseMoveEvent, window, cx| workbench.terminal_mouse_move(id, event, window, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(move |workbench, event: &MouseUpEvent, _, cx| workbench.terminal_mouse_up(id, event, cx)))
            .on_scroll_wheel(cx.listener(move |workbench, event: &ScrollWheelEvent, _, cx| workbench.terminal_wheel(id, event, cx)))
    }

    /// Lays out and paints one pane: the instance takes the columns and rows its pane fits (`_evaluateColsAndRows`).
    fn paint_terminal_pane(&mut self, id: u32, bounds: Bounds<Pixels>, window: &mut Window) {
        let metrics = self.terminal_metrics(window);
        let scrollbar_width = self.terminal_scrollbar_width();
        let sticky_look = self.terminal_sticky_look(id);
        let colors = TerminalColors::new(&self.theme);
        let cursor_inactive_style = self.setting("terminal.integrated.cursorStyleInactive");
        let find_matches = if self.terminal_view.active_instance_id() == Some(id) { self.terminal_view.find.matches.clone() } else { Vec::new() };
        let focused = self.terminal_view.focus.is_focused(window) && self.terminal_view.active_instance_id() == Some(id);
        if focused {
            // `cursorBlinking` is the default style's; a program's DECSCUSR overrides it.
            self.terminal_view.cursor_blinking = self.terminal_view.instance(id).is_some_and(|instance| instance.term.cursor_style().blinking);
        }
        let link_underline = self.terminal_link_underline(id).map(<[_]>::to_vec).unwrap_or_default();
        let overview_ruler = self.terminal_overview_ruler(id);
        let look = Look {
            colors: &colors,
            metrics: &metrics,
            focused,
            cursor_blink_off: self.terminal_view.cursor_blinking && self.terminal_view.cursor_blink_off,
            cursor_inactive_style: cursor_inactive_style.as_str().unwrap_or("outline"),
            cursor_width: self.setting("terminal.integrated.cursorWidth").as_f64().unwrap_or(1.) as f32,
            minimum_contrast_ratio: self.setting("terminal.integrated.minimumContrastRatio").as_f64().unwrap_or(4.5),
            draw_bold_text_in_bright_colors: self.setting("terminal.integrated.drawBoldTextInBrightColors") == true,
            scrollbar_width,
            hovered: self.terminal_view.hovered == Some(id),
            scrollbar_slider: self.color("scrollbarSlider.background"),
            overview_ruler,
            link_underline: &link_underline,
            find_matches: &find_matches,
            find_current: self.terminal_view.find.current,
            find_match: self.color("terminal.findMatchBackground"),
            find_highlight: self.color("terminal.findMatchHighlightBackground"),
        };
        let (cols, rows) = metrics.grid_size(f32::from(bounds.size.width) - GUTTER - scrollbar_width, f32::from(bounds.size.height));
        let config = self.terminal_config();
        self.terminal_view.pane_bounds.insert(id, bounds);
        let Some(instance) = self.terminal_view.instance_mut(id) else { return };
        instance.cell_size = (metrics.cell_width, metrics.cell_height);
        let resized = instance.resize(cols, rows);
        instance.configure(config);
        let sticky = self.terminal_sticky_scroll(id);
        let instance = self.terminal_view.instance(id).expect("resized above");
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            grid::paint(bounds, instance, &look, window);
            if let Some(sticky) = &sticky {
                grid::paint_sticky_scroll(bounds, instance, sticky, &sticky_look, &look, window);
            }
        });
        if resized {
            self.terminal_resized(id);
            self.show_resize_overlay(id);
        }
        self.repaint_while_resize_overlay_shows(id, window);
    }

    pub(super) fn terminal_scrollbar_width(&self) -> f32 {
        if self.setting("workbench.experimental.modernUI") == true { SCROLLBAR_WIDTH_MODERN } else { SCROLLBAR_WIDTH }
    }

    /// The xterm.js options the settings set: `scrollback`, `wordSeparator` and `cursorStyle`, which a program's
    /// DECSCUSR overrides.
    fn terminal_config(&self) -> Config {
        let shape = match self.setting("terminal.integrated.cursorStyle").as_str() {
            Some("line") => CursorShape::Beam,
            Some("underline") => CursorShape::Underline,
            _ => CursorShape::Block,
        };
        Config {
            scrolling_history: self.setting("terminal.integrated.scrollback").as_u64().unwrap_or(1000) as usize,
            semantic_escape_chars: self.setting("terminal.integrated.wordSeparators").as_str().unwrap_or_default().to_owned(),
            default_cursor_style: CursorStyle { shape, blinking: self.setting("terminal.integrated.cursorBlinking") == true },
            osc52: alacritty_terminal::term::Osc52::CopyPaste,
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

    /// terminalContextMenu.ts `openContextMenu` after `rightClickBehavior`: selectWord (the macOS default) selects
    /// the word under the pointer when nothing is selected, copyPaste copies a selection or pastes, paste pastes.
    #[allow(clippy::too_many_arguments)]
    fn terminal_right_click(&mut self, id: u32, row: i32, column: usize, side: Side, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let behavior = self.setting("terminal.integrated.rightClickBehavior");
        let Some(instance) = self.terminal_view.instance_mut(id) else { return };
        let selection = instance.selection_text();
        match behavior.as_str().unwrap_or("selectWord") {
            "copyPaste" | "paste" => {
                match selection.filter(|_| behavior == "copyPaste") {
                    Some(text) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        instance.term.selection = None;
                    }
                    None => {
                        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                            instance.paste(&text);
                        }
                    }
                }
                self.sync_terminals_to_host();
                cx.notify();
                return;
            }
            "selectWord" if selection.is_none() => instance.start_selection(SelectionType::Semantic, row, column, side),
            "nothing" => return,
            _ => {}
        }
        self.sync_terminals_to_host();
        let context = self.key_context(window);
        let anchor = Bounds { origin: position, size: Size::default() };
        self.show_context_menu(MenuId::TerminalInstanceContext, &context, anchor, Vec::new(), window, cx);
    }

    fn terminal_mouse_down(&mut self, id: u32, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal_view.set_active(id);
        // terminalSuggestAddon.ts: a click in the terminal hides the widget.
        self.hide_terminal_suggest(false);
        window.focus(&self.terminal_view.focus, cx);
        cx.stop_propagation();
        let force_selection = event.modifiers.alt && self.setting("terminal.integrated.macOptionClickForcesSelection") == true;
        let (Some(bounds), Some(metrics)) = (self.terminal_view.pane_bounds.get(&id).copied(), self.terminal_view.metrics.clone()) else { return };
        let Some(instance) = self.terminal_view.instance_mut(id) else { return };
        let (row, column, side) = grid::cell_at(bounds, event.position, instance, &metrics);
        let link_modifier = self.terminal_link_modifier(event.modifiers);
        if event.button == MouseButton::Left && link_modifier && self.open_terminal_link(id, row, column, window, cx) {
            return;
        }
        let Some(instance) = self.terminal_view.instance_mut(id) else { return };
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
        if event.button == MouseButton::Right {
            return self.terminal_right_click(id, row, column, side, event.position, window, cx);
        }
        let kind = match (event.button, event.click_count) {
            (MouseButton::Left, 2) => SelectionType::Semantic,
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
        self.sync_terminals_to_host();
        cx.notify();
    }

    fn terminal_mouse_move(&mut self, id: u32, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(bounds), Some(metrics)) = (self.terminal_view.pane_bounds.get(&id).copied(), self.terminal_view.metrics.clone()) else { return };
        let selecting = self.terminal_view.selecting == Some(id) && event.pressed_button == Some(MouseButton::Left);
        let show_command_guide = self.setting("terminal.integrated.shellIntegration.showCommandGuide") != false;
        let Some(instance) = self.terminal_view.instance(id) else { return };
        let (row, column, side) = grid::cell_at(bounds, event.position, instance, &metrics);
        let guide = show_command_guide.then(|| instance.command_guide_rows(instance.viewport_y() + row as i64)).flatten().map(|rows| (id, rows));
        if guide != self.terminal_view.command_guide {
            self.terminal_view.command_guide = guide;
            cx.notify();
        }
        if event.pressed_button.is_none() {
            self.terminal_link_hover(id, row, column, event.modifiers, window, cx);
        }
        let Some(instance) = self.terminal_view.instance_mut(id) else { return };
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
            return self.sync_terminals_to_host();
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
        let modifiers = event.modifiers;
        if (modifiers.platform || modifiers.control) && !modifiers.shift && !modifiers.alt && self.setting("terminal.integrated.mouseWheelZoom") == true {
            self.terminal_wheel_zoom(event.delta);
            return;
        }
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
                let description = self.terminal_description(instance);
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
                    .child(
                        // iconlabel.css `.label-description`: margin-left .5em, opacity .7, font-size .9em.
                        div().flex_1().min_w_0().flex().items_center().overflow_hidden().whitespace_nowrap().child(div().min_w_0().overflow_hidden().text_ellipsis().child(instance.title())).when(!description.is_empty(), |label| {
                            label.child(div().flex_none().ml(px(6.5)).opacity(0.7).text_size(px(11.7)).child(description))
                        }),
                    )
                    .when(TABS_DEFAULT_WIDTH >= TABS_ACTIONBAR_MINIMUM_WIDTH, |row| {
                        row.child(
                            // terminal.css: `.actions { display: none }`, shown on the hovered row.
                            div()
                                .hidden()
                                .group_hover("terminal-tab", |actions| actions.flex())
                                .child(self.terminal_tab_action("split-horizontal", id, false, cx))
                                .child(self.terminal_tab_action("trash", id, true, cx)),
                        )
                    })
                    .on_click(cx.listener(move |workbench, event: &gpui::ClickEvent, window, cx| {
                        workbench.terminal_view.set_active(id);
                        workbench.sync_terminals_to_host();
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
    let path = match profile.is_from_path {
        true => profile.path.file_name().map_or_else(|| profile.path.to_string_lossy(), |name| name.to_string_lossy()),
        false => profile.path.to_string_lossy(),
    };
    if profile.args.is_empty() {
        return path.into_owned();
    }
    let args: Vec<String> = profile.args.iter().map(|arg| if arg.contains(' ') { format!("\"{}\"", arg.replace('"', "\\\"")) } else { arg.clone() }).collect();
    format!("{path} {}", args.join(" "))
}

