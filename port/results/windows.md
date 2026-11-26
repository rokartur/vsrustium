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

## Milestone 11 verification (port-m11e-verify)

`bun port/tools/ui-pair <out> <steps>` on the Studio, reference left, port right, the window 1024x768 at 2x. Steps in `port/fixtures/ui-steps/`; the messages and dialogs pairs run with `ARGS=--extensionDevelopmentPath=<repo>/port/fixtures/extensions/messages`, dialogs with `SETTINGS='{"window.dialogStyle": "custom"}'`, banner with `SETTINGS='{"security.workspace.trust.enabled": true}'`.

- `banner.json` (`banner`): the Restricted Mode banner with its shield, message, Manage and Learn More links and close button at the same place, the Restricted Mode status entry, the status bar at the bottom. Before the fix the banner pushed the status bar out of the window (the part grid did not count its 26px).
- `hot-exit.json`: `sash` (a drag of the side bar sash) the same width in both; `dirty`, then Cmd+Q and a restart: `restored` shows guide.md and the untitled "hot exit text" editor in the same tabs, the untitled one active and dirty with its text, and the Explorer badge "1", the restored editor focused (cursor and word highlight). Before: the port reopened the untitled editor after guide.md as a background tab without focus and had no Explorer badge. `launched` (a second start with `docs/guide.md`) opens it in the running window in both; `switch-window` (Switch Window) the one row with the file icon, "Current Window" and the active-window button, as the reference.
- `messages.json` with `PORT_MESSAGES=info,status,windowProgress,decorations`: `startup` the extension's status entries (`$(bell) Port left`, the "Port warning" entry on `statusBarItem.warningBackground`) and the red "P" decoration on package.json; `center` (Notifications: Show Notifications) the window progress row focused over the information message with its source, Yes and No, at the same pixels after the row focus and separator stopped taking layout room (they moved the text 1 to 2px); `progress` the notification progress toast with Cancel and the `Window progress` status entry; `modal` the NSAlert sheet; `open-dialog` the NSOpenPanel sheet.
- `dialogs.json`: `custom-modal` the custom dialog 450px wide with Close Dialog, the detail in `descriptionForeground` and Discard, Cancel, Save (dialog.ts `rearrangeButtons`; before: 480px, no close button, Cancel first); `save-dialog` the NSSavePanel sheet.
- api-tests `MOCHA_GREP="vscode API - window|vscode API - tree" SUITE_TIMEOUT=900 port/tools/api-tests`: api-folder 43 passing, 0 pending, 1 failing, api-workspace 0/0/0. The failing one is `Tabs - Ensure tabs getter is correct` (`openNotebookDocument`, notebooks are milestone 17).
- The Studio's display ran at 1x for part of these runs; ui-pair now reads the pixel scale from its first capture instead of assuming 2x.
