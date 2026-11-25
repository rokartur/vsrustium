# Inlay hints, color decorators, inline completions

Reference: 1.147.0 dev build on the Studio; port: release `vsrustium`. Both driven by `port/tools/ui-pair`, Dark Modern, fixtures in `port/fixtures/language-features`. Pairs in `raw/inlay-hints/` (ignored), reference above, port below.

## Inlay hints and color picker

`FOLDER` a folder with `a.ts`, `FILE=a.ts`, `SETTINGS='{"typescript.inlayHints.variableTypes.enabled":true,"typescript.inlayHints.parameterNames.enabled":"all"}'`, steps `steps-color.json` (hover the color box, then Show or Focus Standalone Color Picker).

Same: the TypeScript hints `: number` after `total` and `left:` `right:` before the arguments, in `editorInlayHint.*` colors with their background, the columns after them shifted by the hint widths; the 10px color box before `#ff0000`; the hover picker under the box with the header in the color and its label, the saturation box, opacity and hue strips; the standalone picker below the cursor line with `rgb(0, 0, 0)`, the close button and Insert.

Differences: the standalone picker is about 8px narrower than the reference's (the saturation box takes the remaining width of a 314px widget in the port).

## Inline completions

`FILE=a.html`, `SETTINGS='{"emmet.useInlineCompletions":true}'`, steps `steps-ghost.json` (type `ul`, Tab).

Pair `raw/inlay-hints/ghost-pair.png` (reference left, port right), quick suggestions off. Same: neither shows ghost text after `ul` within the run's wait, the Emmet extension answers no inline completion there, so the pair does not exercise the ghost text; it is checked by `cargo test -p app ghost_text inline_completions` instead.

## Language detection

`FILE=a.ts`, steps `steps-detect.json` (Cmd+N, type three lines of Python). Pair `raw/inlay-hints/detect-pair.png`. Same: the untitled tab named after its first line, the text, Plain Text in the status bar in both (neither switches the language for this sample within 3 s). The model itself is checked by `cargo test -p app language_detection` (the Bun worker on `@vscode/vscode-languagedetection`).
