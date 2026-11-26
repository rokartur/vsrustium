---
name: vscode-uplift
description: Plans the port's move to a newer VS Code release. Use when a new VS Code version is out, or when asked what an update of VSRustium to it takes.
---

# VS Code uplift plan

VSRustium ports one VS Code release 1:1. This skill compares that release with a newer one and writes the plan for the uplift. It plans only: it changes no code, and the reference on the Studio stays on the current release until the plan's work starts.

## 1. Versions

- Port release: `version` in `package.json` (the same value is `VERSION` in `native/crates/app/src/main.rs`).
- Newest upstream release: `git ls-remote --tags origin 'refs/tags/1.*' | grep -v '\^{}' | sed 's|.*refs/tags/||' | sort -V | tail -1`.

Equal versions: report "VSRustium follows the newest release" and stop. Otherwise `git fetch origin tag <to> --no-tags`.

## 2. Map the diff

`bun port/tools/uplift-diff.ts <to>` maps every changed upstream file to its `port/ledger.md` row and groups the rows: to port, new upstream unit, removed by decision, tests only, API typings, not mapped. Save its output; the plan cites its numbers.

## 3. Triage

The **skip set** is everything the port leaves out by Artur's decisions: AI (chat, agents, agent host, sessions, Copilot, MCP, language models, inline chat, embeddings, speech), sign-in and accounts (authentication, GitHub sign-in), Settings Sync, remote and tunnels, Microsoft services (experiments, surveys, issue reporter), telemetry, and the integrated browser. The full list is in `port/notes.md` (the "Decision (Artur)" lines) and `port/roadmap.md` "Decisions (Artur)".

Read the diff of every group except "removed by decision" (`git diff <from> <to> -- <files>`) and give each change one verdict:

- **port**: user-visible behavior, API or look the port must match. Write what changed in user terms (a setting, a command, a menu entry, a default, a pixel change), the row, its milestone (from the `Rows:` lines in `port/roadmap.md`) and the row's current status.
- **skip**: a skip-set change, including AI hooks inside kept rows (a chat button in the editor title, an agent context key). Name the skip-set area.
- **n/a**: Electron, web, server or test-only code the port has no counterpart for, as the ledger's `n/a` rows.

New upstream units get a verdict too: a new ledger row (id, area, milestone) or skip. Not-mapped workbench shell files (`layout.ts`, `api/common/extHost*.ts`, `common/*.ts`) get the rows they touch. Proposed API typings are ported only when a built-in extension the port keeps uses them.

Done when every row and file of the tool's output, outside "removed by decision", has a verdict.

## 4. Write the plan

Write `port/plans/uplift-<from>-to-<to>.md`:

1. Summary: both versions, the tool's group counts, lines to port against lines skipped.
2. Setup: check out `<to>` in `~/Developer/vscode-ref` on the Studio, compile it and its extensions (see `port/notes.md` "Reference"), refresh the pixel-check baseline and the api-tests reference numbers (`port/results/api-tests-reference.md`), bump `package.json` and `VERSION`, rerun `bun port/tools/gen-ledger.ts` for new rows.
3. Work items grouped by milestone, each with its row, the change, the upstream files and the check that proves it (ui-pair, api-tests suite, cargo test).
4. Skipped: one line per skip-set area with its line count.
5. A session split: about 3000 changed lines to port per session, grouped by milestone so sessions touch different files.

Then add or refresh the "Uplift to VS Code <to>" milestone in `port/roadmap.md` (scope with the numbers, steps, done when) pointing at the plan, and commit both as `docs(port): plan the uplift to VS Code <to>`.

## 5. Report

To Artur, in Polish: both versions, how much there is to port against how much is skipped, the biggest work items, the proposed sessions, and the plan's path.
