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

## Tasks, find, context menu

ui-pair `/tmp/term/steps5.json` on the Studio at b9d74655619 (release build): Command Palette "tasks run task", the Run Task picker, the `npm` second level, Enter, Cmd+F "build", right click, Run Task > Show All Tasks.

| step | port |
|------|------|
| Run Task picker | Configure a Task under "configured", `npm` under "contributed", Show All Tasks... |
| npm level, Enter | `npm: build` runs in a terminal named `build`: "Executing task: npm run build", `> workspace@0.0.1 build`, `build done`, "Terminal will be reused by tasks, press any key to close it." |
| Cmd+F "build" | the find widget at the top right with the Aa, ab and .* toggles, "4 of 4", previous, next and close |
| right click | New Terminal, Split Terminal / Copy, Paste, Select All / Clear / Kill Terminal with their keybindings |
| Show All Tasks... | the grunt, gulp and jake "task detection is turned off" entries (their `autoDetect` default is off) |

The reference did not follow: its Command Palette ranked "Tasks: Rerun All Running Tasks" (recently used in the reference profile) first for every typed query, so its shots show "No running tasks to restart"; `cua-driver` cannot type the `:` of "Tasks: Run Task". The task picker look is compared by eye with the quick pick of earlier pairs, not pixel by pixel.
