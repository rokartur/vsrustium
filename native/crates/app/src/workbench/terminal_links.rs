//! terminalContrib/links: Cmd+click (the `editor.multiCursorModifier` opposite) on a URL or a file path in the
//! terminal opens it, URLs through terminalUriLinkDetector.ts and paths with their line and column suffix through
//! terminalLocalLinkDetector.ts.

use std::path::{Path, PathBuf};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use gpui::{Context, Window};

use super::Workbench;
use crate::editor::marker_decorations::validate;
use crate::editor::{Selection, VerticalReveal};
use crate::link_computer::compute_links;
use crate::terminal::link_parsing::detect_links;

/// What a click in the terminal opens.
#[derive(Debug, PartialEq)]
enum TerminalLink {
    Url(String),
    File { path: PathBuf, line: Option<u32>, column: Option<u32> },
}

impl Workbench {
    /// `TerminalLinkManager` activation: opens the link under `row` and `column`; false when there is none.
    pub(super) fn open_terminal_link(&mut self, id: u32, row: i32, column: usize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(instance) = self.terminal_view.instance(id) else { return false };
        let (text, columns) = row_text(instance, row);
        let Some(index) = columns.iter().position(|&cell| cell >= column) else { return false };
        let cwd = instance.shell_integration.cwd().map(PathBuf::from).unwrap_or_else(|| instance.launch.cwd.clone());
        let home = std::env::home_dir();
        let Some(link) = link_at(&text, index, &cwd, home.as_deref()) else { return false };
        match link {
            TerminalLink::Url(url) => cx.open_url(&url),
            TerminalLink::File { path, line, column } => self.open_terminal_file_link(path, line, column, window, cx),
        }
        true
    }

    fn open_terminal_file_link(&mut self, path: PathBuf, line: Option<u32>, column: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file(path, true, window, cx);
        let Some(line) = line else { return };
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut() else { return };
        let position = validate(&editor.document.lines, line as usize, column.unwrap_or(1) as usize);
        editor.set_selections(vec![Selection::caret(position)]);
        editor.reveal_range(position, position, VerticalReveal::CenterIfOutsideViewport, viewport);
        self.host.send_selection(editor, "code.jump");
        self.editor_scrolled(cx);
    }
}

/// The row's characters and the column of each (a wide character's spacer cell adds none).
fn row_text(instance: &crate::terminal::TerminalInstance, row: i32) -> (String, Vec<usize>) {
    let grid = instance.term.grid();
    let mut text = String::new();
    let mut columns = Vec::new();
    for column in 0..instance.term.columns() {
        let cell = &grid[Point::new(Line(row), Column(column))];
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        text.push(cell.c);
        columns.push(column);
    }
    (text, columns)
}

/// The link covering the character at `index`: a URL first (the URI detector runs before the local one), then
/// an existing file path resolved against the terminal's cwd and `~`.
// shortcut: links in a line wrapped from the row above are not joined, upstream reads the wrapped line; add with the terminal's logical line reader.
fn link_at(text: &str, index: usize, cwd: &Path, home: Option<&Path>) -> Option<TerminalLink> {
    for link in compute_links(&[text]) {
        if (link.start_column - 1..link.end_column - 1).contains(&index) {
            return Some(TerminalLink::Url(link.url));
        }
    }
    for parsed in detect_links(text) {
        let start = parsed.prefix.as_ref().map_or(parsed.path.index, |prefix| prefix.index);
        let end = parsed.suffix.as_ref().map_or(parsed.path.index + parsed.path.text.chars().count(), |suffix| suffix.suffix.index + suffix.suffix.text.chars().count());
        if !(start..end).contains(&index) {
            continue;
        }
        let raw = parsed.path.text.as_str();
        let path = match (raw.strip_prefix("~/"), home) {
            (Some(rest), Some(home)) => home.join(rest),
            _ => cwd.join(raw),
        };
        if !path.is_file() {
            return None;
        }
        let (line, column) = parsed.suffix.map_or((None, None), |suffix| (suffix.row, suffix.col));
        return Some(TerminalLink::File { path, line, column });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_existing_files_with_suffixes() {
        let dir = std::env::temp_dir().join(format!("terminal-links-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        let text = "see https://example.com/a and src/main.rs:12:5 or missing.rs:3";
        assert_eq!(link_at(text, 6, &dir, None), Some(TerminalLink::Url("https://example.com/a".into())));
        assert_eq!(link_at(text, 32, &dir, None), Some(TerminalLink::File { path: dir.join("src/main.rs"), line: Some(12), column: Some(5) }));
        assert_eq!(link_at(text, 52, &dir, None), None);
        assert_eq!(link_at(text, 0, &dir, None), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
