# vscode-api-tests on the reference build (1.147.0, untouched)

Command, on the Mac Studio in `~/Developer/vscode-ref`: `SHELL=/bin/bash ./scripts/test-integration.sh --suite api-folder` and `--suite api-workspace` (Electron dev build from `npm run electron`).

| suite | passing | pending | failing |
|-------|---------|---------|---------|
| api-folder | 356 | 46 | 0 |
| api-workspace | 4 | 0 | 0 |

`SHELL=/bin/bash` is required on the Studio: under its fish login shell, 6 tests fail on the environment, not the product (the terminal tests capture fish's prompt or time out, and the CDP golden scenario timed out in the same run). With bash all 356 pass.

The port is held to 318/28/0 (folder): the suites and tests of features removed by decision (port/notes.md) leave the target. Counted from the bash run's log, top-level suites:

| suite | passing | pending |
|-------|---------|---------|
| chat | 5 | 3 |
| chat - browser tools | 6 | 0 |
| chat - run_in_terminal | 3 | 12 |
| lm | 5 | 0 |
| vscode API - browser (integrated browser, removed later) | 17 | 3 |
| vscode API - browser CDP (integrated browser, removed later) | 1 | 0 |
| vscode API - env: `env.remoteName` (asserts the removed `vscode.github-authentication` sign-in extension is installed) | 1 | 0 |
| total | 38 | 18 |

Counted from the test sources for the suites removed after the reference run: `browser.test.ts` has 17 tests that run outside a remote and 3 skipped (the remote network test and the two `test.skip` file:// tests), `browser.cdp.test.ts` one golden scenario test that runs outside a remote.
