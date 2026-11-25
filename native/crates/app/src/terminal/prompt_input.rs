//! promptInputModel.ts: the command line the user is typing after the prompt, read back from the terminal buffer.
//! Indexes count chars (Unicode scalar values) of the value, where upstream counts UTF-16 code units.
// Nothing calls the model yet; the terminal instance and terminal-suggest will.
#![allow(dead_code)]

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Grid, Row};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::Color;

use super::shell_integration::BufferPoint;

/// `PromptInputState`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PromptInputState {
    #[default]
    Unknown,
    Input,
    Execute,
}

/// `PromptInputModel`: a best guess of the prompt input from the shell integration sequences and the buffer.
#[derive(Debug)]
pub struct PromptInputModel {
    state: PromptInputState,
    /// `_commandStartMarker.line`, an absolute buffer line.
    command_start_line: Option<i64>,
    command_start_x: usize,
    last_prompt_line: Option<String>,
    continuation_prompt: Option<String>,
    is_fish: bool,
    last_user_input: String,
    value: String,
    cursor_index: Option<usize>,
    ghost_text_index: Option<usize>,
}

impl Default for PromptInputModel {
    fn default() -> Self {
        PromptInputModel {
            state: PromptInputState::Unknown,
            command_start_line: None,
            command_start_x: 0,
            last_prompt_line: None,
            continuation_prompt: None,
            is_fish: false,
            last_user_input: String::new(),
            value: String::new(),
            cursor_index: Some(0),
            ghost_text_index: None,
        }
    }
}

impl PromptInputModel {
    pub fn state(&self) -> PromptInputState {
        self.state
    }

    /// `value`: the full prompt input including ghost text.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// `prefix`: the input up to the cursor, never ghost text.
    pub fn prefix(&self) -> &str {
        substring(&self.value, 0, self.cursor_index.unwrap_or(0))
    }

    /// `suffix`: the input from the cursor to the ghost text or the end.
    pub fn suffix(&self) -> &str {
        let end = self.ghost_text_index.unwrap_or(usize::MAX);
        substring(&self.value, self.cursor_index.unwrap_or(0), end)
    }

    /// `cursorIndex`; `None` where upstream has -1 (executed, or a confident command line).
    pub fn cursor_index(&self) -> Option<usize> {
        self.cursor_index
    }

    /// `ghostTextIndex`; `None` where upstream has -1.
    pub fn ghost_text_index(&self) -> Option<usize> {
        self.ghost_text_index
    }

    /// promptInputModel.ts `setShellType`: fish is the only shell type the model reads differently.
    pub fn set_is_fish(&mut self, is_fish: bool) {
        self.is_fish = is_fish;
    }

    /// promptInputModel.ts `setContinuationPrompt`. Returns whether the input changed.
    pub fn set_continuation_prompt<T: EventListener>(&mut self, term: &Term<T>, value: &str) -> bool {
        self.continuation_prompt = Some(value.to_owned());
        self.sync(term)
    }

    /// promptInputModel.ts `setLastPromptLine`. Returns whether the input changed.
    pub fn set_last_prompt_line<T: EventListener>(&mut self, term: &Term<T>, value: &str) -> bool {
        self.last_prompt_line = Some(value.to_owned());
        self.sync(term)
    }

    /// promptInputModel.ts `setConfidentCommandLine`. Returns whether the input changed.
    pub fn set_confident_command_line(&mut self, value: &str) -> bool {
        if self.value == value {
            return false;
        }
        self.value = value.to_owned();
        self.cursor_index = None;
        self.ghost_text_index = None;
        true
    }

    /// promptInputModel.ts `getCombinedString`: `|` marks the cursor and `[` `]` wrap the ghost text.
    pub fn get_combined_string(&self, empty_string_when_empty: bool) -> String {
        let value = self.value.replace('\n', "\u{23CE}");
        let Some(cursor) = self.cursor_index else {
            return value;
        };
        let mut result = format!("{}|", substring(&value, 0, cursor));
        match self.ghost_text_index {
            Some(ghost) => {
                result.push_str(substring(&value, cursor, ghost));
                result.push_str(&format!("[{}]", substring(&value, ghost, usize::MAX)));
            }
            None => result.push_str(substring(&value, cursor, usize::MAX)),
        }
        if result == "|" && empty_string_when_empty {
            return String::new();
        }
        result
    }

    /// promptInputModel.ts `_handleUserInput`: the data the user last sent to the shell.
    pub fn user_input(&mut self, data: &str) {
        self.last_user_input = data.to_owned();
    }

    /// promptInputModel.ts `_handleCommandStart`, with `start` where the command line begins (the B sequence's
    /// cursor). Returns whether input started (`onDidStartInput`).
    pub fn command_start<T: EventListener>(&mut self, term: &Term<T>, start: BufferPoint) -> bool {
        if self.state == PromptInputState::Input {
            return false;
        }
        self.state = PromptInputState::Input;
        self.command_start_line = Some(start.line);
        self.command_start_x = start.column;
        self.value.clear();
        self.cursor_index = Some(0);

        let Some(prompt) = self.last_prompt_line.as_deref().filter(|prompt| !prompt.is_empty()) else {
            return true;
        };
        let prompt_length = prompt.chars().count();
        if self.command_start_x != prompt_length && Buffer::new(term).text(start.line, true, 0, None).starts_with(prompt) {
            self.command_start_x = prompt_length;
            self.sync(term);
        }
        true
    }

    /// promptInputModel.ts `_handleCommandStartChanged`: the command now starts at the cursor's column.
    pub fn command_start_changed<T: EventListener>(&mut self, term: &Term<T>) {
        if self.state != PromptInputState::Input {
            return;
        }
        self.command_start_x = Buffer::new(term).cursor_x;
        self.sync(term);
    }

    /// promptInputModel.ts `_handleCommandExecuted`. Returns true when the last user input was ^C
    /// (`onDidInterrupt`, fired before `onDidFinishInput`).
    pub fn command_executed(&mut self) -> bool {
        if self.state == PromptInputState::Execute {
            return false;
        }
        self.cursor_index = None;
        if let Some(ghost) = self.ghost_text_index.take() {
            self.value = substring(&self.value, 0, ghost).to_owned();
        }
        let interrupted = self.last_user_input == "\u{3}";
        if interrupted {
            self.last_user_input.clear();
        }
        self.state = PromptInputState::Execute;
        interrupted
    }

    /// promptInputModel.ts `_handleCommandFinished`: an empty value keeps `runCommand` from sending ^C.
    pub fn command_finished(&mut self) {
        self.value.clear();
    }

    /// promptInputModel.ts `_sync`: reads the input from the command start to the cursor and below it. Call it
    /// after each parsed write, cursor move and user input. Returns whether the input changed (`onDidChangeInput`).
    pub fn sync<T: EventListener>(&mut self, term: &Term<T>) -> bool {
        if self.state != PromptInputState::Input {
            return false;
        }
        let Some(mut start_y) = self.command_start_line else {
            return false;
        };
        let buffer = Buffer::new(term);
        let mut cursor_index = None;
        let mut command_line = buffer.text(start_y, true, self.command_start_x, None);
        if self.is_fish && command_line.is_empty() {
            start_y += 1;
            command_line = buffer.text(start_y, true, 0, None);
            cursor_index = Some(if buffer.cursor_y == start_y { buffer.cursor_x as i64 } else { length(command_line.trim_end()) });
        }
        if buffer.row(start_y).is_none() {
            return false;
        }

        let mut value = command_line.clone();
        let mut cursor_index = cursor_index.unwrap_or_else(|| {
            if buffer.cursor_y == start_y {
                buffer.relative_cursor_index(start_y, self.command_start_x).min(length(&command_line))
            } else {
                length(command_line.trim_end())
            }
        });

        for y in start_y + 1..=buffer.cursor_y {
            let line_text = buffer.text(y, true, 0, None);
            if line_text.is_empty() {
                continue;
            }
            let on_cursor_line = buffer.cursor_y == y;
            if buffer.is_wrapped(y) || (on_cursor_line && self.continuation_prompt.is_some() && !self.line_contains_continuation_prompt(&line_text)) {
                value.push_str(&line_text);
                cursor_index += if on_cursor_line { buffer.relative_cursor_index(y, 0) } else { length(&line_text) };
            } else if self.is_fish {
                if value.ends_with('\\') {
                    value.pop();
                    value.push_str(line_text.trim());
                    cursor_index += length(line_text.trim()) - 1;
                } else if line_text.starts_with("      ") {
                    value.push('\n');
                    value.push_str(line_text.trim());
                    cursor_index += length(line_text.trim()) + 1;
                } else {
                    value.push_str(&line_text);
                    cursor_index += length(&line_text);
                }
            } else if self.continuation_prompt.is_none() || self.line_contains_continuation_prompt(&line_text) {
                let trimmed = self.trim_continuation_prompt(&line_text);
                value.push('\n');
                value.push_str(trimmed);
                cursor_index += if on_cursor_line {
                    let prompt_width = self.continuation_prompt_cell_width(&buffer, y, &line_text);
                    buffer.relative_cursor_index(y, prompt_width) + 1
                } else {
                    length(trimmed) + 1
                };
            }
        }

        for y in buffer.cursor_y + 1..buffer.history + buffer.rows {
            let line_text = buffer.text(y, true, 0, None);
            if line_text.is_empty() {
                break;
            }
            if self.is_fish {
                value.push_str(&line_text);
            } else if self.continuation_prompt.is_none() || self.line_contains_continuation_prompt(&line_text) {
                value.push('\n');
                value.push_str(self.trim_continuation_prompt(&line_text));
            } else {
                value.push_str(&line_text);
            }
        }

        let mut value = self.adjust_trailing_whitespace(value, cursor_index);
        let ghost_text_index = self.scan_for_ghost_text(&buffer, start_y, cursor_index, &mut value);

        let cursor_index = usize::try_from(cursor_index).ok();
        let ghost_text_index = usize::try_from(ghost_text_index).ok();
        if self.value == value && self.cursor_index == cursor_index && self.ghost_text_index == ghost_text_index {
            return false;
        }
        self.value = value;
        self.cursor_index = cursor_index;
        self.ghost_text_index = ghost_text_index;
        true
    }

    /// promptInputModel.ts `_sync`, "Adjust trailing whitespace": trimmed cells cannot tell typed spaces apart, so
    /// the trailing spaces come from the previous value, the cursor and the last user input.
    fn adjust_trailing_whitespace(&mut self, value: String, cursor_index: i64) -> String {
        let previous_cursor = self.cursor_index.map_or(-1, |index| index as i64);
        let previous_length = length(&self.value);
        let previous_trimmed_length = length(self.value.trim_end());
        let trimmed_length = length(value.trim_end());
        let mut trailing_whitespace = previous_length - previous_trimmed_length;

        if self.last_user_input == "\x7f" {
            self.last_user_input.clear();
            if cursor_index == previous_cursor - 1 {
                trailing_whitespace = if previous_trimmed_length > trimmed_length && trimmed_length <= cursor_index {
                    (previous_length - 1 - trimmed_length).max(0)
                } else {
                    (trailing_whitespace - 1).max(0)
                };
            }
        }
        if self.last_user_input == "\x1b[3~" {
            self.last_user_input.clear();
            if cursor_index == previous_cursor {
                trailing_whitespace = (trailing_whitespace - 1).max(0);
            }
        }

        let line_count = value.split('\n').count() as i64;
        if line_count == 1 {
            if trimmed_length < length(&value) {
                if self.last_user_input == " " {
                    self.last_user_input.clear();
                    if cursor_index > trimmed_length && cursor_index > previous_cursor {
                        trailing_whitespace += 1;
                    }
                }
                trailing_whitespace = (cursor_index - trimmed_length).max(trailing_whitespace).max(0);
            }
            let char_before_cursor = if cursor_index > 0 { value.chars().nth(cursor_index as usize - 1) } else { None };
            if trailing_whitespace > 0 && cursor_index == previous_cursor + 1 && !self.last_user_input.is_empty() && char_before_cursor != Some(' ') {
                trailing_whitespace = previous_length - previous_cursor;
            }
        } else {
            let prompt_length = self.continuation_prompt.as_deref().map_or(0, length);
            trailing_whitespace = (cursor_index - length(&value) - (line_count - 1) * prompt_length).max(0);
        }

        let lines: Vec<&str> = value.split('\n').map(str::trim_end).collect();
        lines.join("\n") + &" ".repeat(trailing_whitespace.max(0) as usize)
    }

    /// promptInputModel.ts `_scanForGhostText`: italic or dim text at or after the cursor, behind input that is
    /// neither, on the command start line. Reads the new value where upstream reads the previous one.
    fn scan_for_ghost_text(&self, buffer: &Buffer, y: i64, cursor_index: i64, value: &mut String) -> i64 {
        if value.trim().is_empty() {
            return -1;
        }
        let mut proceed_with_ghost_text_check = false;
        let mut x = buffer.cursor_x;
        while x > 0 {
            x -= 1;
            let Some(cell) = buffer.cell(y, x) else {
                break;
            };
            if !cell_text(cell).trim().is_empty() {
                proceed_with_ghost_text_check = !is_styled_like_ghost_text(cell);
                break;
            }
        }

        let mut ghost_text_index = -1;
        if proceed_with_ghost_text_check {
            let mut offset = 0;
            for x in buffer.cursor_x..buffer.content_end(y) {
                let Some(cell) = buffer.cell(y, x) else {
                    break;
                };
                if is_styled_like_ghost_text(cell) {
                    ghost_text_index = cursor_index + offset;
                    break;
                }
                offset += length(&cell_text(cell));
            }
        }
        if ghost_text_index == -1 {
            ghost_text_index = self.scan_for_ghost_text_advanced(buffer, y, cursor_index);
        }

        if ghost_text_index > -1 && substring(value, ghost_text_index as usize, usize::MAX).ends_with(' ') {
            *value = value.trim().to_owned();
            if substring(value, ghost_text_index as usize, usize::MAX).is_empty() {
                ghost_text_index = -1;
            }
        }
        ghost_text_index
    }

    /// promptInputModel.ts `_scanForGhostTextAdvanced`: ghost text may only differ in style from the input before
    /// it. Upstream's last check (no earlier cell shares the style) tests `getChars.length`, always 0, so it never
    /// runs and is not ported.
    fn scan_for_ghost_text_advanced(&self, buffer: &Buffer, y: i64, cursor_index: i64) -> i64 {
        let mut styles: Vec<(Style, Vec<usize>)> = Vec::new();
        let mut position = buffer.cursor_x;
        let mut last_non_whitespace_cell = buffer.cell(y, position);
        let mut next_cell = last_non_whitespace_cell;
        while let Some(cell) = next_cell {
            let style = Style::of(cell);
            match styles.iter_mut().find(|(known, _)| *known == style) {
                Some((_, positions)) => positions.push(position),
                None => styles.push((style, vec![position])),
            }
            position += 1;
            next_cell = buffer.cell(y, position);
            if next_cell.is_some_and(|cell| !cell_text(cell).trim().is_empty()) {
                last_non_whitespace_cell = next_cell;
            }
        }

        let Some(last_cell) = last_non_whitespace_cell.filter(|cell| !cell_text(cell).trim().is_empty()) else {
            return -1;
        };
        let last_style = Style::of(last_cell);
        if buffer.cell(y, self.command_start_x).is_some_and(|cell| Style::of(cell) == last_style) {
            return -1;
        }
        let Some((_, positions)) = styles.iter().find(|(style, _)| *style == last_style) else {
            return -1;
        };
        if positions[0] > buffer.cursor_x + 1 && self.is_position_right_prompt(buffer, y, positions[0]) {
            return -1;
        }
        if positions.windows(2).any(|pair| pair[1] != pair[0] + 1) {
            return -1;
        }
        let ghost_text_index = if Some(buffer.cursor_y) == self.command_start_line {
            positions[0] as i64 - self.command_start_x as i64
        } else {
            positions[0] as i64
        };
        if ghost_text_index >= cursor_index { ghost_text_index } else { -1 }
    }

    /// promptInputModel.ts `_isPositionRightPrompt`: 5+ blank cells before `position` after the command start.
    fn is_position_right_prompt(&self, buffer: &Buffer, y: i64, position: usize) -> bool {
        let mut count = 0;
        for x in (self.command_start_x..position).rev() {
            let blank = buffer.cell(y, x).is_none_or(|cell| cell_text(cell).trim().is_empty());
            if !blank {
                count = 0;
                continue;
            }
            count += 1;
            if count >= 5 {
                return true;
            }
        }
        false
    }

    /// promptInputModel.ts `_trimContinuationPrompt`.
    fn trim_continuation_prompt<'a>(&self, line_text: &'a str) -> &'a str {
        match &self.continuation_prompt {
            Some(prompt) if self.line_contains_continuation_prompt(line_text) => substring(line_text, prompt.chars().count(), usize::MAX),
            _ => line_text,
        }
    }

    /// promptInputModel.ts `_lineContainsContinuationPrompt`.
    fn line_contains_continuation_prompt(&self, line_text: &str) -> bool {
        self.continuation_prompt.as_deref().is_some_and(|prompt| !prompt.is_empty() && line_text.starts_with(prompt.trim_end()))
    }

    /// promptInputModel.ts `_getContinuationPromptCellWidth`: the cells the continuation prompt takes on line `y`.
    fn continuation_prompt_cell_width(&self, buffer: &Buffer, y: i64, line_text: &str) -> usize {
        let Some(prompt) = &self.continuation_prompt else {
            return 0;
        };
        if !line_text.starts_with(prompt.trim_end()) {
            return 0;
        }
        let mut text = String::new();
        let mut x = 0;
        while text != *prompt {
            let cell = buffer.cell(y, x);
            x += 1;
            let Some(cell) = cell else {
                break;
            };
            text.push_str(&cell_text(cell));
        }
        x
    }
}

/// The xterm.js `IBuffer` reads the model makes, over absolute lines (scrollback line 0 is 0, as `BufferPoint`).
struct Buffer<'a> {
    grid: &'a Grid<Cell>,
    history: i64,
    rows: i64,
    cursor_y: i64,
    /// xterm.js `cursorX`, which reaches the column count while a wrap is pending.
    cursor_x: usize,
}

impl<'a> Buffer<'a> {
    fn new<T>(term: &'a Term<T>) -> Self {
        let grid = term.grid();
        let history = grid.history_size() as i64;
        let cursor = &grid.cursor;
        Buffer {
            grid,
            history,
            rows: grid.screen_lines() as i64,
            cursor_y: history + cursor.point.line.0 as i64,
            cursor_x: cursor.point.column.0 + cursor.input_needs_wrap as usize,
        }
    }

    /// `buffer.getLine`.
    fn row(&self, y: i64) -> Option<&'a Row<Cell>> {
        let line = y - self.history;
        if y < 0 || line >= self.rows {
            return None;
        }
        Some(&self.grid[Line(line as i32)])
    }

    /// `getLine(y).getCell(x)`.
    fn cell(&self, y: i64, x: usize) -> Option<&'a Cell> {
        self.row(y).filter(|row| x < row.len()).map(|row| &row[Column(x)])
    }

    /// `IBufferLine.isWrapped`: alacritty marks the wrap on the last cell of the line before.
    fn is_wrapped(&self, y: i64) -> bool {
        self.row(y - 1).is_some_and(|row| row[Column(row.len() - 1)].flags.contains(Flags::WRAPLINE))
    }

    /// The cell after the last one holding a character. xterm.js tells an erased cell from a typed space, alacritty
    /// does not, so on the cursor's line the cells before the cursor count as typed: `ls |` keeps its space.
    fn content_end(&self, y: i64) -> usize {
        let Some(row) = self.row(y) else {
            return 0;
        };
        let last = (0..row.len()).rev().find(|&x| row[Column(x)].c != ' ' || row[Column(x)].zerowidth().is_some());
        let end = last.map_or(0, |x| x + if row[Column(x)].flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 });
        if y == self.cursor_y { end.max(self.cursor_x) } else { end }
    }

    /// `IBufferLine.translateToString(trimRight, startColumn, endColumn)`: a wide char covers two cells.
    fn text(&self, y: i64, trim_right: bool, start: usize, end: Option<usize>) -> String {
        let Some(row) = self.row(y) else {
            return String::new();
        };
        let mut end = end.unwrap_or(usize::MAX).min(row.len());
        if trim_right {
            end = end.min(self.content_end(y));
        }
        let mut text = String::new();
        let mut x = start;
        while x < end {
            let cell = &row[Column(x)];
            text.push(cell.c);
            text.extend(cell.zerowidth().unwrap_or_default());
            x += if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
        }
        text
    }

    /// promptInputModel.ts `_getRelativeCursorIndex`: the chars from `start_x` to the cursor.
    fn relative_cursor_index(&self, y: i64, start_x: usize) -> i64 {
        length(&self.text(y, false, start_x, Some(self.cursor_x)))
    }
}

/// `_getCellStyleAsString` / `_cellStylesMatch`: colors and the attribute flags; alacritty has no blink or overline.
#[derive(Clone, Copy, PartialEq)]
struct Style {
    fg: Color,
    bg: Color,
    flags: Flags,
}

impl Style {
    fn of(cell: &Cell) -> Self {
        let attributes = Flags::BOLD | Flags::ITALIC | Flags::DIM | Flags::ALL_UNDERLINES | Flags::INVERSE | Flags::HIDDEN | Flags::STRIKEOUT;
        Style { fg: cell.fg, bg: cell.bg, flags: cell.flags & attributes }
    }
}

/// `_isCellStyledLikeGhostText`.
fn is_styled_like_ghost_text(cell: &Cell) -> bool {
    cell.flags.intersects(Flags::ITALIC | Flags::DIM)
}

/// `IBufferCell.getChars`: the second cell of a wide char holds nothing.
fn cell_text(cell: &Cell) -> String {
    if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
        return String::new();
    }
    let mut text = cell.c.to_string();
    text.extend(cell.zerowidth().unwrap_or_default());
    text
}

/// JS `length`, in chars.
fn length(text: &str) -> i64 {
    text.chars().count() as i64
}

/// JS `substring(start, end)` in chars: both ends clamp to the text and swap when reversed.
fn substring(text: &str, start: usize, end: usize) -> &str {
    let byte = |index: usize| text.char_indices().nth(index).map_or(text.len(), |(byte, _)| byte);
    &text[byte(start.min(end))..byte(start.max(end))]
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;

    struct Session {
        term: Term<VoidListener>,
        parser: Processor,
        model: PromptInputModel,
    }

    impl Session {
        fn new() -> Self {
            Session::with_size(80, 24)
        }

        fn with_size(columns: usize, rows: usize) -> Self {
            let term = Term::new(Config::default(), &TermSize::new(columns, rows), VoidListener);
            Session { term, parser: Processor::new(), model: PromptInputModel::default() }
        }

        /// `writePromise`: xterm.js syncs the model after each parsed write.
        fn write(&mut self, data: &str) {
            self.parser.advance(&mut self.term, data.as_bytes());
            self.model.sync(&self.term);
        }

        /// `xterm.input(data, true)`.
        fn input(&mut self, data: &str) {
            self.model.sync(&self.term);
            self.model.user_input(data);
        }

        fn fire_command_start(&mut self) {
            let buffer = Buffer::new(&self.term);
            let start = BufferPoint { line: buffer.cursor_y, column: buffer.cursor_x };
            self.model.command_start(&self.term, start);
        }

        fn set_continuation_prompt(&mut self, prompt: &str) {
            self.model.set_continuation_prompt(&self.term, prompt);
        }

        /// `assertPromptInput`.
        #[track_caller]
        fn assert_prompt_input(&self, value_with_cursor: &str) {
            let model = &self.model;
            assert!(model.cursor_index().is_none() || value_with_cursor.contains('|'), "the expected value must contain |");
            assert_eq!(model.get_combined_string(false), value_with_cursor.replace('\n', "\u{23CE}"));
            let value: String = value_with_cursor.chars().filter(|char| !matches!(char, '|' | '[' | ']')).collect();
            assert_eq!(model.value(), value);
            let cursor_index = value_with_cursor.chars().position(|char| char == '|');
            assert_eq!(model.cursor_index(), cursor_index, "value={}", model.value());
            if let (Some(ghost), Some(cursor)) = (model.ghost_text_index(), cursor_index) {
                assert!(cursor <= ghost, "cursorIndex ({cursor}) must be before ghostTextIndex ({ghost})");
            }
        }
    }

    #[test]
    fn basic_input_and_execute() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.assert_prompt_input("|");

        session.write("foo bar");
        session.assert_prompt_input("foo bar|");

        session.write("\r\n");
        session.model.command_executed();
        session.assert_prompt_input("foo bar");

        session.write("(command output)\r\n$ ");
        session.fire_command_start();
        session.assert_prompt_input("|");
    }

    #[test]
    fn should_not_fire_on_did_change_input_events_when_nothing_changes() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.write("foo");
        session.assert_prompt_input("foo|");
        assert!(!session.model.sync(&session.term));
        session.write(" bar");
        session.assert_prompt_input("foo bar|");
        assert!(!session.model.sync(&session.term));
    }

    #[test]
    fn should_fire_on_did_interrupt_followed_by_on_did_finish_when_ctrl_c_is_pressed() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.write("foo");
        session.assert_prompt_input("foo|");

        session.input("\x03");
        session.write("^C");
        assert!(session.model.command_executed());
        assert_eq!(session.model.state(), PromptInputState::Execute);
    }

    #[test]
    fn should_clear_value_when_command_finishes() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.write("echo hello");
        session.assert_prompt_input("echo hello|");

        session.model.command_executed();
        assert_eq!(session.model.value(), "echo hello");
        session.model.command_finished();
        assert_eq!(session.model.value(), "");
    }

    #[test]
    fn cursor_navigation() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.assert_prompt_input("|");

        session.write("foo bar");
        session.assert_prompt_input("foo bar|");
        session.write("\x1b[3D");
        session.assert_prompt_input("foo |bar");
        session.write("\x1b[4D");
        session.assert_prompt_input("|foo bar");
        session.write("\x1b[3C");
        session.assert_prompt_input("foo| bar");
        session.write("\x1b[4C");
        session.assert_prompt_input("foo bar|");
        session.write("\x1b[D");
        session.assert_prompt_input("foo ba|r");
        session.write("\x1b[C");
        session.assert_prompt_input("foo bar|");
    }

    fn ghost_session(text: &str, expected: &str) {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.assert_prompt_input("|");
        session.write(text);
        session.assert_prompt_input(expected);
    }

    #[test]
    fn basic_ghost_text() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.write("foo\x1b[2m bar\x1b[0m\x1b[4D");
        session.assert_prompt_input("foo|[ bar]");
        session.write("\x1b[2D");
        session.assert_prompt_input("f|oo[ bar]");
    }

    #[test]
    fn ghost_text_trailing_whitespace() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.write("foo    ");
        session.write("\x1b[4D");
        session.assert_prompt_input("foo|    ");
    }

    #[test]
    fn basic_ghost_text_one_word() {
        ghost_session("pw\x1b[2md\x1b[1D", "pw|[d]");
    }

    #[test]
    fn ghost_text_with_cursor_navigation() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.write("foo\x1b[2m bar\x1b[0m\x1b[4D");
        session.assert_prompt_input("foo|[ bar]");
        session.write("\x1b[2D");
        session.assert_prompt_input("f|oo[ bar]");
        session.write("\x1b[C");
        session.assert_prompt_input("fo|o[ bar]");
        session.write("\x1b[C");
        session.assert_prompt_input("foo|[ bar]");
    }

    #[test]
    fn ghost_text_with_different_foreground_colors_only() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.write("foo\x1b[38;2;255;0;0m bar\x1b[0m\x1b[4D");
        session.assert_prompt_input("foo|[ bar]");
        session.write("\x1b[2D");
        session.assert_prompt_input("f|oo[ bar]");
    }

    #[test]
    fn no_ghost_text_when_foreground_color_matches_earlier_text() {
        ghost_session("\x1b[38;2;255;0;0mred1\x1b[0m \x1b[38;2;0;255;0mgreen\x1b[0m \x1b[38;2;255;0;0mred2\x1b[0m", "red1 green red2|");
    }

    #[test]
    fn ghost_text_detected_when_foreground_color_is_unique_at_the_end() {
        ghost_session("\x1b[38;2;255;0;0mcmd\x1b[0m \x1b[38;2;0;255;0marg\x1b[0m \x1b[38;2;0;0;255mfinal\x1b[5D", "cmd arg |[final]");
    }

    #[test]
    fn no_ghost_text_when_background_color_matches_earlier_text() {
        ghost_session("\x1b[48;2;255;0;0mred_bg1\x1b[0m \x1b[48;2;0;255;0mgreen_bg\x1b[0m \x1b[48;2;255;0;0mred_bg2\x1b[0m", "red_bg1 green_bg red_bg2|");
    }

    #[test]
    fn ghost_text_detected_when_background_color_is_unique_at_the_end() {
        ghost_session("\x1b[48;2;255;0;0mred_bg\x1b[0m \x1b[48;2;0;255;0mgreen_bg\x1b[0m \x1b[48;2;0;0;255mblue_bg\x1b[7D", "red_bg green_bg |[blue_bg]");
    }

    #[test]
    fn ghost_text_detected_when_bold_style_is_unique_at_the_end() {
        ghost_session("text \x1b[1mBOLD\x1b[4D", "text |[BOLD]");
    }

    #[test]
    fn no_ghost_text_when_earlier_text_has_the_same_bold_style() {
        ghost_session("\x1b[1mBOLD1\x1b[0m normal \x1b[1mBOLD2\x1b[0m", "BOLD1 normal BOLD2|");
    }

    #[test]
    fn ghost_text_detected_when_italic_style_is_unique_at_the_end() {
        ghost_session("text \x1b[3mITALIC\x1b[6D", "text |[ITALIC]");
    }

    #[test]
    fn no_ghost_text_when_earlier_text_has_the_same_italic_style() {
        ghost_session("\x1b[3mITALIC1\x1b[0m normal \x1b[3mITALIC2\x1b[0m", "ITALIC1 normal ITALIC2|");
    }

    #[test]
    fn ghost_text_detected_when_underline_style_is_unique_at_the_end() {
        ghost_session("text \x1b[4mUNDERLINE\x1b[9D", "text |[UNDERLINE]");
    }

    #[test]
    fn no_ghost_text_when_earlier_text_has_the_same_underline_style() {
        ghost_session("\x1b[4mUNDERLINE1\x1b[0m normal \x1b[4mUNDERLINE2\x1b[0m", "UNDERLINE1 normal UNDERLINE2|");
    }

    #[test]
    fn ghost_text_detected_when_strikethrough_style_is_unique_at_the_end() {
        ghost_session("text \x1b[9mSTRIKE\x1b[6D", "text |[STRIKE]");
    }

    #[test]
    fn no_ghost_text_when_earlier_text_has_the_same_strikethrough_style() {
        ghost_session("\x1b[9mSTRIKE1\x1b[0m normal \x1b[9mSTRIKE2\x1b[0m", "STRIKE1 normal STRIKE2|");
    }

    /// Upstream's last step moves the cursor 2 cells past the text; alacritty reads those as typed spaces.
    fn ghost_text_in_long_line(is_fish: bool) {
        let mut session = Session::new();
        session.model.set_is_fish(is_fish);
        session.write("$ ");
        session.fire_command_start();
        session.assert_prompt_input("|");
        session.write("find . -name");
        session.assert_prompt_input("find . -name|");
        session.write("\x1b[2m test\x1b[0m\x1b[4D");
        session.assert_prompt_input("find . -name |[test]");
        session.write("\x1b[C");
        session.assert_prompt_input("find . -name t|[est]");
        session.write("\x1b[C\x1b[C\x1b[C");
        session.assert_prompt_input("find . -name test|");
    }

    #[test]
    fn fish_ghost_text_in_long_line_with_wrapped_content() {
        ghost_text_in_long_line(true);
    }

    #[test]
    fn pwsh_ghost_text_in_long_line_with_wrapped_content() {
        ghost_text_in_long_line(false);
    }

    #[test]
    fn does_not_detect_right_prompt_as_ghost_text() {
        ghost_session(&format!("cmd{}\x1b[38;2;255;0;0mRP\x1b[0m\x1b[8D", " ".repeat(6)), &format!("cmd|{}RP", " ".repeat(6)));
    }

    #[test]
    fn wide_input_korean() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.assert_prompt_input("|");
        session.write("안영");
        session.assert_prompt_input("안영|");
        session.write("\r\n컴퓨터");
        session.assert_prompt_input("안영\n컴퓨터|");
        session.write("\r\n사람");
        session.assert_prompt_input("안영\n컴퓨터\n사람|");
        session.write("\x1b[G");
        session.assert_prompt_input("안영\n컴퓨터\n|사람");
        session.write("\x1b[A");
        session.assert_prompt_input("안영\n|컴퓨터\n사람");
        session.write("\x1b[4C");
        session.assert_prompt_input("안영\n컴퓨|터\n사람");
        session.write("\x1b[1;4H");
        session.assert_prompt_input("안|영\n컴퓨터\n사람");
        session.write("\x1b[D");
        session.assert_prompt_input("|안영\n컴퓨터\n사람");
    }

    /// The emoji on the second and third lines are 2 cells wide here and 1 in xterm.js's Unicode 6 tables, so the
    /// cursor moves 4 cells where upstream moves 2.
    #[test]
    fn emoji_input() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.assert_prompt_input("|");
        session.write("✌️👍");
        session.assert_prompt_input("✌️👍|");
        session.write("\r\n😎😕😅");
        session.assert_prompt_input("✌️👍\n😎😕😅|");
        session.write("\r\n🤔🤷😩");
        session.assert_prompt_input("✌️👍\n😎😕😅\n🤔🤷😩|");
        session.write("\x1b[G");
        session.assert_prompt_input("✌️👍\n😎😕😅\n|🤔🤷😩");
        session.write("\x1b[A");
        session.assert_prompt_input("✌️👍\n|😎😕😅\n🤔🤷😩");
        session.write("\x1b[4C");
        session.assert_prompt_input("✌️👍\n😎😕|😅\n🤔🤷😩");
        session.write("\x1b[1;4H");
        session.assert_prompt_input("✌️|👍\n😎😕😅\n🤔🤷😩");
        session.write("\x1b[D");
        session.assert_prompt_input("|✌️👍\n😎😕😅\n🤔🤷😩");
    }

    #[test]
    fn cursor_index_calculation_with_whitespace() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.write("echo   ");
        session.assert_prompt_input("echo   |");
        session.write("\x1b[3D");
        session.assert_prompt_input("echo|   ");
        session.write("\x1b[C");
        session.assert_prompt_input("echo |  ");
        session.write("\x1b[C");
        session.assert_prompt_input("echo  | ");
        session.write("\x1b[C");
        session.assert_prompt_input("echo   |");
    }

    #[test]
    fn whitespace_preservation_in_cursor_calculation() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.write("ls   -la");
        session.assert_prompt_input("ls   -la|");
        session.write("\x1b[3D");
        session.assert_prompt_input("ls   |-la");
        session.write("\x1b[3D");
        session.assert_prompt_input("ls|   -la");
        session.write("\x1b[2C");
        session.assert_prompt_input("ls  | -la");
    }

    #[test]
    fn delete_whitespace_with_backspace() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.assert_prompt_input("|");
        session.write(" ");
        session.assert_prompt_input(" |");

        let steps = [
            ("\x7f", "\x1b[D", "|"),
            ("    ", "    ", "    |"),
            ("\x1b[D\x1b[D", "\x1b[2D", "  |  "),
            ("\x7f", "\x1b[D", " |  "),
            ("\x7f", "\x1b[D", "|  "),
            (" ", " ", " |  "),
            (" ", " ", "  |  "),
            ("\x1b[C", "\x1b[C", "   | "),
            ("a", "a", "   a| "),
            ("\x7f", "\x1b[D\x1b[K", "   | "),
            ("\x1b[D\x1b[D", "\x1b[2D", " |   "),
            ("\x1b[3~", "", " |  "),
        ];
        for (input, output, expected) in steps {
            session.input(input);
            session.write(output);
            session.assert_prompt_input(expected);
        }
    }

    #[test]
    fn track_whitespace_beyond_cursor() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.write(&" ".repeat(8));
        session.assert_prompt_input(&format!("{}|", " ".repeat(8)));
        session.write("\x1b[4D");
        session.assert_prompt_input(&format!("{0}|{0}", " ".repeat(4)));
    }

    fn continuation_line(session: &mut Session, expected: &str) {
        session.write("\n\r∙ ");
        session.set_continuation_prompt("∙ ");
        session.assert_prompt_input(expected);
    }

    #[test]
    fn basic_2_line() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.write("echo \"a");
        session.assert_prompt_input("echo \"a|");
        continuation_line(&mut session, "echo \"a\n|");
        session.write("b");
        session.assert_prompt_input("echo \"a\nb|");
    }

    #[test]
    fn basic_3_line() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.write("echo \"a");
        session.assert_prompt_input("echo \"a|");
        continuation_line(&mut session, "echo \"a\n|");
        session.write("b");
        session.assert_prompt_input("echo \"a\nb|");
        continuation_line(&mut session, "echo \"a\nb\n|");
        session.write("c");
        session.assert_prompt_input("echo \"a\nb\nc|");
    }

    #[test]
    fn navigate_left_in_multi_line() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.write("echo \"a");
        continuation_line(&mut session, "echo \"a\n|");
        session.write("b");
        session.assert_prompt_input("echo \"a\nb|");
        session.write("\x1b[D");
        session.assert_prompt_input("echo \"a\n|b");
        session.write("\x1b[@c");
        session.assert_prompt_input("echo \"a\nc|b");
        session.write("\x1b[K\n\r∙ ");
        session.assert_prompt_input("echo \"a\nc\n|");
        session.write("b");
        session.assert_prompt_input("echo \"a\nc\nb|");
        session.write(" foo");
        session.assert_prompt_input("echo \"a\nc\nb foo|");
        session.write("\x1b[3D");
        session.assert_prompt_input("echo \"a\nc\nb |foo");
    }

    #[test]
    fn navigate_up_in_multi_line() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.write("echo \"foo");
        continuation_line(&mut session, "echo \"foo\n|");
        session.write("bar");
        continuation_line(&mut session, "echo \"foo\nbar\n|");
        session.write("baz");
        session.assert_prompt_input("echo \"foo\nbar\nbaz|");
        let steps = [
            ("\x1b[A", "echo \"foo\nbar|\nbaz"),
            ("\x1b[D", "echo \"foo\nba|r\nbaz"),
            ("\x1b[D", "echo \"foo\nb|ar\nbaz"),
            ("\x1b[D", "echo \"foo\n|bar\nbaz"),
            ("\x1b[1;9H", "echo \"|foo\nbar\nbaz"),
            ("\x1b[C", "echo \"f|oo\nbar\nbaz"),
            ("\x1b[C", "echo \"fo|o\nbar\nbaz"),
            ("\x1b[C", "echo \"foo|\nbar\nbaz"),
        ];
        for (output, expected) in steps {
            session.write(output);
            session.assert_prompt_input(expected);
        }
    }

    #[test]
    fn navigating_up_when_first_line_contains_invalid_stale_trailing_whitespace() {
        let mut session = Session::new();
        session.write("$ ");
        session.fire_command_start();
        session.write("echo \"foo      \x1b[6D");
        session.assert_prompt_input("echo \"foo|");
        continuation_line(&mut session, "echo \"foo\n|");
        session.write("bar");
        session.assert_prompt_input("echo \"foo\nbar|");
        session.write("\x1b[D");
        session.assert_prompt_input("echo \"foo\nba|r");
        session.write("\x1b[D");
        session.assert_prompt_input("echo \"foo\nb|ar");
        session.write("\x1b[D");
        session.assert_prompt_input("echo \"foo\n|bar");
    }

    #[test]
    fn basic_wrapped_line_no_continuation_prompt() {
        let mut session = Session::with_size(5, 10);
        session.write("$ ");
        session.fire_command_start();
        session.assert_prompt_input("|");
        session.write("ech");
        session.assert_prompt_input("ech|");
        session.write("o ");
        session.assert_prompt_input("echo |");
        session.write("\"a\"");
        session.assert_prompt_input("echo \"a\"| ");
        session.write("\n\r b");
        session.assert_prompt_input("echo \"a\"\n b|");
        session.write("\n\r c");
        session.assert_prompt_input("echo \"a\"\n b\n c|");
    }

    #[test]
    fn basic_wrapped_line_continuation_prompt() {
        let mut session = Session::with_size(5, 10);
        session.set_continuation_prompt("∙ ");
        session.write("$ ");
        session.fire_command_start();
        session.assert_prompt_input("|");
        session.write("ech");
        session.assert_prompt_input("ech|");
        session.write("o ");
        session.assert_prompt_input("echo |");
        session.write("\"a\"");
        session.assert_prompt_input("echo \"a\"| ");
        session.write("\n\r∙ ");
        session.assert_prompt_input("echo \"a\"\n|");
        session.write("b");
        session.assert_prompt_input("echo \"a\"\nb|");
        session.write("\n\r∙ ");
        session.assert_prompt_input("echo \"a\"\nb\n|");
        session.write("c");
        session.assert_prompt_input("echo \"a\"\nb\nc|");
        session.write("\n\r∙ ");
        session.assert_prompt_input("echo \"a\"\nb\nc\n|");
    }

    const FISH_PROMPT: &str = "[I] meganrogge@Megans-MacBook-Pro ~ (main|BISECTING)>";

    #[test]
    fn forward_slash_continuation() {
        let mut session = Session::new();
        session.model.set_is_fish(true);
        session.write("$ ");
        session.assert_prompt_input("|");
        session.write(FISH_PROMPT);
        session.fire_command_start();
        session.write("ech\\");
        session.assert_prompt_input("ech\\|");
        session.write("\no bye");
        session.assert_prompt_input("echo bye|");
    }

    #[test]
    fn newline_with_no_continuation() {
        let mut session = Session::new();
        session.model.set_is_fish(true);
        session.write("$ ");
        session.assert_prompt_input("|");
        session.write(FISH_PROMPT);
        session.fire_command_start();
        session.assert_prompt_input("|");
        session.write("echo \"hi");
        session.assert_prompt_input("echo \"hi|");
        session.write("\nand bye\nwhy\"");
        session.assert_prompt_input("echo \"hi\nand bye\nwhy\"|");
    }

    const PWSH_TITLE: &str = "\x1b[?25l\x1b[2J\x1b[m\x1b[H\x1b]0;C:\\Program Files\\WindowsApps\\Microsoft.PowerShell_7.4.2.0_x64__8wekyb3d8bbwe\\pwsh.exe\x07\x1b[?25h";
    const PWSH_SETUP: [&str; 3] = [
        "\x1b]633;P;IsWindows=True\x07",
        "\x1b]633;P;ContinuationPrompt=\x1b[38;5;8m∙\x1b[0m \x07",
        "\x1b]633;A\x07\x1b]633;P;Cwd=C:\\Github\\microsoft\\vscode\x07\x1b]633;B\x07",
    ];

    fn starship_prompt(time: &str, branch: &str, status: &str) -> String {
        format!(
            "\x1b[34m\r\n\x1b[38;2;17;17;17m\x1b[44m{time} \x1b[34m\x1b[41m \x1b[38;2;17;17;17mvscode \x1b[31m\x1b[43m \x1b[38;2;17;17;17m {branch} \x1b[33m\x1b[46m \x1b[38;2;17;17;17m{status} \x1b[36m\x1b[49m \x1b[mvia \x1b[32m\x1b[1m v18.18.2 \r\n❯\x1b[m "
        )
    }

    fn replay_events(session: &mut Session, events: &[&str]) {
        for data in events {
            session.write(data);
        }
    }

    fn start_pwsh_session(clear_lines: usize, time: &str, branch: &str, status: &str) -> Session {
        let mut session = Session::new();
        session.write(PWSH_TITLE);
        session.write(&format!("\x1b[?25l{}\x1b[K\x1b[H\x1b[?25h", "\x1b[K\r\n".repeat(clear_lines)));
        replay_events(&mut session, &PWSH_SETUP);
        session.write(&starship_prompt(time, branch, status));
        session
    }

    #[test]
    fn input_with_ignored_ghost_text() {
        let mut session = start_pwsh_session(25, "03:13:47", "tyriar/prompt_input_model", "$⇡");
        session.fire_command_start();
        session.assert_prompt_input("|");
        replay_events(
            &mut session,
            &[
                "\x1b[?25l\x1b[93mf\x1b[97m\x1b[2m\x1b[3makecommand\x1b[3;4H\x1b[?25h",
                "\x1b[m",
                "\x1b[93m\x08fo\x1b[9X",
                "\x1b[m",
                "\x1b[?25l\x1b[93m\x1b[3;3Hfoo\x1b[?25h",
                "\x1b[m",
            ],
        );
        session.assert_prompt_input("foo|");
    }

    #[test]
    fn input_with_accepted_and_run_ghost_text() {
        let mut session = start_pwsh_session(25, "03:41:36", "tyriar/prompt_input_model", "$");
        session.set_continuation_prompt("∙ ");
        session.fire_command_start();
        session.assert_prompt_input("|");
        let steps = [
            ("\x1b[?25l\x1b[93me\x1b[97m\x1b[2m\x1b[3mcho \"hello world\"\x1b[3;4H\x1b[?25h", "e|[cho \"hello world\"]"),
            ("\x1b[?25l\x1b[93m\x08ec\x1b[97m\x1b[2m\x1b[3mho \"hello world\"\x1b[3;5H\x1b[?25h", "ec|[ho \"hello world\"]"),
            ("\x1b[?25l\x1b[93m\x1b[3;3Hech\x1b[97m\x1b[2m\x1b[3mo \"hello world\"\x1b[3;6H\x1b[?25h", "ech|[o \"hello world\"]"),
            ("\x1b[?25l\x1b[93m\x1b[3;3Hecho\x1b[97m\x1b[2m\x1b[3m \"hello world\"\x1b[3;7H\x1b[?25h", "echo|[ \"hello world\"]"),
            ("\x1b[?25l\x1b[93m\x1b[3;3Hecho \x1b[97m\x1b[2m\x1b[3m\"hello world\"\x1b[3;8H\x1b[?25h", "echo |[\"hello world\"]"),
            ("\x1b[?25l\x1b[93m\x1b[3;3Hecho \x1b[36m\"hello world\"\x1b[?25h", "echo \"hello world\"|"),
        ];
        for (output, expected) in steps {
            replay_events(&mut session, &[output, "\x1b[m"]);
            session.assert_prompt_input(expected);
        }

        session.write("\x1b]633;E;echo \"hello world\";ff464d39-bc80-4bae-9ead-b1cafc4adf6f\x07\x1b]633;C\x07");
        session.model.command_executed();
        session.assert_prompt_input("echo \"hello world\"");

        replay_events(&mut session, &["\r\n", "hello world\r\n"]);
        session.assert_prompt_input("echo \"hello world\"");

        session.write("\x1b]633;D;0\x07\x1b]633;A\x07\x1b]633;P;Cwd=C:\\Github\\microsoft\\vscode\x07\x1b]633;B\x07");
        session.write(&starship_prompt("03:41:42", "tyriar/prompt_input_model", "$"));
        session.fire_command_start();
        session.assert_prompt_input("|");
    }

    #[test]
    fn input_go_to_start_ctrl_home_delete_word_in_front_ctrl_delete() {
        let mut session = start_pwsh_session(28, "16:07:06", "tyriar/210662", "$!");
        session.fire_command_start();
        session.assert_prompt_input("|");
        replay_events(
            &mut session,
            &[
                "\x1b[?25l\x1b[93mG\x1b[97m\x1b[2m\x1b[3mit push\x1b[3;4H\x1b[?25h",
                "\x1b[m",
                "\x1b[?25l\x1b[93m\x08Ge\x1b[97m\x1b[2m\x1b[3mt-ChildItem -Path a\x1b[3;5H\x1b[?25h",
                "\x1b[m",
                "\x1b[?25l\x1b[93m\x1b[3;3HGet\x1b[97m\x1b[2m\x1b[3m-ChildItem -Path a\x1b[3;6H\x1b[?25h",
            ],
        );
        session.assert_prompt_input("Get|[-ChildItem -Path a]");
        replay_events(&mut session, &["\x1b[m", "\x1b[?25l\x1b[3;3H\x1b[?25h", "\x1b[21X"]);
        assert_eq!(session.model.get_combined_string(false), "|");
    }
}
