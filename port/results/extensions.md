# Extensions on the port

Milestone 18. Run on the Mac Studio from the m18 mirror.

## port/tools/extensions-check

```
ok an ESM extension activates after install
ok Install from VSIX adds, uninstall removes
ok uninstall errors
exit 0
```

## exthost-probe --install-vsix vsix-esm.vsix --uninstall port.vsix-esm

```
{"job":"InstallVsix(\".../vsix-esm.vsix\")","error":null,"runningUserExtensions":["port.vsix-esm"],"activated":["port.vsix-esm"],"activationErrors":[]}
{"job":"Uninstall(\"port.vsix-esm\")","error":null,"runningUserExtensions":["port.vsix-esm"],"activated":["port.vsix-esm"],"activationErrors":[]}
```

The uninstall leaves the activated extension running until a restart, as upstream.

## cargo test

exthost 34 passed (baseline 19), app 961 passed and 1 ignored (baseline 961/1), config 50 (baseline 50).
