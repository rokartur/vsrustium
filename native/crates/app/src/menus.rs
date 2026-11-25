//! Menus of platform/actions/common/actions.ts (`MenuRegistry`, `MenuId`) and menuService.ts: items with a
//! group, an order and a `when`, sorted and grouped as `MenuInfo.compareMenuItems`, flattened with separators
//! between groups as `getFlatContextMenuActions`.

#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::sync::LazyLock;

use regex::Regex;

use crate::command_palette::locale_compare;
use crate::contextkey::{Context, Expr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names, reason = "upstream MenuId names")]
pub enum MenuId {
    MenubarMainMenu,
    MenubarFileMenu,
    MenubarEditMenu,
    MenubarSelectionMenu,
    MenubarViewMenu,
    MenubarGoMenu,
    MenubarHelpMenu,
    MenubarPreferencesMenu,
    ThemesSubMenu,
    ExplorerContext,
    EditorContext,
    EditorTitleContext,
    TitleBarContext,
    TitleBarTitleContext,
    #[cfg(test)]
    Test,
}

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant, reason = "a few dozen items, built once")]
pub enum Entry {
    /// `alt` is the command and title shown while Alt is held (`IMenuItem.alt`).
    Command { id: String, precondition: Option<Expr>, toggled: Option<Expr>, alt: Option<(String, String)> },
    Submenu(MenuId),
}

/// `IMenuItem` or `ISubmenuItem`; `title` keeps upstream's `&&` mnemonics.
#[derive(Clone, Debug)]
pub struct MenuItem {
    pub menu: MenuId,
    pub entry: Entry,
    pub title: String,
    pub group: String,
    pub order: f64,
    pub when: Option<Expr>,
}

/// `MenuItemAction`, `SubmenuItemAction` or `Separator`, as a menu shows them.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    Command { id: String, title: String, enabled: bool, checked: bool, alt: Option<(String, String)> },
    Submenu { title: String, actions: Vec<MenuAction> },
    Separator,
}

/// The workbench's registrations of the features the port has: menu, submenu or command id, title, group, order,
/// `when`, and the command's precondition.
type Row = (MenuId, Option<MenuId>, &'static str, &'static str, &'static str, f64, Option<&'static str>, Option<&'static str>);
const WORKBENCH_MENUS: [Row; 47] = [
    // menubar.contribution.ts
    (MenuId::MenubarMainMenu, Some(MenuId::MenubarFileMenu), "", "&&File", "", 1., None, None),
    (MenuId::MenubarMainMenu, Some(MenuId::MenubarEditMenu), "", "&&Edit", "", 2., None, None),
    (MenuId::MenubarMainMenu, Some(MenuId::MenubarSelectionMenu), "", "&&Selection", "", 3., None, None),
    (MenuId::MenubarMainMenu, Some(MenuId::MenubarViewMenu), "", "&&View", "", 4., None, None),
    (MenuId::MenubarMainMenu, Some(MenuId::MenubarGoMenu), "", "&&Go", "", 5., None, None),
    (MenuId::MenubarMainMenu, Some(MenuId::MenubarHelpMenu), "", "&&Help", "", 8., None, None),
    (MenuId::MenubarMainMenu, Some(MenuId::MenubarPreferencesMenu), "", "&&Preferences", "", 9., Some("isMacNative"), None),
    // fileActions.contribution.ts
    (MenuId::MenubarFileMenu, None, "workbench.action.files.save", "&&Save", "4_save", 1., None, Some("activeEditor || foldersViewVisible && sideBarFocus")),
    (MenuId::MenubarFileMenu, None, "saveAll", "Save A&&ll", "4_save", 3., None, Some("dirtyWorkingCopies")),
    (
        MenuId::MenubarFileMenu,
        None,
        "workbench.action.files.revert",
        "Re&&vert File",
        "6_close",
        1.,
        None,
        Some("activeEditorCanRevert || resourceScheme != 'untitled' && foldersViewVisible && sideBarFocus"),
    ),
    (MenuId::MenubarGoMenu, None, "workbench.action.quickOpen", "Go to &&File...", "3_global_nav", 1., None, None),
    // editorExtensions.ts and clipboard.ts
    (MenuId::MenubarEditMenu, None, "undo", "&&Undo", "1_do", 1., None, None),
    (MenuId::MenubarEditMenu, None, "redo", "&&Redo", "1_do", 2., None, None),
    (MenuId::MenubarEditMenu, None, "editor.action.clipboardCutAction", "Cu&&t", "2_ccp", 1., None, None),
    (MenuId::MenubarEditMenu, None, "editor.action.clipboardCopyAction", "&&Copy", "2_ccp", 2., None, None),
    (MenuId::MenubarEditMenu, None, "editor.action.clipboardPasteAction", "&&Paste", "2_ccp", 4., None, None),
    // comment.ts
    (MenuId::MenubarEditMenu, None, "editor.action.commentLine", "&&Toggle Line Comment", "5_insert", 1., None, Some("!editorReadonly")),
    (MenuId::MenubarEditMenu, None, "editor.action.blockComment", "Toggle &&Block Comment", "5_insert", 2., None, Some("!editorReadonly")),
    (MenuId::MenubarSelectionMenu, None, "editor.action.selectAll", "&&Select All", "1_basic", 1., None, None),
    // smartSelect.ts
    (MenuId::MenubarSelectionMenu, None, "editor.action.smartSelect.expand", "&&Expand Selection", "1_basic", 2., None, None),
    (MenuId::MenubarSelectionMenu, None, "editor.action.smartSelect.shrink", "&&Shrink Selection", "1_basic", 3., None, None),
    // linesOperations.ts
    (MenuId::MenubarSelectionMenu, None, "editor.action.copyLinesUpAction", "&&Copy Line Up", "2_line", 1., None, Some("!editorReadonly")),
    (MenuId::MenubarSelectionMenu, None, "editor.action.copyLinesDownAction", "Co&&py Line Down", "2_line", 2., None, Some("!editorReadonly")),
    (MenuId::MenubarSelectionMenu, None, "editor.action.moveLinesUpAction", "Mo&&ve Line Up", "2_line", 3., None, Some("!editorReadonly")),
    (MenuId::MenubarSelectionMenu, None, "editor.action.moveLinesDownAction", "Move &&Line Down", "2_line", 4., None, Some("!editorReadonly")),
    (MenuId::MenubarSelectionMenu, None, "editor.action.duplicateSelection", "&&Duplicate Selection", "2_line", 5., None, Some("!editorReadonly")),
    // quickAccess.contribution.ts
    (MenuId::MenubarViewMenu, None, "workbench.action.showCommands", "&&Command Palette...", "1_open", 1., None, None),
    (MenuId::MenubarHelpMenu, None, "workbench.action.showCommands", "Show All Commands", "1_welcome", 2., None, None),
    // themes.contribution.ts
    (MenuId::MenubarPreferencesMenu, Some(MenuId::ThemesSubMenu), "", "&&Themes", "2_configuration", 7., None, None),
    // clipboard.ts and quickAccess.contribution.ts: the EditorContext items of the commands the port has.
    (MenuId::EditorContext, None, "editor.action.clipboardCutAction", "Cut", "9_cutcopypaste", 1., Some("!editorReadonly"), None),
    (MenuId::EditorContext, None, "editor.action.clipboardCopyAction", "Copy", "9_cutcopypaste", 2., None, None),
    (MenuId::EditorContext, None, "editor.action.clipboardPasteAction", "Paste", "9_cutcopypaste", 4., Some("!editorReadonly"), None),
    (MenuId::EditorContext, None, "workbench.action.showCommands", "Command Palette...", "z_commands", 1., Some("!editorSimpleInput"), None),
    // editor.contribution.ts: the EditorTitleContext items of the commands the port has.
    (MenuId::EditorTitleContext, None, "workbench.action.closeActiveEditor", "Close", "1_close", 10., Some("!activeEditorCannotClose"), None),
    (MenuId::EditorTitleContext, None, "workbench.action.closeOtherEditors", "Close Others", "1_close", 20., None, Some("groupEditorsCount != '1'")),
    (MenuId::EditorTitleContext, None, "workbench.action.closeEditorsToTheRight", "Close to the Right", "1_close", 30., Some("editorTabsVisible"), Some("!activeEditorIsLastInGroup && !multipleEditorsSelectedInGroup")),
    (MenuId::EditorTitleContext, None, "workbench.action.closeUnmodifiedEditors", "Close Saved", "1_close", 40., None, None),
    (MenuId::EditorTitleContext, None, "workbench.action.closeEditorsInGroup", "Close All", "1_close", 50., None, None),
    (MenuId::EditorTitleContext, None, "workbench.action.keepEditor", "Keep Open", "3_preview", 10., Some("config.workbench.editor.enablePreview"), Some("!activeEditorIsNotPreview")),
    // fileActions.contribution.ts, the ExplorerContext items of the commands the port has.
    (MenuId::ExplorerContext, None, "explorer.newFile", "New File...", "navigation", 4., Some("explorerResourceIsFolder"), Some("!explorerResourceReadonly")),
    (MenuId::ExplorerContext, None, "explorer.newFolder", "New Folder...", "navigation", 6., Some("explorerResourceIsFolder"), Some("!explorerResourceReadonly")),
    (MenuId::ExplorerContext, None, "filesExplorer.cut", "Cut", "5_cutcopypaste", 8., Some("!explorerResourceIsRoot && !explorerResourceReadonly"), None),
    (MenuId::ExplorerContext, None, "filesExplorer.copy", "Copy", "5_cutcopypaste", 10., Some("!explorerResourceIsRoot"), None),
    (MenuId::ExplorerContext, None, "filesExplorer.paste", "Paste", "5_cutcopypaste", 20., Some("explorerResourceIsFolder"), Some("!explorerResourceReadonly && fileCopied")),
    (MenuId::ExplorerContext, None, "renameFile", "Rename...", "7_modification", 10., Some("!explorerResourceIsRoot"), Some("!explorerResourceReadonly")),
    (MenuId::ExplorerContext, None, "moveFileToTrash", "Delete", "7_modification", 20., Some("!explorerResourceIsRoot && explorerResourceMoveableToTrash"), None),
    (MenuId::ExplorerContext, None, "deleteFile", "Delete Permanently", "7_modification", 20., Some("!explorerResourceIsRoot && !explorerResourceMoveableToTrash"), None),
];

/// themes.contribution.ts: `ThemesSubMenu` items; the icon theme pickers are not ported.
const THEMES_SUBMENU: [(&str, &str, f64); 1] = [("workbench.action.selectTheme", "Color Theme", 1.)];

/// titlebarActions.ts `ToggleTitleBarConfigAction`s of the title bar parts the port draws: setting, title, order,
/// `when`. Editor Actions, Notifications and Integrated Browser toggle parts the port has not.
pub const TITLE_BAR_TOGGLES: [(&str, &str, f64, &str); 3] = [
    ("window.commandCenter", "Command Center", 1., "!isCompactTitleBar"),
    ("workbench.navigationControl.enabled", "Navigation Controls", 2., "!isCompactTitleBar && config.window.commandCenter"),
    ("workbench.layoutControl.enabled", "Layout Controls", 4., ""),
];

/// `MenuRegistry` with the workbench's items.
pub fn workbench_menus() -> Vec<MenuItem> {
    let mut items = Vec::new();
    for (menu, submenu, id, title, group, order, when, precondition) in WORKBENCH_MENUS {
        let entry = match submenu {
            Some(submenu) => Entry::Submenu(submenu),
            None => {
                // The Delete item runs Delete Permanently with Alt held.
                let alt = (id == "moveFileToTrash").then(|| ("deleteFile".to_string(), "Delete Permanently".to_string()));
                Entry::Command { id: id.to_string(), precondition: precondition.and_then(Expr::deserialize), toggled: None, alt }
            }
        };
        items.push(MenuItem { menu, entry, title: title.to_string(), group: group.to_string(), order, when: when.and_then(Expr::deserialize) });
    }
    for (id, title, order) in THEMES_SUBMENU {
        let entry = Entry::Command { id: id.to_string(), precondition: None, toggled: None, alt: None };
        items.push(MenuItem { menu: MenuId::ThemesSubMenu, entry, title: title.to_string(), group: String::new(), order, when: None });
    }
    for (setting, title, order, when) in TITLE_BAR_TOGGLES {
        for menu in [MenuId::TitleBarContext, MenuId::TitleBarTitleContext] {
            let toggled = Some(Expr::has(&format!("config.{setting}")));
            let entry = Entry::Command { id: format!("toggle.{setting}"), precondition: None, toggled, alt: None };
            items.push(MenuItem { menu, entry, title: title.to_string(), group: "2_config".into(), order, when: Expr::deserialize(when) });
        }
    }
    items
}

/// `getFlatContextMenuActions(menu.getActions())`: the items of `menu` whose `when` holds, in groups joined by
/// separators. A submenu without actions is left out.
pub fn menu_actions(registry: &[MenuItem], menu: MenuId, context: &dyn Context) -> Vec<MenuAction> {
    let holds = |expr: &Option<Expr>| expr.as_ref().is_none_or(|expr| expr.evaluate(context));
    let mut items: Vec<&MenuItem> = registry.iter().filter(|item| item.menu == menu && holds(&item.when)).collect();
    items.sort_by(|a, b| compare_menu_items(a, b));
    let mut actions = Vec::new();
    let mut group: Option<&str> = None;
    for item in items {
        let action = match &item.entry {
            Entry::Command { id, precondition, toggled, alt } => {
                let checked = toggled.as_ref().is_some_and(|toggled| toggled.evaluate(context));
                MenuAction::Command { id: id.clone(), title: item.title.clone(), enabled: holds(precondition), checked, alt: alt.clone() }
            }
            Entry::Submenu(submenu) => {
                let submenu_actions = menu_actions(registry, *submenu, context);
                if submenu_actions.is_empty() {
                    continue;
                }
                MenuAction::Submenu { title: item.title.clone(), actions: submenu_actions }
            }
        };
        if group.is_some_and(|group| group != item.group) {
            actions.push(MenuAction::Separator);
        }
        group = Some(&item.group);
        actions.push(action);
    }
    actions
}

/// `MenuInfo.compareMenuItems`: groups by name with `navigation` first and no group last, then order, then title.
fn compare_menu_items(a: &MenuItem, b: &MenuItem) -> Ordering {
    if a.group != b.group {
        match (a.group.as_str(), b.group.as_str()) {
            ("", _) => return Ordering::Greater,
            (_, "") => return Ordering::Less,
            ("navigation", _) => return Ordering::Less,
            (_, "navigation") => return Ordering::Greater,
            (a, b) => match locale_compare(a, b) {
                Ordering::Equal => {}
                order => return order,
            },
        }
    }
    a.order.total_cmp(&b.order).then_with(|| locale_compare(&a.title, &b.title))
}

/// labels.ts `mnemonicMenuLabel` on macOS: mnemonics are dropped.
pub fn mnemonic_menu_label(label: &str) -> String {
    static MNEMONIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(&&\w\)|&&").expect("valid pattern"));
    MNEMONIC.replace_all(label, "").into_owned()
}
