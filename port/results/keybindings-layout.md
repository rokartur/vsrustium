# Keybindings on a non-US layout

Studio run of `port/tools/ui-pair` (port only, release build of port-m9b-commands with `keyboard_mapper.rs`), keyboard layout `com.apple.keylayout.PolishPro` ("Polish", `pl`):

- Cmd+Shift+P opens the Command Palette; letters typed key by key reach the filter (`>inspect key mappings`, both Inspect Key Mappings commands listed).
- Developer: Inspect Key Mappings writes the layout info and the "Default Resolved Keybindings (unique only)" table of `MacLinuxKeyboardMapper.dumpDebugInfo` (`cmd+DownArrow => meta+[ArrowDown]`, `ctrl+N => ctrl+[KeyN]`) into an untitled editor.
- The chord Cmd+K Cmd+T dispatches through scan codes and opens Select Color Theme with Dark Modern focused.
- Preferences: File Icon Theme lists None (Disable File Icons), Minimal (Visual Studio Code) vs-minimal, Seti (Visual Studio Code) vs-seti focused, VS Code Modern Icons vscode-modern-icons, under "file icon themes".

The byte-level checks of the mapper are `cargo test -p app keyboard_mapper` against the upstream mac_de_ch, mac_en_us, mac_zh_hant and mac_zh_hant2 fixtures.
