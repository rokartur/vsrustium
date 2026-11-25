//! Quick access (platform/quickinput/browser/quickAccess.ts): one quick pick whose value prefix picks the
//! provider. `>` is the Command Palette (commandsQuickAccess.ts over pickerQuickAccess.ts), no prefix Quick Open
//! (anythingQuickAccess.ts), `:` and `::` Go to Line/Column and Go to Offset (gotoLineQuickAccess.ts), `?` the
//! providers (helpQuickAccess.ts).

use config::configuration::Overrides;
use gpui::{Context, PromptLevel, Window};
use serde_json::Value;

use super::Workbench;
use super::quick_open::{self, FilePickState};
use crate::command_palette::{self, CommandsHistory, MenuItem, PaletteEntry};
use crate::commands::Outcome;
use crate::editor::Position;
use crate::editor_history::EditorHistory;
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Button, Entry, Event, Item, Kind, QuickInput};

const COMMANDS_PREFIX: &str = ">";
const HELP_PREFIX: &str = "?";
/// `CommandsHistory.PREF_KEY_CACHE` and `PREF_KEY_COUNTER`, in StorageScope.PROFILE.
const HISTORY_CACHE_KEY: &str = "commandPalette.mru.cache";
const HISTORY_COUNTER_KEY: &str = "commandPalette.mru.counter";

/// The providers' state that outlives a picker.
pub struct QuickAccess {
    /// `MenuId.CommandPalette` of the workbench and the extensions.
    pub menu: Vec<MenuItem>,
    history: CommandsHistory,
    pub editor_history: EditorHistory,
    /// The state of the shown Quick Open picker.
    pub files: Option<FilePickState>,
    /// The picker opened with `includeHelp` (the command center's), whose empty filter lists the help picks.
    help_input: Option<i64>,
    /// The editor the shown Go to Line or Go to Symbol picker previews in.
    pub goto_line: Option<goto_line::GotoLineState>,
    pub symbols: Option<goto_symbol::SymbolPickState>,
}

impl Default for QuickAccess {
    fn default() -> Self {
        QuickAccess { menu: command_palette::menu(&[]), history: CommandsHistory::new(command_palette::DEFAULT_HISTORY_LENGTH), editor_history: EditorHistory::default(), files: None, help_input: None, goto_line: None, symbols: None }
    }
}

/// What accepting a pick of the shown picker does, by item handle.
#[derive(Clone, Debug)]
pub enum Accept {
    Command { id: String, label: String },
    File { path: std::path::PathBuf },
    /// A help pick: shows the provider of `prefix`.
    Show { prefix: &'static str, include_help: bool },
    Position(Position),
    Symbol(goto_symbol::SymbolTarget),
    Nothing,
}

/// `getHelpPicks`: the help entries with a `commandCenterOrder`, by that order, of the providers the port has
/// (Search for Text, Go to Symbol, Start Debugging and Run Task wait for theirs), with `commandCenterLabel`.
const HELP_ENTRIES: [(&str, Provider, Option<&str>); 4] = [
    ("Go to File", Provider::Files, Some("workbench.action.quickOpen")),
    ("Show and Run Commands", Provider::Commands, Some("workbench.action.showCommands")),
    ("Go to Symbol in Editor", Provider::Symbols, Some("workbench.action.gotoSymbol")),
    ("More", Provider::Help, None),
];

/// The `helpEntries` of each provider the port has, as the `?` picker lists them: prefix, description, command.
const PROVIDER_HELP: [(&str, &str, Option<&str>); 6] = [
    ("", "Go to File", Some("workbench.action.quickOpen")),
    (goto_line::GO_TO_LINE_PREFIX, "Go to Line/Column", Some("workbench.action.gotoLine")),
    (goto_line::GO_TO_OFFSET_PREFIX, "Go to Offset", Some("workbench.action.gotoOffset")),
    (COMMANDS_PREFIX, "Show and Run Commands", Some("workbench.action.showCommands")),
    (goto_symbol::PREFIX, "Go to Symbol in Editor", Some("workbench.action.gotoSymbol")),
    (goto_symbol::PREFIX_BY_CATEGORY, "Go to Symbol in Editor by Category", None),
];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Provider {
    Commands,
    Files,
    GotoLine,
    GotoOffset,
    Help,
    Symbols,
}

impl Provider {
    /// `QuickAccessRegistry.getQuickAccessProvider`: the provider whose prefix starts the value.
    fn for_value(value: &str) -> Provider {
        // The registry tries the longest prefix first.
        match value {
            _ if value.starts_with(goto_line::GO_TO_OFFSET_PREFIX) => Provider::GotoOffset,
            _ if value.starts_with(goto_line::GO_TO_LINE_PREFIX) => Provider::GotoLine,
            _ if value.starts_with(COMMANDS_PREFIX) => Provider::Commands,
            _ if value.starts_with(HELP_PREFIX) => Provider::Help,
            _ if value.starts_with(goto_symbol::PREFIX) => Provider::Symbols,
            _ => Provider::Files,
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Provider::Commands => COMMANDS_PREFIX,
            Provider::Files => "",
            Provider::GotoLine => goto_line::GO_TO_LINE_PREFIX,
            Provider::GotoOffset => goto_line::GO_TO_OFFSET_PREFIX,
            Provider::Help => HELP_PREFIX,
            Provider::Symbols => goto_symbol::PREFIX,
        }
    }

    fn placeholder(self) -> Option<String> {
        match self {
            Provider::Commands => Some("Type the name of a command to run.".into()),
            Provider::Files => Some(quick_open::PLACEHOLDER.into()),
            Provider::GotoLine | Provider::GotoOffset => Some(goto_line::PLACEHOLDER.into()),
            Provider::Help => Some("Type '?' to get help on the actions you can take from here.".into()),
            Provider::Symbols => Some(goto_symbol::PLACEHOLDER.into()),
        }
    }
}

impl Workbench {
    pub(super) fn register_quick_access_commands(&mut self) {
        self.commands.register("workbench.action.showCommands", |workbench, _, window, cx| {
            workbench.show_quick_access(COMMANDS_PREFIX, false, false, window, cx);
            Outcome::done()
        });
        // quickAccessActions.ts: a string argument is the value to show, kept as given.
        self.commands.register("workbench.action.quickOpen", |workbench, args, window, cx| {
            match args.first().and_then(Value::as_str) {
                Some(prefix) => workbench.show_quick_access(prefix, true, false, window, cx),
                None => workbench.show_quick_access("", false, false, window, cx),
            }
            Outcome::done()
        });
        // `QuickAccessAction` with AI features off: Quick Open with the help picks, the command center's click.
        self.commands.register("workbench.action.quickOpenWithModes", |workbench, _, window, cx| {
            workbench.show_quick_access("", true, true, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.gotoLine", |workbench, _, window, cx| {
            workbench.show_quick_access(goto_line::GO_TO_LINE_PREFIX, false, false, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.gotoSymbol", |workbench, _, window, cx| {
            workbench.show_quick_access(goto_symbol::PREFIX, false, false, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.gotoOffset", |workbench, _, window, cx| {
            workbench.show_quick_access(goto_line::GO_TO_OFFSET_PREFIX, false, false, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.clearCommandHistory", |workbench, _, window, cx| workbench.clear_command_history(window, cx));
    }

    /// `CommandsHistory.load` from the profile storage.
    pub(super) fn load_command_history(&mut self) {
        let limit = self.configured_command_history_length();
        let storage = self.profile_storage.lock().unwrap();
        self.quick_access.history = CommandsHistory::load(storage.get(HISTORY_CACHE_KEY), storage.get(HISTORY_COUNTER_KEY), limit);
    }

    /// `CommandsHistory.saveState`. Upstream saves on shutdown only; the port saves on each change, since its
    /// storage writes synchronously and the app can exit without a shutdown phase.
    fn save_command_history(&self) {
        let (cache, counter) = self.quick_access.history.serialize();
        let mut storage = self.profile_storage.lock().unwrap();
        storage.set(HISTORY_CACHE_KEY, cache);
        storage.set(HISTORY_COUNTER_KEY, counter);
    }

    /// `getConfiguredCommandHistoryLength`: `workbench.commandPalette.history`, 0 keeps no history.
    fn configured_command_history_length(&self) -> usize {
        let value = self.settings.lock().unwrap().get_value(Some("workbench.commandPalette.history"), &Overrides::default());
        value.and_then(|value| value.as_u64()).map_or(command_palette::DEFAULT_HISTORY_LENGTH, |length| length as usize)
    }

    /// `QuickAccessController.show`: reuses a visible picker of the same provider, else fills a new one.
    fn show_quick_access(&mut self, value: &str, preserve_value: bool, include_help: bool, window: &mut Window, cx: &mut Context<Self>) {
        // `updateConfiguration`: the configured length applies from the next pick list on.
        let limit = self.configured_command_history_length();
        self.quick_access.history.set_limit(limit);
        let provider = Provider::for_value(value);
        let mut value = value.to_owned();
        let visible = self.quick_inputs.current().filter(|input| matches!(self.quick_inputs.owner(input.id), Some(Owner::QuickAccess(_)))).map(|input| input.id);
        if let Some(id) = visible {
            let input = self.quick_inputs.get_mut(id).expect("the visible input");
            if Provider::for_value(&input.input_box.value) == provider {
                if value != provider.prefix() && !preserve_value {
                    input.set_value(value);
                }
                adjust_value_selection(input, provider, preserve_value);
                self.flush_quick_input_events(window, cx);
                return;
            }
            // Another provider's filter carries over under the new prefix.
            let filter = &input.input_box.value[Provider::for_value(&input.input_box.value).prefix().len()..];
            if !preserve_value && !filter.is_empty() {
                value = format!("{}{filter}", provider.prefix());
            }
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.input_box.value = value;
        adjust_value_selection(&mut input, provider, preserve_value);
        input.placeholder = provider.placeholder();
        // pickerQuickAccess.ts: the provider filters and sorts, the list does not.
        input.match_on_label = false;
        input.match_on_description = false;
        input.match_on_detail = false;
        input.sort_by_label = false;
        // Showing it hides the visible picker, whose Hide event then drops it.
        let id = self.quick_inputs.add(input, Owner::QuickAccess(Vec::new()));
        self.quick_access.help_input = include_help.then_some(id);
        self.show_quick_input(id, window, cx);
        self.update_quick_access_picks(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    /// `updatePickerItems`: the provider of the value fills the picker.
    pub(super) fn update_quick_access_picks(&mut self, id: i64, window: &Window, cx: &mut Context<Self>) {
        let Some(input) = self.quick_inputs.get(id) else { return };
        let value = input.input_box.value.clone();
        let provider = Provider::for_value(&value);
        let filter = value[provider.prefix().len()..].trim().to_owned();
        if !matches!(provider, Provider::GotoLine | Provider::GotoOffset | Provider::Symbols) {
            self.dispose_goto_line(cx);
        }
        if provider != Provider::Symbols {
            self.quick_access.symbols = None;
        }
        let picks = match provider {
            // helpQuickAccess.ts: the exact prefix of another provider after `?` shows that provider.
            Provider::Help if Provider::for_value(&filter) != Provider::Files => {
                let prefix = Provider::for_value(&filter).prefix();
                cx.defer_in(window, move |workbench, window, cx| workbench.show_quick_access(prefix, true, false, window, cx));
                None
            }
            Provider::Help => {
                let (entries, accepts) = self.provider_help_picks(&filter, window);
                Some((entries, accepts, false))
            }
            Provider::Commands => {
                let (entries, accepts) = self.command_picks(&filter, window);
                Some((entries, accepts, false))
            }
            Provider::Files => self.file_picks(id, &filter, window, cx),
            Provider::Symbols => self.symbol_picks(id, &value, window, cx),
            Provider::GotoLine | Provider::GotoOffset => Some((Vec::new(), self.goto_line_picks(id, &value, cx), false)),
        };
        let input = self.quick_inputs.get_mut(id).expect("checked above");
        input.placeholder = provider.placeholder();
        input.can_accept_in_background = provider != Provider::Commands;
        let Some((entries, accepts, busy)) = picks else { return };
        input.busy = busy;
        // The Go to Line provider sets its one pick and its buttons itself.
        if !matches!(provider, Provider::GotoLine | Provider::GotoOffset) {
            input.set_items(entries);
        }
        if let Some(Owner::QuickAccess(picks)) = self.quick_inputs.owner_mut(id) {
            *picks = accepts;
        }
    }

    /// `getHelpPicks` for the picker `id`: each entry with its command's keybinding; the Go to File pick shows Quick
    /// Open without them.
    pub(super) fn help_picks(&self, id: i64, entries: &mut Vec<Entry>, accepts: &mut Vec<Accept>, window: &Window) {
        if self.quick_access.help_input != Some(id) {
            return;
        }
        let context = self.key_context(window);
        for (label, provider, command) in HELP_ENTRIES {
            let keybinding = command.and_then(|command| self.keybindings.resolver.lookup_primary_keybinding(command, &context));
            entries.push(Entry::Item(Item {
                handle: accepts.len() as i64,
                label: label.into(),
                description: Some(provider.prefix().into()).filter(|prefix: &String| !prefix.is_empty()),
                pickable: true,
                keybinding: keybinding.map(|item| item.keybinding.iter().map(|chord| chord.key_caps()).collect()).unwrap_or_default(),
                ..Item::default()
            }));
            accepts.push(Accept::Show { prefix: provider.prefix(), include_help: provider != Provider::Files });
        }
    }

    /// `HelpQuickAccessProvider.getQuickAccessProviders`: the help entries by prefix, each labelled with its prefix
    /// (`…` for Quick Open's empty one) and showing that provider on accept. The list filters on the label.
    fn provider_help_picks(&self, filter: &str, window: &Window) -> (Vec<Entry>, Vec<Accept>) {
        let context = self.key_context(window);
        let mut providers = PROVIDER_HELP.to_vec();
        providers.sort_by(|a, b| command_palette::locale_compare(a.0, b.0));
        let (mut entries, mut accepts) = (Vec::new(), Vec::new());
        for (prefix, description, command) in providers {
            let label = if prefix.is_empty() { "\u{2026}" } else { prefix };
            if !label.contains(filter) {
                continue;
            }
            let keybinding = command.and_then(|command| self.keybindings.resolver.lookup_primary_keybinding(command, &context));
            entries.push(Entry::Item(Item {
                handle: accepts.len() as i64,
                label: label.into(),
                description: Some(description.into()),
                pickable: true,
                keybinding: keybinding.map(|item| item.keybinding.iter().map(|chord| chord.key_caps()).collect()).unwrap_or_default(),
                ..Item::default()
            }));
            accepts.push(Accept::Show { prefix, include_help: false });
        }
        (entries, accepts)
    }

    /// `AbstractCommandsQuickAccessProvider._getPicks` and `toCommandPick`, with "No matching commands" for an empty result.
    fn command_picks(&self, filter: &str, window: &Window) -> (Vec<Entry>, Vec<Accept>) {
        let context = self.key_context(window);
        let mut commands = match self.editors.active() {
            Some(_) => command_palette::editor_action_picks(&context),
            None => Vec::new(),
        };
        commands.extend(command_palette::enabled_commands(&self.quick_access.menu, &context, &self.commands));
        let mut entries = Vec::new();
        let mut accepts = Vec::new();
        for entry in command_palette::command_picks(&commands, filter, &self.quick_access.history) {
            let pick = match entry {
                PaletteEntry::Separator(label) => {
                    entries.push(Entry::Separator(Some(label.to_owned())));
                    continue;
                }
                PaletteEntry::Pick(pick) => pick,
            };
            let keybinding = self.keybindings.resolver.lookup_primary_keybinding(&pick.id, &context);
            let mut buttons = vec![Button { handle: 0, codicon: Some("gear".into()), tooltip: Some("Configure Keybinding".into()), ..Button::default() }];
            if pick.recently_used {
                buttons.push(Button { handle: 1, codicon: Some("close".into()), tooltip: Some("Remove from Recently Used".into()), ..Button::default() });
            }
            entries.push(Entry::Item(Item {
                handle: accepts.len() as i64,
                label: pick.label.clone(),
                description: pick.description,
                pickable: true,
                buttons,
                keybinding: keybinding.map(|item| item.keybinding.iter().map(|chord| chord.key_caps()).collect()).unwrap_or_default(),
                label_highlights: pick.label_highlights,
                ..Item::default()
            }));
            accepts.push(Accept::Command { id: pick.id, label: pick.label });
        }
        if accepts.is_empty() && !filter.is_empty() {
            entries = vec![Entry::Item(Item { handle: 0, label: "No matching commands".into(), pickable: true, ..Item::default() })];
            accepts = vec![Accept::Nothing];
        }
        (entries, accepts)
    }

    /// The picker's listeners: the provider follows the value, accept runs the pick, the close button forgets it.
    pub fn quick_access_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::ChangeValue(_) => self.update_quick_access_picks(id, window, cx),
            Event::Accept(in_background) => {
                let Some(input) = self.quick_inputs.get(id) else { return };
                let Some(&handle) = input.selected_handles().first() else { return };
                let Some(Owner::QuickAccess(picks)) = self.quick_inputs.owner(id) else { return };
                let accept = picks[handle as usize].clone();
                // The "No matching commands" pick has no `accept`: the picker stays.
                if matches!(accept, Accept::Nothing) {
                    return;
                }
                if !in_background {
                    self.hide_quick_input_by_id(id, window, cx);
                }
                self.accept_quick_access_pick(accept, in_background, window, cx);
            }
            Event::TriggerItemButton(handle, _, _) if matches!(self.quick_access_pick(id, handle), Some(Accept::File { .. })) => {
                // `TriggerAction.REMOVE_ITEM` of "Remove from Recently Opened".
                let Some(Accept::File { path, .. }) = self.quick_access_pick(id, handle) else { return };
                self.remove_from_editor_history(&path);
                if let Some(input) = self.quick_inputs.get_mut(id) {
                    input.remove_item(handle);
                }
            }
            Event::TriggerItemButton(handle, button, _) => {
                let Some(Accept::Command { id: command, .. }) = self.quick_access_pick(id, handle) else { return };
                if button == 0 {
                    // commandsQuickAccess.ts: the Keyboard Shortcuts editor at `createKeybindingCommandQuery(commandId, commandWhen)`.
                    self.hide_quick_input_by_id(id, window, cx);
                    let when = self.quick_access.menu.iter().find(|item| item.id == command).and_then(|item| item.precondition.as_ref()).map(|when| format!(" +when:{}", when.serialize()));
                    let query = format!("@command:{command}{}", when.unwrap_or_default());
                    let outcome = self.execute_command(super::keybindings_editor_view::OPEN_COMMAND, vec![Value::String(query)], window, cx);
                    show_failure("Open Keyboard Shortcuts".into(), outcome, window, cx);
                    return;
                }
                self.quick_access.history.remove(&command);
                self.save_command_history();
                if let Some(input) = self.quick_inputs.get_mut(id) {
                    input.remove_item(handle);
                }
            }
            Event::TriggerButton(goto_line::OFFSET_BUTTON, checked) if matches!(self.quick_inputs.get(id).map(|input| Provider::for_value(&input.input_box.value)), Some(Provider::GotoLine | Provider::GotoOffset)) => {
                self.toggle_zero_based_offset(checked);
                self.update_quick_access_picks(id, window, cx);
            }
            Event::Hide => {
                self.dispose_goto_line(cx);
                self.quick_access.symbols = None;
                self.quick_inputs.remove(id);
                if self.quick_access.files.as_ref().is_some_and(|state| state.input == id) {
                    self.quick_access.files = None;
                }
            }
            Event::ChangeActive(handles) => {
                if let Some(Accept::Symbol(target)) = handles.first().and_then(|handle| self.quick_access_pick(id, *handle)) {
                    self.preview_symbol(&target, cx);
                }
            }
            Event::ChangeSelection(_) | Event::TriggerButton(..) => {}
        }
    }

    fn quick_access_pick(&self, id: i64, handle: i64) -> Option<Accept> {
        let Some(Owner::QuickAccess(picks)) = self.quick_inputs.owner(id) else { return None };
        picks.get(handle as usize).cloned()
    }

    /// The command pick's `accept`: remembered in the history, then run; an error shows in a dialog.
    fn accept_quick_access_pick(&mut self, accept: Accept, in_background: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (id, label) = match accept {
            Accept::Command { id, label } => (id, label),
            Accept::File { path, .. } => return self.open_file_pick(path, in_background, window, cx),
            Accept::Show { prefix, include_help } => return self.show_quick_access(prefix, true, include_help, window, cx),
            Accept::Position(position) => return self.go_to_position(position, in_background, window, cx),
            Accept::Symbol(target) => return self.go_to_symbol(&target, in_background, window, cx),
            Accept::Nothing => return,
        };
        self.quick_access.history.push(&id);
        self.save_command_history();
        let outcome = self.execute_command(&id, Vec::new(), window, cx);
        show_failure(label, outcome, window, cx);
    }

    /// `ClearCommandHistoryAction`: asks first, then empties the history.
    fn clear_command_history(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let answer = window.prompt(
            PromptLevel::Warning,
            "Do you want to clear the history of recently used commands?",
            Some("This action is irreversible!"),
            &["Clear", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |workbench, cx| {
            if answer.await == Ok(0) {
                let _ = workbench.update(cx, |workbench, _| {
                    workbench.quick_access.history.clear();
                    workbench.save_command_history();
                });
            }
        })
        .detach();
        Outcome::done()
    }
}

/// The Quick Open picker's `contextKey`, `inFilesPicker`, holds while its provider fills it.
pub fn is_files_picker(value: &str) -> bool {
    Provider::for_value(value) == Provider::Files
}

/// `adjustValueSelection`: the caret at the end when the value is kept, else everything after the prefix selected.
fn adjust_value_selection(input: &mut QuickInput, provider: Provider, preserve_value: bool) {
    let end = input.input_box.value.chars().count();
    let start = if preserve_value { end } else { provider.prefix().chars().count() };
    input.set_value_selection(Some((start, end)));
}

/// Shows the error the command ends with, now or once it finishes.
pub(super) fn show_failure(label: String, outcome: Outcome, window: &mut Window, cx: &mut Context<Workbench>) {
    let pending = match outcome {
        Outcome::Ready(Ok(_)) => return,
        Outcome::Ready(Err(error)) => {
            show_command_error(&label, &error, window, cx);
            return;
        }
        Outcome::Pending(pending) => pending,
    };
    cx.spawn_in(window, async move |_, cx| {
        if let Ok(Err(error)) = pending.await {
            let _ = cx.update(|window, cx| show_command_error(&label, &error, window, cx));
        }
    })
    .detach();
}

/// `dialogService.error(localize('canNotRun', ...), toErrorMessage(error))`.
fn show_command_error(label: &str, error: &Value, window: &mut Window, cx: &mut gpui::App) {
    let message = format!("Command '{label}' resulted in an error");
    let detail = error["message"].as_str().unwrap_or_default().to_owned();
    // The only button closes the dialog, so the answer is not needed.
    drop(window.prompt(PromptLevel::Critical, &message, Some(&detail), &["OK"], cx));
}

mod goto_line;
mod goto_symbol;

#[cfg(test)]
mod tests;
