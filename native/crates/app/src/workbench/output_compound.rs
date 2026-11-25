//! output.contribution.ts `registerAddCompoundLogAction` and `registerRemoveLogAction`: Add Compound Log... joins
//! log channels into one, Remove Output... removes the ones the user added.

use gpui::{Context, Window};

use super::Workbench;
use super::output_actions::{OutputPick, OutputPicks};
use crate::commands::Outcome;
use crate::main_thread_quick_open::Owner;
use crate::notifications::{Notification, Severity};
use crate::quick_input::{Entry, Item, Kind, QuickInput};

impl Workbench {
    pub(super) fn register_output_compound_actions(&mut self) {
        self.commands.register("workbench.action.output.addCompoundLog", |workbench, _, window, cx| {
            workbench.show_add_compound_log(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.output.remove", |workbench, _, window, cx| {
            workbench.show_remove_output(window, cx);
            Outcome::done()
        });
    }

    /// The log channels the user did not add, the workbench's by label, then "Extension Logs".
    fn show_add_compound_log(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (mut extension_logs, mut logs): (Vec<_>, Vec<_>) = self.output.channels.iter().filter(|channel| channel.log && !channel.user).partition(|channel| channel.extension_id.is_some());
        logs.sort_by(|a, b| crate::command_palette::locale_compare(&a.label, &b.label));
        extension_logs.sort_by(|a, b| crate::command_palette::locale_compare(&a.label, &b.label));
        let mut picks = OutputPicks::default();
        for channel in &logs {
            picks.push(Item { label: channel.label.clone(), ..Item::default() }, OutputPick::AddToCompound(channel.id.clone()));
        }
        if !extension_logs.is_empty() && !logs.is_empty() {
            picks.entries.push(Entry::Separator(Some("Extension Logs".into())));
        }
        for channel in &extension_logs {
            picks.push(Item { label: channel.label.clone(), ..Item::default() }, OutputPick::AddToCompound(channel.id.clone()));
        }
        self.show_many_output_picker(picks, window, cx);
    }

    fn show_remove_output(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut picks = OutputPicks::default();
        for channel in self.output.channels.iter().filter(|channel| channel.user) {
            picks.push(Item { label: channel.label.clone(), ..Item::default() }, OutputPick::Remove(channel.id.clone()));
        }
        if picks.picks.is_empty() {
            self.notify(Notification::new(Severity::Info, "No custom outputs to remove."));
            cx.notify();
            return;
        }
        self.show_many_output_picker(picks, window, cx);
    }

    fn show_many_output_picker(&mut self, picks: OutputPicks, window: &mut Window, cx: &mut Context<Self>) {
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select Log".into());
        input.can_select_many = true;
        input.set_items(picks.entries);
        let id = self.quick_inputs.add(input, Owner::Output(picks.picks));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    /// What a many-select output picker accepted: a compound log of the picked channels, shown, or the picked
    /// user channels removed.
    pub(super) fn accept_many_output_picks(&mut self, picks: Vec<OutputPick>, window: &mut Window, cx: &mut Context<Self>) {
        let mut compound = Vec::new();
        for pick in picks {
            match pick {
                OutputPick::AddToCompound(id) => compound.push(id),
                OutputPick::Remove(id) => drop(self.output.remove_channel(&id)),
                _ => {}
            }
        }
        if !compound.is_empty() {
            let id = self.output.register_compound_channel(&compound);
            self.output_channels_changed(window, cx);
            self.show_output_channel(&id, false, window, cx);
            return;
        }
        self.output_channels_changed(window, cx);
    }
}
