# vscode-api-tests on the reference build (1.147.0, untouched)

Command, on the Mac Studio in `~/Developer/vscode-ref`: `SHELL=/bin/bash ./scripts/test-integration.sh --suite api-folder` and `--suite api-workspace` (Electron dev build from `npm run electron`).

| suite | passing | pending | failing |
|-------|---------|---------|---------|
| api-folder | 356 | 46 | 0 |
| api-workspace | 4 | 0 | 0 |

`SHELL=/bin/bash` is required on the Studio: under its fish login shell, 6 tests fail on the environment, not the product (the terminal tests capture fish's prompt or time out, and the CDP golden scenario timed out in the same run). With bash all 356 pass; this is the baseline the port is held to.
