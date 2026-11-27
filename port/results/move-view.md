# Move View

Verified against VS Code 1.147.0 with `port/fixtures/ui-pair/m11/move-view.json` in dark mode: the Move View picker, the destination pickers, Outline into a new panel entry, Timeline into a side bar entry and beside Problems, Search into the Explorer. All 8 steps match. `cargo test -p app` (2594 passed) covers persistence, destinations, visibility, panel placement and focus.

The picker lists only visible views (`visibleViewDescriptors`, so hidden Open Editors stays out). A moved view opens focused (`openView(id, true)`): Search shows its input focused, its Replace row and its title actions. Differences left: the reference lists the Ports view, removed by decision; contributed extension containers take no views from other containers.
