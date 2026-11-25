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
