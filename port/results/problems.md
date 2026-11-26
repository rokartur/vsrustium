# Problems view

Reference: 1.147.0 dev build on the Studio; port: release `vsrustium`. Both driven by `port/tools/ui-pair` with `FOLDER` a copy of `port/fixtures/problems`, `FILE=bad.json`, Dark Modern, steps `port/fixtures/problems/steps.json` (wait, Cmd+Shift+M, shot). Pair `raw/problems/panel-pair.png`, reference above, port below, the lower 700 rows of the 2x frames at half size.

Same: the Problems tab with its blue count badge, the resource row (twistie, JSON file icon, name, count badge after the name), the marker rows (error icon, message, `json(513)` source and code, `[Ln 3, Col 15]`) at the reference's indents, sort order, the status bar counts.

Differences: the indent guide and the first marker focused and selected when the view opens (the filter box, View as Table and the "..." overflow came later, below).

## Filter, table and panel overflow

Same setup with `port/fixtures/problems/steps-filter.json` (Cmd+F, type `comma`, then `zzz`) and `steps-table.json` (click View as Table at 940,505, then the filter). Pairs: `raw/problems/filter-pair.png`, `raw/problems/table-pair.png`, `raw/problems/overflow-pair.png` (reference, then port tree and port table).

Same: the "..." overflow item with Terminal moved into it at 1024px, the filter box and its placeholder, the filter and View as Table/Tree buttons, the filtered tree with the match on `list.filterMatchBackground`, the resource count following the filter, "Showing 1 of 2" while focused, "No results found with provided filter criteria. Clear Filters.", the table's Code, Message, File and Source columns at the reference's widths, the table match in `list.highlightForeground`.

Differences: the first marker focused and selected in the reference (row focus not ported); the table's column borders show in the reference without the pointer over the table (the port draws them on hover, as `.monaco-enable-motion` does); `filter-pair.png` and `overflow-pair.png` were taken before the filter box got its 200px width (150px in them).

## Filter box (milestone 13 branch, before the panel overflow merged)

Steps `port/fixtures/problems/steps-filter.json` (open Problems, type `comma`, shot, select all, type `zzz`, shot). Pairs `raw/problems/filtered-pair.png` and `raw/problems/empty-pair.png`, reference left, port right.

Same: the filter box in the panel title with `Showing 1 of 2` and `Showing 0 of 2` badges and the filter button, the one marker left with `comma` highlighted, "No results found with provided filter criteria. Clear Filters." with the underlined link.

Differences: the port's panel keeps the Terminal tab in the title and squeezes the filter box, whose left part is cut off; the reference moves Terminal into the "..." overflow. The panel title's overflow belongs to the panel part (milestone 11).

## After the milestone 11 merge

One filter (markers_filter.rs, workbench/view_filter.rs, problems_view.rs) after merging both branches' filters; the table keeps the column sashes. Same steps and fixture, shots in `raw/problems-m13c/` on the Studio. Same as the reference: the 200px filter box with its placeholder, "Showing 1 of 2" and "Showing 0 of 2", the `comma` match, "No results found with provided filter criteria. Clear Filters.", Terminal in the "..." overflow, the table's columns and its filtered row. Differences: focused, the reference's box grows to 253px (280px in the table), the port's to 212px (236px), the room the panel's tab layout leaves (workbench/panel.rs `panel_tabs_that_fit`) being about 40px short; the placeholder clips without an ellipsis.

## Milestone 11 verification (port-m11e-verify)

`FOLDER=<copy of port/fixtures/problems> FILE=bad.json bun port/tools/ui-pair <out> port/fixtures/problems/steps-filter.json` and `steps-table.json` on the Studio, shots in `/tmp/m11e-pairs/problems*/`. The focused filter box measured on the screenshots: tree 649 to 899 in the reference, 651 to 899 in the port (251 and 249px), table 649 to 925 and 651 to 925 (277 and 275px; the DOM says 253 and 280 with the border). Problems, Output and Debug Console shown and Terminal in "..." in both modes in both apps. Fixed for this: the tab layout counts the filter at its 200px resting width and runs again only when the title width, the tabs or the active view change (View as Table no longer brings Terminal back); the filter's 4px margin; the separator's right margin is the bar's gap and Collapse All has its 4px margin (the table's box ended 4px short); the Problems badge as paneCompositePart.css draws it; the placeholder ends with an ellipsis; marker rows with the indent guides and the selected codicon color. Left: the Problems tab is 2px wider than the reference's, so the box starts 2px later; the table's column borders show only on hover in the port (the reference runs with `workbench.reduceMotion: on`, port/notes.md).
