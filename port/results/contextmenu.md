# Context menus

## Milestone 9: custom context menu and the Explorer context menu

Reference: 1.147.0 dev build on the Studio, `port/tools/ref-contextmenu` (CDP: opens the menu by a right click on an Explorer row, dumps items, keybindings, enablement, separators, boxes and computed styles, screenshots open, hovered and after ArrowDown). Dumps in `raw/contextmenu/ref/`. Fixtures: `port/fixtures/workspace` and a two-root workspace of `workspace` and `workspace2` ("Second Folder"). The port draws a custom title bar, so `window.menuStyle: inherit` gives the custom menu in the reference too.

Port: release `code-gpui --screenshot-input "right X Y; move X Y; key down"` on the same fixtures, Dark Modern, Light Modern and Default High Contrast.

- Items: the port shows the reference items whose commands it has, in the reference order and groups: New File..., New Folder..., Cut [⌘X], Copy [⌘C], Paste [⌘V], Rename... [Enter], Delete [⌘Backspace] (Delete Permanently with Alt). Files: Cut, Copy | Rename, Delete. Folders: New File, New Folder | Cut, Copy, Paste | Rename, Delete. Empty area and roots: New File, New Folder | Paste. Paste is disabled with nothing copied, as in the reference. Left out: Open to the Side, Open With, Reveal in Finder, Open in Integrated Terminal, Select for Compare, Open Timeline, Find in Folder, Copy Path, Copy Relative Path, Add Folder to Workspace, Open Folder Settings, Remove Folder from Workspace (no such commands in the port yet).
- Look: 13px font, 24px items with 4px side margins and 6px radius, labels padded 0 26px, keybindings at 0.7 opacity, separators 5px margins and a 1px `menu.separatorBackground` line, 8px container radius, 4px bar padding, `--vscode-shadow-lg`, frosted glass. Hovered item background: reference `rgb(42, 45, 46)` (computed style), port `rgb(42, 45, 46)` (pixel); menu background beside it 30,30,30 in both. Item pitch 48px at 2x in both (`raw/contextmenu/folder-notes-hover.png`, reference left, port right).
- Keyboard: ArrowDown after hovering New File... focuses New Folder... in both, with no outline in Dark Modern (`menu.selectionBorder` unset) (`raw/contextmenu/folder-src-key.png`). Up, Down, Home and PageUp, End and PageDown skip separators and disabled items and wrap; Enter and Space run; Tab does nothing; Escape closes and gives focus back. Moving the pointer off the menu unfocuses the item.
- Position: the menu opens 2px below the pointer (`getAnchorRect`) and flips at the window edges by `layout2d` (`context_menu/tests.rs`, cases of `layout.test.ts`).
- Multi-root: root rows and the empty area show the empty-area list (`raw/contextmenu/multi-empty-open.png`).
- Right click selects the row unless it is in the selection, and the command gets the row as argument; the empty area passes `{}` in a multi-root workspace and the sole root in a folder, as `explorerView.ts` does.
- Cursors: pointer on enabled items, default on disabled ones.
- shortcut: frosted glass menus open without upstream's 0.97 scale animation; GPUI has no element transforms. High contrast menus fade in over 83ms as upstream.
- `cargo test -p app`: 297 passed, 1 ignored. pixel-check (menu closed) unchanged: folder 7238/4067, multiroot 7371/4143 (dark/light).
