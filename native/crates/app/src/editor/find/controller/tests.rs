//! find.test.ts and findController.test.ts. Upstream's suite shares one storage, so a test starts with the
//! query options the tests before it stored.

use super::*;
use crate::editor::{EditReason, Selection};
use crate::editor::edit::tests::editor;

const VIEWPORT: Viewport = Viewport { width: 800., height: 600., char_width: 7.2, line_height: 18., sticky_scroll_lines: 0, top_whitespace: 0., stop_rendering_line_after: None };

fn position(line: usize, column: usize) -> Position {
    Position::new(line - 1, column - 1)
}

fn select(editor: &mut OpenEditor, [start_line, start_column, end_line, end_column]: [usize; 4]) {
    editor.set_selections(vec![Selection { anchor: position(start_line, start_column), head: position(end_line, end_column) }]);
}

fn from_selection(selection: &Selection) -> [usize; 4] {
    let (start, end) = (selection.start(), selection.end());
    [start.line + 1, start.column + 1, end.line + 1, end.column + 1]
}

fn selections(editor: &OpenEditor) -> Vec<[usize; 4]> {
    editor.selections.iter().map(from_selection).collect()
}

fn range(start_line: usize, start_column: usize, end_line: usize, end_column: usize) -> std::ops::Range<Position> {
    position(start_line, start_column)..position(end_line, end_column)
}

fn search(search_string: &str) -> NewFindState {
    NewFindState { search_string: Some(search_string.into()), ..Default::default() }
}

/// `TestFindController`: a start that moves focus to the find input sets `findInputFocussed`.
fn start_find(controller: &mut FindController, editor: &mut OpenEditor) {
    controller.find_input_focused = controller.start_find(editor, VIEWPORT) == FocusAction::FocusFindInput;
}

fn start_options(update_search_scope: bool) -> StartOptions {
    StartOptions { force_reveal_replace: false, seed_search_string_from_selection: Seed::None, seed_search_string_from_non_empty_selection: false, seed_search_string_from_global_clipboard: false, should_focus: FocusAction::NoFocusChange, update_search_scope, is_loop: true }
}

fn new_controller(query: QueryOptions) -> FindController {
    FindController::new(FindOptions::default(), query)
}

fn regex() -> QueryOptions {
    QueryOptions { is_regex: true, ..Default::default() }
}

#[test]
fn search_string_at_position() {
    let mut editor = editor("ABC DEF\n0123 456", "\n");
    assert_eq!(selection_search_string(&editor, Seed::Single, false).as_deref(), Some("ABC"));
    select(&mut editor, [1, 3, 1, 3]);
    assert_eq!(selection_search_string(&editor, Seed::Single, false).as_deref(), Some("ABC"));
    select(&mut editor, [1, 5, 1, 5]);
    assert_eq!(selection_search_string(&editor, Seed::Single, false).as_deref(), Some("DEF"));
}

#[test]
fn search_string_with_selection() {
    let mut editor = editor("ABC DEF\n0123 456", "\n");
    for (selection, expected) in [([1, 1, 1, 2], "A"), ([1, 2, 1, 4], "BC"), ([1, 2, 1, 7], "BC DE")] {
        select(&mut editor, selection);
        assert_eq!(selection_search_string(&editor, Seed::Single, false).as_deref(), Some(expected));
    }
}

#[test]
fn search_string_with_multiline_selection() {
    let mut editor = editor("ABC DEF\n0123 456", "\n");
    for selection in [[1, 1, 2, 1], [1, 1, 2, 4], [1, 7, 2, 4]] {
        select(&mut editor, selection);
        assert_eq!(selection_search_string(&editor, Seed::Single, false), None);
    }
}

fn global_find_clipboard_controller() -> FindController {
    FindController::new(FindOptions { global_find_clipboard: true, ..Default::default() }, QueryOptions::default())
}

/// reads from the global clipboard buffer on next find action if buffer exists
#[test]
fn reads_from_the_global_clipboard_buffer_on_next_find_action() {
    let mut editor = editor("ABC\nABC\nXYZ\nABC", "\n");
    crate::clipboard::write_find_text("ABC");
    let mut controller = global_find_clipboard_controller();
    controller.match_find_action(&mut editor, VIEWPORT, true);
    assert_eq!(controller.state.search_string, "ABC");
    assert_eq!(selections(&editor), [[1, 1, 1, 4]]);
}

/// writes to the global clipboard buffer when text changes
#[test]
fn writes_to_the_global_clipboard_buffer_when_text_changes() {
    let mut editor = editor("ABC\nABC\nXYZ\nABC", "\n");
    crate::clipboard::write_find_text("");
    let mut controller = global_find_clipboard_controller();
    controller.change(&mut editor, VIEWPORT, search("ABC"), true);
    assert_eq!(crate::clipboard::read_find_text(), "ABC");
    let mut controller = new_controller(QueryOptions::default());
    controller.change(&mut editor, VIEWPORT, search("XYZ"), true);
    assert_eq!(crate::clipboard::read_find_text(), "ABC", "globalFindClipboard is off by default");
}

/// issue #151311: findWithArgs respects searchString, and the clipboard one
#[test]
fn issue_151311_find_with_args_respects_search_string() {
    for global_find_clipboard in [false, true] {
        for search_string in [None, Some(""), Some("explicit")] {
            let mut editor = editor("needle", "\n");
            let options = FindOptions { global_find_clipboard, seed_search_string_from_selection: if global_find_clipboard { SeedFromSelection::Never } else { SeedFromSelection::Always }, ..Default::default() };
            let mut controller = FindController::new(options, QueryOptions::default());
            select(&mut editor, [1, 1, 1, 7]);
            for _ in 0..3 {
                crate::clipboard::write_find_text("clipboard");
                let new = NewFindState { search_string: search_string.map(str::to_string), is_regex: Some(true), ..Default::default() };
                controller.start_find_with_args(&mut editor, VIEWPORT, new, false);
                let seed = if global_find_clipboard { "clipboard" } else { "needle" };
                assert_eq!(controller.state.search_string, search_string.unwrap_or(seed));
            }
        }
    }
}

/// issue #1857: F3, Find Next, acts like "Find Under Cursor"
#[test]
fn issue_1857_f3_find_next_acts_like_find_under_cursor() {
    let mut editor = editor("ABC\nABC\nXYZ\nABC", "\n");
    let mut controller = new_controller(QueryOptions::default());
    start_find(&mut controller, &mut editor);
    for typed in ["A", "AB", "ABC"] {
        controller.change(&mut editor, VIEWPORT, search(typed), true);
    }
    assert_eq!(selections(&editor), [[1, 1, 1, 4]]);

    controller.close_find_widget(&mut editor, VIEWPORT);
    controller.find_input_focused = false;
    assert_eq!(selections(&editor), [[1, 1, 1, 4]]);

    editor.push_undo_stop();
    editor.execute_edits(vec![(position(1, 1), position(1, 4), String::new(), false)], None, EditReason::Unknown("test"));
    editor.execute_edits(vec![(position(1, 1), position(1, 1), "XYZ".into(), false)], None, EditReason::Unknown("test"));
    editor.push_undo_stop();
    assert_eq!(editor.document.lines[0], "XYZ");
    assert_eq!(selections(&editor), [[1, 4, 1, 4]]);

    controller.match_find_action(&mut editor, VIEWPORT, true);
    assert_eq!(controller.state.search_string, "ABC");
    assert!(!controller.find_input_focused);
}

/// issue #3090: F3 does not loop with two matches on a single line
#[test]
fn issue_3090_f3_does_not_loop_with_two_matches_on_a_single_line() {
    let mut editor = editor("import nls = require('vs/nls');", "\n");
    let mut controller = new_controller(QueryOptions::default());
    select(&mut editor, [1, 9, 1, 9]);
    controller.match_find_action(&mut editor, VIEWPORT, true);
    assert_eq!(selections(&editor), [[1, 26, 1, 29]]);
    controller.match_find_action(&mut editor, VIEWPORT, true);
    assert_eq!(selections(&editor), [[1, 8, 1, 11]]);
}

/// issue #6149: Auto-escape highlighted text for search and replace regex mode
#[test]
fn issue_6149_auto_escape_highlighted_text_for_search_and_replace_regex_mode() {
    let mut editor = editor("var x = (3 * 5)\nvar y = (3 * 5)\nvar z = (3  * 5)", "\n");
    let mut controller = new_controller(QueryOptions::default());
    select(&mut editor, [1, 9, 1, 13]);
    controller.toggle_regex(&mut editor, VIEWPORT);
    start_find(&mut controller, &mut editor);
    controller.match_find_action(&mut editor, VIEWPORT, true);
    assert_eq!(selections(&editor), [[2, 9, 2, 13]]);
    controller.match_find_action(&mut editor, VIEWPORT, true);
    assert_eq!(selections(&editor), [[1, 9, 1, 13]]);
}

/// issue #41027: Don't replace find input value on replace action if find input is active
#[test]
fn issue_41027_dont_replace_find_input_value_on_replace_action_if_find_input_is_active() {
    let mut editor = editor("test", "\n");
    let mut controller = new_controller(regex());
    controller.toggle_regex(&mut editor, VIEWPORT);
    controller.set_search_string(&mut editor, VIEWPORT, "tes.");
    controller.start(&mut editor, VIEWPORT, StartOptions { should_focus: FocusAction::FocusFindInput, ..start_options(false) }, NewFindState::default());
    controller.find_input_focused = true;
    controller.match_find_action(&mut editor, VIEWPORT, true);
    controller.start_find_replace(&mut editor, VIEWPORT);
    assert_eq!(controller.state.search_string, "tes.");
}

fn close_on_result(close_on_result: bool, text: &str, search_string: &str) -> FindController {
    let mut editor = editor(text, "\n");
    let mut controller = FindController::new(FindOptions { close_on_result, ..Default::default() }, QueryOptions::default());
    start_find(&mut controller, &mut editor);
    assert!(controller.state.is_revealed);
    controller.change(&mut editor, VIEWPORT, search(search_string), true);
    controller.match_find_action(&mut editor, VIEWPORT, true);
    controller
}

/// editor.find.closeOnResult: closes find widget when a match is found from explicit navigation
#[test]
fn close_on_result_closes_find_widget_when_a_match_is_found_from_explicit_navigation() {
    assert!(!close_on_result(true, "ABC\nABC\nXYZ", "ABC").state.is_revealed);
}

/// editor.find.closeOnResult: keeps find widget open when no match is found
#[test]
fn close_on_result_keeps_find_widget_open_when_no_match_is_found() {
    let controller = close_on_result(true, "ABC\nDEF\nXYZ", "NO_MATCH");
    assert_eq!(controller.state.matches_count, 0);
    assert!(controller.state.is_revealed);
}

/// editor.find.closeOnResult: disabled keeps find widget open after navigation
#[test]
fn close_on_result_disabled_keeps_find_widget_open_after_navigation() {
    assert!(close_on_result(false, "ABC\nABC\nXYZ", "ABC").state.is_revealed);
}

/// issue #9043: Clear search scope when find widget is hidden
#[test]
fn issue_9043_clear_search_scope_when_find_widget_is_hidden() {
    let mut editor = editor("var x = (3 * 5)\nvar y = (3 * 5)\nvar z = (3 * 5)", "\n");
    let mut controller = new_controller(QueryOptions::default());
    controller.start(&mut editor, VIEWPORT, start_options(false), NewFindState::default());
    assert_eq!(controller.state.search_scope, None);
    controller.change(&mut editor, VIEWPORT, NewFindState { search_scope: Some(Some(vec![range(1, 1, 1, 5)])), ..Default::default() }, false);
    assert_eq!(controller.state.search_scope, Some(vec![range(1, 1, 1, 5)]));
    controller.close_find_widget(&mut editor, VIEWPORT);
    assert_eq!(controller.state.search_scope, None);
}

fn replace_after_find(text: &str, search_string: &str, replace_string: &str) -> (OpenEditor, FindController) {
    let mut editor = editor(text, "\n");
    let mut controller = new_controller(regex());
    start_find(&mut controller, &mut editor);
    let new = NewFindState { search_string: Some(search_string.into()), replace_string: Some(replace_string.into()), is_regex: Some(true), ..Default::default() };
    controller.change(&mut editor, VIEWPORT, new, false);
    controller.move_to_match(&mut editor, VIEWPORT, true);
    (editor, controller)
}

/// issue #18111: Regex replace with single space replaces with no space
#[test]
fn issue_18111_regex_replace_with_single_space_replaces_with_no_space() {
    let (mut editor, mut controller) = replace_after_find("HRESULT OnAmbientPropertyChange(DISPID   dispid);", r"\b\s{3}\b", " ");
    assert_eq!(selections(&editor), [[1, 39, 1, 42]]);
    controller.replace(&mut editor, VIEWPORT);
    assert_eq!(editor.document.lines.join("\n"), "HRESULT OnAmbientPropertyChange(DISPID dispid);");
}

/// issue #24714: Regular expression with ^ in search & replace
#[test]
fn issue_24714_regular_expression_with_caret_in_search_and_replace() {
    let (mut editor, mut controller) = replace_after_find("\nline2\nline3", "^", "x");
    assert_eq!(selections(&editor), [[2, 1, 2, 1]]);
    controller.replace(&mut editor, VIEWPORT);
    assert_eq!(editor.document.lines.join("\n"), "\nxline2\nline3");
}

fn find_next_selection(open_widget: bool) {
    let mut editor = editor("([funny]\n\n([funny]", "\n");
    let mut controller = new_controller(regex());
    if open_widget {
        start_find(&mut controller, &mut editor);
    }
    controller.change(&mut editor, VIEWPORT, NewFindState { is_regex: Some(true), ..Default::default() }, false);
    select(&mut editor, [1, 1, 1, 9]);
    controller.selection_match_find_action(&mut editor, VIEWPORT, true);
    assert_eq!(selections(&editor), [[3, 1, 3, 9]]);
}

/// issue #38232: Find Next Selection, regex enabled
#[test]
fn issue_38232_find_next_selection_regex_enabled() {
    find_next_selection(false);
}

/// issue #38232: Find Next Selection, regex enabled, find widget open
#[test]
fn issue_38232_find_next_selection_regex_enabled_find_widget_open() {
    find_next_selection(true);
}

/// issue #47400, CMD+E supports feeding multiple line of text into the find widget
#[test]
fn issue_47400_cmd_e_supports_feeding_multiple_line_of_text_into_the_find_widget() {
    let mut editor = editor("ABC\nABC\nXYZ\nABC\nABC", "\n");
    let mut controller = new_controller(regex());
    select(&mut editor, [1, 1, 1, 1]);
    start_find(&mut controller, &mut editor);
    select(&mut editor, [1, 1, 2, 4]);
    controller.start_find_with_selection(&mut editor, VIEWPORT);
    assert_eq!(controller.state.search_string.split('\n').collect::<Vec<_>>(), ["ABC", "ABC"]);
    select(&mut editor, [3, 1, 3, 1]);
    controller.start_find_with_selection(&mut editor, VIEWPORT);
}

/// issue #109756, CMD+E with empty cursor should always work
#[test]
fn issue_109756_cmd_e_with_empty_cursor_should_always_work() {
    let mut editor = editor("ABC\nABC\nXYZ\nABC\nABC", "\n");
    let mut controller = new_controller(regex());
    select(&mut editor, [1, 2, 1, 2]);
    controller.start_find_with_selection(&mut editor, VIEWPORT);
    assert_eq!(controller.state.search_string, "ABC");
}

/// FindController query options persistence: matchCase
#[test]
fn query_options_persistence_match_case() {
    let mut editor = editor("abc\nABC\nXYZ\nABC", "\n");
    let mut controller = new_controller(QueryOptions { match_case: true, ..Default::default() });
    start_find(&mut controller, &mut editor);
    controller.change(&mut editor, VIEWPORT, search("ABC"), true);
    assert_eq!(selections(&editor), [[2, 1, 2, 4]]);
}

/// FindController query options persistence: wholeWord
#[test]
fn query_options_persistence_whole_word() {
    let mut editor = editor("ABC\nAB\nXYZ\nABC", "\n");
    let mut controller = new_controller(QueryOptions { whole_word: true, ..Default::default() });
    start_find(&mut controller, &mut editor);
    controller.change(&mut editor, VIEWPORT, search("AB"), true);
    assert_eq!(selections(&editor), [[2, 1, 2, 3]]);
}

/// FindController query options persistence: toggling options is saved
#[test]
fn query_options_persistence_toggling_options_is_saved() {
    let mut editor = editor("ABC\nAB\nXYZ\nABC", "\n");
    let mut controller = new_controller(QueryOptions { whole_word: true, ..Default::default() });
    controller.toggle_regex(&mut editor, VIEWPORT);
    assert!(controller.query_options().is_regex);
}

fn find_in_selection(auto_find_in_selection: AutoFindInSelection, selection: [usize; 4]) -> Option<Vec<std::ops::Range<Position>>> {
    let mut editor = editor("var x = (3 * 5)\nvar y = (3 * 5)\nvar z = (3 * 5)", "\n");
    select(&mut editor, selection);
    let mut controller = FindController::new(FindOptions { auto_find_in_selection, ..Default::default() }, QueryOptions::default());
    controller.start(&mut editor, VIEWPORT, start_options(true), NewFindState::default());
    controller.state.search_scope
}

/// issue #27083: Update search scope once find widget becomes visible
#[test]
fn issue_27083_update_search_scope_once_find_widget_becomes_visible() {
    let mut editor = editor("var x = (3 * 5)\nvar y = (3 * 5)\nvar z = (3 * 5)", "\n");
    let mut controller = FindController::new(FindOptions { auto_find_in_selection: AutoFindInSelection::Always, ..Default::default() }, QueryOptions::default());
    select(&mut editor, [1, 1, 2, 1]);
    controller.start(&mut editor, VIEWPORT, start_options(true), NewFindState::default());
    assert_eq!(controller.state.search_scope, Some(vec![range(1, 1, 2, 1)]));

    controller.close_find_widget(&mut editor, VIEWPORT);
    editor.set_selections(vec![Selection { anchor: position(1, 1), head: position(2, 1) }, Selection { anchor: position(2, 1), head: position(2, 5) }]);
    controller.start(&mut editor, VIEWPORT, start_options(true), NewFindState::default());
    assert_eq!(controller.state.search_scope, Some(vec![range(1, 1, 2, 1), range(2, 1, 2, 5)]));
}

/// issue #58604: Do not update searchScope if it is empty
#[test]
fn issue_58604_do_not_update_search_scope_if_it_is_empty() {
    assert_eq!(find_in_selection(AutoFindInSelection::Always, [1, 2, 1, 2]), None);
}

/// issue #58604: Update searchScope if it is not empty
#[test]
fn issue_58604_update_search_scope_if_it_is_not_empty() {
    assert_eq!(find_in_selection(AutoFindInSelection::Always, [1, 2, 1, 3]), Some(vec![range(1, 2, 1, 3)]));
}

/// issue #27083: Find in selection when multiple lines are selected
#[test]
fn issue_27083_find_in_selection_when_multiple_lines_are_selected() {
    assert_eq!(find_in_selection(AutoFindInSelection::Multiline, [1, 6, 2, 1]), Some(vec![range(1, 6, 2, 1)]));
}
