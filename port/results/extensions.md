# Extensions on the port

Milestone 18. Run on the Mac Studio from the m18 mirror.

## PORT_GALLERY=1 port/tools/extensions-check

```
ok an ESM extension activates after install
ok Install from VSIX adds, uninstall removes
ok an Open VSX extension installs by id, verified, and activates
ok _workbench.downloadResource copies a file and fetches a URL
ok uninstall errors
exit 0
```

The gallery check installed `redhat.vscode-yaml-2.0.0-universal` (the latest release; 2.1.2026112008 is a pre-release) into `extensions/` with the VSIX in `CachedExtensionVSIXs/`, after the ed25519 signature and the manifest digests verified.

## exthost-probe --install-vsix vsix-esm.vsix --uninstall port.vsix-esm

```
{"job":"InstallVsix(\".../vsix-esm.vsix\")","error":null,"runningUserExtensions":["port.vsix-esm"],"activated":["port.vsix-esm"],"activationErrors":[]}
{"job":"Uninstall(\"port.vsix-esm\")","error":null,"runningUserExtensions":["port.vsix-esm"],"activated":["port.vsix-esm"],"activationErrors":[]}
```

The uninstall leaves the activated extension running until a restart, as upstream.

## cargo test

exthost 34 passed (baseline 19), app 961 passed and 1 ignored (baseline 961/1), config 50 (baseline 50).

Open VSX gallery (item 1): exthost 51 passed (baseline 34: +3 request, +5 sign, +9 gallery), app 1035 passed and 1 ignored (baseline 1034/1, +1 `id_and_version`).

## Extensions view, recommendations, display language (items 2 to 4)

Run on the Mac Studio from the m18b-gallery mirror.

- `cargo test -p app -- nls localization recommendation extension`: 49 passed (extensionsViews.test.ts and extensionQuery.test.ts queries, extensionEnablementService.test.ts states, extensions.json from `.vscode/` and `.vsrustium/`, prompt text, NLS index, Configure Display Language items). `cargo test -p exthost -- language_packs scan`: 5 passed.
- `ONLY=port port/tools/ui-pair /tmp/m18-port port/fixtures/extensions/ui-view.json dark` against the reference shots of `REF=~/Developer/vscode-ref-openvsx` (product.overrides.json pointing at open-vsx.org): default views (Installed 0, Popular, Recommended), the `yaml` search (Marketplace view, rows with installs, rating, Install and Install Pre-Release) and the YAML details editor (header, Details README, Marketplace, Categories, Resources) match; the port wraps the Marketplace table's values at a different width.
- Workspace recommendations: `.vscode/extensions.json` with `redhat.vscode-yaml` and `rust-lang.rust-analyzer`, `port/fixtures/extensions/ui-recommendations.json`. Reference and port (`ONLY=port` against the reference shots) both show the prompt "Do you want to install the recommended extensions from rust-lang and Red Hat for this repository?" (Install, Show Recommendations, gear) 8 s after start and the Recommended view with 2 entries, YAML then rust-analyzer. The port's Popular view takes less height above it than the reference's. cua-driver cannot type `@`, so the `@recommended` search is covered by `cargo test -p app extension` only.
- Display language: with MS-CEINTL.vscode-language-pack-de from Open VSX in `--extensions-dir`, the first start writes `languagepacks.json` (`["de"]`); `--locale de` then shows the Command Palette in German ("Alle Befehle anzeigen", "Ansehen: Erweiterungen anzeigen") and the Outline and Timeline panes as "Gliederung" and "Zeitachse". The dev reference cannot be compared: base/node/nls.ts keeps English for builds without a commit.
- `PORT_GALLERY=1 port/tools/extensions-check`: 5 ok, exit 0.
- api-tests `MOCHA_GREP="vscode server cli"` (extensions.test.ts): api-folder 0 passing, 1 pending, 0 failing; the test skips itself outside a remote integration build, as in the reference.
