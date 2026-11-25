//! The line diff of `editor/common/diff` (VS Code 1.147.0): `DefaultLinesDiffComputer` ("advanced") with its
//! sequence algorithms, heuristics and moved-line detection. Columns count chars where upstream counts UTF-16 units.

mod algorithms;
mod computer;
mod heuristics;
mod moved_lines;
mod sequences;
mod string_diff;
#[cfg(test)]
mod tests;

pub use algorithms::Timeout;
pub use computer::{compute_diff, verify_lines_diff, DiffOptions, LinesDiff, MovedText, Validity};
pub use string_diff::string_diff;

use std::fmt;

/// `OffsetRange`: `[start, end)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OffsetRange {
    pub start: usize,
    pub end: usize,
}

impl OffsetRange {
    pub fn new(start: usize, end: usize) -> Self {
        assert!(start <= end, "Invalid range: [{start}, {end})");
        OffsetRange { start, end }
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    fn delta(&self, offset: isize) -> Self {
        OffsetRange::new(self.start.wrapping_add_signed(offset), self.end.wrapping_add_signed(offset))
    }

    fn join(&self, other: &OffsetRange) -> Self {
        OffsetRange::new(self.start.min(other.start), self.end.max(other.end))
    }

    fn intersect(&self, other: &OffsetRange) -> Option<Self> {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        (start <= end).then(|| OffsetRange::new(start, end))
    }

    fn intersects(&self, other: &OffsetRange) -> bool {
        self.start.max(other.start) < self.end.min(other.end)
    }
}

/// `LineRange`: 1-based `[start, end)` lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineRange {
    pub start: usize,
    pub end: usize,
}

impl LineRange {
    pub fn new(start: usize, end: usize) -> Self {
        assert!(start <= end, "Invalid line range: [{start},{end})");
        LineRange { start, end }
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    pub fn contains(&self, line: usize) -> bool {
        self.start <= line && line < self.end
    }

    fn delta(&self, offset: isize) -> Self {
        LineRange::new(self.start.wrapping_add_signed(offset), self.end.wrapping_add_signed(offset))
    }

    pub fn join(&self, other: &LineRange) -> Self {
        LineRange::new(self.start.min(other.start), self.end.max(other.end))
    }

    pub fn intersect(&self, other: &LineRange) -> Option<Self> {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        (start <= end).then(|| LineRange::new(start, end))
    }

    pub fn intersects_or_touches(&self, other: &LineRange) -> bool {
        self.start <= other.end && other.start <= self.end
    }

    fn to_offset_range(self) -> OffsetRange {
        OffsetRange::new(self.start - 1, self.end - 1)
    }
}

impl fmt::Display for LineRange {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "[{},{})", self.start, self.end)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

/// `Range`, 1-based, start never after end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    pub fn new(start_line: usize, start_column: usize, end_line: usize, end_column: usize) -> Self {
        Range::from_positions(Position { line: start_line, column: start_column }, Position { line: end_line, column: end_column })
    }

    fn from_positions(a: Position, b: Position) -> Self {
        if b < a { Range { start: b, end: a } } else { Range { start: a, end: b } }
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "[{},{} -> {},{}]", self.start.line, self.start.column, self.end.line, self.end.column)
    }
}

/// `RangeMapping`: a character-level change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RangeMapping {
    pub original: Range,
    pub modified: Range,
}

impl RangeMapping {
    pub fn flip(&self) -> Self {
        RangeMapping { original: self.modified, modified: self.original }
    }
}

impl fmt::Display for RangeMapping {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{{{}->{}}}", self.original, self.modified)
    }
}

/// `LineRangeMapping` and `DetailedLineRangeMapping`: changed lines and, when known, their character changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineRangeMapping {
    pub original: LineRange,
    pub modified: LineRange,
    pub inner_changes: Option<Vec<RangeMapping>>,
}

impl LineRangeMapping {
    pub fn new(original: LineRange, modified: LineRange) -> Self {
        LineRangeMapping { original, modified, inner_changes: None }
    }

    pub fn flip(&self) -> Self {
        LineRangeMapping { original: self.modified, modified: self.original, inner_changes: self.inner_changes.as_ref().map(|changes| changes.iter().map(RangeMapping::flip).collect()) }
    }

    fn join(&self, other: &LineRangeMapping) -> Self {
        LineRangeMapping::new(self.original.join(&other.original), self.modified.join(&other.modified))
    }

    /// `LineRangeMapping.inverse`: the unchanged line ranges between `mapping`.
    pub fn inverse(mapping: &[LineRangeMapping], original_line_count: usize, modified_line_count: usize) -> Vec<LineRangeMapping> {
        let mut result = Vec::new();
        let mut last_original = 1;
        let mut last_modified = 1;
        for m in mapping {
            let r = LineRangeMapping::new(LineRange::new(last_original, m.original.start), LineRange::new(last_modified, m.modified.start));
            if !r.modified.is_empty() {
                result.push(r);
            }
            last_original = m.original.end;
            last_modified = m.modified.end;
        }
        let r = LineRangeMapping::new(LineRange::new(last_original, original_line_count + 1), LineRange::new(last_modified, modified_line_count + 1));
        if !r.modified.is_empty() {
            result.push(r);
        }
        result
    }

    /// `toRangeMapping2`: the lines as a character range, ending at the next line's start when it exists.
    fn to_range_mapping2(&self, original: &[String], modified: &[String]) -> RangeMapping {
        let valid = |line: usize, lines: &[String]| line >= 1 && line <= lines.len();
        if valid(self.original.end, original) && valid(self.modified.end, modified) {
            return RangeMapping { original: Range::new(self.original.start, 1, self.original.end, 1), modified: Range::new(self.modified.start, 1, self.modified.end, 1) };
        }
        if !self.original.is_empty() && !self.modified.is_empty() {
            return RangeMapping {
                original: Range::from_positions(Position { line: self.original.start, column: 1 }, line_end(self.original.end - 1, original)),
                modified: Range::from_positions(Position { line: self.modified.start, column: 1 }, line_end(self.modified.end - 1, modified)),
            };
        }
        if self.original.start > 1 && self.modified.start > 1 {
            return RangeMapping {
                original: Range::from_positions(line_end(self.original.start - 1, original), line_end(self.original.end - 1, original)),
                modified: Range::from_positions(line_end(self.modified.start - 1, modified), line_end(self.modified.end - 1, modified)),
            };
        }
        panic!("toRangeMapping2: no range for {self:?}");
    }
}

/// `normalizePosition` of `(line, MAX_SAFE_INTEGER)`.
fn line_end(line: usize, lines: &[String]) -> Position {
    if line < 1 {
        return Position { line: 1, column: 1 };
    }
    if line > lines.len() {
        return Position { line: lines.len(), column: char_len(&lines[lines.len() - 1]) + 1 };
    }
    Position { line, column: char_len(&lines[line - 1]) + 1 }
}

fn char_len(text: &str) -> usize {
    text.chars().count()
}

/// `lineRangeMappingFromRangeMappings`: groups character changes that touch the same lines.
fn line_range_mappings_from_range_mappings(alignments: &[RangeMapping], original: &[String], modified: &[String]) -> Vec<LineRangeMapping> {
    let mut changes: Vec<LineRangeMapping> = Vec::new();
    let mut previous: Option<LineRangeMapping> = None;
    for alignment in alignments {
        let m = line_range_mapping(alignment, original, modified);
        // groupAdjacentBy compares each item with the one before it, not with the group so far.
        let grouped = previous.as_ref().is_some_and(|p| p.original.intersects_or_touches(&m.original) || p.modified.intersects_or_touches(&m.modified));
        match changes.last_mut() {
            Some(last) if grouped => {
                last.original = last.original.join(&m.original);
                last.modified = last.modified.join(&m.modified);
                last.inner_changes.as_mut().unwrap().push(*alignment);
            }
            _ => changes.push(m.clone()),
        }
        previous = Some(m);
    }
    changes
}

/// `getLineRangeMapping`.
fn line_range_mapping(mapping: &RangeMapping, original: &[String], modified: &[String]) -> LineRangeMapping {
    let (o, m) = (mapping.original, mapping.modified);
    let mut line_start_delta = 0;
    let mut line_end_delta: isize = 0;
    if m.end.column == 1 && o.end.column == 1 && o.start.line <= o.end.line && m.start.line <= m.end.line {
        line_end_delta = -1;
    }
    if m.start.column - 1 >= char_len(&modified[m.start.line - 1])
        && o.start.column - 1 >= char_len(&original[o.start.line - 1])
        && o.start.line as isize <= o.end.line as isize + line_end_delta
        && m.start.line as isize <= m.end.line as isize + line_end_delta
    {
        line_start_delta = 1;
    }
    LineRangeMapping {
        original: LineRange::new(o.start.line + line_start_delta, (o.end.line + 1).wrapping_add_signed(line_end_delta)),
        modified: LineRange::new(m.start.line + line_start_delta, (m.end.line + 1).wrapping_add_signed(line_end_delta)),
        inner_changes: Some(vec![*mapping]),
    }
}
