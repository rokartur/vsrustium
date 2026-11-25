# Problems view

Reference: 1.147.0 dev build on the Studio; port: release `vsrustium`. Both driven by `port/tools/ui-pair` with `FOLDER` a copy of `port/fixtures/problems`, `FILE=bad.json`, Dark Modern, steps `port/fixtures/problems/steps.json` (wait, Cmd+Shift+M, shot). Pair `raw/problems/panel-pair.png`, reference above, port below, the lower 700 rows of the 2x frames at half size.

Same: the Problems tab with its blue count badge, the resource row (twistie, JSON file icon, name, count badge after the name), the marker rows (error icon, message, `json(513)` source and code, `[Ln 3, Col 15]`) at the reference's indents, sort order, the status bar counts.

Differences, all `shortcut:` in `workbench/problems_view.rs` or the handoff: the filter box with its filter and View as Table actions, the panel's "..." overflow and Views and More Actions menu (the reference moves Terminal into it at this width), the indent guide, and the first marker focused and selected when the view opens.
