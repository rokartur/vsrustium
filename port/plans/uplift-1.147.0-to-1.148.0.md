# Uplift plan: VS Code 1.147.0 to 1.148.0

## 1. Summary

VSRustium follows 1.147.0 (`package.json`, `VERSION` in `native/crates/app/src/main.rs`); the newest upstream release is 1.148.0 (tag 661626d0e72, 2026-11-24). Upstream changes 2,753 files (+160,130/-49,131), 2,583 of them in `src/vs`, `src/vscode-dts`, `extensions` and `product.json`. Stable `vscode.d.ts` is unchanged.

Refreshed on branch `port-m21-plan` (f30c69689bf) against this branch's ledger: 594 rows, 422 `verified`, 126 `removed`, 28 `n/a`, and 17 not closed yet (12 `ported`, 1 `partial`, 4 `todo`; notebooks, auxiliary windows, modernUI, welcome walkthrough), which other sessions are closing on 1.147.0. Three rows the diff touches are among them: `workbench/contrib/modernUI` (todo), `workbench/contrib/files` (ported) and `workbench/contrib/notebook` (ported).

`bun port/tools/uplift-diff.ts 1.148.0` (non-test lines, added plus deleted):

| group | rows | lines |
|-------|------|-------|
| to port (row not removed) | 79 | 10,606 |
| new upstream unit, no ledger row | 2 | 57 |
| removed by decision | 31 | 78,838 |
| tests only | 8 | test files only |
| API typings | 5 | 202 |
| not mapped | 29 folders | `src/vs/sessions` 16,223 (agents window), workbench shell files about 1,100, 49 workbench test files |

Triage of everything outside "removed by decision" (section 7):

```
port      6,548 lines: about 3,500 to write in Rust, 1,977 grammar data and colorize fixtures,
          about 770 that arrive with upstream or the reference's extensions (git, markdown, extHost, typings),
          308 of the floating comment style (a gap older than the uplift, stays with that gap)
skip      5,223 lines inside kept rows (chat, agent sandbox, agent host, experiments, telemetry)
          + 16,223 src/vs/sessions + 78,838 removed rows = 100,284
n/a       1,908 lines (the empty-editor hint moved into a controller, Electron main, IPC, telemetry-only, moves)
```

So about 3 % of the upstream change reaches the port, under 2 % as Rust. Since the first version of this plan, the empty-editor hint, workspace editing and the action widget view were ported on 1.147.0: the hint rewrite is now a move with no visible change (n/a), and the remove-folder vetoes (265 lines) and the action widget changes (67 lines) are now uplift work.

Most of the work is small. The larger items: SCM providers that hide and show at runtime (`$setSourceControlsHidden`, the git extension's worktree support), the add-tab menu and wrapped tabs, comment widget changes, the multi diff editor's file list, folder merge on drop in the Explorer, named pipes in the disk provider, the remove-folder vetoes, and the Debug Console completion ranges.

## 2. Setup

Milestone 22 (the split) runs before this one: `main` starts from one commit with the port tree, and upstream is a plain checkout of an upstream tag at `upstream/` (`VSCODE_SRC`), detached, with no `vscode` branch. Steps marked **[M22]** depend on that split; the text in brackets says what to do if the uplift ran on today's tree instead.

1. On the Studio: check out `1.148.0` in `~/Developer/vscode-ref`, then `npm ci`, compile it and its extensions (port/notes.md "Reference"). Run `port/tools/fetch-builtin-extensions` again.
2. **[M22]** Move upstream to 1.148.0. 1.148.0 is not a descendant of 1.147.0 (`git merge-base --is-ancestor 1.147.0 1.148.0` fails: each release is its own branch); `upstream/` is never edited, so check out the tag: `git -C upstream fetch origin tag 1.148.0 --no-tags`, then `git -C upstream checkout 1.148.0`. [Today's tree: `git merge 1.148.0`.]
3. **[M22]** The port's 9 edits to upstream files (`src/bootstrap-exthost.ts`, `ipc.net.ts`, `extHostInertModules.ts`, `extHostRequireInterceptor.ts`, `extHostExtensionService.ts`, `extensionHostProcess.ts`, `proxyResolver.ts`, `product.json`, `extensions/configuration-editing/package.json`; 236 added, 19 deleted) have no upstream change in 1.148.0, so `port/patches/*.patch` and the `product.json` overlay apply unchanged. Check: `git -C upstream apply --check port/patches/*.patch`, then rebuild the extension host (`port/tools/build-exthost`). [Today's tree: the merge has no conflicts in these files.]
4. **[M22]** Run `bun port/tools/gen-proxy-ids.ts` again; after the split it reads `extHost.protocol.ts` under `VSCODE_SRC`. 1.148.0 appends `MainThreadRemoteAgentHosts` to `MainContext`, so every `ExtHostContext` id after it moves by one. The port has to answer the new proxy as inert, the same as the other removed agent proxies. Check: `cargo test -p exthost`, then exthost-probe 30/30 on Bun and Node.
5. Run the reference dumps again: `port/tools/dump-settings` (core settings and schemas), `port/tools/ref-commands` (commands, context keys), `ref-contextmenu`, `ref-menubar`, `ref-hover` and `ref-dom` where the results use them. These dumps bring in the settings changes without code: `workbench.editor.newTabButton` (renamed from `workbench.editor.experimental.newTabButton`, with a migration), the removed `multiDiffEditor.experimental.fileList`, the new `update.enableEarlyStaging`, `markdown.editor.initialMode` and `workbench.editor.experimental.emptyEditorHintOpenInMarkdownEditor`.
6. Refresh the pixel-check baseline and the api-tests reference numbers (`port/results/api-tests-reference.md`). 1.148.0 adds `browser.tools.test.ts` cases (integrated browser, removed) and an `http.noProxy` step in `proxy.test.ts`.
7. **[M22]** Bump the version to 1.148.0: `VERSION` in `native/crates/app/src/main.rs`, the ledger header, and the port's version source that milestone 22 picks (today the root `package.json`, which after the split is upstream's own file under `upstream/`). `uplift-diff.ts` takes its default `from` from that source, so the next uplift needs it right. Run `bun port/tools/gen-ledger.ts` (it reads upstream through `VSCODE_SRC` after the split). The two new units are skipped: `mainthread/MainThreadRemoteAgentHosts` becomes `removed`, and `mainthread/MainThreadGitExtensionService` stays with the `removed` row `mainthread/MainThreadGitExtension`.
8. **[M22]** Run `bun port/tools/uplift-diff.ts 1.148.0 1.147.0` once more on the split tree and compare its group counts with section 1; it reads the tags from `upstream/`.

## 3. Work items by milestone

Lines are upstream lines (added plus deleted). Each item names its row, the row's status before the uplift, and the check that proves it.

### M7 Syntax colors and themes

- `extension/*` grammars (verified): new upstream grammar commits for cpp, csharp, dart, dotenv, go, java, latex, log, php, razor, rust and scss (1,265 lines of data). They arrive with the reference's extensions. Check: `cargo test -p app syntax` (`colorize_fixtures_match_the_reference`; after the merge, `test-80644_cpp`, `test_env` and `test_tex` change).
- `extension/theme-defaults` (verified), 8 lines: in Light 2026, `editorGroupHeader.connectedTabsBackground` changes from #EAEAEA to #F0F0F3, `tab.divider` is #CECECE and `activityBarTop.inactiveForeground` is #606060. In Light Modern, the quick pick's focused row is #005FB8 with #FFFFFF foreground, icon and highlight. Check: ui-pair 2026-light tab bar and top activity bar, light_modern Cmd+P; the defaultThemeColors.test.ts cases in `cargo test -p app theme`.
- `workbench/services/themes` (verified), 26 lines: modern tabs fall back to `editorGroupHeader.tabsBorder` for the tab border, and `tab.divider` follows a customized `tab.border` unless `tab.divider` is customized itself (`tabDividerColor.ts`). Check: `cargo test -p app theme` with the 3 cases of the new test.
- `editor/contrib/suggest` (verified, M13 row, done here with the colors), 6 lines: the default of `editorSuggestWidget.focusHighlightForeground` becomes `quickInputList.focusHighlightForeground`. When the theme sets `editorSuggestWidget.selectedBackground`, it is `list.focusHighlightForeground` instead (`ifDefinedThenElse`). Check: `cargo test -p app theme` color defaults.

### M8 Editor viewport and editing commands

- `editor/browser` + `editor/contrib/folding` (verified), 71 lines: a click in the empty gutter places the caret. A click in the line-decorations lane with no decoration goes to the line start; only the 0.625em folding chevron toggles; with `editor.mouseStyle: text` the I-beam shows over the empty gutter. A click right of an empty line gives column 1. An empty line with injected text on both sides puts the caret before the after-decoration. Check: `cargo test -p app editor::view` mouse-target cases; ui-pair of the gutter hover.
- `editor/common` (verified), 24 lines: `tabFocusMode` is computed from the global Ctrl+M toggle, so `editorTabMovesFocus` follows it. Hex colors count only before whitespace, a quote, `;`, `,`, `)` or the end of the line. Check: `cargo test -p app tab_focus document_colors`.
- `editor/contrib/contextmenu` (verified), 6 lines: a right click on injected text (inlay hints, ghost text) opens no menu and keeps the selection. Check: `cargo test -p app editor_menus`.
- `editor/contrib/links` + `editor/contrib/hover` (verified), 22 lines: the hover copy button copies `hoverMessageCopyContent`; for a link, that is the URL. Check: `cargo test -p app content_hover links::`.
- `workbench/parts/editor` (verified), about 430 lines:
  - Add-tab (`+`) menu: the entry picked last (`editor.addTabMenu.recentEntry.<menuId>`, profile storage) comes first with "recently used". A second press on `+` while the menu is open picks it.
  - New-tab entries open pinned at the end of the group with the button's editor options (`newTabButton.ts`, `newTabButtonActions.ts`).
  - Wrapped tabs: the `+` button never sits alone on a row, and a tab dropped on `+` goes last (`multiEditorTabsControl.ts`, css).
  - The "Hold Shift to drop into editor" label gets body1 font, 4/8 padding, a small radius and CSS colors (`editorDropTarget.ts`, `dnd.css`).
  - The watermark's "Start Debugging" icon becomes `run`.
  - Check: ui-pair of the open add-tab menu, wrapped tabs and the empty editor; `cargo test -p app new_tab tabs_wrap`.
- `workbench/contrib/codeEditor` (verified), 1,022 lines of the empty-editor hint: upstream moves the hint into `emptyTextEditorHintController.ts` and `emptyEditorHintReopen.ts` with the same text, links and keys, so `workbench/empty_editor_hint.rs` stays. The new "open in Markdown Editor" link is part of the Markdown Editor experiment (skip). Check: `cargo test -p app empty_editor_hint`; ui-pair `port/fixtures/ui-steps/empty-editor-hint.json` against 1.148.0. `simpleFindWidget.ts` (64 lines) belongs to M15.

### M9 Commands, keybindings, menus, quick input

- `platform/actions` (verified), 49 lines: when a split button's primary action is checked, both parts paint `actionBar.toggledBackground`; a disabled dropdown part is drawn at 0.4 opacity; `isAlwaysShown` and `excludePrimaryActionFromDropdown` are new menu options. The multi diff Toggle Files split button (M14) is a caller. Check: ui-pair of that toolbar; `cargo test -p app menus`.
- `platform/actionWidget` (verified), 67 lines: the checked item is revealed after the first layout, `.panel-anchor` marks the row with an open panel, and `aria-expanded` is set only on expandable rows. Port into `workbench/action_widget.rs` and `action_list.rs`. Check: `cargo test -p app action_list code_actions`; ui-pair of the code action menu.
- `platform/quickinput` (verified), 25 lines: focused tree rows use `quickInputList.focusHighlightForeground`; a focused row's icon uses `quickInputList.focusIconForeground`; selected rows use the focus colors; an anchored picker keeps its last anchor rect. Check: ui-pair Cmd+P with a colored icon focused; `cargo test -p app quick_input`.
- `workbench/services/keybinding` (verified), 26 lines: an edit or removal targets the last user entry that matches command, when, args, systemWide and key. Check: `cargo test -p app user_keybindings` with two same-command entries.
- `workbench/parts/titlebar` (verified), 11 lines: `menubarHidden` changes only for the custom menu bar; on the native macOS menu it has no effect. Check: none.

### M10 Settings, storage, profiles

- `workbench/services/configuration` (verified), about 34 lines: the `workbench.editor.experimental.newTabButton` to `workbench.editor.newTabButton` migration, a new `workbench.activityBar.autoHide` description, and `migrateUserSettingsSeparately` (only the local half applies). Check: `port/tools/settings-check configuration` with a user settings file that holds the old key.
- `workbench/services/preferences` (verified), 2 lines: the Keyboard Shortcuts row id includes `systemWide`. Check: `cargo test -p app keybindings_editor`.

### M11 Workbench layout and chrome

- `workbench/contrib/modernUI` (todo; the modern look is drawn always on), about 110 lines:
  - The connected-tab outline, shoulders and strip separator use `modernPanel.border`, then `editorGroupHeader.tabsBorder`, then `tab.border`.
  - The wrapped `+` button lines up with the first tab of the row above.
  - A drop indicator before the last tab is drawn above the sticky `+`.
  - The dialog's frosted glass sits behind the border.
  - SCM badges and counts get a `spacing-size40` right margin.
  - Check: ui-pair of wrapped connected tabs, a modal dialog and the SCM view.
- `workbench/parts/auxiliarybar` (verified), 35 lines: with `workbench.activityBar.autoHide`, the secondary side bar hides its composite bar when it has one container, at any activity bar location. The Maximize precondition adds AI keys only. Check: ui-pair with auto-hide on.
- `workbench/parts/views` (verified), 26 lines: an icon badge that hides number badges keeps their counts in the title and accessible label; the trailing title-action separator is hidden while the part has no global actions. Check: `cargo test -p app activity_bar`; ui-pair of a panel title.
- `workbench/services/progress` (verified), 49 lines: only the latest reporter hides the editor progress bar, and showing the infinite bar again does not restart it or its signal. Check: `cargo test -p app progress`.
- `workbench/services/decorations` (verified), 53 lines: the `alignBadgeLastCharacter` badge span; SCM rows use it. Check: ui-pair of the SCM view.
- `workbench/services/editor` (verified), about 70 lines: `setDefaultEditor` takes an `associationPattern`, plus `onWillSetDefaultEditor`. The working-set and modal toolbar API has no non-AI caller yet. Check: `cargo test -p app editor_resolver`.
- `workbench/services/views` (verified), 3 lines: title actions are rebuilt once, not twice. Check: existing view tests.

### M12 Files, explorer, search, workspace

- `platform/files` (verified), 214 lines: named pipes. A read waits for a writer and can be cancelled; it fails with "Named pipe has no data and no writer". A write with no reader fails at once with "Named pipe has no reader". Pipes get no retries and no fdatasync. `readFile` takes a cancellation token. Check: `cargo test -p files fifo`.
- `workbench/contrib/files` (ported), 160 lines: when a dropped or moved folder already exists in the target, a dialog asks "A folder with the name '{0}' already exists... merge or replace it?" with Merge, Replace and Cancel. Merge recurses and replaces files; a move deletes the emptied source folders; symlinks are never merged. The Explorer header shows the workspace label without the location suffix. Check: `cargo test -p app explorer merge_folders`; ui-pair of the dialog.
- `workbench/contrib/bulkEdit` (verified), 13 lines: deleting a folder that earlier deletes in the same edit emptied becomes its own step. Check: `cargo test -p app bulk_edit`; api-tests workspace applyEdit.
- `mainthread/MainThreadWorkspace` (verified), 82 lines: new `$getWorkspaceFolderLocations()`. The main thread answers by pushing `$acceptWorkspaceFolderLocations([{folder, locations}])` and pushes again when folders change. The only location provider in 1.148.0 is the agent host worktree contribution (removed), so each folder has only its primary location. `$updateWorkspaceFolders` passes `force`. The git extension enables the `workspaceFolderLocations` proposal and listens to it. Check: api-tests `vscode API - workspace`; the git integration run (port/results/scm.md) 91/2/0 as the reference.
- `platform/workspace` (verified), 5 lines: the `workspaceHasVirtualContent` context key. Check: `cargo test -p app context_keys`.
- `workbench/services/workspaces` (verified), 190 lines: the `onWillRemoveFolders` veto ("Remove the folder '{0}'?", "Stop and Remove") and the options object `{donotNotifyError, force}`, in `workbench/workspace_editing.rs`. The debug veto (M16) and the task veto (M15) use it. Check: `cargo test -p app workspace_editing` with a vetoing participant; ui-pair of the dialog with a running task.

### M13 Language features

- `workbench/contrib/comments` (verified), about 240 lines:
  - A controller's submit-thread button comes first in the reply form and is disabled while the reply is empty.
  - The reply and edit editors measure their height again after a width change; the edit editor is the container width minus 14px.
  - `.picker-only` reactions.
  - The `commentsInput` scheme gets no commenting-range prefetch.
  - `showRangeBar: false` hides the gutter range bar.
  - Cmd+Esc moves to the new `workbench.action.collapseVisibleComments`; `hideComment` stays without a key.
  - Check: `cargo test -p app comment`; ui-pair of thread editing.
- `workbench/contrib/comments` floating style, 256 lines: `comments.thread.experimental.style: floating` was not ported at 1.147.0 either, so these width and placement changes go in with it.
- `mainthread/MainThreadComments` (verified), 5 lines: the Comments view container registers once. Check: `cargo test -p app comment`.

### M14 Source control and diff

- `mainthread/MainThreadSCM` + `workbench/contrib/scm` (verified), about 360 lines:
  - New `$setSourceControlsHidden([handle, isHidden][])`: applied in order after each handle's registration, as one view change. The git extension uses `scmProviderOptions`' settable `isHidden` for hidden warm repositories.
  - Repositories come and go from the view, the picker and the welcome as `isHidden` flips; a later-found repository keeps its earlier hidden state.
  - Sort by path compares segment by segment.
  - Show Structural Diff defaults to shown when `scm.structuralDiff.enabled` is on.
  - Group count badges become circles with tabular numbers.
  - The quick diff gutter hit area is only the bar width.
  - Check: api-tests scm; `cargo test -p app scm quick_diff`; ui-pair of the SCM view; the git integration run.
- `workbench/contrib/multiDiffEditor` (verified), 204 lines:
  - `multiDiffEditor.experimental.fileList` is removed: the file list is always on unless the input says `showFileList: false`.
  - Toggle Files becomes a split button (order 100001) with the new `MultiDiffEditorFileListToggle` menu (List/Tree, then Left/Right).
  - The list no longer hides itself; the diffs shrink first.
  - Check: ui-pair of the multi diff title bar; `cargo test -p app multi_diff`.
- `extension/git` (verified), 18 lines of package.json that need the workbench:
  - The stage, unstage and revert selected-ranges title actions use `!isInDiffLeftEditor`. The port sets neither diff-side key, so these buttons would show in every diff editor: set `isInDiffLeftEditor` and `isInDiffRightEditor`.
  - `git.openMergeEditor` is in the palette when `git.activeResourceHasMergeConflicts`.
  - A new "Opening the Git repository..." scm welcome shows while `git.openingLocation`.
  - Check: `cargo test -p app scm` (diff-side keys); ui-pair of the diff editor title.

### M15 Terminal, tasks, output

- `workbench/contrib/codeEditor` `simpleFindWidget.ts` (terminal find, verified), 64 lines: an incomplete count shows "N+", a provider status replaces the count, `canRetry` keeps next and previous enabled, there is no red outline while the count is incomplete, and late updates are dropped. Check: `cargo test -p app terminal_find`.
- `workbench/contrib/terminal` (verified), 7 lines: the tabs list gets 4px top padding. Check: ui-pair of the terminal tabs.
- `workbench/contrib/tasks` (verified), 27 lines: the remove-folder veto for running tasks ("The task '{0}' is running."), on the M12 veto. Check: `cargo test -p app workspace_editing tasks`.

### M16 Debug and testing

- `workbench/contrib/debug` (verified), 198 lines:
  - Debug Console completions honor DAP `start` and `length`; an item whose range is off the cursor line is dropped.
  - Multi-line history suggestions insert the rest of the entry.
  - Disassembly breakpoints are matched by address.
  - The remove-folder veto ("A debug session is running."), on the M12 veto.
  - Check: `cargo test -p app repl disassembly` (the replCompletions cases).

### M17 Webviews and notebooks

- `workbench/contrib/mediaPreview` (verified), 249 lines: the video player's fullscreen control opens the video in the modal editor; Escape returns it inline and playback continues. Check: ui-pair of the video in the modal.
- `workbench/contrib/webview` + `webviewPanel` (verified), 42 lines: `allowFullscreen` (iframe `allow=fullscreen`); unmounted frames stop loading; copy, cut and paste from a menu focus the webview first; a webview editor claims the webview only for its own group. Check: `port/tools/webview-check`; a copy from the Edit menu.
- `extension/markdown-language-features` (verified): the new `markdown.editor.initialMode` (read or edit, default read) is read by the extension itself. Check: settings-check lists it; `FIXTURE=markdown-tests port/tools/webview-check`.

### M18 Extensions

- `workbench/contrib/extensions` (verified), about 125 lines: Copy Link's `when` becomes `extensionHasGalleryLink` (set when the extension has a url), replacing `galleryHasExtensionLink`. The Manage gear in the extension editor hides when the ExtensionContext menu has nothing for the extension. Check: ui-pair of the extension editor header and context menu.
- `platform/extensions` (verified), 3 lines: the `workspaceFolderLocations` proposal arrives with the merge.

### M19 Product shell

- `platform/update` (verified), 11 lines: `update.enableEarlyStaging` (macOS, default false, experimental) is registered by the settings dump and has no effect on the port's GitHub-release updater. Check: settings-check.

## 4. Skipped

- AI, chat and sessions: `workbench/contrib/chat` 19,510, `workbench/contrib/sessions` 24,515, `src/vs/sessions` 16,223, `extension/copilot` 12,249, plus `inlineChat`, `chatWindow`, `parts/chat`, `canvases`, `sessionChanges`, `screenshotTool`, `imageCarousel` and `platform/chat`, about 1,000. Inside kept rows: the chat part in `layout.ts` and `layoutService.ts`, layout actions and context keys (571), `chatImagePreview.ts` and `chatPills.ts` (129), the compact chat window split (28), the Agents window defaults for the Markdown editor (72), editor comments in the Agents window (about 90), and the Agents window narrow features tab (21).
- Agent host and its tools: `platform/agentHost` 15,366, `workbench/services/agentHost` 34, `agentPlugins` 20, the terminal agent sandbox profiles editor (1,302), the agent host sync scope in `request.ts` (9), the folder-location UI whose only 1.148.0 provider is the agent host worktree (about 350), `artifactIntegrations` 236 and the chat artifact proxy (12).
- Remote and tunnels: `remoteTunnel` 50, `MainThreadRemoteAgentHosts` plus `extHostRemoteAgentHosts` plus `remoteAgentHost.d.ts` (134), `serverAgentHostManager.ts` (23).
- Sign-in and GitHub services: `platform/github` 3,122, `workbench/services/github` 171, `workbench/contrib/github` 163, `MainThreadGitHub` 110.
- Microsoft services and experiments: `issue` 490, `onboarding` 375, `customizationMarketplace` 2, update experiments (157), the Markdown Editor promotion nudge (496) and the empty-editor hint's Markdown Editor link (87), experiment flags on settings (9).
- Telemetry: the editor and default-editor telemetry (210), the empty-hint and Markdown Editor telemetry (about 260), the structural diff telemetry (57), `_workbench.getUntitledDocumentId` (5).
- Integrated browser: `platform/browserView` 717, `workbench/contrib/browserView` 203, api-tests `browser.tools.test.ts`.
- Speech: the `dictationDisconnected` signal and setting (12).

## 5. Sessions

About 3,500 lines go into Rust, plus the setup. That makes three sessions, split by milestone so they touch different files:

| session | scope | Rust lines | starts after |
|---------|-------|-----------|--------------|
| 1 | setup (section 2), then M7, M8, M9, M10, M11 | about 1,300 + setup | the 17 open rows closed on 1.147.0, milestone 20's full check, milestone 22 |
| 2 | M12 and M14: files, workspace folder locations, the remove-folder vetoes (with the M15 task and M16 debug parts), SCM `isHidden`, multi diff, git keys | about 1,200 | session 1's upstream move and proxy ids |
| 3 | M13, M15, M16, M17, M18, M19 (without the vetoes) | about 1,000 | session 1's upstream move |

Sessions 2 and 3 can run in parallel worktrees after session 1. The modernUI item (M11) and the Explorer folder merge (M12) wait for their rows (`todo`, `ported` today) to close on 1.147.0 first.

## 6. Done when

The ledger header names 1.148.0. Every row in section 3 is `verified`, `removed` or `n/a` again, or keeps the status it had at 1.147.0 when the gap is older than the uplift (the floating comment style). The full check passes against the 1.148.0 reference: api-tests whole folder, pixel-check, `cargo test`, settings-check, and the git integration run.

## 7. Triage

One line per change: verdict | row | file | lines | change or reason. Line counts are upstream lines, added plus deleted.

### to port

```
n/a  | workbench/contrib/codeEditor (verified, M8)   | emptyTextEditorHint, emptyTextEditorHintController, emptyEditorHintActions | 935 | code moved into a controller and a reopener; same text, links and keys
skip | workbench/contrib/codeEditor              | emptyEditorHintReopen, the classicMarkdownEditor variant | 87 | Markdown Editor link (`workbench.editor.experimental.emptyEditorHintOpenInMarkdownEditor`, experiment, default false)
skip | workbench/contrib/codeEditor              | markdownEditorPromotion, markdownEditorNudgeContext, codeEditor.contribution | 496 | experiments, Markdown Editor nudge
n/a  | workbench/contrib/codeEditor              | emptyTextEditorHintTelemetry | 10 | telemetry variant
port | workbench/contrib/codeEditor (verified, M15)  | find/simpleFindWidget | 64 | incomplete count, status, canRetry
skip | workbench/contrib/terminalContrib         | chatAgentTools/* (5 files) | 1302 | agent terminal sandbox profiles
port | extension/git (verified, M14)             | package.json | 18 | !isInDiffLeftEditor, openMergeEditor in the palette, openingLocation welcome, workspaceFolderLocations proposal
port | extension/git (verified, M14)             | model.ts, workspaceFolderRepositories.ts | 437 | extension code from the reference; needs $getWorkspaceFolderLocations and $setSourceControlsHidden
n/a  | extension/git                             | package.nls.json, artifactProvider, commands, fileSystemProvider, git, hover, repository, timelineProvider, util, tsconfig | 429 | extension code from the reference, existing main-thread calls
port | workbench/parts/editor (verified, M8)       | editorAddTabControl, editorTabsControl, editorPart, editor.ts, editorAddTabControl.css | 186 | recently used add-tab entry, second press picks it
port | workbench/parts/editor (verified, M8)       | multiEditorTabsControl, multieditortabscontrol.css | 65 | + never alone on a row, drop on + goes last
port | workbench/parts/editor (verified, M8)       | editorDropTarget, editordroptarget.css | 29 | drop label look
port | workbench/parts/editor (verified, M8)       | editorGroupWatermark | 2 | run codicon
port | workbench/parts/editor (verified, M8)       | breadcrumbs, breadcrumbsControl | 50 | editor type dropdown onDidOpen (no visible change)
port | workbench/parts/editor (verified, M8)       | editorGroupView | 41 | header inset and single-tab title options (internal)
skip | workbench/parts/editor                    | editorCommands, editorTelemetry, defaultEditorTelemetry, editor.contribution, editorGroupView descriptor | 210 | telemetry
skip | workbench/parts/editor                    | auxiliaryEditorPart, editorParts (compactAllowsSplit), modalEditorPart, contentPart | 36 | AI compact chat window
skip | workbench/parts/editor                    | editorConfiguration | 23 | Agents window Markdown default
port | workbench/parts/editor (verified, M11)      | editorTypePicker, newTabButton | 12 | association pattern; newTabButton setting id, pinned at the end
port | workbench/contrib/comments (verified, M13)| commentFormActions, commentReply, commentNode, commentService, commentsController, commentCommandIds, commentsEditorContribution | 304 | submit-thread button, editor heights, showRangeBar, collapseVisibleComments
port | workbench/contrib/comments (verified, M13)| commentThreadFloatingWidget, ...Layout, commentThreadWidget, review.css | 308 | floating style (not ported at 1.147.0)
skip | workbench/contrib/comments                | commentsConfiguration, comments.contribution, commentsController agents part | 7 + ~90 | Agents window, experiments
port | extension/csharp, latex, rust, cpp, java, dotenv, dart, razor, go, log, php, scss (verified, M7) | syntaxes, cgmanifest | 1265 | new grammar commits, data only; colorize fixtures
port | extension/markdown-language-features (verified, M17) | package.json, package.nls.json, markdownEditorProvider | 99 | markdown.editor.initialMode; @vscode/markdown-editor bump
skip | extension/markdown-language-features      | markdownEditorTelemetry, telemetryReporter, extension*.ts | 253 | telemetry
n/a  | extension/markdown-language-features      | markdownEditorLanguageFeatures, markdownEditorRename | 4 | extension code from the reference
port | workbench/contrib/extensions (verified, M18) | extensions.contribution, extensionsCopyActions, extensionsActions | 129 | extensionHasGalleryLink, Manage gear hides
n/a  | workbench/contrib/extensions              | uninstallExtension move, extensionsSlowActions | 111 | moved code; Electron-only refactor
skip | workbench/contrib/extensions              | extensionEditor, extensionFeaturesTab | 21 | Agents window layout
skip | workbench/contrib/workspaces              | folderLocationExplorerBadge, folderLocationStatusBar, folderLocationPersistence, workspaces.contribution | 258 | folder-location UI; the only provider is the agent host worktree
n/a  | workbench/contrib/workspaces              | folderLocationWorkingSets | 5 | trace logging
port | workbench/contrib/scm (verified, M14)     | scm.ts, scmService, scmViewService, scmViewPane, scm.contribution, scm.css, quickDiffDecorator | 200 | isHidden observable, sort by path segments, structural diff default, count badge, gutter hit area
skip | workbench/contrib/scm                     | scmStructuralDiff | 57 | telemetry
port | workbench/services/workspaces (verified, M12) | abstractWorkspaceEditingService, workspaceEditing | 190 | onWillRemoveFolders veto, options object
skip | workbench/services/workspaces             | workspaceFolderLocationService | 60 | folder-location groups; agent host provider only
port | workbench/contrib/mediaPreview (verified, M17) | mediaPlayerEditorInput, mediaPlayerEditor.css | 249 | video fullscreen in the modal editor
port | platform/files (verified, M12)            | diskFileSystemProvider, files, fileService, io, diskFileSystemProviderClient | 225 | named pipes, cancellable readFile
n/a  | platform/files                            | diskFileSystemProviderServer | 16 | IPC channel
port | workbench/contrib/multiDiffEditor (verified, M14) | all 6 files | 204 | file list always on, Toggle Files split button, no auto-hide
port | workbench/contrib/debug (verified, M16)   | repl, replCompletions, disassemblyView | 150 | completion ranges, multi-line history, breakpoints by address
port | workbench/contrib/debug (verified, M16)   | debugService, debugUtils | 48 | remove-folder veto
port | workbench/contrib/files (ported, M12)     | fileImportExport, explorerViewer, explorerView, newTabButtonActions | 166 | folder merge dialog, header label, new-tab options
skip | workbench/contrib/files                   | files.contribution, files.ts | 6 | telemetry id, sessions container
n/a  | platform/native                           | nativeHostMainService, mouseMoveForwarder, power | 153 | Electron main
skip | platform/native                           | native.ts | 4 | Agents launcher
skip | workbench/contrib/update                  | unattendedRestartExperimentTrigger, updateExperiments, update.contribution | 157 | experiments
port | workbench/contrib/modernUI (todo, M11)    | connectedEditorTabs.css, tabs.css, frostedGlass.css, roundedCorners.css, padding.css, README.md | 110 | tab outline colors, wrapped +, dialog glass, SCM margins
n/a  | workbench/contrib/modernUI                | modernUI.contribution | 27 | DOM cleanup at shutdown
port | platform/update (verified, M19)           | update.config.contribution | 11 | update.enableEarlyStaging
n/a  | platform/update                           | update.ts, updateIpc, electron-main/* | 123 | Squirrel early staging
skip | mainthread/MainThreadGitHub               | mainThreadGitHub | 110 | GitHub sign-in services
port | workbench/services/editor (verified, M11)   | editorResolverService (common, browser), openWithEditorPicker, editorGroupsService | 85 | associationPattern, onWillSetDefaultEditor, working-set API
skip | workbench/services/editor                 | editorResolverService Agents window enum | 49 | Agents window Markdown default
skip | workbench/services/layout                 | layoutService | 82 | chat part layout
skip | workbench/contrib/imageCarousel           | all 5 files | 73 | chat image carousel
port | mainthread/MainThreadWorkspace (verified, M12) | mainThreadWorkspace | 69 | $getWorkspaceFolderLocations, force on update
port | platform/actionWidget (verified, M9)        | actionList, actionWidget.css | 67 | reveal after layout, panel anchor, aria-expanded
port | platform/actions (verified, M9)           | actions.ts, toolbar, menuEntryActionViewItem (.ts, .css) | 55 | checked split button, isAlwaysShown, excludePrimaryActionFromDropdown
n/a  | platform/actions                          | menuService | 7 | cache key cleanup
port | editor/browser (verified, M8)               | mouseHandler, mouseTarget, view, viewLine | 55 | gutter click, I-beam, empty-line caret
n/a  | editor/browser                            | codeEditorWidget | 4 | key reads the computed option
port | workbench/services/progress (verified, M11) | progressIndicator | 49 | latest reporter hides, no restart
port | platform/workspace (verified, M12)        | virtualWorkspace | 7 | hasVirtualWorkspaceContent
skip | platform/workspace                        | folderLocations | 35 | folder-location groups; agent host provider only
port | editor/common (verified, M8)                  | editorOptions, defaultDocumentColorsComputer | 24 | tabFocusMode computed, hex color boundary
port | editor/common (verified, M13)                 | model.ts, textModel.ts | 6 | hoverMessageCopyContent
n/a  | editor/common                             | externalLinesDiffComputer | 8 | Monaco-only comments
skip | editor/common                             | languages.ts | 2 | showRangeBar typing (the behavior is ported under comments)
port | workbench/contrib/webview (verified, M17)   | webview, webviewElement, webviewMessages.d.ts, pre/index.html | 36 | allowFullscreen, unmounted frames, menu clipboard focus
port | workbench/parts/auxiliarybar (verified, M11)| auxiliaryBarPart, auxiliaryBarActions | 35 | autoHide at any location; AI keys in Maximize
port | mainthread/MainThreadSCM (verified, M14)  | mainThreadSCM | 30 | $setSourceControlsHidden
port | workbench/contrib/tasks (verified, M15)   | abstractTaskService, taskService | 27 | remove-folder veto
port | workbench/services/keybinding (verified, M9) | keybindingEditing | 26 | edit targets the matching last entry
port | workbench/services/themes (verified, M7)    | modernTabColorCustomizations, tabDividerColor | 26 | tab border and divider fallback
port | platform/quickinput (verified, M9)        | quickInput.css, quickInputController, quickInputService, quickInput.ts | 25 | focus colors, anchored picker rect
n/a  | platform/configuration                    | configurationModels | 18 | cache only
skip | workbench/contrib/accessibility           | accessibilityConfiguration | 18 | sandbox profiles, dictation
skip | platform/accessibilitySignal              | accessibilitySignalService | 6 | dictation
skip | platform/accessibility                    | accessibleView | 2 | sandbox profiles, chat sessions list
port | editor/contrib/folding (verified, M8)     | folding.css | 15 | chevron-only hit area
port | editor/contrib/hover (verified, M13)      | contentHoverRendered, hoverTypes, markdownHoverParticipant | 14 | copy content
port | workbench/contrib/bulkEdit (verified, M12)  | bulkFileEdits | 13 | parent delete after children
n/a  | workbench/services/host                   | browserHostService | 4 | web host; force on folder update
skip | platform/window                           | window.ts | 11 | Agents launcher options
port | workbench/parts/titlebar (verified, M9)   | menubarControl | 11 | menubarHidden; no effect with the native menu
n/a  | workbench/contrib/keybindings             | systemWideKeybindings.specification.md | 11 | doc only
skip | platform/request                          | request.ts | 9 | agent host sync scope
n/a  | workbench/contrib/notebook                | emptyCellEditorHint | 9 | follows the hint refactor; no notebook hint in the port
n/a  | extension/github                          | package.json | 4 | welcome when clauses use the extension's own key
port | extension/theme-defaults (verified, M7)   | 2026-light.json, light_modern.json | 8 | strip, divider, quick pick focus colors
port | workbench/contrib/terminal (verified, M15)| terminal.css, terminalTabbedView | 7 | tabs list padding (chat entry part skipped)
port | workbench/services/decorations (verified, M11) | decorationsService, decorations.ts | 7 | alignBadgeLastCharacter option
port | editor/contrib/contextmenu (verified, M8) | contextmenu | 6 | no menu on injected text
port | editor/contrib/suggest (verified, M13)    | suggestWidget | 6 | focusHighlightForeground default
port | platform/extensions (verified, M18)       | extensionsApiProposals | 3 | workspaceFolderLocations
skip | platform/extensions                       | extensionsApiProposals | 3 | remoteAgentHost
port | workbench/contrib/webviewPanel (verified, M17) | webviewEditor | 6 | claim only for its own group
skip | platform/editor                           | editor.ts | 5 | compactAllowsSplit
port | mainthread/MainThreadComments (verified, M13) | mainThreadComments | 5 | one view container
port | workbench/services/views (verified, M11)    | viewsService | 3 | title actions once
skip | platform/dialogs                          | dialog.ts | 2 | Copilot SDK line in About
skip | platform/product                          | product.ts | 2 | copilotVersions rename
port | workbench/services/preferences (verified, M10) | keybindingsEditorModel | 2 | systemWide in the row id
port | editor/contrib/links (verified, M8)       | links.ts | 1 | copy the URL
n/a  | platform/windows                          | windowImpl | 1 | Electron fullscreen flag
```

### New upstream units

```
skip | mainthread/MainThreadRemoteAgentHosts (new row, removed) | mainThreadRemoteAgentHosts | 36 | remote agent hosts
n/a  | mainthread/MainThreadGitExtensionService | mainThreadGitExtensionService | 21 | client of the removed IGitService (row mainthread/MainThreadGitExtension removed)
```

### Tests only

```
port | extension/vscode-api-tests (ported, M6)   | proxy.test.ts | 5 | http.noProxy step; reference numbers refreshed in setup
skip | extension/vscode-api-tests                | browser.tools.test.ts | 199 | integrated browser
port | extension/vscode-colorize-tests (verified, M7) | colorize-results (cpp, env, tex) | 712 | fixtures for the grammar bumps
n/a  | base/test, editor/test, platform/mcp, workbench/contrib/relauncher, workbench/services/label | test files | - | upstream unit tests; the label and relauncher cases cover agent and folder-location labels
port | workbench/contrib/themes (verified, M9)   | defaultThemeColors.test.ts | 74 | cases for the M7 theme item
```

### API typings

```
port | mainthread/MainThreadWorkspace (M12) | vscode.proposed.workspaceFolderLocations.d.ts | 61 | used by the git extension
port | mainthread/MainThreadSCM (M14)       | vscode.proposed.scmProviderOptions.d.ts | 8 | settable isHidden, used by the git extension
skip | -                                    | chatArtifactIntegrations, contribChatResources, remoteAgentHost | 75 | AI, remote; no kept extension enables them
```

### Not mapped

```
skip | -                                       | src/vs/sessions (293 files) | 16223 | agents window
n/a  | -                                       | src/vs/code/electron-main/app.ts | 6 | Electron permission for webview fullscreen
skip | -                                       | src/vs/server (2 files) | 23 (+ test) | agent host on the server
n/a  | -                                       | src/vs/workbench tests (49 files) | - | upstream tests and component fixtures
port | mainthread/MainThreadWorkspace, MainThreadSCM | api/common/extHost.protocol.ts | 14 | $getWorkspaceFolderLocations, $acceptWorkspaceFolderLocations, $setSourceControlsHidden; proxy ids move (setup step 3)
skip | -                                       | api/common/extHost.protocol.ts | 20 | remote agent host shape, chat artifact DTOs
port | (merge) mainthread/MainThreadWorkspace, MainThreadSCM | extHost.api.impl.ts, extHostWorkspace.ts, extHostSCM.ts | 147 | extension host side, arrives with the merge
skip | -                                       | extHost.api.impl.ts, extensionHost.contribution.ts, extHostRemoteAgentHosts.ts, extHostChatArtifactIntegrations.ts | 60 | remote agent hosts, chat artifacts
skip | workbench/services/layout               | browser/layout.ts, actions/layoutActions.ts, browser/contextkeys.ts (chat keys), common/contextkeys.ts | 489 | chat part and agents layout
port | platform/workspace (M12)                | browser/contextkeys.ts | 5 | workspaceHasVirtualContent
port | workbench/parts/editor (M8)             | browser/dnd.ts, media/dnd.css | 98 | drop label look (chat pass-through part skipped)
port | workbench/parts/views (M11)             | parts/compositeBarActions.ts, paneCompositePart.ts, paneCompositePart.css | 26 | badge counts in the label, trailing separator
port | workbench/services/decorations (M11)    | browser/labels.ts | 46 | alignBadgeLastCharacter rendering
port | workbench/services/configuration (M10)  | workbench.contribution.ts, common/configuration.ts | 61 | newTabButton rename and migration, autoHide text, separate migrations (Markdown hint experiment setting skipped)
skip | -                                       | chatImagePreview.ts, chatPills.ts, common/chatSessionCommands.ts, workbench.common.main.ts | 144 | chat, sessions
skip | workbench/parts/editor                  | common/editor.ts | 8 | showFileList for session changes (the multi diff behavior is ported under M14)
n/a  | workbench/services/host                 | electron-browser/window.ts | 2 | force on folder update
```
