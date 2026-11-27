# Manage menu and Customize Layout

Verified against VS Code 1.147.0 with `port/fixtures/ui-pair/m11/manage-menu.json` in dark mode (FILE=notes.txt): the Manage menu, the Customize Layout quick pick opened from the Command Palette, Panel shown from the pick (Enter on the third row below Activity Bar), Quick Input Position Center (Enter nine rows below), and the Command Palette afterwards at the center snap line.

- Manage menu: Command Palette, then Profiles, Settings, Extensions, Keyboard Shortcuts, Snippets, Tasks, Themes and Layout Density in that order, with the same keybinding labels (Shift+Cmd+P, Cmd+, , Shift+Cmd+X, Cmd+K Cmd+S). The same as the reference.
- Customize Layout: the title with Restore Defaults and Close, the Visibility, Primary Side Bar Position, Panel Alignment and Quick Input Position sections with their codicons, checks and key caps, the focused row outlined. The same as the reference.
- Panel from the pick: the panel shows, the Panel row gets its eye button and stays focused, and the picker keeps the focus. The same as the reference.
- Quick Input Position Center: the widget moves to the center snap line, Top loses its check and Center gets it. The next Command Palette opens there. The same as the reference.

Fixed during the pairing: a quick pick with its input hidden drew the 2px progress bar and a 2px taller title, so its rows sat 5pt low; its focused row had no `list.focusOutline`; focused and hovered rows drew the key caps without the 30% current-color border; the terminal a shown panel creates took the focus from the picker.

Differences left, outside these rows: the panel's Ports tab (removed by decision); the Command Palette lists no Accounts commands (sign-in removed by decision); after Center, the reference's list is scrolled about 7pt further down than the port's.

Not in the pair: the update items (`7_update`) and the Manage badge show only with an `updateUrl`, which neither the reference dev build nor the port's product.json has; they are covered by `cargo test -p app manage_menu`.

`cargo test -p app` filtered: `the_manage_menu_lists_the_global_activity_items`, `open_user_tasks_writes_a_template_then_opens_the_file`, `customize_layout_shows_the_layout_live_and_restores_the_defaults` and `toggling_the_activity_bar_restores_its_location`.
