# Windows, dialogs and lifecycle

## Milestone 11 windows: dialogs

`port/tools/ui-pair /tmp/m11c-dialogs port/fixtures/ui-pair/dialogs.json dark` with `FILE=notes.txt` on the Studio (reference left, port right):

- `about`: the About message box as an NSAlert sheet, title "VSRustium" (reference "Code - OSS Dev"), Version, Commit, Date and OS lines, OK and Copy (default) buttons, same place and width. The Electron, ElectronBuildId, Chromium, Node.js, V8 and @github/copilot lines are left out by decision, so the port's sheet is shorter.
- `save-confirm` (type in `notes.txt`, Cmd+W): "Do you want to save the changes you made to notes.txt?", "Your changes will be lost if you don't save them.", Save (default), Don't Save, Cancel, the same sheet. The first run drew the warning style without the caution icon; the port now takes NSAlertStyleCritical as Electron's message_box_mac.mm does for warnings.
- `open` (Cmd+O): the NSOpenPanel sheet on the workspace folder, the same sidebar, columns, buttons (New Folder, Cancel, Open) and file list.
- Not from this change: the port shows the "Screen reader usage detected" toast and status entry because cua turns VoiceOver detection on, and the reference's Explorer badge "1" for the dirty file is missing in the port.

## Milestone 11 windows: inactive window

`port/tools/ui-pair /tmp/m11c-window port/fixtures/ui-pair/new-window.json dark` with `FILE=notes.txt` on the Studio: a shot of the window, then Shift+Cmd+N opens a second window and the first, now inactive, is shot again.

- Title bar background: `#1F1F1F` inactive (Dark Modern `titleBar.inactiveBackground`), `#181818` active, the same in both apps; the command center `#262626` inactive, `#242424` active in both.
- The 2048x70 title bar strip differs in 50 pixels inactive and 172 active (`magick compare -metric AE -fuzz 3%`), glyph edges only; the traffic lights turn grey in both.
- New Window opened a second window in both apps.
