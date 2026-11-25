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
