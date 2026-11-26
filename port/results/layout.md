# Workbench layout

## Milestone 11: part grid, sashes, hidden parts

Reference: 1.147.0 dev build on the Studio; port: release `code-gpui`. Both driven by `ui-pair` with fixture `port/fixtures/workspace`, Dark Modern, 1024x768 at 2x, steps `drag (306,300) to (380,300)`, Cmd+B, Cmd+B, `drag (380,300) to (90,300)`. Pairs in `raw/layout/grid-*.png`, reference left, port right. Differences counted with `magick compare -metric AE -fuzz 6%` over the 2048x1536 frames.

| Step | AE | Cause |
|---|---|---|
| start | 649 | glyph antialiasing in the side bar and title (same rows and columns) |
| drag-held | 2290 | the mid-drag frame: cua-driver samples the drag at a different moment on each side (sash 4 pt apart in one run, 1 pt in another) |
| dragged | 652 | glyph antialiasing; side bar edge, sash and highlight on the same device columns (754 to 762) |
| hidden | 274 | glyph antialiasing |
| shown | 4031 | the reference highlights the sash under the still pointer after Cmd+B; gpui reports no hover after keyboard input (`Hitbox::is_hovered` returns false while `last_input_was_keyboard`), so the port highlights it on the next mouse move |
| snapped | 274 | glyph antialiasing; the side bar snapped hidden in both |

Measured: the default grid matches the reference DOM (activity bar 4,35 44x701; side bar 48,35 256x701; editor from 308; status bar 0,740 1024x28). The vertical sash highlight stops 4 pt above the status bar (`--modern-ui-floating-card-statusbar-margin`), device rows 70 to 1471 in both (the reference antialiases the end rows). The title bar's side bar toggle follows the side bar's visibility (`layout-sidebar-left-off` when hidden).

Fixed after the first pairs: the sash highlight ran down to the status bar; the title bar toggle followed the open folder instead of the side bar; a sash's leave could cancel another sash's pending hover.

Checks (Studio):

- `cargo test -p app -- part_grid layout split_view`: 27 passed.
- `cargo test -p app`: 1047 passed, 0 failed, 1 ignored.

## Item 2: panel, side bar, view panes, compact activity bar, empty window

Same setup; pairs in `raw/layout/<run>-<step>.png`, reference left, port right. AE at 6% fuzz over 2048x1536.

| Run | Step | AE | Cause |
|---|---|---|---|
| panes | collapsed | 1574 | glyph antialiasing |
| panes | expanded (click Outline, click Timeline) | 2548 | glyph antialiasing; 366/148/148 pane sizes as the reference |
| panes | sash-held, sash-dragged (Explorer/Outline sash 434 to 334) | 2281, 2293 | glyph antialiasing |
| panes | keys (Up, Space) | 6250 | Space toggled twice in the port (fixed below); the reference also highlights the sash under the still pointer (gpui hover limit as above) |
| welcome | empty window | 11929 | "You can clone a repository locally." and Clone Repository come from the git extension (not ported) |
| top | compact, compact-search | 1338, 3268 | glyph antialiasing; Search view input rows |
| bottom | compact, compact-search | 1715, 3247 | glyph antialiasing; Search view input rows |

Fixed after the pairs: Space toggled a pane header twice (the key handler and gpui's keyboard click on key up); Enter and Space now go through `on_click` only, covered by `explorer_pane_headers_toggle_with_the_mouse_and_keys_and_their_state_is_restored`.

The panel's tab row is not compared: no panel view is registered before the Output view moves in (item 3).

Checks (Studio):

- `cargo test -p app -- view_panes explorer_pane part_grid layout split_view side_bar empty_view panel`: 37 passed.

## Editor groups (milestone 11 item 1)

`port/tools/pixel-check` window `split` (notes.txt, then Cmd+\ splits it into a second group), Dark Modern, 1024x768 at 2x: 15190 different and 38025 anti-aliased of 3145728 pixels. Pair in `raw/layout/split-reference.png` and `raw/layout/split-port.png`.

What differs:

- The reference's active group shows the breadcrumbs' editor type picker ("Text Editor" with a chevron, editorTypePicker.ts); the port has no editor type picker (breadcrumbs, milestone 8).
- The part sash's three-dot gripper (partGrid.css `.part-grid-gap-sash::after`) was missing in this run; added after it, with `workbench.sash.autoHideGrippers` and `modernSash.gripForeground`.
- Minimap and scrollbar edges in both groups, the tab's right edge, the activity bar's selected icon and glyph antialiasing.

Checks (Studio): `cargo test -p app editor_groups` (split, cursor sync across groups, close empty group, close group merging, layouts, Go Back/Forward, Reopen Closed Editor: 7 passed), `cargo test -p app editor_grid` (gridview.ts and grid.test.ts cases: 14 passed), `cargo test -p app navigation` (historyService.test.ts stack cases: 16 passed), `cargo test -p app host_sync` (both groups reach the host on one document: 5 passed).

## Output and Terminal in the panel (milestone 11 item 1)

`port/tools/pixel-check` window `panel` (the split, then Cmd+J), Dark Modern, 1024x768 at 2x: 46704 different and 52306 anti-aliased of 3145728 pixels, from 97109 before two fixes found in the first run: the Terminal view is the default panel view (terminal.contribution.ts `isDefault`) and creates a terminal when shown (terminalView.ts `_initializeTerminal`); the 4px gap above a bottom panel is the panel's margin, not the editor's (floatingPanels.css), so the panel card is 248px high as in the reference. Pair in `raw/layout/panel-reference.png` and `raw/layout/panel-port.png`.

What differs:

- The reference's panel has Problems, Debug Console and Ports views; the port registers Output and Terminal only.
- The shell's own output: fish prints a suggestion line in the reference run, and the port's terminal shows the folder as `/private/tmp/...` where the reference keeps the `/tmp` symlink.
- The breadcrumbs' editor type picker, as in the split window.

Checks (Studio): `cargo test -p app editor_groups` (9 passed, with `the_output_view_shows_in_the_panel_and_takes_the_focus_from_the_groups` and the drop target split areas), `cargo test -p app output_view` (8 passed), `cargo test -p app terminal` (9 passed), `cargo test -p app part_grid` (5 passed), `cargo test -p app panel` (6 passed).

## Part positions (session m11f)

`ui-pair` with `port/fixtures/ui-steps/layout-positions.json`, Dark Modern, 1024x768 at 2x: Cmd+J, then from the Command Palette Move Panel Right, Move Panel Left, Move Panel To Top, Set Panel Alignment to Justify, Toggle Primary Side Bar Position, Set Panel Alignment to Left, then Cmd+B. AE at 6% fuzz over 2048x1536.

| Step | AE | Cause |
|---|---|---|
| bottom | 10653 | the shell's own output (fish's suggestion line) and the Ports tab |
| panel-right | 7723 | the shell's output; the terminal's dimension overlay after the resize |
| panel-left | 7634 | the same |
| panel-top | 12649 | the same |
| justify | 14087 | the shell's output and the Ports tab; the Explorer's Outline header 3 pt lower in the port |
| side-bar-right | 14285 | the same |
| align-left | 16641 | the same, plus the terminal's dimension overlay |
| right-rail | 16018 | the same |

Every part sits where the reference puts it: a side panel beside the editor, the top panel above it, a justified panel under (or over) the side bars, the left-aligned panel under the side bar only, the side bar and activity bar on the right; the maximize action shows only where `panelMaximizationSupportedWhen` allows it. The first two runs froze or dropped keys midway while other sessions drove windows on the Studio; the third ran clean.

Checks (Studio): `cargo test -p app -- layout part_grid`: `the_panel_moves_and_aligns_and_comes_back_where_it_was`, `every_arrangement_lays_out_with_any_part_hidden` and the grid's 16 layout tests pass.

## Zen mode and centered layout (session m11f)

`ui-pair` with `port/fixtures/ui-steps/layout-zen.json`, FILE=notes.txt, `zenMode.fullScreen: false`, Dark Modern, 1024x768 at 2x: Toggle Centered Layout, a drag of the left centered sash, Toggle Centered Layout, Cmd+K Z, Toggle Zen Mode, Toggle Status Bar Visibility. AE at 6% fuzz over 2048x1536 (the clean run).

| Step | AE | Cause |
|---|---|---|
| start | 1169 | glyph antialiasing |
| centered | 4760 | the editor's margins and sash lines at the reference's places; the reference's fading scrollbars |
| uncentered | 4199 | glyph antialiasing, the fading scrollbars |
| zen | 9221 | the centered margins the asymmetric sash drag had left (fixed below); side bars, activity bar and status bar hidden in both |
| zen-exited | 4207 | the parts back in both |
| no-status-bar | 4083 | the status bar hidden in both |

Found by the pairs and fixed: a drag of a centered sash moved only that margin, where splitview.ts `inverseAltBehavior` moves both (Alt moves one); covered by `a_sash_moves_both_margins_unless_alt_is_held` (cargo test -p app -- centered_layout). The drag frames themselves could not be compared after the fix: the reruns lost keys and changed the capture scale while other sessions drove the Studio's screen. Escape Escape leaves zen mode in the port only with the window in front: cua-driver's background keys leave the window inactive, and the chord checker then drops the chord after its 500 ms tick, as upstream's `_scheduleLeaveChordMode` does; the fixture leaves zen mode through the Command Palette, and `zen_mode_hides_the_parts_and_puts_back_the_ones_it_hid` sends Escape Escape with an editor focused.

## Activity badges (session m11f)

`ui-pair` with `port/fixtures/ui-steps/activity-badge.json` (6 s, one shot) on a copy of `port/fixtures/workspace` made a git repository with one changed and one new file: AE 1808 over 2048x1536, glyph antialiasing only; the Source Control item's `2` badge has the reference's size, color and place.

## Secondary side bar (port-m11e-verify)

`bun port/tools/ui-pair <out> port/fixtures/ui-steps/auxiliarybar.json` on the Studio (no folder fixture), shots in `/tmp/aux/`, 2x: Alt+Cmd+B, hover Maximize Secondary Side Bar, click it, click it again, Cmd+Shift+M, drag the Problems tab from 345,505 onto the secondary side bar (shot while held), drop.

Same: the empty part (title with Maximize Secondary Side Bar and Hide Secondary Side Bar, "Drag a view here to display."), the Maximize Secondary Side Bar tooltip below the action, right-aligned at the window edge; maximized, the side bar, the editor and the panel hidden with the part filling the window and the toggled action's `actionBar.toggledBackground`; restored, the earlier layout and width; the narrow panel (aux bar open) with the Problems filter at the top of the view; while dragging, the empty part's content in `editorGroup.dropBackground`; dropped, the Problems view in the secondary side bar (its tab, View as Table, Collapse All, the separator, Maximize, Hide, the filter at the top of the view, the message wrapped), the panel on Terminal with Output and the "..." overflow.

Differences: the reference keeps the tooltip after the click (Restore Secondary Side Bar, and Maximize after the second click), the port hides it on click; the port draws the dragged tab's label as GPUI's drag preview, the reference's OS drag image is not in the capture; the port shows the sash grips while dragging; the terminal's own content and its "..." action (terminal rows).
