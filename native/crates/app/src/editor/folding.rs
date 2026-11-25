//! Folding: the regions of foldingRanges.ts, the indentation provider of indentRangeProvider.ts, the
//! collapse state of foldingModel.ts and the hidden lines of hiddenRangeModel.ts. Region line numbers are
//! 1-based like upstream's; hidden lines are 0-based model lines like the rest of the editor.

use std::ops::Range;

use super::cursor::{Position, Selection};
use super::edit::end_of;
use super::indent_level;

pub const MAX_FOLDING_REGIONS: usize = 0xFFFF;
const MAX_LINE_NUMBER: usize = 0xFFFFFF;
/// `MAX_FOLDING_REGIONS_FOR_INDENT_DEFAULT`, the `editor.foldingMaximumRegions` default.
const MAX_FOLDING_REGIONS_FOR_INDENT: usize = 5000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FoldSource {
    #[default]
    Provider,
    UserDefined,
    Recovered,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FoldRange {
    pub start: usize,
    pub end: usize,
    pub kind: Option<String>,
    pub collapsed: bool,
    pub source: FoldSource,
}

impl FoldRange {
    pub fn new(start: usize, end: usize) -> Self {
        FoldRange { start, end, kind: None, collapsed: false, source: FoldSource::Provider }
    }

    pub fn contained_by(&self, other: &FoldRange) -> bool {
        other.start <= self.start && other.end >= self.end
    }

    fn contains_line(&self, line: usize) -> bool {
        self.start <= line && line <= self.end
    }
}

/// `FoldingRegions`: the ranges sorted by start, each with the index of the range around it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoldingRegions {
    ranges: Vec<FoldRange>,
    parents: Vec<Option<usize>>,
}

impl FoldingRegions {
    pub fn new(ranges: Vec<FoldRange>) -> Self {
        assert!(ranges.len() <= MAX_FOLDING_REGIONS, "invalid startIndexes or endIndexes size");
        let mut parents = Vec::with_capacity(ranges.len());
        let mut stack: Vec<usize> = Vec::new();
        for (index, range) in ranges.iter().enumerate() {
            assert!(range.start <= MAX_LINE_NUMBER && range.end <= MAX_LINE_NUMBER, "startLineNumber or endLineNumber must not exceed {MAX_LINE_NUMBER}");
            while stack.last().is_some_and(|&last| !range.contained_by(&ranges[last])) {
                stack.pop();
            }
            parents.push(stack.last().copied());
            stack.push(index);
        }
        FoldingRegions { ranges, parents }
    }

    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    pub fn ranges(&self) -> &[FoldRange] {
        &self.ranges
    }

    pub fn get(&self, index: usize) -> &FoldRange {
        &self.ranges[index]
    }

    pub fn is_collapsed(&self, index: usize) -> bool {
        self.ranges[index].collapsed
    }

    pub fn set_collapsed(&mut self, index: usize, collapsed: bool) {
        self.ranges[index].collapsed = collapsed;
    }

    pub fn has_types(&self) -> bool {
        self.ranges.iter().any(|range| range.kind.is_some())
    }

    pub fn parent(&self, index: usize) -> Option<usize> {
        self.parents[index]
    }

    /// The innermost region containing `line`.
    pub fn find_range(&self, line: usize) -> Option<usize> {
        let mut index = self.ranges.partition_point(|range| range.start <= line).checked_sub(1)?;
        if self.ranges[index].end >= line {
            return Some(index);
        }
        while let Some(parent) = self.parents[index] {
            if self.ranges[parent].contains_line(line) {
                return Some(parent);
            }
            index = parent;
        }
        None
    }

    /// `sanitizeAndMerge`: the provider ranges `a` with the collapsed or manual ranges `b`; a range that
    /// starts at or before the previous one, ends past `max_line` or crosses its parent's end is dropped.
    pub fn sanitize_and_merge(a: &[FoldRange], b: &[FoldRange], max_line: usize, selection: Option<&[usize]>) -> Vec<FoldRange> {
        let (mut index_a, mut index_b) = (0, 0);
        let mut stacked: Vec<FoldRange> = Vec::new();
        let mut top: Option<FoldRange> = None;
        let mut previous_line = 0;
        let mut result = Vec::new();
        while index_a < a.len() || index_b < b.len() {
            let (next_a, next_b) = (a.get(index_a), b.get(index_b));
            let mut use_range = None;
            if let Some(next_b) = next_b.filter(|next_b| next_a.is_none_or(|next_a| next_a.start >= next_b.start)) {
                match next_a.filter(|next_a| next_a.start == next_b.start) {
                    Some(next_a) => {
                        if next_b.source == FoldSource::UserDefined {
                            use_range = Some(next_b.clone());
                        } else {
                            let mut range = next_a.clone();
                            range.collapsed = next_b.collapsed && (next_a.end == next_b.end || !starts_inside(selection, next_a.start + 1, next_a.end + 1));
                            range.source = FoldSource::Provider;
                            use_range = Some(range);
                        }
                        index_a += 1;
                    }
                    None => {
                        let mut range = next_b.clone();
                        if next_b.collapsed && next_b.source == FoldSource::Provider {
                            range.source = FoldSource::Recovered;
                        }
                        use_range = Some(range);
                    }
                }
                index_b += 1;
            } else {
                let next_a = next_a.expect("one side has a range left");
                // A user defined range that ends after this one wins over it.
                let overridden = b[index_b..].iter().take_while(|prescan| prescan.start <= next_a.end).any(|prescan| prescan.source == FoldSource::UserDefined && prescan.end > next_a.end);
                if !overridden {
                    use_range = Some(next_a.clone());
                }
                index_a += 1;
            }
            let Some(range) = use_range else { continue };
            while top.as_ref().is_some_and(|top| top.end < range.start) {
                top = stacked.pop();
            }
            if range.end > range.start && range.start > previous_line && range.end <= max_line && top.as_ref().is_none_or(|top| top.end >= range.end) {
                previous_line = range.start;
                if let Some(top) = top.take() {
                    stacked.push(top);
                }
                top = Some(range.clone());
                result.push(range);
            }
        }
        result
    }
}

/// `SelectedLines.startsInside`: whether a selection starts on a line in `start..=end`.
fn starts_inside(selection: Option<&[usize]>, start: usize, end: usize) -> bool {
    selection.is_some_and(|lines| lines.iter().any(|line| (start..=end).contains(line)))
}

/// `IFoldingRangeData`: a provider's range, 1-based, with the provider's rank and the range's kind.
#[derive(Clone, Debug, PartialEq)]
pub struct SyntaxRange {
    pub start: usize,
    pub end: usize,
    pub rank: usize,
    pub kind: Option<String>,
}

/// syntaxRangeProvider.ts `sanitizeRanges`: the ranges by start (then rank), each kept when it nests in the open
/// ranges or starts after them, then `RangesCollector.toIndentRanges`: past `limit`, the deepest nesting levels go.
pub fn sanitize_ranges(mut ranges: Vec<SyntaxRange>, limit: usize) -> FoldingRegions {
    ranges.sort_by_key(|range| (range.start, range.rank));
    let mut collected: Vec<(SyntaxRange, usize)> = Vec::new();
    let mut top: Option<SyntaxRange> = None;
    let mut previous: Vec<SyntaxRange> = Vec::new();
    for entry in ranges {
        let Some(current) = top.clone() else {
            collected.push((entry.clone(), previous.len()));
            top = Some(entry);
            continue;
        };
        if entry.start <= current.start {
            continue;
        }
        if entry.end <= current.end {
            previous.push(current);
            collected.push((entry.clone(), previous.len()));
            top = Some(entry);
            continue;
        }
        if entry.start > current.end {
            let mut popped = Some(current);
            while popped.as_ref().is_some_and(|range| entry.start > range.end) {
                popped = previous.pop();
            }
            if let Some(range) = popped {
                previous.push(range);
            }
            top = Some(entry.clone());
        }
        collected.push((entry, previous.len()));
    }
    collected.retain(|(range, _)| range.start <= MAX_LINE_NUMBER && range.end <= MAX_LINE_NUMBER);
    if collected.len() > limit {
        let mut counts: Vec<usize> = Vec::new();
        for &(_, level) in &collected {
            if level < 30 {
                if counts.len() <= level {
                    counts.resize(level + 1, 0);
                }
                counts[level] += 1;
            }
        }
        let mut entries = 0;
        let mut max_level = counts.len();
        for (level, &count) in counts.iter().enumerate() {
            if count > 0 && count + entries > limit {
                max_level = level;
                break;
            }
            entries += count;
        }
        let mut kept = Vec::with_capacity(limit);
        for (range, level) in collected {
            if level < max_level || (level == max_level && entries < limit) {
                if level == max_level {
                    entries += 1;
                }
                kept.push((range, level));
            }
        }
        collected = kept;
    }
    FoldingRegions::new(collected.into_iter().map(|(range, _)| FoldRange { kind: range.kind, ..FoldRange::new(range.start, range.end) }).collect())
}

/// `foldingRules.markers`: one `(start)|(?:end)` pattern when both have the same JavaScript flags, as
/// computeRanges.ts builds it, else the two patterns.
pub struct FoldingMarkers {
    lines: MarkerLines,
    /// The start pattern without its flags, as `foldAllMarkerRegions` compiles it (`new RegExp(markers.start)`).
    pub start: onig::Regex,
    /// The start pattern with its flags, as `collectRegionHeaders` compiles it.
    pub start_with_flags: onig::Regex,
}

enum MarkerLines {
    Combined(onig::Regex),
    Separate { start: onig::Regex, end: onig::Regex },
}

impl FoldingMarkers {
    /// Patterns with their JavaScript flags; of those only `i` changes a match on one line.
    pub fn new((start, start_flags): (&str, &str), (end, end_flags): (&str, &str)) -> Result<Self, onig::Error> {
        let lines = if start_flags == end_flags {
            MarkerLines::Combined(marker_regex(&format!("({start})|(?:{end})"), start_flags)?)
        } else {
            MarkerLines::Separate { start: marker_regex(start, start_flags)?, end: marker_regex(end, end_flags)? }
        };
        Ok(FoldingMarkers { lines, start: marker_regex(start, "")?, start_with_flags: marker_regex(start, start_flags)? })
    }

    /// (is start, is end) for a line.
    pub(crate) fn classify(&self, line: &str) -> (bool, bool) {
        match &self.lines {
            MarkerLines::Combined(regex) => match regex.captures(line) {
                Some(captures) => {
                    let start = captures.pos(1).is_some();
                    (start, !start)
                }
                None => (false, false),
            },
            MarkerLines::Separate { start, end } => {
                let is_start = start.find(line).is_some();
                (is_start, !is_start && end.find(line).is_some())
            }
        }
    }
}

pub(crate) fn marker_regex(pattern: &str, flags: &str) -> Result<onig::Regex, onig::Error> {
    // CAPTURE_GROUP keeps `(start)` group 1 when a marker has named groups (css `(?<label>...)`).
    let mut options = onig::RegexOptions::REGEX_OPTION_CAPTURE_GROUP;
    if flags.contains('i') {
        options |= onig::RegexOptions::REGEX_OPTION_IGNORECASE;
    }
    onig::Regex::with_options(pattern, options, onig::Syntax::default())
}

struct PreviousRegion {
    /// The indent, -1 for the sentinel and -2 for an end marker.
    indent: isize,
    end_above: usize,
    line: usize,
}

/// indentRangeProvider.ts `computeRanges`: a region from each line to the last line below it indented
/// deeper, and from each start marker to its end marker. `offSide` languages end a region before the
/// blank lines that follow it.
pub fn compute_ranges(lines: &[String], tab_size: usize, off_side: bool, markers: Option<&FoldingMarkers>) -> FoldingRegions {
    compute_ranges_limited(lines, tab_size, off_side, markers, MAX_FOLDING_REGIONS_FOR_INDENT)
}

fn compute_ranges_limited(lines: &[String], tab_size: usize, off_side: bool, markers: Option<&FoldingMarkers>, limit: usize) -> FoldingRegions {
    let mut collected: Vec<(usize, usize, usize)> = Vec::new();
    let sentinel = lines.len() + 1;
    let mut previous = vec![PreviousRegion { indent: -1, end_above: sentinel, line: sentinel }];
    for line in (1..=lines.len()).rev() {
        let content = &lines[line - 1];
        let Some(indent) = indent_level(content, tab_size) else {
            if off_side {
                previous.last_mut().expect("the sentinel stays").end_above = line;
            }
            continue;
        };
        let (is_start, is_end) = markers.map_or((false, false), |markers| markers.classify(content));
        if is_end && !is_start {
            previous.push(PreviousRegion { indent: -2, end_above: line, line });
            continue;
        }
        if is_start {
            // Every region since the end marker is discarded; without one the line is a regular line.
            if let Some(marker) = previous.iter().rposition(|region| region.indent == -2) {
                previous.truncate(marker + 1);
                let region = &mut previous[marker];
                collected.push((line, region.line, indent));
                region.line = line;
                region.indent = indent as isize;
                region.end_above = line;
                continue;
            }
        }
        let indent_signed = indent as isize;
        if previous.last().expect("the sentinel stays").indent > indent_signed {
            while previous.last().expect("the sentinel stays").indent > indent_signed {
                previous.pop();
            }
            let end = previous.last().expect("the sentinel stays").end_above - 1;
            if end > line {
                collected.push((line, end, indent));
            }
        }
        let last = previous.last_mut().expect("the sentinel stays");
        if last.indent == indent_signed {
            last.end_above = line;
        } else {
            previous.push(PreviousRegion { indent: indent_signed, end_above: line, line });
        }
    }
    to_indent_ranges(collected, lines, tab_size, limit)
}

/// `RangesCollector.toIndentRanges`: the collected regions in line order; past `limit`, the least indented ones.
fn to_indent_ranges(mut collected: Vec<(usize, usize, usize)>, lines: &[String], tab_size: usize, limit: usize) -> FoldingRegions {
    collected.retain(|&(start, end, _)| start <= MAX_LINE_NUMBER && end <= MAX_LINE_NUMBER);
    collected.reverse();
    if collected.len() > limit {
        let mut occurrences = vec![0; 1000];
        for &(_, _, indent) in &collected {
            if indent < 1000 {
                occurrences[indent] += 1;
            }
        }
        let mut entries = 0;
        let mut max_indent = occurrences.len();
        for (indent, &count) in occurrences.iter().enumerate() {
            if count > 0 && count + entries > limit {
                max_indent = indent;
                break;
            }
            entries += count;
        }
        let mut kept = Vec::with_capacity(limit);
        for &(start, end, _) in &collected {
            let indent = indent_level(&lines[start - 1], tab_size).expect("a region starts on a line with text");
            let below = indent < max_indent;
            if below || (indent == max_indent && entries < limit) {
                if !below {
                    entries += 1;
                }
                kept.push((start, end, 0));
            }
        }
        collected = kept;
    }
    FoldingRegions::new(collected.into_iter().map(|(start, end, _)| FoldRange::new(start, end)).collect())
}

/// `ILineMemento`: a folded or manual region as persisted per file, with a checksum of its first hidden
/// line and last line.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(not(test), expect(dead_code, reason = "per-file fold state is saved once workspace storage lands with milestone 10"))]
pub struct LineMemento {
    pub start: usize,
    pub end: usize,
    pub checksum: Option<u32>,
    pub collapsed: Option<bool>,
    pub source: Option<FoldSource>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldDecoration {
    Expanded,
    Collapsed,
    Hidden,
}

/// The collapse state of one text model's regions and the lines it hides (`FoldingModel`, `HiddenRangeModel`).
#[derive(Clone, Debug, Default)]
pub struct FoldingModel {
    pub regions: FoldingRegions,
    /// Hidden model lines, ascending and disjoint: the lines below each outermost collapsed region's first line.
    hidden: Vec<Range<usize>>,
}

impl FoldingModel {
    pub fn has_hidden(&self) -> bool {
        !self.hidden.is_empty()
    }

    pub fn hidden(&self) -> &[Range<usize>] {
        &self.hidden
    }

    /// `toggleCollapseState`: each region flips once.
    pub fn toggle_collapse_state(&mut self, mut regions: Vec<usize>) {
        if regions.is_empty() {
            return;
        }
        regions.sort_unstable();
        regions.dedup();
        for index in regions {
            let collapsed = !self.regions.is_collapsed(index);
            self.regions.set_collapsed(index, collapsed);
        }
        self.update_hidden();
    }

    /// `update`: new provider regions keep the collapsed and manual ones; a collapsed region a selection
    /// starts inside expands.
    pub fn update(&mut self, new_regions: FoldingRegions, line_count: usize, selection: Option<&[usize]>) {
        let current = self.current_folded_or_manual_ranges(selection);
        let merged = FoldingRegions::sanitize_and_merge(new_regions.ranges(), &current, line_count, selection);
        self.update_post(FoldingRegions::new(merged));
    }

    pub fn update_post(&mut self, regions: FoldingRegions) {
        self.regions = regions;
        self.update_hidden();
    }

    fn current_folded_or_manual_ranges(&self, selection: Option<&[usize]>) -> Vec<FoldRange> {
        let mut ranges = Vec::new();
        for range in self.regions.ranges() {
            if range.collapsed || range.source != FoldSource::Provider {
                let mut range = range.clone();
                range.collapsed = range.collapsed && !starts_inside(selection, range.start + 1, range.end);
                ranges.push(range);
            }
        }
        ranges
    }

    /// `removeManualRanges`: an empty selection removes the manual region around it (all of them when it is
    /// in none), a selection removes the manual regions it touches.
    pub fn remove_manual_ranges(&mut self, selections: &[Selection]) {
        let mut to_remove = Vec::new();
        let mut remove_all = false;
        for selection in selections.iter().filter(|selection| selection.is_empty()) {
            let mut index = self.regions.find_range(selection.head.line + 1);
            while let Some(found) = index.filter(|&found| self.regions.get(found).source == FoldSource::Provider) {
                index = self.regions.parent(found);
            }
            match index {
                Some(index) => to_remove.push(index),
                None => remove_all = true,
            }
        }
        let intersects_selection = |range: &FoldRange| {
            selections.iter().filter(|selection| !selection.is_empty()).any(|selection| !(selection.start().line + 1 > range.end || range.start > selection.end().line + 1))
        };
        let kept = self
            .regions
            .ranges()
            .iter()
            .enumerate()
            .filter(|(index, range)| range.source == FoldSource::Provider || (!remove_all && !to_remove.contains(index) && !intersects_selection(range)))
            .map(|(_, range)| range.clone())
            .collect();
        self.update_post(FoldingRegions::new(kept));
    }

    /// `getMemento`: the folded and manual regions, for persistence only.
    #[cfg_attr(not(test), expect(dead_code, reason = "per-file fold state is saved once workspace storage lands with milestone 10"))]
    pub fn get_memento(&self, lines: &[String]) -> Option<Vec<LineMemento>> {
        let mut result = Vec::new();
        for range in self.current_folded_or_manual_ranges(None) {
            if range.start >= range.end || range.start < 1 || range.end > lines.len() {
                continue;
            }
            let checksum = Some(lines_checksum(lines, range.start + 1, range.end));
            result.push(LineMemento { start: range.start, end: range.end, checksum, collapsed: Some(range.collapsed), source: Some(range.source) });
        }
        (!result.is_empty()).then_some(result)
    }

    /// `applyMemento`: restores the persisted regions whose lines still have their checksum.
    #[cfg_attr(not(test), expect(dead_code, reason = "per-file fold state is restored once workspace storage lands with milestone 10"))]
    pub fn apply_memento(&mut self, lines: &[String], state: &[LineMemento]) {
        let mut restore = Vec::new();
        for memento in state {
            if memento.start >= memento.end || memento.start < 1 || memento.end > lines.len() {
                continue;
            }
            if memento.checksum.is_none_or(|checksum| checksum == lines_checksum(lines, memento.start + 1, memento.end)) {
                let source = memento.source.unwrap_or_default();
                restore.push(FoldRange { start: memento.start, end: memento.end, kind: None, collapsed: memento.collapsed.unwrap_or(true), source });
            }
        }
        let merged = FoldingRegions::sanitize_and_merge(self.regions.ranges(), &restore, lines.len(), None);
        self.update_post(FoldingRegions::new(merged));
    }

    pub fn region_at_line(&self, line: usize) -> Option<usize> {
        self.regions.find_range(line)
    }

    /// `getAllRegionsAtLine`: the regions around `line`, innermost first, with their level counted from it.
    pub fn all_regions_at_line(&self, line: usize, filter: impl Fn(&FoldRange, usize) -> bool) -> Vec<usize> {
        let mut result = Vec::new();
        let mut index = self.regions.find_range(line);
        let mut level = 1;
        while let Some(current) = index {
            if filter(self.regions.get(current), level) {
                result.push(current);
            }
            level += 1;
            index = self.regions.parent(current);
        }
        result
    }

    /// `getRegionsInside`: the regions inside `region` (all of them for `None`), with their nesting level below it.
    pub fn regions_inside(&self, region: Option<usize>, filter: impl Fn(&FoldRange, usize) -> bool) -> Vec<usize> {
        let first = region.map_or(0, |region| region + 1);
        let end = region.map_or(usize::MAX, |region| self.regions.get(region).end);
        let mut result = Vec::new();
        let mut level_stack: Vec<usize> = Vec::new();
        for index in first..self.regions.len() {
            let current = self.regions.get(index);
            if current.start >= end {
                break;
            }
            while level_stack.last().is_some_and(|&last| !current.contained_by(self.regions.get(last))) {
                level_stack.pop();
            }
            level_stack.push(index);
            if filter(current, level_stack.len()) {
                result.push(index);
            }
        }
        result
    }

    /// The decoration of each region, in region order, as `toggleCollapseState` picks its options: hidden
    /// when it ends inside a collapsed region before it.
    pub fn decorations(&self) -> Vec<FoldDecoration> {
        let mut last_hidden_line = 0;
        let mut result = Vec::with_capacity(self.regions.len());
        for range in self.regions.ranges() {
            result.push(match range.end <= last_hidden_line {
                true => FoldDecoration::Hidden,
                false if range.collapsed => FoldDecoration::Collapsed,
                false => FoldDecoration::Expanded,
            });
            if range.collapsed && range.end > last_hidden_line {
                last_hidden_line = range.end;
            }
        }
        result
    }

    /// hiddenRangeModel.ts `updateHiddenRanges`.
    fn update_hidden(&mut self) {
        let mut hidden: Vec<Range<usize>> = Vec::new();
        for range in self.regions.ranges().iter().filter(|range| range.collapsed) {
            // 0-based, the first line stays: 1-based start + 1 is 0-based start.
            let lines = range.start..range.end;
            if hidden.last().is_some_and(|last| last.start <= lines.start && lines.end <= last.end) {
                continue;
            }
            hidden.push(lines);
        }
        self.hidden = hidden;
    }

    /// Whether 0-based model `line` is hidden.
    pub fn is_hidden(&self, line: usize) -> bool {
        hidden_range(&self.hidden, line).is_some()
    }

    /// The lines the view shows (`getLineCount` of the view model).
    pub fn view_line_count(&self, line_count: usize) -> usize {
        line_count - self.hidden.iter().map(|range| range.len()).sum::<usize>()
    }

    /// The 0-based view line of model `line`; a hidden line shows on its fold line (`convertModelPositionToViewPosition`).
    pub fn view_line(&self, line: usize) -> usize {
        let mut hidden_before = 0;
        for range in &self.hidden {
            if range.start > line {
                break;
            }
            if line < range.end {
                return range.start - 1 - hidden_before;
            }
            hidden_before += range.len();
        }
        line - hidden_before
    }

    /// The 0-based model line of view `line`.
    pub fn model_line(&self, view_line: usize) -> usize {
        let mut line = view_line;
        for range in &self.hidden {
            if range.start > line {
                break;
            }
            line += range.len();
        }
        line
    }

    /// `_toModelVisibleRanges`: 0-based model lines `start..=end` without the hidden ones, as inclusive ranges.
    pub fn shown_ranges(&self, start: usize, end: usize) -> Vec<(usize, usize)> {
        let mut result = Vec::new();
        let mut from = start;
        for range in &self.hidden {
            if range.end <= from || range.start > end {
                continue;
            }
            if from < range.start {
                result.push((from, range.start - 1));
            }
            from = range.end;
        }
        if from <= end {
            result.push((from, end));
        }
        result
    }

    /// `adjustSelections`: a selection end on a hidden line moves to the end of the line the range is folded on.
    pub fn adjust_selections(&self, lines: &[String], selections: &mut [Selection]) -> bool {
        let mut changed = false;
        let adjust = |position: Position| {
            hidden_range(&self.hidden, position.line).map(|range| {
                let line = range.start - 1;
                Position::new(line, lines[line].chars().count())
            })
        };
        for selection in selections.iter_mut() {
            let (start, end) = (selection.start(), selection.end());
            let new_start = adjust(start).unwrap_or(start);
            let new_end = adjust(end).unwrap_or(end);
            if (new_start, new_end) != (start, end) {
                changed = true;
                *selection = if selection.anchor <= selection.head { Selection { anchor: new_start, head: new_end } } else { Selection { anchor: new_end, head: new_start } };
            }
        }
        changed
    }

    /// Moves each region's lines with an edit of `start..end` into `text` in `lines` (the lines before it), as
    /// the decoration the region keeps does: collapsed and manual ones grow when typing at their edges.
    pub fn accept_replace(&mut self, lines: &[String], start: Position, end: Position, text: &str) {
        if self.regions.len() == 0 {
            return;
        }
        let new_end = end_of(start, text);
        let mut hidden_until = 0;
        let mut ranges = self.regions.ranges.clone();
        for range in ranges.iter_mut() {
            let is_hidden = range.end <= hidden_until;
            if range.collapsed && range.end > hidden_until {
                hidden_until = range.end;
            }
            // `AlwaysGrowsWhenTypingAtEdges` sticks the start to the char before, `NeverGrows...` the end.
            let grows = !is_hidden && (range.collapsed || range.source != FoldSource::Provider);
            let decoration_start = Position::new(range.start - 1, lines[range.start - 1].chars().count());
            let decoration_end = Position::new(range.end - 1, lines[range.end - 1].chars().count());
            range.start = tracked(decoration_start, start, end, new_end, grows).line + 1;
            range.end = tracked(decoration_end, start, end, new_end, !grows).line + 1;
        }
        self.regions = FoldingRegions::new(ranges);
    }
}

/// intervalTree.ts `nodeAcceptEdit` for one marker, by position: where `marker` goes when `start..end`
/// becomes text ending at `new_end`.
pub(super) fn tracked(marker: Position, start: Position, end: Position, new_end: Position, stick_to_previous: bool) -> Position {
    if marker < start || (marker == start && (start != end || stick_to_previous)) {
        return marker;
    }
    if marker <= end {
        return new_end;
    }
    if marker.line == end.line {
        return Position::new(new_end.line, new_end.column + marker.column - end.column);
    }
    Position::new(marker.line + new_end.line - end.line, marker.column)
}

/// `_getLinesChecksum`: base/common/hash.ts `hash` of the two lines' text, modulo 1000000.
fn lines_checksum(lines: &[String], first: usize, last: usize) -> u32 {
    let text = lines[first - 1].clone() + &lines[last - 1];
    let number_hash = |value: i32, initial: i32| initial.wrapping_shl(5).wrapping_sub(initial).wrapping_add(value);
    let mut hash = number_hash(149417, 0);
    for unit in text.encode_utf16() {
        hash = number_hash(unit as i32, hash);
    }
    // JavaScript's `%` keeps the sign of the dividend.
    (hash % 1000000).unsigned_abs()
}

/// The hidden range containing 0-based `line`.
pub fn hidden_range(hidden: &[Range<usize>], line: usize) -> Option<&Range<usize>> {
    let index = hidden.partition_point(|range| range.start <= line).checked_sub(1)?;
    hidden.get(index).filter(|range| line < range.end)
}

/// The line the view shows after (`down`) or before 0-based `line`, skipping hidden lines; `None` past the edge.
pub fn shown_neighbour(line_count: usize, hidden: &[Range<usize>], line: usize, down: bool) -> Option<usize> {
    if down {
        let next = line + 1;
        let next = hidden_range(hidden, next).map_or(next, |range| range.end);
        return (next < line_count).then_some(next);
    }
    let previous = line.checked_sub(1)?;
    Some(hidden_range(hidden, previous).map_or(previous, |range| range.start - 1))
}

/// A cursor that a move left on a hidden line goes where the view would put it: after the folded lines when
/// moving `forward`, else to the end of the line they are folded into.
pub fn shown_position(lines: &[String], hidden: &[Range<usize>], position: Position, forward: bool) -> Position {
    let Some(range) = hidden_range(hidden, position.line) else {
        return position;
    };
    if forward && range.end < lines.len() {
        return Position::new(range.end, 0);
    }
    let line = range.start - 1;
    Position::new(line, lines[line].chars().count())
}

/// The last line the view shows.
pub fn last_shown_line(line_count: usize, hidden: &[Range<usize>]) -> usize {
    let last = line_count - 1;
    hidden_range(hidden, last).map_or(last, |range| range.start - 1)
}

/// `toggleCollapseState(foldingModel, levels, lineNumbers)`: each region at a line flips, and with
/// `levels` above 1 the regions inside it that are not yet in its new state, up to that depth.
pub fn toggle_collapse_state(model: &mut FoldingModel, levels: usize, lines: &[usize]) {
    let mut to_toggle = Vec::new();
    for &line in lines {
        let Some(region) = model.region_at_line(line) else { continue };
        let collapse = !model.regions.is_collapsed(region);
        to_toggle.push(region);
        if levels > 1 {
            to_toggle.extend(model.regions_inside(Some(region), |range, level| range.collapsed != collapse && level < levels));
        }
    }
    model.toggle_collapse_state(to_toggle);
}

/// `setCollapseStateLevelsDown`: the regions at the lines (all regions without lines) and the ones
/// inside them up to `levels` deep.
pub fn set_collapse_state_levels_down(model: &mut FoldingModel, collapse: bool, levels: usize, lines: Option<&[usize]>) {
    let mut to_toggle = Vec::new();
    match lines.filter(|lines| !lines.is_empty()) {
        Some(lines) => {
            for &line in lines {
                let Some(region) = model.region_at_line(line) else { continue };
                if model.regions.is_collapsed(region) != collapse {
                    to_toggle.push(region);
                }
                if levels > 1 {
                    to_toggle.extend(model.regions_inside(Some(region), |range, level| range.collapsed != collapse && level < levels));
                }
            }
        }
        None => to_toggle.extend(model.regions_inside(None, |range, level| range.collapsed != collapse && level < levels)),
    }
    model.toggle_collapse_state(to_toggle);
}

/// `setCollapseStateLevelsUp`: the regions around the lines, up to `levels` outwards.
#[cfg_attr(not(test), expect(dead_code, reason = "only editor.fold's {direction: \"up\"} argument reaches it, and extensions cannot run editor commands yet"))]
pub fn set_collapse_state_levels_up(model: &mut FoldingModel, collapse: bool, levels: usize, lines: &[usize]) {
    let mut to_toggle = Vec::new();
    for &line in lines {
        to_toggle.extend(model.all_regions_at_line(line, |range, level| range.collapsed != collapse && level <= levels));
    }
    model.toggle_collapse_state(to_toggle);
}

/// `setCollapseStateUp`: the innermost region around each line not yet in the state.
pub fn set_collapse_state_up(model: &mut FoldingModel, collapse: bool, lines: &[usize]) {
    let mut to_toggle = Vec::new();
    for &line in lines {
        to_toggle.extend(model.all_regions_at_line(line, |range, _| range.collapsed != collapse).first());
    }
    model.toggle_collapse_state(to_toggle);
}

/// `setCollapseStateAtLevel`: every region at nesting level `fold_level` except those around a blocked line.
pub fn set_collapse_state_at_level(model: &mut FoldingModel, fold_level: usize, collapse: bool, blocked_lines: &[usize]) {
    let to_toggle = model.regions_inside(None, |range, level| level == fold_level && range.collapsed != collapse && !blocked_lines.iter().any(|&line| range.contains_line(line)));
    model.toggle_collapse_state(to_toggle);
}

/// `setCollapseStateForRest`: every region neither inside nor around the innermost region at a blocked line.
pub fn set_collapse_state_for_rest(model: &mut FoldingModel, collapse: bool, blocked_lines: &[usize]) {
    let mut filtered: Vec<FoldRange> = Vec::new();
    for &line in blocked_lines {
        if let Some(&region) = model.all_regions_at_line(line, |_, _| true).first() {
            filtered.push(model.regions.get(region).clone());
        }
    }
    let to_toggle = model.regions_inside(None, |range, _| filtered.iter().all(|blocked| !blocked.contained_by(range) && !range.contained_by(blocked)) && range.collapsed != collapse);
    model.toggle_collapse_state(to_toggle);
}

/// `setCollapseStateForMatchingLines`: the regions whose first line matches.
pub fn set_collapse_state_for_matching_lines(model: &mut FoldingModel, lines: &[String], regex: &onig::Regex, collapse: bool) {
    let mut to_toggle = Vec::new();
    for (index, range) in model.regions.ranges().iter().enumerate().rev() {
        if range.collapsed != collapse && regex.find(&lines[range.start - 1]).is_some() {
            to_toggle.push(index);
        }
    }
    model.toggle_collapse_state(to_toggle);
}

/// `setCollapseStateForType`: the regions of a `FoldingRangeKind`.
pub fn set_collapse_state_for_type(model: &mut FoldingModel, kind: &str, collapse: bool) {
    let mut to_toggle = Vec::new();
    for (index, range) in model.regions.ranges().iter().enumerate().rev() {
        if range.collapsed != collapse && range.kind.as_deref() == Some(kind) {
            to_toggle.push(index);
        }
    }
    model.toggle_collapse_state(to_toggle);
}

/// `getParentFoldLine`: the first line of the region around `line`, or of its parent when `line` starts it.
pub fn parent_fold_line(line: usize, model: &FoldingModel) -> Option<usize> {
    let region = model.region_at_line(line)?;
    let start = model.regions.get(region).start;
    if line != start {
        return Some(start);
    }
    model.regions.parent(region).map(|parent| model.regions.get(parent).start)
}

/// `getPreviousFoldLine`: the previous sibling's first line when `line` starts a region, else the
/// nearest region start above.
pub fn previous_fold_line(line: usize, model: &FoldingModel) -> Option<usize> {
    let regions = &model.regions;
    match model.region_at_line(line).filter(|&region| regions.get(region).start == line) {
        Some(mut region) => {
            let expected_parent = regions.parent(region);
            let min_line = expected_parent.map_or(0, |parent| regions.get(parent).start);
            while region > 0 {
                region -= 1;
                if regions.get(region).start <= min_line {
                    return None;
                }
                if regions.parent(region) == expected_parent {
                    return Some(regions.get(region).start);
                }
            }
            None
        }
        None => regions.ranges().iter().rev().map(|range| range.start).find(|&start| start < line),
    }
}

/// `getNextFoldLine`: the next sibling's first line when `line` starts a region, else the nearest region start below.
pub fn next_fold_line(line: usize, model: &FoldingModel) -> Option<usize> {
    let regions = &model.regions;
    match model.region_at_line(line).filter(|&region| regions.get(region).start == line) {
        Some(mut region) => {
            let expected_parent = regions.parent(region);
            let max_line = match expected_parent {
                Some(parent) => regions.get(parent).end,
                None => regions.get(regions.len() - 1).end,
            };
            while region + 1 < regions.len() {
                region += 1;
                if regions.get(region).start >= max_line {
                    return None;
                }
                if regions.parent(region) == expected_parent {
                    return Some(regions.get(region).start);
                }
            }
            None
        }
        None => regions.ranges().iter().map(|range| range.start).find(|&start| start > line),
    }
}

#[cfg(test)]
mod tests;
