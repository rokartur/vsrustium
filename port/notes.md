# Port notes

Decisions, traps and findings that the ledger rows point to. Newest at the end of each section.

## Layout

- `native/`: Cargo workspace. `crates/exthost` (framing, RPC, handshake, init data, session dispatcher, `exthost-probe` binary), `crates/app` (`code-gpui`, the GPUI app).
- `src/bootstrap-exthost.ts`: entry of the extension host bundle (`port/tools/build-exthost` writes `out-exthost/bootstrap-exthost.js`). Same bundle on Bun and Node.
- `port/tools/studio <cmd>`: syncs the tree to the Mac Studio (`~/Developer/vscode-gpui-work/port-src`) and runs `<cmd>` there. All builds and runs go through it.
- Reference: untouched 1.147.0 checkout at `~/Developer/vscode-ref` on the Studio (compiled, extensions compiled). Wu `a3ced28` at `~/Developer/wu`.

## Extension host seam

- The Rust parent listens on a Unix socket and passes it as `VSCODE_EXTHOST_IPC_HOOK`; framing is upstream `PersistentProtocol` (13-byte header, ack frames), RPC is `rpcProtocol.ts`. No Electron message ports.
- Proxy ids are creation order in `extHost.protocol.ts`, starting at 1. `port/tools/gen-proxy-ids.ts` writes `proxy_ids.rs` keyed by the MainContext/ExtHostContext key, not the identifier string: `MainThreadLanguageModelTools` is created as `'MainThreadChatSkills'`, and 8 more keys differ from their strings. Rerun the generator when the protocol file changes.
- The local host is `EagerManualStart`: init data has `autoStart: false` and main sends `$startExtensionHost`. With `autoStart: true` the host races the explicit call and rejects it ("Extension host is already started!").
- Startup calls main must make before `$startExtensionHost` (they come from MainThread* constructors upstream): `$initializeConfiguration`, `$initializeWorkspace`, `$initializeTelemetryLevel`, `$acceptEditorTabModel`. Without the tab model, typescript-language-features fails on `window.tabGroups.activeTabGroup`.
- Main must answer `MainThreadWindow.$getInitialState` with `{isFocused, isActive}` and `MainThreadLanguageModelTools.$getTools` with an array, or the host logs unhandled rejections. Every other MainThread* method currently answers `undefined` (ledger: `stub` rows).

## Bun (1.4.5) as the extension host runtime

Host changes, all gated on `process.versions.bun` so Node behaves as upstream:

1. `extensionHostProcess.ts`: skip `@vscode/native-watchdog`. Bun panics on its `uv_thread_create` (oven-sh/bun#18546). The 1 s parent-pid poll still runs; lost: the watchdog that kills a host whose event loop is stuck. The Rust parent kills the child on drop.
2. `extHostExtensionService.ts` CommonJS: Bun's `require()` does not call `Module._load`, so the `vscode` (and other factory) modules are served from `Module._resolveFilename` through `require.cache` under `/__vscode_module__/<uuid>/<name>`, one key per API instance.
3. `extHostExtensionService.ts` ESM: Bun has no `module.registerHooks`, and runtime `onResolve` plugins never see bare specifiers. A synchronous `Bun.plugin` `onLoad` rewrites `from 'vscode'` / `import('vscode')` in ESM extensions to the same `data:` URL the Node hook resolves to. Constraints found: `onLoad` must return an object (no pass-through), must be synchronous (an async one breaks every `require()` of a matched file), and loading a CommonJS file through it turns it into ESM, so the filter only matches `.mjs` and `.js` files of `"type": "module"` extensions outside `node_modules`. Known limit: ESM extensions added after startup are not matched (code comment `shortcut:`).
4. Bun is started with `--conditions=module-sync`: Node resolves `require()` of ESM-only packages through that condition by default (git requires `file-type`, which has only `import`/`module-sync` exports).

## Copilot Chat

`GitHub.copilot-chat` (built in at 1.147.0) does not finish activating within 60 s on either runtime with the stub main thread; it waits on auth/language-model main-thread calls. It is a Microsoft service client.

Decision (Artur): the port ships no sign-in and no AI features. Copilot Chat, chat views, inline chat, language models, MCP and agent parts are removed, and so are the accounts UI and Settings Sync sign-in. The extension API stays as an inert surface, like telemetry: `vscode.authentication`, `vscode.lm` and `vscode.chat` report no providers, so third-party extensions keep loading. Ledger rows for these parts become `removed` (by decision), not `todo`.

Decision (Artur), later: also removed are the remaining AI parts (embeddings, code mapper, speech, transcription, canvases, session changes), Microsoft services (assignment/experiments, share, surveys, issue reporter, emergency alerts, marketplace customizations, data channels, OTel, external services), edit telemetry, and remote/tunnels (SSH, containers, port forwarding, managed sockets, the test resolver). The port keeps one local extension host; their APIs stay inert. Upstream test infrastructure rows are `n/a`.

## Milestone 2: empty workbench

Scope: the window the reference opens with no folder (title bar with command center and layout actions, activity bar, editor part with the letterpress watermark, no-folder status bar) in the modern floating layout, drawn by GPUI from the built-in theme JSON, in Dark Modern, Light Modern and Default High Contrast.

Done when: `port/tools/pixel-check` diffs all three themes; every non-anti-aliased difference left is listed in `port/results/pixels.md` with its cause; the theme tests pass (`cargo test -p app`); the benchmark is rerun with the workbench drawn.

- Theme resolution follows `ColorThemeData`: the `include` chain merged with the including file winning, then the `registerColor` default for the keys the shell paints (`theme.rs` `registry_default`). A key with no value is transparent, like an unset CSS variable.
- `--theme` takes the settings id (`workbench.colorTheme`), not the extension id.
- Codicons are drawn from `@vscode/codicons/src/icons/*.svg`, not `codicon.ttf`: Wu's macOS text system skips any font without an `m` glyph (`gpui_macos` `load_family`). The SVGs and the letterpress are read at runtime from the checkout, not embedded; codicons are CC-BY-4.0, the same files upstream ships.
- `port/tools/ref-screenshot` brings the page to front and waits until the title bar is not `.inactive`; without it a capture could show the inactive title bar or the previous theme.
- shortcut: the port always draws the active-window look (no `titleBar.inactive*`, `window.inactiveBorder`); add it with window focus events.
- shortcut: the window border radius is the macOS 26+ value (16px); older macOS uses 10px.
- shortcut: High Contrast Light has no registry defaults yet; loading it fails with "not ported".

## Milestone 3: open a folder

Scope: `code-gpui <folder>` passes the folder to the extension host as its workspace (`init_data` and `$initializeWorkspace`, the same `IWorkspaceData` as `exthost-probe --workspace`) and draws the window the reference shows with that folder open: Explorer active in the activity bar, the side bar with the folder tree and the collapsed Outline and Timeline sections, the folder name in the command center, the folder status bar. Clicking a folder row expands or collapses it.

Done when: `port/tools/pixel-check` diffs the empty and the folder window (`port/fixtures/workspace`) in all three themes, with every non-anti-aliased difference listed in `port/results/pixels.md`; `cargo test -p app` passes with the tree test.

- The tree (`explorer.rs`) is read from disk on every render: folders first, then names compared case-insensitively (`FileSorter` default), with `files.exclude`'s default names hidden.
- pixel-check copies the fixture outside the repository, so the git extension finds no repository and the reference draws no SCM decorations.
- The reference runs with `security.workspace.trust.enabled: false` (no trust dialog) and `workbench.secondarySideBar.defaultVisibility: hidden` (with AI off the secondary side bar is empty).
- shortcut: no file icon theme; the reference is captured with `workbench.iconTheme: null`. Port Seti when the editor opens files.
- shortcut: no file watcher; the tree shows the disk at the last redraw. Add a watcher with file operations.
- shortcut: names sort with plain lowercase comparison, not `compareFileNamesDefault`'s numeric collation (`file10` before `file9`).
- shortcut: an unreadable folder shows no children, without the reference's error.
- shortcut: rows have no focus outline, hover, keyboard navigation or scrolling; a tree taller than the side bar is clipped. Selection came with milestone 4.

## Milestone 4: open a file

Scope: `code-gpui <folder> <file>`, or a click on a file in the Explorer, opens the file in the editor part the way the reference shows it: the connected tab strip (preview tab on single click, pinned on double click or from the command line) with the split and more actions, breadcrumbs, the text with line numbers, current-line border, indent guides, the textual word highlight under the cursor and the three-lane overview ruler; the status bar gains Ln/Col, indentation, encoding, EOL and language mode. The extension host sees the document, the editor, the tab model and `onLanguage` activation (`host_sync.rs`).

Done when: `port/tools/pixel-check` diffs the empty, folder and file windows (`notes.txt` open) in all three themes, with every non-anti-aliased difference listed in `port/results/pixels.md`; `cargo test -p app` passes with the model, language, host sync and editor tests.

- The model (`editor.rs`) follows upstream: indentation from `indentationGuesser.ts`, guides from `guidesTextModelPart.ts`, word highlight from `textualHighlightProvider.ts` with `USUAL_WORD_SEPARATORS`, tab order from `EditorGroupModel` (new editors right of the active one, a preview replaces the preview, closing activates the most recent).
- Languages (`languages.rs`) come from the scanned `contributes.languages`, matched like `getAssociationByPath`: filename, then the longest filename pattern, then the longest extension.
- Tab, editor and model ids copy `mainThreadEditorTabs.ts` `_generateTabId`, `getId()` and `$model<n>`.
- The reference runs with `editor.minimap.enabled: false`. With a file it parks the pointer on the status bar for 1.5 s so the editor scrollbars fade; the empty and folder windows skip that, since the wait also brings up the side bar sash's grip dots.
- Host check: `code-gpui <fixture> <fixture>/src/main.ts` with `EXTHOST_TRACE=1` activates `vscode.typescript-language-features` and `vscode.emmet` on top of the folder-only set (git-base, npm, github, git, debug-auto-launch, merge-conflict), with no `$onUnexpectedError` or activation error.
- shortcut: no minimap; port it with scrolling past one screen.
- shortcut: no editing and no keyboard; a click sets the cursor. Closed by milestone 5.
- shortcut: the cursor does not blink (the reference is captured with a solid cursor).
- shortcut: `visibleRanges` sent to the host covers the whole document.
- shortcut: no `firstLine` (shebang) language detection; a file with no association is plain text.
- shortcut: High Contrast word highlight border is dashed; GPUI has no dotted border.
- shortcut: the breadcrumb icon slot is empty until the file icon theme is ported.
- shortcut: files are read as UTF-8 (with or without BOM), no encoding guessing or binary detection; a read error is logged, not shown as the editor's error placeholder.

## Milestone 5: edit and save

Scope: one cursor with a selection in the active editor; the `coreCommands.ts` moves (char, word, line edge, document edge, with Shift to select), click, Shift+click, double and triple click and drag selection; typing and IME composition through GPUI's `EntityInputHandler`; Enter, Tab, Shift+Tab, the delete commands (char, word, to line start/end); Cmd+C/X/V with whole-line copy; undo/redo with the stops of `editStack.ts` and `shouldPushStackElementBetween`; Cmd+S writes the file; dirty state on the tab. The host gets `$acceptModelChanged` (with upstream `detailedReason`), `$acceptDirtyStateChanged`, `$acceptModelSaved`, selection changes and one `$acceptTabOperation` per tab change.

Done when: `cargo test -p app` passes with `editor::cursor::tests`, `editor::edit::tests`, the host sync tests and `keys_and_typed_text_edit_the_focused_editor_and_reach_the_host` (keys typed into a window, Cmd+S writes the file, the editor is clean after); `VSCODE_GPUI_EXTENSIONS=~/Developer/vscode-ref/extensions cargo test -p app -- --ignored live_host` gets a new `MainThreadDiagnostics.$changeMany` from TypeScript after an edit; pixel-check of the file window unchanged.

- The pure cursor model (`editor/cursor.rs`) has no GPUI dependency; the edit model and undo history (`editor/edit.rs`) live on `OpenEditor`. Dirty state is upstream's alternative version id: undo back to the saved text is clean.
- Trap: a full `$acceptEditorTabModel` resync fires only `onDidChangeTabGroups`. TypeScript's `TabResourceTracker` listens to `onDidChangeTabs` and skips `geterr` for files it does not see in a tab, so diagnostics never came. `host_sync.rs` now diffs the tab list and sends TAB_OPEN/CLOSE/MOVE/UPDATE like `mainThreadEditorTabs.ts`.
- Editing a preview editor pins it; word highlight stops while typing, as `_onPositionChanged` does.
- shortcut: columns step over chars, not grapheme clusters; a ZWJ emoji takes several steps. Port `getLeftDeleteOffset` and the grapheme iterator when needed.
- shortcut: Enter copies the leading whitespace only; no `onEnter` rules or indentation rules, so `{` does not indent the next line. Tab on a blank line takes the nearest non-blank indent above.
- shortcut: save has no participants (format on save, trim whitespace, final newline), no conflict check and no encoding other than UTF-8.
- shortcut: drag selection only tracks the pointer over lines; IME candidate windows get no bounds (`bounds_for_range` returns None).
- shortcut: one cursor; no multi-cursor, column selection or find.
- shortcut: the cursor still does not blink.

## Milestone 6: API test harness

Scope: `code-gpui <folder> --extensionDevelopmentPath=<dir> --extensionTestsPath=<path>` runs an extension's tests as upstream's extension development host does, and `port/tools/api-tests` runs `extensions/vscode-api-tests` suites api-folder and api-workspace with it.

Done when: `port/tools/api-tests` prints passing/pending/failing per suite and writes them to `port/results/api-tests.md`; the run finishes (failures allowed) without a hang.

- `exthost/src/extension_development.rs`: the extension under development is scanned with `isBuiltin: false`, `isUnderDevelopment: true` and replaces a built-in with the same id (the reference's `extensions/` holds `vscode-api-tests` too); init data gets `extensionDevelopmentLocationURI` and `extensionTestsLocationURI`. Only the `--flag=value` form is read, as `scripts/test-integration.sh` passes it.
- After eager activation the session sends `$extensionTestsExecute`; its reply (0 or 1), 1 if the runner threw, or the host's own code if it died first, becomes code-gpui's exit code, like `_onExtensionHostExit`. A session error before that also exits 1, so a test run never leaves the window open.
- The port log is code-gpui's stderr. Host console entries are formatted like Node's `util.format` (`console.rs`), so mocha's spec reporter reads as in the reference log; `$onUnexpectedError`, activation and runtime errors and `MainThreadLogger.$log` lines go there too.
- The test extension, its compiled tests, `testrunner.js` and mocha come from the reference checkout (same 1.147.0 sources; the port tree has no compiled `out/`), copied to `/tmp` so the tests' file writes leave the reference clean. `rootPath` assertions compare against `__dirname`, so the workspace must be the copy's `testWorkspace`.
- api-tests opens a window, so it takes the Studio GUI lock after the build. `MOCHA_GREP=<pattern>` (read by `testrunner.js`) runs one area, e.g. `MOCHA_GREP='vscode API - workspace'`; later milestones should use it, since the full folder suite does not get past the terminal tests within the timeout.
- Mocha keeps upstream's 60 s per-test timeout. Tests that wait for a main-thread event the stubs never send (quick input, terminal, shell integration) each cost 60 to 120 s, so the folder suite hits its 1800 s `SUITE_TIMEOUT` after 30 of its suites; the table counts the reached tests and lists the rest as not reached.
- One early run ended at 990 s with exit 0 and no mocha summary; the next full run went past the same point and hit the timeout, so the cause is not found. The script marks any run without mocha's summary as `incomplete: exit <code>` (or `timed out`), so such a run cannot pass as complete.
- Finding: the folder suite holds chat, lm, browser-tools and run_in_terminal tests for AI features the port removes by decision; they can never pass, so the parity target of milestone 20 (356/46/0) needs those tests subtracted. Decision for Artur.
- Rows `base/test`, `editor/test`, `platform/test` (upstream unit tests of the TypeScript layers) and `workbench/services/driver` (the smoke-test DOM driver) stay `todo`: the port has no TypeScript workbench to run them on, and the api-tests harness is its end-to-end check. Proposal: the `n/a` status of the roadmap's decision 1.
- A `.code-workspace` argument opens a multi-root workspace since milestone 12; the api-workspace suite passes 4/4.

## Milestone 7: syntax colors and themes

Scope: TextMate tokenization of every built-in grammar, token colors from the theme's `tokenColors`, bracket pair colorization and bracket matching in the editor, all 19 built-in color themes including High Contrast Light, `firstLine` language detection.

Done when: the colorize-tests fixtures give the reference's scopes and colors; pixel-check gains a `src/main.ts` window in dark, light, hc and hclight; `cargo test -p app` runs the grammar tests.

- Closes the milestone 2 shortcut (High Contrast Light not ported) and the milestone 4 shortcut (no `firstLine` detection).
- `native/crates/textmate` ports vscode-textmate 9.3.2 (the version in the reference's node_modules): grammar and plist reading, rules, the tokenizer loop with `\G` anchors, begin/while, injections, embedded languages, the theme trie and scope matchers. Regexes run on Oniguruma through the `onig` crate, the engine vscode-oniguruma wraps.
- `syntax.rs` resolves `tokenColors` like `ColorThemeData`: the editor default rule, the theme rules, then the `token.*-token` defaults. `syntax::tests::colorize_fixtures_match_the_reference` checks the text, scopes and color of every token of the 109 fixtures in the 10 themes the reference results list.
- Brackets (`brackets.rs`) follow `bracketPairsTree`: brackets from the language configuration outside strings, comments and regexes, nesting level colors `editorBracketHighlight.foreground1..6`, unexpected brackets in `editorBracketHighlight.unexpectedBracket.foreground`. The cursor's bracket match follows `matchBracket` then `findEnclosingBrackets`; only a touched bracket shows in the overview ruler.
- Languages fall back to `firstLine` patterns (checked on the first 1000 characters, `FIRST_LINE_DETECTION_LENGTH_LIMIT`) when no filename or extension association matches.
- `ref-screenshot` starts the reference with `--force-color-profile=srgb`. Without it Chromium converts to the main display's color profile, and with a screen sharing display attached to the Studio every non-gray pixel shifts (`#0F4A85` read back as `#224981`); the port renders sRGB.
- The window origin is rounded to whole points: on a display with an odd pixel height the centered origin falls on a half point and the drawable grows to 1538 device pixels.
- shortcut: no `semanticTokenColors`; they color semantic tokens, which come with the language features milestone.
- shortcut: no tree-sitter; upstream uses it for TypeScript only behind `editor.experimental.preferTreeSitter.typescript`, off by default.
- shortcut: the whole document is tokenized on the first draw; upstream tokenizes in the background. Port the background tokenizer when large files stall the first frame.
- shortcut: an edit drops the tokens from its first line down and re-pairs every bracket (`Tokenization::invalidate_from`); upstream updates both incrementally.
- shortcut: `tokenColors` given as a path to a .tmTheme file is not read; no built-in theme uses one.
- shortcut: the theme type reaches the extension host after the eager extensions activate; upstream sends it before `$startExtensionHost`.
- shortcut: no Go to Bracket, Select to Bracket or Force Retokenize commands until keybindings and the command palette land.

## Milestone 10: settings, storage, secrets

Scope shipped: `settings.json` (user, workspace, language overrides) feeding the workbench and `vscode.workspace.getConfiguration`, global and workspace state in `state.vscdb`, extension secrets in the macOS Keychain, the real environment in init data. Not shipped: the Settings editor, JSON settings validation and profiles (see the last items).

Done when: api-tests `configuration.test` and `state.test` pass as in the reference (5/5); a setting changed in `settings.json` reaches an extension's `onDidChangeConfiguration` (`port/tools/settings-check configuration`, step "external edit"); storage survives a restart (`port/tools/settings-check state`).

- `native/crates/config` ports the JSONC scanner and parser (`json.ts`, byte offsets), `jsonEdit.ts` (writes keep comments and layout), `ConfigurationModel` and `Configuration` with language overrides, the configuration registry with scopes and `[lang]` defaults, and the write path of `configurationEditing.ts` with its error messages. Core settings come from `core-settings.json`, dumped once from the reference's registry by `port/tools/dump-settings` (1506 settings, 1.7 MB embedded); extension settings come from each `contributes.configuration`. `[plaintext]`, `[go]`, `[makefile]`, `[shellscript]` and `[yaml]` defaults are added by hand from `modesRegistry.ts`.
- `MainThreadConfiguration` (`exthost/src/main_thread_configuration.rs`) serves updates and removals and sends `$acceptConfigurationChanged` before the reply, as upstream's change event fires before the update resolves. `settings.json` files are polled every 100 ms by mtime and size (upstream batches watcher events by 100 ms) instead of an FSEvents watcher, which comes with milestone 12.
- The workbench reads `workbench.colorTheme` and the editor indentation (`editor.tabSize`, `editor.insertSpaces`, `editor.detectIndentation`, per language) from the same model. The default theme is now the reference's default, Dark 2026; pixel-check passes `--theme`, so its windows are unchanged.
- No policy, remote or in-memory configuration layers: the port has no policy source, remote or memory writes. Init data sends them empty, in upstream's shape.
- `native/crates/storage` ports `SQLiteStorageDatabase` on rusqlite with upstream's `ItemTable`, the integrity check, the `.backup` file and recovery from it, and the in-memory cache that rebuilds a database corrupted at runtime. Writes are synchronous instead of flushed every 100 ms. Extension tests run on in-memory storage, as `storageMainService.ts` does for them.
- Workspace storage is `User/workspaceStorage/<id>` with upstream's macOS id (MD5 of the path and the folder's birth time). `workspace.json` is not written; the port has no stale workspace storage cleanup.
- Telemetry ids: `machineId` and `firstSessionDate` are kept in profile storage, `sessionId` is new per run. `machineId` is a uuid, upstream's fallback when it cannot hash a MAC address; upstream keeps it in `storage.json` (the `platform/state` service, not ported). No telemetry is sent, so `env.isTelemetryEnabled` is false.
- `MainThreadSecretState` keeps each secret as a generic password item (service `Code - OSS Secret Storage`, account the upstream `{"extensionId","key"}` key). Upstream encrypts with Electron safeStorage, whose key sits in the Keychain, and stores the result in application storage; letting the Keychain encrypt the item gives the same protection without a key of our own. Extension tests keep secrets in memory.
- `--extensionDevelopmentPath` and `--extensionTestsPath` are made absolute without resolving symlinks, so `context.extensionPath` matches the reference.
- `port/tools/settings-check` runs four check extensions against the port and the reference and diffs their output. Known differences: the reference fires two empty change events before `editor.wordWrap`; `env.appName` is "Code - OSS Dev" in the reference's dev run; `machineId` format; `isTelemetryEnabled`.
- Upstream tests ported: `json.test.ts`, `jsonEdit.test.ts`, `configurationModels.test.ts`, `configurationRegistry.test.ts`, the write cases of `configurationEditing.test.ts`, `storage.integrationTest.ts`. Not ported: `jsonFormatter`, `jsonParse`, `configurations`, `configurationVariables` (no variable resolver yet), `policyConfiguration` (no policies), `configurationService` and the storage service tests (their service shapes differ from the port's).
- `chat.*` and sign-in settings stay in the registry as data only; no AI or sign-in features by decision.
- Not shipped: the Settings editor. It needs the command palette and keybindings (milestone 9), a list and a text input widget (milestone 9) and the editor (milestone 8), which are in progress in parallel.
- Not shipped: JSON settings validation. Diagnostics come from json-language-features through `MainThreadDiagnostics` and are drawn as squiggles, both milestone 13; the schema needs `vscode://schemas/settings/*` content (`MainThreadDocumentContentProviders`, milestone 12).
- Not shipped: profiles. Switching, creating and exporting go through quick picks and the Settings editor (milestone 9 and above); storage and settings already take their paths from one place, so a profile is a different `User/profiles/<id>` folder once that UI exists.

## Milestone 9: commands, keybindings, quick input

Scope as in port/roadmap.md. Done in this milestone: the command registry, context keys and `when` clauses, keybinding resolution, the quick input widget and MainThreadQuickOpen, and the Command Palette. Not done: Quick Open (files, symbols), the native menu bar, context menus and hovers (see the last items).

- `contextkey.rs` ports the scanner, parser, evaluation and the `And`/`Or` normal forms of `contextkey.ts`; `contextkey/tests.rs` holds the scanner and parser cases of `contextkey.test.ts` and `scanner.test.ts`.
- `commands.rs` is the one registry of the workbench and `vscode.commands`: workbench handlers, extension commands registered through MainThreadCommands (`main_thread_commands.rs`), `onCommand:` activation before a run. `Commands::register_all` takes a whole table in one call for milestone 8's `editor/commands.rs`.
- The extension host forwards requests of chosen proxies to the app (`Session::forward`); replies are read inside `Session::pump`, because an extension whose `activate` awaits a forwarded request deadlocked `session.start` when `main.rs` read them.
- `keybindings.rs` ports `KeybindingResolver` (chords, removals with `-command`, `when` on removal) with the cases of `keybindingResolver.test.ts`. The macOS defaults (`keybindings/default-macos.json`, 1220 rows) are the reference's own export (Developer: Keyboard Shortcuts Export without extension rows); extension rows come from `contributes.keybindings`; `keybindings.json` overrides both. Keys resolve in `cx.intercept_keystrokes`, before the focused element, as upstream's window-level listener does. The milestone 5 editor keys run through the registry under upstream ids (`workbench/editor_commands.rs`).
- Keybinding aliases (`keybindings.json` `args` of the AI sessions window) are not read: no AI by decision.
- `quick_input.rs` is the quick pick and input box model (filtering with `filters.rs`, a port of `filters.ts` with `korean.ts`; selection, buttons, validation with its 100 ms debounce); `quick_input_view.rs` draws it with the Modern UI frosted glass. Extension pickers and input boxes run through `main_thread_quick_open.rs`.
- The Command Palette follows `commandsQuickAccess.ts`: TF-IDF similar commands (`tf_idf.rs`, threshold 0.5, at most 5), "recently used" with a 50-entry LRU, Developer category last, duplicate labels with their id, extension commands from `contributes.commands` with `enablement` as precondition and `menus.commandPalette` `when`.
- The header drag follows `QuickInputDragAndDropController`: grab cursor on the title and the header padding, snap to the top line, the 25% line and the center within 20px, clear of the traffic lights, double click resets.
- Mouse cursors follow the reference CSS: pointer on activity bar items, title bar actions, status bar items with a command, quick input rows, buttons and checkboxes; text on the quick input box; grab on the quick input header.
- Disabled quick input arrows keep the default cursor; sashes (milestone 11), the Explorer (milestone 12) and editor status items (milestone 8) get theirs with those milestones.
- `compare_file_names` sorts by code point where upstream uses `Intl.Collator` with numeric order; `locale_compare` mimics a few ICU root rules for English labels.
- api-tests `api-command: vscode.open`, `vscode.open with untitled supports associated resource`, `api-command: vscode.diff` and `editorCommand with extra args` fail: they need milestone 8 editor groups, tab groups and text editor arguments, and a diff editor (milestone 14).
- The palette history (`commandPalette.mru.cache` and `.counter`, limited by `workbench.commandPalette.history`) and the quick input position (`workbench.quickInput.viewState`) live in the global `state.vscdb`, the same `Storage` the extension host's MainThreadStorage serves (`restore_profile_state`). Upstream writes the history on shutdown only; the port writes it on each change, since its storage is synchronous and the app has no shutdown phase.
- The Color Theme picker (`workbench/themes.rs`) follows `InstalledThemesPicker`: light, dark and high contrast groups with the default theme pinned, a preview 200ms after the focus rests, Escape or any hide without accept restores the theme it opened with. Accepting writes `workbench.colorTheme` through `Settings::update_value_derived` (`updateValue` without a target: the layer that already sets the key, else user settings, where the default value removes the key) and sends `$acceptConfigurationChanged` and `$onColorThemeChange` to the extension host. The Manage Extension and color mode buttons run `workbench.extensions.search` and `workbench.action.openSettings`, which fail with an error dialog until the Extensions view and the Settings editor are ported. Without a Marketplace (product.json has no gallery) upstream shows no install entry either.
- Quick input separators use `descriptionForeground`, not `pickerGroup.foreground`: the CSS rule of quickInput.css outranks the controller's style rule (measured on 1.147).
- shortcut: the palette's gear button opens `keybindings.json`; upstream opens the Keybindings editor at the command, which the port does not have.
- shortcut: `keybindings.json` is polled once a second; the port has no file watcher.
- shortcut: a command that fails from a keybinding is logged to stderr; the port has no notification toasts.
- shortcut: Revert File is a whole-text edit plus a save until milestone 8 has a model revert.
- shortcut: the quick input fades in and out without the 0.97/0.99 scale of upstream's animation; GPUI has no element transforms.
- `fuzzy_scorer.rs` ports `fuzzyScorer.ts` (scoreFuzzy, scoreFuzzy2, scoreItemFuzzy with its cache, compareItemsByFuzzyScore, prepareQuery) on top of `filters::fuzzy_score`, the `fuzzyScore` DP of `filters.ts`. Tests: `fuzzy_scorer/tests.rs` (68 cases of `fuzzyScorer.test.ts`) and `filters/fuzzy_score_tests.rs` (28 `fuzzyScore` cases of `filters.test.ts`). Left out: the Windows path branches (macOS only), lone UTF-16 surrogates (not representable in a Rust `str`), and the `fuzzyScoreGraceful` and `anyScore` assertions, which have no caller in the port yet. Match offsets count chars where upstream counts UTF-16 units; the same for every text outside the BMP the port draws.
- Not done, left for a next session: Quick Open file picks (`anythingQuickAccess.ts`: file search with the exclude settings, editor history and `:line`), the native menu bar from `MenuId.MenubarMainMenu`, context menus and hovers. pixel-check has no quick input window; the palette was compared by an ad-hoc capture (port/results/pixels.md).

## Milestone 12: files, explorer, search, workspace (in progress)

Scope shipped so far: `workspace.fs` and file events, Seti file icons and numeric name sorting in the Explorer, `workspace.findFiles`, `findFiles2` and `findTextInFiles` through ripgrep, multi-root `.code-workspace` windows. Done when (roadmap section 12) is not met yet, see the last items.

- `native/crates/files` is the file service: a `FileSystem` trait, the `Disk` provider (capabilities as `diskFileSystemProviderClient`, no PathCaseSensitive on macOS), delete to Trash through the `trash` crate (NSFileManager, as Electron `shell.trashItem`), and upstream's error names with `_handleError` renames. Other schemes go to the extension providers registered with `$registerFileSystemProvider`.
- `files::watcher` watches through FSEvents (notify), batches events for 75 ms and coalesces them as `watcher.ts` `EventCoalescer`. `files.watcherExclude` patterns are matched by `files::glob`, a port of `glob.ts` with `!` exceptions and its upstream tests, case insensitive as `parseWatcherPatterns`.
- `MainThreadFileSystem` and `MainThreadFileSystemEventService` live in `exthost/src/main_thread_files.rs`; correlated extension watchers each get their own watcher so their events carry the session.
- `native/crates/search` ports `queryBuilder.ts`, `fileSearch.ts` and `ripgrepFileSearch.ts`, and `ripgrepTextSearchEngine.ts` with `textSearchManager.ts`, against the `@vscode/ripgrep-universal` binary in `node_modules`. Excludes with `!` exceptions run ripgrep twice and dedupe, as upstream. `MainThreadWorkspace` (`exthost/src/main_thread_workspace.rs`) runs each search on its own thread, sends `$handleTextSearchResult` before the reply, and kills ripgrep on the host's `Cancel`.
- The icon theme follows `workbench.iconTheme` as `initializeFileIconTheme`: null draws no icons, an unknown id falls back to `vs-seti`. Names sort as `compareFileNamesDefault` (numeric, `file9` before `file10`).
- Explorer rows, section headers and actions use the pointer cursor (list.css, paneview.css, actionbar.css); twisties inherit it from the row, as tree.css sets none.
- shortcut: an extension provider call fails after 300 s; upstream waits forever (`main_thread_files.rs`).
- shortcut: no second ripgrep run without `--follow` when symbolic links loop (`RipgrepSymlinkLoopDetector`), in file and text search.
- shortcut: workspace search skips folders of schemes other than `file`; the port registers no extension search providers (`MainThreadSearch`).
- shortcut: Seti keys with a parent folder (`dir/name`) never match; Seti has none.
- shortcut: an unreadable folder shows no children; the reference shows the error in the tree.
- Text search does not include results from unsaved editors, as the search service's local results would; the port's documents are milestone 8's.
- `config::workspace` (`native/crates/config/src/workspace.rs`) is the workspace of a window: one folder, or the folders of a `.code-workspace` file resolved as `toWorkspaceFolders` (relative paths against the file's folder, `file:` URIs, duplicates dropped), `getFolder` ignoring case as on macOS, and `getWorkspaceLabel` (`<name> (Workspace)`). Its cases come from `workspace.test.ts`.
- Settings of a workspace file: its `settings` key is the workspace layer with WORKSPACE_SCOPES, `launch` and `tasks` its standalone sections; each folder's `.vscode/settings.json` is read with FOLDER_SCOPES, so window settings in a folder of a workspace are ignored as upstream. Workspace-target writes go into the file under `settings` (or `tasks`, `launch`), folder-target writes into the folder the resource is in.
- The workspace id of a workspace file is MD5 of its lowercased path (`getWorkspaceIdentifier`, the stable-id case of `workspaces.test.ts`); it names `User/workspaceStorage/<id>`. Init data and `$initializeWorkspace` carry `configuration` (the file), the label, `transient` and every folder with its name and index. The watcher watches each folder with its own `files.watcherExclude`; search covers every folder.
- The Explorer of a workspace shows its folders as root rows, the first five expanded (`doSetTreeInput` without view state); revealing a file expands its root. Quick pick descriptions of several folders read `<folder> \u2022 <path>` (`label.test.ts` multi-root case) and breadcrumbs start with the folder name, as `BreadcrumbsModel` for WorkbenchState.WORKSPACE.
- shortcut: a workspace folder with a `uri` of another scheme than `file` is skipped; it needs the extension file system providers in the Explorer and search.
- Not done: adding or removing workspace folders (`$updateWorkspaceFolders`, `onDidChangeWorkspaceFolders`), untitled workspaces, and the Explorer view state kept in workspace storage.
- Not done: bulk edits (`MainThreadBulkEdits`) and file operation participants. `workspace.event.test` (`onWillCreate`, `onWillDelete`, `onWillRename`) fails 0/10 and the `applyEdit` tests of `workspace.test` fail with them.
- Not done: the rest of `workspace.test`. Most failures are `openTextDocument` and document events, which need `MainThreadDocuments` (milestone 8).
- Not done: pixel-check of the folder window with Seti icons. The reference capture timed out twice on the Studio (port/results/pixels.md).
- Not done: Explorer file operations (new, rename, delete, drag and drop, copy and paste), workspace trust, recent workspaces, the search view and search editor, local history, opener and URL handling.

## Benchmarks

- `port/tools/bench`: Wu's `script/memory-benchmark` method generalized to N apps (`APPS="name=cmd;..."`), plus an extension host line per app.
- Wu's `.cargo/config.toml` links with Homebrew lld on aarch64-apple-darwin; the Studio has no lld, so Wu is built with `RUSTFLAGS="-C symbol-mangling-version=v0 --cfg tokio_unstable"` (same flags, Apple ld). Env `CARGO_TARGET_*_RUSTFLAGS` does not work for this: cargo appends it to the config array.
