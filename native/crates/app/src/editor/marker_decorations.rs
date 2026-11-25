//! The model decorations of a model's problem markers: editor/common/services/markerDecorationsService.ts.

use std::sync::Arc;

use super::{OpenEditor, Position, char_column, end_of, folding::tracked, word_at};
use crate::markers::{DEPRECATED, HINT, Marker, UNNECESSARY};

/// `MarkerDecorations.MAX_MARKER_DECORATIONS_LIMIT`.
pub const MAX_MARKER_DECORATIONS: usize = 5000;

/// A marker and the range its decoration tracks, in chars.
#[derive(Clone, Debug)]
pub struct MarkerDecoration {
    pub marker: Arc<Marker>,
    pub start: Position,
    pub end: Position,
}

impl OpenEditor {
    /// `MarkerDecorations.update`: markers are compared by identity, so a decoration whose marker is still
    /// there keeps the range it tracked through edits; returns whether anything changed.
    pub fn update_markers(&mut self, markers: &[Arc<Marker>]) -> bool {
        let before = self.markers.len();
        self.markers.retain(|decoration| markers.iter().any(|marker| Arc::ptr_eq(marker, &decoration.marker)));
        let removed = before != self.markers.len();
        let mut added = false;
        for marker in markers {
            if self.markers.iter().any(|decoration| Arc::ptr_eq(&decoration.marker, marker)) {
                continue;
            }
            let (start, end) = decoration_range(&self.document.lines, marker);
            self.markers.push(MarkerDecoration { marker: marker.clone(), start, end });
            added = true;
        }
        removed || added
    }

    /// `TrackedRangeStickiness.NeverGrowsWhenTypingAtEdges` through the replace of `start..end` with `text`.
    pub(super) fn track_markers(&mut self, start: Position, end: Position, text: &str) {
        let new_end = end_of(start, text);
        for decoration in &mut self.markers {
            decoration.start = tracked(decoration.start, start, end, new_end, false);
            decoration.end = tracked(decoration.end, start, end, new_end, true).max(decoration.start);
        }
    }
}

/// `_createDecorationRange`: a hint without tags is 2 columns wide on its first line; an empty range grows to the
/// word at it unless it is at or past the line's last non-whitespace; a whole line (`endColumn` `Number.MAX_VALUE`)
/// starts at its first non-whitespace.
fn decoration_range(lines: &[String], marker: &Marker) -> (Position, Position) {
    let (mut end_line, mut end_column) = (marker.end_line, marker.end_column);
    if marker.severity == HINT && !marker.has_tag(UNNECESSARY) && !marker.has_tag(DEPRECATED) {
        (end_line, end_column) = (marker.start_line, marker.start_column + 2);
    }
    let start = validate(lines, marker.start_line, marker.start_column);
    let end = validate(lines, end_line, end_column).max(start);
    let line = &lines[start.line];
    if start == end {
        let chars: Vec<char> = line.chars().collect();
        let last_non_whitespace = chars.iter().rposition(|c| *c != ' ' && *c != '\t').map_or(chars.len(), |index| index + 1);
        if last_non_whitespace == 0 || end.column >= last_non_whitespace {
            return (start, end);
        }
        return match word_at(line, start.column) {
            Some(word) => (Position::new(start.line, word.start), Position::new(start.line, word.end)),
            None => (start, end),
        };
    }
    if marker.end_column == usize::MAX && marker.start_column == 1 && start.line == end.line {
        let first_non_whitespace = line.chars().position(|c| c != ' ' && c != '\t').unwrap_or(0);
        if first_non_whitespace < end.column {
            return (Position::new(start.line, first_non_whitespace), end);
        }
    }
    (start, end)
}

/// `validatePosition` of a 1-based line and UTF-16 column.
pub fn validate(lines: &[String], line: usize, column: usize) -> Position {
    if line > lines.len() {
        let last = lines.len() - 1;
        return Position::new(last, lines[last].chars().count());
    }
    let line = line.max(1) - 1;
    Position::new(line, char_column(&lines[line], column.max(1) - 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn marker(severity: u8, range: [usize; 4], tags: &[u8]) -> Arc<Marker> {
        let [start_line, start_column, end_line, end_column] = range;
        let data = json!({ "severity": severity, "message": "m", "startLineNumber": start_line, "startColumn": start_column, "endLineNumber": end_line, "endColumn": end_column, "tags": tags });
        Arc::new(Marker::new("o", "file:///a", &data).unwrap())
    }

    fn range(lines: &[&str], marker: &Marker) -> ((usize, usize), (usize, usize)) {
        let lines: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
        let (start, end) = decoration_range(&lines, marker);
        ((start.line, start.column), (end.line, end.column))
    }

    #[test]
    fn a_hint_is_two_columns_on_its_first_line() {
        let hint = marker(HINT, [1, 3, 2, 4], &[]);
        assert_eq!(range(&["let value = 1;", "x"], &hint), ((0, 2), (0, 4)));
        let unnecessary = marker(HINT, [1, 3, 2, 2], &[UNNECESSARY]);
        assert_eq!(range(&["let value = 1;", "x"], &unnecessary), ((0, 2), (1, 1)));
    }

    #[test]
    fn an_empty_range_grows_to_the_word_unless_at_the_line_end() {
        let lines = ["let value = 1;  "];
        assert_eq!(range(&lines, &marker(8, [1, 7, 1, 7], &[])), ((0, 4), (0, 9)));
        assert_eq!(range(&lines, &marker(8, [1, 14, 1, 14], &[])), ((0, 12), (0, 13)));
        assert_eq!(range(&lines, &marker(8, [1, 15, 1, 15], &[])), ((0, 14), (0, 14)));
        assert_eq!(range(&[""], &marker(8, [1, 1, 1, 1], &[])), ((0, 0), (0, 0)));
    }

    #[test]
    fn a_whole_line_marker_starts_at_the_first_non_whitespace() {
        let data = json!({ "severity": 8, "message": "m", "startLineNumber": 1, "startColumn": 1, "endLineNumber": 1, "endColumn": f64::MAX });
        let whole_line = Marker::new("o", "file:///a", &data).unwrap();
        assert_eq!(range(&["    return;"], &whole_line), ((0, 4), (0, 11)));
    }

    #[test]
    fn utf16_columns_and_lines_past_the_end_are_validated() {
        assert_eq!(range(&["a😀b"], &marker(8, [1, 4, 1, 5], &[])), ((0, 2), (0, 3)));
        assert_eq!(range(&["ab"], &marker(8, [1, 1, 9, 1], &[])), ((0, 0), (0, 2)));
    }
}
