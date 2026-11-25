# Code actions, code lens and signature help

## Milestone 13

Reference: 1.147.0 dev build on the Studio. Port: release build. Both through `port/tools/ui-pair` (cua-driver, Dark Modern, 1024x768 at 2x) with the fixture `port/fixtures/workspace` and `src/main.ts` open; the code lens pair opens a copy with `.vscode/settings.json` turning on `typescript.referencesCodeLens.enabled` and `typescript.implementationsCodeLens.enabled`. Steps: click on `main` in line 1, wait 3.5s, Cmd+., Down, Escape; for the lenses, wait 12s.

- Light bulb (`ca-bulb`): both show the yellow `lightbulb` at the start of line 2 (line 1 has no indentation, line 2 has), same pixels.
- Menu (`ca-menu`, `ca-down`): same rows (Rewrite: Convert named export to default export, Infer function return type; Move: Move to file, Move to a new file), the wrench icons, the "Learn more about JS/TS refactorings" action bar, the focus on the first row then the second. First pair: the port drew no wrench (`wrench` is the `symbol-property` glyph, mapped in `symbol_icons::svg_name`), opened 2px higher (contextview.ts makes a point anchor a 1x2 box) and 5px narrower (the empty additional badges box takes a gap). Second pair: border rows at the same y (110pt); the Move header 2px lower (the header's margin moved the rows after it; it now moves only the header). Width within 1px.
- Code lens (`lens`): "0 references" above line 1 at the same place, line 1 pushed down by the same 15px zone (10px font, 1.5 factor), text within 0.5px (`vertical-align: sub`). The port showed no light bulb there: the code action providers registered after the model opened and the port did not ask again; it now asks when a provider registers (`language_providers_changed`).
- Signature help: the reference pair (`hints-open`, `hints-timeout`) ran; the port side stopped twice at a macOS SecurityAgent prompt on the first click after a fresh release build.
- Tests: `cargo test -p app -- code_lens code_actions view_zones editor_message editor_status quick_input_view symbol_icons`: 18 passed.
