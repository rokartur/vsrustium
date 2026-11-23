# Memory benchmark

Method: `port/tools/bench` (Wu's `script/memory-benchmark`): fresh profile per run, 20 s settle, `footprint` summed over the process tree, median of 3. Mac Studio, macOS 27.2.1. Idle, no folder open. Copilot Chat excluded everywhere (removed by decision, see `port/notes.md`).

| app | total MiB | extension host MiB | samples (KiB) |
|---|---|---|---|
| port, Bun host | 109.0 | 57.0 | 111616 111616 111616 |
| port, Node host (control) | 151.0 | 98.0 | 153600 154624 154624 |
| VS Code 1.147.0 reference (dev build) | 1978.2 | 151.0 | 2050241 2003153 2025665 |
| Wu a3ced28 (release) | 156.0 | 0 (no extension host) | 159744 158720 161792 |

Extension host overhead, Bun against Node: -41 MiB (57 vs 98).

Read with care:
- The port draws only a status window so far; its total is the shell plus the extension host, not a workbench. The comparable number today is the extension host line.
- The reference is an unbundled dev build (`VSCODE_DEV=1`), heavier than a packaged release. A packaged reference build is still to measure.
- Wu measured 99 MiB in one earlier full run and 154 to 156 MiB in two later ones; cause not investigated.
- With Copilot Chat loaded, the port's host grows to 197 MiB (Bun) and 735 MiB (Node), because its activation never completes against the stub main thread. The reference host stays at 150 MiB with or without it.

Commands:

```
R=$HOME/Developer/vscode-ref; N=<extensions dir without copilot>
APPS="wu=$HOME/Developer/wu/target/release/wu;vscode=env VSCODE_DEV=1 NODE_ENV=development \"$R/.build/electron/Code - OSS.app/Contents/MacOS/Code - OSS\" $R --disable-extension GitHub.copilot-chat;port-bun=env VSCODE_GPUI_EXTENSIONS=$N native/target/release/code-gpui;port-node=env VSCODE_GPUI_EXTENSIONS=$N native/target/release/code-gpui --node" port/tools/bench
```
