# vscode-api-tests on the port

Written by `port/tools/api-tests` at c6178df6-dirty on 2026-11-24, `SHELL=/bin/bash`, Bun host, built-in extensions and test extension from the reference checkout. Reference: `port/results/api-tests-reference.md` (folder 356/46/0, workspace 4/0/0).

Not reached: tests of the reference run (402 folder, 4 workspace) the port did not get to before the suite ended.

| suite | passing | pending | failing | not reached | run |
|-------|---------|---------|---------|-------------|-----|
| api-folder | 35 | 41 | 133 | 193 | timed out after 1800s |
| api-workspace | 0 | 0 | 4 | 0 | exit 1 after 0s |

## Milestone 9: commands and quick input

Run with `MOCHA_GREP` on the Mac Studio at f598f3a9 (the full folder suite does not finish within its timeout, milestone 6). Baseline at 27feb270 with `vscode API - (commands|quick input)`: 3 passing, 15 failing.

| MOCHA_GREP | passing | pending | failing |
|------------|---------|---------|---------|
| `vscode API - quick input` | 11 | 0 | 0 |
| `vscode API - commands\|showInputBox\|#7013` | 10 | 0 | 4 |

Failing: `editorCommand with extra args`, `api-command: vscode.diff`, `api-command: vscode.open`, `vscode.open with untitled supports associated resource (#138925)`; they need milestone 8 editor groups and a diff editor (port/notes.md).
