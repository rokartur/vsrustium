//! multicursor.test.ts: the find-match actions, "Selected text occurrence matching" and "Find state disassociation".

use super::*;
use crate::editor::edit::tests::editor;
use crate::editor::find::controller::{FindOptions, QueryOptions};

const VIEWPORT: Viewport = Viewport { width: 800., height: 600., char_width: 7.2, line_height: 18., sticky_scroll_lines: 0, top_whitespace: 0., stop_rendering_line_after: None };

#[derive(Clone, Copy, Debug)]
enum Action {
    AddNext,
    AddPrevious,
    MoveNext,
    MovePrevious,
    SelectHighlights,
}

fn run(action: Action, find: &mut FindController, editor: &mut OpenEditor, context: &MultiCursorContext) {
    match action {
        Action::AddNext => selection_to_find_match(find, editor, VIEWPORT, context, true, true),
        Action::AddPrevious => selection_to_find_match(find, editor, VIEWPORT, context, false, true),
        Action::MoveNext => selection_to_find_match(find, editor, VIEWPORT, context, true, false),
        Action::MovePrevious => selection_to_find_match(find, editor, VIEWPORT, context, false, false),
        Action::SelectHighlights => select_all(find, editor, VIEWPORT, context),
    }
}

fn context(match_mode: &str) -> MultiCursorContext {
    MultiCursorContext { text_focused: true, match_mode: match_mode.into() }
}

fn setup(lines: &[&str]) -> (FindController, OpenEditor) {
    (FindController::new(FindOptions::default(), QueryOptions::default()), editor(&lines.join("\n"), "\n"))
}

/// 1-based [start line, start column, end line, end column].
fn select(editor: &mut OpenEditor, ranges: &[[usize; 4]]) {
    let selection = |[a, b, c, d]: [usize; 4]| Selection { anchor: Position::new(a - 1, b - 1), head: Position::new(c - 1, d - 1) };
    editor.set_selections(ranges.iter().map(|range| selection(*range)).collect());
}

fn selections(editor: &OpenEditor) -> Vec<[usize; 4]> {
    editor.selections.iter().map(|selection| [selection.start().line + 1, selection.start().column + 1, selection.end().line + 1, selection.end().column + 1]).collect()
}

fn options(find: &FindController) -> [bool; 3] {
    [find.state.match_case, find.state.whole_word, find.state.is_regex]
}

fn change(find: &mut FindController, editor: &mut OpenEditor, new: NewFindState) {
    find.change(editor, VIEWPORT, new, false);
}

fn lines_selected(lines: &[usize]) -> Vec<[usize; 4]> {
    lines.iter().map(|&line| [line, 1, line, 4]).collect()
}

const OCCURRENCES: [&str; 5] = ["foo", "FOObar", "foobar", "FOO", "foo"];

#[test]
fn selected_text_matches_with_the_setting_and_keeps_the_find_options() {
    use Action::*;
    let cases = [
        (AddNext, 1, vec![1, 2], vec![1, 3]),
        (AddPrevious, 5, vec![5, 4], vec![5, 3]),
        (MoveNext, 1, vec![2], vec![3]),
        (MovePrevious, 5, vec![4], vec![3]),
        (SelectHighlights, 1, vec![1, 2, 3, 4, 5], vec![1, 3, 5]),
    ];
    for (action, start, insensitive, sensitive) in cases {
        for (match_mode, expected) in [("caseInsensitive", &insensitive), ("caseSensitive", &sensitive)] {
            for is_revealed in [false, true] {
                let (mut find, mut editor) = setup(&OCCURRENCES);
                let match_case = match_mode == "caseSensitive";
                let new = NewFindState { search_string: Some("FOO.*".into()), is_revealed: Some(is_revealed), match_case: Some(!match_case), whole_word: Some(true), is_regex: Some(true), ..Default::default() };
                change(&mut find, &mut editor, new);
                select(&mut editor, &[[start, 1, start, 4]]);
                run(action, &mut find, &mut editor, &context(match_mode));
                let actual = (selections(&editor), options(&find));
                assert_eq!(actual, (lines_selected(expected), [!match_case, true, true]), "{action:?} {match_mode} revealed {is_revealed}");
            }
        }
    }
}

#[test]
fn an_empty_selection_always_starts_a_whole_word_case_sensitive_search() {
    use Action::*;
    let cases = [(AddNext, 1, vec![1, 5]), (AddPrevious, 5, vec![5, 1]), (MoveNext, 1, vec![5]), (MovePrevious, 5, vec![1]), (SelectHighlights, 1, vec![1, 5])];
    for (action, start, expected) in cases {
        for match_mode in ["caseSensitive", "caseInsensitive"] {
            let (mut find, mut editor) = setup(&OCCURRENCES);
            change(&mut find, &mut editor, NewFindState { match_case: Some(false), whole_word: Some(false), is_regex: Some(true), ..Default::default() });
            select(&mut editor, &[[start, 2, start, 2]]);
            run(action, &mut find, &mut editor, &context(match_mode));
            run(action, &mut find, &mut editor, &context(match_mode));
            assert_eq!((selections(&editor), options(&find)), (lines_selected(&expected), [true, true, false]), "{action:?} {match_mode}");
        }
    }
}

#[test]
fn the_default_mode_matches_with_the_find_match_case_and_whole_word() {
    let mut actual = Vec::new();
    for (match_case, whole_word) in [(false, false), (false, true), (true, false), (true, true)] {
        let (mut find, mut editor) = setup(&OCCURRENCES);
        change(&mut find, &mut editor, NewFindState { match_case: Some(match_case), whole_word: Some(whole_word), ..Default::default() });
        select(&mut editor, &[[1, 1, 1, 4]]);
        run(Action::AddNext, &mut find, &mut editor, &context("findOptions"));
        actual.push(selections(&editor));
    }
    assert_eq!(actual, [vec![[1, 1, 1, 4], [2, 1, 2, 4]], vec![[1, 1, 1, 4], [4, 1, 4, 4]], vec![[1, 1, 1, 4], [3, 1, 3, 4]], vec![[1, 1, 1, 4], [5, 1, 5, 4]]]);
}

#[test]
fn mixed_case_selections_compare_with_the_selection_setting_and_find_focused_with_the_find() {
    for (text_focused, match_case, expected) in [(true, true, 3), (false, false, 3), (false, true, 2)] {
        let (mut find, mut editor) = setup(&["foo", "FOO", "fooBar"]);
        change(&mut find, &mut editor, NewFindState { search_string: Some("foo".into()), is_revealed: Some(!text_focused), match_case: Some(match_case), ..Default::default() });
        select(&mut editor, &[[1, 1, 1, 4], [2, 1, 2, 4]]);
        let context = MultiCursorContext { text_focused, match_mode: "caseInsensitive".into() };
        run(Action::AddNext, &mut find, &mut editor, &context);
        assert_eq!(selections(&editor).len(), expected, "text focus {text_focused}, match case {match_case}");
    }
}

#[test]
fn selections_of_different_words_expand_to_their_words_first() {
    let (mut find, mut editor) = setup(&["abc pizza", "abc house", "abc bar"]);
    select(&mut editor, &[[1, 1, 1, 1], [2, 5, 2, 5], [3, 1, 3, 4]]);
    run(Action::AddNext, &mut find, &mut editor, &context("findOptions"));
    assert_eq!(selections(&editor), [[1, 1, 1, 4], [2, 5, 2, 10], [3, 1, 3, 4]]);
}

const DISASSOCIATION: [&str; 6] = ["app", "apples", "whatsapp", "app", "App", " app"];

#[test]
fn a_caret_started_session_overrides_the_find_options_until_it_ends() {
    for match_mode in ["findOptions", "caseSensitive", "caseInsensitive"] {
        for ending in ["selection", "blur", "configuration"] {
            let (mut find, mut editor) = setup(&DISASSOCIATION);
            change(&mut find, &mut editor, NewFindState { match_case: Some(false), whole_word: Some(false), is_regex: Some(true), ..Default::default() });
            select(&mut editor, &[[1, 2, 1, 2]]);
            run(Action::AddNext, &mut find, &mut editor, &context(match_mode));
            run(Action::AddNext, &mut find, &mut editor, &context(match_mode));
            let during = (find.state.search_string.clone(), options(&find), find.state.actual_options(), selections(&editor));
            assert_eq!(during, ("app".to_string(), [true, true, false], (true, false, false), vec![[1, 1, 1, 4], [4, 1, 4, 4]]), "{match_mode} {ending}");
            match ending {
                "selection" => {
                    select(&mut editor, &[[2, 1, 2, 4]]);
                    end_session_if_stale(&mut find, &mut editor, VIEWPORT, match_mode);
                }
                "blur" => end_session(&mut find, &mut editor, VIEWPORT),
                _ => end_session_if_stale(&mut find, &mut editor, VIEWPORT, if match_mode == "findOptions" { "caseInsensitive" } else { "findOptions" }),
            }
            assert_eq!(options(&find), [false, false, true], "{match_mode} {ending}");
        }
    }
}

#[test]
fn changing_the_find_options_ends_a_caret_started_session_and_keeps_them() {
    let (mut find, mut editor) = setup(&DISASSOCIATION);
    change(&mut find, &mut editor, NewFindState { match_case: Some(false), whole_word: Some(false), is_regex: Some(true), ..Default::default() });
    select(&mut editor, &[[1, 2, 1, 2]]);
    run(Action::AddNext, &mut find, &mut editor, &context("findOptions"));
    change(&mut find, &mut editor, NewFindState { match_case: Some(false), whole_word: Some(true), ..Default::default() });
    run(Action::AddNext, &mut find, &mut editor, &context("findOptions"));
    run(Action::AddNext, &mut find, &mut editor, &context("findOptions"));
    assert_eq!((selections(&editor), options(&find), find.state.actual_options()), (vec![[1, 1, 1, 4], [4, 1, 4, 4], [5, 1, 5, 4]], [false, true, true], (true, true, false)));
}

#[test]
fn a_session_enters_and_leaves_when_the_selection_changes() {
    let (mut find, mut editor) = setup(&DISASSOCIATION);
    let context = context("findOptions");
    select(&mut editor, &[[1, 2, 1, 2]]);
    let mut steps = Vec::new();
    for _ in 0..3 {
        run(Action::AddNext, &mut find, &mut editor, &context);
        steps.push(selections(&editor));
    }
    assert_eq!(steps, [vec![[1, 1, 1, 4]], vec![[1, 1, 1, 4], [4, 1, 4, 4]], vec![[1, 1, 1, 4], [4, 1, 4, 4], [6, 2, 6, 5]]]);

    let (mut find, mut editor) = setup(&DISASSOCIATION);
    select(&mut editor, &[[1, 2, 1, 2]]);
    run(Action::AddNext, &mut find, &mut editor, &context);
    run(Action::AddNext, &mut find, &mut editor, &context);
    select(&mut editor, &[[1, 1, 1, 4]]);
    run(Action::AddNext, &mut find, &mut editor, &context);
    assert_eq!(selections(&editor), [[1, 1, 1, 4], [2, 1, 2, 4]]);
}

#[test]
fn select_highlights_respects_the_session_mode() {
    let (mut find, mut editor) = setup(&DISASSOCIATION);
    select(&mut editor, &[[1, 2, 1, 2]]);
    let expected = vec![[1, 1, 1, 4], [4, 1, 4, 4], [6, 2, 6, 5]];
    run(Action::SelectHighlights, &mut find, &mut editor, &context("findOptions"));
    assert_eq!(selections(&editor), expected);
    run(Action::SelectHighlights, &mut find, &mut editor, &context("findOptions"));
    assert_eq!(selections(&editor), expected);
}

/// issue #5400: Select All Occurrences takes the shown find widget's regex.
#[test]
fn select_highlights_uses_the_regex_of_the_shown_find_widget() {
    let (mut find, mut editor) = setup(&["something", "someething", "someeething", "nothing"]);
    select(&mut editor, &[[1, 1, 1, 1]]);
    change(&mut find, &mut editor, NewFindState { search_string: Some("some+thing".into()), is_regex: Some(true), is_revealed: Some(true), ..Default::default() });
    run(Action::SelectHighlights, &mut find, &mut editor, &context("findOptions"));
    assert_eq!(selections(&editor), [[1, 1, 1, 10], [2, 1, 2, 11], [3, 1, 3, 12]]);
}

/// issue #6661: touching ranges.
#[test]
fn add_next_works_with_touching_ranges() {
    let (mut find, mut editor) = setup(&["abcabc", "abc", "abcabc"]);
    select(&mut editor, &[[1, 1, 1, 4]]);
    for _ in 0..4 {
        run(Action::AddNext, &mut find, &mut editor, &context("findOptions"));
    }
    assert_eq!(selections(&editor), [[1, 1, 1, 4], [1, 4, 1, 7], [2, 1, 2, 4], [3, 1, 3, 4], [3, 4, 3, 7]]);
}

#[test]
fn add_next_works_with_multiline_selections() {
    let (mut find, mut editor) = setup(&["", "qwe", "rty", "", "qwe", "", "rty", "qwe", "rty"]);
    select(&mut editor, &[[2, 1, 3, 4]]);
    run(Action::AddNext, &mut find, &mut editor, &context("findOptions"));
    assert_eq!(selections(&editor), [[2, 1, 3, 4], [8, 1, 9, 4]]);
}

fn highlights(find: &FindController, editor: &OpenEditor, context: &MultiCursorContext) -> Vec<[usize; 4]> {
    let options = SelectionHighlightOptions { enabled: true, multiline: false, max_length: 200 };
    let ranges = selection_highlight_query(find, editor, context, options).map_or_else(Vec::new, |query| selection_highlight_ranges(editor, &query));
    ranges.iter().map(|range| [range.start.line + 1, range.start.column + 1, range.end.line + 1, range.end.column + 1]).collect()
}

const HIGHLIGHT_TEXT: [&str; 4] = ["foo", "FOO", "fooBar", "FOOBar"];

fn assert_selection_highlighting(match_mode: &str, expected_lines: &[usize]) {
    let (mut find, mut editor) = setup(&HIGHLIGHT_TEXT);
    let new = NewFindState { search_string: Some("f.*".into()), is_revealed: Some(true), match_case: Some(match_mode != "caseSensitive"), whole_word: Some(true), is_regex: Some(true), ..Default::default() };
    change(&mut find, &mut editor, new);
    select(&mut editor, &[[1, 1, 1, 4]]);
    assert_eq!(highlights(&find, &editor, &context(match_mode)), lines_selected(expected_lines));
}

fn assert_duplicate_find_highlights(match_mode: &str, expected_different_case_lines: &[usize]) {
    let (mut find, mut editor) = setup(&HIGHLIGHT_TEXT);
    select(&mut editor, &[[1, 1, 1, 4]]);
    let match_case = match_mode == "caseSensitive";
    change(&mut find, &mut editor, NewFindState { search_string: Some("foo".into()), is_revealed: Some(true), match_case: Some(match_case), whole_word: Some(false), ..Default::default() });
    let same_options = highlights(&find, &editor, &context(match_mode));
    change(&mut find, &mut editor, NewFindState { match_case: Some(!match_case), ..Default::default() });
    assert_eq!((same_options, highlights(&find, &editor, &context(match_mode))), (vec![], lines_selected(expected_different_case_lines)));
}

#[test]
fn selection_highlighting_uses_match_case_false() {
    assert_selection_highlighting("caseInsensitive", &[2, 3, 4]);
}

#[test]
fn only_duplicate_find_highlights_with_the_same_rules_are_suppressed_match_case_false() {
    assert_duplicate_find_highlights("caseInsensitive", &[2, 3, 4]);
}

#[test]
fn selection_highlighting_uses_match_case_true() {
    assert_selection_highlighting("caseSensitive", &[3]);
}

#[test]
fn only_duplicate_find_highlights_with_the_same_rules_are_suppressed_match_case_true() {
    assert_duplicate_find_highlights("caseSensitive", &[3]);
}

#[test]
fn selection_highlighting_follows_the_match_mode_setting() {
    let (find, mut editor) = setup(&HIGHLIGHT_TEXT);
    select(&mut editor, &[[1, 1, 1, 4]]);
    let actual: Vec<_> = ["caseInsensitive", "caseSensitive", "caseInsensitive"].iter().map(|mode| highlights(&find, &editor, &context(mode))).collect();
    assert_eq!(actual, [lines_selected(&[2, 3, 4]), lines_selected(&[3]), lines_selected(&[2, 3, 4])]);
}

#[test]
fn find_regex_select_all_discards_an_existing_selection_driven_session() {
    let (mut find, mut editor) = setup(&["foo", "fooBar", "bar", "barista"]);
    let focused = context("caseInsensitive");
    let blurred = MultiCursorContext { text_focused: false, ..focused.clone() };
    select(&mut editor, &[[1, 1, 1, 4]]);
    run(Action::AddNext, &mut find, &mut editor, &focused);
    change(&mut find, &mut editor, NewFindState { search_string: Some("bar$".into()), is_revealed: Some(true), is_regex: Some(true), match_case: Some(true), ..Default::default() });
    end_session(&mut find, &mut editor, VIEWPORT);
    run(Action::SelectHighlights, &mut find, &mut editor, &blurred);
    change(&mut find, &mut editor, NewFindState { is_revealed: Some(false), ..Default::default() });
    let after_find = highlights(&find, &editor, &blurred);
    run(Action::AddNext, &mut find, &mut editor, &focused);
    assert_eq!((after_find, selections(&editor)), (vec![[2, 4, 2, 7], [4, 1, 4, 4]], vec![[3, 1, 3, 4], [4, 1, 4, 4]]));
}

#[test]
fn selection_highlights_skip_empty_blank_long_and_multiline_selections() {
    let (find, mut editor) = setup(&["foo  foo", "  foo", "foo"]);
    let mode = context("findOptions");
    for range in [[1, 1, 1, 1], [1, 4, 1, 6], [1, 1, 2, 1]] {
        select(&mut editor, &[range]);
        assert_eq!(highlights(&find, &editor, &mode), Vec::<[usize; 4]>::new(), "{range:?}");
    }
    select(&mut editor, &[[3, 1, 3, 4]]);
    let options = SelectionHighlightOptions { enabled: true, multiline: false, max_length: 2 };
    assert_eq!(selection_highlight_query(&find, &editor, &mode, options), None);
    assert_eq!(highlights(&find, &editor, &mode), [[1, 1, 1, 4], [1, 6, 1, 9], [2, 3, 2, 6]]);
}
