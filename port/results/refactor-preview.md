# Refactor Preview

Reference: 1.147.0 dev build on the Studio; port: release `vsrustium`. `port/tools/ui-pair` with `ARGS=--extensionDevelopmentPath=<repo>/port/fixtures/extensions/refactor-preview FILE=src/main.ts`, steps `port/fixtures/extensions/ui-refactor-preview.json`. The fixture extension applies, at startup, a workspace edit that replaces 42 in `src/util.ts` and `src/main.ts` and creates `src/answer.ts`, every edit with `needsConfirmation`. Pair `raw/refactor-preview/preview-pair.png`, reference left.

Same: the Refactor Preview tab added last in the panel and active, the Group Changes By Type title action, the files by name with their folder, "(creating)" on the new file, the unchecked checkboxes of edits that need confirmation, the expanded files, the text edit rows with the removed text struck through on `diffEditor.removedTextBackground` and the new text on `diffEditor.insertedTextBackground`, Apply and Discard.

Differences: the Ports tab (the port has no Ports view, milestone 19); the buttons were 13px wider, without their border, and 5px lower in this pair; after button.css's padding and `button.border` they match the reference (`raw/refactor-preview/buttons-after.png`, reference top, from a port-only run at another window height); the text edit rows 4px to the right.
