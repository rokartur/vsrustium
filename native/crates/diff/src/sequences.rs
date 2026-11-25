//! `lineSequence.ts`, `linesSliceCharSequence.ts` and `utils.ts`.

use crate::algorithms::Sequence;
use crate::{LineRange, OffsetRange, Position, Range};
use std::collections::HashMap;

/// Lines compared by the hash of their trimmed text.
pub(crate) struct LineSequence<'a> {
    pub trimmed_hash: &'a [u32],
    pub lines: &'a [String],
}

impl Sequence for LineSequence<'_> {
    fn element(&self, offset: usize) -> u32 {
        self.trimmed_hash[offset]
    }

    fn len(&self) -> usize {
        self.trimmed_hash.len()
    }

    fn boundary_score(&self, length: usize) -> i32 {
        let before = if length == 0 { 0 } else { indentation(&self.lines[length - 1]) };
        let after = if length == self.lines.len() { 0 } else { indentation(&self.lines[length]) };
        1000 - (before + after) as i32
    }

    fn strongly_equal(&self, offset1: usize, offset2: usize) -> bool {
        self.lines[offset1] == self.lines[offset2]
    }
}

impl LineSequence<'_> {
    pub fn text(&self, range: OffsetRange) -> String {
        self.lines[range.start..range.end].join("\n")
    }
}

fn indentation(line: &str) -> usize {
    line.chars().take_while(|&c| c == ' ' || c == '\t').count()
}

pub(crate) fn is_space(c: u32) -> bool {
    c == ' ' as u32 || c == '\t' as u32
}

/// The characters of a range of lines, `\n` between lines, trimmed per line unless whitespace counts.
pub(crate) struct LinesSliceCharSequence {
    elements: Vec<u32>,
    first_element_offset_by_line: Vec<usize>,
    line_start_offsets: Vec<usize>,
    trimmed_ws_lengths: Vec<usize>,
    range: Range,
}

impl LinesSliceCharSequence {
    pub fn new(lines: &[String], range: Range, consider_whitespace_changes: bool) -> Self {
        let mut sequence = LinesSliceCharSequence { elements: Vec::new(), first_element_offset_by_line: vec![0], line_start_offsets: Vec::new(), trimmed_ws_lengths: Vec::new(), range };
        for line_number in range.start.line..=range.end.line {
            let mut line: Vec<char> = lines[line_number - 1].chars().collect();
            let mut line_start_offset = 0;
            if line_number == range.start.line && range.start.column > 1 {
                line_start_offset = range.start.column - 1;
                line.drain(..line_start_offset.min(line.len()));
            }
            sequence.line_start_offsets.push(line_start_offset);
            let mut trimmed_ws_length = 0;
            if !consider_whitespace_changes {
                let leading = line.iter().take_while(|c| c.is_whitespace()).count();
                trimmed_ws_length = leading;
                line.drain(..leading);
                let trailing = line.iter().rev().take_while(|c| c.is_whitespace()).count();
                line.truncate(line.len() - trailing);
            }
            sequence.trimmed_ws_lengths.push(trimmed_ws_length);
            let line_length = if line_number == range.end.line {
                (range.end.column as isize - 1 - line_start_offset as isize - trimmed_ws_length as isize).min(line.len() as isize)
            } else {
                line.len() as isize
            };
            for c in line.iter().take(line_length.max(0) as usize) {
                sequence.elements.push(*c as u32);
            }
            if line_number < range.end.line {
                sequence.elements.push('\n' as u32);
                sequence.first_element_offset_by_line.push(sequence.elements.len());
            }
        }
        sequence
    }

    pub fn text(&self, range: OffsetRange) -> String {
        self.elements[range.start..range.end].iter().map(|&c| char::from_u32(c).unwrap()).collect()
    }

    /// `translateOffset`: `left` keeps a line start at column 1 rather than after the trimmed whitespace.
    pub fn translate_offset(&self, offset: usize, left: bool) -> Position {
        let i = self.first_element_offset_by_line.iter().rposition(|&value| value <= offset).unwrap();
        let line_offset = offset - self.first_element_offset_by_line[i];
        let trimmed = if line_offset == 0 && left { 0 } else { self.trimmed_ws_lengths[i] };
        Position { line: self.range.start.line + i, column: 1 + self.line_start_offsets[i] + line_offset + trimmed }
    }

    pub fn translate_range(&self, range: OffsetRange) -> Range {
        let start = self.translate_offset(range.start, false);
        let end = self.translate_offset(range.end, true);
        if end < start { Range { start: end, end } } else { Range { start, end } }
    }

    pub fn find_word_containing(&self, offset: usize) -> Option<OffsetRange> {
        if offset >= self.elements.len() || !is_word_char(self.elements[offset]) {
            return None;
        }
        let mut start = offset;
        while start > 0 && is_word_char(self.elements[start - 1]) {
            start -= 1;
        }
        let mut end = offset;
        while end < self.elements.len() && is_word_char(self.elements[end]) {
            end += 1;
        }
        Some(OffsetRange::new(start, end))
    }

    pub fn find_sub_word_containing(&self, offset: usize) -> Option<OffsetRange> {
        if offset >= self.elements.len() || !is_word_char(self.elements[offset]) {
            return None;
        }
        let mut start = offset;
        while start > 0 && is_word_char(self.elements[start - 1]) && !is_upper_case(self.elements[start]) {
            start -= 1;
        }
        let mut end = offset;
        while end < self.elements.len() && is_word_char(self.elements[end]) && !is_upper_case(self.elements[end]) {
            end += 1;
        }
        Some(OffsetRange::new(start, end))
    }

    pub fn count_lines_in(&self, range: OffsetRange) -> usize {
        self.translate_offset(range.end, false).line - self.translate_offset(range.start, false).line
    }

    pub fn extend_to_full_lines(&self, range: OffsetRange) -> OffsetRange {
        let starts = &self.first_element_offset_by_line;
        let start = starts.iter().rev().find(|&&x| x <= range.start).copied().unwrap_or(0);
        let end = starts.iter().find(|&&x| range.end <= x).copied().unwrap_or(self.elements.len());
        OffsetRange::new(start, end)
    }
}

impl Sequence for LinesSliceCharSequence {
    fn element(&self, offset: usize) -> u32 {
        self.elements[offset]
    }

    fn len(&self) -> usize {
        self.elements.len()
    }

    fn boundary_score(&self, length: usize) -> i32 {
        let previous = category(if length > 0 { Some(self.elements[length - 1]) } else { None });
        let next = category(self.elements.get(length).copied());
        if previous == Category::LineBreakCr && next == Category::LineBreakLf {
            return 0;
        }
        if previous == Category::LineBreakLf {
            return 150;
        }
        let mut score = 0;
        if previous != next {
            score += 10;
            if previous == Category::WordLower && next == Category::WordUpper {
                score += 1;
            }
        }
        score + category_score(previous) + category_score(next)
    }

    fn strongly_equal(&self, offset1: usize, offset2: usize) -> bool {
        self.elements[offset1] == self.elements[offset2]
    }
}

fn is_word_char(c: u32) -> bool {
    char::from_u32(c).is_some_and(|c| c.is_ascii_alphanumeric())
}

fn is_upper_case(c: u32) -> bool {
    char::from_u32(c).is_some_and(|c| c.is_ascii_uppercase())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    WordLower,
    WordUpper,
    WordNumber,
    End,
    Other,
    Separator,
    Space,
    LineBreakCr,
    LineBreakLf,
}

fn category_score(category: Category) -> i32 {
    match category {
        Category::WordLower | Category::WordUpper | Category::WordNumber => 0,
        Category::End => 10,
        Category::Other => 2,
        Category::Separator => 30,
        Category::Space => 3,
        Category::LineBreakCr | Category::LineBreakLf => 10,
    }
}

fn category(c: Option<u32>) -> Category {
    let Some(c) = c else { return Category::End };
    match char::from_u32(c).unwrap() {
        '\n' => Category::LineBreakLf,
        '\r' => Category::LineBreakCr,
        ' ' | '\t' => Category::Space,
        'a'..='z' => Category::WordLower,
        'A'..='Z' => Category::WordUpper,
        '0'..='9' => Category::WordNumber,
        ',' | ';' => Category::Separator,
        _ => Category::Other,
    }
}

/// `LineRangeFragment`: a range of lines with its character histogram, to find moved blocks.
pub(crate) struct LineRangeFragment {
    pub range: LineRange,
    pub source: usize,
    total: usize,
    histogram: HashMap<char, isize>,
}

impl LineRangeFragment {
    pub fn new(range: LineRange, lines: &[String], source: usize) -> Self {
        let mut histogram = HashMap::new();
        let mut total = 0;
        for line in &lines[range.start - 1..range.end - 1] {
            for c in line.chars().chain(std::iter::once('\n')) {
                total += 1;
                *histogram.entry(c).or_insert(0) += 1;
            }
        }
        LineRangeFragment { range, source, total, histogram }
    }

    pub fn similarity(&self, other: &LineRangeFragment) -> f64 {
        let mut differences = 0;
        for (c, count) in &self.histogram {
            differences += (count - other.histogram.get(c).unwrap_or(&0)).abs();
        }
        for (c, count) in &other.histogram {
            if !self.histogram.contains_key(c) {
                differences += count.abs();
            }
        }
        1. - differences as f64 / (self.total + other.total) as f64
    }
}
