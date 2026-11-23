# Pixel baseline

`port/tools/pixel-check` on the Mac Studio: the reference workbench captured through CDP (`port/tools/ref-screenshot`, 1024x768 at 2x, frozen: solid cursor, reduced motion, no startup editor, `chat.disableAIFeatures`) against the port's frame read back from Metal (`code-gpui --theme <id> --screenshot`, built with `--features screenshot`). Neither capture needs Screen Recording permission. Diff: `native/crates/pixeldiff`, threshold 0, anti-aliased pixels counted apart.

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
