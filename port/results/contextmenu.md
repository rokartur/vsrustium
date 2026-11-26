# Context menus

## Milestone 9: custom context menu and the Explorer context menu

Reference: 1.147.0 dev build on the Studio, `port/tools/ref-contextmenu` (CDP: opens the menu by a right click on an Explorer row, dumps items, keybindings, enablement, separators, boxes and computed styles, screenshots open, hovered and after ArrowDown). Dumps in `raw/contextmenu/ref/`. Fixtures: `port/fixtures/workspace` and a two-root workspace of `workspace` and `workspace2` ("Second Folder"). The port draws a custom title bar, so `window.menuStyle: inherit` gives the custom menu in the reference too.

Port: release `code-gpui --screenshot-input "right X Y; move X Y; key down"` on the same fixtures, Dark Modern, Light Modern and Default High Contrast.

- Items: the port shows the reference items whose commands it has, in the reference order and groups: New File..., New Folder..., Cut [⌘X], Copy [⌘C], Paste [⌘V], Rename... [Enter], Delete [⌘Backspace] (Delete Permanently with Alt). Files: Cut, Copy | Rename, Delete. Folders: New File, New Folder | Cut, Copy, Paste | Rename, Delete. Empty area and roots: New File, New Folder | Paste. Paste is disabled with nothing copied, as in the reference. Left out: Open to the Side, Open With, Reveal in Finder, Open in Integrated Terminal, Select for Compare, Open Timeline, Find in Folder, Copy Path, Copy Relative Path, Add Folder to Workspace, Open Folder Settings, Remove Folder from Workspace (no such commands in the port yet).
- Milestone 12 verification: files now also show Select for Compare (Compare with Selected once one is selected, Compare Selected with two rows selected) in `3_compare`; the empty area stands for the only root, so it shows New File..., New Folder..., Reveal in Finder | Add Folder to Workspace..., Remove Folder from Workspace | Open Timeline | Paste | Copy Path, Copy Relative Path, the reference's order (ui-pair `port/fixtures/ui-pair/m12/workspace-folders.json`); still left out there: Open in Images Preview, Open in Integrated Terminal, Open Folder Settings, Find in Folder.
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

## Milestone 12: Copy Path, Copy Relative Path, Reveal in Finder

`port/tools/ui-pair` on `port/fixtures/workspace` (copied to `~/m12-ws`), Dark Modern: click the `src` row, Alt+Cmd+C, Shift+Alt+Cmd+C (pasteboard read by the `clipboard` step), right click the row.

- Pasteboard: `<folder>/src` and `src` in both.
- Menu: New File..., New Folder..., Reveal in Finder [⌥⌘R] | Cut, Copy, Paste | Copy Path [⌥⌘C], Copy Relative Path [⇧⌥⌘C] | Rename..., Delete, at the reference's places. The reference also has Open in Images Preview, Open in Integrated Terminal, Find in Folder... and Open Timeline, which the port does not have, and shows Delete Permanently where the port shows Delete. That is the menu's `alt` item (fileActions.contribution.ts: `moveFileToTrash` with `alt: deleteFile`): dom.ts `ModifierKeyEmitter` changes its Alt state only on key events, and the synthesized Alt+Cmd+C leaves it held. A recheck (`scratch/del-steps.json`: click `src`, right click; Alt+Cmd+C, right click; Shift, right click) gives Delete, Delete Permanently, Delete in the reference: one more key event resets it. `explorerResourceMoveableToTrash` is true in both apps (`files.enableTrash` and the disk provider's Trash capability), so the port keeps Delete; no code change.
- `cargo test -p app`: 888 passed, 1 ignored.

## Milestone 12: compact folders

`port/tools/ui-pair` on a folder with `a/b/c/deep.txt`, `lib/one.ts`, `notes.txt`, Dark Modern: click the `a` row, Left, Home, Right, Enter, Escape, right click.

- One row `a/b/c` with dim `/` separators, expanded with `deep.txt`; the focused segment is underlined: `c` after the click, `b` after Left, `a` after Home, at the reference's x positions.
- Enter renames the focused segment: the input holds `b`, selected, in both.
- Right click: the menu targets the row's folder, items as in the section above.
- Still different, not compact folders: the reference shows the pane's title actions, the tree indent guide and the row's path hover while the pointer rests in the view (view and row hover, milestone 11).
- `cargo test -p app`: 891 passed, 1 ignored (`compacts_single_child_folders_into_one_row`, `compressed_navigation_moves_within_its_folders`, `get_context_matches_upstream`).

## Milestone 9: submenus and the Manage menu

`port/tools/ui-pair` with `ARGS=--extensionDevelopmentPath=port/fixtures/extensions/submenus` (a contributed "Port Submenu" first in the Explorer context menu), Dark Modern, and the port's `--screenshot-input "right 120 107; key down; key right"` for the keys (cua's key presses close the port's menu: they reach the window as an activation, a harness limit; the same keys through GPUI's input work).

- Submenu (`raw/contextmenu/submenu-keys-port.png`, the reference's first ui-pair run shows the same): Down focuses Port Submenu, Right opens the submenu beside the menu with First Item focused and the parent item kept highlighted, the submenu's top 4px above its item (`calculateSubmenuMenuLayout`), the chevron (`menu-submenu`) at the item's right. A click on the item opens it too.
- Manage (`raw/contextmenu/manage-menu.png`, reference left): the gear opens the menu right of the bar, bottom aligned with the button, at the reference's place (left edge 45pt, bottom 313pt in both crops). Items: Command Palette... [⇧⌘P] | Keyboard Shortcuts [⌘K ⌘S], Themes >. The reference also has Profiles, Settings, Extensions, Snippets, Tasks (milestones 10, 18, 13, 15) and Layout Density (the compact layout, milestone 11).
- Share: with the github extension's `github.hasGitHubRepo`, Share > Copy vscode.dev Link shows in the Explorer, editor and tab menus (`cargo test -p app the_github_share_items`); the reference shows it for a GitHub clone, the port waits for git repositories (MainThreadSCM, milestone 14) to set the key.
- `cargo test -p app`: 1104 passed, 1 ignored (`submenus_open_on_hover_and_keys_and_run_their_items`, `submenus_open_beside_their_item`, `the_manage_menu_opens_beside_its_button`, `the_manage_menu_lists_the_global_activity_items`).

## Milestone 20: Refactor... and Source Action...

The reference's first editor group holds Change All Occurrences and Refactor... (milestone 8 pair above). The port now registers codeActionCommands.ts's `contextMenuOpts`: Refactor... (order 2) and Source Action... (order 2.1) in `1_modification` while `supportedCodeAction` matches the kind, enabled with `editorHasCodeActionsProvider`. On a plain text model the snippet surround-with provider offers `refactor.surround`, so Refactor... shows, as in the reference shot. Checks: `cargo test -p app editor_menus` (the editor menu now lists Change All Occurrences, Refactor...), `cargo test -p app refactor_and_source_action`.
