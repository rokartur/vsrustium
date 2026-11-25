# Inlay hints, color decorators, inline completions

Reference: 1.147.0 dev build on the Studio; port: release `vsrustium`. Both driven by `port/tools/ui-pair`, Dark Modern, fixtures in `port/fixtures/language-features`. Pairs in `raw/inlay-hints/` (ignored), reference above, port below.

## Inlay hints and color picker

`FOLDER` a folder with `a.ts`, `FILE=a.ts`, `SETTINGS='{"typescript.inlayHints.variableTypes.enabled":true,"typescript.inlayHints.parameterNames.enabled":"all"}'`, steps `steps-color.json` (hover the color box, then Show or Focus Standalone Color Picker).

Same: the TypeScript hints `: number` after `total` and `left:` `right:` before the arguments, in `editorInlayHint.*` colors with their background, the columns after them shifted by the hint widths; the 10px color box before `#ff0000`; the hover picker under the box with the header in the color and its label, the saturation box, opacity and hue strips; the standalone picker below the cursor line with `rgb(0, 0, 0)`, the close button and Insert.

Differences: the standalone picker is about 8px narrower than the reference's (the saturation box takes the remaining width of a 314px widget in the port).

## Inline completions

`FILE=a.html`, `SETTINGS='{"emmet.useInlineCompletions":true}'`, steps `steps-ghost.json` (type `ul`, Tab).
