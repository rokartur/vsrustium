//! terminalStickyScrollOverlay.ts `_refreshNow` and `_updateContent`: the prompt and command lines of the command
//! at the top of the viewport, shown over it while its output scrolls by.

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::TermMode;

use super::{FinishedCommand, TerminalInstance};

/// `Constants.StickyScrollPercentageCap`.
const PERCENTAGE_CAP: f32 = 0.4;

#[derive(Debug, PartialEq)]
pub struct StickyScroll {
    /// The first buffer line the overlay shows, and how many.
    pub start: i64,
    pub count: usize,
    /// More lines than shown: the last row ends in ` …`.
    pub truncated: bool,
    /// Rows the overlay is moved up by as the command's last output line scrolls under it.
    pub shift: usize,
    /// The command's first prompt line, where a click on the overlay scrolls to (`revealCommand`).
    pub reveal_line: i64,
}

impl TerminalInstance {
    /// `None` hides the overlay: the alternate buffer, no command at the viewport top, an ignored command, the
    /// viewport at or above the command's first line, or a pager (`:` or `(END)` on the last row) still running.
    pub fn sticky_scroll(&self, max_line_count: usize, ignored_commands: &[String]) -> Option<StickyScroll> {
        if self.term.mode().contains(TermMode::ALT_SCREEN) {
            return None;
        }
        let base_y = self.term.history_size() as i64;
        let viewport_y = base_y - self.term.grid().display_offset() as i64;
        let partial = self.current.prompt_line.is_some_and(|line| viewport_y >= line);
        let command = if partial { &self.current } else { self.command_for_line(viewport_y)? };
        let trimmed = command.command.trim().to_lowercase();
        if !trimmed.is_empty() && ignored_commands.iter().any(|ignored| ignored.to_lowercase() == trimmed) {
            return None;
        }
        let start_line = command.command_line?;
        if partial && command.executed_line.is_none() {
            return None;
        }

        let prompt_rows = self.prompt_row_count(command, start_line);
        let command_rows = command_row_count(command, start_line);
        let sticky_start = start_line - (prompt_rows - 1);
        let row_offset = match command.end_line {
            Some(end) if !partial => (viewport_y - end + 1).max(0),
            _ => 0,
        };
        let rows = self.term.screen_lines();
        let max_line_count = (max_line_count as i64).min((rows as f32 * PERCENTAGE_CAP).floor() as i64);
        let count = (prompt_rows + command_rows - 1).min(max_line_count) - row_offset;
        let truncated = count < prompt_rows + command_rows - 1;
        if viewport_y <= sticky_start || count <= 0 {
            return None;
        }

        let cursor = self.term.grid().cursor.point;
        if partial && viewport_y == base_y && cursor.line.0 == rows as i32 - 1 {
            let last_row = self.row_text(base_y + rows as i64 - 1);
            if (cursor.column.0 == 1 && last_row.starts_with(':')) || (cursor.column.0 == 5 && last_row.starts_with("(END)")) {
                return None;
            }
        }
        let start = sticky_start + row_offset;
        if partial && (start..start + count).all(|line| self.row_text(line).is_empty()) {
            return None;
        }

        let mut shift = 0;
        if !partial && let Some(end) = command.end_line {
            let last_line = end.min(base_y + cursor.line.0 as i64);
            shift = (viewport_y + count - last_line).max(0) as usize;
        }
        Some(StickyScroll { start, count: count as usize, truncated, shift, reveal_line: sticky_start })
    }

    /// `getCommandForLine` for the finished commands: the last one whose prompt (or command start) is at or above it.
    fn command_for_line(&self, line: i64) -> Option<&FinishedCommand> {
        self.commands.iter().rev().find(|command| command.prompt_line.or(command.command_line).is_some_and(|start| start <= line))
    }

    /// `getPromptRowCount`: the prompt's rows up to the command start, leading blank lines left out.
    fn prompt_row_count(&self, command: &FinishedCommand, start_line: i64) -> i64 {
        let Some(mut prompt_line) = command.prompt_line else { return 1 };
        while prompt_line < start_line && self.row_text(prompt_line).is_empty() {
            prompt_line += 1;
        }
        start_line - prompt_line + 1
    }

    /// A buffer line's text, trailing blanks dropped (`translateToString(true)`).
    pub(crate) fn row_text(&self, line: i64) -> String {
        let history = self.term.history_size() as i64;
        let line = line - history;
        if line < -history || line >= self.term.screen_lines() as i64 {
            return String::new();
        }
        let row = &self.term.grid()[Line(line as i32)];
        let text: String = (0..self.term.columns()).map(|column| row[Column(column)].c).collect();
        text.trim_end_matches([' ', '\0']).to_owned()
    }

    /// markNavigationAddon.ts `revealCommand` with `ScrollPosition.Middle`: the line a quarter of the rows below the
    /// viewport top (`getTargetScrollLine`), unless it is in view already.
    pub fn reveal_line(&mut self, line: i64) {
        let base_y = self.term.history_size() as i64;
        let rows = self.term.screen_lines() as i64;
        let viewport_y = base_y - self.term.grid().display_offset() as i64;
        if (viewport_y..viewport_y + rows).contains(&line) {
            return;
        }
        let target = (line - rows / 4).clamp(0, base_y);
        self.term.scroll_display(Scroll::Delta((viewport_y - target) as i32));
    }
}

/// `getCommandRowCount`: the rows from the command start to C, less the last when C is at its first column.
fn command_row_count(command: &FinishedCommand, start_line: i64) -> i64 {
    let Some(executed_line) = command.executed_line else { return 1 };
    let rows = executed_line.max(start_line) - start_line + 1;
    if command.executed_column == Some(0) { rows - 1 } else { rows }
}

#[cfg(test)]
mod tests;
