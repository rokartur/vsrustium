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
- shortcut: no editing and no keyboard; a click sets the cursor. Add input with the edit operations and `$acceptModelChanged`.
- shortcut: the cursor does not blink (the reference is captured with a solid cursor).
- shortcut: `visibleRanges` sent to the host covers the whole document.
- shortcut: no `firstLine` (shebang) language detection; a file with no association is plain text.
- shortcut: High Contrast word highlight border is dashed; GPUI has no dotted border.
- shortcut: the breadcrumb icon slot is empty until the file icon theme is ported.
- shortcut: files are read as UTF-8 (with or without BOM), no encoding guessing or binary detection; a read error is logged, not shown as the editor's error placeholder.

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
- shortcut: `tokenColors` given as a path to a .tmTheme file is not read; no built-in theme uses one.
- shortcut: the theme type reaches the extension host after the eager extensions activate; upstream sends it before `$startExtensionHost`.
- shortcut: no Go to Bracket, Select to Bracket or Force Retokenize commands until keybindings and the command palette land.

## Benchmarks

- `port/tools/bench`: Wu's `script/memory-benchmark` method generalized to N apps (`APPS="name=cmd;..."`), plus an extension host line per app.
- Wu's `.cargo/config.toml` links with Homebrew lld on aarch64-apple-darwin; the Studio has no lld, so Wu is built with `RUSTFLAGS="-C symbol-mangling-version=v0 --cfg tokio_unstable"` (same flags, Apple ld). Env `CARGO_TARGET_*_RUSTFLAGS` does not work for this: cargo appends it to the config array.
