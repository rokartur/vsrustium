# Move View

Verified against VS Code 1.147.0 with `port/fixtures/ui-pair/m11/move-view.json` in dark mode. The fixture compares the Move View picker, moving Search and Timeline, placing Outline beside Problems, and moving views between containers. `cargo test -p app view_locations` covers persistence, destinations, visibility, panel placement and focus.

The picker omits views that are not currently visible, matching the reference. Side-by-side panel panes match at the captured viewport. One known limit remains: contributed extension views render their own container panes and do not accept views from other containers.
