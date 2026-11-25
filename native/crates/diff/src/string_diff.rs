//! `stringDiff` of base/common/diff/diff.ts, the char diff `computeMoreMinimalEdits` runs on a formatter's edit.
//! Upstream runs `LcsDiff` over UTF-16 char codes; this runs the Myers diff of this crate over chars, which finds
//! an edit script of the same length (they can pick different ones among equal-length scripts).

use std::ops::Range;

use crate::algorithms::{Sequence, Timeout, myers};

struct Chars(Vec<char>);

impl Sequence for Chars {
    fn element(&self, offset: usize) -> u32 {
        self.0[offset] as u32
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn boundary_score(&self, _: usize) -> i32 {
        0
    }

    fn strongly_equal(&self, offset1: usize, offset2: usize) -> bool {
        self.0[offset1] == self.0[offset2]
    }
}

/// The changed char ranges of `original` and `modified`, in order.
pub fn string_diff(original: &str, modified: &str) -> Vec<(Range<usize>, Range<usize>)> {
    let (original, modified) = (Chars(original.chars().collect()), Chars(modified.chars().collect()));
    myers(&original, &modified, &Timeout::infinite())
        .diffs
        .into_iter()
        .map(|diff| (diff.seq1.start..diff.seq1.end, diff.seq2.start..diff.seq2.end))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_changed_chars_are_reported() {
        assert_eq!(string_diff("function  a(){}", "function a() {}"), vec![(9..10, 9..9), (13..13, 12..13)]);
        assert_eq!(string_diff("abc", "abc"), vec![]);
        assert_eq!(string_diff("", "x"), vec![(0..0, 0..1)]);
    }
}
