# Native menu bar

## Milestone 9 list: the menu bar against a fresh reference dump

`port/tools/ref-menubar` on the Studio, 1.147.0 dev build: the renderer's `IMenubarData` in an empty window and in `port/fixtures/workspace` with `notes.txt` open, saved as `native/crates/app/src/workbench/menubar/reference-empty.json` and `reference-file.json`. `cargo test -p app menubar` builds the port's `Workbench::menubar` in the same two windows and compares, menu by menu, every reference item whose command the port registers: label, command, separators, submenus, checked and enabled state.

- Added from the dump: File > Close Editor (`activeEditor || foldersViewVisible && sideBarFocus`), Edit > Find, Replace (`editorFocus || editorIsOpen`), Find in Files, Replace in Files; View > Appearance (Primary and Secondary Side Bar, Panel, Activity Bar Position, Minimap, Sticky Scroll, Render Control Characters), Explorer, Search, Source Control, Extensions, Terminal; Go > Go to Line/Column, Next and Previous Problem; Preferences > Extensions.
- Toggles read `config.<setting>` context keys (contextKeyService.ts `ConfigAwareContextValuesContainer`), so File > Auto Save is unchecked with `files.autoSave` off, as in the reference; before it showed checked.
- Not in the port: reference items whose commands the port lacks (for example Terminal > New Terminal, Run, Help's documentation links), filtered out by the comparison, not hidden.

## Milestone 9: menu bar

Reference: 1.147.0 dev build on the Studio, `port/tools/ref-menubar` (the renderer's `IMenubarData` and Electron's application menu) and the cua-driver AX tree of its menu bar. Port: release `code-gpui`, the same AX tree, in an empty window, `port/fixtures/workspace`, and the folder with `notes.txt` open.

- Items: the port shows the reference items whose commands it has: File (Save, Save All, Revert File), Edit (Undo, Redo, Cut, Copy, Paste), Selection (Select All), View (Command Palette…), Go (Go to File…), Help (Show All Commands), Preferences > Themes > Color Theme [⌘K ⌘T]. Group separators fall where the reference has them between those items.
- Enablement: Save and Revert File disabled in the empty and folder windows, enabled with a file open; Save All disabled with no dirty editor. Same as the reference in all three windows.
- App menu: Preferences (not AppKit's "Settings…"), Services, Hide Code - OSS, Hide Others, Show All, Quit Code - OSS. The reference names itself "Code - OSS Dev" (dev build) and adds About; the port has no about dialog.
- Window menu: Minimize, Zoom, AppKit's Move & Resize and Full Screen Tile, Bring All to Front, as the reference; without Always on Top and Switch Window…, which the port has no commands for.
- Labels: "..." shows as "…" as in Electron's menus.
- Click: Selection > Select All through `cua-driver invoke_menu` selects the whole of notes.txt.
- `cargo test -p app`: 293 passed, 1 ignored.
