# Command center

## Milestone 9: click opens Quick Open

Reference: 1.147.0 dev build on the Studio; port: release `code-gpui`. Both driven by `ui-pair` (cua-driver) with fixture `port/fixtures/workspace`, Dark Modern, 1024x768 at 2x: click the command center at (512, 17), shoot, then Down and Return, shoot. Pairs in `raw/command-center/`, reference on top, port below.

- `open.png`: both show Quick Open with an empty input, the placeholder "Search files by name (append : to go to line or @ to go to symbol), or paste a link", and the help picks with Go to File first (`⌘P`) and Show and Run Commands `>` (`⇧⌘P`). The reference adds Search for Text `%`, Go to Symbol in Editor `@`, Start Debugging `debug`, Run Task `task` and More `?`; the port has no providers for them yet.
- `commands.png`: Down and Return on Show and Run Commands put `>` in the input and list commands in both; the lists differ by the commands the port does not register yet.
- The port's click is delivered with cua-driver's foreground mode. A background click (posted to the pid) on the title bar never reaches GPUI on macOS 27, since AppKit owns the title bar's drag region (`app_owns_titlebar_drag` off); the reference takes it.
- `cargo test -p app title_bar`: the click opens Quick Open with the help picks and the recent file, Down Return goes to `>`, a Cmd+click shows the title menu instead. `cargo test -p app`: 403 passed, 1 ignored.

## Milestone 9: drag region and click latency

The command center is `-webkit-app-region: no-drag` (titlebarpart.css), as are the layout controls and the quick input; the rest of the bar drags the window. The port now owns the title bar drag (`app_owns_titlebar_drag`): the bar's empty space calls AppKit's window drag and the system's double-click action, the no-drag parts stop the press.

Measured on the Studio (macOS 27.2.1), release build, fixture `port/fixtures/workspace`, Dark Modern, with real HID mouse events (CGEvent at the HID tap, an 80 ms press) and wall-clock logs in a scratch build: press to the command center's mouse down, to its click, and to the first frame after the click (Quick Open drawn). Control: a press on the editor area. 9 clicks each.

| | press to mouse down | press to click | press to Quick Open frame | control press to mouse down |
|---|---|---|---|---|
| before (AppKit drag region) | 4-10 ms | 87-92 ms | 98-112 ms, median 103 | 2-5 ms |
| after (app-owned drag region) | 2-6 ms | 86-90 ms | 103-110 ms, median 106 | 2-5 ms |

- No AppKit click delay shows with these events on macOS 27.2.1, before or after: the time is the 80 ms press plus about 20 ms to the frame. A delay that real trackpad clicks may hit was not reproduced.
- Drag, same events: before, a drag from the command center moved the window (88, 83) like the empty bar; after, the empty bar moves the window (60, 60) with the drag and a drag from the command center leaves it in place. A double click on the empty bar zooms the window (no `AppleActionOnDoubleClick` set) before and after.
- `cargo test -p app title_bar`: a press on the empty bar reaches the window move (the test window panics there); presses on the layout controls, the command center and the quick input over the bar do not (removing the layout controls' stop fails the test). `cargo test -p app`: 544 passed, 1 ignored.
