# Pixel baseline

`port/tools/pixel-check` on the Mac Studio: the reference workbench captured through CDP (`port/tools/ref-screenshot`, 1024x768 at 2x, frozen: solid cursor, reduced motion, no startup editor, `chat.disableAIFeatures`) against the port's frame read back from Metal (`code-gpui --theme <id> --screenshot`, built with `--features screenshot`). Neither capture needs Screen Recording permission. Diff: `native/crates/pixeldiff`, threshold 0, anti-aliased pixels counted apart.

## Merge of milestone 8 into the integrate branch

Full `port/tools/pixel-check` in the integrate worktree on the merge's working tree.

| window | dark | light | hc | hclight |
|---|---|---|---|---|
| empty | 3567 | 3600 | 398 | 401 |
| folder | 7238 | 4067 | 972 | 886 |
| file | 10342 | 7164 | 4696 | 5468 |
| main | 11927 | 8685 | 5842 | 7141 |
| multiroot | 7371 | 4143 | 1110 | 951 |
| folded | 21969 | 28077 | 9754 | 11552 |

The reference frames moved since milestone 8's run (1750 to 4642 pixels between the two reference captures of file, main and folded). Against this run's reference frames, milestone 8's own port frames give file 13772/10561/5985/6726, main 14864/11634/6605/7912 and folded 25614/31678/11297/13062, so the merge is closer in all 12. Against the milestone 12 table, file light and hclight rose by 489 and 1308, the same amounts milestone 8's minimap added (6376 to 6865, 3853 to 5161). folder hc and hclight are 12 above the milestone 12 table; not chased.

## Milestone 8: sticky scroll

`WINDOWS="folded file main" port/tools/pixel-check` in the m8-viewport worktree with the sticky scroll widget. The port now pins `#region Serialization` (line 47) over line 58 like the reference; the two crops match by eye. file and main are the merge table's numbers except main hclight (6873, 268 lower). In folded light the sticky shadow rows differ by 1 per channel (228 against 229 at the shadow's top); the rest of the light difference was there before sticky scroll: the current line border on line 75, a full-height column left of the minimap, and the overview ruler cursor mark (milestone 8 item e).

| window | dark | light | hc | hclight |
|---|---|---|---|---|
| file | 10342 | 7164 | 4696 | 5468 |
| main | 11927 | 8685 | 5842 | 6873 |
| folded | 14070 | 27529 | 6987 | 9021 |

## Milestone 8: find widget

`WINDOWS=find port/tools/pixel-check`: notes.txt with `cmd-f t h e` typed (`ref-screenshot` now types bare letters through `Input.insertText`, 100 ms apart, so the reference's find input receives them). The port first left the word highlight on "the" instead of "Port": `wordHighlighter.ts` `_run` ignores selections made while the editor has no text focus, so the port now keeps the highlighted word when a find action moves the selection. The widget shadow was twice as wide: GPUI's `blur_radius` is the Gaussian sigma, CSS's blur is twice the sigma, so the 12 px of `--vscode-shadow-lg` is now 6 (light 66221 to 25512, hclight 62722 to 17703; dark rose 34648 to 45168 because the line 1 range highlight under the old, wider shadow happened to land 1 level darker, as the reference's). Left: the status bar text "Ln 1, Col 19 (3 selected)" sits 3 px right of the reference's (about 3000 pixels in every theme), the find input's caret blink phase, glyph anti-aliasing. The quick input (`blur_radius` 20) and sticky scroll shadows use the same CSS numbers and are likely twice as wide too; not chased here.

| window | dark | light | hc | hclight |
|---|---|---|---|---|
| find | 45168 | 25512 | 9562 | 17703 |

Status bar widths and quick input shadows, `WINDOWS="file find quickopen folded" port/tools/pixel-check`: each editor status item now takes the width of its `tnum`-shaped label plus padding, since GPUI rounds a text box up to a whole px where Chrome keeps the fraction, which moved "Ln 1, Col 19 (3 selected)" 3 px; file dropped in every theme (dark 10342 to 8776, light 7164 to 5630, hc 4696 to 3124, hclight 5468 to 3935), folded too (14070 to 12848, 27529 to 26290, 6987 to 5767, 9021 to 7782). The quick input shadow (`--vscode-shadow-xl` 20 px) is now `blur_radius` 10 and the context menu's 12 px is 6, as the find widget's; sticky scroll's `0 4px 2px -2px` was already sigma 1. The find light count swings between runs (25512, 59601, now 21507), not explained yet; a likely part is the find input caret, which blinks in the port while the reference's is frozen by `editor.cursorBlinking: solid`. New window quickopen (notes.txt open, Cmd+P): the port draws the input caret the reference hides at capture, keeps the "Port" word highlight the reference drops when the editor blurs (`onDidBlurEditorWidget`), and places the row's "recently opened" description and close button a few px apart from the reference's; not chased here.

| window | dark | light | hc | hclight |
|---|---|---|---|---|
| file | 8776 | 5630 | 3124 | 3935 |
| find | 41154 | 21507 | 7189 | 15385 |
| quickopen | 44261 | 64254 | 36394 | 82041 |
| folded | 12848 | 26290 | 5767 | 7782 |

Word highlight on blur and caret phase, full `port/tools/pixel-check`: the editor drops its word highlights when its focus moves out (`onDidBlurEditorWidget`) and runs them again on focus in; `--screenshot` draws one more frame 100 ms before the capture, so an input's caret shows the phase it has at capture time, as Chromium's does. Neither find nor quickopen shows an input caret in either frame now. Only find and quickopen moved; the other six windows kept the numbers above.

| window | dark | light | hc | hclight |
|---|---|---|---|---|
| find | 41078 | 21434 | 7113 | 15312 |
| quickopen | 42349 | 62398 | 36073 | 81662 |

At 2% fuzz (`magick compare -fuzz 2% -metric AE`) find differs in 2452 (dark) and 1600 (light) of 3.1M pixels and quickopen light in 2266: glyph antialiasing, the left edge of each find match box, and in quickopen the row's Open to the Side button. That button needs editor splits, which the port has not (`supportsSplitting`), so the reference's "recently opened" description sits one button further left. The rest of the pixeldiff count is the quick input shadow, which differs by under 2% per pixel (GPUI blur against CSS `box-shadow`).

Frozen input carets and the quick input in high contrast, full `port/tools/pixel-check`: ref-screenshot sets `caret-color: transparent` before the capture and the port's `--screenshot` hides its input carets (`Workbench::input_carets_hidden`), so the blink phase no longer decides a frame; find and quickopen kept their counts, both carets were already off at the 1.6 s mark. In high contrast the reference draws the window border and lays the quick input inside the workbench container, 1 px right and down; the port drew it at the window edge. It now sits at the grid's inset, and its focused row gets the 1 px `listInactiveFocusOutline` (contrastActiveBorder), dashed because GPUI has no dotted border. Only quickopen hc and hclight moved, and main hc and hclight, where the TypeScript server's timing decides whether the reference shows the breadcrumb symbol, the lightbulb and the language status items.

| window | dark | light | hc | hclight |
|---|---|---|---|---|
| main | 11501 | 8189 | 6953 | 6645 |
| quickopen | 42349 | 62398 | 10456 | 20740 |

What is left in quickopen: the Open to the Side row button (no editor splits), which moves the "recently opened" description one button left in the reference while the close button sits at the same x; the quick input shadow, whose GPUI blur bands against the CSS `box-shadow` (under 2% per pixel); the frosted glass fill in dark and light, where the reference blurs the title bar behind the widget's header to #202020 and the port fills #222222; and glyph antialiasing.

## Milestone 9: commands, keybindings, quick input

No new window: pixel-check has no quick input. All 16 port frames are byte-identical to the milestone 7 run (md5 of `port-<theme>.png` per window).

| window | dark | light | hc | hclight |
|---|---|---|---|---|
| empty | 3567 | 3612 | 398 | 401 |
| folder | 7053 | 3882 | 775 | 689 |
| file | 10859 | 6376 | 4396 | 3853 |
| main | 12177 | 7627 | 5335 | 5059 |

- Rerun after the command history, quick input position and theme picker: empty light 3600, every other number as in the table.
- Ad-hoc, theme picker (Cmd+K Cmd+T over the fixture folder, Dark Modern, 2x), the widget crop against the reference's: 2085.9 pixels (0.25%) differ at 10% fuzz; the rest is text antialiasing and the input's focus border.
- empty light (3612, was 3600) and main hclight (5059, was 6626) moved with the reference capture only: its md5 changed, the port's did not; main hclight is the overview ruler artifact of milestone 7.
- Ad-hoc, not a pixel-check result, measured at the palette commit before the drag: the Command Palette over the empty window (Dark Modern, 2x), the widget crop against the reference's, 2803.8 pixels (0.28%) differ at 2% fuzz (`magick compare -fuzz 2% -metric AE`); the rows and the selection highlight sit at the reference's y positions.
- Rerun after Quick Open's file picks: 3567/3600/398/401, 7238/4067/960/874, 11158/6675/4695/4160, 12681/8133/5841/5565 (empty to main, dark to hclight), against the merged reference frames of the milestone 12 table; only main hclight moved (7132 there), the overview ruler artifact.
- Ad-hoc, Quick Open (Cmd+P over port/fixtures/workspace with docs/guide.md, Dark Modern, 2x, `--screenshot` with the picker opened by a scratch extension) against the reference's CDP capture: the empty, `ts` and `util` pickers show the same rows, separators, file icons and highlights; the reference has one more row button, Open to the Side, which needs editor splits (port/notes.md).
- chord window (file window after Cmd+K, the status bar's "(⌘K) was pressed. Waiting for second key of chord..."): dark 12215, light 9281, hclight 7585, against file's 10342/7164/5468 in the same run; hc 6557 from an earlier run. The Studio's pointer must be away from the window: over the minimap it gave file 38071 (the slider's hover state), so park it in a screen corner before the run.
- titlebar window (`window.commandCenter` off, the window title in a 32px bar): dark 8751, light 5620, below file's 10342/7164 of the same run. At 30px high it gave 63067/58822/57378 (every part below the bar 2px off).

## Milestone 12

Measured at the merge of milestones 9, 10 and 12 (`port/tools/pixel-check` in the integrate worktree). The reference capture changed between runs (folder dark md5 `bdd99333` here, `e56bf35b` an hour later), so the milestone 7 numbers are not comparable; against the same reference frames the port before milestone 12 (f8114f55) gives folder 10105/6891/3787/3696 and main 18447/13868/10935/10687, so Seti icons, tab icons and numeric sort lower every count.

| window | dark | light | hc | hclight |
|---|---|---|---|---|
| empty | 3567 | 3600 | 398 | 401 |
| folder | 7238 | 4067 | 960 | 874 |
| file | 11158 | 6675 | 4695 | 4160 |
| main | 12681 | 8133 | 7383 | 7132 |

Why the reference capture moves between runs is not found yet; milestone 8 also hit hung captures (a8512a97 on port-m8-viewport).

## Milestone 8: editor viewport

Scrollbars, scroll shadow and visible ranges added; the frozen captures show the top of each file, so nothing may move. `WINDOWS="file main"`: both unchanged from milestone 7. The first main hc run gave 16065 against a reference capture with different breadcrumbs and the cursor on line 4; the retry gave 5335.

After multi-cursor (ea71708c), one cursor in both captures: `WINDOWS="file main"` gives the same eight numbers.

| window | dark | light | hc | hclight |
|---|---|---|---|---|
| file | 10859 | 6376 | 4396 | 3853 |
| main | 12177 | 7627 | 5335 | 6626 |

After folding: full run with the new `folded` window, `port/fixtures/folding/long.py` with keys `cmd-down up alt-cmd-[ up×12 alt-cmd-[` (lines 76-87 and 100 hidden). empty, folder and file give the milestone 7 numbers; main hclight 5059. In that run the port's folded capture failed with an empty log (likely the 120s `timeout`); the folded row is from a `WINDOWS=folded` run after it. `folded` went from 235332 (dark) to 44105 once the folding actions revealed centered if outside the viewport, and to 23034 once the scroll position truncated to whole pixels (`forceIntegerValues`). What remains is the reference's sticky scroll line (`#region Serialization` pinned over line 57), which comes with sticky scroll.

| window | dark | light | hc | hclight |
|---|---|---|---|---|
| empty | 3567 | 3600 | 398 | 401 |
| folder | 7053 | 3882 | 775 | 689 |
| file | 10859 | 6376 | 4396 | 3853 |
| main | 12177 | 7627 | 5335 | 5059 |
| folded | 23034 | 29087 | 9736 | 8806 |

`port/tools/pixel-check` now runs each port capture through `capture()`: one still alive after 120 s gets `sample` stacks in `<window>/port-<theme>.sample`, then exits 124, so a hang leaves evidence instead of an empty log. Then two full runs back to back (`port/tools/pixel-check && port/tools/pixel-check`): all 20 windows gave numbers in both, each equal to the table above. The empty-log failure did not recur; its cause is unproven.

After the minimap: the reference draws its minimap now (`ref-screenshot` no longer sets `editor.minimap.enabled: false`), and the port's capture waits 1.5 s after activation even without keys, as the reference does with a file, so the scrollbars shown at open fade. The lines clip at the content width like `editorScrollbar.ts`, so the current line border stays out of the column right of the minimap. empty and folder unchanged; `WINDOWS="file main folded"` gives the rows below. In one full run folded dark gave 45702 with the port's minimap slider still shown; the rerun gave the table's number and the cause is unproven. File and main stay above the milestone 7 numbers; one difference is the overview ruler: in the reference its cursor mark is gray (198,199,197 in hclight), in the port `editorCursor.foreground` at 70% as `decorationsOverviewRuler.ts` reads it; not chased in this milestone.

| window | dark | light | hc | hclight |
|---|---|---|---|---|
| empty | 3567 | 3600 | 398 | 401 |
| folder | 7053 | 3882 | 775 | 689 |
| file | 10043 | 6865 | 4397 | 5161 |
| main | 11423 | 8179 | 5336 | 6367 |
| folded | 21817 | 27924 | 9601 | 11399 |

## Milestone 7: syntax colors and themes

New window `main`: `port/fixtures/workspace/src/main.ts` open in the fixture folder, TypeScript grammar colors and bracket pair colors. New theme `hclight` (Default High Contrast Light) for every window. The reference now runs with `--force-color-profile=srgb` (port/notes.md); with it the empty, folder and file windows give the milestone 2, 3 and 4 numbers unchanged. Reference md5 (main): dark `c1172c60`, hc `1cf27616`, hclight `fc82315e`, light `2eba5cf1`.

| window | dark | light | hc | hclight |
|---|---|---|---|---|
| empty | 3567 | 3600 | 398 | 401 |
| folder | 7053 | 3882 | 775 | 689 |
| file | 10859 | 6376 | 4396 | 3853 |
| main | 12177 (0.39%) | 7627 (0.24%) | 5335 (0.17%) | 6626 (0.21%) |

Different pixels; anti-aliased in main: 19723 / 18907 / 21547 / 20630; total 3145728.

Pixels of main more than 2/255 off in any channel, anti-aliased ones included, by region (dark / light / hc / hclight): status bar 6526 / 6333 / 6761 / 6506, side bar 6127 / 4837 / 8052 / 6578, tabs and breadcrumbs 4621 / 4489 / 5065 / 5059, title bar 1598 / 1541 / 1782 / 1695, activity bar 1458 / 1614 / 1911 / 1837, text 819 / 775 / 843 / 874, gutter 194 / 229 / 337 / 253, overview ruler 17 / 34 / 45 / 1352.

- Text: glyph edges only. Every token and bracket color matches the reference; the region has fewer differences than the plain text of the file window.
- Status bar, side bar, tabs, title and activity bar: the milestone 2 to 4 differences (text-width rounding, codicon rasterization, dotted borders drawn dashed).
- Overview ruler in hclight: a reference artifact. In some runs the reference's high contrast ruler canvas keeps its pre-switch paint, without the 1px `editorOverviewRuler.border` and the cursor mark; in this run hclight, in the run before hc (1352 there, 45 here).
- Fixed in this milestone: High Contrast Light outlines the active activity bar item instead of filling it (menuActivityBar.css `.hc-black, .hc-light`), which took hclight folder from 3812 to 689.

No difference is a layout or color mismatch of the port's own making beyond the text-width rounding.

## Milestone 5: edit and save

The file window with the editing work in (cursor drawn only when the editor is focused, selection drawing, dirty tabs) gives the milestone 4 numbers below unchanged in all three themes: dark 10859, light 6376, hc 4396 different pixels.

## Milestone 4: file open

`notes.txt` open from the command line in the fixture folder (`FILE=... ref-screenshot`, `code-gpui <folder> <file>`), cursor at 1:1. The empty and folder windows are diffed in the same run and give the milestone 2 and 3 numbers unchanged. Reference md5: dark `91d050ee`, light `91283cb6`, hc `f841cf99`.

| theme | different | anti-aliased | total |
|---|---|---|---|
| dark (Dark Modern) | 10859 (0.35%) | 26998 | 3145728 |
| light (Light Modern) | 6376 (0.20%) | 25512 | 3145728 |
| high contrast (Default High Contrast) | 4396 (0.14%) | 29520 | 3145728 |

Differences of more than 2/255 per channel by region (dark / light / hc); the rest are 1 or 2 per channel, alpha blending rounding as before (in dark mostly the title bar and side bar as in milestone 3, plus 1363 in the overview ruler's 1px border at half coverage):

- Status bar (1878 / 1907 / 1893): the right-aligned editor items sit up to 2 CSS px left of the reference. GPUI rounds each text box's width up to a whole logical pixel (`gpui/src/elements/text.rs` `size.width.ceil()`), Chrome keeps the fraction ("Ln 1, Col 1" is 61.34px with `tabular-nums`, as `statusbarpart.css` asks and the port now does), so the five items gain 0.05 to 0.67px each and the shift adds up from the right.
- Side bar (305 / 269 / 982): the milestone 3 chevron and label edges, and in high contrast the selected row's outline: the reference draws `1px dotted` activeContrastBorder, the port a dashed border (GPUI has no dotted style).
- Text (239 / 246 / 529): glyph edges; glyph x positions match the reference to the device pixel. In high contrast also the word highlight's dotted border, drawn dashed.
- Tabs and breadcrumbs (300 / 325 / 436): codicon edges (close, split, more, breadcrumb chevron: resvg against Chrome's codicon font), label glyph edges, and the active tab's right edge at x 400.8, which Chrome anti-aliases and GPUI snaps to the device pixel.
- Title bar (119 / 145 / 138) and gutter (13 / 20 / 37): glyph edges.
- Overview ruler (0 / 81 / 79): a reference artifact. `decorationsOverviewRuler.ts` stores the cursor color with the cursor position and only refreshes it on the next cursor move, so after the harness's startup switch from the default dark theme the reference still paints the dark `#AEAFAD` cursor at 0.7 alpha in light and high contrast (sides 193,194,192 on white; 122 on black). The port uses the current theme's `editorCursor.foreground`, as a user who moves the cursor sees.

No difference is a layout or color mismatch of the port's own making beyond the text-width rounding above.

## Milestone 3: folder open

`port/fixtures/workspace` open (`FOLDER=... ref-screenshot`, `code-gpui <folder>`). The empty window is diffed again in the same run and gives the milestone 2 numbers unchanged. Reference md5: dark `e56bf35b`, light `77c70fbd`, hc `32e53f8c`.

| theme | different | anti-aliased | total |
|---|---|---|---|
| dark (Dark Modern) | 7053 (0.22%) | 13693 | 3145728 |
| light (Light Modern) | 3882 (0.12%) | 14352 | 3145728 |
| high contrast (Default High Contrast) | 775 (0.02%) | 15063 | 3145728 |

What the different pixels are, split at a channel difference of 2/255:

- 1 or 2 per channel (dark 6466, light 3292, hc 119): alpha blending rounding, as in milestone 2. New in dark is the Explorer item's active background, `#FFFFFF22` over `#181818`: 54 in Chrome, 55 in the port (exact value 54.8), all 4096 pixels of the 32px square.
- More than 2 (dark 587, light 590, hc 656): glyph edges. About half sit in the side bar (dark 293, light 257, hc 340): the chevrons rasterized by resvg against Chrome's codicon font, and edge pixels of the tree labels. The rest are the milestone 2 title and status bar glyphs.

Before the fix the high-contrast window had 2343 different pixels: an explorer item there has a 20px line from the row top (`explorerviewlet.css`), and the side bar title keeps its 32px line under its 1px bottom border; the port had centered both.

No difference is a layout or color mismatch.

## Milestone 2: empty workbench

| theme | different | anti-aliased | total |
|---|---|---|---|
| dark (Dark Modern) | 3567 (0.11%) | 7759 | 3145728 |
| light (Light Modern) | 3600 (0.11%) | 8958 | 3145728 |
| high contrast (Default High Contrast) | 398 (0.01%) | 8983 | 3145728 |

Two runs of `ref-screenshot` gave byte-identical reference frames. What the different pixels are:

- In dark and light about 90% differ by 1 or 2 of 255 per channel: alpha blending rounds differently in Skia and Metal (command center border over its background: 69 against 70 in dark). High contrast has no translucent colors, so it has almost none.
- The rest, 338 to 347 per theme, are glyph edges: the codicons (reference: `codicon.ttf` through Chrome's text rasterizer; port: the same icons' SVGs through resvg, up to 19/255 at the edges), and text advance in the status bar (" 0 " is about half a device pixel wider in GPUI, so the warning icon and the second "0" land one device pixel right) and in the "Search" label.

No difference is a layout or color mismatch.

## Milestone 1 (before the workbench)

| theme | different | anti-aliased | total |
|---|---|---|---|
| dark | 247892 (7.9%) | 24713 | 3145728 |
| light | 3120870 (99.2%) | 24856 | 3145728 |
| high contrast | 3120455 (99.2%) | 25259 | 3145728 |

The port drew a status window with no themes, diffed against every theme. Control: the reference dark capture against itself gives 0 different.

## Milestone 12 multi-root

New window `multiroot`: `multi.code-workspace` with the fixture folder and a copy of its `docs` folder named "Second Folder". The other 16 frames give the milestone 12 table's counts except main hc 5841 and hclight 5565 (were 7383, 7132), with the reference capture moving as noted above.

| window | dark | light | hc | hclight |
|---|---|---|---|---|
| multiroot | 7371 | 4143 | 1110 | 951 |

The Explorer rows (header `multi (Workspace)`, both roots expanded, root children at depth 2) sit at the reference's positions; the counts are the folder window's plus the second root's rows.

## Milestone 12 Explorer file operations

Rerun after the Explorer file operations, on the Studio. Every frame printed gives the multi-root section's counts (folder light/hc/hclight 4067/960/874, file 11158/6675/4695/4160, main 12681/8133/5841/5565, multiroot 7371/4143/1110/951): the captured windows show no focused tree, edited row or drag, so nothing they draw changed.

## Milestone 12 Explorer checked against the reference

Rerun after the Explorer fixes from the reference comparison (inline name row, NSAlert confirmations), on the Studio, `WINDOWS="folder multiroot"` plus a full run: every frame gives the multi-root section's counts (folder 7238/4067/960/874, multiroot 7371/4143/1110/951, empty, file and main unchanged). A first run gave folder 7967/4796/1689/21117 and multiroot 8628/5400/2367/22279: the Studio's pointer was left over the package.json row by `port/tools/ui-pair`, and the port draws the hover row (hc themes: dashed `contrastActiveBorder` outline). Move the pointer off the window before a pixel-check (`cua-driver call move_cursor '{"x": 3, "y": 400}'`).
