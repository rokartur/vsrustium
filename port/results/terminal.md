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

## grunt, gulp, jake and the external terminal

ui-pair on the Studio at 142fdae6aa3 (release build), fixture `port/fixtures/task-runners` copied to `/tmp/m15-tr` with `npm install`, settings `grunt.autoDetect`, `gulp.autoDetect`, `jake.autoDetect` on and `terminal.external.osxExec` set to a probe app (`/tmp/Probe.app`, writes `/tmp/m15-probe-launched` when it starts). Steps `/tmp/steps-gj.json`: Run Task, `grunt`, Enter, Enter, Enter (Continue without scanning); the same for `jake`; Cmd+Shift+E, Cmd+Shift+C. An earlier pair (`/tmp/m15/steps3`) ran `gulp: default` the same way.

| Check | Port |
|---|---|
| `gulp`, `grunt`, `jake` under "contributed", Configure a Task with its `$(plus)` icon | as the reference |
| second level: `gulp: default`, `gulp: hello`; `jake: hello`; Go back | as the reference |
| the problem matcher prompt (placeholder, the four choices, "associate", the matchers sorted by label) | as the reference (jake) |
| the task runs in a task terminal: `node_modules/.bin/gulp default`, `grunt default`, `jake hello` with their output and "Terminal will be reused by tasks" | as the reference |
| Cmd+Shift+C starts `terminal.external.osxExec` (`open -a /tmp/Probe.app /tmp/m15-tr`) | probe file written |

Difference: the first second-level fetch of a session can come back empty in the port ("No grunt tasks found. Go back"), where the reference lists the tasks; the next fetch has them (Configure Task listed `grunt: default` right after). See port/notes.md milestone 15 handoff.
