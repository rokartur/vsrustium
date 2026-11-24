# vscode-api-tests on the port

Written by `port/tools/api-tests` at c6178df6-dirty on 2026-11-24, `SHELL=/bin/bash`, Bun host, built-in extensions and test extension from the reference checkout. Reference: `port/results/api-tests-reference.md` (folder 356/46/0, workspace 4/0/0).

Not reached: tests of the reference run (402 folder, 4 workspace) the port did not get to before the suite ended.

| suite | passing | pending | failing | not reached | run |
|-------|---------|---------|---------|-------------|-----|
| api-folder | 35 | 41 | 133 | 193 | timed out after 1800s |
| api-workspace | 0 | 0 | 4 | 0 | exit 1 after 0s |

## Milestone 8: editor viewport

`MOCHA_GREP=<suite> SUITE_TIMEOUT=900 port/tools/api-tests` at c681711e on 2026-11-24, one suite per run. There is no `vscode API - documents` suite in 1.147.0; the document tests live in `vscode API - workspace`.

| MOCHA_GREP | passing | pending | failing | before MainThread requests (5fb18b09) |
|------------|---------|---------|---------|---------------------------------------|
| `vscode API - editors` | 13 | 0 | 0 | 0 / 1 / 12 |
| `vscode API - window` (pattern `workspace$` or `window`; only window matched) | 20 | 0 | 20 | |
| `vscode API - workspace ` | 18 | 2 | 76 | |

- editors: every test of editor.test.ts passes, as in the reference.
- window: the text editor tests that need one editor group pass (active text editor, opened via resource, #49125, #134786, #5362, selection change kind). The ViewColumn, tab and `bikes:` content provider tests fail on the missing editor groups and content providers; the input box, quick pick and status bar failures belong to other rows (port/notes.md milestone 8).
- workspace: openTextDocument, eol, the open/change/save events and openTextDocument with selection pass; the failures are untitled documents, content providers, WorkspaceEdit and file events, none in milestone 8 scope.
