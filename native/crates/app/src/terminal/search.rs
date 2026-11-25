//! The find model of the search addon (@xterm/addon-search): matches over the whole buffer, wrapped lines joined.
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::search::Match;
use regex::RegexBuilder;

/// Ports `Constants.DEFAULT_HIGHLIGHT_LIMIT` (`XtermTerminalConstants.SearchHighlightLimit`).
pub const HIGHLIGHT_LIMIT: usize = 1000;

/// Ports `Constants.NON_WORD_CHARACTERS` of `SearchEngine`.
const NON_WORD_CHARACTERS: &str = " ~!@#$%^&*()+`-=[]{}|\\;:\"',./<>?";

/// Ports `ISearchOptions` without `incremental` and `decorations`, which belong to the widget.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub regex: bool,
    pub whole_word: bool,
    pub case_sensitive: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Next,
    Previous,
}

/// Ports `SearchAddon._highlightAllMatches`: every match in buffer order, scrollback included, capped at `HIGHLIGHT_LIMIT`.
pub fn find_matches<T: EventListener>(term: &Term<T>, query: &str, options: SearchOptions) -> Vec<Match> {
    if query.is_empty() {
        return Vec::new();
    }
    let pattern = if options.regex { query.to_owned() } else { regex::escape(query) };
    let Ok(regex) = RegexBuilder::new(&pattern).case_insensitive(!options.case_sensitive).build() else {
        return Vec::new();
    };
    let mut matches = Vec::new();
    let mut line = term.topmost_line();
    while line <= term.bottommost_line() {
        let logical = read_logical_line(term, line);
        for found in regex.find_iter(&logical.text) {
            if found.is_empty() || (options.whole_word && !is_whole_word(&logical.text, found.start(), found.end())) {
                continue;
            }
            matches.push(logical.cell_at(found.start()).0..=logical.cell_at(found.end() - 1).1);
            if matches.len() == HIGHLIGHT_LIMIT {
                return matches;
            }
        }
        line = logical.next_line;
    }
    matches
}

/// Ports `SearchEngine.findNextWithSelection` and `findPreviousWithSelection` over the `find_matches` result.
///
/// `current` is the selected match when the query is unchanged. Otherwise `from` is the selection start; without a
/// selection pass the buffer top for `Next` and `Point::new(term.bottommost_line() + 1, Column(0))` for `Previous`.
pub fn next_index(matches: &[Match], current: Option<usize>, from: Point, direction: Direction) -> Option<usize> {
    let last = matches.len().checked_sub(1)?;
    if let Some(current) = current {
        let current = current.min(last);
        return Some(match direction {
            Direction::Next if current == last => 0,
            Direction::Next => current + 1,
            Direction::Previous if current == 0 => last,
            Direction::Previous => current - 1,
        });
    }
    match direction {
        Direction::Next => Some(matches.iter().position(|found| *found.start() >= from).unwrap_or(0)),
        Direction::Previous => {
            if let Some(index) = matches.iter().position(|found| *found.start() == from) {
                return Some(index);
            }
            Some(matches.iter().rposition(|found| *found.end() < from).unwrap_or(last))
        }
    }
}

/// Ports `SearchLineCache.translateBufferLineToStringWithWrap`: one row plus the rows it wraps into.
struct LogicalLine {
    text: String,
    /// Byte offset of each cell's text, the cell, and its last column (the spacer of a wide char).
    cells: Vec<(usize, Point, Point)>,
    next_line: Line,
}

impl LogicalLine {
    fn cell_at(&self, byte: usize) -> (Point, Point) {
        let index = self.cells.partition_point(|&(offset, ..)| offset <= byte) - 1;
        (self.cells[index].1, self.cells[index].2)
    }
}

fn read_logical_line<T: EventListener>(term: &Term<T>, first: Line) -> LogicalLine {
    let grid = term.grid();
    let last_column = term.last_column();
    let mut text = String::new();
    let mut cells = Vec::new();
    let mut line = first;
    loop {
        let row = &grid[line];
        for column in 0..=last_column.0 {
            let cell = &row[Column(column)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let point = Point::new(line, Column(column));
            let end = if cell.flags.contains(Flags::WIDE_CHAR) && column < last_column.0 { Point::new(line, Column(column + 1)) } else { point };
            cells.push((text.len(), point, end));
            text.push(cell.c);
            text.extend(cell.zerowidth().into_iter().flatten());
        }
        let wraps = row[last_column].flags.contains(Flags::WRAPLINE) && line < term.bottommost_line();
        line += 1;
        if !wraps {
            break;
        }
    }
    while text.ends_with(' ') && cells.last().is_some_and(|&(offset, ..)| offset == text.len() - 1) {
        text.pop();
        cells.pop();
    }
    LogicalLine { text, cells, next_line: line }
}

/// Ports `SearchEngine._isWholeWord`.
fn is_whole_word(text: &str, start: usize, end: usize) -> bool {
    let separates = |c: char| NON_WORD_CHARACTERS.contains(c);
    text[..start].chars().next_back().is_none_or(separates) && text[end..].chars().next().is_none_or(separates)
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;

    fn terminal(columns: usize, rows: usize, data: &str) -> Term<VoidListener> {
        let config = Config { scrolling_history: 1000, ..Config::default() };
        let mut term = Term::new(config, &TermSize::new(columns, rows), VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, data.as_bytes());
        term
    }

    fn point(line: i32, column: usize) -> Point {
        Point::new(Line(line), Column(column))
    }

    fn find(term: &Term<VoidListener>, query: &str, options: SearchOptions) -> Vec<Match> {
        find_matches(term, query, options)
    }

    #[test]
    fn finds_plain_text_in_buffer_order() {
        let term = terminal(40, 5, "hello world\r\nsay hello");
        assert_eq!(find(&term, "hello", SearchOptions::default()), [point(0, 0)..=point(0, 4), point(1, 4)..=point(1, 8)]);
        assert!(find(&term, "", SearchOptions::default()).is_empty());
        assert!(find(&term, "absent", SearchOptions::default()).is_empty());
    }

    #[test]
    fn case_sensitive_option() {
        let term = terminal(40, 5, "Hello hello");
        assert_eq!(find(&term, "hello", SearchOptions::default()).len(), 2);
        assert_eq!(find(&term, "HELLO", SearchOptions::default()).len(), 2);
        let sensitive = SearchOptions { case_sensitive: true, ..SearchOptions::default() };
        assert_eq!(find(&term, "hello", sensitive), [point(0, 6)..=point(0, 10)]);
    }

    #[test]
    fn whole_word_uses_the_addon_word_separators() {
        let term = terminal(40, 5, "foo foobar foo-bar xfoo foo");
        let whole = SearchOptions { whole_word: true, ..SearchOptions::default() };
        assert_eq!(find(&term, "foo", whole), [point(0, 0)..=point(0, 2), point(0, 11)..=point(0, 13), point(0, 24)..=point(0, 26)]);
        assert_eq!(find(&term, "foo", SearchOptions::default()).len(), 5);
    }

    #[test]
    fn regex_option() {
        let term = terminal(40, 5, "a1 b22 C333");
        let regex = SearchOptions { regex: true, ..SearchOptions::default() };
        assert_eq!(find(&term, r"\d+", regex), [point(0, 1)..=point(0, 1), point(0, 4)..=point(0, 5), point(0, 8)..=point(0, 10)]);
        assert_eq!(find(&term, "c3+", regex), [point(0, 7)..=point(0, 10)]);
        assert!(find(&term, "c3+", SearchOptions { case_sensitive: true, ..regex }).is_empty());
        assert!(find(&term, r"\d+", SearchOptions::default()).is_empty(), "without regex the query is literal");
        assert!(find(&term, "x*", regex).is_empty(), "empty matches are skipped");
    }

    #[test]
    fn invalid_regex_finds_nothing() {
        let term = terminal(40, 5, "a(b");
        let regex = SearchOptions { regex: true, ..SearchOptions::default() };
        assert!(find(&term, "(", regex).is_empty());
        assert!(find(&term, "a(?=b)", regex).is_empty());
        assert_eq!(find(&term, "(", SearchOptions::default()), [point(0, 1)..=point(0, 1)]);
    }

    #[test]
    fn matches_across_a_wrapped_line() {
        let term = terminal(10, 5, "0123456789abcdefghijKLM\r\nnext");
        assert_eq!(find(&term, "89ab", SearchOptions::default()), [point(0, 8)..=point(1, 1)]);
        assert_eq!(find(&term, "9abcdefghijk", SearchOptions::default()), [point(0, 9)..=point(2, 0)]);
        assert!(find(&term, "mnext", SearchOptions::default()).is_empty(), "a hard line break does not join lines");
    }

    #[test]
    fn matches_in_scrollback() {
        let term = terminal(20, 3, "needle\r\n1\r\n2\r\n3\r\n4\r\nneedle");
        assert_eq!(find(&term, "needle", SearchOptions::default()), [point(-3, 0)..=point(-3, 5), point(2, 0)..=point(2, 5)]);
    }

    #[test]
    fn wide_characters_cover_both_cells() {
        let term = terminal(20, 3, "日本語x 本");
        assert_eq!(find(&term, "語x", SearchOptions::default()), [point(0, 4)..=point(0, 6)]);
        assert_eq!(find(&term, "本", SearchOptions::default()), [point(0, 2)..=point(0, 3), point(0, 8)..=point(0, 9)]);
    }

    #[test]
    fn wide_character_wrapped_to_the_next_row() {
        let term = terminal(5, 3, "abcd日x");
        assert_eq!(find(&term, "d日x", SearchOptions::default()), [point(0, 3)..=point(1, 2)]);
    }

    #[test]
    fn next_and_previous_wrap_around() {
        let term = terminal(40, 5, "ab ab\r\nab");
        let matches = find(&term, "ab", SearchOptions::default());
        assert_eq!(matches.len(), 3);
        assert_eq!(next_index(&matches, Some(0), point(0, 0), Direction::Next), Some(1));
        assert_eq!(next_index(&matches, Some(2), point(0, 0), Direction::Next), Some(0));
        assert_eq!(next_index(&matches, Some(1), point(0, 0), Direction::Previous), Some(0));
        assert_eq!(next_index(&matches, Some(0), point(0, 0), Direction::Previous), Some(2));
        assert_eq!(next_index(&[], Some(0), point(0, 0), Direction::Next), None);
    }

    #[test]
    fn start_point_without_a_current_match() {
        let term = terminal(40, 5, "ab ab\r\nab");
        let matches = find(&term, "ab", SearchOptions::default());
        let end = Point::new(term.bottommost_line() + 1, Column(0));
        assert_eq!(next_index(&matches, None, point(term.topmost_line().0, 0), Direction::Next), Some(0));
        assert_eq!(next_index(&matches, None, end, Direction::Previous), Some(2));
        assert_eq!(next_index(&matches, None, point(0, 1), Direction::Next), Some(1));
        assert_eq!(next_index(&matches, None, point(1, 5), Direction::Next), Some(0), "past the last match wraps to the first");
        assert_eq!(next_index(&matches, None, point(0, 0), Direction::Previous), Some(0), "a match at the selection start is kept");
        assert_eq!(next_index(&matches, None, point(0, 4), Direction::Previous), Some(0));
        let first = term.topmost_line();
        assert_eq!(next_index(&matches[1..], None, point(first.0, 0), Direction::Previous), Some(1), "before the first match wraps to the last");
    }

    #[test]
    fn result_count_is_capped_at_the_highlight_limit() {
        let term = terminal(40, 5, &format!("{}\r\n", "x".repeat(40)).repeat(30));
        let matches = find(&term, "x", SearchOptions::default());
        assert_eq!(matches.len(), HIGHLIGHT_LIMIT);
        assert_eq!(matches[0], point(term.topmost_line().0, 0)..=point(term.topmost_line().0, 0));
    }
}
