# vscode-api-tests on the reference build (1.147.0, untouched)

Command, on the Mac Studio in `~/Developer/vscode-ref`: `./scripts/test-integration.sh --suite api-folder` and `--suite api-workspace` (Electron dev build from `npm run electron`).

| suite | passing | pending | failing |
|-------|---------|---------|---------|
| api-folder | 350 | 46 | 6 |
| api-workspace | 4 | 0 | 0 |

The 6 failures are environment, not product, and are the baseline the port is held to (same count, same tests):

1. `vscode API - browser CDP / CDP debugging golden scenario`: timed out waiting for a CDP event.
2. `chat - run_in_terminal / sandbox off / no-output command reports empty output`: the Studio's login shell is fish, and its prompt (`... git:(8a14f0cc)`) is captured as output.
3. `chat - run_in_terminal / sandbox off / output with special characters is captured verbatim`: same fish prompt.
4. to 6. `vscode API - terminal / environmentVariableCollection` (apply immediately, variables that don't exist, deleting entries): 20 s timeouts waiting for the echoed value. Likely the fish shell too; not investigated.
