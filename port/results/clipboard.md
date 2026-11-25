# Clipboard

## Milestones 5 to 7: editor/contrib/clipboard

`port/tools/ui-pair` on `port/fixtures/workspace` with `FILE=src/main.ts`, Dark Modern; the `clipboard` step saves the pasteboard text and its `public.html`.

- Copy with an empty selection (end of line 2), a selection (`return 42;`) and Select All: the text and the HTML (`<meta charset='utf-8'>`, the editor color, background and font, one span per token with the Dark Modern token colors, `<br>` for the empty last line) are byte-identical in both (`/tmp/clip-out`).
- Column block: Cmd+Up, 5 x Shift+Alt+Cmd+Right, 2 x Shift+Alt+Cmd+Down, Cmd+C gives `expor\n\tr\n}` with no HTML in both; Cmd+Down, Cmd+V pastes it as a block padded to its width (`}    `), the document text the same in both; Cmd+Up, Cmd+X (whole line), Cmd+Down, Cmd+V gives the same document and HTML in both (`/tmp/clip-out2`). The pasted frame differs in 3932 pixels (0.12%, `magick compare -fuzz 2%`), glyph edges.
- `cargo test -p app clipboard`: 18 passed (columnSelectionPaste.test.ts, textToHtmlTokenizer.test.ts `tokenizeLineToHTML`, cursor.test.ts cut and multi-cursor paste cases). Full `cargo test -p app`: 1105 passed, 1 ignored.
