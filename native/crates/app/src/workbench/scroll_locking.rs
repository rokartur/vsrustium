//! scrollLocking.ts `SyncScroll`: Toggle and Hold Locked Scrolling Across Editors and the "Scrolling Locked" status
//! bar entry. The port has one editor group, so the visible panes are the active editor alone and locked scrolling
//! has no other pane to move; the panes follow once editor groups exist.

use serde_json::json;

use super::Workbench;
use crate::commands::Outcome;
use crate::main_thread_status_bar::{StatusEntry, set_or_update_entry};

const ENTRY_ID: &str = "status.scrollLockingEnabled";
const TOGGLE: &str = "workbench.action.toggleLockedScrolling";

impl Workbench {
    pub(super) fn register_scroll_locking_commands(&mut self) {
        self.commands.register(TOGGLE, |workbench, _, _, cx| {
            workbench.toggle_locked_scrolling();
            cx.notify();
            Outcome::done()
        });
        // shortcut: no keybinding hold mode, so Hold toggles like Toggle (its upstream path without a held key).
        self.commands.register("workbench.action.holdLockedScrolling", |workbench, _, _, cx| {
            workbench.toggle_locked_scrolling();
            cx.notify();
            Outcome::done()
        });
    }

    pub(super) fn locked_scrolling(&self) -> bool {
        self.status_entries.iter().any(|entry| entry.entry_id == ENTRY_ID)
    }

    /// `toggle` and `toggleStatusbarItem`: the prominent entry at priority 102 on the right while active.
    fn toggle_locked_scrolling(&mut self) {
        if self.locked_scrolling() {
            self.status_entries.retain(|entry| entry.entry_id != ENTRY_ID);
            return;
        }
        let entry = StatusEntry {
            entry_id: ENTRY_ID.into(),
            id: ENTRY_ID.into(),
            name: "Scrolling Locked".into(),
            text: "Scrolling Locked".into(),
            tooltip: Some("Lock Scrolling Enabled".into()),
            command: Some(json!(TOGGLE)),
            color: None,
            background_color: None,
            kind: Some("prominent"),
            align_left: false,
            priority: 102.,
            secondary: 0,
            show_progress: false,
        };
        set_or_update_entry(&mut self.status_entries, entry);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::host_sync::HostSync;
    use crate::languages::Languages;
    use crate::theme::load_theme;
    use crate::workbench::Workbench;

    #[gpui::test]
    fn toggling_adds_and_removes_the_status_entry(cx: &mut gpui::TestAppContext) {
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let (sender, _receiver) = std::sync::mpsc::channel();
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(sender), cx));
        workbench.update_in(cx, |workbench, window, cx| {
            drop(workbench.execute_command("workbench.action.toggleLockedScrolling", Vec::new(), window, cx));
            assert!(workbench.locked_scrolling());
            let entry = workbench.status_entries.iter().find(|entry| entry.entry_id == "status.scrollLockingEnabled").unwrap();
            assert_eq!((entry.text.as_str(), entry.kind, entry.align_left), ("Scrolling Locked", Some("prominent"), false));
            drop(workbench.execute_command("workbench.action.toggleLockedScrolling", Vec::new(), window, cx));
            assert!(!workbench.locked_scrolling());
        });
    }
}
