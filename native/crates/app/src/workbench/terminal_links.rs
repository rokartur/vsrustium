//! terminalContrib/links: Cmd+click (the `editor.multiCursorModifier` opposite) on a URL or a file path in the
//! terminal opens it, URLs through terminalUriLinkDetector.ts and paths with their line and column suffix through
//! terminalLocalLinkDetector.ts. Links are read from the whole wrapped line; the hovered link is underlined while
//! the modifier is down and shows terminalLinkManager.ts's hover ("Follow link (cmd + click)").

use std::path::{Path, PathBuf};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use gpui::{Bounds, Context, Modifiers, Window, point, px, size};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use super::terminal_view::grid;
use crate::editor::marker_decorations::validate;
use crate::editor::{Selection, VerticalReveal};
use crate::link_computer::compute_links;
use crate::terminal::link_parsing::detect_links;

/// What a click in the terminal opens.
#[derive(Clone, Debug, PartialEq)]
enum TerminalLink {
    Url(String),
    File { path: PathBuf, line: Option<u32>, column: Option<u32> },
}

/// The link under the pointer: its instance and the cells it covers (grid line, column).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct HoveredLink {
    pub id: u32,
    pub cells: Vec<(i32, usize)>,
    link: TerminalLink,
}

impl Workbench {
    /// `TerminalLinkManager` activation: opens the link under `row` and `column`; false when there is none.
    pub(super) fn open_terminal_link(&mut self, id: u32, row: i32, column: usize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(hovered) = self.terminal_link_at(id, row, column) else { return false };
        self.activate_terminal_link(hovered.link, window, cx);
        true
    }

    fn activate_terminal_link(&mut self, link: TerminalLink, window: &mut Window, cx: &mut Context<Self>) {
        match link {
            TerminalLink::Url(url) => cx.open_url(&url),
            TerminalLink::File { path, line, column } => self.open_terminal_file_link(path, line, column, window, cx),
        }
    }

    /// The link at a viewport row and column.
    fn terminal_link_at(&self, id: u32, row: i32, column: usize) -> Option<HoveredLink> {
        let instance = self.terminal_view.instance(id)?;
        if !(0..instance.term.screen_lines() as i32).contains(&row) {
            return None;
        }
        let line = row - instance.term.grid().display_offset() as i32;
        let (text, cells) = wrapped_line_text(instance, line);
        let index = cells.iter().position(|&(cell_line, cell_column)| cell_line == line && cell_column >= column)?;
        let cwd = instance.shell_integration.cwd().map(PathBuf::from).unwrap_or_else(|| instance.launch.cwd.clone());
        let home = std::env::home_dir();
        let (link, range) = link_at(&text, index, &cwd, home.as_deref())?;
        Some(HoveredLink { id, cells: cells[range].to_vec(), link })
    }

    /// `TerminalLink` hover: the link under the pointer, its hover after the delay; the underline follows the
    /// link modifier (`_isLinkActivationModifierDown`).
    pub(super) fn terminal_link_hover(&mut self, id: u32, row: i32, column: usize, modifiers: Modifiers, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal_view.link_modifier = self.terminal_link_modifier(modifiers);
        let hovered = self.terminal_link_at(id, row, column);
        if hovered.as_ref().map(|link| (&link.cells, link.id)) == self.terminal_view.hovered_link.as_ref().map(|link| (&link.cells, link.id)) {
            return;
        }
        self.terminal_view.hovered_link = hovered.clone();
        cx.notify();
        let Some(hovered) = hovered else {
            self.leave_hover_target(cx);
            return;
        };
        let (Some(pane), Some(metrics)) = (self.terminal_view.pane_bounds.get(&id).copied(), self.terminal_view.metrics.clone()) else { return };
        let (first, last) = (hovered.cells[0], *hovered.cells.last().expect("a link covers a cell"));
        let Some(instance) = self.terminal_view.instance(id) else { return };
        let origin = grid::grid_origin(pane, instance.term.screen_lines(), &metrics);
        let offset = instance.term.grid().display_offset() as i32;
        let cell = |(line, column): (i32, usize)| point(origin.x + px(column as f32 * metrics.cell_width), origin.y + px((line + offset) as f32 * metrics.cell_height));
        let top_left = cell(if first.0 == last.0 { first } else { (first.0, 0) });
        let bottom_right = cell((last.0, last.1 + 1)) + point(px(0.), px(metrics.cell_height));
        let bounds = Bounds { origin: top_left, size: size(bottom_right.x - top_left.x, bottom_right.y - top_left.y) };
        let markdown = self.terminal_link_hover_text(&hovered);
        self.show_managed_hover(("terminal-link", hovered.cells[0].0 as usize * 10_000 + hovered.cells[0].1), HoverContent::Markdown(markdown), HoverOptions::element(), bounds, window, cx);
    }

    /// `modifierDownCallback` and `modifierUpCallback`: the hovered link's underline follows the modifier.
    pub(super) fn terminal_link_modifiers_changed(&mut self, modifiers: Modifiers, cx: &mut Context<Self>) {
        let down = self.terminal_link_modifier(modifiers);
        if down != self.terminal_view.link_modifier {
            self.terminal_view.link_modifier = down;
            if self.terminal_view.hovered_link.is_some() {
                cx.notify();
            }
        }
    }

    pub(super) fn terminal_link_modifier(&self, modifiers: Modifiers) -> bool {
        // `editor.multiCursorModifier` alt (the default) makes Cmd the link modifier.
        if self.setting("editor.multiCursorModifier") == "ctrlCmd" { modifiers.alt } else { modifiers.platform }
    }

    /// `_getLinkHoverString`: "[Follow link](target) (cmd + click)", the click label after the modifier.
    fn terminal_link_hover_text(&self, hovered: &HoveredLink) -> String {
        let click = if self.setting("editor.multiCursorModifier") == "ctrlCmd" { "option + click" } else { "cmd + click" };
        // The hover's markdown only links schemes, so a path links through its file URI. Terminal output is
        // untrusted: characters that could end the link and start another are percent-encoded.
        let target = match &hovered.link {
            TerminalLink::Url(url) => url.clone(),
            TerminalLink::File { path, .. } => crate::editor_history::file_uri_string(path),
        };
        let mut encoded = String::new();
        for character in target.chars() {
            match character {
                c if c.is_whitespace() || c.is_control() || "()[]<>\"'\\".contains(c) => {
                    let mut buffer = [0; 4];
                    for byte in c.encode_utf8(&mut buffer).bytes() {
                        encoded.push_str(&format!("%{byte:02X}"));
                    }
                }
                c => encoded.push(c),
            }
        }
        format!("[Follow link]({encoded}) ({click})")
    }

    /// The cells of the hovered link to underline in pane `id`, while the modifier is down.
    pub(super) fn terminal_link_underline(&self, id: u32) -> Option<&[(i32, usize)]> {
        self.terminal_view.hovered_link.as_ref().filter(|link| link.id == id && self.terminal_view.link_modifier).map(|link| link.cells.as_slice())
    }

    fn open_terminal_file_link(&mut self, path: PathBuf, line: Option<u32>, column: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file(path, true, window, cx);
        let Some(line) = line else { return };
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut() else { return };
        let position = validate(&editor.document().lines, line as usize, column.unwrap_or(1) as usize);
        editor.set_selections(vec![Selection::caret(position)]);
        editor.reveal_range(position, position, VerticalReveal::CenterIfOutsideViewport, viewport);
        self.host.send_selection(editor, "code.jump");
        self.editor_scrolled(cx);
    }
}

/// The characters of the wrapped line through `row` (the rows it continues and those continuing it) and the screen
/// cell of each; a wide character's spacer cell adds none.
fn wrapped_line_text(instance: &crate::terminal::TerminalInstance, row: i32) -> (String, Vec<(i32, usize)>) {
    let grid = instance.term.grid();
    let columns = instance.term.columns();
    let top = -(instance.term.history_size() as i32);
    let bottom = instance.term.screen_lines() as i32 - 1;
    let wraps = |row: i32| grid[Line(row)][Column(columns - 1)].flags.contains(Flags::WRAPLINE);
    let mut first = row;
    while first > top && wraps(first - 1) {
        first -= 1;
    }
    let mut last = row;
    while last < bottom && wraps(last) {
        last += 1;
    }
    let mut text = String::new();
    let mut cells = Vec::new();
    for row in first..=last {
        for column in 0..columns {
            let cell = &grid[Point::new(Line(row), Column(column))];
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            text.push(cell.c);
            cells.push((row, column));
        }
    }
    (text, cells)
}

/// The link covering the character at `index` and its character range: a URL first (the URI detector runs before
/// the local one), then an existing file path resolved against the terminal's cwd and `~`.
fn link_at(text: &str, index: usize, cwd: &Path, home: Option<&Path>) -> Option<(TerminalLink, std::ops::Range<usize>)> {
    for link in compute_links(&[text]) {
        let range = link.start_column - 1..link.end_column - 1;
        if range.contains(&index) {
            return Some((TerminalLink::Url(link.url), range));
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
        return Some((TerminalLink::File { path, line, column }, start..end));
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
        assert_eq!(link_at(text, 6, &dir, None), Some((TerminalLink::Url("https://example.com/a".into()), 4..25)));
        assert_eq!(link_at(text, 32, &dir, None), Some((TerminalLink::File { path: dir.join("src/main.rs"), line: Some(12), column: Some(5) }, 30..46)));
        assert_eq!(link_at(text, 52, &dir, None), None);
        assert_eq!(link_at(text, 0, &dir, None), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_link_wrapped_over_two_rows_is_found_from_either_row() {
        let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let colors = crate::terminal::colors::TerminalColors::new(&crate::theme::load_theme(&[themes], "Dark Modern").unwrap());
        let launch = crate::terminal::ShellLaunchConfig { extension_pty: true, cwd: std::env::temp_dir(), ..Default::default() };
        let (mut instance, _) = crate::terminal::TerminalInstance::new(1, launch, 20, 4, alacritty_terminal::term::Config::default());
        instance.process(b"go https://example.com/abc/def ok", &colors);
        let (text, cells) = wrapped_line_text(&instance, 1);
        assert!(text.starts_with("go https://example.com/abc/def ok"), "{text:?}");
        let index = cells.iter().position(|&cell| cell == (1, 2)).unwrap();
        let (link, range) = link_at(&text, index, Path::new("/"), None).unwrap();
        assert_eq!(link, TerminalLink::Url("https://example.com/abc/def".into()));
        assert_eq!((cells[range.start], cells[range.end - 1]), ((0, 3), (1, 9)));
    }
}
