# Language features: outline, signature help, code actions, comments, type hierarchy, TypeScript

Reference: 1.147.0 dev build on the Studio; port: release `vsrustium`. Both driven by `port/tools/ui-pair`, Dark Modern, fixtures in `port/fixtures/language-features` (`FOLDER` a folder with `a.ts`, `FILE=a.ts`). Pairs in `raw/language-features/` (ignored), reference left, port right.

## Outline title actions

Steps `steps-outline.json` (click the Outline header, hover the Explorer). Pair `outline-pair.png`.

Same: the Outline pane expanded with `add`, `total` and `color` and their symbol icons, the header's Expand All icon (no row has children, so `outlineAllCollapsed` holds) and More Actions.

Differences: the port draws the header's focus border after the click; the reference shows no border on a mouse focus.

## Code actions

Steps `steps-params-actions.json`, second shot (Cmd+Up, Right, Cmd+. on `function`). Pair `actions-pair.png`.

Same: the menu under the cursor with the Rewrite and Move headers, the four TypeScript refactorings, the first one focused, the "Learn more about JS/TS refactorings" documentation link, the light bulb, `Ln 1, Col 2`.

## Signature help

Steps `steps-params-actions.json`, first shot (type `add(1, ` on a new last line). Pair `params-pair-before.png`: the port showed neither the auto-closed `)` nor the hints. Text typed on the keyboard reaches the editor through GPUI's input handler (`replace_text_in_range`), which skipped the `type` command and with it auto-closing and the typed-character triggers; it now runs the `type` command for an insert at the cursor (`cargo test -p app keyboard_input_triggers_the_hints`, which fails without the fix). Rerun `params-pair.png`: same `add(1, )` with the auto-closed `)`, the hints widget above with `right: number` highlighted, the error squiggle. Left: the reference shows the light bulb on line 6 at the moment of the shot.

## Comments

`ARGS=--extensionDevelopmentPath=port/fixtures/extensions/comments` (a controller with an unresolved two-comment thread on line 2 and a one-comment thread on lines 4 to 5, then `workbench.action.focusCommentsPanel`), steps `steps-comments.json`. Pair `comments-pair.png`.

Same: the Comments panel with the a.ts resource row, `Ann · Should this return a number?` with the unresolved icon, `1 reply Last reply from Bob`, `Cid · Rename total`, the gutter glyphs on lines 2 and 5.

Differences, fixed after the pair: the range label sat at the row's right edge (it follows the text), the reply line lacked its trailing `·`, the Comments tab came first (order 10 puts it after Terminal), line 5's glyph had no icon (`comment`), the line decorations lane was not 13px wider. Left: the panel's filter box and title actions, the first thread focused in the reference.

Comment thread widget: steps `steps-thread.json` (click line 2's glyph, click "Reply...", type `agreed`, Cmd+Enter) with the fixture's `port-comments.reply` (`comments/commentThread/context`) and `port-comments.resolve` (`comments/commentThread/title`). Pairs `thread-pair.png`, `reply-pair.png`, `replied-pair.png`.

Same: the zone below line 2 with the arrow under `return`, the unresolved border color, the head with the Resolve check and Collapse, Ann's and Bob's comments with the markdown bold `add`, the "Reply..." button, the expanded input with the focus border and the Reply button, Dee's reply added by the extension's command and the form hidden again, the panel's `2 replies Last reply from Dee`.

Differences, fixed after the pairs: the body lines were 2px short (18px per line), the "Reply..." button lacked its outline (review.css's `outline-color: var(--vscode-contrastBorder)` falls back to the text color), the Comments tab had no unresolved badge, the first thread was not selected in the panel. Left: the input is a single-line input box in a 90px frame, not an editor (no scrollbar, no wrapping).

## Type hierarchy

`ARGS=--extensionDevelopmentPath=port/fixtures/extensions/type-hierarchy`, steps `steps-types.json` (cursor on `add`, Show Type Hierarchy). Pair `types-pair.png`: same peek with "No subtypes of 'add'" centered, its focus border, the supertypes/subtypes toggle and Close. The first run (`types-pair-before.png`) lacked the message's focus border and placed it at the top.

## TypeScript in src/main.ts

The default fixture, `FILE=src/main.ts`, steps `steps-main.json` (a new line `const x: string = 1;`, then `mai`). Pair `main-pair.png`: same suggest widget with `main` and its detail `function main(): number`, the error squiggles under `x` and `mai`, `2` errors in the status bar, the tab and the Explorer. Left: the port's Explorer shows the NPM Scripts view, which the reference hides; the reference's Explorer icon has the dirty-file badge.
