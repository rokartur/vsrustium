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
