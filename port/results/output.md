# Output view

## Milestone 15: the Output view and Cmd+Shift+U

Reference: 1.147.0 dev build on the Studio; port: release `code-gpui`. Both driven by `ui-pair` with fixture `port/fixtures/workspace`, Dark Modern, 1024x768 at 2x. Shots in `raw/output/`.

- `output.png` (Cmd+Shift+U): the reference opens the panel with the Output view on the Tasks channel; the port opens the Output view as an editor tab (port/notes.md milestone 15: no panel before milestone 11) on the Window channel, the port having no Tasks channel. Both show the filter box with the placeholder "Filter (e.g. text, !excludeText, text1,text2)", the channel dropdown, Clear Output and Toggle Auto Scrolling; the reference's narrow panel title moves the gear and Open Output in Editor into its `...` overflow, the port draws them. The reference ends the clipped placeholder with an ellipsis, the port clips it. Fixed after the first pair: the port drew the current line border, which `OutputEditor` turns off (`renderLineHighlight: none`).
- `toggled.png` (click the text, Cmd+Shift+U again): the view closes in both.

Checks (Studio):

- `cargo test -p app output`: 11 passed; `cargo test -p app -p exthost`: app 922 passed, 1 ignored; exthost 18 passed.
