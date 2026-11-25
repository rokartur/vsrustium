//! The auto-indent cases of cursor.test.ts ('Editor Controller'): Enter, typing and Tab under onEnter
//! rules, indentation rules and electric brackets. Positions are upstream's 1-based (line, column).

use serde_json::{Value, json};

use super::*;
use crate::editor::cursor::Selection;
use crate::editor::edit::tests::editor;

mod indentation;

/// `setupIndentRulesLanguage(indentRulesLanguageId, ...)` of the suite's setup.
fn indent_rules_language() -> Value {
    json!({ "indentationRules": {
        "decreaseIndentPattern": r"^\s*((?!\S.*\/[*]).*[*]\/\s*)?[})\]]|^\s*(case\b.*|default):\s*(\/\/.*|\/[*].*[*]\/\s*)?$",
        "increaseIndentPattern": r#"^((?!\/\/).)*(\{[^}"'`]*|\([^)"']*|\[[^\]"']*|^\s*(\{\}|\(\)|\[\]|(case\b.*|default):))\s*(\/\/.*|\/[*].*[*]\/\s*)?$"#,
        "indentNextLinePattern": r"^\s*(for|while|if|else)\b(?!.*[;{}]\s*(\/\/.*|\/[*].*[*]\/\s*)?$)",
        "unIndentedLinePattern": r#"^(?!.*([;{}]|\S:)\s*(\/\/.*|\/[*].*[*]\/\s*)?$)(?!.*(\{[^}"']*|\([^)"']*|\[[^\]"']*|^\s*(\{\}|\(\)|\[\]|(case\b.*|default):))\s*(\/\/.*|\/[*].*[*]\/\s*)?$)(?!^\s*((?!\S.*\/[*]).*[*]\/\s*)?[})\]]|^\s*(case\b.*|default):\s*(\/\/.*|\/[*].*[*]\/\s*)?$)(?!^\s*(for|while|if|else)\b(?!.*[;{}]\s*(\/\/.*|\/[*].*[*]\/\s*)?$))"#,
    } })
}

/// `setupOnEnterLanguage(indentAction)`.
fn on_enter_language(indent: &str) -> Value {
    json!({ "onEnterRules": [{ "beforeText": ".*", "action": { "indent": indent } }] })
}

/// `usingCursor` / `withTestCodeEditor`: a model in a language with the configuration `config`, tab size 4,
/// spaces, `editor.autoIndent` full.
struct Cursor {
    editor: OpenEditor,
    languages: Languages,
    strategy: Strategy,
}

impl Cursor {
    fn new(config: Value, lines: &[&str]) -> Self {
        let mut languages = Languages::new(&[]);
        languages.configure("testLanguage", &config);
        let mut editor = editor(&lines.join("\n"), "\n");
        editor.document_mut().language_id = "testLanguage".into();
        Cursor { editor, languages, strategy: Strategy::Full }
    }

    fn tabs(mut self) -> Self {
        self.editor.document_mut().indentation.insert_spaces = false;
        self
    }

    fn tab_size(mut self, tab_size: usize) -> Self {
        self.editor.document_mut().indentation.tab_size = tab_size;
        self
    }

    fn strategy(mut self, strategy: Strategy) -> Self {
        self.strategy = strategy;
        self
    }

    fn move_to(&mut self, line: usize, column: usize) {
        self.editor.set_selections(vec![Selection::caret(Position::new(line - 1, column - 1))]);
    }

    fn select(&mut self, (anchor_line, anchor_column): (usize, usize), (head_line, head_column): (usize, usize)) {
        self.editor.set_selections(vec![Selection { anchor: Position::new(anchor_line - 1, anchor_column - 1), head: Position::new(head_line - 1, head_column - 1) }]);
    }

    /// `viewModel.type(text, 'keyboard')`.
    fn type_text(&mut self, text: &str) {
        self.editor.type_keyboard(text, AutoIndent { languages: &self.languages, strategy: self.strategy }, false);
    }

    /// `CoreEditingCommands.Tab`.
    fn tab(&mut self) {
        self.editor.tab(AutoIndent { languages: &self.languages, strategy: self.strategy });
    }

    /// `assertCursor`: the selection as (anchor line, anchor column, head line, head column).
    fn cursor(&self) -> (usize, usize, usize, usize) {
        let selection = self.editor.selection();
        (selection.anchor.line + 1, selection.anchor.column + 1, selection.head.line + 1, selection.head.column + 1)
    }

    // The model sits behind a RefCell; a leaked copy keeps the assertions' `&str` tuples.
    fn line(&self, line: usize) -> &'static str {
        self.editor.document().lines[line - 1].clone().leak()
    }

    fn value(&self) -> String {
        self.editor.document().lines.join("\n")
    }
}

#[test]
fn enter_auto_indents_with_insert_spaces_setting() {
    for (indent, text, expected) in [("indent", "\thello", "\thello\r\n        "), ("none", "\thello", "\thello\r\n    "), ("indentOutdent", "\thell()", "\thell(\r\n        \r\n    )")] {
        let mut cursor = Cursor::new(on_enter_language(indent), &[text]);
        cursor.move_to(1, 7);
        cursor.type_text("\n");
        assert_eq!(cursor.editor.document().lines.join("\r\n"), expected, "{indent}");
    }
}

#[test]
fn issue_148256_pressing_enter_creates_line_with_bad_indent() {
    let mut spaces = Cursor::new(json!({}), &["  \t"]);
    spaces.move_to(1, 4);
    spaces.type_text("\n");
    assert_eq!(spaces.value(), "  \t\n    ");
    let mut tabs = Cursor::new(json!({}), &["  \t"]).tabs();
    tabs.move_to(1, 4);
    tabs.type_text("\n");
    assert_eq!(tabs.value(), "  \t\n\t");
}

#[test]
fn issue_115033_indent_and_append_text() {
    let mut cursor = Cursor::new(json!({ "onEnterRules": [{ "beforeText": ".*", "action": { "indent": "indent", "appendText": "x" } }] }), &["text"]);
    cursor.move_to(1, 5);
    cursor.type_text("\n");
    assert_eq!((cursor.line(1), cursor.line(2), cursor.cursor()), ("text", "    x", (2, 6, 2, 6)));
}

#[test]
fn issue_6862_enter_between_brackets_indents_and_outdents() {
    let mut cursor = Cursor::new(on_enter_language("indentOutdent"), &["function foo (params: string) {}"]);
    cursor.move_to(1, 32);
    cursor.type_text("\n");
    assert_eq!(cursor.editor.document().lines, ["function foo (params: string) {", "    ", "}"]);
}

#[test]
fn enter_honors_increase_indent_pattern() {
    let mut cursor = Cursor::new(indent_rules_language(), &["if (true) {", "\tif (true) {"]).tabs();
    cursor.move_to(1, 12);
    cursor.type_text("\n");
    assert_eq!(cursor.cursor(), (2, 2, 2, 2));
    cursor.move_to(3, 13);
    cursor.type_text("\n");
    assert_eq!(cursor.cursor(), (4, 3, 4, 3));
}

#[test]
fn type_honors_decrease_indent_pattern() {
    let mut cursor = Cursor::new(indent_rules_language(), &["if (true) {", "\t"]);
    cursor.move_to(2, 2);
    cursor.type_text("}");
    assert_eq!((cursor.cursor(), cursor.line(2)), ((2, 2, 2, 2), "}"));
}

#[test]
fn enter_honors_unindented_line_pattern() {
    let mut cursor = Cursor::new(indent_rules_language(), &["if (true) {", "\t\t\treturn true"]).tabs();
    cursor.move_to(2, 15);
    cursor.type_text("\n");
    assert_eq!(cursor.cursor(), (3, 2, 3, 2));
}

#[test]
fn enter_honors_indent_next_line_pattern() {
    let mut cursor = Cursor::new(indent_rules_language(), &["if (true)", "\treturn true;", "if (true)", "\t\t\t\treturn true"]).tabs();
    cursor.move_to(2, 14);
    cursor.type_text("\n");
    assert_eq!(cursor.cursor(), (3, 1, 3, 1));
    cursor.move_to(5, 16);
    cursor.type_text("\n");
    assert_eq!(cursor.cursor(), (6, 2, 6, 2));
}

#[test]
fn enter_honors_indent_next_line_pattern_2() {
    let mut cursor = Cursor::new(indent_rules_language(), &["if (true)", "\tif (true)"]).tabs();
    cursor.move_to(2, 11);
    cursor.type_text("\n");
    assert_eq!(cursor.cursor(), (3, 3, 3, 3));
    cursor.type_text("console.log();");
    cursor.type_text("\n");
    assert_eq!(cursor.cursor(), (4, 1, 4, 1));
}

#[test]
fn enter_honors_intential_indent() {
    let mut cursor = Cursor::new(indent_rules_language(), &["if (true) {", "\tif (true) {", "return true;", "}}"]);
    cursor.move_to(3, 13);
    cursor.type_text("\n");
    assert_eq!((cursor.cursor(), cursor.line(3)), ((4, 1, 4, 1), "return true;"));
}

#[test]
fn enter_supports_selection() {
    let mut first = Cursor::new(indent_rules_language(), &["if (true) {", "\tif (true) {", "\t\treturn true;", "\t}a}"]).tabs();
    first.select((4, 3), (4, 4));
    first.type_text("\n");
    assert_eq!((first.cursor(), first.line(4)), ((5, 1, 5, 1), "\t}"));
    let mut second = Cursor::new(indent_rules_language(), &["if (true) {", "\tif (true) {"]).tabs();
    second.select((2, 12), (2, 13));
    second.type_text("\n");
    assert_eq!(second.cursor(), (3, 3, 3, 3));
    second.type_text("\n");
    assert_eq!(second.cursor(), (4, 3, 4, 3));
}

#[test]
fn enter_honors_tab_size_and_insert_spaces() {
    let mut cursor = Cursor::new(indent_rules_language(), &["if (true) {", "\tif (true) {"]);
    cursor.move_to(1, 12);
    cursor.type_text("\n");
    assert_eq!(cursor.cursor(), (2, 5, 2, 5));
    cursor.move_to(3, 13);
    cursor.type_text("\n");
    assert_eq!(cursor.cursor(), (4, 9, 4, 9));
    for (tabs, first, second) in [(false, (2, 5, 2, 5), (4, 9, 4, 9)), (true, (2, 2, 2, 2), (4, 3, 4, 3))] {
        let mut cursor = Cursor::new(indent_rules_language(), &["if (true) {", "    if (true) {"]);
        if tabs {
            cursor = cursor.tabs();
        }
        cursor.move_to(1, 12);
        cursor.type_text("\n");
        assert_eq!(cursor.cursor(), first);
        cursor.move_to(3, 16);
        cursor.type_text("\n");
        assert_eq!((cursor.line(3), cursor.cursor()), ("    if (true) {", second));
    }
}

#[test]
fn enter_supports_intentional_indentation() {
    let mut cursor = Cursor::new(indent_rules_language(), &["\tif (true) {", "\t\tswitch(true) {", "\t\t\tcase true:", "\t\t\t\tbreak;", "\t\t}", "\t}"]).tabs();
    cursor.move_to(5, 4);
    cursor.type_text("\n");
    assert_eq!((cursor.line(5), cursor.cursor()), ("\t\t}", (6, 3, 6, 3)));
}

#[test]
fn enter_should_not_adjust_cursor_position_when_press_enter_in_the_middle_of_a_line() {
    let tabbed = ["if (true) {", "\tif (true) {", "\t\treturn true;", "\t}a}"];
    let spaced = ["if (true) {", "  if (true) {", "    return true;", "  }a}"];
    for (lines, tabs, column, expected, line) in [(tabbed, true, 9, (4, 3, 4, 3), "\t\t true;"), (tabbed, true, 3, (4, 3, 4, 3), "\t\treturn true;"), (spaced, false, 11, (4, 5, 4, 5), "     true;")] {
        let mut cursor = Cursor::new(indent_rules_language(), &lines);
        if tabs {
            cursor = cursor.tabs();
        }
        cursor.move_to(3, column);
        cursor.type_text("\n");
        assert_eq!((cursor.cursor(), cursor.line(4)), (expected, line));
    }
}

#[test]
fn enter_should_adjust_cursor_position_when_press_enter_in_the_middle_of_leading_whitespaces() {
    let cases = [
        (["if (true) {", "\tif (true) {", "\t\treturn true;", "\t}a}"], 2, (4, 2, 4, 2), "\t\treturn true;"),
        (["\tif (true) {", "\t\tif (true) {", "\t    \treturn true;", "\t\t}a}"], 4, (4, 3, 4, 3), "\t\t\treturn true;"),
    ];
    for (lines, column, expected, line) in cases {
        let mut cursor = Cursor::new(indent_rules_language(), &lines).tabs();
        cursor.move_to(3, column);
        cursor.type_text("\n");
        assert_eq!((cursor.cursor(), cursor.line(4)), (expected, line));
        cursor.move_to(4, 1);
        cursor.type_text("\n");
        assert_eq!((cursor.cursor(), cursor.line(5)), ((5, 1, 5, 1), line));
    }
}

#[test]
fn enter_should_adjust_cursor_position_when_press_enter_in_the_middle_of_leading_whitespaces_3() {
    let mut cursor = Cursor::new(indent_rules_language(), &["if (true) {", "  if (true) {", "    return true;", "}a}"]);
    cursor.move_to(3, 2);
    cursor.type_text("\n");
    assert_eq!((cursor.cursor(), cursor.line(4)), ((4, 2, 4, 2), "    return true;"));
    cursor.move_to(4, 3);
    cursor.type_text("\n");
    assert_eq!((cursor.cursor(), cursor.line(5)), ((5, 3, 5, 3), "    return true;"));
}

#[test]
fn enter_should_adjust_cursor_position_when_press_enter_in_the_middle_of_leading_whitespaces_4() {
    let lines = ["if (true) {", "  if (true) {", "\t  return true;", "}a}", "", "if (true) {", "  if (true) {", "\t  return true;", "}a}"];
    let mut cursor = Cursor::new(indent_rules_language(), &lines).tab_size(2);
    cursor.move_to(3, 3);
    cursor.type_text("\n");
    assert_eq!((cursor.cursor(), cursor.line(4)), ((4, 4, 4, 4), "    return true;"));
    cursor.move_to(9, 4);
    cursor.type_text("\n");
    assert_eq!((cursor.cursor(), cursor.line(10)), ((10, 5, 10, 5), "    return true;"));
}

#[test]
fn enter_should_adjust_cursor_position_when_press_enter_in_the_middle_of_leading_whitespaces_5() {
    let mut cursor = Cursor::new(indent_rules_language(), &["if (true) {", "  if (true) {", "    return true;", "    return true;", ""]).tab_size(2);
    cursor.select((3, 5), (4, 3));
    cursor.type_text("\n");
    assert_eq!((cursor.cursor(), cursor.line(4)), ((4, 3, 4, 3), "    return true;"));
}

#[test]
fn monaco_editor_108_auto_indentation_on_enter_with_selection() {
    for (from, to) in [((3, 8), (2, 12)), ((2, 12), (3, 8))] {
        let mut cursor = Cursor::new(indent_rules_language(), &["function baz() {", "\tvar x = 1;", "\t\t\t\t\t\t\treturn x;", "}"]).tabs();
        cursor.select(from, to);
        cursor.type_text("\n");
        assert_eq!((cursor.line(3), cursor.cursor()), ("\treturn x;", (3, 2, 3, 2)));
    }
}

#[test]
fn on_enter_works_if_there_are_no_indentation_rules() {
    let mut cursor = Cursor::new(json!({}), &["<?", "\tif (true) {", "\t\techo $hi;", "\t\techo $bye;", "\t}", "?>"]).tabs();
    cursor.move_to(5, 3);
    cursor.type_text("\n");
    assert_eq!((cursor.line(6), cursor.cursor(), cursor.line(5)), ("\t", (6, 2, 6, 2), "\t}"));
    let mut cursor = Cursor::new(json!({}), &["\tif (5)", "\t\treturn 5;", "\t"]).tabs();
    cursor.move_to(3, 2);
    cursor.type_text("\n");
    assert_eq!((cursor.cursor(), cursor.line(4)), ((4, 2, 4, 2), "\t"));
}

#[test]
fn bug_16543_tab_should_indent_to_correct_indentation_spot_immediately() {
    let mut cursor = Cursor::new(indent_rules_language(), &["function baz() {", "\tfunction hello() { // something here", "\t", "", "\t}", "}"]).tabs();
    cursor.move_to(4, 1);
    cursor.tab();
    assert_eq!(cursor.line(4), "\t\t");
}

#[test]
fn bug_2938_tab_on_white_space_only_lines_indents_straight_to_the_right_spot() {
    for (whitespace, column, expected) in [("\t", 2, "\t\t\t"), ("    ", 1, "\t\t\t"), ("\t\t\t", 3, "\t\t\t\t"), ("\t\t\t\t", 4, "\t\t\t\t\t")] {
        let lines = ["\tfunction baz() {", "\t\tfunction hello() { // something here", "\t\t", whitespace, "\t\t}", "\t}"];
        let mut cursor = Cursor::new(indent_rules_language(), &lines).tabs();
        cursor.move_to(4, column);
        cursor.tab();
        assert_eq!(cursor.line(4), expected, "{whitespace:?}");
    }
}

#[test]
fn bug_31015_tab_with_enter_rules_indents_straight_to_the_right_spot() {
    let mut cursor = Cursor::new(on_enter_language("indent"), &["    if (a) {", "        ", "", "", "    }"]);
    cursor.move_to(3, 1);
    cursor.tab();
    assert_eq!(cursor.editor.document().lines, ["    if (a) {", "        ", "        ", "", "    }"]);
}

#[test]
fn type_honors_indentation_rules_ruby_keywords() {
    let ruby = json!({ "indentationRules": {
        "increaseIndentPattern": r"^\s*((begin|class|def|else|elsif|ensure|for|if|module|rescue|unless|until|when|while)|(.*\sdo\b))\b[^\{;]*$",
        "decreaseIndentPattern": r"^\s*([}\]]([,)]?\s*(#|$)|\.[a-zA-Z_]\w*\b)|(end|rescue|ensure|else|elsif|when)\b)",
    } });
    let mut cursor = Cursor::new(ruby, &["class Greeter", "  def initialize(name)", "    @name = name", "    en"]);
    cursor.move_to(4, 7);
    cursor.type_text("d");
    assert_eq!(cursor.line(4), "  end");
}

#[test]
fn auto_indent_on_type_increase_indent_pattern_has_higher_priority_than_decrease_indent_when_inheriting() {
    let mut cursor = Cursor::new(indent_rules_language(), &["\tif (true) {", "\t\tconsole.log();", "\t} else if {", "\t\tconsole.log()", "\t}"]);
    cursor.move_to(5, 3);
    cursor.type_text("e");
    assert_eq!((cursor.cursor(), cursor.line(5)), ((5, 4, 5, 4), "\t}e"));
}

#[test]
fn type_honors_users_indentation_adjustment() {
    let mut cursor = Cursor::new(indent_rules_language(), &["\tif (true ||", "\t ) {", "\t}", "if (true ||", ") {", "}"]);
    cursor.move_to(2, 3);
    cursor.type_text(" ");
    assert_eq!((cursor.cursor(), cursor.line(2)), ((2, 4, 2, 4), "\t  ) {"));
}

#[test]
fn bug_29972_if_a_line_is_line_comment_open_bracket_should_not_indent_next_line() {
    let mut cursor = Cursor::new(indent_rules_language(), &["if (true) {", "\t// {", "\t\t"]);
    cursor.move_to(3, 3);
    cursor.type_text("}");
    assert_eq!((cursor.cursor(), cursor.line(3)), ((3, 2, 3, 2), "}"));
}

#[test]
fn issue_38261_tab_key_results_in_bizarre_indentation_in_cpp_mode() {
    let config = json!({ "brackets": [["{", "}"], ["[", "]"], ["(", ")"]], "indentationRules": { "increaseIndentPattern": r"(^.*\{[^}]*$)", "decreaseIndentPattern": r"^\s*\}" } });
    let lines = ["int main() {", "  return 0;", "}", "", "bool Foo::bar(const string &a,", "              const string &b) {", "  foo();", "", ")"];
    let mut cursor = Cursor::new(config, &lines).tab_size(2).strategy(Strategy::Advanced);
    cursor.move_to(8, 1);
    cursor.tab();
    let mut expected = lines;
    expected[7] = "  ";
    assert_eq!((cursor.editor.document().lines.clone(), cursor.cursor()), (expected.map(String::from).to_vec(), (8, 3, 8, 3)));
}

#[test]
fn issue_57197_indent_rules_regex_should_be_stateless() {
    let config = json!({ "indentationRules": {
        "decreaseIndentPattern": { "pattern": r"^\s*}$", "flags": "gm" },
        "increaseIndentPattern": { "pattern": r"^(?![^\S\n]*(?!--|––|——)(?:[-❍❑■⬜□☐▪▫–—≡→›✘xX✔✓☑+]|\[[ xX+-]?\])\s[^\n]*)[^\S\n]*(.+:)[^\S\n]*(?:(?=@[^\s*~(]+(?::\/\/[^\s*~(:]+)?(?:\([^)]*\))?)|$)", "flags": "gm" },
    } });
    let mut cursor = Cursor::new(config, &["Project:"]).tabs();
    cursor.move_to(1, 9);
    cursor.type_text("\n");
    assert_eq!(cursor.cursor(), (2, 2, 2, 2));
    cursor.move_to(1, 9);
    cursor.type_text("\n");
    assert_eq!(cursor.cursor(), (2, 2, 2, 2));
}

#[test]
fn typing_in_json() {
    let config = json!({ "brackets": [["{", "}"], ["[", "]"], ["(", ")"]], "indentationRules": {
        "increaseIndentPattern": r#"({+(?=([^"]*"[^"]*")*[^"}]*$))|(\[+(?=([^"]*"[^"]*")*[^"\]]*$))"#,
        "decreaseIndentPattern": r"^\s*[}\]],?\s*$",
    } });
    let lines = ["{", "  \"scripts: {\"", "    \"watch\": \"a {\"", "    \"build{\": \"b\"", "    \"tasks\": []", "    \"tasks\": [\"a\"]", "  \"}\"", "\"}\""];
    let mut cursor = Cursor::new(config, &lines).tab_size(2);
    cursor.move_to(3, 19);
    cursor.type_text("\n");
    assert_eq!(cursor.line(4), "    ");
    cursor.move_to(5, 18);
    cursor.type_text("\n");
    assert_eq!(cursor.line(6), "    ");
    cursor.move_to(7, 15);
    cursor.type_text("\n");
    assert_eq!((cursor.line(8), cursor.line(9)), ("      ", "    ]"));
    cursor.move_to(10, 18);
    cursor.type_text("\n");
    assert_eq!(cursor.line(11), "    ]");
}

#[test]
fn issue_111128_multicursor_enter_issue_with_indentation() {
    let mut cursor = Cursor::new(indent_rules_language(), &["    let a, b, c;"]).tabs();
    cursor.editor.set_selections(vec![Selection::caret(Position::new(0, 10)), Selection::caret(Position::new(0, 13))]);
    cursor.type_text("\n");
    assert_eq!(cursor.value(), "    let a,\n\t b,\n\t c;");
}

#[test]
fn issue_122714_tab_size_1_prevent_typing_a_string_matching_decrease_indent_pattern_in_an_empty_file() {
    let latex = json!({ "indentationRules": { "increaseIndentPattern": r"\\begin{(?!document)([^}]*)}(?!.*\\end{\1})", "decreaseIndentPattern": r"^\s*\\end{(?!document)" } });
    let mut cursor = Cursor::new(latex, &["\\end"]).tab_size(1);
    cursor.move_to(1, 5);
    cursor.type_text("{");
    // Auto-closing pairs are not ported: upstream's `\end{}` reads `\end{` here.
    assert_eq!(cursor.line(1), "\\end{");
}

/// `testElectricCharacter` in `electricCharLanguage`.
fn electric(lines: &[&str], line: usize, column: usize, ch: &str, expected: &str) {
    let mut cursor = Cursor::new(json!({ "brackets": [["{", "}"], ["[", "]"], ["(", ")"]] }), lines);
    cursor.move_to(line, column);
    cursor.type_text(ch);
    assert_eq!(cursor.line(line), expected, "{lines:?} {ch}");
}

#[test]
fn electric_character_cases() {
    electric(&["  if (a) {", ""], 2, 1, "*", "*");
    electric(&["  if (a) {", ""], 2, 1, "}", "  }");
    electric(&["  if (a) {", "    "], 2, 5, "}", "  }");
    electric(&["  if (a) {", "    if (b) {", "    }", "    "], 4, 1, "}", "  }    ");
    electric(&["  if (a) {", "    if (b) {", "    }", "  }  "], 4, 6, "}", "  }  }");
    electric(&["  if (a) {", "// hello"], 2, 1, "}", "  }// hello");
    electric(&["  if (a) {", "  "], 2, 3, "}", "  }");
    electric(&["  if (a) {", "a"], 2, 2, "}", "a}");
    electric(&["foo(() => {", "  ( 1 + 2 ) ", "})"], 2, 13, "*", "  ( 1 + 2 ) *");
    electric(&["(div"], 1, 5, ")", "(div)");
    electric(&["Math.max(", "\t2", "\t3"], 3, 3, ")", "\t3)");
}

#[test]
fn electric_character_issue_23711_replacing_a_backwards_selection_with_a_bracket_deletes_it() {
    let mut cursor = Cursor::new(json!({ "brackets": [["{", "}"], ["[", "]"], ["(", ")"]] }), &["{", "word"]);
    cursor.select((2, 5), (2, 1));
    cursor.type_text("}");
    assert_eq!(cursor.line(2), "}");
}

/// 'Auto Indent On Type - TypeScript/JavaScript' of indentation.test.ts, on the typescript-basics language
/// configuration and grammar.
mod typescript {
    use super::*;
    use crate::syntax::Tokenization;

    fn cursor(lines: &[&str], strategy: Strategy) -> Cursor {
        let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let languages = Languages::new(&[exthost::scan::scan_extension(&app_root.join("extensions/typescript-basics"), true).unwrap()]);
        let mut editor = editor(&lines.join("\n"), "\n");
        editor.document_mut().tokens = Tokenization::new(languages.grammars.grammar("typescript"));
        Cursor { editor, languages, strategy }
    }

    fn enter_at(lines: &[&str], strategy: Strategy, (line, column): (usize, usize), expected: &[&str]) -> Cursor {
        let mut cursor = cursor(lines, strategy);
        cursor.move_to(line, column);
        cursor.type_text("\n");
        assert_eq!(cursor.editor.document().lines, expected);
        cursor
    }

    #[test]
    fn issue_208215_indent_after_arrow_function() {
        enter_at(&["const add1 = (n) =>"], Strategy::Full, (1, 20), &["const add1 = (n) =>", "    "]);
        enter_at(&["const array = [1, 2, 3, 4, 5];", "array.map(", "    v =>"], Strategy::Full, (3, 9), &["const array = [1, 2, 3, 4, 5];", "array.map(", "    v =>", "        "]);
    }

    #[test]
    fn issue_116843_indent_after_arrow_function() {
        enter_at(&["const add1 = (n) =>", "    n + 1;"], Strategy::Full, (2, 11), &["const add1 = (n) =>", "    n + 1;", ""]);
    }

    #[test]
    fn issue_29755_do_not_add_indentation_on_enter_if_indentation_is_already_valid() {
        let lines = ["function f() {", "    const one = 1;", "    const two = 2;", "}"];
        enter_at(&lines, Strategy::Full, (3, 1), &["function f() {", "    const one = 1;", "", "    const two = 2;", "}"]);
    }

    #[test]
    fn issue_36090() {
        let lines = ["class ItemCtrl {", "    getPropertiesByItemId(id) {", "        return this.fetchItem(id)", "            .then(item => {", "                return this.getPropertiesOfItem(item);", "            });", "    }", "}"];
        let mut expected = lines.to_vec();
        expected.insert(7, "    ");
        let cursor = enter_at(&lines, Strategy::Advanced, (7, 6), &expected);
        assert_eq!(cursor.cursor(), (8, 5, 8, 5));
    }

    #[test]
    fn issue_115304_indent_block_comment_on_enter() {
        let cursor = enter_at(&["/** */", "function f() {}"], Strategy::Advanced, (1, 4), &["/**", " * ", " */", "function f() {}"]);
        assert_eq!(cursor.cursor(), (2, 4, 2, 4));
    }

    #[test]
    fn issue_43244_indent_when_lambda_arrow_function_is_detected_outdent_when_end_is_reached() {
        enter_at(&["const array = [1, 2, 3, 4, 5];", "array.map(_)"], Strategy::Full, (2, 12), &["const array = [1, 2, 3, 4, 5];", "array.map(_", "    ", ")"]);
    }

    #[test]
    fn issue_43244_incorrect_indentation_after_if_for_while_without_braces() {
        let mut cursor = enter_at(&["function f() {", "    if (condition)", "}"], Strategy::Full, (2, 19), &["function f() {", "    if (condition)", "        ", "}"]);
        // `viewModel.type` without the 'keyboard' source: no interceptors.
        cursor.editor.type_text("return;");
        cursor.type_text("\n");
        assert_eq!(cursor.editor.document().lines, ["function f() {", "    if (condition)", "        return;", "    ", "}"]);
    }

    #[test]
    fn issue_208232_incorrect_indentation_inside_of_comments() {
        enter_at(&["/**", "indentation done for {", "*/"], Strategy::Full, (2, 23), &["/**", "indentation done for {", "", "*/"]);
    }

    #[test]
    fn issue_209802_allman_style_braces_in_javascript() {
        enter_at(&["if (/*condition*/)"], Strategy::Full, (1, 19), &["if (/*condition*/)", "    "]);
    }

    /// shiftCommand.test.ts `testShiftCommand` / `testUnshiftCommand`: tabs, tab size 4, the JavaScript onEnter rules.
    fn shift(lines: &[&str], unshift: bool, selection: ((usize, usize), (usize, usize)), expected: &[&str], expected_selection: (usize, usize, usize, usize)) {
        let mut cursor = cursor(lines, Strategy::Full).tabs();
        cursor.select(selection.0, selection.1);
        let auto = AutoIndent { languages: &cursor.languages, strategy: cursor.strategy };
        match unshift {
            true => cursor.editor.outdent(auto),
            false => cursor.editor.indent_lines(auto),
        };
        assert_eq!((cursor.editor.document().lines.clone(), cursor.cursor()), (expected.iter().map(|line| line.to_string()).collect(), expected_selection));
    }

    #[test]
    fn issue_348_indenting_around_doc_block_comments() {
        let plain = ["", "/**", " * a doc comment", " */", "function hello() {}"];
        let shifted = ["", "\t/**", "\t * a doc comment", "\t */", "\tfunction hello() {}"];
        shift(&plain, false, ((1, 1), (5, 20)), &shifted, (1, 1, 5, 21));
        shift(&plain, true, ((1, 1), (5, 20)), &plain, (1, 1, 5, 20));
        let mut indented = shifted;
        indented[0] = "\t";
        shift(&indented, true, ((1, 1), (5, 21)), &plain, (1, 1, 5, 20));
    }

    #[test]
    fn issue_1609_wrong_indentation_of_block_comments() {
        let lines = ["", "/**", " * test", " *", " * @type {number}", " */", "var foo = 0;"];
        let expected = ["", "\t/**", "\t * test", "\t *", "\t * @type {number}", "\t */", "\tvar foo = 0;"];
        shift(&lines, false, ((1, 1), (7, 13)), &expected, (1, 1, 7, 14));
    }
}
