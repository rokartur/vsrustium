# Pixel baseline

`port/tools/pixel-check` on the Mac Studio: the reference workbench captured through CDP (`port/tools/ref-screenshot`, 1024x768 at 2x, frozen: solid cursor, reduced motion, no startup editor, `chat.disableAIFeatures`) against the port's frame read back from Metal (`code-gpui --screenshot`, built with `--features screenshot`). Neither capture needs Screen Recording permission. Diff: `native/crates/pixeldiff`, threshold 0, anti-aliased pixels counted apart.

| theme | different | anti-aliased | total |
|---|---|---|---|
| dark | 247892 (7.9%) | 24713 | 3145728 |
| light | 3120870 (99.2%) | 24856 | 3145728 |
| high contrast | 3120455 (99.2%) | 25259 | 3145728 |

The port draws a status window, not a workbench, and has no themes yet, so every theme is diffed against its one dark frame. Dark is low only because the editor background is #1f1f1f in both. Control: the reference dark capture against itself gives 0 different.

Known gaps in the comparison: the reference is a dev build (its status bar shows the test extension's "Hello"); its accounts icon stays visible although the port drops sign-in.
