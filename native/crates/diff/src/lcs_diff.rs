//! `LcsDiff` of base/common/diff/diff.ts over `Int32Array` elements, ported 1:1 so callers such as inline
//! completions' `smartDiff` get upstream's exact tie-breaking, not just any shortest edit script.
//! Indices are `isize` because upstream walks one element outside the bounds (`originalStart - 1`).

/// `DiffChange` of base/common/diff/diffChange.ts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffChange {
    pub original_start: usize,
    pub original_length: usize,
    pub modified_start: usize,
    pub modified_length: usize,
}

/// `new LcsDiff(original, modified).ComputeDiff(pretty).changes` without a `continueProcessingPredicate`.
pub fn lcs_diff(original: &[i32], modified: &[i32], pretty: bool) -> Vec<DiffChange> {
    let mut diff = LcsDiff { original, modified, forward_history: Vec::new(), reverse_history: Vec::new() };
    let mut changes = diff.compute_diff_recursive(0, original.len() as isize - 1, 0, modified.len() as isize - 1);
    if pretty {
        changes = diff.prettify_changes(changes);
    }
    changes
        .into_iter()
        .map(|change| DiffChange {
            original_start: change.original_start as usize,
            original_length: change.original_length as usize,
            modified_start: change.modified_start as usize,
            modified_length: change.modified_length as usize,
        })
        .collect()
}

/// `LocalConstants.MaxDifferencesHistory`.
const MAX_DIFFERENCES_HISTORY: isize = 1447;

#[derive(Clone, Copy, Debug)]
struct Change {
    original_start: isize,
    original_length: isize,
    modified_start: isize,
    modified_length: isize,
}

impl Change {
    fn new(original_start: isize, original_length: isize, modified_start: isize, modified_length: isize) -> Self {
        Change { original_start, original_length, modified_start, modified_length }
    }
}

/// `DiffChangeHelper`.
struct DiffChangeHelper {
    changes: Vec<Change>,
    original_start: isize,
    modified_start: isize,
    original_count: isize,
    modified_count: isize,
}

impl DiffChangeHelper {
    fn new() -> Self {
        DiffChangeHelper {
            changes: Vec::new(),
            original_start: isize::MAX,
            modified_start: isize::MAX,
            original_count: 0,
            modified_count: 0,
        }
    }

    fn mark_next_change(&mut self) {
        if self.original_count > 0 || self.modified_count > 0 {
            self.changes.push(Change::new(
                self.original_start,
                self.original_count,
                self.modified_start,
                self.modified_count,
            ));
        }
        self.original_count = 0;
        self.modified_count = 0;
        self.original_start = isize::MAX;
        self.modified_start = isize::MAX;
    }

    fn add_original_element(&mut self, original_index: isize, modified_index: isize) {
        self.original_start = self.original_start.min(original_index);
        self.modified_start = self.modified_start.min(modified_index);
        self.original_count += 1;
    }

    fn add_modified_element(&mut self, original_index: isize, modified_index: isize) {
        self.original_start = self.original_start.min(original_index);
        self.modified_start = self.modified_start.min(modified_index);
        self.modified_count += 1;
    }

    fn into_changes(mut self) -> Vec<Change> {
        if self.original_count > 0 || self.modified_count > 0 {
            self.mark_next_change();
        }
        self.changes
    }

    fn into_reverse_changes(self) -> Vec<Change> {
        let mut changes = self.into_changes();
        changes.reverse();
        changes
    }
}

/// `LcsDiff`: Myers' O(ND) algorithm with the divide-and-conquer recursion point and history walk of the C# original.
struct LcsDiff<'a> {
    original: &'a [i32],
    modified: &'a [i32],
    forward_history: Vec<Vec<isize>>,
    reverse_history: Vec<Vec<isize>>,
}

/// Where `ComputeRecursionPoint` found the overlap of the forward and reverse searches.
struct Trace<'p> {
    diagonal_forward_base: isize,
    diagonal_forward_start: isize,
    diagonal_forward_end: isize,
    diagonal_forward_offset: isize,
    diagonal_reverse_base: isize,
    diagonal_reverse_start: isize,
    diagonal_reverse_end: isize,
    diagonal_reverse_offset: isize,
    forward_points: &'p [isize],
    reverse_points: &'p [isize],
    mid_original: isize,
    mid_modified: isize,
    delta_is_even: bool,
}

enum RecursionPoint {
    Changes(Vec<Change>),
    Split { mid_original: isize, mid_modified: isize },
}

impl LcsDiff<'_> {
    /// `ElementsAreEqual` (and `ElementsAreStrictEqual`: int sequences have no strict element).
    fn elements_are_equal(&self, original_index: isize, new_index: isize) -> bool {
        self.original[original_index as usize] == self.modified[new_index as usize]
    }

    /// `OriginalElementsAreEqual`.
    fn original_elements_are_equal(&self, index1: isize, index2: isize) -> bool {
        self.original[index1 as usize] == self.original[index2 as usize]
    }

    /// `ModifiedElementsAreEqual`.
    fn modified_elements_are_equal(&self, index1: isize, index2: isize) -> bool {
        self.modified[index1 as usize] == self.modified[index2 as usize]
    }

    /// `ComputeDiffRecursive`; bounds are inclusive.
    fn compute_diff_recursive(
        &mut self,
        mut original_start: isize,
        mut original_end: isize,
        mut modified_start: isize,
        mut modified_end: isize,
    ) -> Vec<Change> {
        while original_start <= original_end
            && modified_start <= modified_end
            && self.elements_are_equal(original_start, modified_start)
        {
            original_start += 1;
            modified_start += 1;
        }
        while original_end >= original_start
            && modified_end >= modified_start
            && self.elements_are_equal(original_end, modified_end)
        {
            original_end -= 1;
            modified_end -= 1;
        }

        if original_start > original_end || modified_start > modified_end {
            if modified_start <= modified_end {
                assert!(original_start == original_end + 1, "originalStart should only be one more than originalEnd");
                return vec![Change::new(original_start, 0, modified_start, modified_end - modified_start + 1)];
            }
            if original_start <= original_end {
                assert!(modified_start == modified_end + 1, "modifiedStart should only be one more than modifiedEnd");
                return vec![Change::new(original_start, original_end - original_start + 1, modified_start, 0)];
            }
            assert!(original_start == original_end + 1, "originalStart should only be one more than originalEnd");
            assert!(modified_start == modified_end + 1, "modifiedStart should only be one more than modifiedEnd");
            return Vec::new();
        }

        match self.compute_recursion_point(original_start, original_end, modified_start, modified_end) {
            RecursionPoint::Changes(changes) => changes,
            RecursionPoint::Split { mid_original, mid_modified } => {
                let left_changes = self.compute_diff_recursive(original_start, mid_original, modified_start, mid_modified);
                let right_changes =
                    self.compute_diff_recursive(mid_original + 1, original_end, mid_modified + 1, modified_end);
                concatenate_changes(left_changes, right_changes)
            }
        }
    }

    /// `WALKTRACE` without the quit-early branch.
    fn walk_trace(&self, trace: Trace) -> Vec<Change> {
        let mut change_helper = DiffChangeHelper::new();
        let mut diagonal_forward_base = trace.diagonal_forward_base;
        let mut forward_points = trace.forward_points;
        let mut diagonal_min = trace.diagonal_forward_start;
        let mut diagonal_max = trace.diagonal_forward_end;
        let mut diagonal_relative = (trace.mid_original - trace.mid_modified) - trace.diagonal_forward_offset;
        let mut last_original_index = isize::MIN;
        let mut history_index = self.forward_history.len() as isize - 1;

        loop {
            let diagonal = diagonal_relative + diagonal_forward_base;
            let d = diagonal as usize;
            if diagonal == diagonal_min || (diagonal < diagonal_max && forward_points[d - 1] < forward_points[d + 1]) {
                let original_index = forward_points[d + 1];
                let modified_index = original_index - diagonal_relative - trace.diagonal_forward_offset;
                if original_index < last_original_index {
                    change_helper.mark_next_change();
                }
                last_original_index = original_index;
                change_helper.add_modified_element(original_index + 1, modified_index);
                diagonal_relative = (diagonal + 1) - diagonal_forward_base;
            } else {
                let original_index = forward_points[d - 1] + 1;
                let modified_index = original_index - diagonal_relative - trace.diagonal_forward_offset;
                if original_index < last_original_index {
                    change_helper.mark_next_change();
                }
                last_original_index = original_index - 1;
                change_helper.add_original_element(original_index, modified_index + 1);
                diagonal_relative = (diagonal - 1) - diagonal_forward_base;
            }

            if history_index >= 0 {
                forward_points = &self.forward_history[history_index as usize];
                diagonal_forward_base = forward_points[0];
                diagonal_min = 1;
                diagonal_max = forward_points.len() as isize - 1;
            }
            history_index -= 1;
            if history_index < -1 {
                break;
            }
        }
        let forward_changes = change_helper.into_reverse_changes();

        let mut change_helper = DiffChangeHelper::new();
        let mut diagonal_reverse_base = trace.diagonal_reverse_base;
        let mut reverse_points = trace.reverse_points;
        diagonal_min = trace.diagonal_reverse_start;
        diagonal_max = trace.diagonal_reverse_end;
        diagonal_relative = (trace.mid_original - trace.mid_modified) - trace.diagonal_reverse_offset;
        last_original_index = isize::MAX;
        history_index = self.reverse_history.len() as isize - if trace.delta_is_even { 1 } else { 2 };

        loop {
            let diagonal = diagonal_relative + diagonal_reverse_base;
            let d = diagonal as usize;
            if diagonal == diagonal_min || (diagonal < diagonal_max && reverse_points[d - 1] >= reverse_points[d + 1]) {
                let original_index = reverse_points[d + 1] - 1;
                let modified_index = original_index - diagonal_relative - trace.diagonal_reverse_offset;
                if original_index > last_original_index {
                    change_helper.mark_next_change();
                }
                last_original_index = original_index + 1;
                change_helper.add_original_element(original_index + 1, modified_index + 1);
                diagonal_relative = (diagonal + 1) - diagonal_reverse_base;
            } else {
                let original_index = reverse_points[d - 1];
                let modified_index = original_index - diagonal_relative - trace.diagonal_reverse_offset;
                if original_index > last_original_index {
                    change_helper.mark_next_change();
                }
                last_original_index = original_index;
                change_helper.add_modified_element(original_index + 1, modified_index + 1);
                diagonal_relative = (diagonal - 1) - diagonal_reverse_base;
            }

            if history_index >= 0 {
                reverse_points = &self.reverse_history[history_index as usize];
                diagonal_reverse_base = reverse_points[0];
                diagonal_min = 1;
                diagonal_max = reverse_points.len() as isize - 1;
            }
            history_index -= 1;
            if history_index < -1 {
                break;
            }
        }
        let reverse_changes = change_helper.into_changes();

        concatenate_changes(forward_changes, reverse_changes)
    }

    /// `ComputeRecursionPoint`: the full change list when the trace fits in the history, else the mid point to split on.
    fn compute_recursion_point(
        &mut self,
        mut original_start: isize,
        original_end: isize,
        mut modified_start: isize,
        modified_end: isize,
    ) -> RecursionPoint {
        let mut diagonal_forward_start = 0;
        let mut diagonal_forward_end = 0;
        let mut diagonal_reverse_start = 0;
        let mut diagonal_reverse_end = 0;

        original_start -= 1;
        modified_start -= 1;

        self.forward_history.clear();
        self.reverse_history.clear();

        let max_differences = (original_end - original_start) + (modified_end - modified_start);
        let num_diagonals = max_differences + 1;
        let mut forward_points = vec![0isize; num_diagonals as usize];
        let mut reverse_points = vec![0isize; num_diagonals as usize];
        let diagonal_forward_base = modified_end - modified_start;
        let diagonal_reverse_base = original_end - original_start;
        let diagonal_forward_offset = original_start - modified_start;
        let diagonal_reverse_offset = original_end - modified_end;

        let delta = diagonal_reverse_base - diagonal_forward_base;
        let delta_is_even = delta % 2 == 0;

        forward_points[diagonal_forward_base as usize] = original_start;
        reverse_points[diagonal_reverse_base as usize] = original_end;

        let mut mid_original = 0;
        let mut mid_modified = 0;

        // Upstream loops while `numDifferences <= (maxDifferences / 2) + 1` in float division.
        let mut num_differences: isize = 1;
        while 2 * num_differences <= max_differences + 2 {
            diagonal_forward_start =
                clip_diagonal_bound(diagonal_forward_base - num_differences, num_differences, diagonal_forward_base, num_diagonals);
            diagonal_forward_end =
                clip_diagonal_bound(diagonal_forward_base + num_differences, num_differences, diagonal_forward_base, num_diagonals);
            let mut diagonal = diagonal_forward_start;
            while diagonal <= diagonal_forward_end {
                let d = diagonal as usize;
                let mut original_index = if diagonal == diagonal_forward_start
                    || (diagonal < diagonal_forward_end && forward_points[d - 1] < forward_points[d + 1])
                {
                    forward_points[d + 1]
                } else {
                    forward_points[d - 1] + 1
                };
                let mut modified_index = original_index - (diagonal - diagonal_forward_base) - diagonal_forward_offset;
                let temp_original_index = original_index;

                while original_index < original_end
                    && modified_index < modified_end
                    && self.elements_are_equal(original_index + 1, modified_index + 1)
                {
                    original_index += 1;
                    modified_index += 1;
                }
                forward_points[d] = original_index;

                if !delta_is_even
                    && (diagonal - diagonal_reverse_base).abs() < num_differences
                    && original_index >= reverse_points[d]
                {
                    mid_original = original_index;
                    mid_modified = modified_index;
                    if temp_original_index <= reverse_points[d] && num_differences <= MAX_DIFFERENCES_HISTORY + 1 {
                        return RecursionPoint::Changes(self.walk_trace(Trace {
                            diagonal_forward_base,
                            diagonal_forward_start,
                            diagonal_forward_end,
                            diagonal_forward_offset,
                            diagonal_reverse_base,
                            diagonal_reverse_start,
                            diagonal_reverse_end,
                            diagonal_reverse_offset,
                            forward_points: &forward_points,
                            reverse_points: &reverse_points,
                            mid_original,
                            mid_modified,
                            delta_is_even,
                        }));
                    }
                    return RecursionPoint::Split { mid_original, mid_modified };
                }
                diagonal += 2;
            }

            diagonal_reverse_start =
                clip_diagonal_bound(diagonal_reverse_base - num_differences, num_differences, diagonal_reverse_base, num_diagonals);
            diagonal_reverse_end =
                clip_diagonal_bound(diagonal_reverse_base + num_differences, num_differences, diagonal_reverse_base, num_diagonals);
            let mut diagonal = diagonal_reverse_start;
            while diagonal <= diagonal_reverse_end {
                let d = diagonal as usize;
                let mut original_index = if diagonal == diagonal_reverse_start
                    || (diagonal < diagonal_reverse_end && reverse_points[d - 1] >= reverse_points[d + 1])
                {
                    reverse_points[d + 1] - 1
                } else {
                    reverse_points[d - 1]
                };
                let mut modified_index = original_index - (diagonal - diagonal_reverse_base) - diagonal_reverse_offset;
                let temp_original_index = original_index;

                while original_index > original_start
                    && modified_index > modified_start
                    && self.elements_are_equal(original_index, modified_index)
                {
                    original_index -= 1;
                    modified_index -= 1;
                }
                reverse_points[d] = original_index;

                if delta_is_even
                    && (diagonal - diagonal_forward_base).abs() <= num_differences
                    && original_index <= forward_points[d]
                {
                    mid_original = original_index;
                    mid_modified = modified_index;
                    if temp_original_index >= forward_points[d] && num_differences <= MAX_DIFFERENCES_HISTORY + 1 {
                        return RecursionPoint::Changes(self.walk_trace(Trace {
                            diagonal_forward_base,
                            diagonal_forward_start,
                            diagonal_forward_end,
                            diagonal_forward_offset,
                            diagonal_reverse_base,
                            diagonal_reverse_start,
                            diagonal_reverse_end,
                            diagonal_reverse_offset,
                            forward_points: &forward_points,
                            reverse_points: &reverse_points,
                            mid_original,
                            mid_modified,
                            delta_is_even,
                        }));
                    }
                    return RecursionPoint::Split { mid_original, mid_modified };
                }
                diagonal += 2;
            }

            if num_differences <= MAX_DIFFERENCES_HISTORY {
                // Slot 0 holds the diagonal base index, the rest copy the computed diagonals.
                let mut temp = vec![diagonal_forward_base - diagonal_forward_start + 1];
                temp.extend_from_slice(&forward_points[diagonal_forward_start as usize..=diagonal_forward_end as usize]);
                self.forward_history.push(temp);

                let mut temp = vec![diagonal_reverse_base - diagonal_reverse_start + 1];
                temp.extend_from_slice(&reverse_points[diagonal_reverse_start as usize..=diagonal_reverse_end as usize]);
                self.reverse_history.push(temp);
            }
            num_differences += 1;
        }

        RecursionPoint::Changes(self.walk_trace(Trace {
            diagonal_forward_base,
            diagonal_forward_start,
            diagonal_forward_end,
            diagonal_forward_offset,
            diagonal_reverse_base,
            diagonal_reverse_start,
            diagonal_reverse_end,
            diagonal_reverse_offset,
            forward_points: &forward_points,
            reverse_points: &reverse_points,
            mid_original,
            mid_modified,
            delta_is_even,
        }))
    }

    /// `PrettifyChanges` without the string-only longest-line pass.
    fn prettify_changes(&self, mut changes: Vec<Change>) -> Vec<Change> {
        let mut i: isize = 0;
        while i < changes.len() as isize {
            let iu = i as usize;
            let original_stop =
                if iu < changes.len() - 1 { changes[iu + 1].original_start } else { self.original.len() as isize };
            let modified_stop =
                if iu < changes.len() - 1 { changes[iu + 1].modified_start } else { self.modified.len() as isize };
            let change = &mut changes[iu];
            let check_original = change.original_length > 0;
            let check_modified = change.modified_length > 0;

            while change.original_start + change.original_length < original_stop
                && change.modified_start + change.modified_length < modified_stop
                && (!check_original
                    || self.original_elements_are_equal(change.original_start, change.original_start + change.original_length))
                && (!check_modified
                    || self.modified_elements_are_equal(change.modified_start, change.modified_start + change.modified_length))
            {
                let start_strict_equal = self.elements_are_equal(change.original_start, change.modified_start);
                let end_strict_equal = self.elements_are_equal(
                    change.original_start + change.original_length,
                    change.modified_start + change.modified_length,
                );
                if end_strict_equal && !start_strict_equal {
                    break;
                }
                change.original_start += 1;
                change.modified_start += 1;
            }

            if iu < changes.len() - 1
                && let Some(merged) = changes_overlap(&changes[iu], &changes[iu + 1])
            {
                changes[iu] = merged;
                changes.remove(iu + 1);
                i -= 1;
            }
            i += 1;
        }

        let mut i = changes.len() as isize - 1;
        while i >= 0 {
            let iu = i as usize;
            let (original_stop, modified_stop) = if iu > 0 {
                let prev_change = changes[iu - 1];
                (prev_change.original_start + prev_change.original_length, prev_change.modified_start + prev_change.modified_length)
            } else {
                (0, 0)
            };
            let change = changes[iu];
            let check_original = change.original_length > 0;
            let check_modified = change.modified_length > 0;

            let mut best_delta = 0;
            let mut best_score =
                self.boundary_score(change.original_start, change.original_length, change.modified_start, change.modified_length);

            let mut delta = 1;
            loop {
                let original_start = change.original_start - delta;
                let modified_start = change.modified_start - delta;
                if original_start < original_stop || modified_start < modified_stop {
                    break;
                }
                if check_original && !self.original_elements_are_equal(original_start, original_start + change.original_length) {
                    break;
                }
                if check_modified && !self.modified_elements_are_equal(modified_start, modified_start + change.modified_length) {
                    break;
                }
                let touching_previous_change = original_start == original_stop && modified_start == modified_stop;
                let score = if touching_previous_change { 5 } else { 0 }
                    + self.boundary_score(original_start, change.original_length, modified_start, change.modified_length);
                if score > best_score {
                    best_score = score;
                    best_delta = delta;
                }
                delta += 1;
            }

            changes[iu].original_start -= best_delta;
            changes[iu].modified_start -= best_delta;

            if iu > 0
                && let Some(merged) = changes_overlap(&changes[iu - 1], &changes[iu])
            {
                changes[iu - 1] = merged;
                changes.remove(iu);
                i += 1;
            }
            i -= 1;
        }

        changes
    }

    /// `_OriginalIsBoundary`: without strings only the sequence ends count.
    fn original_is_boundary(&self, index: isize) -> bool {
        index <= 0 || index >= self.original.len() as isize - 1
    }

    /// `_OriginalRegionIsBoundary`.
    fn original_region_is_boundary(&self, original_start: isize, original_length: isize) -> bool {
        if self.original_is_boundary(original_start) || self.original_is_boundary(original_start - 1) {
            return true;
        }
        if original_length > 0 {
            let original_end = original_start + original_length;
            if self.original_is_boundary(original_end - 1) || self.original_is_boundary(original_end) {
                return true;
            }
        }
        false
    }

    /// `_ModifiedIsBoundary`: without strings only the sequence ends count.
    fn modified_is_boundary(&self, index: isize) -> bool {
        index <= 0 || index >= self.modified.len() as isize - 1
    }

    /// `_ModifiedRegionIsBoundary`.
    fn modified_region_is_boundary(&self, modified_start: isize, modified_length: isize) -> bool {
        if self.modified_is_boundary(modified_start) || self.modified_is_boundary(modified_start - 1) {
            return true;
        }
        if modified_length > 0 {
            let modified_end = modified_start + modified_length;
            if self.modified_is_boundary(modified_end - 1) || self.modified_is_boundary(modified_end) {
                return true;
            }
        }
        false
    }

    /// `_boundaryScore`.
    fn boundary_score(&self, original_start: isize, original_length: isize, modified_start: isize, modified_length: isize) -> isize {
        let original_score = self.original_region_is_boundary(original_start, original_length) as isize;
        let modified_score = self.modified_region_is_boundary(modified_start, modified_length) as isize;
        original_score + modified_score
    }
}

/// `ConcatenateChanges`: fuses the change the recursion split at the seam.
fn concatenate_changes(mut left: Vec<Change>, right: Vec<Change>) -> Vec<Change> {
    if left.is_empty() || right.is_empty() {
        return if !right.is_empty() { right } else { left };
    }
    let last = left.len() - 1;
    if let Some(merged) = changes_overlap(&left[last], &right[0]) {
        left[last] = merged;
        left.extend_from_slice(&right[1..]);
    } else {
        left.extend(right);
    }
    left
}

/// `ChangesOverlap`: the merged change when `left` touches or overlaps `right`.
fn changes_overlap(left: &Change, right: &Change) -> Option<Change> {
    assert!(left.original_start <= right.original_start, "Left change is not less than or equal to right change");
    assert!(left.modified_start <= right.modified_start, "Left change is not less than or equal to right change");

    let original_touches = left.original_start + left.original_length >= right.original_start;
    let modified_touches = left.modified_start + left.modified_length >= right.modified_start;
    if !original_touches && !modified_touches {
        return None;
    }
    let mut original_length = left.original_length;
    let mut modified_length = left.modified_length;
    if original_touches {
        original_length = right.original_start + right.original_length - left.original_start;
    }
    if modified_touches {
        modified_length = right.modified_start + right.modified_length - left.modified_start;
    }
    Some(Change::new(left.original_start, original_length, left.modified_start, modified_length))
}

/// `ClipDiagonalBound`: clips to the boundary or one inside it, by the parity of the boundary and `numDifferences`.
fn clip_diagonal_bound(diagonal: isize, num_differences: isize, diagonal_base_index: isize, num_diagonals: isize) -> isize {
    if diagonal >= 0 && diagonal < num_diagonals {
        return diagonal;
    }
    let diagonals_below = diagonal_base_index;
    let diagonals_above = num_diagonals - diagonal_base_index - 1;
    let diff_even = num_differences % 2 == 0;
    if diagonal < 0 {
        let lower_bound_even = diagonals_below % 2 == 0;
        if diff_even == lower_bound_even { 0 } else { 1 }
    } else {
        let upper_bound_even = diagonals_above % 2 == 0;
        if diff_even == upper_bound_even { num_diagonals - 1 } else { num_diagonals - 2 }
    }
}

#[cfg(test)]
mod tests;
