# Context menus

## Milestone 9: custom context menu and the Explorer context menu

Reference: 1.147.0 dev build on the Studio, `port/tools/ref-contextmenu` (CDP: opens the menu by a right click on an Explorer row, dumps items, keybindings, enablement, separators, boxes and computed styles, screenshots open, hovered and after ArrowDown). Dumps in `raw/contextmenu/ref/`. Fixtures: `port/fixtures/workspace` and a two-root workspace of `workspace` and `workspace2` ("Second Folder"). The port draws a custom title bar, so `window.menuStyle: inherit` gives the custom menu in the reference too.

Port: release `code-gpui --screenshot-input "right X Y; move X Y; key down"` on the same fixtures, Dark Modern, Light Modern and Default High Contrast.

- Items: the port shows the reference items whose commands it has, in the reference order and groups: New File..., New Folder..., Cut [⌘X], Copy [⌘C], Paste [⌘V], Rename... [Enter], Delete [⌘Backspace] (Delete Permanently with Alt). Files: Cut, Copy | Rename, Delete. Folders: New File, New Folder | Cut, Copy, Paste | Rename, Delete. Empty area and roots: New File, New Folder | Paste. Paste is disabled with nothing copied, as in the reference. Left out: Open to the Side, Open With, Reveal in Finder, Open in Integrated Terminal, Select for Compare, Open Timeline, Find in Folder, Copy Path, Copy Relative Path, Add Folder to Workspace, Open Folder Settings, Remove Folder from Workspace (no such commands in the port yet).
- Look: 13px font, 24px items with 4px side margins and 6px radius, labels padded 0 26px, keybindings at 0.7 opacity, separators 5px margins and a 1px `menu.separatorBackground` line, 8px container radius, 4px bar padding, `--vscode-shadow-lg`, frosted glass. Hovered item background: reference `rgb(42, 45, 46)` (computed style), port `rgb(42, 45, 46)` (pixel); menu background beside it 30,30,30 in both. Item pitch 48px at 2x in both (`raw/contextmenu/folder-notes-hover.png`, reference left, port right).
- Keyboard: ArrowDown after hovering New File... focuses New Folder... in both, with no outline in Dark Modern (`menu.selectionBorder` unset) (`raw/contextmenu/folder-src-key.png`): focused item `rgb(42, 45, 46)` in both; the New File... and New Folder... rows (440x110 px at 2x) differ in 119 pixels (0.25%, `magick compare -fuzz 2%`), glyph edges. Up, Down, Home and PageUp, End and PageDown skip separators and disabled items and wrap; Enter and Space run; Tab does nothing; Escape closes and gives focus back. Moving the pointer off the menu unfocuses the item.
- Position: the menu opens 2px below the pointer (`getAnchorRect`) and flips at the window edges by `layout2d` (`context_menu/tests.rs`, cases of `layout.test.ts`).
- Multi-root: root rows and the empty area show the empty-area list (`raw/contextmenu/multi-empty-open.png`).
- Right click selects the row unless it is in the selection, and the command gets the row as argument; the empty area passes `{}` in a multi-root workspace and the sole root in a folder, as `explorerView.ts` does.
- Cursors: pointer on enabled items, default on disabled ones.
- shortcut: frosted glass menus open without upstream's 0.97 scale animation; GPUI has no element transforms. High contrast menus fade in over 83ms as upstream.
- `cargo test -p app`: 297 passed, 1 ignored. pixel-check (menu closed) unchanged: folder 7238/4067, multiroot 7371/4143 (dark/light).

## Milestone 9: editor and tab context menus

Reference: `port/tools/ref-contextmenu editor tab titlebar` on `port/fixtures/workspace` with `notes.txt` open, Dark Modern (`raw/contextmenu/ref/editor/`). Port: release `code-gpui workspace notes.txt --screenshot-input "right X Y; move X+30 Y+14; key down"` at the reference's points (editor 415,493, below the last line; tab 349,51).

- Editor (`raw/contextmenu/editor-key.png`, port left, reference right): Cut [⌘X], Copy [⌘C], Paste [⌘V] | Command Palette... [⇧⌘P], at the same place, same item look; the cursor moves to the end of the last line (status bar Ln 10, Col 1 in both). The reference's first group, Change All Occurrences [⌘F2] and Refactor... [⌃⇧R], is missing: the port has neither command.
- Tab (`raw/contextmenu/tab-key.png`): Close [⌘W], Close Others [⌥⌘T, disabled], Close to the Right [disabled], Close Saved [⌘K U], Close All [⌘K W] | Keep Open [⌘K Enter, disabled], at the same place with the same enablement. ArrowDown after the pointer focuses Close Saved in both. The reference's other groups (Copy Path to Copy into New Window) are missing: the port has none of those commands. The port's menu is narrower since its longest row is shorter.
- Title bar: not done (port/notes.md).
- `cargo test -p app`: 396 passed, 1 ignored (editor_menus: menu items, cursor move and selection kept, focus back on Escape, the close commands on a right-clicked tab, Close Saved keeping a dirty editor, Close All asking for it).

## Milestone 9: extension menu items (contributes.menus)

Reference: `FOLDER=port/fixtures/workspace port/tools/ref-contextmenu /tmp/ref-md README.md src`. Port and reference side by side: `port/tools/ui-pair` with a right click at 93,195 (the README.md row), Dark Modern (`raw/contextmenu/folder-readme-extension.png`, reference left).

- README.md: the port shows markdown-language-features' Open Preview (`navigation`) and Find File References (`4_search`) at the reference's places, then Cut, Copy | Rename..., Delete. The reference's other items (Open to the Side, Open With..., Reveal in Finder, Open in Integrated Terminal, Select for Compare, Open Timeline, Copy Path, Copy Relative Path) are core commands the port does not have yet.
- `cargo test -p app`: 846 passed, 1 ignored.
