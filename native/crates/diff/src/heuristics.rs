//! `heuristicSequenceOptimizations.ts`.

use crate::OffsetRange;
use crate::algorithms::{OffsetPair, Sequence, SequenceDiff};
use crate::sequences::{LineSequence, LinesSliceCharSequence};

pub(crate) fn optimize_sequence_diffs(seq1: &impl Sequence, seq2: &impl Sequence, diffs: Vec<SequenceDiff>) -> Vec<SequenceDiff> {
    let diffs = join_sequence_diffs_by_shifting(seq1, seq2, diffs);
    let diffs = join_sequence_diffs_by_shifting(seq1, seq2, diffs);
    shift_sequence_diffs(seq1, seq2, diffs)
}

/// Shifts insertions and deletions left, then right, to join them with their neighbors.
fn join_sequence_diffs_by_shifting(seq1: &impl Sequence, seq2: &impl Sequence, diffs: Vec<SequenceDiff>) -> Vec<SequenceDiff> {
    if diffs.is_empty() {
        return diffs;
    }
    let mut result = vec![diffs[0]];
    for &diff in &diffs[1..] {
        let previous = *result.last().unwrap();
        let mut cur = diff;
        if cur.seq1.is_empty() || cur.seq2.is_empty() {
            let length = cur.seq1.start - previous.seq1.end;
            let mut d = 1;
            while d <= length {
                if seq1.element(cur.seq1.start - d) != seq1.element(cur.seq1.end - d) || seq2.element(cur.seq2.start - d) != seq2.element(cur.seq2.end - d) {
                    break;
                }
                d += 1;
            }
            d -= 1;
            if d == length {
                *result.last_mut().unwrap() = SequenceDiff::new(OffsetRange::new(previous.seq1.start, cur.seq1.end - length), OffsetRange::new(previous.seq2.start, cur.seq2.end - length));
                continue;
            }
            cur = cur.delta(-(d as isize));
        }
        result.push(cur);
    }

    let mut result2 = Vec::with_capacity(result.len());
    for i in 0..result.len() - 1 {
        let next = result[i + 1];
        let mut cur = result[i];
        if cur.seq1.is_empty() || cur.seq2.is_empty() {
            let length = next.seq1.start - cur.seq1.end;
            let mut d = 0;
            while d < length {
                if !seq1.strongly_equal(cur.seq1.start + d, cur.seq1.end + d) || !seq2.strongly_equal(cur.seq2.start + d, cur.seq2.end + d) {
                    break;
                }
                d += 1;
            }
            if d == length {
                result[i + 1] = SequenceDiff::new(OffsetRange::new(cur.seq1.start + length, next.seq1.end), OffsetRange::new(cur.seq2.start + length, next.seq2.end));
                continue;
            }
            if d > 0 {
                cur = cur.delta(d as isize);
            }
        }
        result2.push(cur);
    }
    result2.push(*result.last().unwrap());
    result2
}

fn shift_sequence_diffs(seq1: &impl Sequence, seq2: &impl Sequence, mut diffs: Vec<SequenceDiff>) -> Vec<SequenceDiff> {
    for i in 0..diffs.len() {
        let previous = i.checked_sub(1).map(|i| diffs[i]);
        let diff = diffs[i];
        let next = diffs.get(i + 1).copied();
        let valid1 = (previous.map_or(0, |p| p.seq1.end + 1), next.map_or(seq1.len() as isize, |n| n.seq1.start as isize - 1));
        let valid2 = (previous.map_or(0, |p| p.seq2.end + 1), next.map_or(seq2.len() as isize, |n| n.seq2.start as isize - 1));
        if diff.seq1.is_empty() {
            diffs[i] = shift_diff_to_better_position(diff, seq1, seq2, valid1, valid2);
        } else if diff.seq2.is_empty() {
            diffs[i] = shift_diff_to_better_position(diff.swap(), seq2, seq1, valid2, valid1).swap();
        }
    }
    diffs
}

/// Moves an insertion (empty in `seq1`) to the offset with the best boundary score. Valid ranges are `(start, end)`
/// and may be empty or inverted at their edges, as `OffsetRange` never is upstream only because nothing reads them so.
fn shift_diff_to_better_position(diff: SequenceDiff, seq1: &impl Sequence, seq2: &impl Sequence, valid1: (usize, isize), valid2: (usize, isize)) -> SequenceDiff {
    const MAX_SHIFT_LIMIT: usize = 100;
    let mut delta_before = 1;
    while diff.seq1.start >= valid1.0 + delta_before
        && diff.seq2.start >= valid2.0 + delta_before
        && seq2.strongly_equal(diff.seq2.start - delta_before, diff.seq2.end - delta_before)
        && delta_before < MAX_SHIFT_LIMIT
    {
        delta_before += 1;
    }
    delta_before -= 1;
    let mut delta_after = 0;
    while ((diff.seq1.start + delta_after) as isize) < valid1.1
        && ((diff.seq2.end + delta_after) as isize) < valid2.1
        && seq2.strongly_equal(diff.seq2.start + delta_after, diff.seq2.end + delta_after)
        && delta_after < MAX_SHIFT_LIMIT
    {
        delta_after += 1;
    }
    if delta_before == 0 && delta_after == 0 {
        return diff;
    }
    let mut best_delta = 0;
    let mut best_score = -1;
    for delta in -(delta_before as isize)..=delta_after as isize {
        let score = seq1.boundary_score(diff.seq1.start.wrapping_add_signed(delta))
            + seq2.boundary_score(diff.seq2.start.wrapping_add_signed(delta))
            + seq2.boundary_score(diff.seq2.end.wrapping_add_signed(delta));
        if score > best_score {
            best_score = score;
            best_delta = delta;
        }
    }
    diff.delta(best_delta)
}

pub(crate) fn remove_short_matches(diffs: Vec<SequenceDiff>) -> Vec<SequenceDiff> {
    let mut result: Vec<SequenceDiff> = Vec::new();
    for s in diffs {
        match result.last_mut() {
            Some(last) if s.seq1.start - last.seq1.end <= 2 || s.seq2.start - last.seq2.end <= 2 => *last = last.join(&s),
            _ => result.push(s),
        }
    }
    result
}

/// `extendDiffsToEntireWordIfAppropriate`: a word mostly changed becomes one change. `force` takes any partly changed word.
pub(crate) fn extend_diffs_to_entire_word_if_appropriate(
    seq1: &LinesSliceCharSequence,
    seq2: &LinesSliceCharSequence,
    diffs: Vec<SequenceDiff>,
    find_parent: impl Fn(&LinesSliceCharSequence, usize) -> Option<OffsetRange>,
    force: bool,
) -> Vec<SequenceDiff> {
    let mut equal_mappings: std::collections::VecDeque<SequenceDiff> = SequenceDiff::invert(&diffs, seq1.len()).into();
    let mut additional = Vec::new();
    let mut last_point = OffsetPair::ZERO;

    let mut scan_word = |pair: OffsetPair, equal_mapping: SequenceDiff, equal_mappings: &mut std::collections::VecDeque<SequenceDiff>| {
        if pair.offset1 < last_point.offset1 || pair.offset2 < last_point.offset2 {
            return;
        }
        let (Some(w1), Some(w2)) = (find_parent(seq1, pair.offset1), find_parent(seq2, pair.offset2)) else { return };
        let mut w = SequenceDiff::new(w1, w2);
        let equal_part = w.intersect(&equal_mapping).unwrap();
        let mut equal_chars1 = equal_part.seq1.len();
        let mut equal_chars2 = equal_part.seq2.len();
        while let Some(&next) = equal_mappings.front() {
            if !(next.seq1.intersects(&w.seq1) || next.seq2.intersects(&w.seq2)) {
                break;
            }
            let v = SequenceDiff::new(find_parent(seq1, next.seq1.start).unwrap(), find_parent(seq2, next.seq2.start).unwrap());
            let equal_part = v.intersect(&next).unwrap();
            equal_chars1 += equal_part.seq1.len();
            equal_chars2 += equal_part.seq2.len();
            w = w.join(&v);
            if w.seq1.end >= next.seq1.end {
                equal_mappings.pop_front();
            } else {
                break;
            }
        }
        let total = w.seq1.len() + w.seq2.len();
        if (force && equal_chars1 + equal_chars2 < total) || ((equal_chars1 + equal_chars2) as f64) < total as f64 * 2. / 3. {
            additional.push(w);
        }
        last_point = w.end_exclusives();
    };

    while let Some(next) = equal_mappings.pop_front() {
        if next.seq1.is_empty() {
            continue;
        }
        scan_word(next.starts(), next, &mut equal_mappings);
        scan_word(next.end_exclusives().delta(-1), next, &mut equal_mappings);
    }
    merge_sequence_diffs(diffs, additional)
}

fn merge_sequence_diffs(diffs1: Vec<SequenceDiff>, diffs2: Vec<SequenceDiff>) -> Vec<SequenceDiff> {
    let mut result: Vec<SequenceDiff> = Vec::new();
    let mut a = diffs1.into_iter().peekable();
    let mut b = diffs2.into_iter().peekable();
    loop {
        let next = match (a.peek(), b.peek()) {
            (Some(x), Some(y)) if x.seq1.start < y.seq1.start => a.next().unwrap(),
            (Some(_), None) => a.next().unwrap(),
            (_, Some(_)) => b.next().unwrap(),
            (None, None) => break,
        };
        match result.last_mut() {
            Some(last) if last.seq1.end >= next.seq1.start => *last = last.join(&next),
            _ => result.push(next),
        }
    }
    result
}

pub(crate) fn remove_very_short_matching_lines_between_diffs(seq1: &LineSequence, diffs: Vec<SequenceDiff>) -> Vec<SequenceDiff> {
    repeat_joining(diffs, |before, after| {
        let unchanged = seq1.text(OffsetRange::new(before.seq1.end, after.seq1.start));
        let without_ws = unchanged.chars().filter(|c| !c.is_whitespace()).count();
        without_ws <= 4 && (before.seq1.len() + before.seq2.len() > 5 || after.seq1.len() + after.seq2.len() > 5)
    })
}

/// Joins neighbors `should_join` accepts, at most 11 passes, until a pass joins nothing.
fn repeat_joining(mut diffs: Vec<SequenceDiff>, should_join: impl Fn(&SequenceDiff, &SequenceDiff) -> bool) -> Vec<SequenceDiff> {
    if diffs.is_empty() {
        return diffs;
    }
    let mut counter = 0;
    loop {
        let mut repeat = false;
        let mut result = vec![diffs[0]];
        for &cur in &diffs[1..] {
            let last = result.last_mut().unwrap();
            if should_join(last, &cur) {
                repeat = true;
                *last = last.join(&cur);
            } else {
                result.push(cur);
            }
        }
        diffs = result;
        let again = counter < 10 && repeat;
        counter += 1;
        if !again {
            return diffs;
        }
    }
}

pub(crate) fn remove_very_short_matching_text_between_long_diffs(seq1: &LinesSliceCharSequence, seq2: &LinesSliceCharSequence, diffs: Vec<SequenceDiff>) -> Vec<SequenceDiff> {
    let diffs = repeat_joining(diffs, |before, after| {
        let unchanged_range = OffsetRange::new(before.seq1.end, after.seq1.start);
        if seq1.count_lines_in(unchanged_range) > 5 || unchanged_range.len() > 500 {
            return false;
        }
        let unchanged = seq1.text(unchanged_range);
        let unchanged = unchanged.trim();
        if unchanged.chars().count() > 20 || unchanged.contains(['\r', '\n']) {
            return false;
        }
        let max = 2. * 40. + 50.;
        let cap = |v: f64| v.min(max);
        let size = |d: &SequenceDiff| {
            let a = cap(seq1.count_lines_in(d.seq1) as f64 * 40. + d.seq1.len() as f64).powf(1.5);
            let b = cap(seq2.count_lines_in(d.seq2) as f64 * 40. + d.seq2.len() as f64).powf(1.5);
            (a + b).powf(1.5)
        };
        size(before) + size(after) > max.powf(1.5).powf(1.5) * 1.3
    });

    let mut new_diffs: Vec<SequenceDiff> = Vec::new();
    for (i, &cur) in diffs.iter().enumerate() {
        let should_mark_as_changed = |text: &str| !text.is_empty() && text.trim().chars().count() <= 3 && cur.seq1.len() + cur.seq2.len() > 100;
        let mut new_diff = cur;
        let full = seq1.extend_to_full_lines(cur.seq1);
        let prefix = seq1.text(OffsetRange::new(full.start, cur.seq1.start));
        if should_mark_as_changed(&prefix) {
            new_diff = new_diff.delta_start(-(prefix.chars().count() as isize));
        }
        let suffix = seq1.text(OffsetRange::new(cur.seq1.end, full.end));
        if should_mark_as_changed(&suffix) {
            new_diff = new_diff.delta_end(suffix.chars().count() as isize);
        }
        let available = SequenceDiff::from_offset_pairs(
            i.checked_sub(1).map_or(OffsetPair::ZERO, |p| diffs[p].end_exclusives()),
            diffs.get(i + 1).map_or(OffsetPair::MAX, |n| n.starts()),
        );
        let result = new_diff.intersect(&available).unwrap();
        match new_diffs.last_mut() {
            Some(last) if result.starts() == last.end_exclusives() => *last = last.join(&result),
            _ => new_diffs.push(result),
        }
    }
    new_diffs
}
