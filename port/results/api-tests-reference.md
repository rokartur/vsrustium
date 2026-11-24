# vscode-api-tests on the reference build (1.147.0, untouched)

Command, on the Mac Studio in `~/Developer/vscode-ref`: `SHELL=/bin/bash ./scripts/test-integration.sh --suite api-folder` and `--suite api-workspace` (Electron dev build from `npm run electron`).

| suite | passing | pending | failing |
|-------|---------|---------|---------|
| api-folder | 356 | 46 | 0 |
| api-workspace | 4 | 0 | 0 |

`SHELL=/bin/bash` is required on the Studio: under its fish login shell, 6 tests fail on the environment, not the product (the terminal tests capture fish's prompt or time out, and the CDP golden scenario timed out in the same run). With bash all 356 pass.

The port is held to 337/31/0 (folder): the suites of features removed by decision (port/notes.md) leave the target. Counted from the bash run's log, top-level suites:

| suite | passing | pending |
|-------|---------|---------|
| chat | 5 | 3 |
| chat - browser tools | 6 | 0 |
| chat - run_in_terminal | 3 | 12 |
| lm | 5 | 0 |
| total | 19 | 15 |
