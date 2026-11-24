# Pixel baseline

`port/tools/pixel-check` on the Mac Studio: the reference workbench captured through CDP (`port/tools/ref-screenshot`, 1024x768 at 2x, frozen: solid cursor, reduced motion, no startup editor, `chat.disableAIFeatures`) against the port's frame read back from Metal (`code-gpui --theme <id> --screenshot`, built with `--features screenshot`). Neither capture needs Screen Recording permission. Diff: `native/crates/pixeldiff`, threshold 0, anti-aliased pixels counted apart.

## Milestone 8: editor viewport

Scrollbars, scroll shadow and visible ranges added; the frozen captures show the top of each file, so nothing may move. `WINDOWS="file main"`: both unchanged from milestone 7. The first main hc run gave 16065 against a reference capture with different breadcrumbs and the cursor on line 4; the retry gave 5335.

| window | dark | light | hc | hclight |
|---|---|---|---|---|
| file | 10859 | 6376 | 4396 | 3853 |
| main | 12177 | 7627 | 5335 | 6626 |

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
