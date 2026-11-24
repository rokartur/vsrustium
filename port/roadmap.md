# Port roadmap

Milestones from today to a shippable port, each with its scope, its done-when check and the ledger rows it moves. `port/ledger.md` stays the source of truth for status and evidence; this file only says in which milestone a row moves. Every `todo`, `stub` and `ported` row of the ledger is named exactly once below (checked by the command at the end).

Rule for every milestone (Artur): a feature ships end to end and 1:1 with VS Code 1.147.0: UI, keyboard and mouse, the main-thread side the extension host calls, persistence, same look and behavior, upstream logic and tests ported, checked against the reference. Fewer complete features beat more partial ones; a `shortcut:` is only for a missing platform capability or a product decision.

Mouse cursors are part of the look (Artur): every element gets the cursor the reference CSS gives it (`rg 'cursor:' src/vs --glob '*.css'`): `pointer` on clickable items (activity bar, tabs, list rows, status bar items with a command, buttons, links), `text` over editor text and input fields, the resize cursors on sashes, `default` elsewhere. GPUI: `.cursor_pointer()`, `.cursor_text()`, `.cursor_ew_resize()` and the like on the element, never a window-wide cursor.

Ledger today: 64 verified, 54 ported, 61 stub, 307 todo, 92 removed, 14 n/a (of 592 rows).

## Done

| milestone | scope | evidence |
|---|---|---|
| 1 extension host seam | Rust parent, upstream framing and RPC, Bun host, 30/30 built-ins activate | `exthost-probe`, `port/results/benchmark.md` |
| 2 empty workbench | title bar, activity bar, letterpress, status bar in 3 themes | `port/results/pixels.md` milestone 2 |
| 3 open a folder | Explorer tree, folder in init data and `$initializeWorkspace` | `port/results/pixels.md` milestone 3 |
| 4 open a file | tabs, breadcrumbs, gutter, guides, word highlight, overview ruler, host sees documents | `port/results/pixels.md` milestone 4 |
| 5 edit and save | one cursor with selection, core moves and deletes, typing and IME, undo/redo, clipboard, save, dirty tabs, host sync with tab operations | `port/notes.md` milestone 5, `cargo test -p app` and its ignored `live_host` test |

## 5. Edit and save (done)

Scope: one cursor with selection, the `coreCommands.ts` cursor and delete commands, typing with IME through GPUI's input handler, undo/redo with `editStack.ts` stops, clipboard, save and dirty state; the host hears `$acceptModelChanged`, `$acceptDirtyStateChanged`, `$acceptModelSaved` and selection changes. Written in `editor/cursor.rs`, `editor/edit.rs`, `workbench/text_input.rs`, `host_sync.rs`.

Done when: `cargo test -p app` passes with the cursor, edit and host sync tests (including `live_host_takes_edits_and_typescript_sees_the_new_text`); typing in `notes.txt` then Cmd+S writes the file and the tab loses its dirty dot; pixel-check of the three earlier windows unchanged.

Notes shortcuts closed here: no editing and no keyboard. The cursor still does not blink. Rows this milestone left `todo` or `stub` moved to milestone 8.

Rows: `editor/contrib/clipboard` `editor/contrib/wordOperations`

## 6. API test harness (done)

Scope: run `extensions/vscode-api-tests` against the port, so every later milestone reports a pass count against the reference baseline (`port/results/api-tests-reference.md`: folder 356/46/0, workspace 4/0/0). Needs `--extensionDevelopmentPath` / `--extensionTestsPath` in `code-gpui`, the test runner's exit code reaching the parent, host errors and console forwarded to the port's log, and a `port/tools/api-tests` script on the Studio.

Done when: `port/tools/api-tests` prints passing/pending/failing per suite for the port and writes them to `port/results/api-tests.md`; the run finishes (failures allowed) without a hang.

Rows: `extension/vscode-api-tests` `mainthread/MainThreadExtensionService` `mainthread/MainThreadErrors` `mainthread/MainThreadConsole` `mainthread/MainThreadLogger` `platform/log` `workbench/services/log` `workbench/services/driver` `base/test` `editor/test` `platform/test`

## 7. Syntax colors and themes (done)

Scope: TextMate grammars from `contributes.grammars` (vscode-textmate's algorithm, Oniguruma regexes), token colors from the theme's `tokenColors` and `semanticTokenColors`, bracket pair colorization and matching, every built-in color theme including High Contrast Light, `firstLine` language detection.

Done when: `vscode-colorize-tests` give the same token scopes and colors as the reference for every fixture; pixel-check gains a `src/main.ts` window in 4 themes (+ hc-light); `cargo test -p app` grammar tests.

Notes shortcuts closed here: High Contrast Light not ported; no `firstLine` detection.

Rows: `workbench/services/textMate` `workbench/services/treeSitter` `workbench/services/themes` `workbench/services/language` `mainthread/MainThreadTheming` `platform/theme` `editor/contrib/tokenization` `editor/contrib/bracketMatching` `extension/vscode-colorize-tests` `extension/vscode-colorize-perf-tests` `extension/theme-defaults` `extension/theme-abyss` `extension/theme-kimbie-dark` `extension/theme-monokai` `extension/theme-monokai-dimmed` `extension/theme-quietlight` `extension/theme-red` `extension/theme-solarized-dark` `extension/theme-solarized-light` `extension/theme-tomorrow-night-blue` `extension/bat` `extension/clojure` `extension/coffeescript` `extension/cpp` `extension/csharp` `extension/css` `extension/dart` `extension/diff` `extension/docker` `extension/dotenv` `extension/fsharp` `extension/go` `extension/groovy` `extension/handlebars` `extension/hlsl` `extension/html` `extension/ini` `extension/java` `extension/javascript` `extension/json` `extension/julia` `extension/latex` `extension/less` `extension/log` `extension/lua` `extension/make` `extension/markdown-basics` `extension/objective-c` `extension/perl` `extension/php` `extension/powershell` `extension/pug` `extension/python` `extension/r` `extension/razor` `extension/restructuredtext` `extension/ruby` `extension/rust` `extension/scss` `extension/shaderlab` `extension/shellscript` `extension/sql` `extension/swift` `extension/typescript-basics` `extension/vb` `extension/xml` `extension/yaml`

## 8. Editor viewport and editing commands

Scope: scrolling (wheel, scrollbar, reveal), real `visibleRanges`, minimap, folding, sticky scroll, multi-cursor, find/replace widget, comment toggling, line operations, smart select, font zoom, the editor context menu, editor splits left for milestone 11.

Done when: api-tests `editor.test`, `window.test` (text editor parts) and `documents.test` pass as in the reference; pixel-check gains a scrolled long-file window with minimap and a folded region; `cargo test -p app` folding and multi-cursor tests.

Notes shortcuts closed here: no minimap; `visibleRanges` covers the whole document; High Contrast dotted borders drawn dashed (needs a dotted border in GPUI or a custom path).

Rows: `editor/browser` `workbench/parts/editor` `workbench/contrib/codeEditor` `editor/contrib/folding` `workbench/contrib/folding` `editor/contrib/stickyScroll` `editor/contrib/longLinesHelper` `editor/contrib/middleScroll` `editor/contrib/fontZoom` `editor/contrib/multicursor` `editor/contrib/find` `editor/contrib/comment` `editor/contrib/linesOperations` `editor/contrib/smartSelect` `editor/contrib/inPlaceReplace` `editor/contrib/anchorSelect` `editor/contrib/unicodeHighlighter` `editor/contrib/sectionHeaders` `editor/contrib/placeholderText` `editor/contrib/contextmenu` `editor/contrib/message` `editor/contrib/floatingMenu` `editor/contrib/dnd` `mainthread/MainThreadDocuments` `mainthread/MainThreadTextEditors` `mainthread/MainThreadClipboard` `mainthread/MainThreadEditorTabs` `editor/common` `editor/contrib/cursorUndo` `editor/contrib/wordPartOperations` `editor/contrib/caretOperations` `editor/contrib/lineSelection` `editor/contrib/indentation` `editor/contrib/insertFinalNewLine` `editor/contrib/toggleTabFocusMode` `editor/contrib/editorState` `editor/contrib/readOnlyMessage` `editor/contrib/unusualLineTerminators` `platform/undoRedo` `platform/clipboard` `platform/editor` `workbench/services/clipboard` `workbench/services/textfile` `workbench/services/workingCopy` `workbench/services/untitled` `workbench/services/textmodelResolver` `workbench/services/model` `workbench/services/textresourceProperties` `workbench/services/filesConfiguration`

## 9. Commands, keybindings, menus, quick input

Scope: command registry shared by the workbench and `vscode.commands`, context keys and `when` clauses, keybinding resolution from `contributes.keybindings` and `keybindings.json`, the native macOS menu bar, context menus, hovers, Command Palette and Quick Open (files, symbols, commands, theme picker), title bar and activity bar clicks.

Done when: api-tests `commands.test` and `quickInput.test` pass as in the reference; Cmd+Shift+P runs a command contributed by an extension; Cmd+P opens a fixture file; `cargo test -p app` `when`-clause and keybinding resolution tests against upstream cases.

Rows: `mainthread/MainThreadCommands` `mainthread/MainThreadQuickOpen` `platform/commands` `platform/action` `platform/actions` `platform/actionWidget` `platform/contextkey` `platform/contextview` `platform/keybinding` `platform/keyboardLayout` `platform/globalKeybindings` `platform/quickinput` `platform/menubar` `platform/hover` `platform/list` `platform/registry` `workbench/services/commands` `workbench/services/actions` `workbench/services/keybinding` `workbench/services/quickinput` `workbench/services/contextmenu` `workbench/services/menubar` `workbench/contrib/commands` `workbench/contrib/quickaccess` `workbench/contrib/keybindings` `workbench/contrib/keybindingsExport` `workbench/contrib/themes` `workbench/contrib/list` `editor/contrib/quickAccess` `workbench/parts/titlebar` `workbench/parts/activitybar`

## 10. Settings, storage, profiles

Scope: `settings.json` (user, workspace, folder, language overrides) feeding both the workbench and `vscode.workspace.getConfiguration`, configuration schema from `contributes.configuration`, the Settings editor and JSON settings with schema validation, global and workspace storage on disk, secret storage in the macOS Keychain, profiles, real `environment` in init data.

Done when: api-tests `configuration.test` and `state.test` pass as in the reference; a setting changed in `settings.json` reaches an extension's `onDidChangeConfiguration`; storage survives a restart.

Rows: `seam/init-data` `mainthread/MainThreadConfiguration` `mainthread/MainThreadStorage` `mainthread/MainThreadSecretState` `mainthread/MainThreadProfileContentHandlers` `platform/configuration` `platform/storage` `platform/state` `platform/environment` `platform/userData` `platform/userDataProfile` `platform/policy` `platform/jsonschemas` `platform/secrets` `platform/encryption` `workbench/services/configuration` `workbench/services/configurationResolver` `workbench/services/storage` `workbench/services/preferences` `workbench/services/environment` `workbench/services/userData` `workbench/services/userDataProfile` `workbench/services/policies` `workbench/services/secrets` `workbench/services/encryption` `workbench/contrib/preferences` `workbench/contrib/userDataProfile` `workbench/contrib/encryption` `workbench/contrib/policyExport` `extension/configuration-editing`

## 11. Workbench layout and chrome

Scope: the part grid with sashes and hidden parts, panel and secondary side bar, view containers and collapsible views, tree views from extensions, editor groups and splits, notifications, dialogs (native open/save), progress, banner, status bar entries from extensions, decorations, window lifecycle (close, quit, hot exit backup), multiple windows, focus and inactive look.

Done when: api-tests `window.test` (messages, progress, tree views, status bar) pass as in the reference; pixel-check gains a window with panel and split editor; hot exit restores an unsaved buffer after quit.

Notes shortcuts closed here: active-window look only; window radius fixed at macOS 26 value; tree rows without focus, hover, keyboard or scrolling.

Rows: `mainthread/MainThreadStatusBar` `mainthread/MainThreadMessageService` `mainthread/MainThreadProgress` `mainthread/MainThreadDiaglogs` `mainthread/MainThreadTreeViews` `mainthread/MainThreadDecorations` `mainthread/MainThreadWindow` `workbench/parts/partGrid` `workbench/parts/views` `workbench/parts/sidebar` `workbench/parts/statusbar` `workbench/parts/panel` `workbench/parts/auxiliarybar` `workbench/parts/notifications` `workbench/parts/dialogs` `workbench/parts/banner` `workbench/contrib/sash` `workbench/contrib/modernUI` `workbench/contrib/limitIndicator` `workbench/services/layout` `workbench/services/views` `workbench/services/panecomposite` `workbench/services/activity` `workbench/services/editor` `workbench/services/history` `workbench/services/notification` `workbench/services/dialogs` `workbench/services/progress` `workbench/services/banner` `workbench/services/statusbar` `workbench/services/decorations` `workbench/services/title` `workbench/services/host` `workbench/services/lifecycle` `workbench/services/auxiliaryWindow` `platform/layout` `platform/history` `platform/notification` `platform/dialogs` `platform/progress` `platform/window` `platform/windows` `platform/auxiliaryWindow` `platform/lifecycle` `platform/backup` `platform/native` `platform/launch` `platform/dnd`

## 12. Files, explorer, search, workspace

Scope: file system service with watcher (FSEvents) and extension file system providers, Explorer file operations (new, rename, delete to Trash, drag and drop, copy/paste), Seti file icons, numeric name collation, workspace trust, multi-root `.code-workspace`, recent workspaces, text search through ripgrep, search view and search editor, bulk edits, local history, opener and URL handling.

Done when: api-tests `workspace.test`, `workspace.fs.test`, `workspace.event.test`, `workspace.tasks` excluded, and the api-workspace suite pass as in the reference; pixel-check folder window with Seti icons (reference captured without `workbench.iconTheme: null`); `cargo test -p app` sorting test with `file9`/`file10`.

Notes shortcuts closed here: no file icon theme; no watcher; plain lowercase sort; unreadable folder shows nothing; breadcrumb icon slot empty; UTF-8 only reads, no binary detection.

Rows: `mainthread/MainThreadFileSystem` `mainthread/MainThreadFileSystemEventService` `mainthread/MainThreadWorkspace` `mainthread/MainThreadLabelService` `mainthread/MainThreadSearch` `mainthread/MainThreadDocumentContentProviders` `mainthread/MainThreadBulkEdits` `mainthread/MainThreadUrls` `mainthread/MainThreadUriOpeners` `platform/files` `platform/label` `platform/uriIdentity` `platform/path` `platform/workspace` `platform/workspaces` `platform/opener` `platform/url` `workbench/services/files` `workbench/services/label` `workbench/services/path` `workbench/services/workspaces` `workbench/services/search` `workbench/services/url` `workbench/contrib/files` `workbench/contrib/workspace` `workbench/contrib/workspaces` `workbench/contrib/search` `workbench/contrib/searchEditor` `workbench/contrib/bulkEdit` `workbench/contrib/localHistory` `workbench/contrib/opener` `workbench/contrib/externalUriOpener` `workbench/contrib/url` `extension/theme-seti` `extension/theme-modern-icons` `extension/search-result`

## 13. Language features

Scope: everything `MainThreadLanguageFeatures` registers, drawn by the native editor: completion, hover, signature help, code actions and lightbulb, code lens, inlay hints, go to definition and peek, references, rename, formatting, document symbols and Outline, semantic tokens, semantic word highlight, links, color picker, linked editing, snippets, call and type hierarchy, inline completions from extensions; diagnostics with squiggles and the Problems panel; comments threads; Emmet; the language status item.

Done when: api-tests `languages.test`, `languagefeatures.test`-family and `comments` cases pass as in the reference; with `src/main.ts` open TypeScript completion, hover, go to definition and a diagnostic squiggle work (pixel-check window with a squiggle and the suggest widget).

Rows: `mainthread/MainThreadLanguageFeatures` `mainthread/MainThreadLanguages` `mainthread/MainThreadDiagnostics` `mainthread/MainThreadComments` `editor/contrib/suggest` `editor/contrib/hover` `editor/contrib/parameterHints` `editor/contrib/codeAction` `editor/contrib/codelens` `editor/contrib/inlayHints` `editor/contrib/gotoSymbol` `editor/contrib/gotoError` `editor/contrib/peekView` `editor/contrib/zoneWidget` `editor/contrib/rename` `editor/contrib/format` `editor/contrib/documentSymbols` `editor/contrib/semanticTokens` `editor/contrib/wordHighlighter` `editor/contrib/links` `editor/contrib/colorPicker` `editor/contrib/linkedEditing` `editor/contrib/snippet` `editor/contrib/symbolIcons` `editor/contrib/inlineCompletions` `editor/contrib/inlineProgress` `editor/contrib/dropOrPasteInto` `editor/contrib/structuralDiff` `platform/markers` `platform/markdown` `workbench/services/suggest` `workbench/services/outline` `workbench/services/languageStatus` `workbench/services/languageDetection` `workbench/services/inlineCompletions` `workbench/contrib/markers` `workbench/contrib/outline` `workbench/contrib/codeActions` `workbench/contrib/format` `workbench/contrib/inlayHints` `workbench/contrib/inlineCompletions` `workbench/contrib/callHierarchy` `workbench/contrib/typeHierarchy` `workbench/contrib/comments` `workbench/contrib/snippets` `workbench/contrib/emmet` `workbench/contrib/languageStatus` `workbench/contrib/languageDetection` `workbench/contrib/dropOrPasteInto` `extension/typescript-language-features` `extension/css-language-features` `extension/html-language-features` `extension/json-language-features` `extension/php-language-features` `extension/emmet` `extension/references-view` `extension/extension-editing`

## 14. Source control and diff

Scope: SCM view and input box, quick diff gutter, the diff editor (side by side and inline), multi-file diff, merge editor and merge-conflict decorations, timeline, the git, git-base and github extensions fully working (no sign-in: github features that need a token stay off).

Done when: api-tests SCM cases pass as in the reference; with the fixture copied into a git repository the SCM view lists a changed file and opening it shows the diff (pixel-check window).

Rows: `mainthread/MainThreadSCM` `mainthread/MainThreadQuickDiff` `mainthread/MainThreadDocumentDiff` `mainthread/MainThreadGitExtension` `mainthread/MainThreadGitHub` `mainthread/MainThreadTimeline` `platform/git` `platform/github` `workbench/services/github` `workbench/contrib/scm` `workbench/contrib/git` `workbench/contrib/github` `workbench/contrib/timeline` `workbench/contrib/mergeEditor` `workbench/contrib/multiDiffEditor` `workbench/contrib/scrollLocking` `editor/contrib/diffEditorBreadcrumbs` `extension/git` `extension/git-base` `extension/github` `extension/merge-conflict`

## 15. Terminal, tasks, output

Scope: integrated terminal (PTY from Rust, VT parser and renderer in GPUI, or Wu's terminal if it has one), shell integration, extension terminals and pseudoterminals, tasks and problem matchers, npm/grunt/gulp/jake task providers, external terminal, Output panel and log viewer.

Done when: api-tests `terminal.test`, `terminal.shellIntegration.test` and `workspace.tasks.test` pass as in the reference (run with `SHELL=/bin/bash` as the baseline was); `npm: build` task from the fixture runs in a terminal.

Rows: `mainthread/MainThreadTerminalService` `mainthread/MainThreadTerminalShellIntegration` `mainthread/MainThreadTask` `mainthread/MainThreadOutputService` `platform/terminal` `platform/shell` `platform/process` `platform/externalTerminal` `workbench/services/terminal` `workbench/services/output` `workbench/services/process` `workbench/contrib/terminal` `workbench/contrib/terminalContrib` `workbench/contrib/tasks` `workbench/contrib/externalTerminal` `workbench/contrib/output` `workbench/contrib/logs` `extension/terminal-suggest` `extension/npm` `extension/grunt` `extension/gulp` `extension/jake`

## 16. Debug and testing

Scope: debug adapter protocol client, launch.json, breakpoints in the gutter, call stack, variables, watch, debug console, JS debugger (`ms-vscode.js-debug` is a marketplace built-in, check its license and Open VSX availability), Test Explorer and test results.

Done when: api-tests `debug.test` and testing cases pass as in the reference; a breakpoint in a Node script from the fixture stops and shows variables.

Rows: `mainthread/MainThreadDebugService` `mainthread/MainThreadTesting` `platform/debug` `workbench/contrib/debug` `workbench/contrib/testing` `extension/debug-auto-launch` `extension/debug-server-ready`

## 17. Webviews and notebooks

Scope: a WKWebView embedded in the GPUI window for webview panels, webview views and custom editors, with the upstream message and resource protocol; Markdown preview, image/media preview, simple browser, editor insets; notebooks (ipynb) with renderers and kernels.

Done when: api-tests `webview.test`, `customEditor` and `notebook*.test` cases pass as in the reference; Markdown preview of the fixture README draws next to its source.

Rows: `mainthread/MainThreadWebviews` `mainthread/MainThreadWebviewPanels` `mainthread/MainThreadWebviewViews` `mainthread/MainThreadCustomEditors` `mainthread/MainThreadEditorInsets` `mainthread/MainThreadNotebook` `mainthread/MainThreadNotebookEditors` `mainthread/MainThreadNotebookDocuments` `mainthread/MainThreadNotebookKernels` `mainthread/MainThreadNotebookRenderers` `mainthread/MainThreadInteractive` `mainthread/MainThreadBrowsers` `platform/webview` `platform/browserView` `workbench/services/browserView` `workbench/services/notebook` `workbench/contrib/webview` `workbench/contrib/webviewPanel` `workbench/contrib/webviewView` `workbench/contrib/customEditor` `workbench/contrib/notebook` `workbench/contrib/interactive` `workbench/contrib/replNotebook` `workbench/contrib/markdown` `workbench/contrib/mediaPreview` `workbench/contrib/browserView` `workbench/contrib/imageCarousel` `extension/markdown-language-features` `extension/markdown-math` `extension/mermaid-markdown-features` `extension/ipynb` `extension/notebook-renderers` `extension/simple-browser`

## 18. Extensions: install, update, gallery

Scope: Extensions view backed by Open VSX, install/uninstall/update/disable, VSIX install, signature and checksum checks, recommendations, ESM extensions added after startup on Bun, language packs and display language.

Done when: installing an extension from Open VSX activates it without restart (an ESM one included); api-tests `extensions.test` pass as in the reference.

Notes shortcuts closed here: ESM extensions added after startup not matched on Bun.

Rows: `seam/vscode-module-bun` `mainthread/MainThreadDownloadService` `mainthread/MainThreadLocalization` `gallery/open-vsx` `platform/extensionManagement` `platform/extensionRecommendations` `platform/extensionResourceLoader` `platform/extensions` `platform/download` `platform/request` `platform/checksum` `platform/sign` `platform/languagePacks` `workbench/services/extensionManagement` `workbench/services/extensionRecommendations` `workbench/services/extensions` `workbench/services/request` `workbench/services/checksum` `workbench/services/integrity` `workbench/services/localization` `workbench/contrib/extensions` `workbench/contrib/localization`

## 19. Product shell, privacy, accessibility, release

Scope: welcome page and walkthroughs, update channel for the port's own builds, relaunch, splash, accessibility (VoiceOver through GPUI's accessibility tree, accessibility signals, high-contrast checks), process explorer and performance views, telemetry inert end to end, packaged and signed `.app`.

Done when: the telemetry source search (no telemetry endpoint string reachable) and network capture (zero outbound requests in a 10 min session with no user action) pass; VoiceOver reads the editor line and the Explorer rows; the signed `.app` passes `spctl -a`; benchmark rerun against a packaged reference build.

Rows: `telemetry/source-search` `telemetry/network-capture` `mainthread/MainThreadTelemetry` `mainthread/MainThreadPower` `mainthread/MainThreadMeteredConnection` `platform/telemetry` `platform/product` `platform/update` `platform/accessibility` `platform/accessibilitySignal` `platform/userInteraction` `platform/meteredConnection` `platform/diagnostics` `platform/profiling` `platform/onboarding` `workbench/services/telemetry` `workbench/services/update` `workbench/services/accessibility` `workbench/services/power` `workbench/services/timer` `workbench/services/userActivity` `workbench/services/userAttention` `workbench/contrib/telemetry` `workbench/contrib/update` `workbench/contrib/relauncher` `workbench/contrib/splash` `workbench/contrib/accessibility` `workbench/contrib/accessibilitySignals` `workbench/contrib/performance` `workbench/contrib/processExplorer` `workbench/contrib/meteredConnection` `workbench/contrib/onboarding` `workbench/contrib/welcomeBanner` `workbench/contrib/welcomeGettingStarted` `workbench/contrib/welcomeOnboarding` `workbench/contrib/welcomeViews` `workbench/contrib/welcomeWalkthrough`

## 20. Parity

Scope: nothing new; close the gaps the earlier milestones left.

Done when: `port/tools/api-tests` gives 337/31/0 (folder) and 4/0/0 (workspace): the reference baseline 356/46/0 less the suites of removed features (port/results/api-tests-reference.md); pixel-check of every window has no difference beyond anti-aliasing and the causes listed in `port/results/pixels.md`; every ledger row is `verified`, `removed` or `n/a`.

## Decisions (Artur)

Made: AI, Microsoft-service and telemetry parts and remote/tunnels are `removed` (41 rows); upstream test infrastructure (`base/test`, `editor/test`, `platform/test`, `workbench/services/driver`) is `n/a`; the api-tests suites of removed features leave the milestone 20 target; Electron process, IPC and DOM layers (`base/browser` `base/parts` `platform/domWidget` `platform/ipc` `platform/protocol` `platform/sandbox` `platform/sharedProcess` `platform/utilityProcess` `workbench/services/sharedProcess` `workbench/services/utilityProcess`) are `n/a`, replaced by the native architecture.

Open:

1. Architecture rows: Electron, DOM and process-model layers the Rust/GPUI port replaces rather than ports. Proposal: a new ledger status `n/a` (replaced by the native architecture), so they stop counting as work. Rows: `base/common` `base/node` `platform/instantiation` `platform/observable` `platform/webWorker` `platform/gpu` `platform/cssDev` `editor/standalone` `editor/contrib/gpu` `workbench/contrib/screenshotTool`

## Open findings outside the milestones

- Bun host dropped from 57 to 41 MiB between milestones 1 and 2, not investigated (`port/results/benchmark.md`).
- Wu measured 99 MiB once and 154 to 156 MiB later, cause not investigated.
- Bun runs without `@vscode/native-watchdog` (oven-sh/bun#18546): a host with a stuck event loop is not killed. Recheck on each Bun upgrade.

## Coverage check

Every `todo`, `stub` and `ported` ledger row appears once in this file. The first command lists rows missing here, the second rows named twice; both print nothing when the file is complete:

```
comm -23 <(rg -o '^\| (\S+) \| .* \| (todo|stub|ported) \|' -r '$1' port/ledger.md | sort) <(rg -o '`([a-z]+/[A-Za-z0-9/.-]+)`' -r '$1' port/roadmap.md | sort -u)
rg '^(Rows:|[0-9]\. )' port/roadmap.md | rg -o '`[a-z]+/[A-Za-z0-9/.-]+`' | sort | uniq -d
```
