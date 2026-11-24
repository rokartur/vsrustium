//! Quick access (platform/quickinput/browser/quickAccess.ts): one quick pick whose value prefix picks the
//! provider. `>` is the Command Palette (commandsQuickAccess.ts over pickerQuickAccess.ts).

use gpui::{Context, PromptLevel, Window};
use serde_json::Value;

use super::Workbench;
use crate::command_palette::{self, CommandsHistory, MenuItem, PaletteEntry};
use crate::commands::Outcome;
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Button, Entry, Event, Item, Kind, QuickInput};

const COMMANDS_PREFIX: &str = ">";

/// The providers' state that outlives a picker.
pub struct QuickAccess {
    /// `MenuId.CommandPalette` of the workbench and the extensions.
    pub menu: Vec<MenuItem>,
    /// shortcut: kept for the session, upstream stores it under `workbench.commandPalette.history`; the port has no storage service until Milestone 10.
    history: CommandsHistory,
}

impl Default for QuickAccess {
    fn default() -> Self {
        QuickAccess { menu: command_palette::menu(&[]), history: CommandsHistory::new(command_palette::DEFAULT_HISTORY_LENGTH) }
    }
}

/// What accepting a pick of the shown picker does, by item handle.
#[derive(Clone, Debug)]
pub enum Accept {
    Command { id: String, label: String, recently_used: bool },
    Nothing,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Provider {
    Commands,
    None,
}

impl Provider {
    /// `QuickAccessRegistry.getQuickAccessProvider`: the provider whose prefix starts the value.
    fn for_value(value: &str) -> Provider {
        if value.starts_with(COMMANDS_PREFIX) { Provider::Commands } else { Provider::None }
    }

    fn prefix(self) -> &'static str {
        match self {
            Provider::Commands => COMMANDS_PREFIX,
            Provider::None => "",
        }
    }

    fn placeholder(self) -> Option<String> {
        match self {
            Provider::Commands => Some("Type the name of a command to run.".into()),
            Provider::None => None,
        }
    }
}

impl Workbench {
    pub(super) fn register_quick_access_commands(&mut self) {
        self.commands.register("workbench.action.showCommands", |workbench, _, window, cx| {
            workbench.show_quick_access(COMMANDS_PREFIX, false, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.clearCommandHistory", |workbench, _, window, cx| workbench.clear_command_history(window, cx));
    }

    /// `QuickAccessController.show`: reuses a visible picker of the same provider, else fills a new one.
    fn show_quick_access(&mut self, value: &str, preserve_value: bool, window: &mut Window, cx: &mut Context<Self>) {
        let provider = Provider::for_value(value);
        let mut value = value.to_owned();
        let visible = self.quick_inputs.current().filter(|input| matches!(self.quick_inputs.owner(input.id), Some(Owner::QuickAccess(_)))).map(|input| input.id);
        if let Some(id) = visible {
            let input = self.quick_inputs.get_mut(id).expect("the visible input");
            if Provider::for_value(&input.value) == provider {
                if value != provider.prefix() && !preserve_value {
                    input.set_value(value);
                }
                adjust_value_selection(input, provider, preserve_value);
                self.flush_quick_input_events(window, cx);
                return;
            }
            // Another provider's filter carries over under the new prefix.
            let filter = &input.value[Provider::for_value(&input.value).prefix().len()..];
            if !preserve_value && !filter.is_empty() {
                value = format!("{}{filter}", provider.prefix());
            }
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.value = value;
        adjust_value_selection(&mut input, provider, preserve_value);
        input.placeholder = provider.placeholder();
        // pickerQuickAccess.ts: the provider filters and sorts, the list does not.
        input.match_on_label = false;
        input.match_on_description = false;
        input.match_on_detail = false;
        input.sort_by_label = false;
        // Showing it hides the visible picker, whose Hide event then drops it.
        let id = self.quick_inputs.add(input, Owner::QuickAccess(Vec::new()));
        self.show_quick_input(id, window, cx);
        self.update_quick_access_picks(id, window);
        self.flush_quick_input_events(window, cx);
    }

    /// `updatePickerItems`: the provider of the value fills the picker.
    fn update_quick_access_picks(&mut self, id: i64, window: &Window) {
        let Some(input) = self.quick_inputs.get(id) else { return };
        let provider = Provider::for_value(&input.value);
        let filter = input.value[provider.prefix().len()..].trim().to_owned();
        let (entries, accepts) = match provider {
            Provider::Commands => self.command_picks(&filter, window),
            Provider::None => (Vec::new(), Vec::new()),
        };
        let input = self.quick_inputs.get_mut(id).expect("checked above");
        input.placeholder = provider.placeholder();
        input.set_items(entries);
        if let Some(Owner::QuickAccess(picks)) = self.quick_inputs.owner_mut(id) {
            *picks = accepts;
        }
    }

    /// `AbstractCommandsQuickAccessProvider._getPicks` and `toCommandPick`, with "No matching commands" for an empty result.
    fn command_picks(&self, filter: &str, window: &Window) -> (Vec<Entry>, Vec<Accept>) {
        let context = self.key_context(window);
        let commands = command_palette::enabled_commands(&self.quick_access.menu, &context);
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
            accepts.push(Accept::Command { id: pick.id, label: pick.label, recently_used: pick.recently_used });
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
            Event::ChangeValue(_) => self.update_quick_access_picks(id, window),
            Event::Accept => {
                let Some(input) = self.quick_inputs.get(id) else { return };
                let Some(&handle) = input.selected_handles().first() else { return };
                let Some(Owner::QuickAccess(picks)) = self.quick_inputs.owner(id) else { return };
                let accept = picks[handle as usize].clone();
                // The "No matching commands" pick has no `accept`: the picker stays.
                if matches!(accept, Accept::Nothing) {
                    return;
                }
                self.hide_quick_input_by_id(id, window, cx);
                self.accept_quick_access_pick(accept, window, cx);
            }
            Event::TriggerItemButton(handle, button, _) => {
                let Some(Owner::QuickAccess(picks)) = self.quick_inputs.owner(id) else { return };
                let Accept::Command { id: command, .. } = picks[handle as usize].clone() else { return };
                if button == 0 {
                    // shortcut: opens keybindings.json where upstream opens the Keybindings editor at the command; the port has no Keybindings editor yet.
                    self.hide_quick_input_by_id(id, window, cx);
                    let outcome = self.execute_command("workbench.action.openGlobalKeybindingsFile", Vec::new(), window, cx);
                    show_failure("Open Keyboard Shortcuts (JSON)".into(), outcome, window, cx);
                    return;
                }
                self.quick_access.history.remove(&command);
                if let Some(input) = self.quick_inputs.get_mut(id) {
                    input.remove_item(handle);
                }
            }
            Event::Hide => self.quick_inputs.remove(id),
            Event::ChangeActive(_) | Event::ChangeSelection(_) | Event::TriggerButton(..) => {}
        }
    }

    /// The command pick's `accept`: remembered in the history, then run; an error shows in a dialog.
    fn accept_quick_access_pick(&mut self, accept: Accept, window: &mut Window, cx: &mut Context<Self>) {
        let Accept::Command { id, label, .. } = accept else { return };
        self.quick_access.history.push(&id);
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
                let _ = workbench.update(cx, |workbench, _| workbench.quick_access.history.clear());
            }
        })
        .detach();
        Outcome::done()
    }
}

/// `adjustValueSelection`: the caret at the end when the value is kept, else everything after the prefix selected.
fn adjust_value_selection(input: &mut QuickInput, provider: Provider, preserve_value: bool) {
    let end = input.value.chars().count();
    let start = if preserve_value { end } else { provider.prefix().chars().count() };
    input.set_value_selection(Some((start, end)));
}

/// Shows the error the command ends with, now or once it finishes.
fn show_failure(label: String, outcome: Outcome, window: &mut Window, cx: &mut Context<Workbench>) {
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
    let _ = window.prompt(PromptLevel::Critical, &message, Some(&detail), &["OK"], cx);
}

#[cfg(test)]
mod tests;
