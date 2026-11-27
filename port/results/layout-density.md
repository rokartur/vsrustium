# Layout density

Verified against VS Code 1.147.0 with `port/fixtures/ui-pair/m11/layout-density.json` in dark mode (FILE=notes.txt), once with default settings and once with SETTINGS `{"window.density.layout": "compact"}`: the parts with the Problems panel open, the Manage menu, and its Layout Density submenu.

- Default: the parts match (pane headers, card gaps, panel tabs, status bar).
- Compact: the 40px rail, the cards joined without gaps or radii, the 1px seams, the square first editor and panel tab corners, the pane headers 1px above the card's bottom, the 26px status bar and the Layout Density submenu with Compact checked all match.

Differences left, outside this row: the reference's panel has a Ports tab (removed by decision), so at the compact panel width its Terminal tab goes to the overflow and the port's does not; the port's Manage menu lacks Settings, Extensions and Tasks.

`cargo test -p app` (2643 passed after the merge of port-integrate) covers the grid (`compact_density_narrows_the_rail_and_centers_the_sashes`), the commands (`the_compact_layout_density_narrows_the_rail_and_joins_the_cards`) and the menu (`layout_density_is_in_the_manage_menu_with_the_current_density_checked`).
