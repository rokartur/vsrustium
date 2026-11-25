# Welcome page, view welcome content (milestone 19)

Reference: 1.147.0 dev build on the Studio; port: release `vsrustium`. Both driven by `port/tools/ui-pair` with `port/fixtures/ui-steps/welcome.json` in an empty window, Dark Modern, 1024x768 at 2x, both opening the Welcome page on startup:

```
FOLDER= SETTINGS={\"workbench.startupEditor\":\"welcomePage\"} port/tools/ui-pair /tmp/ui-welcome port/fixtures/ui-steps/welcome.json
```

Shots: `welcome` (startup), `explorer` (activity bar Explorer), `scm` (activity bar Source Control), `walkthrough` (a click on Get started with VS Code).

- `explorer`: the empty view's welcome content is the reference's: "You have not yet opened a folder." with Open Folder, the "add a folder" paragraph (an editor is open), then git's two `explorer` entries (You can clone a repository locally, Clone Repository, read our docs).
- `scm`: git's empty-workspace entry (In order to use Git features..., Open Folder, Clone Repository, read our docs), alone in the container without a pane header, as the reference.
- `welcome`: same grid, header, Start, Recent, Walkthroughs tiles and the show-on-startup checkbox. Left as differences: the header reads `nameLong` (VSRustium); Connect to... is a remote entry (removed by decision); the reference's large headings render heavier (Chromium's system font at 35 and 19.5 px); the reference's tab shows an icon and its focused page a focus outline; the reference counts Choose your theme done in some runs (its `onSettingChanged:workbench.colorTheme` fires while user settings load at startup), the port does not.
- `walkthrough`: back link, title and description, steps with completion circles, the expanded step's text, button and keyboard tip, Mark Done and Next Section. Left as differences: the tip draws a chord as one key cap (`⌘K ⌘T`) where the reference draws one cap per key.

Fixed after the first runs: tiles inherited the list's 24 px line height (the CSS sets `line-height: normal`); step paragraphs did not wrap; the unchecked step circle used a codicon name (circle-large-outline) that ships as circle-large.svg; the details title and description lacked h2 and p margins; a Changes header above the SCM welcome.
