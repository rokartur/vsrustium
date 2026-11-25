//! markNavigationAddon.ts `scrollToPreviousMark` and `scrollToNextMark` with `ScrollPosition.Top`: the prompt lines
//! of the finished commands (and of the running one), the viewport top moved to the one before or after it.

use alacritty_terminal::grid::{Dimensions, Scroll};

use super::TerminalInstance;

impl TerminalInstance {
    /// `_getMarkers(skipEmptyCommands: true)`: commands with an exit code, then the executed current command.
    fn command_marks(&self) -> Vec<i64> {
        let mut marks: Vec<i64> = self.commands.iter().filter(|command| command.exit_code.is_some()).filter_map(|command| command.prompt_line.or(command.command_line)).collect();
        if let (Some(prompt), Some(_)) = (self.current.prompt_line, self.current.executed_line) {
            marks.push(prompt);
        }
        marks
    }

    /// The commands the gutter decorates: each finished one (its index in `commands`) at its prompt line with its
    /// exit code, then the prompt being entered or run, without either.
    pub fn decorated_commands(&self) -> Vec<(Option<usize>, i64, Option<i32>)> {
        let finished = self.commands.iter().enumerate().filter_map(|(index, command)| Some((Some(index), command.prompt_line.or(command.command_line)?, command.exit_code)));
        let mut decorated: Vec<_> = finished.collect();
        decorated.extend(self.current.prompt_line.map(|line| (None, line, None)));
        decorated
    }

    /// The mark above the viewport top, else the top of the buffer.
    pub fn scroll_to_previous_mark(&mut self) {
        let viewport_y = self.viewport_y();
        match self.command_marks().into_iter().rev().find(|line| *line < viewport_y) {
            Some(line) => self.scroll_to_line(line),
            None => self.term.scroll_display(Scroll::Top),
        }
    }

    /// The mark below the viewport top, else the bottom.
    pub fn scroll_to_next_mark(&mut self) {
        let viewport_y = self.viewport_y();
        match self.command_marks().into_iter().find(|line| *line > viewport_y) {
            Some(line) => self.scroll_to_line(line),
            None => self.term.scroll_display(Scroll::Bottom),
        }
    }

    fn viewport_y(&self) -> i64 {
        self.term.history_size() as i64 - self.term.grid().display_offset() as i64
    }

    /// xterm's `scrollToLine`: the line at the viewport top, as far as the scrollback allows.
    fn scroll_to_line(&mut self, line: i64) {
        let target = line.clamp(0, self.term.history_size() as i64);
        self.term.scroll_display(Scroll::Delta((self.viewport_y() - target) as i32));
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::term::Config;

    use super::super::{ShellLaunchConfig, TerminalInstance, colors};

    #[test]
    fn previous_and_next_walk_the_finished_commands_prompts() {
        let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let colors = colors::TerminalColors::new(&crate::theme::load_theme(&[themes], "Dark Modern").unwrap());
        let launch = ShellLaunchConfig { extension_pty: true, cwd: std::env::temp_dir(), ..Default::default() };
        let (mut instance, _) = TerminalInstance::new(1, launch, 40, 5, Config { scrolling_history: 1000, ..Config::default() });
        let command = |name: &str| format!("\x1b]633;A\x07$ \x1b]633;B\x07{name}\r\n\x1b]633;C\x07{}\x1b]633;D;0\x07", "out\r\n".repeat(9));
        // Prompts on lines 0 and 10, the third prompt (line 20) not run yet; 21 lines, 16 of them scrollback.
        instance.process(format!("{}{}\x1b]633;A\x07$ ", command("a"), command("b")).as_bytes(), &colors);
        let viewport = |instance: &TerminalInstance| instance.viewport_y();
        assert_eq!(viewport(&instance), 16);
        instance.scroll_to_previous_mark();
        assert_eq!(viewport(&instance), 10);
        instance.scroll_to_previous_mark();
        assert_eq!(viewport(&instance), 0);
        instance.scroll_to_previous_mark();
        assert_eq!(viewport(&instance), 0);
        instance.scroll_to_next_mark();
        assert_eq!(viewport(&instance), 10);
        instance.scroll_to_next_mark();
        assert_eq!(viewport(&instance), 16, "past the last mark: the bottom");
        assert_eq!(instance.decorated_commands(), [(Some(0), 0, Some(0)), (Some(1), 10, Some(0)), (None, 20, None)]);
    }
}
