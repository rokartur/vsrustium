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

After Quick Open's file picks (`vscode API - quick input|vscode API - commands|showInputBox|#7013|vscode API - configuration|vscode API - globalState|vscode API - workspace-fs|vscode API - workspace events|vscode API - workspace `, SUITE_TIMEOUT=900): api-folder 59 passing, 2 pending, 77 failing, the same counts as before the change; api-workspace 0/0/4 as in M12 (multi-root).

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

## M12 Explorer file operations

Same `MOCHA_GREP` and setup as the bulk file edits section, on the Studio, the working tree of the Explorer file operations commit. Baseline: the bulk file edits section.

| suite | passing | pending | failing | not reached | run |
|-------|---------|---------|---------|-------------|-----|
| api-folder | 74 | 2 | 62 | 264 | exit 1 after 8s |
| api-workspace | 4 | 0 | 0 | 0 | exit 0 after 0s |

Unchanged, as expected: the Explorer operations add no extension API.

## Merge of milestone 8 into the integrate branch

`port/tools/api-tests` on the Studio, the working tree of the merge, same setup. Baseline: the M12 Explorer file operations section (api-folder 74/2/62, api-workspace 4/0/0) and the milestone 8 runs below.

| MOCHA_GREP | passing | pending | failing |
|------------|---------|---------|---------|
| the M12 pattern (`vscode API - quick input\|vscode API - commands\|showInputBox\|#7013\|vscode API - configuration\|vscode API - globalState\|vscode API - workspace-fs\|vscode API - workspace events\|vscode API - workspace `), api-folder | 81 | 2 | 55 |
| the same, api-workspace | 4 | 0 | 0 |
| `vscode API - editors` | 13 | 0 | 0 |
| `vscode API - window` | 26 | 0 | 14 |

New passes against the M12 run, none lost: editorCommand with extra args; encoding: openTextDocument - invalid encoding falls back to default; eol, read; events: onDidOpenTextDocument, onDidChangeTextDocument, onDidSaveTextDocument; onWillCreate/onDidCreate, make changes, edit new file fails; openTextDocument; openTextDocument, with selection. `vscode API - window` went from 20/0/20 on milestone 8 to 26/0/14 with milestone 9's quick input. One api-workspace run gave 0/0/0 with `extension host failed: Address already in use (os error 48)`; two reruns gave 4/0/0.

## Milestone 8: editor viewport

`MOCHA_GREP=<suite> SUITE_TIMEOUT=900 port/tools/api-tests` at c681711e on 2026-11-24, one suite per run. There is no `vscode API - documents` suite in 1.147.0; the document tests live in `vscode API - workspace`.

| MOCHA_GREP | passing | pending | failing | before MainThread requests (5fb18b09) |
|------------|---------|---------|---------|---------------------------------------|
| `vscode API - editors` | 13 | 0 | 0 | 0 / 1 / 12 |
| `vscode API - window` (pattern `workspace$` or `window`; only window matched) | 20 | 0 | 20 | |
| `vscode API - workspace ` | 18 | 2 | 76 | |

- editors: every test of editor.test.ts passes, as in the reference.
- window: the text editor tests that need one editor group pass (active text editor, opened via resource, #49125, #134786, #5362, selection change kind). The ViewColumn, tab and `bikes:` content provider tests fail on the missing editor groups and content providers; the input box, quick pick and status bar failures belong to other rows (port/notes.md milestone 8).
- After multi-cursor (ea71708c): the same three runs give the same counts, editors 13/0/0, window 20/0/20, workspace 18/2/76.
- After folding, with the scroll position truncated to whole pixels: editors 13/0/0.
- workspace: openTextDocument, eol, the open/change/save events and openTextDocument with selection pass; the failures are untitled documents, content providers, WorkspaceEdit and file events, none in milestone 8 scope.
## M12 Explorer checked against the reference

Same `MOCHA_GREP` and setup as the bulk file edits section, on the Studio, the working tree of the Explorer reference-comparison commit.

| suite | passing | pending | failing | not reached | run |
|-------|---------|---------|---------|-------------|-----|
| api-folder | 74 | 2 | 62 | 264 | exit 1 after 7s |
| api-workspace | 4 | 0 | 0 | 0 | exit 0 after 0s |

Unchanged: the changes are UI only.

## M12 workspace documents

`MOCHA_GREP='vscode API - workspace' SUITE_TIMEOUT=900 port/tools/api-tests` at 923a8923-dirty (text edits, untitled documents, content providers, save participants). Baseline before this work with the same grep: api-folder 54/2/53.

| suite | passing | pending | failing | not reached | run |
|-------|---------|---------|---------|-------------|-----|
| api-folder | 94 | 2 | 13 | 293 | exit 1 after 10s |
| api-workspace | 4 | 0 | 0 | 0 | exit 0 after 1s |

Left: 8 encoding tests, 3 saves of `fake-fs` documents, `openTextDocument, actual casing first`, `createFileSystemWatcher`.

After the encoding port (`files::encoding`, `$resolveDecoding`, `$validateDetectedEncoding`, `$resolveEncoding`, `options.encoding` of `$tryOpenDocument`), same grep:

| suite | passing | pending | failing | not reached | run |
|-------|---------|---------|---------|-------------|-----|
| api-folder | 102 | 2 | 5 | 293 | exit 1 after 13s |
| api-workspace | 4 | 0 | 0 | 0 | exit 0 after 1s |

Left: 3 saves of `fake-fs` documents, `openTextDocument, actual casing first`, `createFileSystemWatcher`.

After saves through the extension host (`workbench.action.files.save`, `$save`, `$saveAll`), same grep: api-folder 105/2/2/293 (exit 1 after 10s), api-workspace 4/0/0. Left: `openTextDocument, actual casing first`, `createFileSystemWatcher`.

After canonical casing in `$tryOpenDocument`, same grep: api-folder 106/2/1/293 (exit 1 after 10s), api-workspace 4/0/0. Left: `createFileSystemWatcher`.

After a recursive `$watch` stays recursive when its stat fails, same grep: api-folder 107/2/0/293 (exit 0 after 11s), api-workspace 4/0/0. Every test of the workspace suites passes.

The combined grep of the baseline (`vscode API - quick input|vscode API - commands|showInputBox|#7013|vscode API - configuration|vscode API - globalState|vscode API - workspace-fs|vscode API - workspace events|vscode API - workspace `) at the same commit: api-folder 133/2/3/264 (exit 1 after 12s) against 81/2/55 before, api-workspace 4/0/0. The 3 left are `vscode API - commands`: `vscode.diff` and `vscode.open` (`_workbench.diff`, `_workbench.open` not registered) and `vscode.open with untitled supports associated resource`.

## M12 participant confirmation

Same `MOCHA_GREP` and setup as the bulk file edits section, on the Studio, the working tree of the participant confirmation commit.

| suite | passing | pending | failing | not reached | run |
|-------|---------|---------|---------|-------------|-----|
| api-folder | 74 | 2 | 62 | 264 | exit 1 after 7s |
| api-workspace | 4 | 0 | 0 | 0 | exit 0 after 0s |

Unchanged, as expected: extension test runs show no participant dialog.
