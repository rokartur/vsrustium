# Language features: outline, signature help, code actions, comments

Reference: 1.147.0 dev build on the Studio; port: release `vsrustium`. Both driven by `port/tools/ui-pair`, Dark Modern, fixtures in `port/fixtures/language-features` (`FOLDER` a folder with `a.ts`, `FILE=a.ts`). Pairs in `raw/language-features/` (ignored), reference left, port right.

## Outline title actions

Steps `steps-outline.json` (click the Outline header, hover the Explorer). Pair `outline-pair.png`.

Same: the Outline pane expanded with `add`, `total` and `color` and their symbol icons, the header's Expand All icon (no row has children, so `outlineAllCollapsed` holds) and More Actions.

Differences: the port draws the header's focus border after the click; the reference shows no border on a mouse focus.

## Code actions

Steps `steps-params-actions.json`, second shot (Cmd+Up, Right, Cmd+. on `function`). Pair `actions-pair.png`.

Same: the menu under the cursor with the Rewrite and Move headers, the four TypeScript refactorings, the first one focused, the "Learn more about JS/TS refactorings" documentation link, the light bulb, `Ln 1, Col 2`.

## Signature help

Steps `steps-params-actions.json`, first shot (type `add(1, ` on a new last line). Pair `params-pair-before.png`: the port showed neither the auto-closed `)` nor the hints. Text typed on the keyboard reaches the editor through GPUI's input handler (`replace_text_in_range`), which skipped the `type` command and with it auto-closing and the typed-character triggers; it now runs the `type` command for an insert at the cursor (`cargo test -p app keyboard_input_triggers_the_hints`). Rerun pending.

## Comments

`ARGS=--extensionDevelopmentPath=port/fixtures/extensions/comments` (a controller with an unresolved two-comment thread on line 2 and a one-comment thread on lines 4 to 5, then `workbench.action.focusCommentsPanel`), steps `steps-comments.json`. Pair `comments-pair.png`.

Same: the Comments panel with the a.ts resource row, `Ann · Should this return a number?` with the unresolved icon, `1 reply Last reply from Bob`, `Cid · Rename total`, the gutter glyphs on lines 2 and 5.

Differences, fixed after the pair: the range label sat at the row's right edge (it follows the text), the reply line lacked its trailing `·`, the Comments tab came first (order 10 puts it after Terminal), line 5's glyph had no icon (`comment`), the line decorations lane was not 13px wider. Left: the panel's filter box and title actions, the first thread focused in the reference.
