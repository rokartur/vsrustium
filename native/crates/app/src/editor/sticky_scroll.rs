//! Sticky scroll: the scope tree of stickyScrollElement.ts, the candidate lines of stickyScrollProvider.ts and
//! the widget state of stickyScrollController.ts `findScrollWidgetState`. Line numbers are 1-based like upstream's.

use std::ops::Range;

use super::folding::FoldingRegions;
use super::{OpenEditor, Viewport};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StickyRange {
    pub start: usize,
    pub end: usize,
}

/// `StickyElement`: a scope and the scopes inside it, sorted by start line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StickyElement {
    pub range: Option<StickyRange>,
    pub children: Vec<StickyElement>,
}

/// `StickyLineCandidate`: a scope whose first line may stick, `top` pixels below the widget's top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StickyLineCandidate {
    pub start: usize,
    pub end: usize,
    pub top: f32,
    pub height: f32,
}

/// `StickyScrollWidgetState`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StickyScrollState {
    pub start_line_numbers: Vec<usize>,
    pub end_line_numbers: Vec<usize>,
    pub last_line_relative_position: f32,
}

/// `StickyModelFromCandidateFoldingProvider._fromFoldingRegions`: each region one scope, ending one line below
/// the region so its closing line stays covered.
pub fn from_folding_regions(regions: &FoldingRegions) -> StickyElement {
    let mut stack: Vec<(Option<usize>, StickyElement)> = vec![(None, StickyElement::default())];
    for index in 0..regions.len() {
        let parent = regions.parent(index);
        while stack.last().expect("the root stays on the stack").0 != parent {
            close_scope(&mut stack);
        }
        let range = regions.get(index);
        stack.push((Some(index), StickyElement { range: Some(StickyRange { start: range.start, end: range.end + 1 }), children: Vec::new() }));
    }
    while stack.len() > 1 {
        close_scope(&mut stack);
    }
    stack.pop().expect("the root stays on the stack").1
}

fn close_scope(stack: &mut Vec<(Option<usize>, StickyElement)>) {
    let (_, element) = stack.pop().expect("a scope above the root");
    stack.last_mut().expect("the root stays on the stack").1.children.push(element);
}

/// `getCandidateStickyLinesIntersecting`: the scopes around `range`, outermost first, without the ones a fold
/// hides. `hidden` holds 0-based hidden model lines; a scope is valid only if its end is a line of the model.
pub fn candidates_intersecting(root: &StickyElement, range: StickyRange, line_count: usize, line_height: f32, hidden: &[Range<usize>]) -> Vec<StickyLineCandidate> {
    let mut result = Vec::new();
    collect_candidates(root, range, line_count, line_height, &mut result, 0., None);
    // hiddenRangeModel areas are 1-based `start + 1 ..= end` of each hidden range.
    result.retain(|candidate| !hidden.iter().any(|lines| candidate.start >= lines.start + 1 && candidate.end <= lines.end + 1));
    result
}

fn collect_candidates(element: &StickyElement, range: StickyRange, line_count: usize, line_height: f32, result: &mut Vec<StickyLineCandidate>, top: f32, last_start: Option<usize>) {
    let starts: Vec<usize> = element.children.iter().filter_map(|child| child.range.map(|range| range.start)).collect();
    let lower = search_index(&starts, range.start);
    let upper = search_index(&starts, range.end);
    let mut last_line = last_start;
    for child in element.children.iter().take(upper + 1).skip(lower) {
        let Some(StickyRange { start, end }) = child.range else { continue };
        if end > start + 1 && range.start <= end + 1 && start <= range.end + 1 && Some(start) != last_line && start >= 1 && end <= line_count {
            last_line = Some(start);
            result.push(StickyLineCandidate { start, end: end - 1, top, height: line_height });
            collect_candidates(child, range, line_count, line_height, result, top + line_height, Some(start));
        }
    }
}

/// `binarySearch` then `updateIndex`: the index of `key`, else of the last start before it, else 0.
fn search_index(starts: &[usize], key: usize) -> usize {
    match starts.binary_search(&key) {
        Ok(index) => index,
        Err(insertion) => insertion.saturating_sub(1),
    }
}

/// `findScrollWidgetState`: the candidates whose first line scrolled above the widget line they would stick to
/// while their last line is still below it; the last one pushes up when the scope's end reaches it.
/// `top_for_line` gives a 1-based model line's top in the editor's content, a hidden line its fold line's.
pub fn find_widget_state(candidates: &[StickyLineCandidate], scroll_top: f32, max_lines: usize, top_for_line: impl Fn(usize) -> f32) -> StickyScrollState {
    let mut state = StickyScrollState::default();
    for candidate in candidates {
        let bottom_of_element = candidate.top + candidate.height;
        let top_of_beginning_line = top_for_line(candidate.start) - scroll_top;
        let bottom_of_end_line = top_for_line(candidate.end) + candidate.height - scroll_top;
        if candidate.top > top_of_beginning_line && candidate.top <= bottom_of_end_line {
            state.start_line_numbers.push(candidate.start);
            state.end_line_numbers.push(candidate.end + 1);
            if bottom_of_element > bottom_of_end_line {
                state.last_line_relative_position = bottom_of_end_line - bottom_of_element;
            }
        }
        if state.start_line_numbers.len() == max_lines {
            break;
        }
    }
    state
}

/// `_onDidResize`: at most a quarter of the editor's lines, and `maxLineCount`.
pub fn max_sticky_lines(editor_height: f32, line_height: f32, max_line_count: usize) -> usize {
    ((editor_height / line_height * 0.25).round() as usize).min(max_line_count)
}

impl OpenEditor {
    /// `findScrollWidgetState` over the visible lines; empty with sticky scroll off (`sticky_scroll_lines` 0).
    pub fn sticky_scroll_state(&self, viewport: Viewport) -> StickyScrollState {
        let max_lines = max_sticky_lines(viewport.height, viewport.line_height, viewport.sticky_scroll_lines);
        if max_lines == 0 {
            return StickyScrollState::default();
        }
        let view_lines = self.visible_lines(viewport);
        let range = StickyRange { start: self.model_line(view_lines.start) + 1, end: self.model_line(view_lines.end - 1) + 1 };
        let candidates = candidates_intersecting(&self.sticky_model, range, self.document.lines.len(), viewport.line_height, self.folding.hidden());
        find_widget_state(&candidates, self.scroll.top, max_lines, |line| viewport.line_top(self.view_line(line - 1)))
    }

    /// `_toggleFoldingRegionForLine`: the region starting on the sticky line `line` (1-based) at `index` flips,
    /// then the scroll puts its start, or once collapsed its fold line, 1px below that sticky line's top.
    pub fn toggle_sticky_fold(&mut self, line: usize, index: usize, viewport: Viewport) -> bool {
        if !self.click_fold_control(line - 1, true, false, false) {
            return false;
        }
        let region = self.folding.region_at_line(line).expect("the region toggled");
        let range = self.folding.regions.get(region);
        let target = if range.collapsed { range.end } else { range.start };
        let top = viewport.line_top(self.view_line(target - 1)) - viewport.line_height * index as f32 + 1.;
        self.set_scroll(top, self.scroll.left, viewport);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::folding::{FoldRange, compute_ranges};

    const LINE_HEIGHT: f32 = 19.;

    fn scope(start: usize, end: usize, children: Vec<StickyElement>) -> StickyElement {
        StickyElement { range: Some(StickyRange { start, end }), children }
    }

    /// The outline of stickyScroll.test.ts `documentSymbolProviderForTestModel` as
    /// `_stickyModelFromOutlineModel` builds it: each symbol from its selection line to its range's end.
    fn test_model() -> StickyElement {
        StickyElement { range: None, children: vec![scope(1, 3, vec![]), scope(7, 12, vec![scope(9, 12, vec![scope(10, 11, vec![])])]), scope(13, 14, vec![])] }
    }

    fn state_at(model: &StickyElement, line_count: usize, hidden: &[Range<usize>], scroll_top: f32) -> Vec<usize> {
        let is_hidden = |line: usize| hidden.iter().any(|lines| lines.contains(&(line - 1)));
        let visible: Vec<usize> = (1..=line_count).filter(|&line| !is_hidden(line)).collect();
        // A hidden line shows on its fold line.
        let view_line = |line: usize| visible.iter().rposition(|&shown| shown <= line).unwrap();
        let first_view = (scroll_top / LINE_HEIGHT).floor() as usize;
        let last_view = (((scroll_top + 500.) / LINE_HEIGHT).ceil() as usize).min(visible.len()) - 1;
        let model_line = |view: usize| visible[view];
        let range = StickyRange { start: model_line(first_view), end: model_line(last_view) };
        let candidates = candidates_intersecting(model, range, line_count, LINE_HEIGHT, hidden);
        find_widget_state(&candidates, scroll_top, 5, |line| view_line(line) as f32 * LINE_HEIGHT).start_line_numbers
    }

    #[test]
    fn candidates_intersecting_a_range() {
        let model = test_model();
        let candidate = |start, end, top| StickyLineCandidate { start, end, top, height: LINE_HEIGHT };
        assert_eq!(candidates_intersecting(&model, StickyRange { start: 1, end: 4 }, 14, LINE_HEIGHT, &[]), vec![candidate(1, 2, 0.)]);
        assert_eq!(candidates_intersecting(&model, StickyRange { start: 8, end: 10 }, 14, LINE_HEIGHT, &[]), vec![candidate(7, 11, 0.), candidate(9, 11, LINE_HEIGHT)]);
        assert_eq!(candidates_intersecting(&model, StickyRange { start: 10, end: 13 }, 14, LINE_HEIGHT, &[]), vec![candidate(7, 11, 0.), candidate(9, 11, LINE_HEIGHT)]);
    }

    /// issue #157180: render the line of the scope's definition.
    #[test]
    fn sticks_the_scope_definition_line() {
        let model = test_model();
        assert_eq!(state_at(&model, 14, &[], 1.), vec![1]);
        assert_eq!(state_at(&model, 14, &[], LINE_HEIGHT + 1.), vec![1]);
        assert_eq!(state_at(&model, 14, &[], 4. * LINE_HEIGHT + 1.), Vec::<usize>::new());
        assert_eq!(state_at(&model, 14, &[], 8. * LINE_HEIGHT + 1.), vec![7, 9]);
        assert_eq!(state_at(&model, 14, &[], 9. * LINE_HEIGHT + 1.), vec![7, 9]);
        assert_eq!(state_at(&model, 14, &[], 10. * LINE_HEIGHT + 1.), vec![7]);
    }

    /// issue #156268: no sticky lines from a folded region.
    #[test]
    fn skips_scopes_in_folded_regions() {
        let model = test_model();
        // Hidden areas 2..=2 and 10..=11 as 0-based hidden lines.
        let hidden = [1..2, 9..11];
        assert_eq!(state_at(&model, 14, &hidden, 1.), vec![1]);
        assert_eq!(state_at(&model, 14, &hidden, LINE_HEIGHT + 1.), Vec::<usize>::new());
        assert_eq!(state_at(&model, 14, &hidden, 6. * LINE_HEIGHT + 1.), vec![7, 9]);
        assert_eq!(state_at(&model, 14, &hidden, 7. * LINE_HEIGHT + 1.), vec![7]);
        assert_eq!(state_at(&model, 14, &hidden, 10. * LINE_HEIGHT + 1.), Vec::<usize>::new());
    }

    /// issue #159271: a child scope starting on its parent's line. The outline provider skips the child and
    /// keeps its children (`_stickyModelFromOutlineElement`), so TestClass holds bar directly.
    #[test]
    fn child_scope_on_the_parent_line() {
        let model = StickyElement { range: None, children: vec![scope(1, 5, vec![scope(2, 4, vec![])])] };
        assert_eq!(state_at(&model, 6, &[], 1.), vec![1, 2]);
        assert_eq!(state_at(&model, 6, &[], LINE_HEIGHT + 1.), vec![1, 2]);
        assert_eq!(state_at(&model, 6, &[], 2. * LINE_HEIGHT + 1.), vec![1]);
        assert_eq!(state_at(&model, 6, &[], 3. * LINE_HEIGHT + 1.), vec![1]);
        assert_eq!(state_at(&model, 6, &[], 4. * LINE_HEIGHT + 1.), Vec::<usize>::new());
    }

    #[test]
    fn folding_regions_nest_into_scopes() {
        let lines: Vec<String> = ["class A:", "    def f():", "        pass", "    def g():", "        pass", "x = 1"].iter().map(|line| line.to_string()).collect();
        let model = from_folding_regions(&compute_ranges(&lines, 4, true, None));
        assert_eq!(model, StickyElement { range: None, children: vec![scope(1, 6, vec![scope(2, 4, vec![]), scope(4, 6, vec![])])] });
        let regions = FoldingRegions::new(vec![FoldRange::new(1, 4), FoldRange::new(2, 3), FoldRange::new(6, 8)]);
        assert_eq!(from_folding_regions(&regions), StickyElement { range: None, children: vec![scope(1, 5, vec![scope(2, 4, vec![])]), scope(6, 9, vec![])] });
    }

    /// `_toggleFoldingRegionForLine`: the toggled scope's start, or once collapsed its fold line, stays 1px
    /// below the top of its sticky line.
    #[test]
    fn a_folding_icon_click_keeps_the_scope_under_its_sticky_line() {
        let line_height = crate::editor::LINE_HEIGHT;
        let body = vec!["        x;"; 30].join("\n");
        let mut editor = crate::editor::edit::tests::editor(&format!("class A {{\n    b() {{\n{body}\n    }}\n}}\nx;"), "\n");
        let viewport = Viewport { width: 400., height: 360., char_width: 7.2, line_height: 18., sticky_scroll_lines: 5, top_whitespace: 0., stop_rendering_line_after: None };
        editor.set_scroll(10. * line_height, 0., viewport);
        assert_eq!(editor.sticky_scroll_state(viewport).start_line_numbers, vec![1, 2]);
        assert!(editor.toggle_sticky_fold(2, 1, viewport));
        assert_eq!((editor.folding.hidden(), editor.scroll.top), (&[2..32][..], 1.));
        assert!(editor.toggle_sticky_fold(2, 1, viewport));
        assert_eq!((editor.folding.hidden(), editor.scroll.top), (&[][..], 1.));
        assert!(!editor.toggle_sticky_fold(3, 2, viewport));
    }

    #[test]
    fn at_most_a_quarter_of_the_editor() {
        assert_eq!(max_sticky_lines(500., LINE_HEIGHT, 5), 5);
        assert_eq!(max_sticky_lines(500., LINE_HEIGHT, 3), 3);
        assert_eq!(max_sticky_lines(100., LINE_HEIGHT, 5), 1);
    }
}
