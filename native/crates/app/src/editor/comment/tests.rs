//! lineCommentCommand.test.ts and blockCommentCommand.test.ts. Selections are upstream's 1-based
//! (anchor line, anchor column, head line, head column).

use super::*;
use crate::editor::edit::tests::editor;

type Sel = (usize, usize, usize, usize);

fn selection((anchor_line, anchor_column, head_line, head_column): Sel) -> Selection {
    Selection { anchor: Position::new(anchor_line - 1, anchor_column - 1), head: Position::new(head_line - 1, head_column - 1) }
}

/// `testCommand`: one cursor at `given` runs the command, then the text and the selection.
fn run(lines: &[&str], given: Sel, expected: &[&str], expected_selection: Sel, make: impl FnOnce(&[String], Selection) -> Command) {
    let mut model = editor(&lines.join("\n"), "\n");
    model.set_selections(vec![selection(given)]);
    let made = make(&model.document.lines, model.selections[0]);
    model.execute_commands(vec![made], command("tests"));
    assert_eq!(model.document.lines, expected);
    assert_eq!(model.selections, [selection(expected_selection)]);
}

const MARKS: Comments<'static> = Comments { line: Some("!@#"), line_no_indent: false, block: Some(("<!@#", "#@!>")) };

#[allow(clippy::too_many_arguments)]
fn line_with(comments: Comments<'static>, kind: LineCommentKind, insert_space: bool, ignore_empty_lines: bool, lines: &[&str], given: Sel, expected: &[&str], expected_selection: Sel) {
    run(lines, given, expected, expected_selection, |text, selection| line_comment_command(text, selection, 4, kind, insert_space, ignore_empty_lines, false, &|_, _| comments));
}

fn line(lines: &[&str], given: Sel, expected: &[&str], expected_selection: Sel) {
    line_with(MARKS, LineCommentKind::Toggle, true, true, lines, given, expected, expected_selection);
}

fn line_force_add(lines: &[&str], given: Sel, expected: &[&str], expected_selection: Sel) {
    line_with(MARKS, LineCommentKind::ForceAdd, true, true, lines, given, expected, expected_selection);
}

fn line_no_indent(lines: &[&str], given: Sel, expected: &[&str], expected_selection: Sel) {
    line_with(Comments { line_no_indent: true, ..MARKS }, LineCommentKind::Toggle, true, true, lines, given, expected, expected_selection);
}

fn line_rem(lines: &[&str], given: Sel, expected: &[&str], expected_selection: Sel) {
    line_with(Comments { line: Some("rem"), ..Comments::default() }, LineCommentKind::Toggle, true, true, lines, given, expected, expected_selection);
}

fn line_without_space(lines: &[&str], given: Sel, expected: &[&str], expected_selection: Sel) {
    line_with(Comments { line: Some("!@#"), ..Comments::default() }, LineCommentKind::Toggle, false, true, lines, given, expected, expected_selection);
}

fn line_keeping_empty_lines(lines: &[&str], given: Sel, expected: &[&str], expected_selection: Sel) {
    line_with(MARKS, LineCommentKind::Toggle, true, false, lines, given, expected, expected_selection);
}

fn line_as_parens(lines: &[&str], given: Sel, expected: &[&str], expected_selection: Sel) {
    line_with(Comments { line: None, line_no_indent: false, block: Some(("(", ")")) }, LineCommentKind::Toggle, true, true, lines, given, expected, expected_selection);
}

fn line_as_block(lines: &[&str], given: Sel, expected: &[&str], expected_selection: Sel) {
    line_with(Comments { line: None, ..MARKS }, LineCommentKind::Toggle, true, true, lines, given, expected, expected_selection);
}

/// The outer language's tokenizer gives a line starting with two spaces to the inner language.
fn line_in_mixed_modes(lines: &[&str], given: Sel, expected: &[&str], expected_selection: Sel) {
    let outer = Comments { line: Some("//"), line_no_indent: false, block: Some(("/*", "*/")) };
    let inner = Comments { line: None, line_no_indent: false, block: Some(("{/*", "*/}")) };
    run(lines, given, expected, expected_selection, |text, selection| {
        line_comment_command(text, selection, 4, LineCommentKind::Toggle, true, true, false, &|line, _| if text[line].starts_with("  ") { inner } else { outer })
    });
}

fn block_with(insert_space: bool, lines: &[&str], given: Sel, expected: &[&str], expected_selection: Sel) {
    let comments = Comments { line: Some("!@#"), line_no_indent: false, block: Some(("<0", "0>")) };
    run(lines, given, expected, expected_selection, |text, selection| block_comment_command(text, selection, insert_space, &|_, _| comments));
}

fn block(lines: &[&str], given: Sel, expected: &[&str], expected_selection: Sel) {
    block_with(true, lines, given, expected, expected_selection);
}

fn block_without_space(lines: &[&str], given: Sel, expected: &[&str], expected_selection: Sel) {
    block_with(false, lines, given, expected, expected_selection);
}

fn preflight(tokens: [&str; 4]) -> Vec<LinePreflight<'_>> {
    tokens.into_iter().map(|comment| LinePreflight { ignore: false, comment, offset: 0, length: comment.len() }).collect()
}

fn lines(text: &[&str]) -> Vec<String> {
    text.iter().map(|line| line.to_string()).collect()
}

#[test]
fn analyze_lines_fills_in_the_lines() {
    let mut data = preflight(["//", "rem", "!@#", "!@#"]);
    let should_remove = analyze_lines(LineCommentKind::Toggle, true, &lines(&["\t\t", "    ", "    c", "\t\td"]), &mut data, 0, true, false, false);
    assert!(!should_remove);
    assert_eq!(data.iter().map(|line| line.comment).collect::<Vec<_>>(), ["//", "rem", "!@#", "!@#"]);
    assert_eq!(data.iter().map(|line| line.ignore).collect::<Vec<_>>(), [true, true, false, false]);
    assert_eq!(data.iter().map(|line| line.offset).collect::<Vec<_>>(), [2, 4, 4, 2]);

    let mut data = preflight(["//", "rem", "!@#", "!@#"]);
    let should_remove = analyze_lines(LineCommentKind::Toggle, true, &lines(&["\t\t", "    rem ", "    !@# c", "\t\t!@#d"]), &mut data, 0, true, false, false);
    assert!(should_remove);
    assert_eq!(data.iter().map(|line| line.comment).collect::<Vec<_>>(), ["//", "rem", "!@#", "!@#"]);
    assert_eq!(data.iter().map(|line| line.ignore).collect::<Vec<_>>(), [true, false, false, false]);
    assert_eq!(data.iter().map(|line| line.offset).collect::<Vec<_>>(), [2, 4, 4, 2]);
    assert_eq!(data.iter().map(|line| line.length).collect::<Vec<_>>(), [2, 4, 4, 3]);
}

#[test]
fn normalize_insertion_point_aligns_the_tokens() {
    let check = |given: &[(&str, usize)], indent_size: usize, expected: &[usize], name: &str| {
        let text: Vec<String> = given.iter().map(|(line, _)| line.to_string()).collect();
        let mut data: Vec<LinePreflight> = given.iter().map(|&(_, offset)| LinePreflight { ignore: false, comment: "", offset, length: 0 }).collect();
        normalize_insertion_point(&text, &mut data, 0, indent_size);
        assert_eq!(data.iter().map(|line| line.offset).collect::<Vec<_>>(), expected, "{name}");
    };
    check(&[("  XX", 2), ("    YY", 4)], 4, &[0, 0], "Bug 16696");
    check(&[("\t\t\tXX", 3), ("    \tYY", 5), ("        ZZ", 8), ("\t\tTT", 2)], 4, &[2, 5, 8, 2], "Test1");
    check(&[("\t\t\t   XX", 6), ("    \t\t\t\tYY", 8), ("        ZZ", 8), ("\t\t    TT", 6)], 4, &[2, 5, 8, 2], "Test2");
    check(&[("\t\t", 2), ("\t\t\t", 3), ("\t\t\t\t", 4), ("\t\t\t", 3)], 4, &[2, 2, 2, 2], "Test3");
    check(&[("\t\t", 2), ("\t\t\t", 3), ("\t\t\t\t", 4), ("\t\t\t", 3), ("    ", 4)], 2, &[2, 2, 2, 2, 4], "Test4");
    check(&[("\t\t", 2), ("\t\t\t", 3), ("\t\t\t\t", 4), ("\t\t\t", 3), ("    ", 4)], 4, &[1, 1, 1, 1, 4], "Test5");
    check(&[(" \t", 2), ("  \t", 3), ("   \t", 4), ("    ", 4), ("\t", 1)], 4, &[2, 3, 4, 4, 1], "Test6");
    check(&[(" \t\t", 3), ("  \t\t", 4), ("   \t\t", 5), ("    \t", 5), ("\t", 1)], 4, &[2, 3, 4, 4, 1], "Test7");
    check(&[("\t", 1), ("    ", 4)], 4, &[1, 4], "Test8:4");
    check(&[("\t", 1), ("   ", 3)], 4, &[0, 0], "Test8:3");
    check(&[("\t", 1), ("  ", 2)], 4, &[0, 0], "Test8:2");
    check(&[("\t", 1), (" ", 1)], 4, &[0, 0], "Test8:1");
    check(&[("\t", 1), ("", 0)], 4, &[0, 0], "Test8:0");
}

/// comment.ts: of two cursors on one line, the later added one leaves that line alone; the grown selections
/// then overlap and merge. The action is one undo step.
#[test]
fn comment_lines_comments_a_line_shared_by_two_cursors_once() {
    let mut model = editor("a\nb\nc", "\n");
    let languages = Languages::new(&[]);
    model.set_selections(vec![selection((1, 1, 2, 2)), selection((2, 1, 3, 2))]);
    model.comment_lines(LineCommentKind::Toggle, true, true, &languages);
    assert_eq!(model.document.lines, ["a", "b", "c"], "plaintext has no comment tokens");
    let mut model = editor("a\nb\nc", "\n");
    let languages = typescript();
    model.set_selections(vec![selection((1, 1, 2, 2)), selection((2, 1, 3, 2))]);
    model.comment_lines(LineCommentKind::Toggle, true, true, &languages);
    assert_eq!(model.document.lines, ["// a", "// b", "// c"]);
    assert_eq!(model.selections, [selection((1, 4, 3, 5))]);
    model.undo();
    assert_eq!(model.document.lines, ["a", "b", "c"]);
}

fn typescript() -> Languages {
    let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    Languages::new(&[exthost::scan::scan_extension(&app_root.join("extensions/typescript-basics"), true).unwrap()])
}

mod line_comment {
    use super::*;

    #[test]
    fn comment_single_line() {
        line(&["some text", "\tsome more text"], (1, 1, 1, 1), &["!@# some text", "\tsome more text"], (1, 5, 1, 5));
    }

    #[test]
    fn case_insensitive() {
        line_rem(&["REM some text"], (1, 1, 1, 1), &["some text"], (1, 1, 1, 1));
    }

    #[test]
    fn comment_with_token_column_fixed() {
        line_no_indent(&["some text", "\tsome more text"], (2, 1, 2, 1), &["some text", "!@# \tsome more text"], (2, 5, 2, 5));
    }

    #[test]
    fn detects_indentation() {
        line(&["\tsome text", "\tsome more text"], (2, 2, 1, 1), &["\t!@# some text", "\t!@# some more text"], (2, 2, 1, 1));
    }

    #[test]
    fn detects_mixed_indentation() {
        line(&["\tsome text", "    some more text"], (2, 2, 1, 1), &["\t!@# some text", "    !@# some more text"], (2, 2, 1, 1));
    }

    #[test]
    fn ignores_whitespace_lines() {
        line(&["\tsome text", "\t   ", "", "\tsome more text"], (4, 2, 1, 1), &["\t!@# some text", "\t   ", "", "\t!@# some more text"], (4, 2, 1, 1));
    }

    #[test]
    fn removes_its_own() {
        line(&["\t!@# some text", "\t   ", "\t\t!@# some more text"], (3, 2, 1, 1), &["\tsome text", "\t   ", "\t\tsome more text"], (3, 2, 1, 1));
    }

    #[test]
    fn works_in_only_whitespace() {
        line(&["\t    ", "\t", "\t\tsome more text"], (3, 1, 1, 1), &["\t!@#     ", "\t!@# ", "\t\tsome more text"], (3, 1, 1, 1));
    }

    #[test]
    fn bug_9697_whitespace_before_comment_token() {
        line(&["\t !@#first", "\tsecond line"], (1, 1, 1, 1), &["\t first", "\tsecond line"], (1, 1, 1, 1));
    }

    #[test]
    fn bug_10162_line_comment_before_caret() {
        line(&["first!@#", "\tsecond line"], (1, 1, 1, 1), &["!@# first!@#", "\tsecond line"], (1, 5, 1, 5));
    }

    #[test]
    fn comment_single_line_leading_whitespace() {
        line(&["first!@#", "\tsecond line"], (2, 3, 2, 1), &["first!@#", "\t!@# second line"], (2, 7, 2, 1));
    }

    #[test]
    fn ignores_invisible_selection() {
        line(&["first", "\tsecond line", "third line", "fourth line", "fifth"], (2, 1, 1, 1), &["!@# first", "\tsecond line", "third line", "fourth line", "fifth"], (2, 1, 1, 5));
    }

    #[test]
    fn multiple_lines() {
        line(&["first", "\tsecond line", "third line", "fourth line", "fifth"], (2, 4, 1, 1), &["!@# first", "!@# \tsecond line", "third line", "fourth line", "fifth"], (2, 8, 1, 5));
    }

    #[test]
    fn multiple_modes_on_multiple_lines() {
        line(&["first", "\tsecond line", "third line", "fourth line", "fifth"], (4, 4, 3, 1), &["first", "\tsecond line", "!@# third line", "!@# fourth line", "fifth"], (4, 8, 3, 5));
    }

    #[test]
    fn toggle_single_line() {
        line(&["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 1, 1, 1), &["!@# first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 5, 1, 5));
        line(&["!@# first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 4, 1, 4), &["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 1, 1, 1));
    }

    #[test]
    fn toggle_multiple_lines() {
        line(&["first", "\tsecond line", "third line", "fourth line", "fifth"], (2, 4, 1, 1), &["!@# first", "!@# \tsecond line", "third line", "fourth line", "fifth"], (2, 8, 1, 5));
        line(&["!@# first", "!@# \tsecond line", "third line", "fourth line", "fifth"], (2, 7, 1, 4), &["first", "\tsecond line", "third line", "fourth line", "fifth"], (2, 3, 1, 1));
    }

    #[test]
    fn issue_5964_ctrl_to_create_comment_when_cursor_is_at_the_beginning_of_the_line_puts_the_cursor_in_a_strange_position() {
        line(&["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 1, 1, 1), &["!@# first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 5, 1, 5));
    }

    #[test]
    fn issue_35673_comment_hotkeys_throws_the_cursor_before_the_comment() {
        line(&["first", "", "\tsecond line", "third line", "fourth line", "fifth"], (2, 1, 2, 1), &["first", "!@# ", "\tsecond line", "third line", "fourth line", "fifth"], (2, 5, 2, 5));
        line(&["first", "\t", "\tsecond line", "third line", "fourth line", "fifth"], (2, 2, 2, 2), &["first", "\t!@# ", "\tsecond line", "third line", "fourth line", "fifth"], (2, 6, 2, 6));
    }

    #[test]
    fn issue_2837_add_line_comment_fault_when_blank_lines_involved() {
        line_force_add(&["    if displayName == \"\":", "        displayName = groupName", "    description = getAttr(attributes, \"description\")", "    mailAddress = getAttr(attributes, \"mail\")", "", "    print \"||Group name|%s|\" % displayName", "    print \"||Description|%s|\" % description", "    print \"||Email address|[mailto:%s]|\" % mailAddress`"], (1, 1, 8, 56), &["    !@# if displayName == \"\":", "    !@#     displayName = groupName", "    !@# description = getAttr(attributes, \"description\")", "    !@# mailAddress = getAttr(attributes, \"mail\")", "", "    !@# print \"||Group name|%s|\" % displayName", "    !@# print \"||Description|%s|\" % description", "    !@# print \"||Email address|[mailto:%s]|\" % mailAddress`"], (1, 1, 8, 60));
    }

    #[test]
    fn issue_47004_toggle_comments_shouldn_t_move_cursor() {
        line_force_add(&["    A line", "    Another line"], (2, 7, 1, 1), &["    !@# A line", "    !@# Another line"], (2, 11, 1, 1));
    }

    #[test]
    fn insertspace_false() {
        line_without_space(&["some text"], (1, 1, 1, 1), &["!@#some text"], (1, 4, 1, 4));
    }

    #[test]
    fn insertspace_false_does_not_remove_space() {
        line_without_space(&["!@#    some text"], (1, 1, 1, 1), &["    some text"], (1, 1, 1, 1));
    }

    #[test]
    fn does_not_ignore_whitespace_lines() {
        line_keeping_empty_lines(&["\tsome text", "\t   ", "", "\tsome more text"], (4, 2, 1, 1), &["!@# \tsome text", "!@# \t   ", "!@# ", "!@# \tsome more text"], (4, 6, 1, 5));
    }

    #[test]
    fn removes_its_own_2() {
        line_keeping_empty_lines(&["\t!@# some text", "\t   ", "\t\t!@# some more text"], (3, 2, 1, 1), &["\tsome text", "\t   ", "\t\tsome more text"], (3, 2, 1, 1));
    }

    #[test]
    fn works_in_only_whitespace_2() {
        line_keeping_empty_lines(&["\t    ", "\t", "\t\tsome more text"], (3, 1, 1, 1), &["\t!@#     ", "\t!@# ", "\t\tsome more text"], (3, 1, 1, 1));
    }

    #[test]
    fn comments_single_line() {
        line_keeping_empty_lines(&["some text", "\tsome more text"], (1, 1, 1, 1), &["!@# some text", "\tsome more text"], (1, 5, 1, 5));
    }

    #[test]
    fn detects_indentation_2() {
        line_keeping_empty_lines(&["\tsome text", "\tsome more text"], (2, 2, 1, 1), &["\t!@# some text", "\t!@# some more text"], (2, 2, 1, 1));
    }

    #[test]
    fn fall_back_to_block_comment_command() {
        line_as_parens(&["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 1, 1, 1), &["( first )", "\tsecond line", "third line", "fourth line", "fifth"], (1, 3, 1, 3));
    }

    #[test]
    fn fall_back_to_block_comment_command_toggle() {
        line_as_parens(&["(first)", "\tsecond line", "third line", "fourth line", "fifth"], (1, 7, 1, 2), &["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 6, 1, 1));
    }

    #[test]
    fn bug_9513_expand_single_line_to_uncomment_auto_block() {
        line_as_parens(&["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 1, 1, 1), &["( first )", "\tsecond line", "third line", "fourth line", "fifth"], (1, 3, 1, 3));
    }

    #[test]
    fn bug_9691_always_expand_selection_to_line_boundaries() {
        line_as_parens(&["first", "\tsecond line", "third line", "fourth line", "fifth"], (3, 2, 1, 3), &["( first", "\tsecond line", "third line )", "fourth line", "fifth"], (3, 2, 1, 5));
        line_as_parens(&["(first", "\tsecond line", "third line)", "fourth line", "fifth"], (3, 11, 1, 2), &["first", "\tsecond line", "third line", "fourth line", "fifth"], (3, 11, 1, 1));
    }

    #[test]
    fn no_selection_uses_indentation() {
        line_as_block(&["\t\tfirst\t    ", "\t\tsecond line", "\tthird line", "fourth line", "\t\t<!@#fifth#@!>\t\t"], (1, 1, 1, 1), &["\t\t<!@# first\t     #@!>", "\t\tsecond line", "\tthird line", "fourth line", "\t\t<!@#fifth#@!>\t\t"], (1, 1, 1, 1));
        line_as_block(&["\t\t<!@#first\t    #@!>", "\t\tsecond line", "\tthird line", "fourth line", "\t\t<!@#fifth#@!>\t\t"], (1, 1, 1, 1), &["\t\tfirst\t   ", "\t\tsecond line", "\tthird line", "fourth line", "\t\t<!@#fifth#@!>\t\t"], (1, 1, 1, 1));
    }

    #[test]
    fn can_remove() {
        line_as_block(&["\t\tfirst\t    ", "\t\tsecond line", "\tthird line", "fourth line", "\t\t<!@#fifth#@!>\t\t"], (5, 1, 5, 1), &["\t\tfirst\t    ", "\t\tsecond line", "\tthird line", "fourth line", "\t\tfifth\t\t"], (5, 1, 5, 1));
        line_as_block(&["\t\tfirst\t    ", "\t\tsecond line", "\tthird line", "fourth line", "\t\t<!@#fifth#@!>\t\t"], (5, 3, 5, 3), &["\t\tfirst\t    ", "\t\tsecond line", "\tthird line", "fourth line", "\t\tfifth\t\t"], (5, 3, 5, 3));
        line_as_block(&["\t\tfirst\t    ", "\t\tsecond line", "\tthird line", "fourth line", "\t\t<!@#fifth#@!>\t\t"], (5, 4, 5, 4), &["\t\tfirst\t    ", "\t\tsecond line", "\tthird line", "fourth line", "\t\tfifth\t\t"], (5, 3, 5, 3));
        line_as_block(&["\t\tfirst\t    ", "\t\tsecond line", "\tthird line", "fourth line", "\t\t<!@#fifth#@!>\t\t"], (5, 16, 5, 3), &["\t\tfirst\t    ", "\t\tsecond line", "\tthird line", "fourth line", "\t\tfifth\t\t"], (5, 8, 5, 3));
        line_as_block(&["\t\tfirst\t    ", "\t\tsecond line", "\tthird line", "fourth line", "\t\t<!@#fifth#@!>\t\t"], (5, 12, 5, 7), &["\t\tfirst\t    ", "\t\tsecond line", "\tthird line", "fourth line", "\t\tfifth\t\t"], (5, 8, 5, 3));
        line_as_block(&["\t\tfirst\t    ", "\t\tsecond line", "\tthird line", "fourth line", "\t\t<!@#fifth#@!>\t\t"], (5, 18, 5, 18), &["\t\tfirst\t    ", "\t\tsecond line", "\tthird line", "fourth line", "\t\tfifth\t\t"], (5, 10, 5, 10));
    }

    #[test]
    fn issue_993_remove_comment_does_not_work_consistently_in_html() {
        line_as_block(&["     asd qwe", "     asd qwe", ""], (1, 1, 3, 1), &["     <!@# asd qwe", "     asd qwe #@!>", ""], (1, 1, 3, 1));
        line_as_block(&["     <!@#asd qwe", "     asd qwe#@!>", ""], (1, 1, 3, 1), &["     asd qwe", "     asd qwe", ""], (1, 1, 3, 1));
    }

    #[test]
    fn issue_24047_part_1_commenting_code_in_jsx_files() {
        line_in_mixed_modes(&["import React from 'react';", "const Loader = () => (", "  <div>", "    Loading...", "  </div>", ");", "export default Loader;"], (1, 1, 7, 22), &["// import React from 'react';", "// const Loader = () => (", "//   <div>", "//     Loading...", "//   </div>", "// );", "// export default Loader;"], (1, 4, 7, 25));
    }

    #[test]
    fn issue_24047_part_2_commenting_code_in_jsx_files() {
        line_in_mixed_modes(&["import React from 'react';", "const Loader = () => (", "  <div>", "    Loading...", "  </div>", ");", "export default Loader;"], (3, 4, 3, 4), &["import React from 'react';", "const Loader = () => (", "  {/* <div> */}", "    Loading...", "  </div>", ");", "export default Loader;"], (3, 8, 3, 8));
    }

    #[test]
    fn issue_36173_commenting_code_in_jsx_tag_body() {
        line_in_mixed_modes(&["<div>", "  {123}", "</div>"], (2, 4, 2, 4), &["<div>", "  {/* {123} */}", "</div>"], (2, 8, 2, 8));
    }

}

mod block_comment {
    use super::*;

    #[test]
    fn empty_selection_wraps_itself() {
        block(&["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 3, 1, 3), &["fi<0  0>rst", "\tsecond line", "third line", "fourth line", "fifth"], (1, 6, 1, 6));
    }

    #[test]
    fn invisible_selection_ignored() {
        block(&["first", "\tsecond line", "third line", "fourth line", "fifth"], (2, 1, 1, 1), &["<0 first", " 0>\tsecond line", "third line", "fourth line", "fifth"], (1, 4, 2, 1));
    }

    #[test]
    fn bug9511() {
        block(&["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 6, 1, 1), &["<0 first 0>", "\tsecond line", "third line", "fourth line", "fifth"], (1, 4, 1, 9));
        block(&["<0first0>", "\tsecond line", "third line", "fourth line", "fifth"], (1, 8, 1, 3), &["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 1, 1, 6));
    }

    #[test]
    fn one_line_selection() {
        block(&["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 6, 1, 3), &["fi<0 rst 0>", "\tsecond line", "third line", "fourth line", "fifth"], (1, 6, 1, 9));
    }

    #[test]
    fn one_line_selection_toggle() {
        block(&["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 6, 1, 3), &["fi<0 rst 0>", "\tsecond line", "third line", "fourth line", "fifth"], (1, 6, 1, 9));
        block(&["fi<0rst0>", "\tsecond line", "third line", "fourth line", "fifth"], (1, 8, 1, 5), &["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 3, 1, 6));
        block(&["<0 first 0>", "\tsecond line", "third line", "fourth line", "fifth"], (1, 10, 1, 1), &["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 1, 1, 6));
        block(&["<0 first0>", "\tsecond line", "third line", "fourth line", "fifth"], (1, 9, 1, 1), &["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 1, 1, 6));
        block(&["<0first 0>", "\tsecond line", "third line", "fourth line", "fifth"], (1, 9, 1, 1), &["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 1, 1, 6));
        block(&["fi<0rst0>", "\tsecond line", "third line", "fourth line", "fifth"], (1, 8, 1, 5), &["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 3, 1, 6));
    }

    #[test]
    fn multi_line_selection() {
        block(&["first", "\tsecond line", "third line", "fourth line", "fifth"], (2, 4, 1, 1), &["<0 first", "\tse 0>cond line", "third line", "fourth line", "fifth"], (1, 4, 2, 4));
    }

    #[test]
    fn multi_line_selection_toggle() {
        block(&["first", "\tsecond line", "third line", "fourth line", "fifth"], (2, 4, 1, 1), &["<0 first", "\tse 0>cond line", "third line", "fourth line", "fifth"], (1, 4, 2, 4));
        block(&["<0first", "\tse0>cond line", "third line", "fourth line", "fifth"], (2, 4, 1, 3), &["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 1, 2, 4));
        block(&["<0 first", "\tse0>cond line", "third line", "fourth line", "fifth"], (2, 4, 1, 3), &["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 1, 2, 4));
        block(&["<0first", "\tse 0>cond line", "third line", "fourth line", "fifth"], (2, 4, 1, 3), &["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 1, 2, 4));
        block(&["<0 first", "\tse 0>cond line", "third line", "fourth line", "fifth"], (2, 4, 1, 3), &["first", "\tsecond line", "third line", "fourth line", "fifth"], (1, 1, 2, 4));
    }

    #[test]
    fn fuzzy_removes() {
        block(&["asd <0 qwe", "asd 0> qwe"], (2, 5, 1, 7), &["asd qwe", "asd qwe"], (1, 5, 2, 4));
        block(&["asd <0 qwe", "asd 0> qwe"], (2, 5, 1, 6), &["asd qwe", "asd qwe"], (1, 5, 2, 4));
        block(&["asd <0 qwe", "asd 0> qwe"], (2, 5, 1, 5), &["asd qwe", "asd qwe"], (1, 5, 2, 4));
        block(&["asd <0 qwe", "asd 0> qwe"], (2, 5, 1, 11), &["asd qwe", "asd qwe"], (1, 5, 2, 4));
        block(&["asd <0 qwe", "asd 0> qwe"], (2, 1, 1, 11), &["asd qwe", "asd qwe"], (1, 5, 2, 4));
        block(&["asd <0 qwe", "asd 0> qwe"], (2, 7, 1, 11), &["asd qwe", "asd qwe"], (1, 5, 2, 4));
    }

    #[test]
    fn bug_30358() {
        block(&["<0 start 0> middle end"], (1, 20, 1, 23), &["<0 start 0> middle <0 end 0>"], (1, 23, 1, 26));
        block(&["<0 start 0> middle <0 end 0>"], (1, 13, 1, 19), &["<0 start 0> <0 middle 0> <0 end 0>"], (1, 16, 1, 22));
    }

    #[test]
    fn issue_34618() {
        block(&["<0  0> middle end"], (1, 4, 1, 4), &[" middle end"], (1, 1, 1, 1));
    }

    #[test]
    fn insertspace_false() {
        block_without_space(&["some text"], (1, 1, 1, 5), &["<0some0> text"], (1, 3, 1, 7));
    }

    #[test]
    fn insertspace_false_does_not_remove_space() {
        block_without_space(&["<0 some 0> text"], (1, 4, 1, 8), &[" some  text"], (1, 1, 1, 7));
    }

}
