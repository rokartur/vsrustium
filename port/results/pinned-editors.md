# Pinned editors and the Open Editors view (milestone 12 gaps)

ui-pair against VS Code 1.147.0, dark theme, folder `port/fixtures/workspace`, file `notes.txt`, release build.

## port/fixtures/ui-pair/m12/pinned-tabs.json

Opens main.ts, util.ts and guide.md, returns to util.ts, pins it (Cmd+K Shift+Enter), types into it, hovers, focuses Open Editors, right clicks the tab, closes with Cmd+W and Cmd+K Cmd+W.

- `tabs`, `pinned`, `pinned-dirty`: tab order, the `pinned` and `pinned-dirty` actions, and tab positions match. Before this change the port put 8px after every inactive tab; the reference puts it only after the last tab.
- `open-editors`: the sticky row with its `pinned-dirty` action, the focused selection and the `src` description at full opacity (iconlabel.css `.selected .label-description`) match.
- `tab-menu`: matches item for item, Copy Breadcrumbs Path included (`breadcrumbs.copyPath`), except Move into New Window and Copy into New Window (the port has no auxiliary windows, port/notes.md).
- `close-sticky-keyboard`: Cmd+W on the sticky util.ts activates guide.md and keeps util.ts (`preventPinnedEditorClose` keyboardAndMouse), as in the reference.
- `close-all`: Cmd+K Cmd+W keeps only the sticky util.ts, as in the reference.
- Left: after a native context menu closes over the pin action, the reference shows its hover background and the "Unpin Editor" tooltip without pointer motion. The port now dispatches GPUI's mouse-move event after the menu-less frame is drawn, matching Chromium's synthetic mouse move after layout.

## port/fixtures/ui-pair/m12/pinned-tab-sizing.json

Pins main.ts and util.ts, opens guide.md (`sizing`), then activates util.ts (`sizing-active`). Run once for each setting:

- `workbench.editor.pinnedTabSizing: compact`: the sticky tabs are 28px and show only the icon, the active one connected to the editor; matches.
- `workbench.editor.pinnedTabSizing: shrink`: the sticky tabs are 80px with the label cut by an ellipsis and the pin action; the geometry matches. Left: in the 16.5px left for the name, Chrome keeps the first letter and clips the ellipsis after it ("m…"); GPUI's truncation keeps only letters that fit with the whole ellipsis, so it draws "…" alone.
- `workbench.editor.pinnedTabsOnSeparateRow: true`: the sticky tabs sit in a 30px bar above the strip; the active sticky tab is a 24px fill inset 2px with rounded corners and the lower strip has no active tab; matches.

Reference measurements come from `port/tools/ref-dom` dumps of `.part.editor .title`: the compact tab is 28px with the icon at +5, the shrink tab is 80px, upper-bar tabs are 28px high with the label and icon 2px from the top and the action 4px from the top.

## Tests

`cargo test -p app -- outline_pane sticky pinned_editors open_editors editor_menus editor_groups explorer_actions`: 75 passed.
