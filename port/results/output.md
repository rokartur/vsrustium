# Output view

## Milestone 15: the Output view and Cmd+Shift+U

Reference: 1.147.0 dev build on the Studio; port: release `code-gpui`. Both driven by `ui-pair` with fixture `port/fixtures/workspace`, Dark Modern, 1024x768 at 2x. Shots in `raw/output/`.

- `output.png` (Cmd+Shift+U): the reference opens the panel with the Output view on the Tasks channel; the port opens the Output view as an editor tab (port/notes.md milestone 15: no panel before milestone 11) on the Window channel, the port having no Tasks channel. Both show the filter box with the placeholder "Filter (e.g. text, !excludeText, text1,text2)", the channel dropdown, Clear Output and Toggle Auto Scrolling; the reference's narrow panel title moves the gear and Open Output in Editor into its `...` overflow, the port draws them. The reference ends the clipped placeholder with an ellipsis, the port clips it. Fixed after the first pair: the port drew the current line border, which `OutputEditor` turns off (`renderLineHighlight: none`).
- `toggled.png` (click the text, Cmd+Shift+U again): the view closes in both.

Checks (Studio):

- `cargo test -p app output`: 11 passed; `cargo test -p app -p exthost`: app 922 passed, 1 ignored; exthost 18 passed.

## Milestone 15: logs

Same setup, steps: Command Palette, "Set Log Level", Return; then "Show Logs", Return.

- `setloglevel.png`: same layout in both: the All separator on Trace, the levels with Info checked and described Default, then Extension Logs and Logs separators. The lists differ only in the channels each app has: the reference has main process logs (Main, MCP Gateway, Shared, ...) and more extension loggers; the port has Window and Extension Host.
- `showlogs.png`: "Select Log", the port's logs first, then an Extension Logs separator, as in the reference; the same channel difference.

Checks (Studio, after the logs and keybindings commits):

- `cargo test -p app output`: 14 passed; `cargo test -p app -p exthost`: app 926 passed, 1 ignored; exthost 18 passed.
- `MOCHA_GREP="rpc|workspace" port/tools/api-tests`: api-folder 126 passing, 5 pending, 5 failing (Notebook Document `workspace edit API`, notebooks not ported); api-workspace 4 passing.
