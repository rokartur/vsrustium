# Command center

## Milestone 9: click opens Quick Open

Reference: 1.147.0 dev build on the Studio; port: release `code-gpui`. Both driven by `ui-pair` (cua-driver) with fixture `port/fixtures/workspace`, Dark Modern, 1024x768 at 2x: click the command center at (512, 17), shoot, then Down and Return, shoot. Pairs in `raw/command-center/`, reference on top, port below.

- `open.png`: both show Quick Open with an empty input, the placeholder "Search files by name (append : to go to line or @ to go to symbol), or paste a link", and the help picks with Go to File first (`⌘P`) and Show and Run Commands `>` (`⇧⌘P`). The reference adds Search for Text `%`, Go to Symbol in Editor `@`, Start Debugging `debug`, Run Task `task` and More `?`; the port has no providers for them yet.
- `commands.png`: Down and Return on Show and Run Commands put `>` in the input and list commands in both; the lists differ by the commands the port does not register yet.
- The port's click is delivered with cua-driver's foreground mode. A background click (posted to the pid) on the title bar never reaches GPUI on macOS 27, since AppKit owns the title bar's drag region (`app_owns_titlebar_drag` off); the reference takes it.
- `cargo test -p app title_bar`: the click opens Quick Open with the help picks and the recent file, Down Return goes to `>`, a Cmd+click shows the title menu instead. `cargo test -p app`: 403 passed, 1 ignored.
