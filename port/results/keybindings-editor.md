# Keyboard Shortcuts editor

## Milestone 9: the modal editor, search, record keys, define keybinding

Reference: 1.147.0 dev build on the Studio; port: release `code-gpui`. Both driven by `ui-pair` (cua-driver) with fixture `port/fixtures/workspace`, Dark Modern, 1024x768 at 2x. Pairs in `raw/keybindings-editor/`, reference on top, port below.

- `open.png` (Cmd+K Cmd+S): both open the modal over the workbench with the "Keyboard Shortcuts" header, the open-in-product, move-to-main-window, maximize and close actions, the search box with Record Keys, Sort by Precedence and Clear, and the Command, Keybinding, When, Source table with alternating rows. The port draws Open Modal Editor in Main Window disabled (`shortcut:`, the editor area hosts text editors only). The rows differ by the commands the port does not register yet; the order and the `-` for an empty when match.
- `search.png` (type `copy`): rows grow to 40 px for a matched command id, the matched parts are bold in `list.highlightForeground`, the extension source is a link color.
- `record.png` (Record Keys, Cmd+C): the "Recording Keys" badge, the toggled icon and the search `"cmd+c"` with the matched key caps highlighted, in both, and the same rows with their titles ("Terminal: Copy Selection"). The badge is modern UI's (`fontSize.label3` 10px, `cornerRadius.small`), 82x22 in both. The search caret blinks in both; a frame holds the phase of its moment (three frames 260 ms apart: caret, caret, none in the reference; none, caret, caret in the port).
- `define.png` (Cmd+Down, Return, Cmd+K Cmd+J): Cmd+Down focuses the table and selects the first row in both. In the reference Return opens the define widget. In the port the define widget opens and then closes on blur: under `ui-pair` the port window turns inactive after the key post (`window.is_window_active()` false in the blur, measured with a scratch log), and the widget cancels on blur as upstream's `onBlur(() => this.onCancel())` does on a DOM blur. Interactive keys keep the window active; `keys_define_a_keybinding_that_lands_in_keybindings_json` covers Return, the recorded chords, Enter and the written keybindings.json.
- `escape.png`: Escape in the define widget closes the modal in both: the modal's capture listener runs `workbench.action.closeModalEditor` before the widget's key handler.
- Fixed after the first pairs: the table columns take the proportional `SplitView` layout (weights of the whole width, 120 px minimum, the 40 px actions column taken from Source then When), and command ids and when expressions take the monospace `.code` style, `foreground` at 0.8 over `WORKBENCH_BACKGROUND` or over the row's selection, focus or hover colors.

Checks (Studio):

- `cargo test -p app keybindings_editor_view`: 6 passed.
- `cargo test -p app`: 591 passed, 1 ignored. `cargo test -p config`: 50 passed.
