# Navigation (milestone 13)

Reference: 1.147.0 dev build on the Studio. Port: release build. Both through `port/tools/ui-pair` with `FILE=src/main.ts` on a copy of `port/fixtures/workspace`, Dark Modern, 1024x768 at 2x. Every run first types `main()` on line 4 so `main` has a caller. Shots in `/tmp/m13b-all`, `/tmp/m13b-ctx` and `/tmp/m13b-out` on the Studio (not kept).

| Shot | Steps | Result |
| --- | --- | --- |
| peek-def | click `main()` on line 4, Alt+F12 | Same zone, head `main.ts src - Definitions (1)`, preview with line 1 marked, tree row selected. The reference shows the folder as an absolute path because its temp folder is reached through `/var` vs `/private/var`; the port shows `src`. |
| peek-refs | Shift+F12 | Same: `References (2)`, both rows, the second selected, its line outlined in the preview. |
| goto-symbol | Cmd+Shift+O with the cursor outside every symbol | `symbols (1)`, `main`, no active pick in both (fixed in this milestone: the port activated the first pick). |
| workspace-symbols | Cmd+T, `answer` | `No matching workspace symbols` in both (TypeScript's workspace symbols skip constants). |
| call-peek | Peek Call Hierarchy from the Command Palette on `main` | Same: `Callers of 'main'`, `main` expanded with the caller `main.ts src` focused, preview of `main.ts` with the call marked, the outgoing toggle and Close in the head. |
| outline-open | expand Outline, click `main` | Same row inset, selection and icon; the cursor moves to the name without word highlights (fixed in this milestone). The breadcrumbs show `main` in both. |

Differences left:
- Shift+Alt+H runs `references-view.showCallHierarchy` in the reference: the references-view extension's tree view, which needs `MainThreadTreeViews` (a stub). The core peek is reached through Peek Call Hierarchy.
- The Outline view's title actions (Collapse All, the `...` menu with Follow Cursor, Filter on Type, Sort By) are not drawn.
- The reference outline row's icon takes the row's foreground while the tree is unfocused too; the port keeps the symbol color there.
- The peek preview has no overview ruler or scrollbar (it is drawn lines, not an embedded editor).
