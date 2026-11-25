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

## HTML and CSS

`FOLDER` a copy of port/fixtures/language-features. `FILE=a.html`, steps `steps-html.json` (`<di` on line 2): pair `html-pair.png`, the same four tag completions (`dialog`, `div`, `details`, `datalist`) and breadcrumbs `a.html > body > di`. The reference's title bar has the Show Preview and "Text Editor" items of other contributions. `FILE=b.css`, steps `steps-css.json` (`col` in a rule): pair `css-pair.png`, the same property list with `columns: ;` as Emmet Abbreviation, the same 2 errors.

## References view

Steps `steps-references.json` (cursor on `add`, Shift+Alt+F12). Pair `references-pair.png`: the reference shows the References view with 2 results; the port does nothing, `references-view.tree.focus` is not a command since the extension's views container is not drawn (milestone 11).

## Paste with providers

`FOLDER` a copy of port/fixtures/language-features, `FILE=d.md`, steps `steps-paste.json` (copy `https://example.com`, select `word`, paste, then Cmd+.). Pair `paste-pair.png`: the markdown extension's "Insert Markdown Link" edit applied as a snippet with `word` selected, the lightbulb and the post-paste widget at the end of the pasted text. Pair `paste-menu-pair.png`: the same menu with Insert Markdown Link checked, Insert Plain Text and the "Configure preferred paste action..." bar. Fixed after the first pair: the widget sat at the cursor, not at the end of the pasted range, and the menu had no configure bar. Left: the menu is 2px higher and 6px narrower.

Drop into the editor (Shift while dragging files) is checked by `cargo test -p app paste_edits` only: ui-pair cannot drag files from the Finder.

## PHP

`FILE=c.php`, steps `steps-php.json` (`str_re` on line 2). Pair `php-pair.png`: the same twelve completions with `str_repeat`'s signature detail.

## Extension manifests

`FOLDER` port/fixtures/language-features/manifest, `FILE=package.json`, steps `steps-manifest.json`. Pair `manifest-pair.png`: extension-editing's "SVGs are not a valid image source." warning under `images/icon.svg` and in Problems, as in the reference. Left, outside milestone 13: the NPM Scripts view and the Terminal tab show in the port only.

## JSON settings

`FOLDER` a copy of port/fixtures/language-features/settings, `FILE=.vscode/settings.json` (`"editor.fontSize": "big"`), steps `steps-json.json` (Cmd+Shift+M). Pair `json-pair.png`: the squiggle under `"big"`, `Incorrect type. Expected "number".` in Problems with its first row selected and focused, the warning count in the status bar, as in the reference. The first run left the row unselected; the Problems view now selects the first marker when it takes focus (markersView.ts `focus`).

## Links

`FILE=d.md`, steps `steps-links.json`. Pair `link-pair.png`: `https://example.com` underlined in the link color, as in the reference. The hover the step aims at shows in neither window, so the link hover is not compared.

## Problems quick fix

`FOLDER` a copy of port/fixtures/language-features/quickfix, `FILE=e.ts` (`Mathh.max(1, 2);`), steps `steps-problems-fix.json` (Cmd+Shift+M, Cmd+.). Pair `problems-fix-pair.png`: the selected marker's row with its light bulb, the "Change spelling to 'Math'" menu below it, the related information row, the squiggle and the editor light bulb, as in the reference. Fixed after the pair: the row's light bulb was the blue autofix icon; the reference draws the plain one in the row's color. Left: the reference's row has the focused selection colors while the menu is open, the port the unfocused ones.

## Snippets file schema

`FOLDER` a copy of port/fixtures/language-features/snippets, `FILE=.vscode/a.code-snippets` (`"prefix": 1`), steps `steps-snippets-schema.json`. Pair `snippets-schema-pair.png`: `Incorrect type. Expected one of string, array.` at Ln 3, Col 13 from the served `vscode://schemas/snippets`, as in the reference.

## Go to Next Problem

`FOLDER` a copy of port/fixtures/language-features/quickfix, `FILE=e.ts`, steps `steps-goto-error.json` (F8). Pair `goto-error-pair.png`: the zone below line 1 with the error border and arrow, `e.ts 1 of 1 problem` with the next, previous and close actions, the message with `ts(2552)` and the related `lib.es5.d.ts(767, 13)` link, the word selected at the marker, as in the reference. Neither draws a resize sash (gotoErrorWidget.ts creates its zone without `isResizeable`).

## Semantic tokens and language status

`FOLDER` a copy of port/fixtures/language-features/semantic, `FILE=f.ts` (`const value`, used twice), steps `steps-semantic.json`. Pair `semantic-pair.png`: the readonly `value` in #4FC1FF on all three lines, as in the reference. Pair `language-status-pair.png` (hover on `{}`): the same two TypeScript rows, "No tsconfig" with Configure TSConfig and "6.0.3 - TypeScript version" with Select Version, each with its pin. Left: the reference shows the `const` word highlight and a light bulb on line 1, the port neither; not traced yet, likely the reference asking before the TypeScript server was ready (a textual highlight); the hovered `{}` item has `statusBarItem.hoverBackground` in the reference only (the status bar part draws no item hover).

## Keybindings schema

Empty `FOLDER`, steps `steps-keybindings.json`: Preferences: Open Keyboard Shortcuts (JSON), then `{"key": "f9", "command": "editor.action.refactor", "args": {}}` typed into the array. Pair `keybindings-schema-pair.png`: "Missing property "kind"." at Ln 3, Col 60, the only problem, in both. An earlier run that opened Default Keybindings showed the port failing to load `vscode://schemas/inlineCompletionProviderIdArgs`, now served from the inline completion providers (`inlineCompletionProviderGetMatcher`). Left: the Default Keybindings content itself (the port lists fewer commands), which is the keybinding service's, not this schema's.

## Comment editing and commands

`ARGS=--extensionDevelopmentPath=port/fixtures/extensions/comments`, now with a reaction handler, a `+1` reaction (2, reacted) on Ann's comment, `port-comments.edit` (`comments/comment/title`, `comment == editable`) and `port-comments.save` (`comments/comment/context`). Steps `steps-comment-add.json`: Expand All Comments, click the `+1` reaction, click Ann's edit action, Cmd+Enter.

Pairs `comment-expanded-pair.png`, `comment-reacted-pair.png`, `comment-editing-pair.png`, `comment-saved-pair.png`. Same: both threads expanded, the edit icon on Ann's comment only, `+1 2` with the reaction picker after it and the picker alone under Bob's, `+1 1` after the toggle, the edit form with the text and the caret at its end and the primary Save button, `(edited)` after Save. Differences, fixed after the pairs: the edit input sat 4px low and used 13px text (the comment editor takes `editor.fontSize`, 12px, as the reply input now does too), the picker was missing. Left: the reference's input is an editor with a scrollbar; the Save button is 3px taller in the port.

Steps `steps-comment-commands.json`: Go to Line 3, Add Comment on Current Selection, Escape, Toggle Editor Commenting. Pairs `comment-add-pair.png` and `comment-toggled-pair.png`: the template thread "Start discussion" with the trash action and the reply input focused with "Type a new comment" (fixed: the port showed the collapsed button, as it revealed only templates already expanded), then no glyphs, ranges, threads or reserved gutter after the toggle. Not matched: after Escape the reference kept the thread while the port collapsed it (and deleted it, being empty), as `workbench.action.hideComment` reads in commentsEditorContribution.ts; likely the synthesized Escape never released the reference's keybinding hold mode, which the command awaits first. The `+` glyph on hover is checked by cargo test only: ui-pair's pointer moves did not reach the port's gutter rows.
