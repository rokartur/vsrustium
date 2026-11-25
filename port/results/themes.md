# Color themes against the reference

`OUT=/tmp/pc-themes3 WINDOWS=main THEMES="dark light hc hclight abyss kimbie monokai dimmed quiet red sdark slight tnb" port/tools/pixel-check` on the Studio, at the commit that added this file: `port/fixtures/workspace/src/main.ts` open in each built-in color theme, 2048x1536 pixels (3145728).

| theme | different | anti-aliased |
|-------|-----------|--------------|
| Dark Modern | 11437 | 20825 |
| Light Modern | 8125 | 20073 |
| Default High Contrast | 5327 | 22592 |
| Default High Contrast Light | 6377 | 21757 |
| Abyss | 9965 | 21046 |
| Kimbie Dark | 8758 | 20919 |
| Monokai | 9640 | 21161 |
| Monokai Dimmed | 5611 | 20587 |
| Quiet Light | 8881 | 20128 |
| Red | 5788 | 20782 |
| Solarized Dark | 8361 | 21092 |
| Solarized Light | 8296 | 20488 |
| Tomorrow Night Blue | 5598 | 21115 |

Before this change the nine theme extensions gave 119281 to 233427 different pixels. The causes:

- `editor.lineHighlightBackground` was not drawn; the port drew only the line border. currentLineHighlight.ts draws the background when the theme has one (`editor.inactiveLineHighlightBackground` while the editor is not focused) and the border only without a visible background or when the theme sets the border.
- Pane headers: modernUI/paneHeaders.css paints them with `sideBar.background` and an inset separator, where the port used `sideBarSectionHeader.background`. The two are equal in the default themes.
- The activity bar paints `modernActivityBar.background` (Solarized Light sets it apart from `activityBar.background`), `modernActivityBar.inactiveBackground` while the window is not focused, and the `modernActivityBar.border`.

What is left in each frame is the same in every theme and belongs to other milestones: the breadcrumbs' document symbol (`main`), the code action light bulb and the TypeScript language status item (milestone 13), and the status bar items shifted by it.

## Color variables

`port/tools/ref-dom <dir> colors:<theme>` (with `THEME=<theme>`) writes the reference workbench's `--vscode-*` variables; `port/fixtures/theme-colors` keeps the color ids of `webview/color_ids.txt` for the 13 themes. `cargo test -p app theme::tests::resolved_colors_match_the_reference_workbench_variables` checks that every color the port resolves equals the reference's (the alpha within 1/255, as `rgba()` prints two decimals). It found and fixed: the `dropdown.*` ids (the port registered them as `select.*`, so `checkbox.*` and `menu.background` ignored the themes' dropdown colors), `minimap.findMatchHighlight` (`withOpacity` sets the alpha, it does not multiply it), and the webview variable names of ids with two dots.
