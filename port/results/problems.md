# Problems view

Reference: 1.147.0 dev build on the Studio; port: release `vsrustium`. Both driven by `port/tools/ui-pair` with `FOLDER` a copy of `port/fixtures/problems`, `FILE=bad.json`, Dark Modern, steps `port/fixtures/problems/steps.json` (wait, Cmd+Shift+M, shot). Pair `raw/problems/panel-pair.png`, reference above, port below, the lower 700 rows of the 2x frames at half size.

Same: the Problems tab with its blue count badge, the resource row (twistie, JSON file icon, name, count badge after the name), the marker rows (error icon, message, `json(513)` source and code, `[Ln 3, Col 15]`) at the reference's indents, sort order, the status bar counts.

Differences: the indent guide and the first marker focused and selected when the view opens (the filter box, View as Table and the "..." overflow came later, below).

## Filter, table and panel overflow

Same setup with `port/fixtures/problems/steps-filter.json` (Cmd+F, type `comma`, then `zzz`) and `steps-table.json` (click View as Table at 940,505, then the filter). Pairs: `raw/problems/filter-pair.png`, `raw/problems/table-pair.png`, `raw/problems/overflow-pair.png` (reference, then port tree and port table).

Same: the "..." overflow item with Terminal moved into it at 1024px, the filter box and its placeholder, the filter and View as Table/Tree buttons, the filtered tree with the match on `list.filterMatchBackground`, the resource count following the filter, "Showing 1 of 2" while focused, "No results found with provided filter criteria. Clear Filters.", the table's Code, Message, File and Source columns at the reference's widths, the table match in `list.highlightForeground`.

Differences: the first marker focused and selected in the reference (row focus not ported); the table's column borders show in the reference without the pointer over the table (the port draws them on hover, as `.monaco-enable-motion` does); `filter-pair.png` and `overflow-pair.png` were taken before the filter box got its 200px width (150px in them).
