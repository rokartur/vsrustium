//! copyLinesCommand.test.ts, moveLinesCommand.test.ts, sortLinesCommand.test.ts and linesOperations.test.ts.
//! Selections are upstream's 1-based (anchor line, anchor column, head line, head column).

use super::*;
use crate::editor::edit::tests::editor;

type Sel = (usize, usize, usize, usize);

fn selection((anchor_line, anchor_column, head_line, head_column): Sel) -> Selection {
    Selection { anchor: Position::new(anchor_line - 1, anchor_column - 1), head: Position::new(head_line - 1, head_column - 1) }
}

fn with_selections(lines: &[&str], given: &[Sel]) -> OpenEditor {
    let mut model = editor(&lines.join("\n"), "\n");
    if !given.is_empty() {
        model.set_selections(given.iter().map(|&given| selection(given)).collect());
    }
    model
}

fn set(model: &mut OpenEditor, given: &[Sel]) {
    model.set_selections(given.iter().map(|&given| selection(given)).collect());
}

fn selections(model: &OpenEditor) -> Vec<Sel> {
    model.selections.iter().map(|s| (s.anchor.line + 1, s.anchor.column + 1, s.head.line + 1, s.head.column + 1)).collect()
}

/// `assertSelection` and `toString` compare ranges: start and end, not the direction.
fn ranges(model: &OpenEditor) -> Vec<Sel> {
    model.selections.iter().map(|s| (s.start().line + 1, s.start().column + 1, s.end().line + 1, s.end().column + 1)).collect()
}

fn text_of(model: &OpenEditor) -> String {
    model.document.lines.join("\n")
}

const FIVE: [&str; 5] = ["first", "second line", "third line", "fourth line", "fifth"];

/// `testCommand` with one action that runs one command for one cursor.
fn run(lines: &[&str], given: Sel, action: impl FnOnce(&mut OpenEditor), expected: &[&str], expected_selection: Sel) {
    let mut model = with_selections(lines, &[given]);
    action(&mut model);
    assert_eq!(model.document.lines, expected);
    assert_eq!(selections(&model), [expected_selection]);
}

fn copy_down(given: Sel, expected: &[&str], expected_selection: Sel) {
    run(&FIVE, given, |model| _ = model.copy_lines(true), expected, expected_selection);
}

fn copy_up(given: Sel, expected: &[&str], expected_selection: Sel) {
    run(&FIVE, given, |model| _ = model.copy_lines(false), expected, expected_selection);
}

#[test]
fn copy_lines_command_cases() {
    copy_down((1, 3, 1, 1), &["first", "first", "second line", "third line", "fourth line", "fifth"], (2, 3, 2, 1));
    copy_up((1, 3, 1, 1), &["first", "first", "second line", "third line", "fourth line", "fifth"], (1, 3, 1, 1));
    copy_down((5, 3, 5, 1), &["first", "second line", "third line", "fourth line", "fifth", "fifth"], (6, 3, 6, 1));
    copy_up((5, 3, 5, 1), &["first", "second line", "third line", "fourth line", "fifth", "fifth"], (5, 3, 5, 1));
    // issue #1322
    copy_up((3, 11, 3, 11), &["first", "second line", "third line", "third line", "fourth line", "fifth"], (3, 11, 3, 11));
    copy_up((5, 6, 5, 6), &["first", "second line", "third line", "fourth line", "fifth", "fifth"], (5, 6, 5, 6));
    copy_up((4, 3, 2, 1), &["first", "second line", "third line", "fourth line", "second line", "third line", "fourth line", "fifth"], (4, 3, 2, 1));
    // ignore empty selection
    copy_up((2, 1, 1, 1), &["first", "first", "second line", "third line", "fourth line", "fifth"], (2, 1, 1, 1));
}

#[test]
fn copy_lines_of_two_cursors_on_one_line_copies_it_once() {
    let mut model = with_selections(&FIVE, &[(2, 1, 2, 1), (2, 4, 2, 4)]);
    model.copy_lines(true);
    assert_eq!(model.document.lines, ["first", "second line", "second line", "third line", "fourth line", "fifth"]);
    assert_eq!(selections(&model), [(3, 1, 3, 1), (3, 4, 3, 4)]);
}

#[test]
fn duplicate_selection() {
    let mut model = with_selections(&FIVE, &[(2, 2, 2, 2), (3, 2, 3, 2)]);
    model.duplicate_selection();
    assert_eq!(model.document.lines, ["first", "second line", "second line", "third line", "third line", "fourth line", "fifth"]);
    assert_eq!(ranges(&model), [(3, 2, 3, 2), (5, 2, 5, 2)]);

    let mut model = with_selections(&FIVE, &[(2, 1, 2, 4), (3, 1, 3, 4)]);
    model.duplicate_selection();
    assert_eq!(model.document.lines, ["first", "secsecond line", "thithird line", "fourth line", "fifth"]);
    assert_eq!(ranges(&model), [(2, 4, 2, 7), (3, 4, 3, 7)]);
}

fn move_down(lines: &[&str], given: Sel, expected: &[&str], expected_selection: Sel) {
    run(lines, given, |model| _ = model.move_lines(true), expected, expected_selection);
}

fn move_up(lines: &[&str], given: Sel, expected: &[&str], expected_selection: Sel) {
    run(lines, given, |model| _ = model.move_lines(false), expected, expected_selection);
}

#[test]
fn move_lines_command_cases() {
    // move first up / last down disabled
    move_up(&FIVE, (1, 1, 1, 1), &FIVE, (1, 1, 1, 1));
    move_down(&FIVE, (5, 1, 5, 1), &FIVE, (5, 1, 5, 1));
    move_down(&FIVE, (1, 4, 1, 1), &["second line", "first", "third line", "fourth line", "fifth"], (2, 4, 2, 1));
    move_up(&FIVE, (2, 1, 2, 1), &["second line", "first", "third line", "fourth line", "fifth"], (1, 1, 1, 1));
    // issue #1322a, #1322b, #1322c
    move_up(&FIVE, (2, 12, 2, 12), &["second line", "first", "third line", "fourth line", "fifth"], (1, 12, 1, 12));
    move_up(&FIVE, (5, 6, 5, 6), &["first", "second line", "third line", "fifth", "fourth line"], (4, 6, 4, 6));
    move_up(&FIVE, (5, 6, 5, 1), &["first", "second line", "third line", "fifth", "fourth line"], (4, 6, 4, 1));
    move_up(&FIVE, (5, 1, 5, 1), &["first", "second line", "third line", "fifth", "fourth line"], (4, 1, 4, 1));
    move_down(&FIVE, (4, 1, 4, 1), &["first", "second line", "third line", "fifth", "fourth line"], (5, 1, 5, 1));
    move_down(&FIVE, (4, 4, 2, 2), &["first", "fifth", "second line", "third line", "fourth line"], (5, 4, 3, 2));
    // invisible selection is ignored
    move_down(&FIVE, (2, 1, 1, 1), &["second line", "first", "third line", "fourth line", "fifth"], (3, 1, 2, 1));
}

#[test]
fn move_line_should_still_work_as_before_if_there_is_no_indentation_rules() {
    let lines = ["if (true) {", "    var task = new Task(() => {", "        var work = 1234;", "    });", "}"];
    move_up(&lines, (3, 1, 3, 1), &["if (true) {", "        var work = 1234;", "    var task = new Task(() => {", "    });", "}"], (2, 1, 2, 1));
}

#[test]
fn move_an_empty_line_over_an_empty_line_moves_the_cursor_only() {
    move_down(&["a", "", "", "b"], (2, 1, 2, 1), &["a", "", "", "b"], (3, 1, 3, 1));
    move_up(&["", "x", ""], (3, 1, 3, 1), &["", "", "x"], (2, 1, 2, 1));
}

/// `testCommand` with a `SortLinesCommand`, which unlike the action never widens one line to the document.
fn sort(given: Sel, descending: bool, expected: &[&str], expected_selection: Sel) {
    let mut model = with_selections(&FIVE, &[given]);
    let made = sort_lines_command(&model.document.lines, model.selections[0], descending);
    model.execute_commands(vec![made], command("tests"));
    assert_eq!(model.document.lines, expected);
    assert_eq!(selections(&model), [expected_selection]);
}

#[test]
fn sort_lines_command_cases() {
    // no op unless at least two lines selected
    sort((1, 3, 1, 1), false, &FIVE, (1, 3, 1, 1));
    sort((1, 3, 2, 1), false, &FIVE, (1, 3, 2, 1));
    sort((3, 3, 4, 2), false, &["first", "second line", "fourth line", "third line", "fifth"], (3, 3, 4, 1));
    sort((1, 1, 5, 1), false, &["first", "fourth line", "second line", "third line", "fifth"], (1, 1, 5, 1));
    sort((1, 1, 5, 6), false, &["fifth", "first", "fourth line", "second line", "third line"], (1, 1, 5, 11));
    sort((1, 1, 5, 1), true, &["third line", "second line", "fourth line", "first", "fifth"], (1, 1, 5, 1));
    sort((1, 1, 5, 6), true, &["third line", "second line", "fourth line", "first", "fifth"], (1, 1, 5, 6));
}

#[test]
fn sort_lines_action() {
    let mut model = with_selections(&["omicron", "beta", "alpha"], &[(1, 1, 3, 5)]);
    model.sort_lines(false);
    assert_eq!(model.document.lines, ["alpha", "beta", "omicron"]);
    assert_eq!(ranges(&model), [(1, 1, 3, 7)]);

    let mut model = with_selections(&["omicron", "beta", "alpha"], &[]);
    model.sort_lines(false);
    assert_eq!(model.document.lines, ["alpha", "beta", "omicron"]);

    let mut model = with_selections(&["omicron", "beta", "alpha", "", "omicron", "beta", "alpha"], &[(1, 1, 3, 5), (5, 1, 7, 5)]);
    model.sort_lines(false);
    assert_eq!(model.document.lines, ["alpha", "beta", "omicron", "", "alpha", "beta", "omicron"]);
    assert_eq!(ranges(&model), [(1, 1, 3, 7), (5, 1, 7, 7)]);

    // applies to whole document when selection is single line
    let mut model = with_selections(&["omicron", "beta", "alpha"], &[(2, 1, 2, 4)]);
    model.sort_lines(false);
    assert_eq!(model.document.lines, ["alpha", "beta", "omicron"]);

    let mut model = with_selections(&["alpha", "beta", "omicron"], &[(1, 1, 3, 7)]);
    model.sort_lines(true);
    assert_eq!(model.document.lines, ["omicron", "beta", "alpha"]);
    assert_eq!(ranges(&model), [(1, 1, 3, 5)]);

    let mut model = with_selections(&["alpha", "beta", "omicron", "", "alpha", "beta", "omicron"], &[(1, 1, 3, 7), (5, 1, 7, 7)]);
    model.sort_lines(true);
    assert_eq!(model.document.lines, ["omicron", "beta", "alpha", "", "omicron", "beta", "alpha"]);
    assert_eq!(ranges(&model), [(1, 1, 3, 5), (5, 1, 7, 5)]);
}

#[test]
fn delete_duplicate_lines_action() {
    let lines = ["alpha", "beta", "beta", "beta", "alpha", "omicron"];
    let mut model = with_selections(&lines, &[(1, 3, 6, 4)]);
    model.delete_duplicate_lines();
    assert_eq!(model.document.lines, ["alpha", "beta", "omicron"]);
    assert_eq!(ranges(&model), [(1, 1, 3, 8)]);

    let mut model = with_selections(&lines, &[]);
    model.delete_duplicate_lines();
    assert_eq!(model.document.lines, ["alpha", "beta", "omicron"]);
    assert!(model.selection().is_empty());

    let mut model = with_selections(&["alpha", "beta", "beta", "omicron", "", "alpha", "alpha", "beta"], &[(1, 2, 4, 3), (6, 2, 8, 3)]);
    model.delete_duplicate_lines();
    assert_eq!(model.document.lines, ["alpha", "beta", "omicron", "", "alpha", "beta"]);
    assert_eq!(ranges(&model), [(1, 1, 3, 8), (5, 1, 6, 5)]);

    // applies to whole document when selection is single line
    let mut model = with_selections(&["alpha", "beta", "alpha", "omicron"], &[(2, 1, 2, 2)]);
    model.delete_duplicate_lines();
    assert_eq!(model.document.lines, ["alpha", "beta", "omicron"]);
}

#[test]
fn delete_all_left_action() {
    // should delete to the left of the cursor
    let mut model = with_selections(&["one", "two", "three"], &[(1, 2, 1, 2)]);
    model.delete_all_left();
    assert_eq!(model.document.lines[0], "ne");
    set(&mut model, &[(2, 2, 2, 2), (3, 2, 3, 2)]);
    model.delete_all_left();
    assert_eq!(model.document.lines[1..], ["wo", "hree"]);

    // should jump to the previous line when on first column
    let mut model = with_selections(&["one", "two", "three"], &[(2, 1, 2, 1)]);
    model.delete_all_left();
    assert_eq!(model.document.lines[0], "onetwo");
    set(&mut model, &[(1, 1, 1, 1), (2, 1, 2, 1)]);
    model.delete_all_left();
    assert_eq!(model.document.lines, ["onetwothree"]);
    set(&mut model, &[(1, 1, 1, 1)]);
    model.delete_all_left();
    assert_eq!(model.document.lines, ["onetwothree"]);
}

#[test]
fn delete_all_left_keeps_deleting_lines_in_multi_cursor_mode() {
    let lines = ["hi my name is Carlos Matos", "BCC", "waso waso waso", "my wife doesnt believe in me", "nonononono", "bitconneeeect"];
    let mut model = with_selections(&lines, &[(3, 5, 3, 5), (2, 4, 2, 4), (5, 11, 5, 11)]);
    model.delete_all_left();
    assert_eq!((model.document.lines[1].as_str(), model.document.lines[2].as_str(), model.document.lines[4].as_str()), ("", " waso waso", ""));
    assert_eq!(ranges(&model), [(3, 1, 3, 1), (2, 1, 2, 1), (5, 1, 5, 1)]);
    model.delete_all_left();
    assert_eq!(model.document.lines[0], "hi my name is Carlos Matos waso waso");
    assert_eq!(ranges(&model), [(1, 27, 1, 27), (2, 29, 2, 29)]);
}

#[test]
fn delete_all_left_works_in_multi_cursor_mode() {
    let mut model = with_selections(&["hello", "world", "hello world", "hello", "bonjour", "hola", "world", "hello world"], &[(1, 2, 1, 2), (1, 4, 1, 4)]);
    model.delete_all_left();
    assert_eq!(model.document.lines[0], "lo");
    set(&mut model, &[(2, 2, 2, 2), (2, 4, 2, 5)]);
    model.delete_all_left();
    assert_eq!(model.document.lines[1], "d");
    set(&mut model, &[(3, 2, 3, 5), (3, 7, 3, 7)]);
    model.delete_all_left();
    assert_eq!(model.document.lines[2], "world");
    set(&mut model, &[(4, 3, 4, 3), (4, 5, 5, 4)]);
    model.delete_all_left();
    assert_eq!(model.document.lines[3], "jour");
    set(&mut model, &[(5, 3, 6, 3), (6, 5, 7, 5), (7, 7, 7, 7)]);
    model.delete_all_left();
    assert_eq!(model.document.lines[4], "world");
}

#[test]
fn delete_all_left_pushes_an_undo_stop() {
    // issue #36234
    let mut model = with_selections(&["one", "two", "three"], &[(1, 1, 1, 1)]);
    model.type_text("Typing some text here on line ");
    assert_eq!((model.document.lines[0].as_str(), selections(&model)), ("Typing some text here on line one", vec![(1, 31, 1, 31)]));
    model.delete_all_left();
    assert_eq!((model.document.lines[0].as_str(), selections(&model)), ("one", vec![(1, 1, 1, 1)]));
    model.undo();
    assert_eq!((model.document.lines[0].as_str(), selections(&model)), ("Typing some text here on line one", vec![(1, 31, 1, 31)]));
}

const JOIN: [&str; 11] = ["hello", "world", "hello ", "world", "hello\t\t", "\tworld", "hello   ", "\tworld", "", "", "hello world"];

#[test]
fn join_lines_inserts_a_space_if_necessary() {
    let mut model = with_selections(&JOIN, &[(1, 2, 1, 2)]);
    let steps: [(Sel, &str, Sel); 5] = [
        ((1, 2, 1, 2), "hello world", (1, 6, 1, 6)),
        ((2, 2, 2, 2), "hello world", (2, 7, 2, 7)),
        ((3, 2, 3, 2), "hello world", (3, 7, 3, 7)),
        ((4, 2, 5, 3), "hello world", (4, 2, 4, 8)),
        ((5, 1, 7, 3), "hello world", (5, 1, 5, 3)),
    ];
    for (given, line, expected) in steps {
        set(&mut model, &[given]);
        model.join_lines();
        assert_eq!((model.document.lines[given.0 - 1].as_str(), ranges(&model)), (line, vec![expected]), "{given:?}");
    }
}

#[test]
fn join_lines_at_the_end_of_document() {
    // #50471
    let mut model = with_selections(&["hello", "world"], &[(2, 1, 2, 1)]);
    model.join_lines();
    assert_eq!(model.document.lines, ["hello", "world"]);
    assert_eq!(ranges(&model), [(2, 6, 2, 6)]);
}

#[test]
fn join_lines_works_in_multi_cursor_mode() {
    let mut model = with_selections(&JOIN, &[(5, 2, 5, 2), (1, 2, 1, 2), (3, 2, 4, 2), (5, 4, 6, 3), (7, 5, 8, 4), (10, 1, 10, 1)]);
    model.join_lines();
    assert_eq!(text_of(&model), "hello world\nhello world\nhello world\nhello world\n\nhello world");
    assert_eq!(ranges(&model), [(3, 4, 3, 8), (1, 6, 1, 6), (2, 2, 2, 8), (4, 5, 4, 9), (6, 1, 6, 1)]);
}

#[test]
fn join_lines_pushes_an_undo_stop() {
    let mut model = with_selections(&["hello", "world"], &[(1, 6, 1, 6)]);
    model.type_text(" my dear");
    assert_eq!((model.document.lines[0].as_str(), selections(&model)), ("hello my dear", vec![(1, 14, 1, 14)]));
    model.join_lines();
    assert_eq!((model.document.lines[0].as_str(), selections(&model)), ("hello my dear world", vec![(1, 14, 1, 14)]));
    model.undo();
    assert_eq!((model.document.lines[0].as_str(), selections(&model)), ("hello my dear", vec![(1, 14, 1, 14)]));
}

fn reversed(lines: &[&str], given: &[Sel]) -> OpenEditor {
    let mut model = with_selections(lines, given);
    model.reverse_lines();
    model
}

#[test]
fn reverse_lines_action() {
    assert_eq!(reversed(&["alice", "bob", "charlie"], &[]).document.lines, ["charlie", "bob", "alice"]);
    // excludes empty last line
    assert_eq!(reversed(&["alice", "bob", "charlie", ""], &[]).document.lines, ["charlie", "bob", "alice", ""]);
    // updates cursor
    assert_eq!(selections(&reversed(&["alice", "bob", "charlie"], &[(3, 3, 3, 3)])), [(1, 3, 1, 3)]);
    // preserves cursor on empty last line
    assert_eq!(selections(&reversed(&["alice", "bob", "charlie", ""], &[(4, 1, 4, 1)])), [(4, 1, 4, 1)]);
    // preserves selected text when selections do not span lines
    let model = reversed(&["alice", "bob", "charlie", ""], &[(1, 1, 1, 3), (2, 1, 2, 4), (3, 1, 3, 5)]);
    let selected: Vec<String> = model.selections.iter().map(|s| text_between(&model.document.lines, s.start(), s.end())).collect();
    assert_eq!(selected, ["al", "bob", "char"]);
    let five = ["line1", "line2", "line3", "line4", "line5"];
    assert_eq!(reversed(&five, &[(2, 1, 4, 6)]).document.lines, ["line1", "line4", "line3", "line2", "line5"]);
    assert_eq!(reversed(&five, &[(2, 3, 4, 3)]).document.lines, ["line1", "line4", "line3", "line2", "line5"]);
    let six = ["line1", "line2", "line3", "line4", "line5", "line6"];
    assert_eq!(reversed(&six, &[(1, 1, 2, 6), (4, 1, 5, 6)]).document.lines, ["line2", "line1", "line3", "line5", "line4", "line6"]);
    // updates selection positions after reversal
    assert_eq!(ranges(&reversed(&["line1", "line2", "line3", "line4"], &[(1, 2, 3, 3)])), [(1, 3, 3, 2)]);
    // applies to whole document when selection is single line
    assert_eq!(reversed(&["line1", "line2", "line3"], &[(2, 1, 2, 6)]).document.lines, ["line3", "line2", "line1"]);
    // excludes end line when selection ends at column 1
    assert_eq!(reversed(&five, &[(2, 1, 4, 1)]).document.lines, ["line1", "line3", "line2", "line4", "line5"]);
    assert_eq!(reversed(&["omicron", "beta", "alpha"], &[(2, 1, 2, 4)]).document.lines, ["alpha", "beta", "omicron"]);
}

#[test]
fn transpose() {
    let mut model = with_selections(&["hello world", "", "", "   "], &[]);
    let steps: [(Sel, usize, &str, Sel); 5] = [
        ((1, 1, 1, 1), 1, "hello world", (1, 2, 1, 2)),
        ((1, 6, 1, 6), 1, "hell oworld", (1, 7, 1, 7)),
        ((1, 12, 1, 12), 1, "hell oworl", (2, 2, 2, 2)),
        ((3, 1, 3, 1), 3, "", (4, 1, 4, 1)),
        ((4, 2, 4, 2), 4, "   ", (4, 3, 4, 3)),
    ];
    for (given, line, text, expected) in steps {
        set(&mut model, &[given]);
        model.transpose();
        assert_eq!((model.document.lines[line - 1].as_str(), ranges(&model)), (text, vec![expected]), "{given:?}");
    }

    // fix #16633
    let mut model = with_selections(&["", "", "hello", "world", "", "hello world", "", "hello world"], &[]);
    let steps: [(Sel, usize, &str, Sel); 4] = [
        ((1, 1, 1, 1), 2, "", (2, 1, 2, 1)),
        ((3, 6, 3, 6), 4, "oworld", (4, 2, 4, 2)),
        ((6, 12, 6, 12), 7, "d", (7, 2, 7, 2)),
        ((8, 12, 8, 12), 8, "hello world", (8, 12, 8, 12)),
    ];
    for (given, line, text, expected) in steps {
        set(&mut model, &[given]);
        model.transpose();
        assert_eq!((model.document.lines[line - 1].as_str(), ranges(&model)), (text, vec![expected]), "{given:?}");
    }
}

/// Runs the case action on one selection, then the line and the selection's range.
fn case_steps(lines: &[&str], steps: &[(Sel, CaseTransform, &str, Option<Sel>)]) {
    let mut model = with_selections(lines, &[]);
    for &(given, transform, line, expected) in steps {
        set(&mut model, &[given]);
        model.transform_case(transform);
        assert_eq!(model.document.lines[given.0 - 1], line, "{given:?} {transform:?}");
        if let Some(expected) = expected {
            assert_eq!(ranges(&model), [expected], "{given:?} {transform:?}");
        }
    }
}

#[test]
fn toggle_case() {
    use CaseTransform::*;
    let lines = [
        "hello world",
        "öçşğü",
        "parseHTMLString",
        "getElementById",
        "insertHTML",
        "PascalCase",
        "CSSSelectorsList",
        "iD",
        "tEST",
        "öçşÖÇŞğüĞÜ",
        "audioConverter.convertM4AToMP3();",
        "snake_case",
        "Capital_Snake_Case",
        "function helloWorld() {",
        "return someGlobalObject.printHelloWorld(\"en\", \"utf-8\");",
        "}",
        "helloWorld();",
        "'JavaScript'",
        "parseHTML4String",
        "_accessor: ServicesAccessor",
    ];
    case_steps(
        &lines,
        &[
            ((1, 1, 1, 12), Upper, "HELLO WORLD", Some((1, 1, 1, 12))),
            ((1, 1, 1, 12), Lower, "hello world", Some((1, 1, 1, 12))),
            ((1, 3, 1, 3), Upper, "HELLO world", Some((1, 3, 1, 3))),
            ((1, 4, 1, 4), Lower, "hello world", Some((1, 4, 1, 4))),
            ((1, 1, 1, 12), Title, "Hello World", Some((1, 1, 1, 12))),
            ((2, 1, 2, 6), Upper, "ÖÇŞĞÜ", Some((2, 1, 2, 6))),
            ((2, 1, 2, 6), Lower, "öçşğü", Some((2, 1, 2, 6))),
            ((2, 1, 2, 6), Title, "Öçşğü", Some((2, 1, 2, 6))),
            ((3, 1, 3, 16), Snake, "parse_html_string", Some((3, 1, 3, 18))),
            ((4, 1, 4, 15), Snake, "get_element_by_id", Some((4, 1, 4, 18))),
            ((5, 1, 5, 11), Snake, "insert_html", Some((5, 1, 5, 12))),
            ((6, 1, 6, 11), Snake, "pascal_case", Some((6, 1, 6, 12))),
            ((7, 1, 7, 17), Snake, "css_selectors_list", Some((7, 1, 7, 19))),
            ((8, 1, 8, 3), Snake, "i_d", Some((8, 1, 8, 4))),
            ((9, 1, 9, 5), Snake, "t_est", Some((9, 1, 9, 6))),
            ((10, 1, 10, 11), Snake, "öçş_öç_şğü_ğü", Some((10, 1, 10, 14))),
            ((11, 1, 11, 34), Snake, "audio_converter.convert_m4a_to_mp3();", Some((11, 1, 11, 38))),
            ((12, 1, 12, 11), Snake, "snake_case", Some((12, 1, 12, 11))),
            ((13, 1, 13, 19), Snake, "capital_snake_case", Some((13, 1, 13, 19))),
            ((14, 1, 17, 14), Snake, "function hello_world() {", Some((14, 1, 17, 15))),
            ((18, 1, 18, 13), Snake, "'java_script'", Some((18, 1, 18, 14))),
            ((19, 1, 19, 17), Snake, "parse_html4_string", Some((19, 1, 19, 19))),
            ((20, 1, 20, 28), Snake, "_accessor: services_accessor", Some((20, 1, 20, 29))),
        ],
    );
    let mut model = with_selections(&lines, &[(14, 1, 17, 14)]);
    model.transform_case(Snake);
    assert_eq!(model.document.lines[13..17], ["function hello_world() {", "return some_global_object.print_hello_world(\"en\", \"utf-8\");", "}", "hello_world();"]);

    case_steps(
        &["foO baR BaZ", "foO'baR'BaZ", "foO[baR]BaZ", "foO`baR~BaZ", "foO^baR%BaZ", "foO$baR!BaZ", "'physician's assistant'"],
        &[
            ((1, 1, 1, 12), Title, "Foo Bar Baz", None),
            ((2, 1, 2, 12), Title, "Foo'bar'baz", None),
            ((3, 1, 3, 12), Title, "Foo[Bar]Baz", None),
            ((4, 1, 4, 12), Title, "Foo`Bar~Baz", None),
            ((5, 1, 5, 12), Title, "Foo^Bar%Baz", None),
            ((6, 1, 6, 12), Title, "Foo$Bar!Baz", None),
            ((7, 1, 7, 23), Title, "'Physician's Assistant'", None),
        ],
    );

    let camel_lines = [
        "camel from words",
        "from_snake_case",
        "from-kebab-case",
        "alreadyCamel",
        "ReTain_some_CAPitalization",
        "my_var.test_function()",
        "öçş_öç_şğü_ğü",
        "XMLHttpRequest",
        "\tfunction hello_world() {",
        "\t\treturn some_global_object;",
        "\t}",
    ];
    case_steps(
        &camel_lines,
        &[
            ((1, 1, 1, 18), Camel, "camelFromWords", None),
            ((2, 1, 2, 15), Camel, "fromSnakeCase", None),
            ((3, 1, 3, 15), Camel, "fromKebabCase", None),
            ((4, 1, 4, 12), Camel, "alreadyCamel", None),
            ((5, 1, 5, 26), Camel, "reTainSomeCAPitalization", None),
            ((6, 1, 6, 23), Camel, "myVar.testFunction()", None),
            ((7, 1, 7, 14), Camel, "öçşÖçŞğüĞü", None),
            ((8, 1, 8, 14), Camel, "XMLHttpRequest", None),
        ],
    );
    let mut model = with_selections(&camel_lines, &[(9, 1, 11, 2)]);
    model.transform_case(Camel);
    assert_eq!(model.document.lines[8..11], ["\tfunction helloWorld() {", "\t\treturn someGlobalObject;", "\t}"]);

    case_steps(
        &["", "   "],
        &[
            ((1, 1, 1, 1), Upper, "", Some((1, 1, 1, 1))),
            ((1, 1, 1, 1), Lower, "", Some((1, 1, 1, 1))),
            ((2, 2, 2, 2), Upper, "   ", Some((2, 2, 2, 2))),
            ((2, 2, 2, 2), Lower, "   ", Some((2, 2, 2, 2))),
        ],
    );

    case_steps(
        &[
            "hello world",
            "öçşğü",
            "parseHTMLString",
            "getElementById",
            "PascalCase",
            "öçşÖÇŞğüĞÜ",
            "audioConverter.convertM4AToMP3();",
            "Capital_Snake_Case",
            "parseHTML4String",
            "_accessor: ServicesAccessor",
            "Kebab-Case",
        ],
        &[
            ((1, 1, 1, 12), Kebab, "hello world", Some((1, 1, 1, 12))),
            ((2, 1, 2, 6), Kebab, "öçşğü", Some((2, 1, 2, 6))),
            ((3, 1, 3, 16), Kebab, "parse-html-string", Some((3, 1, 3, 18))),
            ((4, 1, 4, 15), Kebab, "get-element-by-id", Some((4, 1, 4, 18))),
            ((5, 1, 5, 11), Kebab, "pascal-case", Some((5, 1, 5, 12))),
            ((6, 1, 6, 11), Kebab, "öçş-öç-şğü-ğü", Some((6, 1, 6, 14))),
            ((7, 1, 7, 34), Kebab, "audio-converter.convert-m4a-to-mp3();", Some((7, 1, 7, 38))),
            ((8, 1, 8, 19), Kebab, "capital-snake-case", Some((8, 1, 8, 19))),
            ((9, 1, 9, 17), Kebab, "parse-html4-string", Some((9, 1, 9, 19))),
            ((10, 1, 10, 28), Kebab, "_accessor: services-accessor", Some((10, 1, 10, 29))),
            ((11, 1, 11, 11), Kebab, "kebab-case", Some((11, 1, 11, 11))),
        ],
    );

    let pascal_lines = [
        "hello world",
        "öçşğü",
        "parseHTMLString",
        "getElementById",
        "PascalCase",
        "öçşÖÇŞğüĞÜ",
        "audioConverter.convertM4AToMP3();",
        "Capital_Snake_Case",
        "parseHTML4String",
        "Kebab-Case",
        "FOO_BAR",
        "FOO BAR A",
        "xML_HTTP-reQUEsT",
        "ÉCOLE",
        "ΩMEGA_CASE",
        "ДОМ_ТЕСТ",
    ];
    let pascal_steps = [
        ((1, 1, 1, 12), Pascal, "HelloWorld", Some((1, 1, 1, 11))),
        ((2, 1, 2, 6), Pascal, "Öçşğü", Some((2, 1, 2, 6))),
        ((3, 1, 3, 16), Pascal, "ParseHTMLString", Some((3, 1, 3, 16))),
        ((4, 1, 4, 15), Pascal, "GetElementById", Some((4, 1, 4, 15))),
        ((5, 1, 5, 11), Pascal, "PascalCase", Some((5, 1, 5, 11))),
        ((6, 1, 6, 11), Pascal, "ÖçşÖÇŞğüĞÜ", Some((6, 1, 6, 11))),
        ((7, 1, 7, 34), Pascal, "AudioConverter.ConvertM4AToMP3();", Some((7, 1, 7, 34))),
        ((8, 1, 8, 19), Pascal, "CapitalSnakeCase", Some((8, 1, 8, 17))),
        ((9, 1, 9, 17), Pascal, "ParseHTML4String", Some((9, 1, 9, 17))),
        ((10, 1, 10, 11), Pascal, "KebabCase", Some((10, 1, 10, 10))),
        ((11, 1, 11, 8), Pascal, "FooBar", Some((11, 1, 11, 7))),
        ((12, 1, 12, 10), Pascal, "FooBarA", Some((12, 1, 12, 8))),
        ((13, 1, 13, 17), Pascal, "XmlHttpReQUEsT", Some((13, 1, 13, 15))),
        ((14, 1, 14, 6), Pascal, "École", Some((14, 1, 14, 6))),
        ((15, 1, 15, 11), Pascal, "ΩmegaCase", Some((15, 1, 15, 10))),
        ((16, 1, 16, 9), Pascal, "ДомТест", Some((16, 1, 16, 8))),
    ];
    case_steps(&pascal_lines, &pascal_steps);
    let mut model = with_selections(&pascal_lines, &[]);
    for &(given, transform, ..) in &pascal_steps[..10] {
        set(&mut model, &[given]);
        model.transform_case(transform);
    }
    set(&mut model, &[(9, 1, 10, 11)]);
    model.transform_case(Pascal);
    assert_eq!(model.document.lines[8..10], ["ParseHTML4String", "KebabCase"]);
    assert_eq!(ranges(&model), [(9, 1, 10, 10)]);
}

#[test]
fn delete_all_right_action() {
    // should be noop on empty
    let mut model = with_selections(&[""], &[]);
    model.delete_all_right();
    assert_eq!((model.document.lines.clone(), ranges(&model)), (vec![String::new()], vec![(1, 1, 1, 1)]));
    set(&mut model, &[(1, 1, 1, 1), (1, 1, 1, 1), (1, 1, 1, 1)]);
    model.delete_all_right();
    assert_eq!((model.document.lines.clone(), ranges(&model)), (vec![String::new()], vec![(1, 1, 1, 1)]));

    // should delete selected range
    let mut model = with_selections(&["hello", "world"], &[(1, 2, 1, 5)]);
    model.delete_all_right();
    assert_eq!((model.document.lines.clone(), ranges(&model)), (vec!["ho".to_string(), "world".into()], vec![(1, 2, 1, 2)]));
    set(&mut model, &[(1, 1, 2, 4)]);
    model.delete_all_right();
    assert_eq!((model.document.lines.clone(), ranges(&model)), (vec!["ld".to_string()], vec![(1, 1, 1, 1)]));
    set(&mut model, &[(1, 1, 1, 3)]);
    model.delete_all_right();
    assert_eq!((model.document.lines.clone(), ranges(&model)), (vec![String::new()], vec![(1, 1, 1, 1)]));

    // should delete to the right of the cursor
    let mut model = with_selections(&["hello", "world"], &[(1, 3, 1, 3)]);
    model.delete_all_right();
    assert_eq!((model.document.lines.clone(), ranges(&model)), (vec!["he".to_string(), "world".into()], vec![(1, 3, 1, 3)]));
    set(&mut model, &[(2, 1, 2, 1)]);
    model.delete_all_right();
    assert_eq!((model.document.lines.clone(), ranges(&model)), (vec!["he".to_string(), String::new()], vec![(2, 1, 2, 1)]));

    // should join two lines, if at the end of the line
    let mut model = with_selections(&["hello", "world"], &[(1, 6, 1, 6)]);
    for expected in ["helloworld", "hello", "hello"] {
        set(&mut model, &[(1, 6, 1, 6)]);
        model.delete_all_right();
        assert_eq!((model.document.lines.clone(), ranges(&model)), (vec![expected.to_string()], vec![(1, 6, 1, 6)]));
    }
}

#[test]
fn delete_all_right_works_with_multiple_cursors_and_undo() {
    let mut model = with_selections(&["hello", "there", "world"], &[(1, 3, 1, 3), (1, 6, 1, 6), (3, 4, 3, 4)]);
    let steps: [(&[&str], &[Sel]); 5] = [
        (&["hethere", "wor"], &[(1, 3, 1, 3), (2, 4, 2, 4)]),
        (&["he", "wor"], &[(1, 3, 1, 3), (2, 4, 2, 4)]),
        (&["hewor"], &[(1, 3, 1, 3), (1, 6, 1, 6)]),
        (&["he"], &[(1, 3, 1, 3)]),
        (&["he"], &[(1, 3, 1, 3)]),
    ];
    for (lines, expected) in steps {
        model.delete_all_right();
        assert_eq!((model.document.lines.clone(), ranges(&model)), (lines.iter().map(|line| line.to_string()).collect(), expected.to_vec()));
    }

    let mut model = with_selections(&["hello", "there", "world"], &[(1, 3, 1, 3), (1, 6, 1, 6), (3, 4, 3, 4)]);
    model.delete_all_right();
    assert_eq!((model.document.lines.clone(), ranges(&model)), (vec!["hethere".to_string(), "wor".into()], vec![(1, 3, 1, 3), (2, 4, 2, 4)]));
    model.undo();
    assert_eq!(ranges(&model), [(1, 3, 1, 3), (1, 6, 1, 6), (3, 4, 3, 4)]);
    model.redo();
    assert_eq!(ranges(&model), [(1, 3, 1, 3), (2, 4, 2, 4)]);
}

#[test]
fn insert_line_before_and_after() {
    let text = ["First line", "Second line", "Third line"];
    let before: [(usize, Sel, [&str; 4]); 3] = [
        (1, (1, 1, 1, 1), ["", "First line", "Second line", "Third line"]),
        (2, (2, 1, 2, 1), ["First line", "", "Second line", "Third line"]),
        (3, (3, 1, 3, 1), ["First line", "Second line", "", "Third line"]),
    ];
    for (line, expected_selection, expected) in before {
        let mut model = with_selections(&text, &[(line, 3, line, 3)]);
        model.insert_line_before();
        assert_eq!((model.document.lines.clone(), ranges(&model)), (expected.map(String::from).to_vec(), vec![expected_selection]));
    }
    let after: [(usize, Sel, [&str; 4]); 3] = [
        (1, (2, 1, 2, 1), ["First line", "", "Second line", "Third line"]),
        (2, (3, 1, 3, 1), ["First line", "Second line", "", "Third line"]),
        (3, (4, 1, 4, 1), ["First line", "Second line", "Third line", ""]),
    ];
    for (line, expected_selection, expected) in after {
        let mut model = with_selections(&text, &[(line, 3, line, 3)]);
        model.insert_line_after();
        assert_eq!((model.document.lines.clone(), ranges(&model)), (expected.map(String::from).to_vec(), vec![expected_selection]));
    }
}

#[test]
fn indent_lines() {
    // Bug 18276: Indentation broken when selection is empty
    let mut model = with_selections(&["function baz() {"], &[(1, 2, 1, 2)]);
    model.document.indentation.insert_spaces = false;
    model.indent_lines();
    assert_eq!((model.document.lines[0].as_str(), ranges(&model)), ("\tfunction baz() {", vec![(1, 3, 1, 3)]));
    model.tab();
    assert_eq!(model.document.lines[0], "\tf\tunction baz() {");

    // issue #80736: the added tab is not selected
    let mut model = with_selections(&["Some text"], &[(1, 1, 1, 1)]);
    model.document.indentation.insert_spaces = false;
    model.indent_lines();
    assert_eq!((model.document.lines[0].as_str(), ranges(&model)), ("\tSome text", vec![(1, 2, 1, 2)]));

    // Indenting on empty line should move cursor
    let mut model = with_selections(&[""], &[(1, 1, 1, 1)]);
    model.indent_lines();
    assert_eq!((model.document.lines[0].as_str(), ranges(&model)), ("    ", vec![(1, 5, 1, 5)]));
}

fn delete_lines(lines: &[&str], given: &[Sel], expected: &[&str], expected_selections: &[Sel]) {
    let mut model = with_selections(lines, given);
    model.delete_lines();
    assert_eq!(text_of(&model), expected.join("\n"));
    assert_eq!(selections(&model), expected_selections);
}

#[test]
fn delete_lines_action() {
    // issue #62112
    let mut model = with_selections(&["a", "foo boo", "too", "c"], &[(2, 4, 2, 4), (2, 8, 2, 8), (3, 4, 3, 4)]);
    model.delete_lines();
    assert_eq!(text_of(&model), "a\nc");

    delete_lines(&FIVE, &[(2, 3, 2, 3)], &["first", "third line", "fourth line", "fifth"], &[(2, 3, 2, 3)]);
    delete_lines(&FIVE, &[(1, 5, 1, 5)], &["second line", "third line", "fourth line", "fifth"], &[(1, 5, 1, 5)]);
    delete_lines(&FIVE, &[(5, 2, 5, 2)], &["first", "second line", "third line", "fourth line"], &[(4, 2, 4, 2)]);
    delete_lines(&FIVE, &[(3, 3, 2, 2)], &["first", "fourth line", "fifth"], &[(2, 2, 2, 2)]);
    delete_lines(&FIVE, &[(1, 4, 1, 5)], &["second line", "third line", "fourth line", "fifth"], &[(1, 5, 1, 5)]);
    delete_lines(&FIVE, &[(5, 1, 5, 2)], &["first", "second line", "third line", "fourth line"], &[(4, 2, 4, 2)]);
    delete_lines(&FIVE, &[(4, 1, 2, 1)], &["first", "fourth line", "fifth"], &[(2, 1, 2, 1)]);
    delete_lines(&FIVE, &[(2, 1, 1, 5)], &["second line", "third line", "fourth line", "fifth"], &[(1, 5, 1, 5)]);
    delete_lines(&FIVE, &[(4, 1, 5, 2)], &["first", "second line", "third line"], &[(3, 2, 3, 2)]);
}

#[test]
fn delete_lines_multicursor() {
    let class = |method: &str, value: &str| {
        [format!("\t{method}() {{"), "\t\tif (true) {".into(), format!("\t\t\treturn \"{value}\";"), "\t\t}".into(), "\t}".into(), String::new()]
    };
    let mut lines = vec!["class P {".to_string(), String::new()];
    for (method, value) in [("getA", "a"), ("getB", "b"), ("getC", "c")] {
        lines.extend(class(method, value));
    }
    lines.pop();
    lines.push("}".into());
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let expected: Vec<&str> = lines.iter().enumerate().filter(|(index, _)| ![3, 9, 15].contains(index)).map(|(_, line)| *line).collect();
    delete_lines(&lines, &[(4, 1, 5, 1), (10, 1, 11, 1), (16, 1, 17, 1)], &expected, &[(4, 1, 4, 1), (9, 1, 9, 1), (14, 1, 14, 1)]);
}

#[test]
fn every_line_action_is_one_undo_step() {
    let mut model = with_selections(&FIVE, &[(2, 1, 2, 1)]);
    model.move_lines(true);
    model.copy_lines(true);
    assert_eq!(model.document.lines, ["first", "third line", "second line", "second line", "fourth line", "fifth"]);
    model.undo();
    assert_eq!(model.document.lines, ["first", "third line", "second line", "fourth line", "fifth"]);
    model.undo();
    assert_eq!(model.document.lines, FIVE);
}
