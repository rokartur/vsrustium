# Source control extensions

Extension integration tests run against the port and the reference on the Mac Studio. Port: `port/scratch`-style
runner (vsrustium with `--extensionDevelopmentPath` and `--extensionTestsPath` on the reference checkout's compiled
extension, Bun host). Reference: `SHELL=/bin/bash ./scripts/test-integration.sh --suite <name>` in `~/Developer/vscode-ref`.

| suite | port passing | port pending | port failing | reference passing | reference pending | reference failing |
|-------|--------------|--------------|--------------|-------------------|-------------------|-------------------|
| git | 91 | 2 | 0 | 91 | 2 | 0 |
| git-base | 14 | 0 | 0 | 14 | 0 | 0 |

git-base reference: the same runner on the reference build (`scripts/test-integration.sh --suite git-base` needs pnpm, which the Studio lacks).
github: `out/test/index.js` is not a test runner either build accepts ("does not point to a valid extension test runner" in both); the suite is not in `scripts/test-integration.sh`.

Fixed on the way: sinon fake timers in git's `git ignore decorations` suite held the socket extension host's writes
(ipc.net.ts `ProtocolWriter` scheduled with the faked `setTimeout`), and the SCM input box's text model was released
with the host models, which panicked on the next commit (`stages correctly` was the last test before the fix).

## SCM pixel-check window

`port/tools/ui-pair` on `port/fixtures/workspace` copied into a git repository with `notes.txt` changed: Ctrl+Shift+G,
the SCM view lists `notes.txt` (M), a click opens `notes.txt (Working Tree)`, the inline diff with the added line 10.

## merge-conflict

ui-pair with `FILE=conflict.txt` (a `<<<<<<< HEAD` / `=======` / `>>>>>>> feature` block): current and incoming
decorations, the lens line `Accept Current Change | Accept Incoming Change | Accept Both Changes | Compare Changes`
in the UI font, and the previous/next conflict actions in the editor title, as in the reference shot.
