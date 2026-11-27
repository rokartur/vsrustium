//! Menus of platform/actions/common/actions.ts (`MenuRegistry`, `MenuId`) and menuService.ts: items with a
//! group, an order and a `when`, sorted and grouped as `MenuInfo.compareMenuItems`, flattened with separators
//! between groups as `getFlatContextMenuActions`.

#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::sync::LazyLock;

use exthost::scan::ScannedExtension;
use regex::Regex;
use serde_json::Value;

use crate::command_palette::locale_compare;
use crate::contextkey::{Context, Expr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names, reason = "upstream MenuId names")]
pub enum MenuId {
    MenubarMainMenu,
    MenubarFileMenu,
    NewFile,
    MenubarRecentMenu,
    MenubarEditMenu,
    MenubarSelectionMenu,
    MenubarViewMenu,
    MenubarGoMenu,
    MenubarLayoutMenu,
    MenubarSwitchEditorMenu,
    MenubarSwitchGroupMenu,
    MenubarHelpMenu,
    MenubarPreferencesMenu,
    MenubarAppearanceMenu,
    ActivityBarPositionMenu,
    PanelPositionMenu,
    PanelAlignmentMenu,
    /// layoutDensityActions.ts `LayoutDensityMenu`.
    LayoutDensityMenu,
    ThemesSubMenu,
    /// userDataProfile.ts `ProfilesMenu` and `OpenProfileMenu`.
    ProfilesMenu,
    OpenProfileMenu,
    GlobalActivity,
    MenubarCopy,
    MenubarShare,
    ExplorerContext,
    ExplorerContextShare,
    OpenEditorsContext,
    SearchContext,
    EditorContext,
    EditorContextCopy,
    EditorContextShare,
    EditorContextPeek,
    EditorTitleContext,
    EditorLineNumberContext,
    EditorTitleContextShare,
    EditorSplitMoveSubmenu,
    StickyScrollContext,
    TitleBarContext,
    TitleBarTitleContext,
    ScmTitle,
    ScmResourceGroupContext,
    ScmResourceStateContext,
    ScmInputBox,
    /// `MenuId.SCMSourceControl`: a repository row's context menu.
    ScmSourceControl,
    /// scmViewPane.ts `Menus.ViewSort`: the View & Sort submenu.
    ScmViewSort,
    ScmHistoryTitle,
    ScmHistoryItemContext,
    ScmHistoryItemChangeContext,
    ScmChangeContext,
    DiffEditorHunkToolbar,
    /// `MenuId.MultiDiffEditorFileToolbar`: a multi diff editor file header's toolbar.
    MultiDiffEditorFileToolbar,
    EditorTitle,
    /// `MenuId.InteractiveToolbar`: the interactive window's and REPL editor's toolbar.
    InteractiveToolbar,
    /// `MenuId.NotebookOutputToolbar`: the `...` dropdown left of a notebook output.
    NotebookOutputToolbar,
    /// `MenuId.NotebookEditorLayoutConfigure`: the notebook editor title's Customize Notebook... submenu.
    NotebookEditorLayoutConfigure,
    /// `MenuId.NotebookToolbar`: the notebook editor's toolbar, its secondary items in the `...` dropdown.
    NotebookToolbar,
    /// `MenuId.NotebookCellTitle`: a cell's toolbar, `inline*` groups as buttons, the rest in its `...` dropdown.
    NotebookCellTitle,
    /// `MenuId.NotebookCellDelete`: the cell toolbar's last button.
    NotebookCellDelete,
    /// `MenuId.NotebookCellInsert`: the cell toolbar's Insert Cell submenu.
    NotebookCellInsert,
    /// `MenuId.InteractiveCellTitle` and `InteractiveCellDelete`: the cell toolbar of an interactive window or REPL.
    InteractiveCellTitle,
    InteractiveCellDelete,
    TimelineTitle,
    TimelineItemContext,
    /// timeline.contribution.ts "Filter Timeline": a toggle per timeline source.
    TimelineFilterSubMenu,
    ViewTitle,
    ViewItemContext,
    EditorContent,
    TerminalEditorInstanceContext,
    TerminalInstanceContext,
    CommentThreadTitle,
    CommentThreadActions,
    CommentTitle,
    CommentActions,
    TerminalStickyScrollContext,
    /// A `contributes.submenus` entry, `MenuId.for('api:<id>')`, by registration order.
    Api(usize),
    #[cfg(test)]
    Test,
}

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant, reason = "a few dozen items, built once")]
pub enum Entry {
    Command { id: String, precondition: Option<Expr>, toggled: Option<Toggled>, alt: Option<Alt> },
    Submenu { submenu: MenuId, toggled: Option<Expr> },
}

/// `ICommandAction.toggled`: checked while `condition` holds, with `title` as the label then.
#[derive(Clone, Debug)]
pub struct Toggled {
    pub condition: Expr,
    pub title: Option<String>,
}

impl Toggled {
    pub fn when(condition: Option<Expr>) -> Option<Self> {
        condition.map(|condition| Self { condition, title: None })
    }
}

/// `IMenuItem.alt`: the command shown while Alt is held.
#[derive(Clone, Debug)]
pub struct Alt {
    pub id: String,
    pub title: String,
    pub precondition: Option<Expr>,
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
    /// `alt` is set only while its precondition holds, as `getActionForAltMode` shows it only then.
    Command { id: String, title: String, enabled: bool, checked: bool, alt: Option<(String, String)> },
    Submenu { title: String, actions: Vec<MenuAction>, checked: bool },
    Separator,
}

/// The workbench's registrations of the features the port has: menu, submenu or command id, title, group, order,
/// `when`, and the command's precondition.
type Row = (MenuId, Option<MenuId>, &'static str, &'static str, &'static str, f64, Option<&'static str>, Option<&'static str>);
/// taskService.ts `TaskExecutionSupportedContext`.
const TASK_EXECUTION_SUPPORTED: &str = "shellExecutionSupported && processExecutionSupported || customExecutionSupported";

const WORKBENCH_MENUS: &[Row] = &[
    // menubar.contribution.ts
    (MenuId::MenubarMainMenu, Some(MenuId::MenubarFileMenu), "", "&&File", "", 1., None, None),
    (MenuId::MenubarMainMenu, Some(MenuId::MenubarEditMenu), "", "&&Edit", "", 2., None, None),
    (MenuId::MenubarMainMenu, Some(MenuId::MenubarSelectionMenu), "", "&&Selection", "", 3., None, None),
    (MenuId::MenubarMainMenu, Some(MenuId::MenubarViewMenu), "", "&&View", "", 4., None, None),
    (MenuId::MenubarMainMenu, Some(MenuId::MenubarGoMenu), "", "&&Go", "", 5., None, None),
    (MenuId::MenubarMainMenu, Some(MenuId::MenubarHelpMenu), "", "&&Help", "", 8., None, None),
    (MenuId::MenubarMainMenu, Some(MenuId::MenubarPreferencesMenu), "", "&&Preferences", "", 9., Some("isMacNative"), None),
    // workspaceActions.ts, windowActions.ts and editor.contribution.ts
    (MenuId::MenubarFileMenu, None, "workbench.action.files.openFileFolder", "&&Open...", "2_open", 1., Some("isMacNative && openFolderWorkspaceSupport"), Some("isMacNative && openFolderWorkspaceSupport")),
    (MenuId::MenubarFileMenu, None, "workbench.action.files.openFolder", "Open &&Folder...", "2_open", 2., Some("openFolderWorkspaceSupport"), Some("openFolderWorkspaceSupport")),
    (MenuId::MenubarFileMenu, None, "workbench.action.openWorkspace", "Open Wor&&kspace from File...", "2_open", 3., Some("enterMultiRootWorkspaceSupport"), Some("enterMultiRootWorkspaceSupport")),
    (MenuId::MenubarFileMenu, Some(MenuId::MenubarRecentMenu), "", "Open &&Recent", "2_open", 4., None, None),
    (MenuId::MenubarFileMenu, None, "addRootFolder", "A&&dd Folder to Workspace...", "3_workspace", 1., Some("enterMultiRootWorkspaceSupport && !isSessionsWindow || !isSessionsWindow && workbenchState == 'workspace'"), None),
    (MenuId::MenubarFileMenu, None, "workbench.action.saveWorkspaceAs", "Save Workspace As...", "3_workspace", 2., Some("enterMultiRootWorkspaceSupport && !isSessionsWindow"), Some("enterMultiRootWorkspaceSupport && !isSessionsWindow")),
    (MenuId::MenubarFileMenu, None, "workbench.action.duplicateWorkspaceInNewWindow", "Duplicate Workspace", "3_workspace", 3., Some("enterMultiRootWorkspaceSupport && !isSessionsWindow"), Some("enterMultiRootWorkspaceSupport && !isSessionsWindow")),
    (MenuId::MenubarFileMenu, None, "workbench.action.closeFolder", "Close &&Folder", "6_close", 3., Some("workbenchState == 'folder' && emptyWorkspaceSupport && !isSessionsWindow"), Some("workbenchState != 'empty' && emptyWorkspaceSupport && !isSessionsWindow")),
    (MenuId::MenubarFileMenu, None, "workbench.action.closeFolder", "Close &&Workspace", "6_close", 3., Some("workbenchState == 'workspace' && emptyWorkspaceSupport && !isSessionsWindow"), Some("workbenchState != 'empty' && emptyWorkspaceSupport && !isSessionsWindow")),
    // windowActions.ts (electron-browser) `CloseWindowAction`
    (MenuId::MenubarFileMenu, None, "workbench.action.closeWindow", "Clos&&e Window", "6_close", 4., None, None),
    (MenuId::MenubarFileMenu, None, "workbench.action.newWindow", "New &&Window", "1_new", 3., None, None),
    (MenuId::MenubarRecentMenu, None, "workbench.action.openRecent", "&&More...", "y_more", 1., None, None),
    (MenuId::MenubarRecentMenu, None, "workbench.action.clearRecentFiles", "&&Clear Recently Opened...", "z_clear", 1., None, None),
    // fileActions.contribution.ts
    (MenuId::MenubarFileMenu, None, "workbench.action.files.newUntitledFile", "&&New Text File", "1_new", 1., None, None),
    // newFile.contribution.ts
    (MenuId::MenubarFileMenu, None, "welcome.showNewFileEntries", "New File...", "1_new", 2., Some("!isSessionsWindow"), Some("!isSessionsWindow")),
    (MenuId::NewFile, None, "workbench.action.files.newUntitledFile", "Text File", "file", 1., None, None),
    (MenuId::MenubarFileMenu, None, "workbench.action.files.save", "&&Save", "4_save", 1., None, Some("activeEditor || foldersViewVisible && sideBarFocus")),
    (MenuId::MenubarFileMenu, None, "workbench.action.files.saveAs", "Save &&As...", "4_save", 2., None, Some("activeEditor || foldersViewVisible && sideBarFocus")),
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
    (MenuId::MenubarGoMenu, None, "editor.action.dirtydiff.next", "Next &&Change", "7_change_nav", 0., None, None),
    (MenuId::MenubarGoMenu, None, "editor.action.dirtydiff.previous", "Previous &&Change", "7_change_nav", 0., None, None),
    // searchActionsSymbol.ts and gotoSymbolQuickAccess.ts
    (MenuId::MenubarGoMenu, None, "workbench.action.showAllSymbols", "Go to Symbol in &&Workspace...", "3_global_nav", 2., None, None),
    (MenuId::MenubarGoMenu, None, "workbench.action.gotoSymbol", "Go to &&Symbol in Editor...", "4_symbol_nav", 1., None, None),
    // editor.contribution.ts, editorActions.ts and openEditorsView.ts: the editor group items of the menu bar.
    (MenuId::MenubarRecentMenu, None, "workbench.action.reopenClosedEditor", "&&Reopen Closed Editor", "1_editor", 1., None, Some("canReopenClosedEditor")),
    (MenuId::MenubarViewMenu, Some(MenuId::MenubarLayoutMenu), "", "Editor &&Layout", "2_appearance", 2., Some("!isSessionsWindow"), None),
    (MenuId::MenubarLayoutMenu, None, "workbench.action.splitEditorUp", "Split &&Up", "1_split", 1., None, None),
    (MenuId::MenubarLayoutMenu, None, "workbench.action.splitEditorDown", "Split &&Down", "1_split", 2., None, None),
    (MenuId::MenubarLayoutMenu, None, "workbench.action.splitEditorLeft", "Split &&Left", "1_split", 3., None, None),
    (MenuId::MenubarLayoutMenu, None, "workbench.action.splitEditorRight", "Split &&Right", "1_split", 4., None, None),
    (MenuId::MenubarLayoutMenu, None, "workbench.action.editorLayoutSingle", "&&Single", "4_layouts", 1., None, None),
    (MenuId::MenubarLayoutMenu, None, "workbench.action.editorLayoutTwoColumns", "&&Two Columns", "4_layouts", 3., None, None),
    (MenuId::MenubarLayoutMenu, None, "workbench.action.editorLayoutThreeColumns", "T&&hree Columns", "4_layouts", 4., None, None),
    (MenuId::MenubarLayoutMenu, None, "workbench.action.editorLayoutTwoRows", "T&&wo Rows", "4_layouts", 5., None, None),
    (MenuId::MenubarLayoutMenu, None, "workbench.action.editorLayoutThreeRows", "Three &&Rows", "4_layouts", 6., None, None),
    (MenuId::MenubarLayoutMenu, None, "workbench.action.editorLayoutTwoByTwoGrid", "&&Grid (2x2)", "4_layouts", 7., None, None),
    (MenuId::MenubarLayoutMenu, None, "workbench.action.editorLayoutTwoRowsRight", "Two R&&ows Right", "4_layouts", 8., None, None),
    (MenuId::MenubarLayoutMenu, None, "workbench.action.editorLayoutTwoColumnsBottom", "Two &&Columns Bottom", "4_layouts", 9., None, None),
    (MenuId::MenubarLayoutMenu, None, "workbench.action.toggleEditorGroupLayout", "Flip &&Layout", "5_flip", 1., None, None),
    (MenuId::MenubarGoMenu, None, "workbench.action.navigateBack", "&&Back", "1_history_nav", 1., None, Some("canNavigateBack")),
    (MenuId::MenubarGoMenu, None, "workbench.action.navigateForward", "&&Forward", "1_history_nav", 2., None, Some("canNavigateForward")),
    (MenuId::MenubarGoMenu, None, "workbench.action.navigateToLastEditLocation", "&&Last Edit Location", "1_history_nav", 3., None, Some("canNavigateToLastEditLocation")),
    (MenuId::MenubarGoMenu, Some(MenuId::MenubarSwitchEditorMenu), "", "Switch &&Editor", "2_editor_nav", 1., None, None),
    (MenuId::MenubarSwitchEditorMenu, None, "workbench.action.openNextRecentlyUsedEditor", "&&Next Used Editor", "3_any_used", 1., None, None),
    (MenuId::MenubarSwitchEditorMenu, None, "workbench.action.openPreviousRecentlyUsedEditor", "&&Previous Used Editor", "3_any_used", 2., None, None),
    (MenuId::MenubarSwitchEditorMenu, None, "workbench.action.openNextRecentlyUsedEditorInGroup", "&&Next Used Editor in Group", "5_group_used", 1., None, None),
    (MenuId::MenubarSwitchEditorMenu, None, "workbench.action.openPreviousRecentlyUsedEditorInGroup", "&&Previous Used Editor in Group", "5_group_used", 2., None, None),
    (MenuId::MenubarGoMenu, Some(MenuId::MenubarSwitchGroupMenu), "", "Switch &&Group", "2_editor_nav", 2., None, None),
    (MenuId::MenubarSwitchGroupMenu, None, "workbench.action.focusFirstEditorGroup", "Group &&1", "1_focus_index", 1., None, None),
    (MenuId::MenubarSwitchGroupMenu, None, "workbench.action.focusSecondEditorGroup", "Group &&2", "1_focus_index", 2., None, None),
    (MenuId::MenubarSwitchGroupMenu, None, "workbench.action.focusThirdEditorGroup", "Group &&3", "1_focus_index", 3., None, Some("multipleEditorGroups")),
    (MenuId::MenubarSwitchGroupMenu, None, "workbench.action.focusFourthEditorGroup", "Group &&4", "1_focus_index", 4., None, Some("multipleEditorGroups")),
    (MenuId::MenubarSwitchGroupMenu, None, "workbench.action.focusFifthEditorGroup", "Group &&5", "1_focus_index", 5., None, Some("multipleEditorGroups")),
    (MenuId::MenubarSwitchGroupMenu, None, "workbench.action.focusNextGroup", "&&Next Group", "2_next_prev", 1., None, Some("multipleEditorGroups")),
    (MenuId::MenubarSwitchGroupMenu, None, "workbench.action.focusPreviousGroup", "&&Previous Group", "2_next_prev", 2., None, Some("multipleEditorGroups")),
    (MenuId::MenubarSwitchGroupMenu, None, "workbench.action.focusLeftGroup", "Group &&Left", "3_directional", 1., None, Some("multipleEditorGroups")),
    (MenuId::MenubarSwitchGroupMenu, None, "workbench.action.focusRightGroup", "Group &&Right", "3_directional", 2., None, Some("multipleEditorGroups")),
    (MenuId::MenubarSwitchGroupMenu, None, "workbench.action.focusAboveGroup", "Group &&Above", "3_directional", 3., None, Some("multipleEditorGroups")),
    (MenuId::MenubarSwitchGroupMenu, None, "workbench.action.focusBelowGroup", "Group &&Below", "3_directional", 4., None, Some("multipleEditorGroups")),
    // bracketMatching.ts
    (MenuId::MenubarGoMenu, None, "editor.action.jumpToBracket", "Go to &&Bracket", "5_infile_nav", 2., None, None),
    // editorExtensions.ts and clipboard.ts
    (MenuId::MenubarEditMenu, None, "undo", "&&Undo", "1_do", 1., None, None),
    (MenuId::MenubarEditMenu, None, "redo", "&&Redo", "1_do", 2., None, None),
    (MenuId::MenubarEditMenu, None, "editor.action.clipboardCutAction", "Cu&&t", "2_ccp", 1., None, None),
    (MenuId::MenubarEditMenu, None, "editor.action.clipboardCopyAction", "&&Copy", "2_ccp", 2., None, None),
    (MenuId::MenubarEditMenu, None, "editor.action.clipboardPasteAction", "&&Paste", "2_ccp", 4., None, None),
    // comment.ts
    (MenuId::MenubarEditMenu, None, "editor.action.commentLine", "&&Toggle Line Comment", "5_insert", 1., None, Some("!editorReadonly")),
    (MenuId::MenubarEditMenu, None, "editor.action.blockComment", "Toggle &&Block Comment", "5_insert", 2., None, Some("!editorReadonly")),
    // expandAbbreviation.ts.
    (MenuId::MenubarEditMenu, None, "editor.emmet.action.expandAbbreviation", "Emmet: E&&xpand Abbreviation", "5_insert", 3., Some("!isSessionsWindow"), Some("!editorReadonly")),
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
    // multicursor.ts
    (MenuId::MenubarSelectionMenu, None, "editor.action.insertCursorAbove", "&&Add Cursor Above", "3_multi", 2., None, None),
    (MenuId::MenubarSelectionMenu, None, "editor.action.insertCursorBelow", "A&&dd Cursor Below", "3_multi", 3., None, None),
    (MenuId::MenubarSelectionMenu, None, "editor.action.insertCursorAtEndOfEachLineSelected", "Add C&&ursors to Line Ends", "3_multi", 4., None, None),
    (MenuId::MenubarSelectionMenu, None, "editor.action.addSelectionToNextFindMatch", "Add &&Next Occurrence", "3_multi", 5., None, None),
    (MenuId::MenubarSelectionMenu, None, "editor.action.addSelectionToPreviousFindMatch", "Add P&&revious Occurrence", "3_multi", 6., None, None),
    (MenuId::MenubarSelectionMenu, None, "editor.action.selectHighlights", "Select All &&Occurrences", "3_multi", 7., None, None),
    // quickAccess.contribution.ts
    (MenuId::MenubarViewMenu, None, "workbench.action.showCommands", "&&Command Palette...", "1_open", 1., None, None),
    // layoutActions.ts, activitybarPart.ts and viewsService.ts `openCommandActionDescriptor` entries of the views the port has.
    (MenuId::MenubarViewMenu, Some(MenuId::MenubarAppearanceMenu), "", "&&Appearance", "2_appearance", 1., Some("!isSessionsWindow"), None),
    (MenuId::MenubarAppearanceMenu, None, "workbench.action.toggleSidebarPosition", "&&Move Primary Side Bar Right", "3_workbench_layout_move", 2., Some("config.workbench.sideBar.location != 'right' && !isSessionsWindow"), None),
    (MenuId::MenubarAppearanceMenu, None, "workbench.action.toggleSidebarPosition", "&&Move Primary Side Bar Left", "3_workbench_layout_move", 2., Some("config.workbench.sideBar.location == 'right' && !isSessionsWindow"), None),
    (MenuId::MenubarAppearanceMenu, Some(MenuId::ActivityBarPositionMenu), "", "Activity Bar Position", "3_workbench_layout_move", 2., Some("!isSessionsWindow"), None),
    // panelActions.ts
    (MenuId::MenubarAppearanceMenu, Some(MenuId::PanelPositionMenu), "", "Panel Position", "3_workbench_layout_move", 4., Some("!isSessionsWindow"), None),
    (MenuId::MenubarAppearanceMenu, Some(MenuId::PanelAlignmentMenu), "", "Align Panel", "3_workbench_layout_move", 5., None, None),
    (MenuId::MenubarViewMenu, None, "workbench.view.explorer", "&&Explorer", "3_sidebar", 0., Some("viewContainer.workbench.view.explorer.enabled"), Some("viewContainer.workbench.view.explorer.enabled")),
    (MenuId::MenubarViewMenu, None, "workbench.view.search", "&&Search", "3_sidebar", 1., Some("workbench.view.search.active"), Some("workbench.view.search.active")),
    (MenuId::MenubarViewMenu, None, "workbench.view.scm", "Source &&Control", "3_sidebar", 2., Some("workbench.scm.active"), Some("workbench.scm.active")),
    (MenuId::MenubarViewMenu, None, "workbench.view.debug", "&&Run", "3_sidebar", 3., Some("viewContainer.workbench.view.debug.enabled"), Some("viewContainer.workbench.view.debug.enabled")),
    (MenuId::MenubarViewMenu, None, "workbench.view.extensions", "E&&xtensions", "3_sidebar", 4., Some("viewContainer.workbench.view.extensions.enabled"), Some("viewContainer.workbench.view.extensions.enabled")),
    // repl.ts: Debug Console in the panel group.
    (MenuId::MenubarViewMenu, None, "workbench.debug.action.toggleRepl", "De&&bug Console", "5_panel", 2., None, None),
    // preferences.contribution.ts
    (MenuId::MenubarPreferencesMenu, None, "workbench.action.openSettings", "&&Settings", "2_configuration", 2., None, None),
    (MenuId::MenubarViewMenu, None, "workbench.action.terminal.toggleTerminal", "&&Terminal", "5_panel", 3., Some("terminal.active"), Some("terminal.active")),
    (MenuId::MenubarPreferencesMenu, None, "workbench.view.extensions", "&&Extensions", "2_configuration", 3., Some("!isSessionsWindow"), Some("viewContainer.workbench.view.extensions.enabled")),
    // fileActions.contribution.ts, findController.ts, searchActionsFind.ts, searchActionsNav.ts, quickAccess.contribution.ts and gotoError.ts
    (MenuId::MenubarFileMenu, None, "workbench.action.closeActiveEditor", "&&Close Editor", "6_close", 2., None, Some("activeEditor || foldersViewVisible && sideBarFocus")),
    (MenuId::MenubarEditMenu, None, "actions.find", "&&Find", "3_find", 1., None, Some("editorFocus || editorIsOpen")),
    (MenuId::MenubarEditMenu, None, "editor.action.startFindReplaceAction", "&&Replace", "3_find", 2., None, Some("editorFocus || editorIsOpen")),
    (MenuId::MenubarEditMenu, None, "workbench.action.findInFiles", "Find &&in Files", "4_find_global", 1., Some("!isSessionsWindow"), Some("!isSessionsWindow")),
    (MenuId::MenubarEditMenu, None, "workbench.action.replaceInFiles", "Replace in Files", "4_find_global", 2., Some("!isSessionsWindow"), Some("!isSessionsWindow")),
    (MenuId::MenubarGoMenu, None, "workbench.action.gotoLine", "Go to &&Line/Column...", "5_infile_nav", 1., None, None),
    (MenuId::MenubarGoMenu, None, "editor.action.marker.nextInFiles", "Next &&Problem", "6_problem_nav", 1., None, None),
    (MenuId::MenubarGoMenu, None, "editor.action.marker.prevInFiles", "Previous &&Problem", "6_problem_nav", 2., None, None),
    (MenuId::MenubarHelpMenu, None, "workbench.action.showCommands", "Show All Commands", "1_welcome", 2., None, None),
    // gettingStarted.contribution.ts and processExplorer.contribution.ts
    (MenuId::MenubarHelpMenu, None, "workbench.action.openWalkthrough", "Welcome", "1_welcome", 1., None, None),
    (MenuId::MenubarHelpMenu, None, "welcome.showAllWalkthroughs", "Open Walkthrough...", "1_welcome", 3., None, None),
    // walkThrough.contribution.ts
    (MenuId::MenubarHelpMenu, None, "workbench.action.showInteractivePlayground", "Editor Playgrou&&nd", "1_welcome", 3., None, None),
    (MenuId::MenubarHelpMenu, None, "workbench.action.openProcessExplorer", "Open &&Process Explorer", "5_tools", 2., None, None),
    // preferences.contribution.ts
    (MenuId::MenubarPreferencesMenu, None, "workbench.action.openGlobalKeybindings", "Keyboard Shortcuts", "2_configuration", 4., None, None),
    // configureSnippets.ts `ConfigureSnippetsAction`; the native menu shows the title, as the reference does.
    (MenuId::MenubarPreferencesMenu, None, "workbench.action.openSnippets", "Configure Snippets", "2_configuration", 5., None, None),
    // themes.contribution.ts
    (MenuId::MenubarPreferencesMenu, Some(MenuId::ThemesSubMenu), "", "&&Themes", "2_configuration", 7., None, None),
    // task.contribution.ts `UserTasksGlobalActionContribution`, `TaskExecutionSupportedContext`.
    (MenuId::MenubarPreferencesMenu, None, "workbench.action.tasks.openUserTasks", "Tasks", "2_configuration", 6., Some(TASK_EXECUTION_SUPPORTED), None),
    // The activity bar's Manage menu (quickAccess, preferences, extensions, tasks and themes contributions).
    (MenuId::GlobalActivity, None, "workbench.action.showCommands", "Command Palette...", "1_command", 1., None, None),
    (MenuId::GlobalActivity, None, "workbench.action.openSettings", "Settings", "2_configuration", 2., None, None),
    (MenuId::GlobalActivity, None, "workbench.view.extensions", "Extensions", "2_configuration", 3., None, None),
    (MenuId::GlobalActivity, None, "workbench.action.tasks.openUserTasks", "Tasks", "2_configuration", 6., Some(TASK_EXECUTION_SUPPORTED), None),
    (MenuId::GlobalActivity, None, "workbench.action.openGlobalKeybindings", "Keyboard Shortcuts", "2_configuration", 4., None, None),
    (MenuId::GlobalActivity, None, "workbench.action.openSnippets", "Snippets", "2_configuration", 5., None, None),
    (MenuId::GlobalActivity, Some(MenuId::ThemesSubMenu), "", "Themes", "2_configuration", 7., None, None),
    // modernUI.contribution.ts
    (MenuId::GlobalActivity, Some(MenuId::LayoutDensityMenu), "", "Layout Density", "2_configuration", 8., Some("config.workbench.experimental.modernUI == true"), None),
    // update.ts `appendUpdateMenuItems`: the states the port's update service reaches (workbench/update.rs).
    (MenuId::GlobalActivity, None, "update.check", "Check for Updates...", "7_update", 0., Some("updateState == 'idle'"), None),
    (MenuId::GlobalActivity, None, "update.checking", "Checking for Updates...", "7_update", 0., Some("updateState == 'checking for updates'"), Some("false")),
    (MenuId::GlobalActivity, None, "update.downloadNow", "Download Update (1)", "7_update", 0., Some("updateState == 'available for download'"), None),
    // clipboard.ts, editor.contribution.ts: the Copy As and Share submenus, shown once an extension fills them.
    (MenuId::MenubarEditMenu, Some(MenuId::MenubarCopy), "", "Copy As", "2_ccp", 3., None, None),
    (MenuId::MenubarFileMenu, Some(MenuId::MenubarShare), "", "Share", "45_share", 1., Some("!isSessionsWindow"), None),
    (MenuId::EditorContext, Some(MenuId::EditorContextCopy), "", "Copy As", "9_cutcopypaste", 3., None, None),
    (MenuId::EditorContext, Some(MenuId::EditorContextShare), "", "Share", "11_share", -1., Some("resourceScheme != 'output' && editorTextFocus"), None),
    (MenuId::ExplorerContext, Some(MenuId::ExplorerContextShare), "", "Share", "11_share", -1., None, None),
    (MenuId::EditorTitleContext, Some(MenuId::EditorTitleContextShare), "", "Share", "11_share", -1., Some("!multipleEditorsSelectedInGroup"), None),
    // goToCommands.ts
    (MenuId::EditorContext, None, "editor.action.revealDefinition", "Go to Definition", "navigation", 1.1, None, Some("editorHasDefinitionProvider")),
    (MenuId::EditorContext, None, "editor.action.revealDeclaration", "Go to Declaration", "navigation", 1.3, None, Some("editorHasDeclarationProvider && !isInEmbeddedEditor")),
    (MenuId::EditorContext, None, "editor.action.goToTypeDefinition", "Go to Type Definition", "navigation", 1.4, None, Some("editorHasTypeDefinitionProvider")),
    (MenuId::EditorContext, None, "editor.action.goToImplementation", "Go to Implementations", "navigation", 1.45, None, Some("editorHasImplementationProvider")),
    (MenuId::EditorContext, None, "editor.action.goToReferences", "Go to References", "navigation", 1.45, None, Some("editorHasReferenceProvider && !inReferenceSearchEditor && !isInEmbeddedEditor")),
    (MenuId::EditorContext, Some(MenuId::EditorContextPeek), "", "Peek", "navigation", 100., None, None),
    (MenuId::EditorContextPeek, None, "editor.action.peekDefinition", "Peek Definition", "peek", 2., None, Some("editorHasDefinitionProvider && !inReferenceSearchEditor && !isInEmbeddedEditor")),
    (MenuId::EditorContextPeek, None, "editor.action.peekDeclaration", "Peek Declaration", "peek", 3., None, Some("editorHasDeclarationProvider && !inReferenceSearchEditor && !isInEmbeddedEditor")),
    (MenuId::EditorContextPeek, None, "editor.action.peekTypeDefinition", "Peek Type Definition", "peek", 4., None, Some("editorHasTypeDefinitionProvider && !inReferenceSearchEditor && !isInEmbeddedEditor")),
    (MenuId::EditorContextPeek, None, "editor.action.peekImplementation", "Peek Implementations", "peek", 5., None, Some("editorHasImplementationProvider && !inReferenceSearchEditor && !isInEmbeddedEditor")),
    (MenuId::EditorContextPeek, None, "editor.action.referenceSearch.trigger", "Peek References", "peek", 6., None, Some("editorHasReferenceProvider && !inReferenceSearchEditor && !isInEmbeddedEditor")),
    // callHierarchy.contribution.ts and typeHierarchy.contribution.ts
    (MenuId::EditorContextPeek, None, "editor.showCallHierarchy", "Peek Call Hierarchy", "navigation", 1000., None, Some("editorHasCallHierarchyProvider && !inReferenceSearchEditor && !isInEmbeddedEditor")),
    (MenuId::EditorContextPeek, None, "editor.showTypeHierarchy", "Peek Type Hierarchy", "navigation", 1000., None, Some("editorHasTypeHierarchyProvider && !inReferenceSearchEditor && !isInEmbeddedEditor")),
    (MenuId::MenubarGoMenu, None, "editor.action.revealDefinition", "Go to &&Definition", "4_symbol_nav", 2., None, None),
    (MenuId::MenubarGoMenu, None, "editor.action.revealDeclaration", "Go to &&Declaration", "4_symbol_nav", 3., None, None),
    (MenuId::MenubarGoMenu, None, "editor.action.goToTypeDefinition", "Go to &&Type Definition", "4_symbol_nav", 3., None, None),
    (MenuId::MenubarGoMenu, None, "editor.action.goToImplementation", "Go to &&Implementations", "4_symbol_nav", 4., None, None),
    (MenuId::MenubarGoMenu, None, "editor.action.goToReferences", "Go to &&References", "4_symbol_nav", 5., None, None),
    // rename.ts, multicursor.ts, clipboard.ts and quickAccess.contribution.ts: the EditorContext items of the commands the port has.
    (MenuId::EditorContext, None, "editor.action.rename", "Rename Symbol", "1_modification", 1.1, Some("!editorReadonly && editorHasRenameProvider"), Some("!editorReadonly && editorHasRenameProvider")),
    // formatActions.ts, formatActionsMultiple.ts
    (MenuId::EditorContext, None, "editor.action.formatDocument", "Format Document", "1_modification", 1.3, Some("!inCompositeEditor && !editorReadonly && editorHasDocumentFormattingProvider"), Some("!inCompositeEditor && !editorReadonly && editorHasDocumentFormattingProvider")),
    (MenuId::EditorContext, None, "editor.action.formatDocument.multiple", "Format Document With...", "1_modification", 1.3, Some("!editorReadonly && editorHasMultipleDocumentFormattingProvider"), Some("!editorReadonly && editorHasMultipleDocumentFormattingProvider")),
    (MenuId::EditorContext, None, "editor.action.formatSelection", "Format Selection", "1_modification", 1.31, Some("!editorReadonly && editorHasDocumentSelectionFormattingProvider && editorHasSelection"), Some("!editorReadonly && editorHasDocumentSelectionFormattingProvider")),
    (MenuId::EditorContext, None, "editor.action.formatSelection.multiple", "Format Selection With...", "1_modification", 1.31, Some("!editorReadonly && editorHasMultipleDocumentSelectionFormattingProvider && editorHasSelection"), Some("!editorReadonly && editorHasMultipleDocumentSelectionFormattingProvider")),
    (MenuId::EditorContext, None, "editor.action.changeAll", "Change All Occurrences", "1_modification", 1.2, Some("!editorReadonly && editorTextFocus"), None),
    // codeActionCommands.ts `contextKeyForSupportedActions`: shown when a provider offers the kind.
    (MenuId::EditorContext, None, "editor.action.refactor", "Refactor...", "1_modification", 2., Some("!editorReadonly && supportedCodeAction =~ /(\\s|^)refactor\\b/"), Some("!editorReadonly && editorHasCodeActionsProvider")),
    (MenuId::EditorContext, None, "editor.action.sourceAction", "Source Action...", "1_modification", 2.1, Some("!editorReadonly && supportedCodeAction =~ /(\\s|^)source\\b/"), Some("!editorReadonly && editorHasCodeActionsProvider")),
    (MenuId::EditorContext, None, "editor.action.clipboardCutAction", "Cut", "9_cutcopypaste", 1., Some("!editorReadonly"), None),
    (MenuId::EditorContext, None, "editor.action.clipboardCopyAction", "Copy", "9_cutcopypaste", 2., None, None),
    (MenuId::EditorContext, None, "editor.action.clipboardPasteAction", "Paste", "9_cutcopypaste", 4., Some("!editorReadonly"), None),
    (MenuId::EditorContext, None, "workbench.action.showCommands", "Command Palette...", "z_commands", 1., Some("!editorSimpleInput"), None),
    // output.contribution.ts and viewsService.ts `openCommandActionDescriptor.mnemonicTitle`
    (MenuId::EditorContext, None, "workbench.output.action.clearOutput", "Clear Output", "", 0., Some("inOutput"), None),
    // markers.contribution.ts: Problems before Output in the panel group.
    (MenuId::MenubarViewMenu, None, "workbench.actions.view.problems", "&&Problems", "5_panel", 0., None, None),
    (MenuId::MenubarViewMenu, None, "workbench.action.output.toggleOutput", "&&Output", "5_panel", 1., Some("workbench.panel.output.active"), None),
    // terminalMenus.ts: the TerminalEditorInstanceContext items of the commands the port has.
    (MenuId::TerminalEditorInstanceContext, None, "workbench.action.terminal.split", "Split Terminal", "1_create", 0., None, None),
    (MenuId::TerminalEditorInstanceContext, None, "workbench.action.terminal.new", "New Terminal", "1_create", 0., None, None),
    (MenuId::TerminalEditorInstanceContext, None, "workbench.action.terminal.killEditor", "Kill Terminal", "7_kill", 0., None, None),
    (MenuId::TerminalEditorInstanceContext, None, "workbench.action.terminal.copySelection", "Copy", "3_edit", 1., None, None),
    (MenuId::TerminalEditorInstanceContext, None, "workbench.action.terminal.copySelectionAsHtml", "Copy as HTML", "3_edit", 2., None, None),
    (MenuId::TerminalEditorInstanceContext, None, "workbench.action.terminal.paste", "Paste", "3_edit", 3., None, None),
    (MenuId::TerminalEditorInstanceContext, None, "workbench.action.terminal.clear", "Clear", "5_clear", 0., None, None),
    (MenuId::TerminalEditorInstanceContext, None, "workbench.action.terminal.selectAll", "Select All", "3_edit", 3., None, None),
    // terminalMenus.ts: the TerminalInstanceContext items of the commands the port has, the menu of the panel's terminal.
    (MenuId::TerminalInstanceContext, None, "workbench.action.terminal.killViewOrEditor", "Kill Terminal", "7_kill", 0., None, None),
    (MenuId::TerminalInstanceContext, None, "workbench.action.terminal.copySelection", "Copy", "3_edit", 1., None, None),
    (MenuId::TerminalInstanceContext, None, "workbench.action.terminal.copySelectionAsHtml", "Copy as HTML", "3_edit", 2., None, None),
    (MenuId::TerminalInstanceContext, None, "workbench.action.terminal.paste", "Paste", "3_edit", 3., None, None),
    (MenuId::TerminalInstanceContext, None, "workbench.action.terminal.clear", "Clear", "5_clear", 0., None, None),
    (MenuId::TerminalInstanceContext, None, "workbench.action.terminal.selectAll", "Select All", "3_edit", 3., None, None),
    // editor.contribution.ts: the EditorTitleContext items of the commands the port has.
    (MenuId::EditorTitleContext, None, "workbench.action.closeActiveEditor", "Close", "1_close", 10., Some("!activeEditorCannotClose"), None),
    (MenuId::EditorTitleContext, None, "workbench.action.closeOtherEditors", "Close Others", "1_close", 20., None, Some("groupEditorsCount != '1'")),
    (MenuId::EditorTitleContext, None, "workbench.action.closeEditorsToTheRight", "Close to the Right", "1_close", 30., Some("editorTabsVisible"), Some("!activeEditorIsLastInGroup && !multipleEditorsSelectedInGroup")),
    (MenuId::EditorTitleContext, None, "workbench.action.closeUnmodifiedEditors", "Close Saved", "1_close", 40., None, None),
    (MenuId::EditorTitleContext, None, "workbench.action.closeEditorsInGroup", "Close All", "1_close", 50., None, None),
    (MenuId::EditorTitleContext, None, "workbench.action.keepEditor", "Keep Open", "3_preview", 10., Some("config.workbench.editor.enablePreview"), Some("!activeEditorIsNotPreview")),
    (MenuId::EditorTitleContext, None, "workbench.action.splitEditor", "Split Right", "5_split", 10., Some("!splitEditorsVertically"), None),
    (MenuId::EditorTitleContext, None, "workbench.action.splitEditor", "Split Down", "5_split", 10., Some("splitEditorsVertically"), None),
    (MenuId::EditorTitleContext, Some(MenuId::EditorSplitMoveSubmenu), "", "Split & Move", "5_split", 15., None, None),
    (MenuId::EditorSplitMoveSubmenu, None, "workbench.action.splitEditorUp", "Split Up", "1_split", 10., None, None),
    (MenuId::EditorSplitMoveSubmenu, None, "workbench.action.splitEditorDown", "Split Down", "1_split", 20., None, None),
    (MenuId::EditorSplitMoveSubmenu, None, "workbench.action.splitEditorLeft", "Split Left", "1_split", 30., None, None),
    (MenuId::EditorSplitMoveSubmenu, None, "workbench.action.splitEditorRight", "Split Right", "1_split", 40., None, None),
    (MenuId::EditorSplitMoveSubmenu, None, "workbench.action.moveEditorToAboveGroup", "Move Above", "2_move", 10., None, None),
    (MenuId::EditorSplitMoveSubmenu, None, "workbench.action.moveEditorToBelowGroup", "Move Below", "2_move", 20., None, None),
    (MenuId::EditorSplitMoveSubmenu, None, "workbench.action.moveEditorToLeftGroup", "Move Left", "2_move", 30., None, None),
    (MenuId::EditorSplitMoveSubmenu, None, "workbench.action.moveEditorToRightGroup", "Move Right", "2_move", 40., None, None),
    // fileActions.contribution.ts `appendEditorTitleContextMenuItem`.
    (MenuId::EditorTitleContext, None, "workbench.action.files.save", "Save", "1_close_save", 10., Some("activeEditorIsDirty"), None),
    (MenuId::EditorTitleContext, None, "workbench.action.files.saveAs", "Save As...", "1_close_save", 20., Some("activeEditorIsDirty"), Some("!multipleEditorsSelectedInGroup")),
    (MenuId::EditorTitleContext, None, "revealInExplorer", "Reveal in Explorer View", "2_files", 1., Some("isFileSystemResource"), Some("!multipleEditorsSelectedInGroup")),
    (MenuId::EditorTitleContext, None, "workbench.action.reopenWithEditor", "Reopen Editor With...", "1_open", 10., Some("activeEditorAvailableEditorIds"), None),
    (MenuId::EditorTitleContext, None, "breadcrumbs.copyPath", "Copy Breadcrumbs Path", "1_cutcopypaste", 100., Some("breadcrumbsPossible && breadcrumbsHasSymbols"), None),
    (MenuId::EditorTitleContext, None, "workbench.action.pinEditor", "Pin", "3_preview", 20., Some("!activeEditorIsPinned"), None),
    (MenuId::EditorTitleContext, None, "workbench.action.unpinEditor", "Unpin", "3_preview", 20., Some("activeEditorIsPinned"), None),
    // fileActions.contribution.ts and electron-browser/fileActions.contribution.ts `appendEditorTitleContextMenuItem`.
    (MenuId::EditorTitleContext, None, "copyFilePath", "Copy Path", "1_cutcopypaste", 0., Some("isFileSystemResource"), None),
    (MenuId::EditorTitleContext, None, "copyRelativeFilePath", "Copy Relative Path", "1_cutcopypaste", 0., Some("isFileSystemResource"), None),
    (MenuId::EditorTitleContext, None, "revealFileInOS", "Reveal in Finder", "2_files", 0., Some("resourceScheme == 'file' || resourceScheme == 'vscode-userdata'"), Some("!multipleEditorsSelectedInGroup")),
    // fileActions.contribution.ts: MenuId.OpenEditorsContext.
    (MenuId::OpenEditorsContext, None, "explorer.openToSide", "Open to the Side", "navigation", 10., Some("isFileSystemResource || resourceScheme == 'untitled'"), None),
    (MenuId::OpenEditorsContext, None, "revealFileInOS", "Reveal in Finder", "navigation", 20., Some("resourceScheme == 'file' || resourceScheme == 'vscode-userdata'"), None),
    (MenuId::OpenEditorsContext, None, "workbench.action.reopenWithEditor", "Reopen Editor With...", "1_open", 10., Some("activeEditorAvailableEditorIds && !groupFocusedInOpenEditors"), None),
    (MenuId::OpenEditorsContext, None, "copyFilePath", "Copy Path", "1_cutcopypaste", 10., Some("isFileSystemResource"), None),
    (MenuId::OpenEditorsContext, None, "copyRelativeFilePath", "Copy Relative Path", "1_cutcopypaste", 20., Some("isFileSystemResource"), None),
    (MenuId::OpenEditorsContext, None, "workbench.action.files.save", "Save", "2_save", 10., Some("resourceScheme == 'untitled' || !groupFocusedInOpenEditors && !readonlyEditorFocusedInOpenEditors && !autoSaveAfterShortDelayContext"), Some("dirtyEditorFocusedInOpenEditors")),
    (MenuId::OpenEditorsContext, None, "workbench.action.files.revert", "Revert File", "2_save", 20., Some("!groupFocusedInOpenEditors && !readonlyEditorFocusedInOpenEditors && resourceScheme != 'untitled' && !autoSaveAfterShortDelayContext"), Some("dirtyEditorFocusedInOpenEditors")),
    (MenuId::OpenEditorsContext, None, "workbench.files.action.saveAllInGroup", "Save All", "2_save", 30., Some("groupFocusedInOpenEditors"), Some("dirtyWorkingCopies")),
    (MenuId::OpenEditorsContext, None, "workbench.files.action.compareWithSaved", "Compare with Saved", "3_compare", 10., Some("isFileSystemResource && !autoSaveAfterShortDelayContext && !listDoubleSelection"), Some("dirtyEditorFocusedInOpenEditors")),
    (MenuId::OpenEditorsContext, None, "compareFiles", "Compare with Selected", "3_compare", 20., Some("hasResource && resourceSelectedForCompare && (isFileSystemResource || resourceScheme == 'untitled') && !listDoubleSelection"), None),
    (MenuId::OpenEditorsContext, None, "selectForCompare", "Select for Compare", "3_compare", 30., Some("hasResource && (isFileSystemResource || resourceScheme == 'untitled') && !listDoubleSelection"), None),
    (MenuId::OpenEditorsContext, None, "compareSelected", "Compare Selected", "3_compare", 30., Some("hasResource && listDoubleSelection && openEditorsSelectedFileOrUntitled"), None),
    (MenuId::OpenEditorsContext, None, "workbench.action.closeActiveEditor", "Close", "4_close", 10., Some("!groupFocusedInOpenEditors"), None),
    (MenuId::OpenEditorsContext, None, "workbench.action.closeOtherEditors", "Close Others", "4_close", 20., Some("!groupFocusedInOpenEditors"), None),
    (MenuId::OpenEditorsContext, None, "workbench.action.closeUnmodifiedEditors", "Close Saved", "4_close", 30., None, None),
    (MenuId::OpenEditorsContext, None, "workbench.action.closeEditorsInGroup", "Close All", "4_close", 40., None, None),
    // fileActions.contribution.ts, the ExplorerContext items of the commands the port has.
    (MenuId::ExplorerContext, None, "explorer.openToSide", "Open to the Side", "navigation", 10., Some("!explorerResourceIsFolder && hasResource"), None),
    (MenuId::ExplorerContext, None, "explorer.newFile", "New File...", "navigation", 4., Some("explorerResourceIsFolder"), Some("!explorerResourceReadonly")),
    (MenuId::ExplorerContext, None, "explorer.newFolder", "New Folder...", "navigation", 6., Some("explorerResourceIsFolder"), Some("!explorerResourceReadonly")),
    (MenuId::ExplorerContext, None, "addRootFolder", "Add Folder to Workspace...", "2_workspace", 10., Some("explorerResourceIsRoot && (enterMultiRootWorkspaceSupport || workbenchState == 'workspace')"), None),
    // preferences.contribution.ts.
    (MenuId::ExplorerContext, None, "_workbench.action.openFolderSettings", "Open Folder Settings", "2_workspace", 20., Some("explorerResourceIsRoot && explorerResourceIsFolder"), None),
    (MenuId::ExplorerContext, None, "removeRootFolder", "Remove Folder from Workspace", "2_workspace", 30., Some("explorerResourceIsRoot && explorerResourceIsFolder && workspaceFolderCount != '0' && (enterMultiRootWorkspaceSupport || workbenchState == 'workspace')"), None),
    (MenuId::ExplorerContext, None, "compareFiles", "Compare with Selected", "3_compare", 20., Some("!explorerResourceIsFolder && hasResource && resourceSelectedForCompare && !listDoubleSelection"), None),
    (MenuId::ExplorerContext, None, "selectForCompare", "Select for Compare", "3_compare", 30., Some("!explorerResourceIsFolder && hasResource && !listDoubleSelection"), None),
    (MenuId::ExplorerContext, None, "compareSelected", "Compare Selected", "3_compare", 30., Some("!explorerResourceIsFolder && hasResource && listDoubleSelection"), None),
    // timeline.contribution.ts; the port's resource keys name `resourceSet` `hasResource`.
    (MenuId::ExplorerContext, None, "files.openTimeline", "Open Timeline", "4_timeline", 1., Some("hasResource && timelineHasProvider"), None),
    (MenuId::ExplorerContext, None, "revealFileInOS", "Reveal in Finder", "navigation", 20., Some("resourceScheme == 'file' || resourceScheme == 'vscode-userdata'"), None),
    // imageCarousel.contribution.ts `OpenImagesInCarouselFromExplorerAction`.
    (MenuId::ExplorerContext, None, "workbench.action.openImagesInCarousel", "Open in Images Preview", "navigation", 25., Some("config.imageCarousel.explorerContextMenu.enabled && (explorerResourceIsFolder || resourceExtname =~ /^\\.(png|jpg|jpeg|jpe|gif|webp|svg|bmp|ico|mp4|webm|mov)$/i)"), None),
    // externalTerminal.contribution.ts; the port has no remote, so the `vscode-remote` alternative is left out.
    (MenuId::ExplorerContext, None, "openInIntegratedTerminal", "Open in Integrated Terminal", "navigation", 30., Some("resourceScheme == 'file' && (config.terminal.explorerKind == 'integrated' || config.terminal.explorerKind == 'both')"), None),
    (MenuId::ExplorerContext, None, "openInTerminal", "Open in External Terminal", "navigation", 31., Some("resourceScheme == 'file' && (config.terminal.explorerKind == 'external' || config.terminal.explorerKind == 'both')"), None),
    // searchActionsFind.ts.
    (MenuId::ExplorerContext, None, "filesExplorer.findInFolder", "Find in Folder...", "4_search", 10., Some("explorerResourceIsFolder"), None),
    (MenuId::ExplorerContext, None, "filesExplorer.cut", "Cut", "5_cutcopypaste", 8., Some("!explorerResourceIsRoot && !explorerResourceReadonly"), None),
    (MenuId::ExplorerContext, None, "filesExplorer.copy", "Copy", "5_cutcopypaste", 10., Some("!explorerResourceIsRoot"), None),
    (MenuId::ExplorerContext, None, "filesExplorer.paste", "Paste", "5_cutcopypaste", 20., Some("explorerResourceIsFolder"), Some("!explorerResourceReadonly && fileCopied")),
    (MenuId::ExplorerContext, None, "copyFilePath", "Copy Path", "6_copypath", 10., Some("isFileSystemResource"), None),
    (MenuId::ExplorerContext, None, "copyRelativeFilePath", "Copy Relative Path", "6_copypath", 20., Some("isFileSystemResource"), None),
    (MenuId::ExplorerContext, None, "renameFile", "Rename...", "7_modification", 10., Some("!explorerResourceIsRoot"), Some("!explorerResourceReadonly")),
    (MenuId::ExplorerContext, None, "moveFileToTrash", "Delete", "7_modification", 20., Some("!explorerResourceIsRoot && explorerResourceMoveableToTrash"), None),
    (MenuId::ExplorerContext, None, "deleteFile", "Delete Permanently", "7_modification", 20., Some("!explorerResourceIsRoot && !explorerResourceMoveableToTrash"), None),
    // searchActionsRemoveReplace.ts, searchActionsFind.ts and searchActionsCopy.ts: the SearchContext items.
    (MenuId::SearchContext, None, "search.action.replace", "Replace", "search", 1., Some("replaceActive && matchFocus && isEditableItem"), None),
    (MenuId::SearchContext, None, "search.action.replaceAllInFile", "Replace All", "search", 1., Some("replaceActive && fileMatchFocus && isEditableItem"), None),
    (MenuId::SearchContext, None, "search.action.replaceAllInFolder", "Replace All", "search", 1., Some("replaceActive && folderMatchFocus && isEditableItem"), None),
    (MenuId::SearchContext, None, "search.action.remove", "Dismiss", "search", 2., None, None),
    (MenuId::SearchContext, None, "search.action.restrictSearchToFolder", "Restrict Search to Folder", "search", 3., Some("folderMatchWithResourceFocus"), None),
    (MenuId::SearchContext, None, "search.action.expandRecursively", "Expand Recursively", "search", 4., Some("folderMatchFocus && hasSearchResult"), None),
    (MenuId::SearchContext, None, "search.action.excludeFromSearch", "Exclude Folder from Search", "search", 4., Some("folderMatchWithResourceFocus"), None),
    (MenuId::SearchContext, None, "search.action.excludeFileTypeFromSearch", "Exclude File Type from Search", "search", 5., Some("fileMatchFocus"), None),
    (MenuId::SearchContext, None, "search.action.includeFileTypeInSearch", "Include File Type from Search", "search", 6., Some("fileMatchFocus"), None),
    (MenuId::SearchContext, None, "search.action.copyMatch", "Copy", "search_2", 1., Some("fileMatchOrMatchFocus"), None),
    (MenuId::SearchContext, None, "search.action.copyPath", "Copy Path", "search_2", 2., Some("fileMatchOrFolderMatchWithResourceFocus"), None),
    (MenuId::SearchContext, None, "search.action.copyAll", "Copy All", "search_2", 3., Some("hasSearchResult"), None),
    (MenuId::SearchContext, None, "search.action.revealInSideBar", "Reveal in Explorer View", "search_3", 1., Some("fileMatchFocus && hasSearchResult"), None),
    // editActions.ts and cellOutputActions.ts; "Open in Output Preview" needs `notebook.output.openInPreviewEditor.enabled`,
    // a setting no build registers, and "Add Cell Output to Chat" needs chat.
    (MenuId::NotebookOutputToolbar, None, "notebook.cell.clearOutputs", "Clear Cell Outputs", "", 0., Some("notebookCellHasOutputs && notebookEditable && notebookCellEditable && notebookCellIsFirstOutput && config.notebook.consolidatedOutputButton"), None),
    (MenuId::NotebookOutputToolbar, None, "notebook.cellOuput.showEmptyOutputs", "Show Empty Outputs", "", 0., Some("notebookCellHasOutputs && notebookCellHasHiddenOutputs"), None),
    (MenuId::NotebookOutputToolbar, None, "notebook.cellOutput.copy", "Copy Cell Output", "", 0., Some("notebookCellHasOutputs"), None),
    (MenuId::NotebookOutputToolbar, None, "notebook.cellOutput.saveImage", "Save Image", "", 0., Some("notebookCellOutputMimetype =~ /^image\\//"), None),
    // The cell toolbar: sectionActions.ts, editActions.ts, executeActions.ts (`executeCondition`) and cellCommands.ts
    // in `CellToolbarOrder`; editActions.ts `DeleteCellAction` in NotebookCellDelete.
    (MenuId::NotebookCellTitle, None, "notebook.section.runCells", "Run Cells In Section", "inline/cell", 0., Some("notebookCellType == 'markup'"), None),
    (MenuId::NotebookCellTitle, None, "notebook.cell.edit", "Edit Cell", "inline/cell", 1., Some("notebookEditable && notebookCellType == 'markup' && !notebookCellMarkdownEditMode && notebookCellEditable"), None),
    (MenuId::NotebookCellTitle, None, "notebook.cell.executeCellsAbove", "Execute Above Cells", "inline/cell", 2., Some("notebookCellType == 'code' && (notebookKernelCount > 0 || notebookKernelSourceCount > 0 || notebookMissingKernelExtension) && config.notebook.consolidatedRunButton == false"), None),
    (MenuId::NotebookCellTitle, None, "notebook.cell.executeCellAndBelow", "Execute Cell and Below", "inline/cell", 3., Some("notebookCellType == 'code' && (notebookKernelCount > 0 || notebookKernelSourceCount > 0 || notebookMissingKernelExtension) && config.notebook.consolidatedRunButton == false"), None),
    (MenuId::NotebookCellTitle, None, "notebook.cell.quitEdit", "Stop Editing Cell", "inline/cell", 4., Some("notebookCellType == 'markup' && notebookCellMarkdownEditMode && notebookCellEditable"), None),
    (MenuId::NotebookCellTitle, None, "notebook.cell.split", "Split Cell", "inline/cell", 5., Some("notebookEditable && notebookCellEditable && !notebookCellInputIsCollapsed"), None),
    (MenuId::NotebookCellTitle, None, "notebook.cell.clearOutputs", "Clear Cell Outputs", "inline/output", 6., Some("notebookCellType == 'code' && (notebookKernelCount > 0 || notebookKernelSourceCount > 0) && notebookCellHasOutputs && notebookEditable && notebookCellEditable && !config.notebook.consolidatedOutputButton"), None),
    (MenuId::NotebookCellDelete, None, "notebook.cell.delete", "Delete Cell", "inline/cell", 0., Some("notebookEditable"), None),
    (MenuId::InteractiveCellDelete, None, "notebook.cell.delete", "Delete Cell", "inline/cell", 0., None, None),
    // The cell toolbar's `...`: notebookClipboard.ts, coreActions.ts, cellCommands.ts and layoutActions.ts in
    // `CellOverflowToolbarGroups`; Show Cell Line Numbers is added in `workbench_menus` for its check.
    (MenuId::NotebookCellTitle, None, "notebook.cell.cut", "Cut Cell", "1_copy", 1., Some("notebookEditorFocused && notebookEditable && notebookCellEditable"), None),
    (MenuId::NotebookCellTitle, None, "notebook.cell.copy", "Copy Cell", "1_copy", 2., Some("notebookEditorFocused"), None),
    (MenuId::NotebookCellTitle, None, "notebook.cell.paste", "Paste Cell", "1_copy", 3., Some("notebookEditorFocused && notebookEditable"), None),
    (MenuId::NotebookCellTitle, Some(MenuId::NotebookCellInsert), "", "Insert Cell", "2_insert", 0., Some("notebookEditable"), None),
    (MenuId::NotebookCellTitle, None, "notebook.cell.changeToCode", "Change Cell to Code", "3_edit", 0., Some("notebookEditorFocused && notebookEditable && notebookCellEditable && notebookCellType == 'markup'"), Some("activeEditor == 'workbench.editor.notebook' && notebookCellType == 'markup'")),
    (MenuId::NotebookCellTitle, None, "notebook.cell.changeToMarkdown", "Change Cell to Markdown", "3_edit", 0., Some("notebookEditorFocused && notebookEditable && notebookCellEditable && notebookCellType == 'code'"), Some("activeEditor == 'workbench.editor.notebook' && notebookCellType == 'code'")),
    (MenuId::NotebookCellTitle, None, "notebook.cell.joinAbove", "Join With Previous Cell", "3_edit", 10., Some("notebookEditorFocused && notebookEditable"), None),
    (MenuId::NotebookCellTitle, None, "notebook.cell.joinBelow", "Join With Next Cell", "3_edit", 11., Some("notebookEditorFocused && notebookEditable"), None),
    (MenuId::NotebookCellTitle, None, "notebook.cell.joinSelected", "Join Selected Cells", "3_edit", 12., Some("notebookEditorFocused && notebookEditable"), None),
    (MenuId::NotebookCellTitle, None, "notebook.cell.copyDown", "Copy Cell Down", "3_edit", 13., Some("notebookEditorFocused && notebookEditable && notebookCellEditable"), None),
    (MenuId::NotebookCellTitle, None, "notebook.cell.moveUp", "Move Cell Up", "3_edit", 14., Some("config.notebook.dragAndDropEnabled == false"), None),
    (MenuId::NotebookCellTitle, None, "notebook.cell.moveDown", "Move Cell Down", "3_edit", 14., Some("config.notebook.dragAndDropEnabled == false"), None),
    (MenuId::NotebookCellTitle, Some(MenuId::EditorContextShare), "", "Share", "4_share", 0., None, None),
    (MenuId::NotebookCellTitle, None, "notebook.toggleCellToolbarPosition", "Toggle Cell Toolbar Position", "View", 1., None, None),
    // insertCellActions.ts.
    (MenuId::NotebookCellInsert, None, "notebook.cell.insertCodeCellAbove", "Insert Code Cell Above", "", 0., None, None),
    (MenuId::NotebookCellInsert, None, "notebook.cell.insertCodeCellBelow", "Insert Code Cell Below", "", 1., None, None),
    (MenuId::NotebookCellInsert, None, "notebook.cell.insertMarkdownCellAbove", "Insert Markdown Cell Above", "", 2., None, None),
    (MenuId::NotebookCellInsert, None, "notebook.cell.insertMarkdownCellBelow", "Insert Markdown Cell Below", "", 3., None, None),
    // layoutActions.ts: the notebook toolbar's `...`; Breadcrumbs and Line Numbers are added in `workbench_menus`.
    (MenuId::NotebookToolbar, None, "workbench.notebook.layout.configure", "Customize Notebook Layout", "notebookLayout", 1., Some("config.notebook.globalToolbar == true"), None),
];

/// windowActions.ts, layoutActions.ts, auxiliaryBarActions.ts, panelActions.ts, activitybarPart.ts, toggleMinimap.ts, stickyScrollActions.ts and
/// toggleRenderControlCharacter.ts: the Appearance items the port has, checked by their `toggled`.
type ToggleRow = (MenuId, &'static str, &'static str, &'static str, f64, &'static str, &'static str);
const APPEARANCE_TOGGLES: [ToggleRow; 25] = [
    (MenuId::MenubarAppearanceMenu, "workbench.action.toggleZenMode", "Zen Mode", "1_toggle_view", 2., "!isSessionsWindow", "inZenMode"),
    (MenuId::MenubarAppearanceMenu, "workbench.action.toggleCenteredLayout", "&&Centered Layout", "1_toggle_view", 3., "!isSessionsWindow", "isCenteredLayout"),
    (MenuId::MenubarAppearanceMenu, "workbench.action.toggleStatusbarVisibility", "S&&tatus Bar", "2_workbench_layout", 3., "!isSessionsWindow", "config.workbench.statusBar.visible == true"),
    (MenuId::PanelPositionMenu, "workbench.action.positionPanelTop", "Top", "", 5., "", "panelPosition == 'top'"),
    (MenuId::PanelPositionMenu, "workbench.action.positionPanelLeft", "Left", "", 6., "", "panelPosition == 'left'"),
    (MenuId::PanelPositionMenu, "workbench.action.positionPanelRight", "Right", "", 7., "", "panelPosition == 'right'"),
    (MenuId::PanelPositionMenu, "workbench.action.positionPanelBottom", "Bottom", "", 8., "", "panelPosition == 'bottom'"),
    (MenuId::PanelAlignmentMenu, "workbench.action.alignPanelLeft", "Left", "", 5., "!isSessionsWindow", "panelAlignment == 'left'"),
    (MenuId::PanelAlignmentMenu, "workbench.action.alignPanelRight", "Right", "", 5., "!isSessionsWindow", "panelAlignment == 'right'"),
    (MenuId::PanelAlignmentMenu, "workbench.action.alignPanelCenter", "Center", "", 5., "", "panelAlignment == 'center'"),
    (MenuId::PanelAlignmentMenu, "workbench.action.alignPanelJustify", "Justify", "", 5., "", "panelAlignment == 'justify'"),
    (MenuId::MenubarAppearanceMenu, "workbench.action.toggleFullScreen", "&&Full Screen", "1_toggle_view", 1., "", "isFullscreen"),
    (MenuId::MenubarAppearanceMenu, "workbench.action.toggleSidebarVisibility", "&&Primary Side Bar", "2_workbench_layout", 1., "!isSessionsWindow", "sideBarVisible"),
    (MenuId::MenubarAppearanceMenu, "workbench.action.toggleAuxiliaryBar", "&&Secondary Side Bar", "2_workbench_layout", 2., "!isSessionsWindow", "secondarySideBarVisible"),
    (MenuId::MenubarAppearanceMenu, "workbench.action.togglePanel", "&&Panel", "2_workbench_layout", 5., "", "panelVisible"),
    // layoutDensityActions.ts
    (MenuId::LayoutDensityMenu, "workbench.action.setLayoutDensity.default", "Default", "", 1., "", "config.window.density.layout == 'default'"),
    (MenuId::LayoutDensityMenu, "workbench.action.setLayoutDensity.compact", "Compact", "", 2., "", "config.window.density.layout == 'compact'"),
    (MenuId::ActivityBarPositionMenu, "workbench.action.activityBarLocation.default", "&&Default", "", 1., "", "config.workbench.activityBar.location == 'default'"),
    (MenuId::ActivityBarPositionMenu, "workbench.action.activityBarLocation.top", "&&Top", "", 2., "", "config.workbench.activityBar.location == 'top'"),
    (MenuId::ActivityBarPositionMenu, "workbench.action.activityBarLocation.bottom", "&&Bottom", "", 3., "", "config.workbench.activityBar.location == 'bottom'"),
    (MenuId::ActivityBarPositionMenu, "workbench.action.activityBarLocation.hide", "&&Hidden", "", 4., "", "config.workbench.activityBar.location == 'hidden'"),
    (MenuId::MenubarAppearanceMenu, "editor.action.toggleMinimap", "&&Minimap", "4_editor", 1., "", "config.editor.minimap.enabled"),
    (MenuId::MenubarAppearanceMenu, "breadcrumbs.toggle", "&&Breadcrumbs", "4_editor", 2., "", "config.breadcrumbs.enabled"),
    (MenuId::MenubarAppearanceMenu, "editor.action.toggleStickyScroll", "&&Sticky Scroll", "4_editor", 3., "", "config.editor.stickyScroll.enabled"),
    (MenuId::MenubarAppearanceMenu, "editor.action.toggleRenderControlCharacter", "Render &&Control Characters", "4_editor", 5., "", "config.editor.renderControlCharacters"),
];

/// themes.contribution.ts: `ThemesSubMenu` items.
const THEMES_SUBMENU: [(&str, &str, f64); 3] = [("workbench.action.selectTheme", "Color Theme", 1.), ("workbench.action.selectIconTheme", "File Icon Theme", 2.), ("workbench.action.selectProductIconTheme", "Product Icon Theme", 3.)];

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
    for (menu, submenu, id, title, group, order, when, precondition) in WORKBENCH_MENUS.iter().copied() {
        let entry = match submenu {
            Some(submenu) => Entry::Submenu { submenu, toggled: None },
            None => {
                // The Delete item runs Delete Permanently with Alt held.
                let alt = (id == "moveFileToTrash").then(|| Alt { id: "deleteFile".into(), title: "Delete Permanently".into(), precondition: None });
                Entry::Command { id: id.to_string(), precondition: precondition.and_then(Expr::deserialize), toggled: None, alt }
            }
        };
        items.push(MenuItem { menu, entry, title: title.to_string(), group: group.to_string(), order, when: when.and_then(Expr::deserialize) });
    }
    // fileActions.contribution.ts: checked unless `files.autoSave` is `off`.
    let entry = Entry::Command { id: "workbench.action.toggleAutoSave".into(), precondition: None, toggled: Toggled::when(Expr::deserialize("config.files.autoSave != 'off'")), alt: None };
    items.push(MenuItem { menu: MenuId::MenubarFileMenu, entry, title: "A&&uto Save".into(), group: "5_autosave".into(), order: 1., when: None });
    // stickyScrollActions.ts and breadcrumbsControl.ts: the toggles' checked titles, the only ones the menu shows while it is open.
    for (id, setting, title) in [("breadcrumbs.toggle", "breadcrumbs.enabled", "Breadcrumbs"), ("editor.action.toggleStickyScroll", "editor.stickyScroll.enabled", "Sticky Scroll")] {
        let entry = Entry::Command { id: id.into(), precondition: None, toggled: Toggled::when(Expr::deserialize(&format!("config.{setting} == true"))), alt: None };
        items.push(MenuItem { menu: MenuId::StickyScrollContext, entry, title: title.into(), group: String::new(), order: 0., when: None });
    }
    // terminal.stickyScroll.contribution.ts: Toggle Sticky Scroll, checked by its setting.
    let entry = Entry::Command { id: "workbench.action.terminal.toggleStickyScroll".into(), precondition: None, toggled: Toggled::when(Expr::deserialize("config.terminal.integrated.stickyScroll.enabled == true")), alt: None };
    items.push(MenuItem { menu: MenuId::TerminalStickyScrollContext, entry, title: "Sticky Scroll".into(), group: String::new(), order: 0., when: None });
    // layoutActions.ts: the notebook editor title's Customize Notebook... gear and its submenu, with the toggles'
    // short titles (toolbar dropdowns render `shortTitle`). Not ported: Toggle Cell Toolbar Position (the cell
    // toolbar is always on the right, `notebook.cellToolbarLocation` is not read).
    let notebook = Expr::deserialize("activeEditor == 'workbench.editor.notebook'");
    let entry = Entry::Submenu { submenu: MenuId::NotebookEditorLayoutConfigure, toggled: None };
    items.push(MenuItem { menu: MenuId::EditorTitle, entry, title: "Customize Notebook...".into(), group: "navigation".into(), order: -1., when: notebook.clone() });
    let entry = Entry::Command { id: "workbench.notebook.layout.configure.editorTitle".into(), precondition: None, toggled: None, alt: None };
    items.push(MenuItem { menu: MenuId::NotebookEditorLayoutConfigure, entry, title: "Customize Notebook Layout".into(), group: "notebookLayout".into(), order: 1., when: notebook.clone() });
    let toggles = [
        ("notebook.toggleLineNumbersFromEditorTitle", "Line Numbers", "config.notebook.lineNumbers != 'off'", Some("notebookEditorFocused"), 1., notebook.clone()),
        ("breadcrumbs.toggleFromEditorTitle", "Breadcrumbs", "config.breadcrumbs.enabled == true", None, 2., None),
    ];
    for (id, title, checked, precondition, order, when) in toggles {
        let toggled = Toggled::when(Expr::deserialize(checked));
        let entry = Entry::Command { id: id.into(), precondition: precondition.and_then(Expr::deserialize), toggled, alt: None };
        items.push(MenuItem { menu: MenuId::NotebookEditorLayoutConfigure, entry, title: title.into(), group: "notebookLayoutDetails".into(), order, when });
    }
    let entry = Entry::Command { id: "notebook.toggleCellToolbarPositionFromEditorTitle".into(), precondition: None, toggled: None, alt: None };
    items.push(MenuItem { menu: MenuId::NotebookEditorLayoutConfigure, entry, title: "Toggle Cell Toolbar Position".into(), group: "notebookLayoutDetails".into(), order: 3., when: None });
    // cellEditorOptions.ts `ToggleActiveLineNumberAction`, checked by the cell's own setting, else the notebook's.
    let toggled = Toggled::when(Expr::deserialize("notebookCellLineNumbers == 'on' || notebookCellLineNumbers == 'inherit' && config.notebook.lineNumbers == 'on'"));
    let entry = Entry::Command { id: "notebook.cell.toggleLineNumbers".into(), precondition: notebook.clone(), toggled, alt: None };
    items.push(MenuItem { menu: MenuId::NotebookCellTitle, entry, title: "Show Cell Line Numbers".into(), group: "View".into(), order: 1., when: None });
    // breadcrumbsControl.ts and cellEditorOptions.ts in the notebook toolbar's `...`, titled as the toolbar renders
    // them (Line Numbers' `shortTitle`).
    let global_toolbar = Expr::deserialize("config.notebook.globalToolbar == true");
    let toolbar_toggles = [
        ("breadcrumbs.toggle", "Breadcrumbs", "config.breadcrumbs.enabled == true", None, None),
        ("notebook.toggleLineNumbers", "Line Numbers", "config.notebook.lineNumbers != 'off'", Expr::deserialize("notebookEditorFocused"), global_toolbar),
    ];
    for (id, title, checked, precondition, when) in toolbar_toggles {
        let entry = Entry::Command { id: id.into(), precondition, toggled: Toggled::when(Expr::deserialize(checked)), alt: None };
        items.push(MenuItem { menu: MenuId::NotebookToolbar, entry, title: title.into(), group: "notebookLayout".into(), order: 2., when });
    }
    // toggleWordWrap.ts: checked while the active editor wraps, enabled while it can toggle.
    let entry = Entry::Command { id: "editor.action.toggleWordWrap".into(), precondition: Some(Expr::has("canToggleWordWrap")), toggled: Toggled::when(Some(Expr::has("editorWordWrap"))), alt: None };
    items.push(MenuItem { menu: MenuId::MenubarViewMenu, entry, title: "&&Word Wrap".into(), group: "6_editor".into(), order: 1., when: None });
    // toggleWordWrap.ts: the editor title action of a file dominated by long lines.
    for (title, when) in [("Disable wrapping for this file", "isDominatedByLongLines && isWordWrapMinified"), ("Enable wrapping for this file", "!inDiffEditor && isDominatedByLongLines && !isWordWrapMinified")] {
        let entry = Entry::Command { id: "editor.action.toggleWordWrap".into(), precondition: None, toggled: None, alt: None };
        items.push(MenuItem { menu: MenuId::EditorTitle, entry, title: title.into(), group: "navigation".into(), order: 1., when: Expr::deserialize(when) });
    }
    // fileActions.contribution.ts `appendSaveConflictEditorTitleAction`: the conflict editor's accept and revert.
    for (id, title, order) in [("workbench.files.action.acceptLocalChanges", "Use your changes and overwrite file contents", -10.), ("workbench.files.action.revertLocalChanges", "Discard your changes and revert to file contents", -9.)] {
        let entry = Entry::Command { id: id.into(), precondition: None, toggled: None, alt: None };
        items.push(MenuItem { menu: MenuId::EditorTitle, entry, title: title.into(), group: "navigation".into(), order, when: Expr::deserialize("saveConflictResolutionContext") });
    }
    // localHistoryCommands.ts: Restore Contents on the editor of a local history entry.
    let entry = Entry::Command { id: "workbench.action.localHistory.restoreViaEditor".into(), precondition: None, toggled: None, alt: None };
    items.push(MenuItem { menu: MenuId::EditorTitle, entry, title: "Restore Contents".into(), group: "navigation".into(), order: -10., when: Expr::deserialize("resourceScheme == 'vscode-local-history'") });
    // toggleColumnSelection.ts: checked while `config.editor.columnSelection` is true.
    let entry = Entry::Command { id: "editor.action.toggleColumnSelection".into(), precondition: None, toggled: Toggled::when(Some(Expr::has("config.editor.columnSelection"))), alt: None };
    items.push(MenuItem { menu: MenuId::MenubarSelectionMenu, entry, title: "Column &&Selection Mode".into(), group: "4_config".into(), order: 2., when: None });
    // toggleMultiCursorModifier.ts: the item names the modifier the toggle switches to.
    for (title, current) in [("Switch to Alt+Click for Multi-Cursor", "ctrlCmd"), ("Switch to Cmd+Click for Multi-Cursor", "altKey")] {
        let entry = Entry::Command { id: "workbench.action.toggleMultiCursorModifier".into(), precondition: None, toggled: None, alt: None };
        let when = Expr::deserialize(&format!("multiCursorModifier == '{current}'"));
        items.push(MenuItem { menu: MenuId::MenubarSelectionMenu, entry, title: title.into(), group: "4_config".into(), order: 1., when });
    }
    // The Appearance items with their check: menu, command, title, group, order, `when`, `toggled`.
    for (menu, id, title, group, order, when, toggled) in APPEARANCE_TOGGLES {
        let entry = Entry::Command { id: id.into(), precondition: None, toggled: Toggled::when(Expr::deserialize(toggled)), alt: None };
        items.push(MenuItem { menu, entry, title: title.into(), group: group.into(), order, when: Expr::deserialize(when) });
    }
    for (id, title, order) in THEMES_SUBMENU {
        let entry = Entry::Command { id: id.to_string(), precondition: None, toggled: None, alt: None };
        items.push(MenuItem { menu: MenuId::ThemesSubMenu, entry, title: title.to_string(), group: String::new(), order, when: None });
    }
    for (setting, title, order, when) in TITLE_BAR_TOGGLES {
        for menu in [MenuId::TitleBarContext, MenuId::TitleBarTitleContext] {
            let toggled = Toggled::when(Some(Expr::has(&format!("config.{setting}"))));
            let entry = Entry::Command { id: format!("toggle.{setting}"), precondition: None, toggled, alt: None };
            items.push(MenuItem { menu, entry, title: title.to_string(), group: "2_config".into(), order, when: Expr::deserialize(when) });
        }
    }
    items.extend(crate::workbench::scm_actions::menu_items());
    items
}

/// menusExtensionPoint.ts `apiMenus`: the `contributes.menus` keys of the menus the port shows. `commandPalette`
/// is read by `command_palette::menu`.
const API_MENUS: [(&str, MenuId); 35] = [
    ("file/newFile", MenuId::NewFile),
    ("editor/context", MenuId::EditorContext),
    ("explorer/context", MenuId::ExplorerContext),
    ("editor/title/context", MenuId::EditorTitleContext),
    ("editor/lineNumber/context", MenuId::EditorLineNumberContext),
    ("scm/title", MenuId::ScmTitle),
    ("scm/resourceGroup/context", MenuId::ScmResourceGroupContext),
    ("scm/resourceState/context", MenuId::ScmResourceStateContext),
    ("scm/inputBox", MenuId::ScmInputBox),
    ("scm/change/title", MenuId::ScmChangeContext),
    ("diffEditor/gutter/hunk", MenuId::DiffEditorHunkToolbar),
    ("multiDiffEditor/resource/title", MenuId::MultiDiffEditorFileToolbar),
    ("editor/title", MenuId::EditorTitle),
    ("interactive/toolbar", MenuId::InteractiveToolbar),
    ("notebook/toolbar", MenuId::NotebookToolbar),
    ("notebook/cell/title", MenuId::NotebookCellTitle),
    ("interactive/cell/title", MenuId::InteractiveCellTitle),
    ("scm/sourceControl", MenuId::ScmSourceControl),
    ("scm/history/title", MenuId::ScmHistoryTitle),
    ("scm/historyItem/context", MenuId::ScmHistoryItemContext),
    ("timeline/title", MenuId::TimelineTitle),
    ("timeline/item/context", MenuId::TimelineItemContext),
    ("view/title", MenuId::ViewTitle),
    ("view/item/context", MenuId::ViewItemContext),
    ("editor/context/copy", MenuId::EditorContextCopy),
    ("editor/context/share", MenuId::EditorContextShare),
    ("explorer/context/share", MenuId::ExplorerContextShare),
    ("editor/title/context/share", MenuId::EditorTitleContextShare),
    ("file/share", MenuId::MenubarShare),
    ("menuBar/edit/copy", MenuId::MenubarCopy),
    ("editor/content", MenuId::EditorContent),
    ("comments/commentThread/title", MenuId::CommentThreadTitle),
    ("comments/commentThread/context", MenuId::CommentThreadActions),
    ("comments/comment/title", MenuId::CommentTitle),
    ("comments/comment/context", MenuId::CommentActions),
];

/// The `icon` of each `contributes.commands` entry with one: a `$(codicon)` name, or the light and dark image paths.
pub fn command_icons(extensions: &[ScannedExtension]) -> std::collections::HashMap<String, CommandIcon> {
    let mut icons = crate::workbench::scm_actions::command_icons();
    // toggleWordWrap.ts: the editor title items' `Codicon.wordWrap`.
    icons.insert("editor.action.toggleWordWrap".to_string(), CommandIcon::Codicon("word-wrap".into()));
    // localHistory.ts `LOCAL_HISTORY_ICON_RESTORE`.
    icons.insert("workbench.action.localHistory.restoreViaEditor".to_string(), CommandIcon::Codicon("check".into()));
    icons.insert("workbench.files.action.acceptLocalChanges".to_string(), CommandIcon::Codicon("check".into()));
    icons.insert("workbench.files.action.revertLocalChanges".to_string(), CommandIcon::Codicon("discard".into()));
    // notebookIcons.ts: the cell toolbar's buttons.
    let notebook_icons = [
        ("notebook.section.runCells", "play"),
        ("notebook.cell.edit", "pencil"),
        ("notebook.cell.executeCellsAbove", "run-above"),
        ("notebook.cell.executeCellAndBelow", "run-below"),
        ("notebook.cell.quitEdit", "check"),
        ("notebook.cell.split", "split-vertical"),
        ("notebook.cell.clearOutputs", "clear-all"),
        ("notebook.cell.delete", "trash"),
        ("notebook.cell.moveUp", "arrow-up"),
        ("notebook.cell.moveDown", "arrow-down"),
    ];
    for (id, icon) in notebook_icons {
        icons.insert(id.to_string(), CommandIcon::Codicon(icon.into()));
    }
    for extension in extensions {
        let commands = match &extension.description["contributes"]["commands"] {
            Value::Array(commands) => commands.iter().collect(),
            command @ Value::Object(_) => vec![command],
            _ => Vec::new(),
        };
        for command in commands {
            let Some(id) = command["command"].as_str() else { continue };
            let icon = match &command["icon"] {
                Value::String(icon) => match icon.strip_prefix("$(").and_then(|icon| icon.strip_suffix(')')) {
                    Some(codicon) => CommandIcon::Codicon(codicon.to_string()),
                    None => CommandIcon::Image(extension.location.join(icon), extension.location.join(icon)),
                },
                Value::Object(paths) => {
                    let path = |key: &str| extension.location.join(paths.get(key).and_then(Value::as_str).unwrap_or_default());
                    CommandIcon::Image(path("light"), path("dark"))
                }
                _ => continue,
            };
            icons.insert(id.to_string(), icon);
        }
    }
    icons
}

/// A command's icon in a toolbar (`ICommandAction.icon`).
#[derive(Clone, Debug, PartialEq)]
pub enum CommandIcon {
    Codicon(String),
    Image(std::path::PathBuf, std::path::PathBuf),
}

/// menusExtensionPoint.ts, the `submenus` and `menus` handlers: every extension's `contributes.menus` items of the
/// port's menus and of contributed submenus, with their command's title and enablement from `contributes.commands`.
/// An item of a command or submenu nobody contributes is skipped, as is a menu whose items are not all valid.
pub fn extension_menus(extensions: &[ScannedExtension]) -> Vec<MenuItem> {
    let commands: Vec<_> = extensions.iter().flat_map(crate::command_palette::contributed_commands).collect();
    // MenuRegistry.addCommand: the last registration of an id wins.
    let find_command = |id: &str| commands.iter().rev().find(|command| command.id == id);
    let submenus = contributed_submenus(extensions);
    let mut items = Vec::new();
    let mut submenu_items: Vec<(MenuId, MenuId)> = Vec::new();
    for extension in extensions {
        let Some(menus) = extension.description["contributes"]["menus"].as_object() else { continue };
        for (key, entries) in menus {
            let Some(entries) = entries.as_array().filter(|entries| entries.iter().all(is_valid_item)) else { continue };
            let api_menu = API_MENUS.iter().find(|(api_key, _)| api_key == key).map(|(_, menu)| *menu);
            // menusExtensionPoint.ts: `editor/content` is `proposed: 'contribEditorContentMenu'`.
            if key == "editor/content" && !extension.description["enabledApiProposals"].as_array().is_some_and(|proposals| proposals.iter().any(|proposal| proposal == "contribEditorContentMenu")) {
                continue;
            }
            let Some(menu) = api_menu.or_else(|| submenus.iter().find(|(id, ..)| id == key).map(|(_, menu, _)| *menu)) else { continue };
            for entry in entries {
                let (entry_value, title) = match entry["command"].as_str() {
                    Some(id) => {
                        let Some(command) = find_command(id) else { continue };
                        let alt = entry["alt"].as_str().and_then(find_command).map(|alt| Alt { id: alt.id.clone(), title: alt.title.clone(), precondition: alt.precondition.clone() });
                        (Entry::Command { id: command.id.clone(), precondition: command.precondition.clone(), toggled: None, alt }, command.title.clone())
                    }
                    None => {
                        let Some((_, submenu, label)) = submenus.iter().find(|(id, ..)| entry["submenu"] == **id) else { continue };
                        if submenu_items.contains(&(menu, *submenu)) {
                            continue;
                        }
                        submenu_items.push((menu, *submenu));
                        (Entry::Submenu { submenu: *submenu, toggled: None }, label.clone())
                    }
                };
                let (group, order) = group_and_order(entry["group"].as_str().unwrap_or_default());
                let when = entry["when"].as_str().and_then(Expr::deserialize);
                items.push(MenuItem { menu, entry: entry_value, title, group, order, when });
            }
        }
    }
    items
}

/// `contributes.submenus`: id, menu id and label; the first registration of an id wins. Icons are not kept: they
/// show in toolbars (`editor/title`, view titles), which the port has no menus for yet.
fn contributed_submenus(extensions: &[ScannedExtension]) -> Vec<(String, MenuId, String)> {
    let mut submenus: Vec<(String, MenuId, String)> = Vec::new();
    for extension in extensions {
        for submenu in extension.description["contributes"]["submenus"].as_array().into_iter().flatten() {
            let (Some(id), Some(label)) = (submenu["id"].as_str(), submenu["label"].as_str()) else { continue };
            if id.is_empty() || label.is_empty() || submenus.iter().any(|(known, ..)| known == id) {
                continue;
            }
            submenus.push((id.to_string(), MenuId::Api(submenus.len()), label.to_string()));
        }
    }
    submenus
}

/// `schema.isValidMenuItem` and `isValidSubmenuItem`: `command` or `submenu` a string, `alt`, `when` and
/// `group` absent or strings.
fn is_valid_item(item: &Value) -> bool {
    let optional_string = |value: &Value| value.is_string() || value.is_null() || *value == false || *value == 0 || *value == "";
    let required = if item["command"].is_string() { optional_string(&item["alt"]) } else { item["submenu"].is_string() };
    required && optional_string(&item["when"]) && optional_string(&item["group"])
}

/// `group@order`: the order after the last `@` (0 when it is not a number), the group alone otherwise.
fn group_and_order(group: &str) -> (String, f64) {
    match group.rfind('@') {
        Some(index) if index > 0 => (group[..index].to_string(), group[index + 1..].trim().parse().ok().filter(|order: &f64| !order.is_nan()).unwrap_or(0.)),
        _ => (group.to_string(), 0.),
    }
}

/// `getFlatContextMenuActions(menu.getActions())`: the items of `menu` whose `when` holds, in groups joined by
/// separators. A submenu without actions is left out.
pub fn menu_actions(registry: &[MenuItem], menu: MenuId, context: &dyn Context) -> Vec<MenuAction> {
    menu_group_actions(registry, menu, context, |_| true)
}

/// `menu_actions` of the groups `keep` takes, as `getActionBarActions` splits a toolbar's buttons from its `...`.
pub fn menu_group_actions(registry: &[MenuItem], menu: MenuId, context: &dyn Context, keep: impl Fn(&str) -> bool) -> Vec<MenuAction> {
    let holds = |expr: &Option<Expr>| expr.as_ref().is_none_or(|expr| expr.evaluate(context));
    let mut items: Vec<&MenuItem> = registry.iter().filter(|item| item.menu == menu && keep(&item.group) && holds(&item.when)).collect();
    items.sort_by(|a, b| compare_menu_items(a, b));
    let mut actions = Vec::new();
    let mut group: Option<&str> = None;
    for item in items {
        let action = match &item.entry {
            Entry::Command { id, precondition, toggled, alt } => {
                let toggled = toggled.as_ref().filter(|toggled| toggled.condition.evaluate(context));
                let title = toggled.and_then(|toggled| toggled.title.clone()).unwrap_or_else(|| item.title.clone());
                let alt = alt.as_ref().filter(|alt| holds(&alt.precondition)).map(|alt| (alt.id.clone(), alt.title.clone()));
                MenuAction::Command { id: id.clone(), title, enabled: holds(precondition), checked: toggled.is_some(), alt }
            }
            Entry::Submenu { submenu, toggled } => {
                let submenu_actions = menu_actions(registry, *submenu, context);
                if submenu_actions.is_empty() {
                    continue;
                }
                let checked = toggled.as_ref().is_some_and(|toggled| toggled.evaluate(context));
                MenuAction::Submenu { title: item.title.clone(), actions: submenu_actions, checked }
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
