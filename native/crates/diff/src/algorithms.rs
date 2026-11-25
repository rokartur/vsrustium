//! `algorithms/diffAlgorithm.ts`, `dynamicProgrammingDiffing.ts` and `myersDiffAlgorithm.ts`.

use crate::OffsetRange;
use std::cell::Cell;
use std::time::{Duration, Instant};

/// `ISequence`.
pub(crate) trait Sequence {
    fn element(&self, offset: usize) -> u32;
    fn len(&self) -> usize;
    fn boundary_score(&self, length: usize) -> i32;
    fn strongly_equal(&self, offset1: usize, offset2: usize) -> bool;
}

/// `ITimeout`: `InfiniteTimeout` without a deadline, else `DateTimeout`, which stays invalid once it ran out.
pub struct Timeout {
    deadline: Option<Instant>,
    valid: Cell<bool>,
}

impl Timeout {
    /// `maxComputationTimeMs` 0 means no limit.
    pub fn new(max_ms: u64) -> Self {
        Timeout { deadline: (max_ms > 0).then(|| Instant::now() + Duration::from_millis(max_ms)), valid: Cell::new(true) }
    }

    pub fn infinite() -> Self {
        Timeout { deadline: None, valid: Cell::new(true) }
    }

    pub(crate) fn is_valid(&self) -> bool {
        if let Some(deadline) = self.deadline
            && self.valid.get()
            && Instant::now() >= deadline
        {
            self.valid.set(false);
        }
        self.valid.get()
    }
}

/// `OffsetPair`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OffsetPair {
    pub offset1: usize,
    pub offset2: usize,
}

impl OffsetPair {
    pub const ZERO: OffsetPair = OffsetPair { offset1: 0, offset2: 0 };
    pub const MAX: OffsetPair = OffsetPair { offset1: usize::MAX / 4, offset2: usize::MAX / 4 };

    pub fn delta(self, offset: isize) -> Self {
        OffsetPair { offset1: self.offset1.wrapping_add_signed(offset), offset2: self.offset2.wrapping_add_signed(offset) }
    }
}

/// `SequenceDiff`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SequenceDiff {
    pub seq1: OffsetRange,
    pub seq2: OffsetRange,
}

impl SequenceDiff {
    pub fn new(seq1: OffsetRange, seq2: OffsetRange) -> Self {
        SequenceDiff { seq1, seq2 }
    }

    /// The equal ranges between `diffs`.
    pub fn invert(diffs: &[SequenceDiff], doc1_length: usize) -> Vec<SequenceDiff> {
        let mut result = Vec::with_capacity(diffs.len() + 1);
        for i in 0..=diffs.len() {
            let a = i.checked_sub(1).map(|i| diffs[i]);
            let b = diffs.get(i);
            let start = a.map_or(OffsetPair::ZERO, |a| a.end_exclusives());
            let end = match (b, a) {
                (Some(b), _) => b.starts(),
                (None, Some(a)) => OffsetPair { offset1: doc1_length, offset2: a.seq2.end + doc1_length - a.seq1.end },
                (None, None) => OffsetPair { offset1: doc1_length, offset2: doc1_length },
            };
            result.push(SequenceDiff::from_offset_pairs(start, end));
        }
        result
    }

    pub fn from_offset_pairs(start: OffsetPair, end: OffsetPair) -> Self {
        SequenceDiff::new(OffsetRange::new(start.offset1, end.offset1), OffsetRange::new(start.offset2, end.offset2))
    }

    pub fn swap(self) -> Self {
        SequenceDiff::new(self.seq2, self.seq1)
    }

    pub fn join(&self, other: &SequenceDiff) -> Self {
        SequenceDiff::new(self.seq1.join(&other.seq1), self.seq2.join(&other.seq2))
    }

    pub fn delta(self, offset: isize) -> Self {
        SequenceDiff::new(self.seq1.delta(offset), self.seq2.delta(offset))
    }

    /// Upstream lets the start go negative before its caller intersects it with offsets from 0; 0 gives the same result.
    pub fn delta_start(self, offset: isize) -> Self {
        let start = |r: OffsetRange| OffsetRange::new(r.start.saturating_add_signed(offset), r.end);
        SequenceDiff::new(start(self.seq1), start(self.seq2))
    }

    pub fn delta_end(self, offset: isize) -> Self {
        let end = |r: OffsetRange| OffsetRange::new(r.start, r.end.wrapping_add_signed(offset));
        SequenceDiff::new(end(self.seq1), end(self.seq2))
    }

    pub fn intersect(&self, other: &SequenceDiff) -> Option<Self> {
        Some(SequenceDiff::new(self.seq1.intersect(&other.seq1)?, self.seq2.intersect(&other.seq2)?))
    }

    pub fn starts(&self) -> OffsetPair {
        OffsetPair { offset1: self.seq1.start, offset2: self.seq2.start }
    }

    pub fn end_exclusives(&self) -> OffsetPair {
        OffsetPair { offset1: self.seq1.end, offset2: self.seq2.end }
    }
}

/// `DiffAlgorithmResult`.
pub(crate) struct DiffResult {
    pub diffs: Vec<SequenceDiff>,
    pub hit_timeout: bool,
}

impl DiffResult {
    fn trivial(seq1: &impl Sequence, seq2: &impl Sequence, hit_timeout: bool) -> Self {
        DiffResult { diffs: vec![SequenceDiff::new(OffsetRange::new(0, seq1.len()), OffsetRange::new(0, seq2.len()))], hit_timeout }
    }
}

/// `DynamicProgrammingDiffing.compute`: the LCS that prefers long runs of matches (`equalityScore` weights them).
pub(crate) fn dynamic_programming(seq1: &impl Sequence, seq2: &impl Sequence, timeout: &Timeout, equality_score: Option<&dyn Fn(usize, usize) -> f64>) -> DiffResult {
    if seq1.len() == 0 || seq2.len() == 0 {
        return DiffResult::trivial(seq1, seq2, false);
    }
    let (w, h) = (seq1.len(), seq2.len());
    let at = |s1: usize, s2: usize| s1 + s2 * w;
    let mut lcs = vec![0f64; w * h];
    let mut directions = vec![0u8; w * h];
    let mut lengths = vec![0usize; w * h];
    for s1 in 0..w {
        for s2 in 0..h {
            if !timeout.is_valid() {
                return DiffResult::trivial(seq1, seq2, true);
            }
            let horizontal = if s1 == 0 { 0. } else { lcs[at(s1 - 1, s2)] };
            let vertical = if s2 == 0 { 0. } else { lcs[at(s1, s2 - 1)] };
            let extended = if seq1.element(s1) == seq2.element(s2) {
                let mut score = if s1 == 0 || s2 == 0 { 0. } else { lcs[at(s1 - 1, s2 - 1)] };
                if s1 > 0 && s2 > 0 && directions[at(s1 - 1, s2 - 1)] == 3 {
                    score += lengths[at(s1 - 1, s2 - 1)] as f64;
                }
                score + equality_score.map_or(1., |f| f(s1, s2))
            } else {
                -1.
            };
            let value = horizontal.max(vertical).max(extended);
            if value == extended {
                let previous = if s1 > 0 && s2 > 0 { lengths[at(s1 - 1, s2 - 1)] } else { 0 };
                lengths[at(s1, s2)] = previous + 1;
                directions[at(s1, s2)] = 3;
            } else if value == horizontal {
                directions[at(s1, s2)] = 1;
            } else {
                directions[at(s1, s2)] = 2;
            }
            lcs[at(s1, s2)] = value;
        }
    }

    let mut result = Vec::new();
    let mut last1 = w as isize;
    let mut last2 = h as isize;
    let mut report = |s1: isize, s2: isize, result: &mut Vec<SequenceDiff>| {
        if s1 + 1 != last1 || s2 + 1 != last2 {
            result.push(SequenceDiff::new(OffsetRange::new((s1 + 1) as usize, last1 as usize), OffsetRange::new((s2 + 1) as usize, last2 as usize)));
        }
        last1 = s1;
        last2 = s2;
    };
    let (mut s1, mut s2) = (w as isize - 1, h as isize - 1);
    while s1 >= 0 && s2 >= 0 {
        match directions[at(s1 as usize, s2 as usize)] {
            3 => {
                report(s1, s2, &mut result);
                s1 -= 1;
                s2 -= 1;
            }
            1 => s1 -= 1,
            _ => s2 -= 1,
        }
    }
    report(-1, -1, &mut result);
    result.reverse();
    DiffResult { diffs: result, hit_timeout: false }
}

struct SnakePath {
    prev: Option<std::rc::Rc<SnakePath>>,
    x: usize,
    y: usize,
    length: usize,
}

/// Grows on demand and maps negative indices to their own array, as `FastInt32Array` and `FastArrayNegativeIndices`.
struct NegativeIndexed<T: Clone + Default> {
    positive: Vec<T>,
    negative: Vec<T>,
}

impl<T: Clone + Default> NegativeIndexed<T> {
    fn new() -> Self {
        NegativeIndexed { positive: Vec::new(), negative: Vec::new() }
    }

    fn get(&self, index: isize) -> T {
        let (array, i) = if index < 0 { (&self.negative, (-index - 1) as usize) } else { (&self.positive, index as usize) };
        array.get(i).cloned().unwrap_or_default()
    }

    fn set(&mut self, index: isize, value: T) {
        let (array, i) = if index < 0 { (&mut self.negative, (-index - 1) as usize) } else { (&mut self.positive, index as usize) };
        if i >= array.len() {
            array.resize(i + 1, T::default());
        }
        array[i] = value;
    }
}

/// `MyersDiffAlgorithm.compute`: O(ND) with the snake paths kept for the backtrack.
pub(crate) fn myers(seq1: &impl Sequence, seq2: &impl Sequence, timeout: &Timeout) -> DiffResult {
    if seq1.len() == 0 || seq2.len() == 0 {
        return DiffResult::trivial(seq1, seq2, false);
    }
    let (len_x, len_y) = (seq1.len() as isize, seq2.len() as isize);
    let snake = |mut x: isize, mut y: isize| {
        while x < len_x && y < len_y && seq1.element(x as usize) == seq2.element(y as usize) {
            x += 1;
            y += 1;
        }
        x
    };
    let mut v: NegativeIndexed<isize> = NegativeIndexed::new();
    v.set(0, snake(0, 0));
    let mut paths: NegativeIndexed<Option<std::rc::Rc<SnakePath>>> = NegativeIndexed::new();
    let v0 = v.get(0);
    paths.set(0, (v0 != 0).then(|| std::rc::Rc::new(SnakePath { prev: None, x: 0, y: 0, length: v0 as usize })));
    let mut d = 0isize;
    let mut k;
    'outer: loop {
        d += 1;
        if !timeout.is_valid() {
            return DiffResult::trivial(seq1, seq2, true);
        }
        let lower = -d.min(len_y + d % 2);
        let upper = d.min(len_x + d % 2);
        k = lower;
        while k <= upper {
            let top = if k == upper { -1 } else { v.get(k + 1) };
            let left = if k == lower { -1 } else { v.get(k - 1) + 1 };
            let x = top.max(left).min(len_x);
            let y = x - k;
            if x > len_x || y > len_y {
                k += 2;
                continue;
            }
            let new_max_x = snake(x, y);
            v.set(k, new_max_x);
            let last = if x == top { paths.get(k + 1) } else { paths.get(k - 1) };
            paths.set(k, if new_max_x != x { Some(std::rc::Rc::new(SnakePath { prev: last, x: x as usize, y: y as usize, length: (new_max_x - x) as usize })) } else { last });
            if v.get(k) == len_x && v.get(k) - k == len_y {
                break 'outer;
            }
            k += 2;
        }
    }

    let mut path = paths.get(k);
    let mut result = Vec::new();
    let mut last1 = seq1.len();
    let mut last2 = seq2.len();
    loop {
        let (end_x, end_y) = path.as_ref().map_or((0, 0), |p| (p.x + p.length, p.y + p.length));
        if end_x != last1 || end_y != last2 {
            result.push(SequenceDiff::new(OffsetRange::new(end_x, last1), OffsetRange::new(end_y, last2)));
        }
        let Some(p) = path else { break };
        last1 = p.x;
        last2 = p.y;
        path = p.prev.clone();
    }
    result.reverse();
    DiffResult { diffs: result, hit_timeout: false }
}
