//! `defaultLinesDiffComputer.ts`, `linesDiffComputer.ts` and `verifyLinesDiff.ts`.

use crate::algorithms::{dynamic_programming, myers, SequenceDiff, Timeout};
use crate::heuristics::*;
use crate::moved_lines::compute_moved_lines;
use crate::sequences::{LineSequence, LinesSliceCharSequence};
use crate::{line_range_mappings_from_range_mappings, char_len, LineRange, LineRangeMapping, OffsetRange, Range, RangeMapping};
use std::collections::HashMap;

/// `ILinesDiffComputerOptions`.
#[derive(Clone, Copy, Debug, Default)]
pub struct DiffOptions {
    pub ignore_trim_whitespace: bool,
    /// 0: no limit.
    pub max_computation_time_ms: u64,
    pub compute_moves: bool,
    pub extend_to_subwords: bool,
}

/// `MovedText`.
#[derive(Clone, Debug, PartialEq)]
pub struct MovedText {
    pub line_range_mapping: LineRangeMapping,
    pub changes: Vec<LineRangeMapping>,
}

/// `LinesDiff`.
#[derive(Clone, Debug, PartialEq)]
pub struct LinesDiff {
    pub changes: Vec<LineRangeMapping>,
    pub moves: Vec<MovedText>,
    pub hit_timeout: bool,
}

/// `DefaultLinesDiffComputer.computeDiff`.
pub fn compute_diff(original: &[String], modified: &[String], options: DiffOptions) -> LinesDiff {
    if original.len() <= 1 && original == modified {
        return LinesDiff { changes: Vec::new(), moves: Vec::new(), hit_timeout: false };
    }
    if (original.len() == 1 && original[0].is_empty()) || (modified.len() == 1 && modified[0].is_empty()) {
        let mapping = RangeMapping {
            original: Range::new(1, 1, original.len(), char_len(&original[original.len() - 1]) + 1),
            modified: Range::new(1, 1, modified.len(), char_len(&modified[modified.len() - 1]) + 1),
        };
        let change = LineRangeMapping { original: LineRange::new(1, original.len() + 1), modified: LineRange::new(1, modified.len() + 1), inner_changes: Some(vec![mapping]) };
        return LinesDiff { changes: vec![change], moves: Vec::new(), hit_timeout: false };
    }

    let timeout = Timeout::new(options.max_computation_time_ms);
    let consider_whitespace_changes = !options.ignore_trim_whitespace;
    let mut hashes: HashMap<&str, u32> = HashMap::new();
    let original_hashes: Vec<u32> = original.iter().map(|l| get_or_create_hash(&mut hashes, l.trim())).collect();
    let modified_hashes: Vec<u32> = modified.iter().map(|l| get_or_create_hash(&mut hashes, l.trim())).collect();
    let seq1 = LineSequence { trimmed_hash: &original_hashes, lines: original };
    let seq2 = LineSequence { trimmed_hash: &modified_hashes, lines: modified };

    let alignment = if original.len() + modified.len() < 1700 {
        let score = |o: usize, m: usize| {
            if original[o] == modified[m] {
                if modified[m].is_empty() { 0.1 } else { 1. + (1. + utf16_len(&modified[m]) as f64).ln() }
            } else {
                0.99
            }
        };
        dynamic_programming(&seq1, &seq2, &timeout, Some(&score))
    } else {
        myers(&seq1, &seq2, &timeout)
    };
    let mut hit_timeout = alignment.hit_timeout;
    let line_alignments = optimize_sequence_diffs(&seq1, &seq2, alignment.diffs);
    let line_alignments = remove_very_short_matching_lines_between_diffs(&seq1, line_alignments);

    let mut alignments: Vec<RangeMapping> = Vec::new();
    let scan_for_whitespace_changes = |start1: usize, start2: usize, count: usize, alignments: &mut Vec<RangeMapping>, hit_timeout: &mut bool| {
        if !consider_whitespace_changes {
            return;
        }
        for i in 0..count {
            if original[start1 + i] != modified[start2 + i] {
                let diff = SequenceDiff::new(OffsetRange::new(start1 + i, start1 + i + 1), OffsetRange::new(start2 + i, start2 + i + 1));
                let (mappings, timed_out) = refine_diff(original, modified, diff, &timeout, consider_whitespace_changes, options);
                alignments.extend(mappings);
                *hit_timeout |= timed_out;
            }
        }
    };
    let (mut last1, mut last2) = (0, 0);
    for diff in &line_alignments {
        scan_for_whitespace_changes(last1, last2, diff.seq1.start - last1, &mut alignments, &mut hit_timeout);
        last1 = diff.seq1.end;
        last2 = diff.seq2.end;
        let (mappings, timed_out) = refine_diff(original, modified, *diff, &timeout, consider_whitespace_changes, options);
        hit_timeout |= timed_out;
        alignments.extend(mappings);
    }
    scan_for_whitespace_changes(last1, last2, original.len() - last1, &mut alignments, &mut hit_timeout);

    let changes = line_range_mappings_from_range_mappings(&alignments, original, modified);
    let moves = if options.compute_moves {
        compute_moved_lines(&changes, original, modified, &original_hashes, &modified_hashes, &timeout)
            .into_iter()
            .map(|m| {
                let diff = SequenceDiff::new(m.original.to_offset_range(), m.modified.to_offset_range());
                let (mappings, _) = refine_diff(original, modified, diff, &timeout, consider_whitespace_changes, options);
                MovedText { changes: line_range_mappings_from_range_mappings(&mappings, original, modified), line_range_mapping: m }
            })
            .collect()
    } else {
        Vec::new()
    };
    LinesDiff { changes, moves, hit_timeout }
}

/// `getOrCreateHash`: the next number for each new text.
fn get_or_create_hash<'a>(hashes: &mut HashMap<&'a str, u32>, text: &'a str) -> u32 {
    let next = hashes.len() as u32;
    *hashes.entry(text).or_insert(next)
}

/// The DP's equality score takes upstream's UTF-16 length.
fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `refineDiff`: the character changes inside changed lines.
fn refine_diff(original: &[String], modified: &[String], diff: SequenceDiff, timeout: &Timeout, consider_whitespace_changes: bool, options: DiffOptions) -> (Vec<RangeMapping>, bool) {
    let lines = LineRangeMapping::new(LineRange::new(diff.seq1.start + 1, diff.seq1.end + 1), LineRange::new(diff.seq2.start + 1, diff.seq2.end + 1));
    let range = lines.to_range_mapping2(original, modified);
    let slice1 = LinesSliceCharSequence::new(original, range.original, consider_whitespace_changes);
    let slice2 = LinesSliceCharSequence::new(modified, range.modified, consider_whitespace_changes);
    use crate::algorithms::Sequence;
    let result = if slice1.len() + slice2.len() < 500 { dynamic_programming(&slice1, &slice2, timeout, None) } else { myers(&slice1, &slice2, timeout) };
    let mut diffs = optimize_sequence_diffs(&slice1, &slice2, result.diffs);
    diffs = extend_diffs_to_entire_word_if_appropriate(&slice1, &slice2, diffs, |seq, idx| seq.find_word_containing(idx), false);
    if options.extend_to_subwords {
        diffs = extend_diffs_to_entire_word_if_appropriate(&slice1, &slice2, diffs, |seq, idx| seq.find_sub_word_containing(idx), true);
    }
    diffs = remove_short_matches(diffs);
    diffs = remove_very_short_matching_text_between_long_diffs(&slice1, &slice2, diffs);
    let mappings = diffs.iter().map(|d| RangeMapping { original: slice1.translate_range(d.seq1), modified: slice2.translate_range(d.seq2) }).collect();
    (mappings, result.hit_timeout)
}

/// `LinesDiffValidity`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Validity {
    Valid,
    Invalid,
    Unverified,
}

/// `verifyLinesDiff`: applies the character changes to the original and compares with the modified lines.
pub fn verify_lines_diff(diff: &LinesDiff, original: &[String], modified: &[String], ignore_trim_whitespace: bool) -> Validity {
    let mut replacements = Vec::new();
    for change in &diff.changes {
        let Some(inner) = &change.inner_changes else { return Validity::Unverified };
        replacements.extend(inner.iter().copied());
    }
    let Some(result) = apply_range_mappings(&replacements, original, modified) else { return Validity::Invalid };
    let result_lines: Vec<&str> = result.split('\n').collect();
    let valid = if ignore_trim_whitespace {
        result_lines.len() == modified.len() && result_lines.iter().zip(modified).all(|(a, b)| a.trim() == b.trim())
    } else {
        result == modified.join("\n")
    };
    if valid { Validity::Valid } else { Validity::Invalid }
}

/// Replaces each mapping's original range by the modified range's text; None when ranges overlap or fall outside.
pub fn apply_range_mappings(mappings: &[RangeMapping], original: &[String], modified: &[String]) -> Option<String> {
    let offsets = |lines: &[String]| {
        let mut starts = Vec::with_capacity(lines.len());
        let mut offset = 0;
        for line in lines {
            starts.push(offset);
            offset += char_len(line) + 1;
        }
        starts
    };
    let (original_starts, modified_starts) = (offsets(original), offsets(modified));
    let to_offset = |starts: &[usize], lines: &[String], line: usize, column: usize| -> Option<usize> {
        let text = lines.get(line.checked_sub(1)?)?;
        (column >= 1 && column <= char_len(text) + 1).then(|| starts[line - 1] + column - 1)
    };
    let original_text: Vec<char> = original.join("\n").chars().collect();
    let modified_text: Vec<char> = modified.join("\n").chars().collect();
    let mut sorted: Vec<&RangeMapping> = mappings.iter().collect();
    sorted.sort_by_key(|m| m.original.start);
    let mut result = String::new();
    let mut position = 0;
    for m in sorted {
        let start = to_offset(&original_starts, original, m.original.start.line, m.original.start.column)?;
        let end = to_offset(&original_starts, original, m.original.end.line, m.original.end.column)?;
        let new_start = to_offset(&modified_starts, modified, m.modified.start.line, m.modified.start.column)?;
        let new_end = to_offset(&modified_starts, modified, m.modified.end.line, m.modified.end.column)?;
        if start < position {
            return None;
        }
        result.extend(&original_text[position..start]);
        result.extend(&modified_text[new_start..new_end]);
        position = end;
    }
    result.extend(&original_text[position..]);
    Some(result)
}
