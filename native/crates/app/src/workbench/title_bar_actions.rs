//! titlebarActions.ts: the `ToggleTitleBarConfigAction`s, and the title bar's context menus with window.ts
//! `provideCustomTitleContextMenu`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::{Bounds, Context, Pixels, Point, Size, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::contextkey;
use crate::menus::{Entry, MenuAction, MenuId, MenuItem, TITLE_BAR_TOGGLES, menu_actions};

const REVEAL_PATH_COMMAND: &str = "workbench.action.revealPathInFinder";
/// The command center's one item, `CommandCenterCenter`.
const QUICK_OPEN_WITH_MODES: &str = "workbench.action.quickOpenWithModes";

impl Workbench {
    pub(super) fn register_title_bar_actions(&mut self) {
        // menuService.ts `createConfigureKeybindingAction` of the command center item, which has no `when`.
        self.commands.register(format!("configureKeybinding/{QUICK_OPEN_WITH_MODES}"), |workbench, _, window, cx| {
            workbench.execute_command(super::keybindings_editor_view::OPEN_COMMAND, vec![json!(format!("@command:{QUICK_OPEN_WITH_MODES}"))], window, cx)
        });
        for (setting, ..) in TITLE_BAR_TOGGLES {
            self.commands.register(format!("toggle.{setting}"), move |workbench, _, _, cx| {
                // `updateValue(section, !value)`
                let enabled = contextkey::truthy(Some(&workbench.setting(setting)));
                let mut settings = workbench.settings.lock().unwrap();
                match settings.update_value_derived(setting, Some(&json!(!enabled))) {
                    Ok(change) if !change.is_empty() => workbench.host.send_configuration_change(&settings, &change),
                    Ok(_) => {}
                    Err(error) => return Outcome::error(format!("writing {setting}: {error}")),
                }
                cx.notify();
                Outcome::done()
            });
        }
    }

    /// The title bar's `contextmenu` listener: `menu` is `TitleBarTitleContext` over the title, else `TitleBarContext`.
    pub(super) fn show_title_bar_context_menu(&mut self, menu: MenuId, pointer: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.editors.active().filter(|editor| editor.document.scheme == "file").map(|editor| editor.document.path.clone());
        self.provide_custom_title_context_menu(file.as_deref());
        let context = self.title_bar_context(window);
        self.show_context_menu(menu, &context, Bounds { origin: pointer, size: Size::default() }, Vec::new(), window, cx);
    }

    /// The command center toolbar's own `contextmenu` listener (toolbar.ts): the item's Configure Keybinding, then
    /// the toolbar's `contextMenu: MenuId.TitleBarContext` toggles.
    pub(super) fn show_command_center_context_menu(&mut self, pointer: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let context = self.title_bar_context(window);
        let configure_keybinding = MenuAction::Command { id: format!("configureKeybinding/{QUICK_OPEN_WITH_MODES}"), title: "Configure Keybinding".into(), enabled: true, checked: false, alt: None };
        let mut actions = vec![configure_keybinding, MenuAction::Separator];
        actions.extend(menu_actions(&self.menus, MenuId::TitleBarContext, &context));
        self.show_context_menu_actions(actions, &context, Bounds { origin: pointer, size: Size::default() }, Vec::new(), window, cx);
    }

    fn title_bar_context(&self, window: &Window) -> HashMap<String, Value> {
        let mut context = self.key_context(window);
        for (setting, ..) in TITLE_BAR_TOGGLES {
            context.insert(format!("config.{setting}"), self.setting(setting));
        }
        context
    }

    /// window.ts `provideCustomTitleContextMenu`: one item per segment of the active file's path, from the file up to
    /// `/`, each revealing that path in Finder. Upstream redoes this when the active editor changes; the menu is the
    /// only reader, so the port does it when the menu shows.
    fn provide_custom_title_context_menu(&mut self, file: Option<&Path>) {
        let stale: Vec<String> = self.commands.ids().filter(|id| id.starts_with(REVEAL_PATH_COMMAND)).map(str::to_string).collect();
        for id in stale {
            self.commands.unregister(&id);
        }
        self.menus.retain(|item| !matches!(&item.entry, Entry::Command { id, .. } if id.starts_with(REVEAL_PATH_COMMAND)));
        let Some(file) = file else { return };
        for (index, (path, title)) in reveal_path_items(file).into_iter().enumerate() {
            let id = format!("{REVEAL_PATH_COMMAND}{}", index + 1);
            self.commands.register(id.clone(), move |_, _, _, cx| {
                cx.reveal_path(&path);
                Outcome::done()
            });
            let entry = Entry::Command { id, precondition: None, toggled: None, alt: None };
            self.menus.push(MenuItem { menu: MenuId::TitleBarTitleContext, entry, title, group: "1_file".into(), order: -(index as f64 + 1.), when: None });
        }
    }
}

/// The paths and labels of `provideCustomTitleContextMenu`, by segment count from 1: the label of a folder item is
/// the name of the folder holding the path it reveals, so the file's folder item reveals the file itself.
fn reveal_path_items(file: &Path) -> Vec<(PathBuf, String)> {
    let file = file.to_string_lossy();
    let segments: Vec<&str> = file.split('/').collect();
    let mut items = Vec::new();
    for count in 1..=segments.len() {
        let is_file = count == segments.len();
        let path_offset = if is_file { count } else { count + 1 };
        let path = PathBuf::from(segments[..path_offset].join("/"));
        let named = if is_file { Some(path.as_path()) } else { path.parent() };
        let label = named.and_then(Path::file_name).map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        items.push((path, if label.is_empty() { "/".into() } else { label }));
    }
    items
}

#[cfg(test)]
mod tests;
