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

Fixed since: ui-pair `/tmp/m15-steps-grunt.json` (fresh profile, `/tmp/m15-tr`) lists `grunt: default` and `grunt: hello` on the first second-level pick; a log line in `provide_tasks` showed grunt's `$registerTaskProvider` reaching the workbench before `$provideTasks`.

## OSC 99, decorations, overview ruler, Copy as HTML, Record Terminal Session

ui-pair on the Studio, release build of the working tree after 8c5e44335d4, folder `/tmp/m15-tt` with three bash scripts (`osc`: `printf '\e]99;i=1:a=report;Hello\e\\'`, `fix`: prints `git push --set-upstream origin feat` and exits 1, `hi`: `echo hi`), sourced by name because `cua-driver` cannot type punctuation. Steps `/tmp/m15-steps-term.json`.

| Check | Port |
|---|---|
| OSC 99 `Hello` toast with the info icon | as the reference |
| gutter decorations: success, error, the running prompt's empty circle | as the reference |
| overview ruler: blue marks left, the failed command's red mark right, the border | as the reference |
| Terminal: Select All, then Copy as HTML: text, `<pre><div style=...>` rows padded to the columns | as the reference once the font fallbacks (`, monospace, AppleBraille`) were added |
| Record Terminal Session: a new terminal, "Recording terminal session..." with a spinner in the status bar, JSON of resize, output and `promptInputChange` events on the clipboard | as the reference; the port's session ended (5 s idle) before the typed `source hi` arrived, the reference's after it: the shell start timing differs |
| Focus Accessible Terminal View with a selection; quick fix for `source fix` (the command line is not `git push`) | nothing opens in either |

Differences: the port kept the OSC 99 toast past the reference's purge (the reference had hidden it by the next shot); the reference showed the initial hint at a new terminal's empty prompt, the port did not; added since.

## Initial hint

ui-pair `/tmp/m15-steps-hint.json` (folder `/tmp/m15-tt`): Ctrl+` opens a bash terminal, then `e` is typed. Both show "Show suggestions" with the ⌃ and Space key caps and "Don't show this again" below it (the compact layout of a pane under 500px) one cell after the cursor, and both drop the hint at the first key.
