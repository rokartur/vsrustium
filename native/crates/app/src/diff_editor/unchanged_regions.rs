//! diffEditorViewModel.ts `UnchangedRegion` and `updateUnchangedRegions`: the unchanged stretches between changes
//! that `diffEditor.hideUnchangedRegions.enabled` collapses, each with the lines revealed at its top and bottom.

use diff::{LineRange, LineRangeMapping};

/// diffEditorViewModel.ts `RevealPreference`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevealPreference {
    FromCloserSide,
    FromTop,
    FromBottom,
}

/// One unchanged stretch: 1-based first lines on both sides, its length and the lines shown at either end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnchangedRegion {
    pub original_line: usize,
    pub modified_line: usize,
    pub line_count: usize,
    pub visible_top: usize,
    pub visible_bottom: usize,
}

impl UnchangedRegion {
    fn new(original_line: usize, modified_line: usize, line_count: usize) -> Self {
        UnchangedRegion { original_line, modified_line, line_count, visible_top: 0, visible_bottom: 0 }
    }

    /// `UnchangedRegion.fromDiffs`: the stretches without changes, less `min_context` lines next to each change,
    /// that still hide at least `min_hidden` lines.
    pub fn from_diffs(changes: &[LineRangeMapping], original_count: usize, modified_count: usize, min_hidden: usize, min_context: usize) -> Vec<Self> {
        let mut result = Vec::new();
        for mapping in LineRangeMapping::inverse(changes, original_count, modified_count) {
            let mut original_start = mapping.original.start;
            let mut modified_start = mapping.modified.start;
            let mut length = mapping.original.len();
            let at_start = original_start == 1 && modified_start == 1;
            let at_end = original_start + length == original_count + 1 && modified_start + length == modified_count + 1;
            if (at_start || at_end) && length >= min_context + min_hidden {
                if at_start && !at_end {
                    length -= min_context;
                }
                if at_end && !at_start {
                    original_start += min_context;
                    modified_start += min_context;
                    length -= min_context;
                }
                result.push(UnchangedRegion::new(original_start, modified_start, length));
            } else if length >= min_context * 2 + min_hidden {
                result.push(UnchangedRegion::new(original_start + min_context, modified_start + min_context, length - min_context * 2));
            }
        }
        result
    }

    pub fn original_range(&self) -> LineRange {
        LineRange::new(self.original_line, self.original_line + self.line_count)
    }

    pub fn modified_range(&self) -> LineRange {
        LineRange::new(self.modified_line, self.modified_line + self.line_count)
    }

    pub fn hidden_original(&self) -> LineRange {
        LineRange::new(self.original_line + self.visible_top, self.original_line + self.line_count - self.visible_bottom)
    }

    pub fn hidden_modified(&self) -> LineRange {
        LineRange::new(self.modified_line + self.visible_top, self.modified_line + self.line_count - self.visible_bottom)
    }

    /// `shouldHideControls` (never dragged here): every line shows.
    pub fn is_fully_revealed(&self) -> bool {
        self.visible_top + self.visible_bottom == self.line_count
    }

    pub fn show_more_above(&mut self, count: usize) {
        self.visible_top = (self.visible_top + count).min(self.line_count - self.visible_bottom);
    }

    pub fn show_more_below(&mut self, count: usize) {
        self.visible_bottom = (self.visible_bottom + count).min(self.line_count - self.visible_top);
    }

    pub fn show_all(&mut self) {
        self.visible_bottom = self.line_count - self.visible_top;
    }

    pub fn collapse_all(&mut self) {
        self.visible_top = 0;
        self.visible_bottom = 0;
    }

    /// `showModifiedLine` for a hidden 1-based `line`.
    pub fn show_modified_line(&mut self, line: usize, preference: RevealPreference) {
        let top = line + 1 - (self.modified_line + self.visible_top);
        let bottom = (self.modified_line + self.line_count - self.visible_bottom) - line;
        if (preference == RevealPreference::FromCloserSide && top < bottom) || preference == RevealPreference::FromTop {
            self.visible_top += top;
        } else {
            self.visible_bottom += bottom;
        }
    }

    /// `setVisibleRanges`: the region split around the hidden modified lines `visible` leaves, each part keeping the
    /// lines above its hidden part shown.
    fn set_visible_ranges(self, visible: &[LineRangeMapping]) -> Vec<Self> {
        let hidden = subtract(self.modified_range(), visible.iter().map(|mapping| mapping.modified));
        if hidden.is_empty() {
            let mut shown = self;
            shown.show_all();
            return vec![shown];
        }
        let mut result = Vec::new();
        let (mut original_start, mut modified_start) = (self.original_line, self.modified_line);
        let modified_end = self.modified_line + self.line_count;
        for (index, range) in hidden.iter().enumerate() {
            let end = if index == hidden.len() - 1 { modified_end } else { range.end };
            let mut part = UnchangedRegion::new(original_start, modified_start, end - modified_start);
            part.visible_top = range.start - modified_start;
            part.visible_bottom = part.modified_line + part.line_count - range.end;
            result.push(part);
            original_start += part.line_count;
            modified_start += part.line_count;
        }
        result
    }
}

/// A drag of a region's top or bottom bar: the bar, the pointer's start and the visible line count it started at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnchangedDrag {
    pub region: usize,
    pub top: bool,
    pub start_y: f32,
    pub start_count: usize,
    pub moved: bool,
}

/// A document symbol of the modified model's outline (`OutlineModel.asListOfDocumentSymbols`), 1-based lines.
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineSymbol {
    pub name: String,
    pub kind: u64,
    pub start_line: usize,
    pub end_line: usize,
}

/// diffEditorBreadcrumbs `getBreadcrumbItems`: the symbols starting in `range` and ending after it, largest first.
pub fn breadcrumb_items(symbols: &[OutlineSymbol], range: LineRange) -> Vec<&OutlineSymbol> {
    let mut items: Vec<&OutlineSymbol> = symbols.iter().filter(|symbol| range.contains(symbol.start_line) && !range.contains(symbol.end_line)).collect();
    items.sort_by_key(|symbol| std::cmp::Reverse(symbol.end_line - symbol.start_line));
    items
}

/// `asListOfDocumentSymbols` of `$provideDocumentSymbols` answers: every symbol with its children, depth first.
pub fn flatten_symbols(symbols: &serde_json::Value, into: &mut Vec<OutlineSymbol>) {
    for symbol in symbols.as_array().into_iter().flatten() {
        into.push(OutlineSymbol {
            name: symbol["name"].as_str().unwrap_or_default().to_string(),
            kind: symbol["kind"].as_u64().unwrap_or_default(),
            start_line: symbol["range"]["startLineNumber"].as_u64().unwrap_or_default() as usize,
            end_line: symbol["range"]["endLineNumber"].as_u64().unwrap_or_default() as usize,
        });
        flatten_symbols(&symbol["children"], into);
    }
}

/// `updateUnchangedRegions`: the regions of a new diff, keeping the lines the last regions showed.
/// shortcut: the last regions keep their line numbers where upstream tracks them through edits with decorations; an
/// edit above a revealed region can collapse it again. Upgrade when models get tracked decorations.
pub fn transfer(new: Vec<UnchangedRegion>, last: &[UnchangedRegion], original_count: usize, modified_count: usize) -> Vec<UnchangedRegion> {
    if last.is_empty() {
        return new;
    }
    let mut hidden = Vec::new();
    for region in last {
        let (original, modified) = (region.hidden_original(), region.hidden_modified());
        let clip = |range: LineRange, count: usize| LineRange::new(range.start.min(count + 1), range.end.min(count + 1));
        hidden.push(LineRangeMapping::new(clip(original, original_count), clip(modified, modified_count)));
    }
    let visible = LineRangeMapping::inverse(&hidden, original_count, modified_count);
    let mut result = Vec::new();
    for region in new {
        let intersecting: Vec<LineRangeMapping> = visible
            .iter()
            .filter(|mapping| intersects_strict(mapping.original, region.original_range()) && intersects_strict(mapping.modified, region.modified_range()))
            .cloned()
            .collect();
        result.extend(region.set_visible_ranges(&intersecting));
    }
    result
}

fn intersects_strict(a: LineRange, b: LineRange) -> bool {
    a.start < b.end && b.start < a.end
}

/// `LineRangeSet.subtractFrom`: the parts of `range` none of `ranges` cover.
fn subtract(range: LineRange, ranges: impl Iterator<Item = LineRange>) -> Vec<LineRange> {
    let mut covered: Vec<LineRange> = ranges.filter(|other| other.start < range.end && range.start < other.end).collect();
    covered.sort_by_key(|other| other.start);
    let mut result = Vec::new();
    let mut from = range.start;
    for other in covered {
        if other.start > from {
            result.push(LineRange::new(from, other.start));
        }
        from = from.max(other.end);
    }
    if from < range.end {
        result.push(LineRange::new(from, range.end));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(original: (usize, usize), modified: (usize, usize)) -> LineRangeMapping {
        LineRangeMapping::new(LineRange::new(original.0, original.1), LineRange::new(modified.0, modified.1))
    }

    #[test]
    fn regions_keep_context_lines_around_changes() {
        // 100 lines, line 50 changed: lines 1..47 and 54..101 hide, 3 lines of context each side.
        let regions = UnchangedRegion::from_diffs(&[change((50, 51), (50, 51))], 100, 100, 3, 3);
        assert_eq!(regions, vec![UnchangedRegion::new(1, 1, 46), UnchangedRegion::new(54, 54, 47)]);
        // A stretch shorter than context twice plus the minimum stays.
        assert!(UnchangedRegion::from_diffs(&[change((5, 6), (5, 6)), change((12, 13), (12, 13))], 14, 14, 3, 3).is_empty());
    }

    #[test]
    fn reveal_moves_the_hidden_range() {
        let mut region = UnchangedRegion::new(10, 12, 40);
        region.show_more_above(20);
        assert_eq!(region.hidden_modified(), LineRange::new(32, 52));
        region.show_more_below(30);
        assert!(region.is_fully_revealed());
        region.collapse_all();
        region.show_modified_line(45, RevealPreference::FromCloserSide);
        assert_eq!(region.hidden_modified(), LineRange::new(12, 45));
    }

    #[test]
    fn breadcrumbs_name_the_symbols_around_the_hidden_start() {
        let symbols = serde_json::json!([
            { "name": "Outer", "kind": 4, "range": { "startLineNumber": 2, "endLineNumber": 90 }, "children": [
                { "name": "inner", "kind": 5, "range": { "startLineNumber": 5, "endLineNumber": 60 }, "children": [] },
                { "name": "inside", "kind": 5, "range": { "startLineNumber": 12, "endLineNumber": 14 }, "children": [] },
            ] },
        ]);
        let mut list = Vec::new();
        flatten_symbols(&symbols, &mut list);
        let names: Vec<&str> = breadcrumb_items(&list, LineRange::new(1, 40)).iter().map(|symbol| symbol.name.as_str()).collect();
        assert_eq!(names, ["Outer", "inner"]);
    }

    #[test]
    fn a_new_diff_keeps_revealed_lines() {
        let mut last = UnchangedRegion::from_diffs(&[change((50, 51), (50, 51))], 100, 100, 3, 3);
        last[1].show_all();
        let new = UnchangedRegion::from_diffs(&[change((50, 51), (50, 51))], 100, 100, 3, 3);
        let kept = transfer(new, &last, 100, 100);
        assert!(!kept[0].is_fully_revealed());
        assert!(kept[1].is_fully_revealed());
    }
}
