# vscode-api-tests on the port

Written by `port/tools/api-tests` at c6178df6-dirty on 2026-11-24, `SHELL=/bin/bash`, Bun host, built-in extensions and test extension from the reference checkout. Reference: `port/results/api-tests-reference.md` (folder 356/46/0, workspace 4/0/0).

Not reached: tests of the reference run (402 folder, 4 workspace) the port did not get to before the suite ended.

| suite | passing | pending | failing | not reached | run |
|-------|---------|---------|---------|-------------|-----|
| api-folder | 35 | 41 | 133 | 193 | timed out after 1800s |
| api-workspace | 0 | 0 | 4 | 0 | exit 1 after 0s |

## Milestone 20 sweep: the whole folder suite

`MOCHA_GREP='^(?!(chat|lm |vscode API - browser|Notebook|ipynb|Interactive Window))' SUITE_TIMEOUT=780 port/tools/api-tests` on the Studio (branch port-m20-sweep): every suite except the removed features (chat, lm, the integrated browser) and notebooks (session m17b).

| run | api-folder passing / pending / failing | api-workspace |
|-----|---------------------------------------|---------------|
| at 44a309311e1 (port-integrate) | 274 / 17 / 7, exit 1 after 252 s | 4 / 0 / 0 |
| after the four milestone 20 code commits | 278 / 17 / 3, exit 1 after 41 s | 4 / 0 / 0 |

The suite runs to its end. Fixed in between: `vscode API - network proxy support` basic auth (`$lookupAuthorization`), and the three CustomExecution task tests (`runTask` with a task name showed the problem matcher picker; the 60 s timeouts are gone, hence 41 s). Left: `env.remoteName` (asserts the removed `vscode.github-authentication`), `custom root certificate` (passes with `--node`; Bun's TLS secure context has no `context.addCACert`), `Tabs - Ensure tabs getter is correct` (opens a notebook, milestone 17).
## Milestone 11: tree views and editor groups

`MOCHA_GREP="vscode API - tree|vscode API - window" SUITE_TIMEOUT=600 port/tools/api-tests` on the Studio, the working tree after the panel commit d1e955ab4cf with tree views and decorations: api-folder 33 passing, 0 pending, 11 failing (exit 1 after 10s). All four tree.test.ts cases pass (`TreeView - element already registered`, `... after rapid root refresh`, `... during switch and update`, `... after refresh`). The 11 failing are window.test.ts cases that open editors in view columns, which `$tryShowTextDocument` ignored then, plus `Tabs - vscode.open & vscode.diff` (fake-fs, `_workbench.diff`) and `showWorkspaceFolderPick` (`_workbench.pickWorkspaceFolder` not found).

After the view column commit bbe3bd19e40, Close Editors in Other Groups, Close All Editors across groups, `$acceptEditorPositionData` and moving a group telling the host, same pattern without the tree suite (`MOCHA_GREP="vscode API - window" SUITE_TIMEOUT=600`): api-folder 37 passing, 0 pending, 3 failing (exit 1 after 18s), api-workspace 0/0/0. Left: `Tabs - vscode.open & vscode.diff` ("Unable to resolve resource fake-fs:/...": a file system provider's resource in `_workbench.open`), `Tabs - Ensure tabs getter is correct` (`openNotebookDocument`, no notebooks) and `showWorkspaceFolderPick` (`_workbench.pickWorkspaceFolder`).

After the port-integrate merge 624b11540c5, with `_workbench.pickWorkspaceFolder`, provider resources and view columns in `_workbench.open` and the view column in `_workbench.diff` (same pattern): api-folder 39 passing, 0 pending, 1 failing (exit 1 after 6s), api-workspace 0/0/0. Left: `Tabs - Ensure tabs getter is correct` (`openNotebookDocument`, notebooks are milestone 17).

## Milestone 16: debug and testing

`MOCHA_GREP="vscode API - debug|createTestController" SUITE_TIMEOUT=600 port/tools/api-tests` on the Studio at e032c3f1-dirty. Before: `vscode API - debug` 2 passing, 1 failing (`start debugging failure`). After: api-folder 4 passing, 2 pending, 0 failing: `breakpoints`, `function breakpoint`, `start debugging failure`, `no rpc, createTestController(...)`. The 2 pending are `test.skip` upstream, as in the reference run.

## Milestone 11: notifications, progress, status bar

`MOCHA_GREP="vscode API - window" SUITE_TIMEOUT=900 port/tools/api-tests` on the Studio, same setup. Before (f8bd4a40 plus notifications): api-folder 27 passing, 13 failing. After the extension status bar entries: 29 passing, 0 pending, 11 failing; `createStatusBar - static` and `createStatusBar - static, CANNOT change some props` now pass. The 11 failing need editor groups (view columns, tabs) and `showWorkspaceFolderPick`. window.test.ts has no message or progress cases.

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

At port-integrate 472a393b2b5 (the onCommand activation wait and keybindings export merged), run by the coordinator on the Studio with `MOCHA_GREP="vscode API - commands|vscode API - quick input|showInputBox|#7013" SUITE_TIMEOUT=900`: api-folder 23 passing, 0 pending, 2 failing (baseline 21/0/4: quick input 11/0/0 and commands|showInputBox|#7013 10/0/4); api-workspace 0/0/0. The 2 left are `api-command: vscode.open` (a second editor group, milestone 11) and `api-command: vscode.diff` (`_workbench.diff`, milestone 14). `vscode API - editors` 13/0/0 and `vscode API - window` 29/0/11, unchanged.

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
- After the tab host requests (`$moveTab`, `$closeTab`, `$closeGroup`), save participants and overtype, uncommitted on top of 33ddd203: editors 13/0/0, window 27/0/13, the counts the merge section above gives. Tabs - Move Tab and Tabs - Close Tabs are commented out in 1.147.0's window.test.ts, so the host requests count in neither.
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

## M12 Search view

On the Studio, the working tree of the Search view commit. Combined grep of the baseline: api-folder 133/2/3/264 (exit 1 after 12s), api-workspace 4/0/0, unchanged. `MOCHA_GREP='vscode API - workspace'`: api-folder 107/2/0/293 (exit 0 after 10s), api-workspace 4/0/0, unchanged.

## M12 `_workbench.open`

On the Studio, the working tree of the `_workbench.open` commit. Combined grep of the baseline: api-folder 134/2/2/264 (exit 1 after 13s) against 133/2/3, api-workspace 4/0/0. `MOCHA_GREP='vscode API - workspace'`: api-folder 107/2/0/293 (exit 0 after 13s), api-workspace 4/0/0, unchanged. `MOCHA_GREP='vscode API - commands'`: 4 passing, 2 failing; `vscode.open with untitled supports associated resource (#138925)` passes now. Left: `api-command: vscode.open` fails at `tabGroups.all.length` 1 !== 2 after `ViewColumn.Two` (one editor group), `api-command: vscode.diff` with "command '_workbench.diff' not found" (no diff editor, milestone 14).

## M13 diagnostics

On the Studio, the working tree of the diagnostics and Next Problem commit. `MOCHA_GREP="vscode API - languages" SUITE_TIMEOUT=300 port/tools/api-tests`: api-folder 3 passing, 0 pending, 4 failing, 395 not reached (the pattern filters them). Passing: both `setTextDocumentLanguage` tests and `diagnostics, read & event`. Failing: link detector, diagnostics & CodeActionProvider, completions with document filters, folding command; they need `MainThreadLanguageFeatures`.

## M13 language features

On the Studio, the working tree of the `_executeFoldingRangeProvider` commit. `MOCHA_GREP="vscode API - languages" SUITE_TIMEOUT=300 port/tools/api-tests`: api-folder 7 passing, 0 pending, 0 failing, 395 not reached (exit 0); api-workspace 0/0/0. The four failing in M13 diagnostics pass: link detector, diagnostics & CodeActionProvider, completions with document filters, folding command.

After the paste and drop providers and the snippet transforms and merge (00c1b4d219c), `MOCHA_GREP="vscode API - editor|vscode API - languages" SUITE_TIMEOUT=600`: api-folder 20 passing, 0 pending, 0 failing (exit 0 after 4s), api-workspace 0/0/0. editor.test.ts's `insertSnippet` cases pass.

## Milestone 17: webviews

`MOCHA_GREP=<pattern> SUITE_TIMEOUT=900 port/tools/api-tests` on the Studio, the working tree of the webview panels commit 0af14672.

| MOCHA_GREP | passing | pending | failing |
|------------|---------|---------|---------|
| `vscode API - webview` | 1 | 0 | 0 |
| the M12 combined pattern, api-folder | 134 | 2 | 2 |
| the same, api-workspace | 4 | 0 | 0 |
| `vscode API - window` | 29 | 0 | 11 |
| `vscode API - editors` | 13 | 0 | 0 |

webview.test.ts ("loads many local resources concurrently without crashing") passes as in the reference. The other counts equal their baselines above (M12 `_workbench.open`, Milestone 11, Milestone 8): nothing lost.

## M10 settings and storage

On the Studio at 1ed0e932 (settings.json reload in the workbench, `.vsrustium` folders). `MOCHA_GREP="vscode API - configuration|vscode API - globalState" SUITE_TIMEOUT=300 port/tools/api-tests`: api-folder 5/0/0/397 (exit 0 after 1s), api-workspace 0/0/0/4. All three `configuration` tests and both `globalState / workspaceState` tests pass.

## M12 workspace trust

`MOCHA_GREP="vscode API - env|vscode API - workspace" SUITE_TIMEOUT=900 port/tools/api-tests` on the Studio, the working tree of the workspace trust commit (`--disable-workspace-trust`, as the reference run has `security.workspace.trust.enabled: false`). api-folder 113 passing, 2 pending, 7 failing, 280 not reached (exit 1 after 13s); api-workspace 4/0/0. The 7 failing are the 6 `env.power` tests and `env.remoteName`, as at e9ae1460.

## Milestone 15: terminal and tasks

On the Studio at b9d74655619, `SHELL=/bin/bash MOCHA_GREP='vscode API - ([tT]erminal|tasks)' SUITE_TIMEOUT=600 port/tools/api-tests`: api-folder 47 passing, 4 pending, 0 failing, 351 not reached (exit 0 after 20s); api-workspace 0/0/0/4. terminal.test.ts, terminal.shellIntegration.test.ts and workspace.tasks.test.ts all pass; the 4 pending are upstream `test.skip` ("Test if env is set", "should listen to all future terminal data events", "should respect dimension overrides", "dependsOn task should start with a different processId (#118256)").

## M12 verification

`MOCHA_GREP="vscode API - (workspace|file system|env|types|tasks)" SUITE_TIMEOUT=900 port/tools/api-tests` on the Studio at 44a309311e1 (port-m12b-verify): api-folder 132 passing, 3 pending, 4 failing, 263 not reached (exit 1 after 212s); api-workspace 4/0/0 as the reference. Every test of `vscode API - workspace`, `workspace-fs`, `workspace events`, `workspace-watcher`, `file system` and `types` passes. Failing: `env.remoteName` (expects the removed github-authentication, as before) and 3 `vscode API - tasks` tests that time out after 60 s in this combined run ("main-thread startup survives provider disposal before the extension host observes the start", "provider disposal before execution preserves an extension-held task", "task should start and shutdown successfully"; milestone 15's, roadmap section 12 leaves workspace.tasks out). The 3 pending are upstream `test.skip`.
