//! markNavigationAddon.ts `scrollToPreviousMark` and `scrollToNextMark`: the prompt lines of the finished commands
//! (and of the running one), walked from the last one navigated to and revealed with a temporary highlight.

use std::time::{Duration, Instant};

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

    /// `getCommandForLine` when it finds a full command: the rows of its command guide, from its prompt to the row
    /// before its end (`showCommandGuide`), none past 200 rows.
    pub fn command_guide_rows(&self, line: i64) -> Option<std::ops::Range<i64>> {
        if self.current.prompt_line.is_some_and(|prompt| line >= prompt) {
            return None;
        }
        let command = self.commands.iter().rev().find(|command| command.prompt_line.or(command.command_line).is_some_and(|start| start <= line))?;
        let start = command.prompt_line.or(command.command_line)?;
        let end = command.end_line.filter(|_| command.executed_line.is_some())?;
        (end - start <= 200).then_some(start..end)
    }

    /// `scrollToPreviousMark(ScrollPosition.Middle)`: from the last mark navigated to, or from the viewport once the
    /// user scrolled away from it; past the first mark the buffer's top.
    pub fn scroll_to_previous_mark(&mut self) {
        let marks = self.command_marks();
        let index = match (self.scrolled_from_current_mark(), self.current_mark) {
            (true, _) => marks.iter().filter(|line| **line < self.viewport_y()).count() as i64 - 1,
            (false, CurrentMark::Bottom) => marks.len() as i64 - 1,
            (false, CurrentMark::Top) => -1,
            (false, CurrentMark::Line(current)) => marks.iter().filter(|line| **line < current).count() as i64 - 1,
        };
        match usize::try_from(index) {
            Ok(index) => self.scroll_to_command(marks[index]),
            Err(_) => {
                self.current_mark = CurrentMark::Top;
                self.term.scroll_display(Scroll::Top);
                self.navigation_highlight = None;
            }
        }
    }

    /// `scrollToNextMark(ScrollPosition.Middle)`; past the last mark the buffer's bottom.
    pub fn scroll_to_next_mark(&mut self) {
        let marks = self.command_marks();
        let index = match (self.scrolled_from_current_mark(), self.current_mark) {
            (true, _) => marks.iter().filter(|line| **line <= self.viewport_y()).count(),
            (false, CurrentMark::Bottom) => marks.len(),
            (false, CurrentMark::Top) => 0,
            (false, CurrentMark::Line(current)) => marks.iter().filter(|line| **line <= current).count(),
        };
        match marks.get(index) {
            Some(&line) => self.scroll_to_command(line),
            None => {
                self.current_mark = CurrentMark::Bottom;
                self.term.scroll_display(Scroll::Bottom);
                self.navigation_highlight = None;
            }
        }
    }

    /// The user scrolled since the last navigation: the current mark is out of view, or the viewport left the
    /// boundary it was at.
    fn scrolled_from_current_mark(&self) -> bool {
        match self.current_mark {
            CurrentMark::Line(line) => !self.line_in_viewport(line),
            CurrentMark::Top => self.viewport_y() != 0,
            CurrentMark::Bottom => self.viewport_y() != self.term.history_size() as i64,
        }
    }

    /// `_scrollToCommand`: the command at the mark revealed with its prompt and command lines, else the mark's line.
    fn scroll_to_command(&mut self, mark: i64) {
        self.current_mark = CurrentMark::Line(mark);
        let command = self.commands.iter().find(|command| command.command_line == Some(mark) || command.prompt_line == Some(mark)).unwrap_or(&self.current);
        match command.command_line {
            Some(start_line) => {
                let start = start_line - (self.prompt_row_count(command, start_line) - 1);
                let end = start_line + super::sticky_scroll::command_row_count(command, start_line) - 1;
                self.reveal_rows(start, end);
            }
            None => self.reveal_rows(mark, mark),
        }
    }

    /// `_scrollToMarker(start, ScrollPosition.Middle, end)`: a start out of view goes a quarter of the rows below
    /// the viewport top (`getTargetScrollLine`), then `registerTemporaryDecoration` marks the rows.
    pub fn reveal_rows(&mut self, start: i64, end: i64) {
        if !self.line_in_viewport(start) {
            let target = (start - self.term.screen_lines() as i64 / 4).clamp(0, self.term.history_size() as i64);
            self.term.scroll_display(Scroll::Delta((self.viewport_y() - target) as i32));
        }
        self.navigation_highlight = Some(NavigationHighlight { rows: start..=end, outline_until: Instant::now() + OUTLINE_DURATION });
    }

    fn line_in_viewport(&self, line: i64) -> bool {
        let viewport_y = self.viewport_y();
        (viewport_y..viewport_y + self.term.screen_lines() as i64).contains(&line)
    }

    pub fn viewport_y(&self) -> i64 {
        self.term.history_size() as i64 - self.term.grid().display_offset() as i64
    }
}

/// markNavigationAddon.ts `Boundary` or the marker navigated to last; typing goes back to the bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CurrentMark {
    Top,
    Bottom,
    Line(i64),
}

/// `registerTemporaryDecoration` with `showOutline`: the revealed rows keep the `.terminal-scroll-highlight` bar
/// until the next navigation, their outline only until `outline_until`.
#[derive(Clone, Debug, PartialEq)]
pub struct NavigationHighlight {
    pub rows: std::ops::RangeInclusive<i64>,
    pub outline_until: Instant,
}

/// "Number picked to align with symbol highlight in the editor".
pub const OUTLINE_DURATION: Duration = Duration::from_millis(350);

#[cfg(test)]
mod tests {
    use alacritty_terminal::term::Config;

    use super::super::{ShellLaunchConfig, TerminalInstance, colors};

    #[test]
    fn previous_and_next_walk_the_finished_commands_prompts() {
        let app_root = std::path::Path::new(env!("VSCODE_SRC")).to_path_buf();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let colors = colors::TerminalColors::new(&crate::theme::load_theme(&[themes], "Dark Modern").unwrap());
        let launch = ShellLaunchConfig { extension_pty: true, cwd: std::env::temp_dir(), ..Default::default() };
        let (mut instance, _) = TerminalInstance::new(1, launch, 40, 5, Config { scrolling_history: 1000, ..Config::default() });
        let command = |name: &str| format!("\x1b]633;A\x07$ \x1b]633;B\x07{name}\r\n\x1b]633;C\x07{}\x1b]633;D;0\x07", "out\r\n".repeat(9));
        // Prompts on lines 0 and 10, the third prompt (line 20) not run yet; 21 lines, 16 of them scrollback.
        instance.process(format!("{}{}\x1b]633;A\x07$ ", command("a"), command("b")).as_bytes(), &colors);
        let viewport = |instance: &TerminalInstance| instance.viewport_y();
        assert_eq!(viewport(&instance), 16);
        let highlighted = |instance: &TerminalInstance| instance.navigation_highlight.as_ref().map(|highlight| highlight.rows.clone());
        // `ScrollPosition.Middle`: a quarter of the 5 rows (1) above the revealed prompt.
        instance.scroll_to_previous_mark();
        assert_eq!((viewport(&instance), highlighted(&instance)), (9, Some(10..=10)));
        instance.scroll_to_previous_mark();
        assert_eq!((viewport(&instance), highlighted(&instance)), (0, Some(0..=0)));
        instance.scroll_to_previous_mark();
        assert_eq!((viewport(&instance), highlighted(&instance)), (0, None), "past the first mark: the top");
        instance.scroll_to_next_mark();
        assert_eq!((viewport(&instance), highlighted(&instance)), (0, Some(0..=0)), "a mark in view is highlighted without a scroll");
        instance.scroll_to_next_mark();
        assert_eq!(viewport(&instance), 9);
        instance.scroll_to_next_mark();
        assert_eq!((viewport(&instance), highlighted(&instance)), (16, None), "past the last mark: the bottom");
        instance.term.scroll_display(alacritty_terminal::grid::Scroll::Delta(5));
        instance.scroll_to_previous_mark();
        assert_eq!(viewport(&instance), 9, "scrolled away: counted from the viewport top (11)");
        instance.input("x");
        instance.scroll_to_previous_mark();
        assert_eq!(highlighted(&instance), Some(10..=10), "typing goes back to the bottom");
        assert_eq!(instance.decorated_commands(), [(Some(0), 0, Some(0)), (Some(1), 10, Some(0)), (None, 20, None)]);
        assert_eq!(instance.command_guide_rows(4), Some(0..10));
        assert_eq!(instance.command_guide_rows(10), Some(10..20));
        assert_eq!(instance.command_guide_rows(20), None, "the prompt waiting for a command is no full command");
    }
}
