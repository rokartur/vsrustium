//! Go to Line/Column and Go to Offset (gotoLineQuickAccess.ts over editorNavigationQuickAccess.ts): the `:` and
//! `::` providers of quick access, previewing the position in the active editor while the value changes.

use gpui::{Context, Window};

use super::Accept;
use crate::editor::{Position, Scroll, Selection, VerticalReveal, char_column, column_from_visible, visible_column};
use crate::quick_input::{Button, Entry, Item};
use crate::workbench::Workbench;

pub const GO_TO_LINE_PREFIX: &str = ":";
pub const GO_TO_OFFSET_PREFIX: &str = "::";
const ZERO_BASED_OFFSET_STORAGE_KEY: &str = "gotoLine.useZeroBasedOffset";
pub const PLACEHOLDER: &str = "Type the line number and optional column to go to (e.g. :42:5 for line 42, column 5). Type :: to go to a character offset (e.g. ::1024 for character 1024 from the start of the file). Use negative values to navigate backwards.";
const NO_EDITOR: &str = "Open a text editor first to go to a line or an offset.";
pub const OFFSET_BUTTON: i64 = 0;

/// The editor the picker previews in: its view state when the picker opened or last moved the cursor, restored when
/// the picker goes away (`restoreViewState`), and the previewed position (`addDecorations`).
pub struct GotoLineState {
    model_id: u32,
    selections: Vec<Selection>,
    scroll: Scroll,
    highlight: Option<Position>,
}

impl GotoLineState {
    pub fn previews(&self, model_id: u32) -> bool {
        self.model_id == model_id
    }

    /// The previewed position in the editor of `model_id`.
    pub fn highlight(&self, model_id: u32) -> Option<Position> {
        self.highlight.filter(|_| self.model_id == model_id)
    }
}

/// `parsePosition`: the position the value names, if any, whether it is an offset, and the pick's label.
#[derive(Debug, PartialEq)]
struct Parsed {
    position: Option<Position>,
    in_offset_mode: bool,
    label: String,
}

impl Workbench {
    /// `provideWithTextEditor`'s `updatePickerAndEditor`: one pick that says where Enter goes, the offset toggle in
    /// offset mode, and the editor revealing and highlighting that line.
    pub(super) fn goto_line_picks(&mut self, id: i64, value: &str, cx: &mut Context<Self>) -> Vec<Accept> {
        let viewport = self.editors.viewport;
        let zero_based = self.use_zero_based_offset();
        let Some(editor) = self.editors.active_mut() else {
            let input = self.quick_inputs.get_mut(id).expect("the shown picker");
            input.buttons.clear();
            input.set_items(vec![Entry::Item(Item { handle: 0, label: NO_EDITOR.into(), pickable: true, ..Item::default() })]);
            return vec![Accept::Nothing];
        };
        let state = self.quick_access.goto_line.get_or_insert_with(|| GotoLineState { model_id: editor.model_id, selections: editor.selections.clone(), scroll: editor.scroll, highlight: None });
        let document = editor.document();
        let parsed = parse_position(&document.lines, document.eol, document.indentation.tab_size, &value.trim()[GO_TO_LINE_PREFIX.len()..], zero_based);
        drop(document);
        state.highlight = parsed.position;
        if let Some(position) = parsed.position {
            editor.reveal_range(position, position, VerticalReveal::Center, viewport);
        }
        let input = self.quick_inputs.get_mut(id).expect("the shown picker");
        input.buttons = match parsed.in_offset_mode {
            true => vec![Button { handle: OFFSET_BUTTON, codicon: Some("index-zero".into()), tooltip: Some("Toggle Zero-Based Offset".into()), toggle: Some(zero_based), location: Some(3), ..Button::default() }],
            false => Vec::new(),
        };
        input.set_items(vec![Entry::Item(Item { handle: 0, label: parsed.label, pickable: true, ..Item::default() })]);
        self.editor_scrolled(cx);
        vec![parsed.position.map_or(Accept::Nothing, Accept::Position)]
    }

    /// The offset toggle: `useZeroBasedOffset` follows its new state.
    pub(super) fn toggle_zero_based_offset(&mut self, checked: Option<bool>) {
        let checked = checked.unwrap_or(!self.use_zero_based_offset());
        self.profile_storage.lock().unwrap().set(ZERO_BASED_OFFSET_STORAGE_KEY, checked.to_string());
    }

    /// `gotoLocation`: the cursor at the position, revealed in the center; the editor takes focus unless the pick was
    /// accepted in the background. The view state to restore becomes this one, as the cursor change updates it.
    pub(super) fn go_to_position(&mut self, position: Position, in_background: bool, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut() else { return };
        editor.set_selections(vec![Selection { anchor: position, head: position }]);
        editor.reveal_range(position, position, VerticalReveal::Center, viewport);
        self.host.send_selection(editor, "code.jump");
        if let Some(state) = self.quick_access.goto_line.as_mut().filter(|state| state.model_id == editor.model_id) {
            (state.selections, state.scroll) = (editor.selections.clone(), editor.scroll);
        }
        self.editor_scrolled(cx);
        if !in_background {
            window.focus(&self.editor_focus, cx);
        }
    }

    /// The provider's disposal (picker hidden or another prefix typed): the highlight goes and the editor gets back the
    /// view state it had, when it is still the active one.
    pub(super) fn dispose_goto_line(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.quick_access.goto_line.take() else { return };
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut().filter(|editor| editor.model_id == state.model_id) else { return };
        if editor.selections != state.selections {
            editor.set_selections(state.selections);
            self.host.send_selection(editor, "restoreState");
        }
        editor.set_scroll(state.scroll.top, state.scroll.left, viewport);
        self.editor_scrolled(cx);
    }

    /// `gotoLine.useZeroBasedOffset` in StorageScope.APPLICATION, false by default.
    fn use_zero_based_offset(&self) -> bool {
        self.profile_storage.lock().unwrap().get(ZERO_BASED_OFFSET_STORAGE_KEY) == Some("true")
    }
}

/// `parsePosition` over the value after the `:` prefix: `::offset` (negative from the end), else `line`, `line:col`,
/// `line,col` or `line#col` with negative values counted from the end and clipped to the text.
fn parse_position(lines: &[String], eol: &str, tab_size: usize, value: &str, zero_based: bool) -> Parsed {
    if let Some(offset) = value.strip_prefix(':') {
        let max_offset = value_length(lines, eol.len()) as i64;
        let Some(mut offset) = parse_int(offset) else {
            let label = match zero_based {
                true => format!("Type a character position to go to (from 0 to {}).", max_offset - 1),
                false => format!("Type a character position to go to (from 1 to {max_offset})."),
            };
            return Parsed { position: None, in_offset_mode: true, label };
        };
        let reverse = offset < 0;
        if !zero_based {
            offset -= offset.signum();
        }
        if reverse {
            offset += max_offset;
        }
        let position = position_at(lines, eol.len(), offset);
        let column = visible_column(&lines[position.line], position.column, tab_size) + 1;
        return Parsed { position: Some(position), in_offset_mode: true, label: format!("Press 'Enter' to go to line {} at column {column}.", position.line + 1) };
    }
    let parts: Vec<&str> = value.split([',', ':', '#']).collect();
    let max_line = lines.len() as i64;
    let Some(line_number) = parse_int(parts[0].trim()) else {
        return Parsed { position: None, in_offset_mode: false, label: format!("Type a line number to go to (from 1 to {max_line}).") };
    };
    let line_number = if line_number >= 0 { line_number } else { max_line + 1 + line_number }.clamp(1, max_line);
    let line = &lines[line_number as usize - 1];
    let max_column = visible_column(line, line.chars().count(), tab_size) as i64 + 1;
    let column = parts.get(1).and_then(|part| parse_int(part.trim()));
    let Some(column) = column else {
        let label = match parts.len() {
            1 => format!("Press 'Enter' to go to line {line_number} or enter colon : to add a column number."),
            _ => format!("Press 'Enter' to go to line {line_number} or enter a column number (from 1 to {max_column})."),
        };
        return Parsed { position: Some(Position::new(line_number as usize - 1, 0)), in_offset_mode: false, label };
    };
    let column = if column >= 0 { column } else { max_column + column }.clamp(1, max_column);
    let real_column = column_from_visible(line, column as usize - 1, tab_size);
    Parsed {
        position: Some(Position::new(line_number as usize - 1, real_column)),
        in_offset_mode: false,
        label: format!("Press 'Enter' to go to line {line_number} at column {column}."),
    }
}

/// JavaScript's `parseInt(value, 10)`: leading whitespace, an optional sign and the digits up to the first other char.
fn parse_int(value: &str) -> Option<i64> {
    let value = value.trim_start();
    let (sign, digits) = match value.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, value.strip_prefix('+').unwrap_or(value)),
    };
    let end = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
    digits[..end].parse::<i64>().ok().map(|number| sign * number)
}

/// `getValueLength`: UTF-16 code units, line breaks included.
fn value_length(lines: &[String], eol_length: usize) -> usize {
    lines.iter().map(|line| line.encode_utf16().count()).sum::<usize>() + eol_length * (lines.len() - 1)
}

/// `getPositionAt`: the position of a UTF-16 offset, clipped to the text; an offset inside a line break is the line's end.
fn position_at(lines: &[String], eol_length: usize, offset: i64) -> Position {
    let mut remaining = offset.max(0) as usize;
    for (index, line) in lines.iter().enumerate() {
        let length = line.encode_utf16().count();
        if remaining <= length || index == lines.len() - 1 {
            return Position::new(index, char_column(line, remaining.min(length)));
        }
        remaining = remaining.saturating_sub(length + eol_length);
        if remaining == 0 {
            return Position::new(index + 1, 0);
        }
    }
    unreachable!("a document has a line")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_parse_like_parse_position() {
        let lines: Vec<String> = ["a\tb", "hello", "😀x"].map(String::from).to_vec();
        let parse = |value: &str| {
            let parsed = parse_position(&lines, "\n", 4, value, false);
            (parsed.position.map(|position| (position.line, position.column)), parsed.label)
        };
        assert_eq!(parse(""), (None, "Type a line number to go to (from 1 to 3).".into()));
        assert_eq!(parse("2"), (Some((1, 0)), "Press 'Enter' to go to line 2 or enter colon : to add a column number.".into()));
        assert_eq!(parse("9:"), (Some((2, 0)), "Press 'Enter' to go to line 3 or enter a column number (from 1 to 3).".into()));
        assert_eq!(parse("-1"), (Some((2, 0)), "Press 'Enter' to go to line 3 or enter colon : to add a column number.".into()));
        // Column 4 is inside the tab, nearer its stop at visible column 4 than its start at 1: the column after it.
        assert_eq!(parse("1,4"), (Some((0, 2)), "Press 'Enter' to go to line 1 at column 4.".into()));
        assert_eq!(parse("1#-1"), (Some((0, 2)), "Press 'Enter' to go to line 1 at column 5.".into()));
        assert_eq!(parse(":"), (None, "Type a character position to go to (from 1 to 13).".into()));
        assert_eq!(parse(":5"), (Some((1, 0)), "Press 'Enter' to go to line 2 at column 1.".into()));
        assert_eq!(parse(":-1"), (Some((2, 2)), "Press 'Enter' to go to line 3 at column 3.".into()));
        assert_eq!(parse_position(&lines, "\n", 4, ":0", true).position, Some(Position::new(0, 0)));
        assert_eq!(position_at(&lines, 1, 3), Position::new(0, 3));
        assert_eq!(position_at(&lines, 1, 99), Position::new(2, 2));
    }

    /// gotoLineQuickAccess.test.ts `runTest` and `runTabTest`: 1-based line and column, as upstream asserts them.
    fn assert_parses(lines: &[&str], cases: &[(&str, usize, usize, bool)]) {
        let lines: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
        for &(input, line, column, zero_based) in cases {
            let position = parse_position(&lines, "\n", 4, input, zero_based).position.unwrap();
            assert_eq!((position.line + 1, position.column + 1), (line, column), "{input:?} zero_based {zero_based}");
        }
    }

    #[test]
    fn parse_position_works_as_expected() {
        let cases = [
            ("-100", 1, 1, false), ("-5", 1, 1, false), ("-1", 5, 1, false), ("0", 1, 1, false), ("1", 1, 1, false), ("2", 2, 1, false),
            ("5", 5, 1, false), ("6", 5, 1, false), ("7", 5, 1, false), ("100", 5, 1, false),
            ("2:-100", 2, 1, false), ("2:-5", 2, 2, false), ("2:-1", 2, 6, false), ("2:0", 2, 1, false), ("2:1", 2, 1, false),
            ("2:2", 2, 2, false), ("2:6", 2, 6, false), ("2:7", 2, 7, false), ("2:8", 2, 7, false), ("2:100", 2, 7, false),
            (":-1000", 1, 1, false), (":-10", 4, 5, false), (":-1", 5, 7, false), (":0", 1, 1, false), (":1", 1, 1, false),
            (":10", 2, 3, false), (":1000", 5, 7, false),
            (":-1000", 1, 1, true), (":-10", 4, 4, true), (":-1", 5, 6, true), (":0", 1, 1, true), (":1", 1, 2, true),
            (":10", 2, 4, true), (":1000", 5, 7, true),
            ("-1#6", 5, 6, false), ("2,4", 2, 4, false), ("  2  :  3  ", 2, 3, false),
        ];
        assert_parses(&["line 1", "line 2", "line 3", "line 4", "line 5"], &cases);
    }

    #[test]
    fn parse_position_works_with_tabs() {
        let cases = [
            ("1:1", 1, 1), ("1:2", 1, 1), ("1:3", 1, 1), ("1:4", 1, 2), ("1:5", 1, 2), ("1:6", 1, 3), ("1:11", 1, 8), ("1:12", 1, 8),
            ("1:-5", 1, 3), ("1:-6", 1, 2), ("1:-7", 1, 2), ("1:-8", 1, 1), ("1:-9", 1, 1), ("1:-10", 1, 1), ("1:-11", 1, 1),
            ("2:1", 2, 1), ("2:2", 2, 1), ("2:3", 2, 1), ("2:4", 2, 2), ("2:5", 2, 2), ("2:8", 2, 3), ("2:9", 2, 3), ("2:11", 2, 5),
        ];
        assert_parses(&["\tline 1", "\t\tline 2", "\tline 3"], &cases.map(|(input, line, column)| (input, line, column, false)));
    }
}
