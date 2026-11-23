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

`GitHub.copilot-chat` (built in at 1.147.0) does not finish activating within 60 s on either runtime with the stub main thread; it waits on auth/language-model main-thread calls. It is a Microsoft service client; whether it ships at all is a product decision (open question for Artur), since the port must not add Microsoft endpoints.

## Benchmarks

- `port/tools/bench`: Wu's `script/memory-benchmark` method generalized to N apps (`APPS="name=cmd;..."`), plus an extension host line per app.
- Wu's `.cargo/config.toml` links with Homebrew lld on aarch64-apple-darwin; the Studio has no lld, so Wu is built with `RUSTFLAGS="-C symbol-mangling-version=v0 --cfg tokio_unstable"` (same flags, Apple ld). Env `CARGO_TARGET_*_RUSTFLAGS` does not work for this: cargo appends it to the config array.
