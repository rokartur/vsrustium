# Editor windows and tab drags between windows

Verified against VS Code 1.147.0 with `port/fixtures/ui-pair/m11/auxiliary-windows.json` (`FILE=notes.txt`, dark): Move Editor into New Window (the editor window and the emptied main window), the editor window moved and resized, a tab dragged from the editor window onto the main window's empty part (the drop overlay there, the editor moved, the emptied window closed), Copy Editor into New Window, Cmd+Shift+W in the editor window (the copy closes, the main window keeps its editor), a tab dragged out of the main window onto the desktop (a new editor window there), Cmd+Q and the restart. The window list after each step (titles, sizes, count) is the reference's at every step. `cargo test -p app auxiliary_windows` (4 tests: move with Cmd+W in the editor window, close moving the dirty editor back, a drop across parts, the parts state restored).

Differences left:
- The editor window's title bar has no Turn On Compact Mode action (compact mode is not ported: it needs `showTabs: none` per part) and its tab bar no Lock Group action (locked groups are not ported).
- While a tab drags over another window the port draws its drag image (the tab label) in that window; Chromium's native drag image is a window of its own the captures leave out.
- With a file on the command line neither app restores the editor windows (editorParts.ts restores them only when the main part restores its state); the restore itself is covered by the test above, not by the pair.
- The main window's restored editor shows no Text Editor control until its active editor changes (editor_type_picker.rs `set_restored_editor_type_controls`); the reference shows it after the editor window closes. Owned by the editor type control.
