//! terminalQuickAccess.ts: `term ` lists the open terminals with Rename and Kill buttons, then Create New Terminal
//! and Create New Terminal With Profile.

use gpui::{Context, Window};

use super::Workbench;
use super::quick_access::Accept;
use crate::filters::{matches_fuzzy_icon_aware, parse_label_with_icons};
use crate::quick_input::{Button, Entry, Item};

/// `TerminalQuickAccessProvider.PREFIX`.
pub const TERMINAL_PREFIX: &str = "term ";
pub const PLACEHOLDER: &str = "Type the name of a terminal to open.";
const RENAME_BUTTON: i64 = 0;
const KILL_BUTTON: i64 = 1;

impl Workbench {
    /// `_getPicks`: the panel's terminals by group (`1`, or `1.2` in a split group), each `$(icon) index: title`.
    pub(super) fn terminal_picks(&self, filter: &str) -> (Vec<Entry>, Vec<Accept>) {
        let mut entries = vec![Entry::Separator(Some("panel".into()))];
        let mut accepts = Vec::new();
        for (group_index, group) in self.terminal_view.group_instances().iter().enumerate() {
            for (terminal_index, &id) in group.iter().enumerate() {
                let Some(instance) = self.terminal_view.instance(id) else { continue };
                let index = if group.len() > 1 { format!("{}.{}", group_index + 1, terminal_index + 1) } else { format!("{}", group_index + 1) };
                let icon = instance.launch.icon.clone().unwrap_or_else(|| "terminal".into());
                let label = format!("$({icon}) {index}: {}", instance.title());
                let Some(highlights) = matches_fuzzy_icon_aware(filter, &parse_label_with_icons(&label)) else { continue };
                let buttons = vec![button(RENAME_BUTTON, "edit", "Rename Terminal"), button(KILL_BUTTON, "trash", "Kill Terminal")];
                entries.push(Entry::Item(Item { handle: accepts.len() as i64, label, label_highlights: highlights, buttons, ..Item::default() }));
                accepts.push(Accept::Terminal(id));
            }
        }
        // No terminal editors in the port: the "editor" separator only shows after the panel's picks.
        if entries.len() > 1 {
            entries.push(Entry::Separator(Some("editor".into())));
            entries.push(Entry::Separator(None));
        }
        // Upstream writes `$(plus)`, an alias codicons ships only as add.svg.
        for (label, command) in [("Create New Terminal", "workbench.action.terminal.new"), ("Create New Terminal With Profile...", "workbench.action.terminal.newWithProfile")] {
            entries.push(Entry::Item(Item { handle: accepts.len() as i64, label: format!("$(add) {label}"), ..Item::default() }));
            accepts.push(Accept::Command { id: command.into(), label: label.into() });
        }
        (entries, accepts)
    }

    /// The pick's `accept`: `showPanel(!inBackground)` and the terminal made active.
    pub(super) fn accept_terminal_pick(&mut self, id: u32, in_background: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal_view.set_active(id);
        self.open_panel_view(super::terminal_view::TERMINAL_VIEW_ID, cx);
        if !in_background {
            window.focus(&self.terminal_view.focus, cx);
        }
        self.sync_terminals_to_host();
        cx.notify();
    }

    /// The pick's `trigger`: Rename runs `workbench.action.terminal.rename` for it, Kill disposes it and drops the pick.
    /// Returns whether the pick goes.
    pub(super) fn trigger_terminal_pick_button(&mut self, id: u32, button: i64, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match button {
            RENAME_BUTTON => {
                self.terminal_view.set_active(id);
                drop(self.execute_command("workbench.action.terminal.rename", Vec::new(), window, cx));
                false
            }
            KILL_BUTTON => {
                self.kill_terminal(id, window, cx);
                true
            }
            _ => false,
        }
    }
}

fn button(handle: i64, codicon: &str, tooltip: &str) -> Button {
    Button { handle, codicon: Some(codicon.into()), tooltip: Some(tooltip.into()), ..Button::default() }
}
