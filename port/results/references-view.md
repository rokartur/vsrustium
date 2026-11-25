# References view

Reference: 1.147.0 dev build on the Studio; port: release `vsrustium`. `port/tools/ui-pair` with `FILE=src/main.ts` in the default workspace fixture, steps `port/fixtures/references-view/steps.json` (two Alt+Right and Right to `main`, Shift+Alt+F12). Pair `raw/references-view/references-pair.png`, reference left.

Same: the References container appearing in the activity bar and opening, the merged title "References", the Refresh, Clear and Collapse All title actions, "1 result in 1 file", the file row with its icon and relative folder, the reference row with `main` on `list.filterMatchBackground` and its inline Dismiss action, selected and focused.

Differences: the indent guide (tree indent guides are not ported in extension tree views), the rows 2px lower in the port.

## Find All References, call and type hierarchy (after the milestone 11 merge)

`port/tools/ui-pair` with `FOLDER` a copy of `port/fixtures/language-features`, `FILE=a.ts`, `ARGS=--extensionDevelopmentPath=<repo>/port/fixtures/extensions/type-hierarchy`, steps `port/fixtures/references-view/steps-hierarchies.json` (click on `add`, Shift+Alt+F12, shot; Shift+Alt+H, shot; Types: Show Type Hierarchy, shot). Shots in `raw/references-m13c/` on the Studio, reference and port.

Same: References with "2 results in 1 file", the a.ts file row and both reference rows with `add` on `list.filterMatchBackground`, the first selected; "References: Callers Of" with the Show Outgoing Calls, Refresh, Clear and Collapse All actions, the expanded `add` root selected with its Dismiss action, the collapsed `a.ts /tmp/m13c-refs` caller; "References: Subtypes Of" with Show Supertypes, the `add a.ts` root with Dismiss.

Differences: the reference ends clipped row labels and the view title with an ellipsis ("References: Subtype...", "function add(left: number, ..."), the port clips them; on the References tree the reference shows the selected row's inline Dismiss action, the port only on hover; the tree indent guide; the rows 2px lower in the port.
