//! listWidget.ts `TypeNavigationController` in the lists that give a `keyboardNavigationLabelProvider`: the Explorer
//! and the Keyboard Shortcuts table. Typed characters move the focus to the next row whose label starts with them,
//! for 800ms after the last one. The Search results tree gives none, so typing there does nothing.

use std::time::Duration;

use gpui::{Context, Keystroke, Task, Window};

use super::Workbench;
use crate::explorer::Row;

#[cfg(test)]
mod tests;

const CLEAR_DELAY: Duration = Duration::from_millis(800);

/// The word being typed; one for the workbench, since only the focused list takes keys.
#[derive(Default)]
pub struct TypeNavigation {
    /// The characters typed since the last pause; empty while idle.
    word: String,
    /// `list.triggerTypeNavigation` ran, for `workbench.list.typeNavigationMode` `trigger`.
    triggered: bool,
    clear: Option<Task<()>>,
}

impl Workbench {
    /// The `onChar` chain: an unmodified printable key while a list with type navigation has the focus, and while
    /// not typing yet only one no keybinding takes (`createKeyboardNavigationEventFilter`). Returns whether it navigated.
    pub(super) fn list_type_navigation(&mut self, keystroke: &Keystroke, unbound: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let explorer = self.explorer.focus.is_focused(window) && self.folder.is_some();
        let table = self.keybindings_editor.table_focus.is_focused(window);
        if !explorer && !table {
            return false;
        }
        let modifiers = &keystroke.modifiers;
        // keybindingService.ts `mightProducePrintableCharacter`: no Ctrl, Cmd or Alt.
        if modifiers.control || modifiers.platform || modifiers.alt {
            return false;
        }
        let Some(character) = keystroke.key_char.as_deref().filter(|text| text.chars().count() == 1 && !text.chars().any(char::is_control)) else { return false };
        let automatic = self.setting("workbench.list.typeNavigationMode") != "trigger";
        let navigation = &mut self.explorer.type_navigation;
        let idle = navigation.word.is_empty();
        if !(automatic || navigation.triggered) || !(unbound || !idle) {
            return false;
        }
        navigation.word.push_str(character);
        let word = navigation.word.clone();
        navigation.clear = Some(cx.spawn_in(window, async |workbench, cx| {
            cx.background_executor().timer(CLEAR_DELAY).await;
            let _ = workbench.update(cx, |workbench, _| {
                let navigation = &mut workbench.explorer.type_navigation;
                navigation.word.clear();
                navigation.triggered = false;
            });
        }));
        if table {
            let (labels, focus) = self.keybindings_table_navigation_labels();
            if let Some(index) = type_navigation_target(&labels, focus, &word, idle) {
                self.focus_keybinding_entry(index, window, cx);
            }
            return true;
        }
        let tree = self.folder.as_mut().expect("checked above");
        let rows = tree.rows();
        let focus = tree.focused.as_ref().and_then(|focused| rows.iter().position(|row| row.path == *focused));
        let labels: Vec<String> = rows.iter().map(explorer_label).collect();
        if let Some(index) = type_navigation_target(&labels, focus, &word, idle) {
            tree.focused = Some(rows[index].path.clone());
            self.explorer.scroll.scroll_to_item(index);
            cx.notify();
        }
        true
    }

    pub(super) fn register_type_navigation_commands(&mut self) {
        // listCommands.ts, with its `list.toggleKeyboardNavigation` alias.
        for id in ["list.triggerTypeNavigation", "list.toggleKeyboardNavigation"] {
            self.commands.register(id, |workbench, _, _, _| {
                let navigation = &mut workbench.explorer.type_navigation;
                navigation.triggered = !navigation.triggered;
                crate::commands::Outcome::done()
            });
        }
    }
}

/// explorerView.ts `keyboardNavigationLabelProvider`: the name, a compact row's folders joined by `/`.
fn explorer_label(row: &Row) -> String {
    match row.compressed.is_empty() {
        true => row.name.clone(),
        false => row.compressed.iter().map(|(_, name)| name.as_str()).collect::<Vec<_>>().join("/"),
    }
}

/// `onInput` without `typeNavigationEnabled`: from the focused row (the next one when a new word starts), the first
/// row whose label starts with `word`, ignoring case.
fn type_navigation_target(labels: &[String], focus: Option<usize>, word: &str, idle: bool) -> Option<usize> {
    let start = focus.unwrap_or(0);
    let delta = usize::from(idle);
    let word = word.to_lowercase();
    for offset in 0..labels.len() {
        let index = (start + offset + delta) % labels.len();
        if labels[index].to_lowercase().starts_with(&word) {
            return Some(index);
        }
    }
    None
}
