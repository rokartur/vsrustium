//! Ported from src/vs/editor/contrib/folding/test/browser: foldingRanges, foldingModel, hiddenRangeModel,
//! indentRangeProvider and indentFold.

use super::*;

fn lines(text: &[&str]) -> Vec<String> {
    text.iter().map(|line| line.to_string()).collect()
}

fn fold_range(start: usize, end: usize, collapsed: bool, source: FoldSource, kind: Option<&str>) -> FoldRange {
    FoldRange { start, end, kind: kind.map(str::to_string), collapsed, source }
}

fn provider(start: usize, end: usize, collapsed: bool, kind: &str) -> FoldRange {
    fold_range(start, end, collapsed, FoldSource::Provider, Some(kind))
}

fn region_markers() -> FoldingMarkers {
    FoldingMarkers::new((r"^\s*#region\b", ""), (r"^\s*#endregion\b", "")).unwrap()
}

fn comment_markers() -> FoldingMarkers {
    FoldingMarkers::new((r"^\/\/#region$", ""), (r"^\/\/#endregion$", "")).unwrap()
}

fn model_of(text: &[String], markers: Option<&FoldingMarkers>) -> FoldingModel {
    let mut model = FoldingModel::default();
    model.update(compute_ranges(text, 4, false, markers), text.len(), None);
    model
}

/// (start, end, collapsed) of every region.
fn ranges(model: &FoldingModel) -> Vec<(usize, usize, bool)> {
    model.regions.ranges().iter().map(|range| (range.start, range.end, range.collapsed)).collect()
}

fn folded(model: &FoldingModel) -> Vec<(usize, usize)> {
    model.regions.ranges().iter().filter(|range| range.collapsed).map(|range| (range.start, range.end)).collect()
}

fn regions_of(model: &FoldingModel, indexes: Vec<usize>) -> Vec<(usize, usize)> {
    indexes.into_iter().map(|index| (model.regions.get(index).start, model.regions.get(index).end)).collect()
}

/// (start, end, parent index) as indentRangeProvider.test.ts `assertRanges` compares them.
fn assert_indent_ranges(text: &[&str], expected: &[(usize, usize, Option<usize>)], off_side: bool, markers: Option<&FoldingMarkers>) {
    let actual = compute_ranges(&lines(text), 4, off_side, markers);
    let actual: Vec<_> = (0..actual.len()).map(|index| (actual.get(index).start, actual.get(index).end, actual.parent(index))).collect();
    assert_eq!(actual, expected, "{text:?}");
}

#[test]
fn test_max_folding_regions() {
    let mut text = Vec::new();
    let mut collected = Vec::new();
    for _ in 0..MAX_FOLDING_REGIONS {
        collected.insert(0, (text.len(), text.len() + 1, 0));
        text.push("#region".to_string());
        text.push("#endregion".to_string());
    }
    collected.reverse();
    let actual = to_indent_ranges(collected, &text, 4, MAX_FOLDING_REGIONS);
    assert_eq!(actual.len(), MAX_FOLDING_REGIONS);
}

#[test]
fn find_range() {
    let text = lines(&["#region", "#endregion", "class A {", "  void foo() {", "    if (true) {", "        return;", "    }", "", "    if (true) {", "      return;", "    }", "  }", "}"]);
    let actual = compute_ranges(&text, 4, false, Some(&region_markers()));
    let expected = [Some(0), Some(0), Some(1), Some(2), Some(3), Some(3), Some(2), Some(2), Some(4), Some(4), Some(2), Some(1), None];
    for (line, expected) in (1..).zip(expected) {
        assert_eq!(actual.find_range(line), expected, "{line}");
    }
}

#[test]
fn set_collapsed() {
    let mut text = vec!["#region".to_string(); 500];
    text.extend(vec!["#endregion".to_string(); 500]);
    let mut actual = compute_ranges(&text, 4, false, Some(&region_markers()));
    assert_eq!(actual.len(), 500);
    for index in 0..500 {
        actual.set_collapsed(index, index % 3 == 0);
    }
    for index in 0..500 {
        assert_eq!(actual.is_collapsed(index), index % 3 == 0, "line {index}");
    }
}

#[test]
fn sanitize_and_merge1() {
    let a = [
        fold_range(0, 100, false, FoldSource::Provider, None),
        provider(1, 100, false, "A"),
        provider(1, 100, false, "Z"),
        fold_range(10, 10, false, FoldSource::Provider, None),
        provider(20, 80, false, "C1"),
        provider(22, 80, true, "D1"),
        fold_range(90, 101, false, FoldSource::Provider, None),
    ];
    let b = [fold_range(20, 80, true, FoldSource::Provider, None), fold_range(18, 80, true, FoldSource::Provider, None), provider(21, 81, true, "Z"), provider(22, 80, true, "D2")];
    let result = FoldingRegions::sanitize_and_merge(&a, &b, 100, None);
    assert_eq!(result, [provider(1, 100, false, "A"), provider(20, 80, true, "C1"), provider(22, 80, true, "D1")]);
}

#[test]
fn sanitize_and_merge2() {
    let a = [provider(1, 100, false, "a1"), provider(2, 100, false, "a2"), provider(3, 19, false, "a3"), provider(20, 71, false, "a4"), provider(21, 29, false, "a5"), provider(81, 91, false, "a6")];
    let user = |start, end, kind| fold_range(start, end, true, FoldSource::UserDefined, Some(kind));
    let b = [provider(30, 39, true, "b1"), user(40, 49, "b2"), user(50, 100, "b3"), user(80, 90, "b4"), user(92, 100, "b5")];
    let result = FoldingRegions::sanitize_and_merge(&a, &b, 100, None);
    let expected = [
        provider(1, 100, false, "a1"),
        provider(2, 100, false, "a2"),
        provider(3, 19, false, "a3"),
        provider(21, 29, false, "a5"),
        fold_range(30, 39, true, FoldSource::Recovered, Some("b1")),
        user(40, 49, "b2"),
        user(50, 100, "b3"),
        user(80, 90, "b4"),
        user(92, 100, "b5"),
    ];
    assert_eq!(result, expected);
}

#[test]
fn sanitize_and_merge3() {
    let recovered = |start, end, kind| fold_range(start, end, true, FoldSource::Recovered, Some(kind));
    let a = [provider(1, 100, false, "a1"), provider(10, 29, false, "a2"), recovered(35, 39, "a3")];
    let b = [recovered(10, 29, "b1"), provider(20, 28, true, "b2"), recovered(30, 39, "b3")];
    let result = FoldingRegions::sanitize_and_merge(&a, &b, 100, None);
    assert_eq!(result, [provider(1, 100, false, "a1"), provider(10, 29, true, "a2"), recovered(20, 28, "b2"), recovered(30, 39, "b3"), recovered(35, 39, "a3")]);
}

#[test]
fn sanitize_and_merge4() {
    let a = [provider(1, 100, false, "a1")];
    let b = [provider(20, 28, true, "b1"), provider(30, 38, true, "b2")];
    let result = FoldingRegions::sanitize_and_merge(&a, &b, 100, None);
    let recovered = |start, end, kind| fold_range(start, end, true, FoldSource::Recovered, Some(kind));
    assert_eq!(result, [provider(1, 100, false, "a1"), recovered(20, 28, "b1"), recovered(30, 38, "b2")]);
}

const COMMENTED_CLASS: [&str; 8] = ["/**", " * Comment", " */", "class A {", "  void foo() {", "    // comment {", "  }", "}"];

#[test]
fn get_region_at_line() {
    let model = model_of(&lines(&COMMENTED_CLASS), None);
    assert_eq!(ranges(&model), [(1, 3, false), (4, 7, false), (5, 6, false)]);
    let expected = [Some(0), Some(0), Some(0), Some(1), Some(2), Some(2), Some(1), None];
    for (line, expected) in (1..).zip(expected) {
        assert_eq!(model.region_at_line(line), expected, "{line}");
    }
}

#[test]
fn collapse() {
    let text = lines(&COMMENTED_CLASS);
    let mut model = model_of(&text, None);
    let computed = compute_ranges(&text, 4, false, None);
    model.toggle_collapse_state(vec![model.region_at_line(1).unwrap()]);
    model.update(computed.clone(), text.len(), None);
    assert_eq!(ranges(&model), [(1, 3, true), (4, 7, false), (5, 6, false)]);
    model.toggle_collapse_state(vec![model.region_at_line(5).unwrap()]);
    model.update(computed.clone(), text.len(), None);
    assert_eq!(ranges(&model), [(1, 3, true), (4, 7, false), (5, 6, true)]);
    model.toggle_collapse_state(vec![model.region_at_line(7).unwrap()]);
    model.update(computed, text.len(), None);
    assert_eq!(ranges(&model), [(1, 3, true), (4, 7, true), (5, 6, true)]);
}

/// Applies an edit to `text` and moves the model's regions with it, as the text model's decorations do.
fn edit(model: &mut FoldingModel, text: &mut Vec<String>, start: Position, end: Position, inserted: &str) {
    model.accept_replace(text, start, end, inserted);
    let mut joined = text.join("\n");
    let offset = |position: Position| text[..position.line].iter().map(|line| line.len() + 1).sum::<usize>() + text[position.line].char_indices().nth(position.column).map_or(text[position.line].len(), |(index, _)| index);
    joined.replace_range(offset(start)..offset(end), inserted);
    *text = joined.split('\n').map(str::to_string).collect();
}

#[test]
fn update() {
    let mut text = lines(&COMMENTED_CLASS);
    let mut model = model_of(&text, None);
    model.toggle_collapse_state(vec![model.region_at_line(2).unwrap(), model.region_at_line(5).unwrap()]);
    edit(&mut model, &mut text, Position::new(3, 0), Position::new(3, 0), "//hello\n");
    model.update(compute_ranges(&text, 4, false, None), text.len(), None);
    assert_eq!(ranges(&model), [(1, 3, true), (5, 8, false), (6, 7, true)]);
}

#[test]
fn delete() {
    let mut text = lines(&[
        "function foo() {",
        "  switch (x) {",
        "    case 1:",
        "      //hello1",
        "      break;",
        "    case 2:",
        "      //hello2",
        "      break;",
        "    case 3:",
        "      //hello3",
        "      break;",
        "  }",
        "}",
    ]);
    let mut model = model_of(&text, None);
    assert_eq!(ranges(&model), [(1, 12, false), (2, 11, false), (3, 5, false), (6, 8, false), (9, 11, false)]);
    model.toggle_collapse_state(vec![model.region_at_line(6).unwrap()]);
    edit(&mut model, &mut text, Position::new(5, 10), Position::new(8, 0), "");
    model.update(compute_ranges(&text, 4, true, None), text.len(), Some(&[7]));
    assert_eq!(ranges(&model), [(1, 9, false), (2, 8, false), (3, 5, false), (6, 8, false)]);
}

#[test]
fn get_regions_inside() {
    let model = model_of(&lines(&COMMENTED_CLASS), None);
    let inside = |region| regions_of(&model, model.regions_inside(region, |_, _| true));
    assert_eq!(inside(None), [(1, 3), (4, 7), (5, 6)]);
    assert_eq!(inside(Some(0)), []);
    assert_eq!(inside(Some(1)), [(5, 6)]);
    assert_eq!(inside(Some(2)), []);
}

const NESTED_IFS: [&str; 13] = ["//#region", "//#endregion", "class A {", "  void foo() {", "    if (true) {", "        return;", "    }", "", "    if (true) {", "      return;", "    }", "  }", "}"];

#[test]
fn get_regions_inside_with_level() {
    let text = lines(&["//#region", "//#endregion", "class A {", "  void foo() {", "    if (true) {", "        return;", "    }", "    if (true) {", "      return;", "    }", "  }", "}"]);
    let model = model_of(&text, Some(&comment_markers()));
    assert_eq!(ranges(&model), [(1, 2, false), (3, 11, false), (4, 10, false), (5, 6, false), (8, 9, false)]);
    let at_level = |region, wanted| regions_of(&model, model.regions_inside(region, |_, level| level == wanted));
    assert_eq!(at_level(None, 1), [(1, 2), (3, 11)]);
    assert_eq!(at_level(None, 2), [(4, 10)]);
    assert_eq!(at_level(None, 3), [(5, 6), (8, 9)]);
    assert_eq!(at_level(Some(1), 1), [(4, 10)]);
    assert_eq!(at_level(Some(1), 2), [(5, 6), (8, 9)]);
    assert_eq!(at_level(Some(2), 1), [(5, 6), (8, 9)]);
    assert_eq!(regions_of(&model, model.regions_inside(Some(1), |range, _| range.start < 9 && 9 <= range.end)), [(4, 10), (8, 9)]);
    assert_eq!(at_level(Some(0), 1), []);
}

#[test]
fn get_region_at_line2() {
    let text = lines(&["//#region", "class A {", "  void foo() {", "    if (true) {", "      //hello", "    }", "", "  }", "}", "//#endregion", ""]);
    let model = model_of(&text, Some(&comment_markers()));
    assert_eq!(ranges(&model), [(1, 10, false), (2, 8, false), (3, 7, false), (4, 5, false)]);
    let at = |line| regions_of(&model, model.all_regions_at_line(line, |_, _| true));
    let (r1, r2, r3, r4) = ((1, 10), (2, 8), (3, 7), (4, 5));
    assert_eq!(at(1), [r1]);
    assert_eq!(at(2), [r2, r1]);
    assert_eq!(at(3), [r3, r2, r1]);
    assert_eq!(at(4), [r4, r3, r2, r1]);
    assert_eq!(at(5), [r4, r3, r2, r1]);
    assert_eq!(at(6), [r3, r2, r1]);
    assert_eq!(at(7), [r3, r2, r1]);
    assert_eq!(at(8), [r2, r1]);
    assert_eq!(at(9), [r1]);
    assert_eq!(at(10), [r1]);
    assert_eq!(at(11), []);
}

const R1: (usize, usize) = (1, 2);
const R2: (usize, usize) = (3, 12);
const R3: (usize, usize) = (4, 11);
const R4: (usize, usize) = (5, 6);
const R5: (usize, usize) = (9, 10);

#[test]
fn set_collapse_state_recursivly() {
    let mut model = model_of(&lines(&NESTED_IFS), Some(&comment_markers()));
    assert_eq!(ranges(&model), [(1, 2, false), (3, 12, false), (4, 11, false), (5, 6, false), (9, 10, false)]);
    set_collapse_state_levels_down(&mut model, true, usize::MAX, Some(&[4]));
    assert_eq!(folded(&model), [R3, R4, R5]);
    set_collapse_state_levels_down(&mut model, false, usize::MAX, Some(&[8]));
    assert_eq!(folded(&model), []);
    set_collapse_state_levels_down(&mut model, true, usize::MAX, Some(&[12]));
    assert_eq!(folded(&model), [R2, R3, R4, R5]);
    set_collapse_state_levels_down(&mut model, false, usize::MAX, Some(&[7]));
    assert_eq!(folded(&model), [R2]);
    set_collapse_state_levels_down(&mut model, false, usize::MAX, None);
    assert_eq!(folded(&model), []);
    set_collapse_state_levels_down(&mut model, true, usize::MAX, None);
    assert_eq!(folded(&model), [R1, R2, R3, R4, R5]);
}

#[test]
fn set_collapse_state_at_level_test() {
    let mut text = lines(&NESTED_IFS);
    text.truncate(12);
    text.extend(lines(&["  //#region", "  const bar = 9;", "  //#endregion", "}"]));
    let markers = FoldingMarkers::new((r"^\s*\/\/#region$", ""), (r"^\s*\/\/#endregion$", "")).unwrap();
    let mut model = model_of(&text, Some(&markers));
    let (r2, r6) = ((3, 15), (13, 15));
    assert_eq!(ranges(&model), [(1, 2, false), (3, 15, false), (4, 11, false), (5, 6, false), (9, 10, false), (13, 15, false)]);
    set_collapse_state_at_level(&mut model, 1, true, &[]);
    assert_eq!(folded(&model), [R1, r2]);
    set_collapse_state_at_level(&mut model, 1, false, &[5]);
    assert_eq!(folded(&model), [r2]);
    set_collapse_state_at_level(&mut model, 1, false, &[1]);
    assert_eq!(folded(&model), []);
    set_collapse_state_at_level(&mut model, 2, true, &[]);
    assert_eq!(folded(&model), [R3, r6]);
    set_collapse_state_at_level(&mut model, 2, false, &[5, 6]);
    assert_eq!(folded(&model), [R3]);
    set_collapse_state_at_level(&mut model, 3, true, &[4, 9]);
    assert_eq!(folded(&model), [R3, R4]);
    set_collapse_state_at_level(&mut model, 3, false, &[4, 9]);
    assert_eq!(folded(&model), [R3]);
}

#[test]
fn set_collapse_state_levels_down_test() {
    let mut model = model_of(&lines(&NESTED_IFS), Some(&comment_markers()));
    set_collapse_state_levels_down(&mut model, true, 1, Some(&[4]));
    assert_eq!(folded(&model), [R3]);
    set_collapse_state_levels_down(&mut model, true, 2, Some(&[4]));
    assert_eq!(folded(&model), [R3, R4, R5]);
    set_collapse_state_levels_down(&mut model, false, 2, Some(&[3]));
    assert_eq!(folded(&model), [R4, R5]);
    set_collapse_state_levels_down(&mut model, false, 2, Some(&[2]));
    assert_eq!(folded(&model), [R4, R5]);
    set_collapse_state_levels_down(&mut model, true, 4, Some(&[2]));
    assert_eq!(folded(&model), [R1, R4, R5]);
    set_collapse_state_levels_down(&mut model, false, 4, Some(&[2, 3]));
    assert_eq!(folded(&model), []);
}

#[test]
fn set_collapse_state_levels_up_test() {
    let mut model = model_of(&lines(&NESTED_IFS), Some(&comment_markers()));
    set_collapse_state_levels_up(&mut model, true, 1, &[4]);
    assert_eq!(folded(&model), [R3]);
    set_collapse_state_levels_up(&mut model, true, 2, &[4]);
    assert_eq!(folded(&model), [R2, R3]);
    set_collapse_state_levels_up(&mut model, false, 4, &[1, 3, 4]);
    assert_eq!(folded(&model), []);
    set_collapse_state_levels_up(&mut model, true, 2, &[10]);
    assert_eq!(folded(&model), [R3, R5]);
}

#[test]
fn set_collapse_state_up_test() {
    let mut model = model_of(&lines(&NESTED_IFS), Some(&comment_markers()));
    set_collapse_state_up(&mut model, true, &[5]);
    assert_eq!(folded(&model), [R4]);
    set_collapse_state_up(&mut model, true, &[5]);
    assert_eq!(folded(&model), [R3, R4]);
    set_collapse_state_up(&mut model, true, &[4]);
    assert_eq!(folded(&model), [R2, R3, R4]);
}

#[test]
fn set_collapse_state_for_matching_lines_test() {
    let text = lines(&["/**", " * the class", " */", "class A {", "  /**", "   * the foo", "   */", "  void foo() {", "    /*", "     * the comment", "     */", "  }", "}"]);
    let mut model = model_of(&text, Some(&comment_markers()));
    assert_eq!(ranges(&model), [(1, 3, false), (4, 12, false), (5, 7, false), (8, 11, false), (9, 11, false)]);
    set_collapse_state_for_matching_lines(&mut model, &text, &onig::Regex::new(r"^\s*/\*").unwrap(), true);
    assert_eq!(folded(&model), [(1, 3), (5, 7), (9, 11)]);
}

#[test]
fn set_collapse_state_for_rest_test() {
    let mut model = model_of(&lines(&NESTED_IFS), Some(&comment_markers()));
    set_collapse_state_for_rest(&mut model, true, &[5]);
    assert_eq!(folded(&model), [R1, R5]);
    set_collapse_state_for_rest(&mut model, false, &[5]);
    assert_eq!(folded(&model), []);
    set_collapse_state_for_rest(&mut model, true, &[1]);
    assert_eq!(folded(&model), [R2, R3, R4, R5]);
    set_collapse_state_for_rest(&mut model, true, &[3]);
    assert_eq!(folded(&model), [R1, R2, R3, R4, R5]);
}

#[test]
fn folding_decoration() {
    let text = lines(&["class A {", "  void foo() {", "    if (true) {", "      hoo();", "    }", "  }", "}"]);
    let mut model = model_of(&text, None);
    let computed = compute_ranges(&text, 4, false, None);
    use FoldDecoration::*;
    assert_eq!(model.decorations(), [Expanded, Expanded, Expanded]);
    model.toggle_collapse_state(vec![model.region_at_line(2).unwrap()]);
    assert_eq!(ranges(&model), [(1, 6, false), (2, 5, true), (3, 4, false)]);
    assert_eq!(model.decorations(), [Expanded, Collapsed, Hidden]);
    model.update(computed.clone(), text.len(), None);
    assert_eq!(model.decorations(), [Expanded, Collapsed, Hidden]);
    model.toggle_collapse_state(vec![model.region_at_line(1).unwrap()]);
    assert_eq!(ranges(&model), [(1, 6, true), (2, 5, true), (3, 4, false)]);
    assert_eq!(model.decorations(), [Collapsed, Hidden, Hidden]);
    model.update(computed.clone(), text.len(), None);
    assert_eq!(model.decorations(), [Collapsed, Hidden, Hidden]);
    model.toggle_collapse_state(vec![model.region_at_line(1).unwrap(), model.region_at_line(3).unwrap()]);
    assert_eq!(ranges(&model), [(1, 6, false), (2, 5, true), (3, 4, true)]);
    assert_eq!(model.decorations(), [Expanded, Collapsed, Hidden]);
    model.update(computed, text.len(), None);
    assert_eq!(model.decorations(), [Expanded, Collapsed, Hidden]);
}

#[test]
fn fold_jumping() {
    let text = lines(&["class A {", "  void foo() {", "    if (1) {", "      a();", "    } else if (2) {", "      if (true) {", "        b();", "      }", "    } else {", "      c();", "    }", "  }", "}"]);
    let model = model_of(&text, None);
    assert_eq!(ranges(&model), [(1, 12, false), (2, 11, false), (3, 4, false), (5, 8, false), (6, 7, false), (9, 10, false)]);
    let parent = |line| parent_fold_line(line, &model);
    let previous = |line| previous_fold_line(line, &model);
    let next = |line| next_fold_line(line, &model);
    assert_eq!([parent(7), parent(6), parent(5), parent(2), parent(1)], [Some(6), Some(5), Some(2), Some(1), None]);
    assert_eq!([previous(10), previous(9), previous(5), previous(3), previous(4), previous(7), previous(8)], [Some(9), Some(5), Some(3), None, Some(3), Some(6), Some(6)]);
    assert_eq!([next(3), next(5), next(9), next(4), next(7), next(8)], [Some(5), Some(9), None, Some(5), Some(9), Some(9)]);
}

#[test]
fn fold_jumping_issue_129503() {
    let model = model_of(&lines(&["", "if True:", "  print(1)", "if True:", "  print(1)", ""]), None);
    assert_eq!(ranges(&model), [(2, 3, false), (4, 6, false)]);
    let next: Vec<_> = (1..=6).map(|line| next_fold_line(line, &model)).collect();
    assert_eq!(next, [Some(2), Some(4), Some(4), None, None, None]);
    let previous: Vec<_> = (1..=6).map(|line| previous_fold_line(line, &model)).collect();
    assert_eq!(previous, [None, None, Some(2), Some(2), Some(4), Some(4)]);
}

fn manual_model(regions: &[(usize, usize, FoldSource)]) -> FoldingModel {
    let mut model = FoldingModel::default();
    let ranges = regions.iter().map(|&(start, end, source)| fold_range(start, end, false, source, None)).collect();
    model.update(FoldingRegions::new(ranges), 100, None);
    model
}

/// A selection from 1-based (line, column) to (line, column), as `new Range` gives it.
fn range(start: (usize, usize), end: (usize, usize)) -> Selection {
    Selection { anchor: Position::new(start.0 - 1, start.1 - 1), head: Position::new(end.0 - 1, end.1 - 1) }
}

#[test]
fn remove_manual_ranges_cursor_on_manual_range_removes_innermost_only() {
    use FoldSource::*;
    let mut model = manual_model(&[(1, 6, Provider), (2, 5, UserDefined), (3, 4, UserDefined)]);
    assert_eq!(ranges(&model), [(1, 6, false), (2, 5, false), (3, 4, false)]);
    model.remove_manual_ranges(&[range((4, 1), (4, 1))]);
    assert_eq!(ranges(&model), [(1, 6, false), (2, 5, false)]);
    model.remove_manual_ranges(&[range((3, 1), (3, 1))]);
    assert_eq!(ranges(&model), [(1, 6, false)]);
}

#[test]
fn remove_manual_ranges_cursor_skips_provider_ranges_to_remove_nearest_manual_range() {
    use FoldSource::*;
    let mut model = manual_model(&[(1, 5, UserDefined), (2, 4, Provider)]);
    model.remove_manual_ranges(&[range((3, 1), (3, 1))]);
    assert_eq!(ranges(&model), [(2, 4, false)]);
}

#[test]
fn remove_manual_ranges_cursor_not_on_manual_range_removes_all_manual_ranges() {
    use FoldSource::*;
    let mut model = manual_model(&[(2, 4, Provider), (3, 4, UserDefined)]);
    assert_eq!(ranges(&model), [(2, 4, false), (3, 4, false)]);
    model.remove_manual_ranges(&[range((6, 1), (6, 2))]);
    assert_eq!(ranges(&model), [(2, 4, false), (3, 4, false)]);
    model.remove_manual_ranges(&[range((6, 1), (6, 1))]);
    assert_eq!(ranges(&model), [(2, 4, false)]);
}

#[test]
fn remove_manual_ranges_single_line_selection_removes_all_intersecting_manual_ranges() {
    use FoldSource::*;
    let mut model = manual_model(&[(1, 6, Provider), (2, 5, UserDefined), (3, 4, UserDefined)]);
    model.remove_manual_ranges(&[range((4, 1), (4, 2))]);
    assert_eq!(ranges(&model), [(1, 6, false)]);
}

#[test]
fn remove_manual_ranges_selection_range_removes_intersecting_manual_ranges() {
    use FoldSource::*;
    let mut model = manual_model(&[(1, 5, Provider), (2, 3, UserDefined), (4, 5, UserDefined)]);
    assert_eq!(ranges(&model), [(1, 5, false), (2, 3, false), (4, 5, false)]);
    model.remove_manual_ranges(&[range((2, 1), (3, 1))]);
    assert_eq!(ranges(&model), [(1, 5, false), (4, 5, false)]);
}

#[test]
fn has_ranges() {
    let text = lines(&["/**", " * Comment", " */", "class A {", "  void foo() {", "    if (true) {", "      //hello", "    }", "  }", "}"]);
    let mut model = model_of(&text, None);
    assert!(model.hidden().is_empty());
    let hidden_lines = |model: &FoldingModel| (1..=10).filter(|line| model.is_hidden(line - 1)).collect::<Vec<_>>();
    model.toggle_collapse_state(vec![model.region_at_line(1).unwrap(), model.region_at_line(6).unwrap()]);
    // hiddenRangeModel's 1-based r(2, 3), r(7, 7).
    assert_eq!(model.hidden(), [1..3, 6..7]);
    assert_eq!(hidden_lines(&model), [2, 3, 7]);
    model.toggle_collapse_state(vec![model.region_at_line(4).unwrap()]);
    assert_eq!(model.hidden(), [1..3, 4..9]);
    assert_eq!(hidden_lines(&model), [2, 3, 5, 6, 7, 8, 9]);
    model.toggle_collapse_state(vec![model.region_at_line(1).unwrap(), model.region_at_line(6).unwrap(), model.region_at_line(4).unwrap()]);
    assert!(model.hidden().is_empty());
    assert!(hidden_lines(&model).is_empty());
}

#[test]
fn view_lines_skip_hidden_lines() {
    let text = lines(&["/**", " * Comment", " */", "class A {", "  void foo() {", "    if (true) {", "      //hello", "    }", "  }", "}"]);
    let mut model = model_of(&text, None);
    model.toggle_collapse_state(vec![model.region_at_line(1).unwrap(), model.region_at_line(6).unwrap()]);
    assert_eq!(model.view_line_count(text.len()), 7);
    let view: Vec<_> = (0..10).map(|line| model.view_line(line)).collect();
    assert_eq!(view, [0, 0, 0, 1, 2, 3, 3, 4, 5, 6]);
    let shown: Vec<_> = (0..7).map(|line| model.model_line(line)).collect();
    assert_eq!(shown, [0, 3, 4, 5, 7, 8, 9]);
    assert_eq!(model.shown_ranges(0, 9), [(0, 0), (3, 5), (7, 9)]);
    assert_eq!(model.shown_ranges(4, 5), [(4, 5)]);
    assert_eq!(shown_neighbour(10, model.hidden(), 0, true), Some(3));
    assert_eq!(shown_neighbour(10, model.hidden(), 3, false), Some(0));
    assert_eq!(shown_neighbour(10, model.hidden(), 9, true), None);
    assert_eq!(shown_position(&text, model.hidden(), Position::new(1, 0), true), Position::new(3, 0));
    assert_eq!(shown_position(&text, model.hidden(), Position::new(2, 3), false), Position::new(0, 3));
    assert_eq!(shown_position(&text, model.hidden(), Position::new(4, 1), true), Position::new(4, 1));
}

#[test]
fn memento_restores_regions_whose_lines_are_unchanged() {
    let text = lines(&COMMENTED_CLASS);
    let mut model = model_of(&text, None);
    model.toggle_collapse_state(vec![model.region_at_line(5).unwrap()]);
    let memento = model.get_memento(&text).unwrap();
    assert_eq!(memento.len(), 1);
    let mut restored = model_of(&text, None);
    restored.apply_memento(&text, &memento);
    assert_eq!(folded(&restored), [(5, 6)]);
    let mut changed = text.clone();
    changed[5] = "    // changed {".into();
    let mut stale = model_of(&changed, None);
    stale.apply_memento(&changed, &memento);
    assert_eq!(folded(&stale), []);
}

#[test]
fn fold_one_level() {
    let text = ["A", "  A", "  A", "  A"];
    assert_indent_ranges(&text, &[(1, 4, None)], true, None);
    assert_indent_ranges(&text, &[(1, 4, None)], false, None);
}

#[test]
fn fold_two_levels() {
    let text = ["A", "  A", "  A", "    A", "    A"];
    assert_indent_ranges(&text, &[(1, 5, None), (3, 5, Some(0))], true, None);
    assert_indent_ranges(&text, &[(1, 5, None), (3, 5, Some(0))], false, None);
}

#[test]
fn fold_three_levels() {
    let text = ["A", "  A", "    A", "      A", "A"];
    let expected = [(1, 4, None), (2, 4, Some(0)), (3, 4, Some(1))];
    assert_indent_ranges(&text, &expected, true, None);
    assert_indent_ranges(&text, &expected, false, None);
}

#[test]
fn fold_decreasing_indent() {
    let text = ["    A", "  A", "A"];
    assert_indent_ranges(&text, &[], true, None);
    assert_indent_ranges(&text, &[], false, None);
}

#[test]
fn fold_java() {
    let text = ["class A {", "  void foo() {", "    console.log();", "    console.log();", "  }", "", "  void bar() {", "    console.log();", "  }", "}", "interface B {", "  void bar();", "}"];
    assert_indent_ranges(&text, &[(1, 9, None), (2, 4, Some(0)), (7, 8, Some(0)), (11, 12, None)], false, None);
}

#[test]
fn fold_javadoc() {
    let text = ["/**", " * Comment", " */", "class A {", "  void foo() {", "  }", "}"];
    assert_indent_ranges(&text, &[(1, 3, None), (4, 6, None)], false, None);
}

#[test]
fn fold_whitespace_java() {
    let text = ["class A {", "", "  void foo() {", "     ", "     return 0;", "  }", "      ", "}"];
    assert_indent_ranges(&text, &[(1, 7, None), (3, 5, Some(0))], false, None);
}

#[test]
fn fold_whitespace_python() {
    let text = ["def a:", "  pass", "   ", "  def b:", "    pass", "  ", "      ", "def c: # since there was a deintent here"];
    assert_indent_ranges(&text, &[(1, 5, None), (4, 5, Some(0))], true, None);
}

#[test]
fn fold_tabs() {
    let text = ["class A {", "\t\t", "\tvoid foo() {", "\t \t//hello", "\t    return 0;", "  \t}", "      ", "}"];
    assert_indent_ranges(&text, &[(1, 7, None), (3, 5, Some(0))], false, None);
}

#[test]
fn folding_with_regions() {
    let markers = region_markers();
    let cases: [(&[&str], &[(usize, usize, Option<usize>)], bool); 15] = [
        (&["class A {", "  #region", "  void foo() {", "     ", "     return 0;", "  }", "  #endregion", "}"], &[(1, 7, None), (2, 7, Some(0)), (3, 5, Some(1))], false),
        (&["var x;", "#region", "void foo() {", "     ", "     return 0;", "  }", "#endregion", ""], &[(2, 7, None), (3, 6, Some(0))], false),
        (&["var x;", "#region", "#endregion", "#region", "", "#endregion", "var y;"], &[(2, 3, None), (4, 6, None)], false),
        (&["var x;", "#region", "#region", "", "#endregion", "#endregion", "var y;"], &[(2, 6, None), (3, 5, Some(0))], false),
        (&["class A {", "  #region", "", "  #region", "", "  #endregion", "  // comment", "  #endregion", "}"], &[(1, 8, None), (2, 8, Some(0)), (4, 6, Some(1))], false),
        (&["class A {", "#region", "  // comment", "}"], &[(2, 3, None)], false),
        (&["", "#region", "#region", "#region", "  // comment", "#endregion", "#endregion", " // hello"], &[(3, 7, None), (4, 6, Some(0))], false),
        (&["if (x)", "  return;", "", "#region", "  // comment", "#endregion"], &[(1, 3, None), (4, 6, None)], false),
        (&["if (x)", "  log();", "", "    #region", "      // comment", "    #endregion"], &[(1, 6, None), (2, 6, Some(0)), (4, 6, Some(1))], false),
        (&["#region", "  // comment", "  if (x)", "    return;", "", "#endregion"], &[(1, 6, None), (3, 5, Some(0))], false),
        (&["#region", "  // comment", "", "#endregion", "  if (x)", "    return;"], &[(1, 4, None), (5, 6, None)], false),
        (&["#region", "  ", "", "#endregion", ""], &[(1, 4, None)], true),
        (&["#region", "  ", "#region", "", "#endregion", "", "#endregion", ""], &[(1, 7, None), (3, 5, Some(0))], true),
        (
            &["function thisFoldsToEndOfPage() {", "  const variable = []", "    // #region", "    .reduce((a, b) => a,[]);", "}", "", "function thisFoldsProperly() {", "  const foo = \"bar\"", "}"],
            &[(1, 4, None), (2, 4, Some(0)), (7, 8, None)],
            false,
        ),
        (&["#Region", "#endregion", "#regionsandmore", "#endregion", "#region", "#end region", "#region", "#endregionff"], &[], true),
    ];
    for (text, expected, off_side) in cases {
        assert_indent_ranges(text, expected, off_side, Some(&markers));
    }
}

#[test]
fn issue_79359() {
    let text = ["#region", "", "class A", "  foo", "", "class A", "  foo", "", "#endregion"];
    assert_indent_ranges(&text, &[(1, 9, None), (3, 4, Some(0)), (6, 7, Some(0))], true, Some(&region_markers()));
}

#[test]
fn markers_with_stateful_flags() {
    let markers = |start_flags, end_flags| FoldingMarkers::new((r"^\s*#region\b", start_flags), (r"^\s*#endregion\b", end_flags)).unwrap();
    assert_indent_ranges(&["#region", "content", "#endregion"], &[(1, 3, None)], false, Some(&markers("g", "g")));
    assert_indent_ranges(&["#REGION", "content", "#endregion"], &[(1, 3, None)], false, Some(&markers("gi", "g")));
    assert_indent_ranges(&["#REGION", "content", "#ENDREGION"], &[], false, Some(&markers("gi", "g")));
    assert_indent_ranges(&["#REGION", "content", "#ENDREGION"], &[], false, Some(&markers("g", "gi")));
}

#[test]
fn limit_by_indent() {
    let text = lines(&["A", "  A", "  A", "    A", "      A", "    A", "      A", "      A", "         A", "      A", "         A", "  A", "              A", "                 A", "A", "  A"]);
    let (r1, r2, r3, r4, r5, r6, r7, r8, r9) = ((1, 14), (3, 11), (4, 5), (6, 11), (8, 9), (10, 11), (12, 14), (13, 14), (15, 16));
    let limited = |limit| {
        let regions = compute_ranges_limited(&text, 4, true, None, limit);
        regions.ranges().iter().map(|range| (range.start, range.end)).collect::<Vec<_>>()
    };
    assert_eq!(limited(1000), [r1, r2, r3, r4, r5, r6, r7, r8, r9]);
    assert_eq!(limited(9), [r1, r2, r3, r4, r5, r6, r7, r8, r9]);
    assert_eq!(limited(8), [r1, r2, r3, r4, r5, r6, r7, r9]);
    assert_eq!(limited(7), [r1, r2, r3, r4, r5, r7, r9]);
    assert_eq!(limited(6), [r1, r2, r3, r4, r7, r9]);
    assert_eq!(limited(5), [r1, r2, r3, r7, r9]);
    assert_eq!(limited(4), [r1, r2, r7, r9]);
    assert_eq!(limited(3), [r1, r2, r9]);
    assert_eq!(limited(2), [r1, r9]);
    assert_eq!(limited(1), [r1]);
    assert_eq!(limited(0), []);
}
