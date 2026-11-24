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
