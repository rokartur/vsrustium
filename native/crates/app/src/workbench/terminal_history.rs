//! terminal.history.contribution.ts and terminalRunRecentQuickPick.ts: the command and directory history kept
//! across sessions, Run Recent Command... and Go to Recent Directory....

use std::path::PathBuf;

use alacritty_terminal::grid::{Dimensions, Scroll};
use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::terminal_view::TerminalInput;
use crate::commands::Outcome;
use crate::editor::TextDocument;
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Button, Entry, Event, Header, Item, Kind, QuickInput};
use crate::terminal::history::{self, ShellFileHistory, TerminalPersistedHistory};

const REMOVE_BUTTON: i64 = 0;
const OUTPUT_BUTTON: i64 = 1;
const PIN_BUTTON: i64 = 2;
const PINNED_BUTTON: i64 = 3;
const OPEN_FILE_BUTTON: i64 = 4;
/// The handle of the `{shell} history` header, apart from the picks' handles.
const SHELL_FILE_HEADER: i64 = -1;
/// `TerminalStorageKeys.PinnedRecentCommandsPrefix`.
const PINNED_KEY_PREFIX: &str = "terminal.pinnedRecentCommands";
const FUZZY_BUTTON: i64 = 0;
/// `TerminalOutputProvider.scheme`.
const TERMINAL_OUTPUT_SCHEME: &str = "TERMINAL_OUTPUT";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RecentKind {
    Command,
    Cwd,
}

/// One pick of the Run Recent picker: the text it runs and where it came from.
#[derive(Clone, Debug)]
pub struct RecentPick {
    raw_label: String,
    /// The finished command of this session it shows, by index in the instance's `commands`.
    command: Option<usize>,
    previous_session: bool,
}

/// The two persisted histories, loaded on first use (`getCommandHistory`, `getDirectoryHistory`).
#[derive(Default)]
pub(super) struct TerminalHistories {
    commands: Option<TerminalPersistedHistory<Value>>,
    dirs: Option<TerminalPersistedHistory<Value>>,
    /// The display offset before the picker previewed a command (`markTracker.saveScrollState`).
    saved_scroll: Option<(u32, usize)>,
    /// The shell history file the picker's `{shell} history` header opens.
    shell_file: Option<PathBuf>,
}

impl Workbench {
    pub(super) fn register_terminal_history_actions(&mut self) {
        self.commands.register("workbench.action.terminal.runRecentCommand", |workbench, _, window, cx| {
            workbench.run_recent(RecentKind::Command, false, String::new(), window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.goToRecentDirectory", |workbench, _, window, cx| {
            workbench.run_recent(RecentKind::Cwd, false, String::new(), window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.clearPreviousSessionHistory", |workbench, _, _, _| {
            workbench.history(RecentKind::Command).clear();
            workbench.save_history(RecentKind::Command);
            Outcome::done()
        });
    }

    fn history(&mut self, kind: RecentKind) -> &mut TerminalPersistedHistory<Value> {
        let limit = self.setting("terminal.integrated.shellIntegration.history").as_u64().map_or(history::DEFAULT_HISTORY_LIMIT, |limit| limit as usize);
        let (entries_key, timestamp_key) = keys(kind);
        let storage = self.profile_storage.lock().unwrap();
        let stored = storage.get(entries_key).map(str::to_owned);
        let timestamp = storage.get(timestamp_key).and_then(|timestamp| timestamp.parse().ok()).unwrap_or(0);
        drop(storage);
        let slot = match kind {
            RecentKind::Command => &mut self.terminal_view.history.commands,
            RecentKind::Cwd => &mut self.terminal_view.history.dirs,
        };
        // Another window saved since this one loaded (`onDidChangeValue`): load again.
        if slot.as_ref().is_none_or(|history| history.is_stale(timestamp)) {
            *slot = Some(TerminalPersistedHistory::load(stored.as_deref(), timestamp, limit));
        }
        let history = slot.as_mut().expect("loaded above");
        history.set_limit(limit);
        history
    }

    fn save_history(&mut self, kind: RecentKind) {
        let (json, timestamp) = {
            let history = self.history(kind);
            (history.to_json(), history.timestamp())
        };
        let (entries_key, timestamp_key) = keys(kind);
        let mut storage = self.profile_storage.lock().unwrap();
        storage.set(entries_key, json);
        storage.set(timestamp_key, timestamp.to_string());
    }

    /// `TerminalHistoryContribution`'s listeners: finished commands and new cwds join the histories.
    pub(super) fn record_terminal_history(&mut self, id: u32, events: &[crate::terminal::shell_integration::ShellEvent]) {
        use crate::terminal::shell_integration::ShellEvent;
        let shell_type = self.terminal_view.instance(id).and_then(|instance| instance.shell_type);
        for event in events {
            match event {
                ShellEvent::CommandFinished(command) if !command.command_line.trim().is_empty() => {
                    self.history(RecentKind::Command).add(&command.command_line, json!({ "shellType": shell_type }));
                    self.save_history(RecentKind::Command);
                }
                ShellEvent::Cwd(cwd) => {
                    self.history(RecentKind::Cwd).add(cwd, json!({}));
                    self.save_history(RecentKind::Cwd);
                }
                _ => {}
            }
        }
    }

    /// `showRunRecentQuickPick`.
    fn run_recent(&mut self, kind: RecentKind, fuzzy: bool, value: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.terminal_view.active_instance_id() else {
            self.open_terminal_view(window, cx);
            return;
        };
        let (entries, picks) = self.recent_entries(kind, id, value.is_empty());
        if picks.is_empty() {
            return;
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some(match kind {
            RecentKind::Command => "Select a command to run (hold Option-key to edit the command)".into(),
            RecentKind::Cwd => "Select a directory to go to (hold Option-key to edit the command)".into(),
        });
        input.sort_by_label = false;
        input.match_on_label_contiguous = !fuzzy;
        input.buttons = vec![Button { handle: FUZZY_BUTTON, codicon: Some("search-fuzzy".into()), tooltip: Some("Fuzzy search".into()), toggle: Some(fuzzy), location: Some(3), ..Button::default() }];
        input.set_items(entries);
        if !value.is_empty() {
            input.set_value(value);
        }
        let quick_input = self.quick_inputs.add(input, Owner::Terminal(TerminalInput::RunRecent { kind, picks, fuzzy, instance: id }));
        self.show_quick_input(quick_input, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    /// The picks with quickPickPin.ts `showWithPinnedItems`: a Pin button first on every item, and while the filter
    /// is empty the stored pinned items on top with a Pinned button. Pinned copies always get picks, so the
    /// handles hold whether they show or not.
    fn recent_entries(&mut self, kind: RecentKind, id: u32, show_pinned: bool) -> (Vec<Entry>, Vec<RecentPick>) {
        let (mut entries, mut picks) = match kind {
            RecentKind::Command => self.recent_command_picks(id),
            RecentKind::Cwd => self.recent_cwd_picks(id),
        };
        let mut pinned_items: Vec<Item> = Vec::new();
        for stored in self.pinned_recent(id) {
            let wanted = pin_identifier(stored["label"].as_str().unwrap_or_default(), stored["description"].as_str(), stored["detail"].as_str());
            let found = entries.iter().find_map(|entry| match entry {
                Entry::Item(item) if item_identifier(item) == wanted => Some(item.clone()),
                _ => None,
            });
            let Some(mut item) = found else { continue };
            if pinned_items.iter().any(|pinned| item_identifier(pinned) == wanted) {
                continue;
            }
            picks.push(picks[item.handle as usize].clone());
            item.handle = picks.len() as i64 - 1;
            item.buttons.insert(0, button(PINNED_BUTTON, "pinned", "Pinned command"));
            pinned_items.push(item);
        }
        for entry in &mut entries {
            if let Entry::Item(item) = entry {
                item.buttons.insert(0, button(PIN_BUTTON, "pin", "Pin command"));
            }
        }
        if show_pinned && !pinned_items.is_empty() {
            let mut pinned = vec![Entry::Separator(Some("pinned".into()))];
            pinned.extend(pinned_items.into_iter().map(Entry::Item));
            entries.splice(0..0, pinned);
        }
        (entries, picks)
    }

    fn pinned_key(&self, id: u32) -> String {
        let shell_type = self.terminal_view.instance(id).and_then(|instance| instance.shell_type).unwrap_or("undefined");
        format!("{PINNED_KEY_PREFIX}.{shell_type}")
    }

    fn pinned_recent(&self, id: u32) -> Vec<Value> {
        let stored = self.workspace_storage.lock().unwrap().get(&self.pinned_key(id)).map(str::to_owned);
        stored.and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default()
    }

    /// `updatePinnedItems`: a Pinned button unpins the item, a Pin button pins it (label, description, detail).
    fn toggle_recent_pin(&mut self, id: u32, item: &Item, unpin: bool) {
        let mut pinned = self.pinned_recent(id);
        let identifier = item_identifier(item);
        if unpin {
            pinned.retain(|stored| pin_identifier(stored["label"].as_str().unwrap_or_default(), stored["description"].as_str(), stored["detail"].as_str()) != identifier);
        } else {
            pinned.push(json!({ "label": item.label, "description": item.description, "detail": item.detail }));
        }
        let key = self.pinned_key(id);
        self.workspace_storage.lock().unwrap().set(&key, Value::Array(pinned).to_string());
    }

    fn recent_command_picks(&mut self, id: u32) -> (Vec<Entry>, Vec<RecentPick>) {
        let Some(instance) = self.terminal_view.instance(id) else { return (Vec::new(), Vec::new()) };
        let home = std::env::var("HOME").unwrap_or_default();
        let mut entries = Vec::new();
        let mut picks: Vec<RecentPick> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for (index, command) in instance.commands.iter().enumerate().rev() {
            let label = command.command.trim().to_owned();
            if label.is_empty() || seen.contains(&label) {
                continue;
            }
            let mut description = collapse_tilde_path(command.cwd.as_deref(), &home);
            match command.exit_code {
                Some(-1) => description.push_str(" failed"),
                Some(code) if code != 0 => description.push_str(&format!(" exitCode: {code}")),
                _ => {}
            }
            let buttons = if command.output.is_empty() { Vec::new() } else { vec![button(OUTPUT_BUTTON, "output", "View Command Output")] };
            entries.push(Entry::Item(Item { handle: picks.len() as i64, label: format_label(&label), description: Some(description.trim().to_owned()).filter(|description| !description.is_empty()), buttons, ..Item::default() }));
            picks.push(RecentPick { raw_label: label.clone(), command: Some(index), previous_session: false });
            seen.push(label);
        }
        if !entries.is_empty() {
            entries.insert(0, Entry::Header(header("current session", None, Vec::new())));
        }
        let shell_type = instance.shell_type;
        let previous: Vec<String> = self.terminal_view.history.commands.as_ref().map_or_else(Vec::new, |history| {
            history.entries().iter().filter(|(_, info)| info["shellType"].as_str() == shell_type).map(|(label, _)| label.clone()).collect()
        });
        let mut previous_entries = Vec::new();
        for label in previous.into_iter().rev() {
            if seen.contains(&label) {
                continue;
            }
            previous_entries.push((label.clone(), vec![button(REMOVE_BUTTON, "close", "Remove from Command History")], true));
            seen.push(label);
        }
        push_section(&mut entries, &mut picks, Entry::Header(header("previous session", None, Vec::new())), previous_entries);
        let file_history = shell_type.and_then(|shell_type| history::fetch_shell_file_history(shell_type, &PathBuf::from(&home), &|name| std::env::var(name).ok()).ok().flatten());
        self.terminal_view.history.shell_file = None;
        if let Some(ShellFileHistory { commands, source_label, source_resource }) = file_history {
            let mut file_entries = Vec::new();
            for label in commands.into_iter().rev() {
                if !seen.contains(&label) {
                    seen.push(label.clone());
                    file_entries.push((label, Vec::new(), false));
                }
            }
            if !file_entries.is_empty() {
                self.terminal_view.history.shell_file = Some(source_resource);
            }
            let open_file = button(OPEN_FILE_BUTTON, "go-to-file", "Open File");
            push_section(&mut entries, &mut picks, Entry::Header(Header { handle: SHELL_FILE_HEADER, ..header(&format!("{} history", shell_type.unwrap_or_default()), Some(source_label), vec![open_file]) }), file_entries);
        }
        (entries, picks)
    }

    fn recent_cwd_picks(&self, id: u32) -> (Vec<Entry>, Vec<RecentPick>) {
        let Some(instance) = self.terminal_view.instance(id) else { return (Vec::new(), Vec::new()) };
        let mut entries = Vec::new();
        let mut picks = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        if !instance.cwds.is_empty() {
            entries.push(Entry::Separator(Some("current session".into())));
            for cwd in instance.cwds.iter().rev() {
                seen.push(cwd.clone());
                entries.push(Entry::Item(Item { handle: picks.len() as i64, label: cwd.clone(), ..Item::default() }));
                picks.push(RecentPick { raw_label: cwd.clone(), command: None, previous_session: false });
            }
        }
        let previous: Vec<String> = self.terminal_view.history.dirs.as_ref().map_or_else(Vec::new, |history| history.entries().iter().filter(|(_, info)| info.is_null() || info["remoteAuthority"].is_null()).map(|(label, _)| label.clone()).collect());
        let mut previous_entries = Vec::new();
        for label in previous.into_iter().rev() {
            if !seen.contains(&label) {
                seen.push(label.clone());
                previous_entries.push((label, vec![button(REMOVE_BUTTON, "close", "Remove from Command History")], true));
            }
        }
        push_section(&mut entries, &mut picks, Entry::Separator(Some("previous session".into())), previous_entries);
        (entries, picks)
    }

    pub(super) fn run_recent_input_event(&mut self, quick_input: i64, event: Event, kind: RecentKind, picks: Vec<RecentPick>, fuzzy: bool, instance: u32, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Accept(_) => {
                let Some(handle) = self.quick_inputs.get(quick_input).and_then(|input| input.active_handles().first().copied()) else { return };
                let pick = &picks[handle as usize];
                let text = match kind {
                    RecentKind::Cwd => format!("cd {}", prepare_path_for_shell(&pick.raw_label)),
                    RecentKind::Command => pick.raw_label.clone(),
                };
                // Option held: the command is typed, not run, and the terminal takes the focus.
                let edit = window.modifiers().alt;
                self.terminal_view.history.saved_scroll = None;
                self.hide_quick_input_by_id(quick_input, window, cx);
                if let Some(terminal) = self.terminal_view.instance_mut(instance) {
                    terminal.term.scroll_display(Scroll::Bottom);
                }
                self.run_terminal_command(instance, text, !edit, window, cx);
                self.open_panel_view(super::terminal_view::TERMINAL_VIEW_ID, cx);
                if edit {
                    window.focus(&self.terminal_view.focus, cx);
                }
            }
            Event::TriggerButton(FUZZY_BUTTON, checked) => {
                let value = self.quick_inputs.get(quick_input).map(|input| input.input_box.value.clone()).unwrap_or_default();
                self.hide_quick_input_by_id(quick_input, window, cx);
                self.run_recent(kind, checked == Some(true), value, window, cx);
            }
            // `onDidTriggerSeparatorButton`: the header's Open File.
            Event::TriggerItemButton(SHELL_FILE_HEADER, _, _) => {
                if let Some(file) = self.terminal_view.history.shell_file.clone() {
                    self.hide_quick_input_by_id(quick_input, window, cx);
                    self.open_file(file, true, window, cx);
                }
            }
            Event::TriggerItemButton(handle, index, _) => {
                let Some(input) = self.quick_inputs.get(quick_input) else { return };
                let value = input.input_box.value.clone();
                let item = input.items().iter().find_map(|entry| match entry {
                    Entry::Item(item) if item.handle == handle => Some(item.clone()),
                    _ => None,
                });
                let Some(item) = item else { return };
                match item.buttons.get(index as usize).map(|button| button.handle) {
                    Some(PIN_BUTTON | PINNED_BUTTON) => {
                        self.toggle_recent_pin(instance, &item, item.buttons[index as usize].handle == PINNED_BUTTON);
                        self.hide_quick_input_by_id(quick_input, window, cx);
                        self.run_recent(kind, fuzzy, value, window, cx);
                    }
                    Some(REMOVE_BUTTON) if picks[handle as usize].previous_session => {
                        self.history(kind).remove(&picks[handle as usize].raw_label);
                        self.save_history(kind);
                        self.hide_quick_input_by_id(quick_input, window, cx);
                        self.run_recent(kind, fuzzy, value, window, cx);
                    }
                    Some(OUTPUT_BUTTON) => self.open_command_output(instance, picks[handle as usize].command, window, cx),
                    _ => {}
                }
            }
            Event::ChangeActive(active) => self.preview_recent_command(instance, active.first().and_then(|handle| picks[*handle as usize].command), cx),
            // `showWithPinnedItems`: the pinned items show only while the filter is empty.
            Event::ChangeValue(value) => {
                let (entries, _) = self.recent_entries(kind, instance, value.is_empty());
                if let Some(input) = self.quick_inputs.get_mut(quick_input) {
                    input.set_items(entries);
                }
                cx.notify();
            }
            Event::Hide => {
                self.restore_recent_scroll(cx);
                self.quick_inputs.remove(quick_input);
            }
            _ => {}
        }
    }

    /// The output button: the command's output in an editor (`TerminalOutputProvider`).
    fn open_command_output(&mut self, instance: u32, command: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = command.and_then(|index| self.terminal_view.instance(instance)?.commands.get(index).cloned()) else { return };
        let path = PathBuf::from(format!("{}... {}", command.command, crate::workbench::terminal_history::from_now(command.timestamp)));
        let document = TextDocument::from_text(TERMINAL_OUTPUT_SCHEME, &path, &command.output, "utf8".into(), None, &self.languages, &self.settings.lock().unwrap());
        self.editors.open(document, false);
        self.editors_changed(cx);
        let _ = window;
    }

    /// `onDidChangeActive`: the command's prompt and command line scrolled into view, the scroll kept to restore.
    fn preview_recent_command(&mut self, instance: u32, command: Option<usize>, cx: &mut Context<Self>) {
        let Some(terminal) = self.terminal_view.instance_mut(instance) else { return };
        let Some(command) = command.and_then(|index| terminal.commands.get(index)) else {
            self.restore_recent_scroll(cx);
            return;
        };
        let Some(line) = command.prompt_line.or(command.command_line) else { return };
        let saved = &mut self.terminal_view.history.saved_scroll;
        let terminal = self.terminal_view.instances.iter_mut().find(|terminal| terminal.id == instance).expect("found above");
        saved.get_or_insert((instance, terminal.term.grid().display_offset()));
        let history_size = terminal.term.history_size() as i64;
        let offset = (history_size - line).clamp(0, history_size);
        let delta = offset - terminal.term.grid().display_offset() as i64;
        terminal.term.scroll_display(Scroll::Delta(delta as i32));
        cx.notify();
    }

    fn restore_recent_scroll(&mut self, cx: &mut Context<Self>) {
        let Some((instance, offset)) = self.terminal_view.history.saved_scroll.take() else { return };
        if let Some(terminal) = self.terminal_view.instance_mut(instance) {
            let delta = offset as i64 - terminal.term.grid().display_offset() as i64;
            terminal.term.scroll_display(Scroll::Delta(delta as i32));
        }
        cx.notify();
    }
}

fn keys(kind: RecentKind) -> (&'static str, &'static str) {
    match kind {
        RecentKind::Command => (history::ENTRIES_KEY_COMMANDS, history::TIMESTAMP_KEY_COMMANDS),
        RecentKind::Cwd => (history::ENTRIES_KEY_DIRS, history::TIMESTAMP_KEY_DIRS),
    }
}

fn button(handle: i64, codicon: &str, tooltip: &str) -> Button {
    Button { handle, codicon: Some(codicon.into()), tooltip: Some(tooltip.into()), ..Button::default() }
}

/// A labelled section of picks, oldest last as `unshift` builds it upstream.
fn push_section(entries: &mut Vec<Entry>, picks: &mut Vec<RecentPick>, separator: Entry, section: Vec<(String, Vec<Button>, bool)>) {
    if section.is_empty() {
        return;
    }
    entries.push(separator);
    for (raw_label, buttons, previous_session) in section {
        entries.push(Entry::Item(Item { handle: picks.len() as i64, label: format_label(&raw_label), buttons, ..Item::default() }));
        picks.push(RecentPick { raw_label, command: None, previous_session });
    }
}

/// A separator with `buttons` set, which upstream uses to "force full sized separators".
fn header(label: &str, description: Option<&str>, buttons: Vec<Button>) -> Header {
    Header { handle: 0, label: label.into(), description: description.map(str::to_owned), buttons }
}

/// quickPickPin.ts `getItemIdentifier` of an item without `id`: label, description and detail as a template
/// string joins them, `undefined` included.
/// shortcut: session commands carry an `id` upstream, so their pins never match again after the picker closes;
/// here they match by label like every other item.
fn pin_identifier(label: &str, description: Option<&str>, detail: Option<&str>) -> String {
    format!("{label}{}{}", description.unwrap_or("undefined"), detail.unwrap_or("undefined"))
}

fn item_identifier(item: &Item) -> String {
    pin_identifier(&item.label, item.description.as_deref(), item.detail.as_deref())
}

/// `formatLabel`: line breaks as ⏎, runs of three or more spaces as ⋯.
pub(super) fn format_label(label: &str) -> String {
    let label = label.replace("\r\n", "\u{23CE}").replace('\n', "\u{23CE}");
    regex::Regex::new(r"\s\s\s+").unwrap().replace_all(&label, "\u{22EF}").into_owned()
}

/// labels.ts `collapseTildePath` with `/`.
pub(super) fn collapse_tilde_path(path: Option<&str>, home: &str) -> String {
    let Some(path) = path else { return String::new() };
    if home.is_empty() {
        return path.to_owned();
    }
    let home = home.trim_end_matches('/');
    match path.strip_prefix(home) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

/// terminalInstance.ts `preparePathForShell` for posix shells: quotes a path with special characters.
pub(super) fn prepare_path_for_shell(path: &str) -> String {
    if path.contains(['\\', '\'', '"', ' ', '$', '`', '(', ')', '&', ';', '|', '<', '>', '*', '?', '!', '#', '~']) {
        format!("'{}'", path.replace('\'', "\\'"))
    } else {
        path.to_owned()
    }
}

/// date.ts `fromNow(timestamp, true)` for the output editor's title.
pub(super) fn from_now(timestamp: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as u64);
    let seconds = now.saturating_sub(timestamp) / 1000;
    match seconds {
        0..30 => "now".into(),
        30..60 => format!("{seconds} secs ago"),
        60..3600 => format!("{} mins ago", seconds / 60),
        3600..86400 => format!("{} hrs ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_show_line_breaks_and_long_space_runs() {
        assert_eq!(format_label("echo a\nb   c"), "echo a\u{23CE}b\u{22EF}c");
    }

    #[test]
    fn cwds_under_home_are_tilde_collapsed() {
        assert_eq!(collapse_tilde_path(Some("/Users/a/src"), "/Users/a"), "~/src");
        assert_eq!(collapse_tilde_path(Some("/Users/ab"), "/Users/a"), "/Users/ab");
        assert_eq!(collapse_tilde_path(None, "/Users/a"), "");
    }
}
