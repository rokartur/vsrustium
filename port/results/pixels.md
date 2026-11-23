# Pixel baseline

`port/tools/pixel-check` on the Mac Studio: the reference workbench captured through CDP (`port/tools/ref-screenshot`, 1024x768 at 2x, frozen: solid cursor, reduced motion, no startup editor, `chat.disableAIFeatures`) against the port's frame read back from Metal (`code-gpui --theme <id> --screenshot`, built with `--features screenshot`). Neither capture needs Screen Recording permission. Diff: `native/crates/pixeldiff`, threshold 0, anti-aliased pixels counted apart.

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
