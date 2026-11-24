# vscode-api-tests on the port

Written by `port/tools/api-tests` at c6178df6-dirty on 2026-11-24, `SHELL=/bin/bash`, Bun host, built-in extensions and test extension from the reference checkout. Reference: `port/results/api-tests-reference.md` (folder 356/46/0, workspace 4/0/0).

Not reached: tests of the reference run (402 folder, 4 workspace) the port did not get to before the suite ended.

| suite | passing | pending | failing | not reached | run |
|-------|---------|---------|---------|-------------|-----|
| api-folder | 35 | 41 | 133 | 193 | timed out after 1800s |
| api-workspace | 0 | 0 | 4 | 0 | exit 1 after 0s |

## M10

`MOCHA_GREP="vscode API - configuration|vscode API - globalState" SUITE_TIMEOUT=300 port/tools/api-tests` at 96df8471, same setup. The two suites have 5 tests; not reached counts the reference tests the pattern filters out.

| suite | passing | pending | failing | not reached | run |
|-------|---------|---------|---------|-------------|-----|
| api-folder | 5 | 0 | 0 | 397 | exit 0 after 1s |
| api-workspace | 0 | 0 | 0 | 4 | exit 0 after 0s |

Passing: configuration, language defaults; configuration, defaults; configuration, name vs property; and the two `globalState / workspaceState` tests.

## Milestone 9: commands and quick input

Run with `MOCHA_GREP` on the Mac Studio at f598f3a9 (the full folder suite does not finish within its timeout, milestone 6). Baseline at 27feb270 with `vscode API - (commands|quick input)`: 3 passing, 15 failing.

| MOCHA_GREP | passing | pending | failing |
|------------|---------|---------|---------|
| `vscode API - quick input` | 11 | 0 | 0 |
| `vscode API - commands\|showInputBox\|#7013` | 10 | 0 | 4 |

Failing: `editorCommand with extra args`, `api-command: vscode.diff`, `api-command: vscode.open`, `vscode.open with untitled supports associated resource (#138925)`; they need milestone 8 editor groups and a diff editor (port/notes.md).

After the command history, quick input position and theme picker commits, one run with `vscode API - quick input|vscode API - commands|showInputBox|#7013|vscode API - configuration`: 24 passing, 0 pending, the same 4 failing.

## M12

`MOCHA_GREP="vscode API - workspace-fs|vscode API - workspace events|vscode API - workspace " SUITE_TIMEOUT=600 port/tools/api-tests` at 9f6772f3 on the Studio, same setup. Not reached counts the reference tests the pattern filters out.

| suite | passing | pending | failing | not reached | run |
|-------|---------|---------|---------|-------------|-----|
| api-folder | 33 | 2 | 73 | 294 | exit 1 after 182s |
| api-workspace | 0 | 0 | 4 | 0 | exit 1 after 1s |

Passing: all 12 of `workspace-fs`; in `workspace`, findFiles, findFiles2 and findTextInFiles with their exclude and cancellation tests (11). `workspace events` fails 0/10 (no bulk edits); api-workspace needs multi-root.

## M12 multi-root

`MOCHA_GREP="vscode API - quick input|vscode API - commands|showInputBox|#7013|vscode API - configuration|vscode API - globalState|vscode API - workspace-fs|vscode API - workspace events|vscode API - workspace " SUITE_TIMEOUT=900 port/tools/api-tests` on the Studio, same setup, the working tree of the multi-root commit. Baseline at 0ff1a06a with the same pattern: api-folder 59/2/77, api-workspace 0/0/4.

| suite | passing | pending | failing | not reached | run |
|-------|---------|---------|---------|-------------|-----|
| api-folder | 59 | 2 | 77 | 264 | exit 1 after 188s |
| api-workspace | 4 | 0 | 0 | 0 | exit 0 after 0s |

api-workspace matches the reference (4/0/0): rootPath, workspaceFile, workspaceFolders, getWorkspaceFolder. api-folder is unchanged.

## M12 bulk file edits

Same `MOCHA_GREP` and setup as the multi-root section, on the Studio, the working tree of the bulk edits commit. Baseline: the multi-root section (api-folder 59/2/77).

| suite | passing | pending | failing | not reached | run |
|-------|---------|---------|---------|-------------|-----|
| api-folder | 74 | 2 | 62 | 264 | exit 1 after 7s |
| api-workspace | 4 | 0 | 0 | 0 | exit 0 after 1s |

`workspace events` passes 6/10: onWillCreate/onDidCreate, onWillDelete/onDidDelete and its three "make changes" cases, onWillRename/onDidRename. The 4 failing ones open or edit text documents (`openTextDocument`, text edits from participants), milestone 8. New passes in `workspace`: applyEdit should fail when renaming deleted resource, rename & ignoreIfExists, delete & ignoreIfNotExists, #111867 1/3, 2/3 (fake-fs provider) and 3/3, creating binary files, #42667. "applyEdit should fail when editing deleted/renamed from resource" pass because any text edit fails today, not because of the version check. The other applyEdit tests need text documents.
