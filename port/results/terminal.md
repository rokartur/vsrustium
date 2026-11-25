# Terminal

ui-pair against the reference 1.147.0, dark theme, fish as the Studio's login shell (`$SHELL`), steps under `/tmp/term` on the Studio.

| Step | Result |
| --- | --- |
| `ctrl+\``, `echo hello; printf ...`, return | Same prompt, colors, cursor and line positions; the port's view is an editor tab, the reference's is the panel |
| `ls -G /` | Same ANSI colors (directories blue, symlinks magenta), same rows |
| `cmd+\` (split) | Both panes reflow to half width, tabs list shows `┌`/`└` split rows, the new pane takes focus (block cursor), the old one shows the outline cursor |
| `ctrl+shift+\`` (new) | Third terminal in its own group, active row highlighted with `terminal.tab.activeBorder` |

Differences left: the shell integration gutter decoration (item 2, shell integration), the "..." view menu, the `/private/var` path of the workspace folder (port/notes.md, "Milestone 15: integrated terminal").

`cargo test -p app`: 1043 passed, 1 ignored (baseline 1034); the terminal tests run a shell in a PTY (`terminal/tests.rs`).
