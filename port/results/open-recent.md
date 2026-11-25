# Open Recent picker

## Milestone 12: Ctrl+R

Reference: 1.147.0 dev build on the Studio; port: release `code-gpui`. Both driven by `ui-pair` (cua-driver) with fixture `port/fixtures/workspace`, Dark Modern, 1024x768 at 2x: click the editor, Ctrl+R, shoot (`picker`).

- Both show the placeholder "Select to open (hold Cmd-key to force new window or Option-key for same window)", the `workspace` row with its folder path as given (`/var/folders/...`, no realpath), the "folders" separator and the Remove from Recently Opened button on the focused row.
- The focused row's button was gray (`icon.foreground`) in the port and white in the reference: `quickInputList.focusIconForeground` is unset by default, so the list's `.codicon` rule holds an invalid CSS variable and the icon takes the row color. The port now falls back to `quickInputList.focusForeground` (quick_input_view.rs).
- `magick compare -fuzz 2% -metric AE` of the whole window: 5402 of 3.1M pixels, from the longer temp folder name of the reference run (the "folders" label moves) and glyph antialiasing.
- `cargo test -p app quick_input`: 28 passed; `cargo test -p app recently_opened menubar`: see commit 83755c04bff.
