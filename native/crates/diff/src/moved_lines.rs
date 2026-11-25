//! `computeMovedLines.ts`.

use crate::algorithms::{myers, SequenceDiff, Timeout};
use crate::sequences::{is_space, LineRangeFragment, LinesSliceCharSequence};
use crate::{LineRange, LineRangeMapping, Range};
use std::collections::HashMap;

pub(crate) fn compute_moved_lines(changes: &[LineRangeMapping], original: &[String], modified: &[String], hashed_original: &[u32], hashed_modified: &[u32], timeout: &Timeout) -> Vec<LineRangeMapping> {
    let (mut moves, excluded) = moves_from_simple_deletions_to_simple_insertions(changes, original, modified, timeout);
    if !timeout.is_valid() {
        return Vec::new();
    }
    let filtered: Vec<LineRangeMapping> = changes.iter().enumerate().filter(|(i, _)| !excluded.contains(i)).map(|(_, c)| c.clone()).collect();
    moves.extend(unchanged_moves(filtered, hashed_original, hashed_modified, original, modified, timeout));
    let moves = join_close_consecutive_moves(moves);
    let moves = moves.into_iter().filter(|current| {
        let lines: Vec<&str> = original[current.original.start - 1..current.original.end - 1].iter().map(|l| l.trim()).collect();
        let text_length = lines.iter().map(|l| l.chars().count()).sum::<usize>() + lines.len().saturating_sub(1);
        text_length >= 15 && lines.iter().filter(|l| l.chars().count() >= 2).count() >= 2
    });
    remove_moves_in_same_diff(changes, moves.collect())
}

fn moves_from_simple_deletions_to_simple_insertions(changes: &[LineRangeMapping], original: &[String], modified: &[String], timeout: &Timeout) -> (Vec<LineRangeMapping>, Vec<usize>) {
    let mut moves = Vec::new();
    let deletions: Vec<LineRangeFragment> = changes.iter().enumerate().filter(|(_, c)| c.modified.is_empty() && c.original.len() >= 3).map(|(i, c)| LineRangeFragment::new(c.original, original, i)).collect();
    let mut insertions: Vec<LineRangeFragment> = changes.iter().enumerate().filter(|(_, c)| c.original.is_empty() && c.modified.len() >= 3).map(|(i, c)| LineRangeFragment::new(c.modified, modified, i)).collect();
    let mut excluded = Vec::new();
    for deletion in &deletions {
        let mut highest = -1.;
        let mut best = None;
        for (i, insertion) in insertions.iter().enumerate() {
            let similarity = deletion.similarity(insertion);
            if similarity > highest {
                highest = similarity;
                best = Some(i);
            }
        }
        if highest > 0.90
            && let Some(best) = best
        {
            let insertion = insertions.remove(best);
            moves.push(LineRangeMapping::new(deletion.range, insertion.range));
            excluded.push(deletion.source);
            excluded.push(insertion.source);
        }
        if !timeout.is_valid() {
            break;
        }
    }
    (moves, excluded)
}

struct PossibleMapping {
    modified: LineRange,
    original: LineRange,
}

fn unchanged_moves(mut changes: Vec<LineRangeMapping>, hashed_original: &[u32], hashed_modified: &[u32], original: &[String], modified: &[String], timeout: &Timeout) -> Vec<LineRangeMapping> {
    let mut moves = Vec::new();
    let mut original_3_line_hashes: HashMap<(u32, u32, u32), Vec<LineRange>> = HashMap::new();
    for change in &changes {
        for i in change.original.start..change.original.end.saturating_sub(2) {
            let key = (hashed_original[i - 1], hashed_original[i], hashed_original[i + 1]);
            original_3_line_hashes.entry(key).or_default().push(LineRange::new(i, i + 3));
        }
    }

    let mut possible: Vec<PossibleMapping> = Vec::new();
    changes.sort_by_key(|c| c.modified.start);
    for change in &changes {
        let mut last_mappings: Vec<usize> = Vec::new();
        for i in change.modified.start..change.modified.end.saturating_sub(2) {
            let key = (hashed_modified[i - 1], hashed_modified[i], hashed_modified[i + 1]);
            let current = LineRange::new(i, i + 3);
            let mut next_mappings = Vec::new();
            for range in original_3_line_hashes.get(&key).into_iter().flatten() {
                let extended = last_mappings.iter().copied().find(|&last| possible[last].original.end + 1 == range.end && possible[last].modified.end + 1 == current.end);
                if let Some(last) = extended {
                    let mapping = &mut possible[last];
                    mapping.original = LineRange::new(mapping.original.start, range.end);
                    mapping.modified = LineRange::new(mapping.modified.start, current.end);
                    next_mappings.push(last);
                    continue;
                }
                possible.push(PossibleMapping { modified: current, original: *range });
                next_mappings.push(possible.len() - 1);
            }
            last_mappings = next_mappings;
        }
        if !timeout.is_valid() {
            return Vec::new();
        }
    }

    possible.sort_by(|a, b| b.modified.len().cmp(&a.modified.len()));
    let mut modified_set = LineRangeSet::default();
    let mut original_set = LineRangeSet::default();
    for mapping in &possible {
        let diff = mapping.modified.start as isize - mapping.original.start as isize;
        let modified_sections = modified_set.subtract_from(mapping.modified);
        let original_translated = original_set.subtract_from(mapping.original).with_delta(diff);
        for s in modified_sections.intersection(&original_translated).ranges {
            if s.len() < 3 {
                continue;
            }
            let original_range = s.delta(-diff);
            moves.push(LineRangeMapping::new(original_range, s));
            modified_set.add(s);
            original_set.add(original_range);
        }
    }

    moves.sort_by_key(|m| m.original.start);
    for i in 0..moves.len() {
        let mv = moves[i].clone();
        let last_where = |f: &dyn Fn(&LineRangeMapping) -> bool| changes.iter().rposition(f).map(|i| &changes[i]).unwrap();
        let first_orig = last_where(&|c| c.original.start <= mv.original.start);
        let first_mod = last_where(&|c| c.modified.start <= mv.modified.start);
        let lines_above = (mv.original.start - first_orig.original.start).max(mv.modified.start - first_mod.modified.start);
        let last_orig = last_where(&|c| c.original.start < mv.original.end);
        let last_mod = last_where(&|c| c.modified.start < mv.modified.end);
        let lines_below = (last_orig.original.end as isize - mv.original.end as isize).max(last_mod.modified.end as isize - mv.modified.end as isize).max(0) as usize;

        let mut extend_to_top = 0;
        while extend_to_top < lines_above {
            let (orig_line, mod_line) = (mv.original.start as isize - extend_to_top as isize - 1, mv.modified.start as isize - extend_to_top as isize - 1);
            if orig_line < 1 || mod_line < 1 {
                break;
            }
            let (orig_line, mod_line) = (orig_line as usize, mod_line as usize);
            if modified_set.contains(mod_line) || original_set.contains(orig_line) || !are_lines_similar(&original[orig_line - 1], &modified[mod_line - 1], timeout) {
                break;
            }
            extend_to_top += 1;
        }
        if extend_to_top > 0 {
            original_set.add(LineRange::new(mv.original.start - extend_to_top, mv.original.start));
            modified_set.add(LineRange::new(mv.modified.start - extend_to_top, mv.modified.start));
        }
        let mut extend_to_bottom = 0;
        while extend_to_bottom < lines_below {
            let (orig_line, mod_line) = (mv.original.end + extend_to_bottom, mv.modified.end + extend_to_bottom);
            if orig_line > original.len() || mod_line > modified.len() {
                break;
            }
            if modified_set.contains(mod_line) || original_set.contains(orig_line) || !are_lines_similar(&original[orig_line - 1], &modified[mod_line - 1], timeout) {
                break;
            }
            extend_to_bottom += 1;
        }
        if extend_to_bottom > 0 {
            original_set.add(LineRange::new(mv.original.end, mv.original.end + extend_to_bottom));
            modified_set.add(LineRange::new(mv.modified.end, mv.modified.end + extend_to_bottom));
        }
        if extend_to_top > 0 || extend_to_bottom > 0 {
            moves[i] = LineRangeMapping::new(
                LineRange::new(mv.original.start - extend_to_top, mv.original.end + extend_to_bottom),
                LineRange::new(mv.modified.start - extend_to_top, mv.modified.end + extend_to_bottom),
            );
        }
    }
    moves
}

fn are_lines_similar(line1: &str, line2: &str, timeout: &Timeout) -> bool {
    if line1.trim() == line2.trim() {
        return true;
    }
    let (chars1, chars2): (Vec<char>, Vec<char>) = (line1.chars().collect(), line2.chars().collect());
    if chars1.len() > 300 && chars2.len() > 300 {
        return false;
    }
    // Upstream's ranges end at column `length`, so the last character of each line is left out.
    let result = myers(
        &LinesSliceCharSequence::new(&[line1.to_string()], Range::new(1, 1, 1, chars1.len()), false),
        &LinesSliceCharSequence::new(&[line2.to_string()], Range::new(1, 1, 1, chars2.len()), false),
        timeout,
    );
    let mut common_non_space = 0;
    for seq in SequenceDiff::invert(&result.diffs, chars1.len()) {
        for idx in seq.seq1.start..seq.seq1.end {
            if chars1.get(idx).is_some_and(|&c| !is_space(c as u32)) {
                common_non_space += 1;
            }
        }
    }
    let longer = if chars1.len() > chars2.len() { &chars1 } else { &chars2 };
    // Upstream counts up to line1's length in the longer line.
    let longer_length = longer.iter().take(chars1.len()).filter(|&&c| !is_space(c as u32)).count();
    common_non_space as f64 / longer_length as f64 > 0.6 && longer_length > 10
}

fn join_close_consecutive_moves(mut moves: Vec<LineRangeMapping>) -> Vec<LineRangeMapping> {
    if moves.is_empty() {
        return moves;
    }
    moves.sort_by_key(|m| m.original.start);
    let mut result = vec![moves[0].clone()];
    for current in &moves[1..] {
        let last = result.last_mut().unwrap();
        let original_dist = current.original.start as isize - last.original.end as isize;
        let modified_dist = current.modified.start as isize - last.modified.end as isize;
        if original_dist >= 0 && modified_dist >= 0 && original_dist + modified_dist <= 2 {
            *last = last.join(current);
            continue;
        }
        result.push(current.clone());
    }
    result
}

fn remove_moves_in_same_diff(changes: &[LineRangeMapping], moves: Vec<LineRangeMapping>) -> Vec<LineRangeMapping> {
    moves
        .into_iter()
        .filter(|m| {
            let before_original = changes.iter().rposition(|c| c.original.start < m.original.end);
            let before_modified = changes.iter().rposition(|c| c.modified.start < m.modified.end);
            before_original.is_none() || before_original != before_modified
        })
        .collect()
}

/// `LineRangeSet`: sorted, disjoint, non-touching line ranges.
#[derive(Default)]
struct LineRangeSet {
    ranges: Vec<LineRange>,
}

impl LineRangeSet {
    fn add(&mut self, range: LineRange) {
        if range.is_empty() {
            return;
        }
        let start = self.ranges.iter().position(|r| r.end >= range.start).unwrap_or(self.ranges.len());
        let end = self.ranges.iter().rposition(|r| r.start <= range.end).map_or(0, |i| i + 1);
        if start == end {
            self.ranges.insert(start, range);
        } else if start == end - 1 {
            self.ranges[start] = self.ranges[start].join(&range);
        } else {
            let joined = self.ranges[start].join(&self.ranges[end - 1]).join(&range);
            self.ranges.splice(start..end, [joined]);
        }
    }

    fn contains(&self, line: usize) -> bool {
        self.ranges.iter().rev().find(|r| r.start <= line).is_some_and(|r| r.end > line)
    }

    fn subtract_from(&self, range: LineRange) -> LineRangeSet {
        let start = self.ranges.iter().position(|r| r.end >= range.start).unwrap_or(self.ranges.len());
        let end = self.ranges.iter().rposition(|r| r.start <= range.end).map_or(0, |i| i + 1);
        if start == end {
            return LineRangeSet { ranges: vec![range] };
        }
        let mut result = Vec::new();
        let mut line = range.start;
        for r in &self.ranges[start..end] {
            if r.start > line {
                result.push(LineRange::new(line, r.start));
            }
            line = r.end;
        }
        if line < range.end {
            result.push(LineRange::new(line, range.end));
        }
        LineRangeSet { ranges: result }
    }

    fn intersection(&self, other: &LineRangeSet) -> LineRangeSet {
        let mut result = Vec::new();
        let (mut i1, mut i2) = (0, 0);
        while i1 < self.ranges.len() && i2 < other.ranges.len() {
            let (r1, r2) = (self.ranges[i1], other.ranges[i2]);
            if let Some(i) = r1.intersect(&r2)
                && !i.is_empty()
            {
                result.push(i);
            }
            if r1.end < r2.end {
                i1 += 1;
            } else {
                i2 += 1;
            }
        }
        LineRangeSet { ranges: result }
    }

    fn with_delta(&self, delta: isize) -> LineRangeSet {
        LineRangeSet { ranges: self.ranges.iter().map(|r| r.delta(delta)).collect() }
    }
}
