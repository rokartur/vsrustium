//! listWidget.ts `TypeNavigationController` in the Explorer: typed characters move the focus to the next row
//! whose label starts with them, for 800ms after the last one.

use std::time::Duration;

use gpui::{Context, Keystroke, Task, Window};

use super::Workbench;
use crate::explorer::Row;

#[cfg(test)]
mod tests;

const CLEAR_DELAY: Duration = Duration::from_millis(800);

#[derive(Default)]
pub struct TypeNavigation {
    /// The characters typed since the last pause; empty while idle.
    word: String,
    /// `list.triggerTypeNavigation` ran, for `workbench.list.typeNavigationMode` `trigger`.
    triggered: bool,
    clear: Option<Task<()>>,
}

impl Workbench {
    /// The `onChar` chain: an unmodified printable key while the tree has the focus, and while not typing yet only
    /// one no keybinding takes (`createKeyboardNavigationEventFilter`). Returns whether it navigated.
    pub(super) fn explorer_type_navigation(&mut self, keystroke: &Keystroke, unbound: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.explorer.focus.is_focused(window) || self.folder.is_none() {
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
        let tree = self.folder.as_mut().expect("checked above");
        let rows = tree.rows();
        let focus = tree.focused.as_ref().and_then(|focused| rows.iter().position(|row| row.path == *focused));
        if let Some(index) = type_navigation_target(&rows, focus, &word, idle) {
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

/// `onInput` without `typeNavigationEnabled`: from the focused row (the next one when a new word starts), the first
/// row whose label starts with `word`, ignoring case. A compact row's label is its folders joined by `/`.
fn type_navigation_target(rows: &[Row], focus: Option<usize>, word: &str, idle: bool) -> Option<usize> {
    let start = focus.unwrap_or(0);
    let delta = usize::from(idle);
    let word = word.to_lowercase();
    for offset in 0..rows.len() {
        let index = (start + offset + delta) % rows.len();
        let row = &rows[index];
        let label = match row.compressed.is_empty() {
            true => row.name.clone(),
            false => row.compressed.iter().map(|(_, name)| name.as_str()).collect::<Vec<_>>().join("/"),
        };
        if label.to_lowercase().starts_with(&word) {
            return Some(index);
        }
    }
    None
}
