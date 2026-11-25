//! wordOperations.test.ts and wordPartOperations.test.ts, with wordTestUtils.ts. Positions are 0-based here.

use super::*;
use crate::editor::OpenEditor;
use crate::editor::auto_indent::Strategy;
use crate::editor::edit::tests::{editor, no_rules};

fn at(line: usize, column: usize) -> Position {
    Position::new(line, column)
}

fn selection(anchor: (usize, usize), head: (usize, usize)) -> Selection {
    Selection { anchor: at(anchor.0, anchor.1), head: at(head.0, head.1) }
}

/// `getValue`.
fn value(editor: &OpenEditor) -> String {
    editor.document().lines.join("\n")
}

const NO_AUTO_CLOSING: AutoClosing = AutoClosing { delete: "auto", brackets: "languageDefined", quotes: "languageDefined", pairs: &[], auto_closed: &[] };

fn run_move(editor: &mut OpenEditor, id: &str, locales: &[String]) {
    let (_, word_move) = MOVE_COMMANDS.into_iter().find(|(command, _)| *command == id).expect("a word move command");
    let classifier = WordClassifier::new(DEFAULT_WORD_SEPARATORS, locales);
    editor.move_word(&classifier, word_move.navigation, word_move.right, word_move.select);
}

fn run_delete_with(editor: &mut OpenEditor, id: &'static str, auto_closing: &AutoClosing) {
    let (_, word_delete) = DELETE_COMMANDS.into_iter().find(|(command, _)| *command == id).expect("a word delete command");
    editor.delete_word(&WordClassifier::new(DEFAULT_WORD_SEPARATORS, &[]), auto_closing, word_delete, id);
}

fn run_delete(editor: &mut OpenEditor, id: &'static str) {
    run_delete_with(editor, id, &NO_AUTO_CLOSING);
}

fn delete_inside_word(editor: &mut OpenEditor, only_word: bool) {
    editor.delete_inside_word(&WordClassifier::new(DEFAULT_WORD_SEPARATORS, &[]), only_word);
}

/// `deserializePipePositions`: the text without `|`.
fn without_pipes(expected: &str) -> String {
    expected.replace('|', "")
}

/// `serializePipePositions`.
fn with_pipes(text: &str, mut stops: Vec<Position>) -> String {
    stops.sort();
    let mut out = String::new();
    for (line, content) in text.split('\n').enumerate() {
        if line > 0 {
            out.push('\n');
        }
        for (column, c) in content.chars().chain(std::iter::once('\0')).enumerate() {
            while stops.first() == Some(&at(line, column)) {
                out.push('|');
                stops.remove(0);
            }
            if c != '\0' {
                out.push(c);
            }
        }
    }
    assert!(stops.is_empty(), "Unexpected left over positions!!!");
    out
}

/// `testRepeatedActionAndExtractPositions`; `from` past the end clamps to the end of the text.
fn repeated(text: &str, from: Position, action: impl Fn(&mut OpenEditor), record: impl Fn(&OpenEditor) -> Position, stop: impl Fn(&OpenEditor) -> bool) -> Vec<Position> {
    let mut editor = editor(text, "\n");
    let lines = editor.document().lines.clone();
    let line = from.line.min(lines.len() - 1);
    let start = at(line, from.column.min(lines[line].chars().count()));
    editor.set_selections(vec![Selection::caret(start)]);
    let mut stops = Vec::new();
    loop {
        action(&mut editor);
        stops.push(record(&editor));
        if stop(&editor) {
            break;
        }
        assert!(stops.len() <= 1000, "Endless loop detected involving position {:?}!", editor.selection().head);
    }
    stops
}

const FAR: Position = Position { line: 1000, column: 1000 };

/// A cursor move repeated from `from` until the cursor reaches `last`.
fn move_stops(expected: &str, from: Position, last: Position, id: &str, locales: &[String]) -> String {
    let text = without_pipes(expected);
    let stops = repeated(&text, from, |editor| run_move(editor, id, locales), |editor| editor.selection().head, |editor| editor.selection().head == last);
    with_pipes(&text, stops)
}

/// A one-line delete from the end, recording the cursor, until the text is empty.
fn delete_left_stops(expected: &str, id: &'static str) -> String {
    let text = without_pipes(expected);
    let stops = repeated(&text, at(1000, 10000), |editor| run_delete(editor, id), |editor| editor.selection().head, |editor| text_is_empty(editor));
    with_pipes(&text, stops)
}

/// A one-line delete from the start, recording how much of the text is gone, until the text is empty.
fn delete_right_stops(expected: &str, id: &'static str) -> String {
    let text = without_pipes(expected);
    let length = text.chars().count();
    let stops = repeated(&text, at(0, 0), |editor| run_delete(editor, id), |editor| at(0, length - value(editor).chars().count()), |editor| text_is_empty(editor));
    with_pipes(&text, stops)
}

fn text_is_empty(editor: &OpenEditor) -> bool {
    value(editor).is_empty()
}

const LINES: &str = "    \tMy First Line\t \n\tMy Second Line\n    Third Line🐶\n\n1";

#[test]
fn cursor_word_left_simple() {
    let expected = "|    \t|My |First |Line\t \n|\t|My |Second |Line\n|    |Third |Line🐶\n|\n|1";
    assert_eq!(move_stops(expected, FAR, at(0, 0), "cursorWordLeft", &[]), expected);
}

#[test]
fn cursor_word_left_with_selection() {
    let mut editor = editor(LINES, "\n");
    editor.set_selections(vec![Selection::caret(at(4, 1))]);
    run_move(&mut editor, "cursorWordLeftSelect", &[]);
    assert_eq!(editor.selection(), selection((4, 1), (4, 0)));
}

#[test]
fn cursor_word_left_issue_832() {
    let expected = "|   |/* |Just |some   |more   |text |a|+= |3 |+|5-|3 |+ |7 |*/  ";
    assert_eq!(move_stops(expected, FAR, at(0, 0), "cursorWordLeft", &[]), expected);
}

#[test]
fn cursor_word_left_issue_48046_word_selection_does_not_work_as_usual() {
    let expected = "|deep.|object.|property";
    assert_eq!(move_stops(expected, at(0, 20), at(0, 0), "cursorWordLeft", &[]), expected);
}

#[test]
fn cursor_word_left_recognize_words() {
    let expected = "|/* |これ|は|テスト|です |/*";
    assert_eq!(move_stops(expected, FAR, at(0, 0), "cursorWordLeftSelect", &["ja".to_string()]), expected);
}

#[test]
fn cursor_word_left_does_not_recognize_words() {
    let expected = "|/* |これはテストです |/*";
    assert_eq!(move_stops(expected, FAR, at(0, 0), "cursorWordLeftSelect", &[]), expected);
}

#[test]
fn cursor_word_left_issue_169904_cursors_out_of_sync() {
    let text = [
        ".grid1 {",
        "  display: grid;",
        "  grid-template-columns:",
        "    [full-start] minmax(1em, 1fr)",
        "    [main-start] minmax(0, 40em) [main-end]",
        "    minmax(1em, 1fr) [full-end];",
        "}",
        ".grid2 {",
        "  display: grid;",
        "  grid-template-columns:",
        "    [full-start] minmax(1em, 1fr)",
        "    [main-start] minmax(0, 40em) [main-end] minmax(1em, 1fr) [full-end];",
        "}",
    ]
    .join("\n");
    let mut editor = editor(&text, "\n");
    editor.set_selections(vec![Selection::caret(at(4, 43)), Selection::caret(at(5, 31)), Selection::caret(at(11, 43)), Selection::caret(at(11, 71))]);
    run_move(&mut editor, "cursorWordLeft", &[]);
    assert_eq!(editor.selections, [Selection::caret(at(4, 42)), Selection::caret(at(5, 30)), Selection::caret(at(11, 42)), Selection::caret(at(11, 70))]);
}

#[test]
fn cursor_word_left_select_issue_74369_behaves_like_cursor_word_left() {
    let expected = "|this.|is.|a.|test";
    assert_eq!(move_stops(expected, at(0, 14), at(0, 0), "cursorWordLeftSelect", &[]), expected);
}

#[test]
fn cursor_word_start_left() {
    let expected = "|   |/* |Just |some   |more   |text |a|+= |3 |+|5|-|3 |+ |7 |*/  ";
    assert_eq!(move_stops(expected, FAR, at(0, 0), "cursorWordStartLeft", &[]), expected);
}

#[test]
fn cursor_word_start_left_issue_51119_vs_compatibility() {
    let expected = "|this|.|is|.|a|.|test";
    assert_eq!(move_stops(expected, FAR, at(0, 0), "cursorWordStartLeft", &[]), expected);
}

#[test]
fn issue_51275_cursor_word_start_left_pushes_an_undo_stop() {
    let mut editor = editor("", "\n");
    for c in "foo bar baz".chars() {
        editor.type_keyboard(&c.to_string(), no_rules(Strategy::Full), false);
    }
    assert_eq!(value(&editor), "foo bar baz");
    run_move(&mut editor, "cursorWordStartLeft", &[]);
    run_move(&mut editor, "cursorWordStartLeft", &[]);
    editor.type_keyboard("q", no_rules(Strategy::Full), false);
    assert_eq!(value(&editor), "foo qbar baz");
    editor.undo();
    assert_eq!(value(&editor), "foo bar baz");
}

#[test]
fn cursor_word_end_left() {
    let expected = "|   /*| Just| some|   more|   text| a|+=| 3| +|5|-|3| +| 7| */|  ";
    assert_eq!(move_stops(expected, FAR, at(0, 0), "cursorWordEndLeft", &[]), expected);
}

#[test]
fn cursor_word_right_simple() {
    let expected = "    \tMy| First| Line|\t |\n\tMy| Second| Line|\n    Third| Line🐶|\n|\n1|";
    assert_eq!(move_stops(expected, at(0, 0), at(4, 1), "cursorWordRight", &[]), expected);
}

#[test]
fn cursor_word_right_selection() {
    let mut editor = editor(LINES, "\n");
    editor.set_selections(vec![Selection::caret(at(0, 0))]);
    run_move(&mut editor, "cursorWordRightSelect", &[]);
    assert_eq!(editor.selection(), selection((0, 0), (0, 7)));
}

#[test]
fn cursor_word_right_issue_832() {
    let expected = "   /*| Just| some|   more|   text| a|+=| 3| +5|-3| +| 7| */|  |";
    assert_eq!(move_stops(expected, at(0, 0), at(0, 49), "cursorWordRight", &[]), expected);
}

#[test]
fn cursor_word_right_issue_41199() {
    let expected = "console|.log|(err|)|";
    assert_eq!(move_stops(expected, at(0, 0), at(0, 16), "cursorWordRight", &[]), expected);
}

#[test]
fn cursor_word_right_recognize_words() {
    let expected = "/*| これ|は|テスト|です|/*|";
    assert_eq!(move_stops(expected, at(0, 0), at(0, 13), "cursorWordRight", &["ja".to_string()]), expected);
}

#[test]
fn cursor_word_right_does_not_recognize_words() {
    let expected = "/*| これはテストです|/*|";
    assert_eq!(move_stops(expected, at(0, 0), at(0, 13), "cursorWordRight", &[]), expected);
}

#[test]
fn move_word_end_right() {
    let expected = "   /*| Just| some|   more|   text| a|+=| 3| +5|-3| +| 7| */|  |";
    assert_eq!(move_stops(expected, at(0, 0), at(0, 49), "cursorWordEndRight", &[]), expected);
}

#[test]
fn move_word_start_right() {
    let expected = "   |/* |Just |some   |more   |text |a|+= |3 |+|5|-|3 |+ |7 |*/  |";
    assert_eq!(move_stops(expected, at(0, 0), at(0, 49), "cursorWordStartRight", &[]), expected);
}

#[test]
fn issue_51119_cursor_word_start_right_vs_compatibility() {
    let expected = "this|.|is|.|a|.|test|";
    assert_eq!(move_stops(expected, at(0, 0), at(0, 14), "cursorWordStartRight", &[]), expected);
}

#[test]
fn issue_64810_cursor_word_start_right_skips_first_word_after_newline() {
    let expected = "Hello |World|\n|Hei |mailman|";
    assert_eq!(move_stops(expected, at(0, 0), at(1, 11), "cursorWordStartRight", &[]), expected);
}

// The upstream helpers swap the Select ids into the non-select runs; the stops are the same either way.
#[test]
fn cursor_word_accessibility_left() {
    let expected = "|   /* |Just |some   |more   |text |a+= |3 +|5-|3 + |7 */  ";
    assert_eq!(move_stops(expected, FAR, at(0, 0), "cursorWordAccessibilityLeftSelect", &[]), expected);
}

#[test]
fn cursor_word_accessibility_right() {
    let expected = "   /* |Just |some   |more   |text |a+= |3 +|5-|3 + |7 */  |";
    assert_eq!(move_stops(expected, at(0, 0), at(0, 49), "cursorWordAccessibilityRight", &[]), expected);
}

/// One delete on `LINES` with the cursor or selection given; returns the line and the cursor after it.
fn delete_once(id: &'static str, selected: Selection, line: usize) -> (String, Position) {
    let mut editor = editor(LINES, "\n");
    editor.set_selections(vec![selected]);
    run_delete(&mut editor, id);
    (editor.document().lines[line].clone(), editor.selection().head)
}

#[test]
fn delete_word_left_and_right_on_the_sample_lines() {
    let caret = |line, column| Selection::caret(at(line, column));
    assert_eq!(delete_once("deleteWordLeft", selection((2, 6), (2, 8)), 2), ("    Thd Line🐶".into(), at(2, 6)));
    assert_eq!(delete_once("deleteWordLeft", caret(0, 0), 0), ("    \tMy First Line\t ".into(), at(0, 0)));
    assert_eq!(delete_once("deleteWordLeft", caret(2, 10), 2), ("    Line🐶".into(), at(2, 4)));
    assert_eq!(delete_once("deleteWordLeft", caret(1, 10), 1), ("\tMy  Line".into(), at(1, 4)));
    assert_eq!(delete_once("deleteWordLeft", caret(0, 11), 0), ("    \tMy st Line\t ".into(), at(0, 8)));
    assert_eq!(delete_once("deleteWordRight", selection((2, 6), (2, 8)), 2), ("    Thd Line🐶".into(), at(2, 6)));
    assert_eq!(delete_once("deleteWordRight", caret(4, 1), 4), ("1".into(), at(4, 1)));
    assert_eq!(delete_once("deleteWordRight", caret(2, 0), 2), ("Third Line🐶".into(), at(2, 0)));
    assert_eq!(delete_once("deleteWordRight", caret(1, 4), 1), ("\tMy  Line".into(), at(1, 4)));
    assert_eq!(delete_once("deleteWordRight", caret(0, 10), 0), ("    \tMy Fi Line\t ".into(), at(0, 10)));
}

#[test]
fn delete_word_left_issue_832() {
    let expected = "|   |/* |Just |some |text |a|+= |3 |+|5 |*/|  ";
    assert_eq!(delete_left_stops(expected, "deleteWordLeft"), expected);
}

#[test]
fn delete_word_start_left() {
    let expected = "|   |/* |Just |some |text |a|+= |3 |+|5 |*/  ";
    assert_eq!(delete_left_stops(expected, "deleteWordStartLeft"), expected);
}

#[test]
fn delete_word_end_left() {
    let expected = "|   /*| Just| some| text| a|+=| 3| +|5| */|  ";
    assert_eq!(delete_left_stops(expected, "deleteWordEndLeft"), expected);
}

#[test]
fn delete_word_left_issue_24947() {
    for id in ["deleteWordLeft", "deleteWordStartLeft", "deleteWordEndLeft"] {
        let mut editor = editor("{\n}", "\n");
        editor.set_selections(vec![Selection::caret(at(1, 0))]);
        run_delete(&mut editor, id);
        assert_eq!(editor.document().lines[0], "{}", "{id}");
    }
}

#[test]
fn delete_word_right_issue_832() {
    let expected = "   |/*| |Just| |some| |text| |a|+=| |3| |+|5|-|3| |*/|  |";
    assert_eq!(delete_right_stops(expected, "deleteWordRight"), expected);
}

#[test]
fn delete_word_right_issue_3882() {
    for id in ["deleteWordRight", "deleteWordStartRight", "deleteWordEndRight"] {
        let mut editor = editor("public void Add( int x,\n                 int y )", "\n");
        editor.set_selections(vec![Selection::caret(at(0, 23))]);
        run_delete(&mut editor, id);
        assert_eq!(editor.document().lines[0], "public void Add( int x,int y )", "{id}");
    }
}

#[test]
fn delete_word_start_right() {
    let expected = "   |/* |Just |some |text |a|+= |3 |+|5|-|3 |*/  |";
    assert_eq!(delete_right_stops(expected, "deleteWordStartRight"), expected);
}

#[test]
fn delete_word_end_right() {
    let expected = "   /*| Just| some| text| a|+=| 3| +|5|-|3| */|  |";
    assert_eq!(delete_right_stops(expected, "deleteWordEndRight"), expected);
}

#[test]
fn delete_word_right_issue_3882_1_at_the_end_of_a_line() {
    let mut editor = editor("A line with text.\n   And another one", "\n");
    editor.set_selections(vec![Selection::caret(at(0, 17))]);
    run_delete(&mut editor, "deleteWordRight");
    assert_eq!(editor.document().lines[0], "A line with text.And another one");
}

#[test]
fn delete_word_left_issue_3882_2_at_the_start_of_a_line() {
    let mut editor = editor("A line with text.\n   And another one", "\n");
    editor.set_selections(vec![Selection::caret(at(1, 0))]);
    run_delete(&mut editor, "deleteWordLeft");
    assert_eq!(editor.document().lines[0], "A line with text.   And another one");
}

#[test]
fn delete_word_left_issue_91855_deletes_a_matching_pair() {
    let pairs = [("\"".to_string(), "\"".to_string())];
    let always = AutoClosing { delete: "always", pairs: &pairs, ..NO_AUTO_CLOSING };
    let mut editor = editor("a \"\"", "\n");
    editor.set_selections(vec![Selection::caret(at(0, 3))]);
    run_delete_with(&mut editor, "deleteWordLeft", &always);
    assert_eq!(editor.document().lines[0], "a ");
}

/// Delete Word repeated from `column` of a one-line text; each value after a run.
fn delete_inside_word_runs(line: &str, column: usize, runs: usize) -> Vec<String> {
    let mut editor = editor(line, "\n");
    editor.set_selections(vec![Selection::caret(at(0, column))]);
    (0..runs).map(|_| {
        delete_inside_word(&mut editor, false);
        value(&editor)
    }).collect()
}

#[test]
fn delete_inside_word_empty_line() {
    let mut editor = editor("Line1\n\nLine2", "\n");
    editor.set_selections(vec![Selection::caret(at(1, 0))]);
    delete_inside_word(&mut editor, false);
    assert_eq!(value(&editor), "Line1\nLine2");
}

#[test]
fn delete_inside_word_in_whitespace() {
    assert_eq!(delete_inside_word_runs("Just  some text.", 5, 1), ["Justsome text."]);
    assert_eq!(delete_inside_word_runs("Just     some text.", 5, 1), ["Justsome text."]);
    assert_eq!(delete_inside_word_runs("Just     \"some text.", 5, 7), ["Just\"some text.", "\"some text.", "some text.", "text.", ".", "", ""]);
}

#[test]
fn delete_inside_word_in_non_words() {
    assert_eq!(delete_inside_word_runs("x=3+4+5+6", 6, 7), ["x=3+45+6", "x=3++6", "x=36", "x=", "x", "", ""]);
}

#[test]
fn delete_inside_word_in_words() {
    assert_eq!(delete_inside_word_runs("This is interesting", 6, 4), ["This interesting", "This", "", ""]);
    assert_eq!(delete_inside_word_runs("This  is  interesting", 6, 4), ["This  interesting", "This", "", ""]);
}

#[test]
fn delete_inside_word_only_word() {
    let mut last_word = editor("hello world", "\n");
    last_word.set_selections(vec![Selection::caret(at(0, 8))]);
    delete_inside_word(&mut last_word, true);
    assert_eq!(value(&last_word), "hello ");
    let mut words = editor("This is interesting", "\n");
    words.set_selections(vec![Selection::caret(at(0, 6))]);
    delete_inside_word(&mut words, true);
    assert_eq!(value(&words), "This  interesting");
}

/// wordPartOperations.test.ts: word part moves and deletes; the #158667 cases leave `-` out of the separators.
#[test]
fn word_part_moves_and_deletes_stop_like_upstream() {
    let kebab_separators = "!\"#&'()*+,./:;<=>?@[\\]^`{|}\u{b7}";
    let classifier = |expected: &str| WordClassifier::new(if expected.contains("kebab") { kebab_separators } else { DEFAULT_WORD_SEPARATORS }, &[]);
    let end = |text: &str| {
        let last = text.split('\n').count() - 1;
        at(last, text.split('\n').last().unwrap().chars().count())
    };
    let part_stops = |expected: &str, right: bool| {
        let text = without_pipes(expected);
        let classifier = classifier(expected);
        let (from, last) = if right { (at(0, 0), end(&text)) } else { (end(&text), at(0, 0)) };
        let stops = repeated(&text, from, |editor| editor.move_word_part(&classifier, right, false), |editor| editor.selection().head, |editor| editor.selection().head == last);
        with_pipes(&text, stops)
    };
    let left_cases = [
        "|start| |line|\n|this|Is|A|Camel|Case|Var|  |this_|is_|a_|snake_|case_|var| |THIS_|IS_|CAPS_|SNAKE| |this_|IS|Mixed|Use|\n|end| |line",
        "|myvar| |=| |'|demonstration|     |of| |selection| |with| |space|'",
        "|myvar| |=| |'|demonstration_____|of| |selection| |with| |space|'",
        "|foo_|bar",
        "|this-|is-|a-|kebab-|case-|var| |THIS-|IS-|CAPS-|KEBAB| |this-|IS|Mixed|Use",
    ];
    for expected in left_cases {
        assert_eq!(part_stops(expected, false), expected);
    }
    let right_cases = [
        "start| |line|\n|this|Is|A|Camel|Case|Var|  |this|_is|_a|_snake|_case|_var| |THIS|_IS|_CAPS|_SNAKE| |this|_IS|Mixed|Use|\n|end| |line|",
        "myvar| |=| |'|demonstration|     |of| |selection| |with| |space|'|",
        "myvar| |=| |'|demonstration|_____of| |selection| |with| |space|'|",
        ";| |--| |1|\n|;|        |--| |2|\n|;|    |#|3|\n|;|   |#|4|",
        "foo|_bar|",
        "this|-is|-a|-kebab|-case|-var| |THIS|-IS|-CAPS|-KEBAB| |this|-IS|Mixed|Use|",
    ];
    for expected in right_cases {
        assert_eq!(part_stops(expected, true), expected);
    }
    let delete_stops = |expected: &str, right: bool| {
        let text = without_pipes(expected);
        let length = text.chars().count();
        let classifier = classifier(expected);
        let (id, from) = if right { ("deleteWordPartRight", at(0, 0)) } else { ("deleteWordPartLeft", FAR) };
        let (_, word_delete) = DELETE_COMMANDS.into_iter().find(|(command, _)| *command == id).unwrap();
        let record = |editor: &OpenEditor| if right { at(0, length - value(editor).chars().count()) } else { editor.selection().head };
        let stops = repeated(&text, from, |editor| { editor.delete_word(&classifier, &NO_AUTO_CLOSING, word_delete, id); }, record, |editor| text_is_empty(editor));
        with_pipes(&text, stops)
    };
    let delete_left = [
        "|   |/*| |Just| |some| |text| |a|+=| |3| |+|5|-|3| |*/|  |this|Is|A|Camel|Case|Var|  |this_|is_|a_|snake_|case_|var| |THIS_|IS_|CAPS_|SNAKE| |this_|IS|Mixed|Use",
        "|this-|is-|a-|kebab-|case-|var| |THIS-|IS-|CAPS-|KEBAB| |this-|IS|Mixed|Use",
    ];
    for expected in delete_left {
        assert_eq!(delete_stops(expected, false), expected);
    }
    let delete_right = [
        "   |/*| |Just| |some| |text| |a|+=| |3| |+|5|-|3| |*/|  |this|Is|A|Camel|Case|Var|  |this|_is|_a|_snake|_case|_var| |THIS|_IS|_CAPS|_SNAKE| |this|_IS|Mixed|Use|",
        "this|-is|-a|-kebab|-case|-var| |THIS|-IS|-CAPS|-KEBAB| |this|-IS|Mixed|Use|",
    ];
    for expected in delete_right {
        assert_eq!(delete_stops(expected, true), expected);
    }
}

#[test]
fn segmenter_locales_validate_like_upstream() {
    assert_eq!(segmenter_locales(Some(&serde_json::json!("ja"))), ["ja"]);
    assert_eq!(segmenter_locales(Some(&serde_json::json!(["zh-CN", 3, "not a locale!"]))), ["zh-CN"]);
    assert!(segmenter_locales(Some(&serde_json::json!(true))).is_empty());
}
