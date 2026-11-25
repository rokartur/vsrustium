//! The native macOS menu bar: `MenuId.MenubarMainMenu` as menubarControl.ts (electron-browser) sends it, built
//! into the application menu as platform/menubar/electron-main/menubar.ts does, with its app and Window menus.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use gpui::{Action, App, Context, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType, Window, WindowHandle, actions};
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{NSApplication, NSMenuItem};
use objc2_foundation::NSString;
use serde_json::{Value, json};

use super::Workbench;
use crate::keybindings::Chord;
use super::workspace_actions::recent_openable;
use crate::menus::{MenuAction, MenuId, menu_actions, mnemonic_menu_label};
use crate::recently_opened;

actions!(menubar, [Hide, HideOthers, ShowAll]);

/// A menu item that runs a command.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = menubar, no_json)]
pub struct RunMenuCommand {
    pub id: String,
}

/// An Open Recent entry of the menu bar, by its index in `Menubar::recent`.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = menubar, no_json)]
pub struct OpenRecentMenuEntry {
    pub index: usize,
}

/// A disabled item. AppKit enables an item only when GPUI has a listener for its action, and this one has none.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = menubar, no_json)]
pub struct DisabledMenuCommand {
    pub id: String,
}

/// The key context of the bindings that give menu items their key equivalents. No element sets it, so the keys
/// still go through the workbench's own resolver first.
const MENU_KEY_EQUIVALENT: &str = "menubar_key_equivalent";

/// `productService.nameLong`.
pub const NAME_LONG: &str = "VSRustium";

/// What the menu bar shows: the top-level menus with mnemonics dropped, and each command's primary keybinding.
#[derive(Debug, PartialEq)]
pub struct Menubar {
    pub menus: Vec<(String, Vec<MenuAction>)>,
    pub keybindings: BTreeMap<String, Vec<Chord>>,
    /// `getOpenRecentActions`: the labels and entries of the recent folders and workspaces, then of the files.
    pub recent: Vec<(String, Value)>,
    pub recent_workspaces: usize,
}

/// menubarControl.ts `MAX_MENU_RECENT_ENTRIES` and `MAX_MENU_RECENT_LABEL_LENGTH`.
const MAX_MENU_RECENT_ENTRIES: usize = 10;
const MAX_MENU_RECENT_LABEL_LENGTH: usize = 120;

/// Runs the menu actions in `window`, the only window of the app.
pub fn install(window: WindowHandle<Workbench>, cx: &mut App) {
    cx.on_action(move |action: &RunMenuCommand, cx| {
        // With the window active, GPUI calls this inside its `window.update`, where a nested update fails.
        let id = action.id.clone();
        cx.defer(move |cx| {
            let _ = window.update(cx, |workbench, window, cx| workbench.run_menu_command(&id, window, cx));
        });
    });
    cx.on_action(move |action: &OpenRecentMenuEntry, cx| {
        let index = action.index;
        cx.defer(move |cx| {
            let _ = window.update(cx, |workbench, window, cx| workbench.open_recent_menu_entry(index, window, cx));
        });
    });
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
}

impl Workbench {
    /// `MenubarControl.getMenubarMenus` with the keybindings of `getMenubarKeybinding` and `getAdditionalKeybindings`.
    pub fn menubar(&self, window: &Window) -> Menubar {
        let context = self.key_context(window);
        let mut menus = Vec::new();
        for action in menu_actions(&self.menus, MenuId::MenubarMainMenu, &context) {
            if let MenuAction::Submenu { title, actions, .. } = action {
                menus.push((mnemonic_menu_label(&title), actions));
            }
        }
        let mut ids = vec!["workbench.action.quit".to_string()];
        for (_, actions) in &menus {
            collect_command_ids(actions, &mut ids);
        }
        let mut keybindings = BTreeMap::new();
        for id in ids {
            if let Some(item) = self.keybindings.resolver.lookup_primary_keybinding(&id, &context) {
                keybindings.insert(id, item.keybinding.clone());
            }
        }
        // Entries of other schemes (remote) are left out: the port opens `file` paths only.
        let menu_entries = |entries: &[Value]| -> Vec<(String, Value)> {
            let entries = entries.iter().filter(|entry| recently_opened::path(entry).is_some()).take(MAX_MENU_RECENT_ENTRIES);
            // menubarControl.ts escapes `&` (`unmnemonicLabel`) so Electron shows it; GPUI shows the label as is.
            entries.map(|entry| (truncate_middle(&recently_opened::label(entry), MAX_MENU_RECENT_LABEL_LENGTH), entry.clone())).collect()
        };
        let mut recent = menu_entries(&self.recently_opened.workspaces);
        let recent_workspaces = recent.len();
        recent.extend(menu_entries(&self.recently_opened.files));
        Menubar { menus, keybindings, recent, recent_workspaces }
    }

    /// Sets the application menu when what it shows changed, as `updateMenubar` does on menu changes.
    pub(super) fn update_menubar(&mut self, window: &Window, cx: &mut Context<Self>) {
        let menubar = self.menubar(window);
        if self.installed_menubar.as_ref() == Some(&menubar) {
            return;
        }
        let (menus, key_equivalents) = native_menus(&menubar);
        cx.clear_key_bindings();
        cx.bind_keys(key_equivalents);
        cx.set_menus(menus);
        // AppKit retitles the app menu's first "Preferences" item to "Settings…" on a later main-queue turn.
        cx.spawn(async |_, _| fix_up_app_kit_menus()).detach();
        self.installed_menubar = Some(menubar);
    }

    /// menubar.ts `createOpenRecentMenuItem`: an entry that fails to open leaves the list.
    fn open_recent_menu_entry(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, entry)) = self.installed_menubar.as_ref().and_then(|menubar| menubar.recent.get(index)).cloned() else { return };
        if !self.open_window(vec![recent_openable(&entry)], false, window, cx) {
            let uri = recently_opened::location(&entry).to_string();
            recently_opened::remove(&mut self.shared_storage.lock().unwrap(), &[&uri]);
            self.recently_opened_changed(window, cx);
        }
    }

    /// `runActionInRenderer` of a menu click: the command gets `{ from: 'menu' }` (window.ts `vscode:runAction`).
    fn run_menu_command(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let outcome = self.execute_command(id, vec![json!({ "from": "menu" })], window, cx);
        super::keyboard::report_failure(outcome, cx);
    }
}

fn collect_command_ids(actions: &[MenuAction], ids: &mut Vec<String>) {
    for action in actions {
        match action {
            MenuAction::Command { id, .. } => ids.push(id.clone()),
            MenuAction::Submenu { actions, .. } => collect_command_ids(actions, ids),
            MenuAction::Separator => {}
        }
    }
}

/// `Menubar.install`: the app menu, the workbench's menus, then the Window menu before Help.
/// shortcut: no Check for Updates, Always on Top or Switch Window; the port has no updates, always-on-top windows
/// or window switching.
fn native_menus(menubar: &Menubar) -> (Vec<Menu>, Vec<KeyBinding>) {
    let mut key_equivalents = vec![
        KeyBinding::new("cmd-h", Hide, Some(MENU_KEY_EQUIVALENT)),
        KeyBinding::new("alt-cmd-h", HideOthers, Some(MENU_KEY_EQUIVALENT)),
    ];
    let mut app_items = vec![native_command("workbench.action.showAboutDialog", &format!("About {NAME_LONG}"), true, false, menubar, &mut key_equivalents)];
    if let Some((_, preferences)) = menubar.menus.iter().find(|(title, _)| title == "Preferences") {
        app_items.push(MenuItem::separator());
        app_items.push(MenuItem::submenu(Menu::new("Preferences").items(native_items(preferences, menubar, &mut key_equivalents))));
        app_items.push(MenuItem::separator());
    }
    app_items.extend([
        MenuItem::os_submenu("Services", SystemMenuType::Services),
        MenuItem::separator(),
        MenuItem::action(format!("Hide {NAME_LONG}"), Hide),
        MenuItem::action("Hide Others", HideOthers),
        MenuItem::action("Show All", ShowAll),
        MenuItem::separator(),
        native_command("workbench.action.quit", &format!("Quit {NAME_LONG}"), true, false, menubar, &mut key_equivalents),
    ]);
    let mut menus = vec![Menu::new(NAME_LONG).items(app_items)];
    let window_menu = || Menu::new("Window").window_menu(true);
    for (title, actions) in &menubar.menus {
        match title.as_str() {
            "Preferences" => continue,
            "Help" => menus.push(window_menu()),
            _ => {}
        }
        menus.push(Menu::new(title.clone()).items(native_items(actions, menubar, &mut key_equivalents)));
    }
    if !menubar.menus.iter().any(|(title, _)| title == "Help") {
        menus.push(window_menu());
    }
    (menus, key_equivalents)
}

fn native_items(actions: &[MenuAction], menubar: &Menubar, key_equivalents: &mut Vec<KeyBinding>) -> Vec<MenuItem> {
    let mut items = Vec::new();
    for action in actions {
        // menubarControl.ts `insertActionsBefore`: the recent entries come before Open Recent's "More...".
        if matches!(action, MenuAction::Command { id, .. } if id == "workbench.action.openRecent") {
            for (index, (label, _)) in menubar.recent.iter().enumerate() {
                if index == menubar.recent_workspaces && index > 0 {
                    items.push(MenuItem::separator());
                }
                items.push(MenuItem::action(label.clone(), OpenRecentMenuEntry { index }));
            }
            if !menubar.recent.is_empty() {
                items.push(MenuItem::separator());
            }
        }
        items.push(match action {
            MenuAction::Command { id, title, enabled, checked, .. } => native_command(id, title, *enabled, *checked, menubar, key_equivalents),
            MenuAction::Submenu { title, actions, .. } => MenuItem::submenu(Menu::new(native_label(title)).items(native_items(actions, menubar, key_equivalents))),
            MenuAction::Separator => MenuItem::separator(),
        });
    }
    items
}

/// menubar.ts `createMenuItem` with `withKeybinding`: a keybinding of one chord is the item's key equivalent; one
/// AppKit cannot show, such as a chord sequence, is appended to the label in brackets.
fn native_command(id: &str, title: &str, enabled: bool, checked: bool, menubar: &Menubar, key_equivalents: &mut Vec<KeyBinding>) -> MenuItem {
    let mut label = native_label(title);
    let chords = menubar.keybindings.get(id);
    let key_equivalent = match chords.map(Vec::as_slice) {
        Some([chord]) => chord.gpui_keystroke(),
        _ => None,
    };
    if let (None, Some(chords)) = (&key_equivalent, chords) {
        let labels: Vec<String> = chords.iter().map(Chord::label).collect();
        label = format!("{label} [{}]", labels.join(" "));
    }
    // menubar.ts gives the clipboard items their roles, so they also reach native text fields.
    let os_action = match id {
        "editor.action.clipboardCutAction" => Some(OsAction::Cut),
        "editor.action.clipboardCopyAction" => Some(OsAction::Copy),
        "editor.action.clipboardPasteAction" => Some(OsAction::Paste),
        _ => None,
    };
    let item = match enabled {
        true => action_item(label, RunMenuCommand { id: id.to_string() }, os_action, key_equivalent, key_equivalents),
        false => action_item(label, DisabledMenuCommand { id: id.to_string() }, os_action, key_equivalent, key_equivalents),
    };
    item.checked(checked)
}

/// strings.ts `truncateMiddle` with its one-character ellipsis.
fn truncate_middle(value: &str, max_length: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max_length {
        return value.to_string();
    }
    let prefix: String = chars[..max_length.div_ceil(2) - 1].iter().collect();
    let suffix: String = chars[chars.len() - max_length / 2..].iter().collect();
    format!("{prefix}…{suffix}")
}

/// The label Electron's Cocoa menu shows: mnemonics dropped and "..." drawn as "…".
fn native_label(title: &str) -> String {
    mnemonic_menu_label(&crate::nls::localize(title)).replace("...", "…")
}

fn action_item<A: Action + Clone>(label: String, action: A, os_action: Option<OsAction>, key_equivalent: Option<String>, key_equivalents: &mut Vec<KeyBinding>) -> MenuItem {
    if let Some(keystroke) = key_equivalent {
        key_equivalents.push(KeyBinding::new(&keystroke, action.clone(), Some(MENU_KEY_EQUIVALENT)));
    }
    match os_action {
        Some(os_action) => MenuItem::os_action(label, action, os_action),
        None => MenuItem::action(label, action),
    }
}

/// What GPUI's menus cannot express, set on the AppKit menus gpui_macos `create_menu_bar` built.
fn fix_up_app_kit_menus() {
    // Test threads have no AppKit menus.
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    let preferences = app.mainMenu().and_then(|menu| menu.itemAtIndex(0)).and_then(|item| item.submenu()).and_then(|menu| menu.itemAtIndex(0));
    if let Some(item) = preferences
        && item.submenu().is_some()
    {
        item.setTitle(&NSString::from_str("Preferences"));
    }
    // menubar.ts `setMacWindowMenu` roles. AppKit adds its Move & Resize and tiling items after `performZoom:`.
    let Some(window_menu) = app.windowsMenu() else { return };
    let item = |title: &str, action, key: &str| unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &NSString::from_str(title), Some(action), &NSString::from_str(key))
    };
    window_menu.removeAllItems();
    window_menu.addItem(&item("Minimize", sel!(performMiniaturize:), "m"));
    window_menu.addItem(&item("Zoom", sel!(performZoom:), ""));
    window_menu.addItem(&NSMenuItem::separatorItem(mtm));
    window_menu.addItem(&item("Bring All to Front", sel!(arrangeInFront:), ""));
}
