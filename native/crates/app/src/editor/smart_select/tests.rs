//! smartSelect.test.ts. Ranges are upstream's 1-based (line, column, line, column).

use serde_json::json;

use super::*;
use crate::editor::edit::tests::editor;

type Expected = (usize, usize, usize, usize);

const OPTIONS: Options = Options { select_leading_and_trailing_whitespace: true, select_subwords: true };

/// The suite's `mockJSMode`: `()`, `{}` and `[]`, and a word pattern where `-` and `|` are word chars.
fn mock_js(text: &str) -> (OpenEditor, Languages) {
    let mut languages = Languages::new(&[]);
    languages.configure("mockJSMode", &json!({ "brackets": [["(", ")"], ["{", "}"], ["[", "]"]], "wordPattern": r#"(-?\d*\.\d\w*)|([^`\~\!\@\#\$\%\^\&\*\(\)\=\+\[\{\]\}\\\;\:\'\"\,\.\<\>\/\?\s]+)"# }));
    let mut editor = editor(text, "\n");
    editor.document_mut().language_id = "mockJSMode".into();
    (editor, languages)
}

fn spans(expected: &[Expected]) -> Vec<Span> {
    expected.iter().map(|&(start_line, start_column, end_line, end_column)| (Position::new(start_line - 1, start_column), Position::new(end_line - 1, end_column))).collect()
}

fn word_pattern<'a>(editor: &OpenEditor, languages: &'a Languages) -> Option<&'a onig::Regex> {
    languages.rules(&editor.document().language_id).unwrap().word_pattern.as_ref()
}

fn brackets_of(editor: &OpenEditor, languages: &Languages) -> Vec<PairBracket> {
    editor.document().tokens.pairs(languages, &editor.document().lines, &editor.document().language_id).brackets
}

/// `assertGetRangesToPosition`: `expected` lists the outermost range first.
fn assert_ranges_to_position(text: &[&str], line: usize, column: usize, expected: &[Expected], select_leading_and_trailing_whitespace: bool) {
    let (editor, languages) = mock_js(&text.join("\n"));
    let brackets = brackets_of(&editor, &languages);
    let options = Options { select_leading_and_trailing_whitespace, select_subwords: true };
    let actual = provide_selection_ranges(&editor.document().lines, &char_brackets(&editor.document().lines, &brackets), word_pattern(&editor, &languages), Position::new(line - 1, column), options);
    let mut expected = spans(expected);
    expected.reverse();
    assert_eq!(actual, expected);
}

enum Provider {
    Bracket,
    Word { select_subwords: bool },
}

/// `assertRanges`: one provider's ranges at the `|` of `value`.
fn assert_ranges(provider: Provider, value: &str, expected: &[Expected]) {
    let index = value.find('|').unwrap();
    let before = &value[..index];
    let line = before.matches('\n').count();
    let column = before.chars().count() - before.rfind('\n').map_or(0, |newline| before[..=newline].chars().count()) + 1;
    let (editor, languages) = mock_js(&value.replacen('|', "", 1));
    let lines = &editor.document().lines;
    let position = Position::new(line, column);
    let actual = match provider {
        Provider::Bracket => bracket_ranges(lines, &char_brackets(lines, &brackets_of(&editor, &languages)), position),
        Provider::Word { select_subwords } => word_ranges(lines, word_pattern(&editor, &languages), position, select_subwords),
    };
    assert_eq!(actual, spans(expected), "{value}");
}

#[test]
fn get_ranges_to_position_1() {
    let text = ["function a(bar, foo){", "\tif (bar) {", "\t\treturn (bar + (2 * foo))", "\t}", "}"];
    let expected = [(1, 1, 5, 2), (1, 21, 5, 2), (1, 22, 5, 1), (2, 1, 4, 3), (2, 1, 4, 3), (2, 2, 4, 3), (2, 11, 4, 3), (2, 12, 4, 2), (3, 1, 3, 27), (3, 3, 3, 27), (3, 10, 3, 27), (3, 11, 3, 26), (3, 17, 3, 26), (3, 18, 3, 25)];
    assert_ranges_to_position(&text, 3, 20, &expected, true);
}

#[test]
fn config_select_leading_and_trailing_whitespace() {
    assert_ranges_to_position(&["aaa", "\tbbb", ""], 2, 3, &[(1, 1, 3, 1), (2, 1, 2, 5), (2, 2, 2, 5)], true);
    assert_ranges_to_position(&["aaa", "\tbbb", ""], 2, 3, &[(1, 1, 3, 1), (2, 2, 2, 5)], false);
}

#[test]
fn issue_56886_skip_empty_lines_correctly() {
    let text = ["function a(bar, foo){", "\tif (bar) {", "", "\t}", "}"];
    assert_ranges_to_position(&text, 3, 1, &[(1, 1, 5, 2), (1, 21, 5, 2), (1, 22, 5, 1), (2, 1, 4, 3), (2, 1, 4, 3), (2, 2, 4, 3), (2, 11, 4, 3), (2, 12, 4, 2)], true);
}

#[test]
fn issue_56886_do_not_skip_lines_with_only_whitespaces() {
    let text = ["function a(bar, foo){", "\tif (bar) {", " ", "\t}", "}"];
    let expected = [(1, 1, 5, 2), (1, 21, 5, 2), (1, 22, 5, 1), (2, 1, 4, 3), (2, 1, 4, 3), (2, 2, 4, 3), (2, 11, 4, 3), (2, 12, 4, 2), (3, 1, 3, 2), (3, 1, 3, 2)];
    assert_ranges_to_position(&text, 3, 1, &expected, true);
}

#[test]
fn issue_40658_cursor_at_first_position_inside_brackets_selects_the_line_inside() {
    assert_ranges_to_position(&[" [ ]", " { } ", "( ) "], 2, 3, &[(1, 1, 3, 5), (2, 1, 2, 6), (2, 2, 2, 5), (2, 3, 2, 4)], true);
}

#[test]
fn issue_40658_cursor_in_empty_brackets_reveals_brackets_first() {
    assert_ranges_to_position(&[" [] ", " { } ", "  ( ) "], 1, 3, &[(1, 1, 3, 7), (1, 1, 1, 5), (1, 2, 1, 4), (1, 3, 1, 3)], true);
}

#[test]
fn issue_40658_tokens_before_bracket_are_revealed_first() {
    assert_ranges_to_position(&["  [] ", " { } ", "selectthis( ) "], 3, 11, &[(1, 1, 3, 15), (3, 1, 3, 15), (3, 1, 3, 14), (3, 1, 3, 11)], true);
}

#[test]
fn bracket_selection() {
    assert_ranges(Provider::Bracket, "(|)", &[(1, 2, 1, 2), (1, 1, 1, 3)]);
    assert_ranges(Provider::Bracket, "[[[](|)]]", &[(1, 6, 1, 6), (1, 5, 1, 7), (1, 3, 1, 7), (1, 2, 1, 8), (1, 2, 1, 8), (1, 1, 1, 9)]);
    assert_ranges(Provider::Bracket, "[a[](|)a]", &[(1, 6, 1, 6), (1, 5, 1, 7), (1, 2, 1, 8), (1, 1, 1, 9)]);
    assert_ranges(Provider::Bracket, "fofof|fofo", &[]);
    assert_ranges(Provider::Bracket, "[[[]()]]|", &[]);
    assert_ranges(Provider::Bracket, "|[[[]()]]", &[]);
    assert_ranges(Provider::Bracket, "[|[[]()]]", &[(1, 2, 1, 8), (1, 1, 1, 9)]);
    assert_ranges(Provider::Bracket, "[[[]()]|]", &[(1, 2, 1, 8), (1, 1, 1, 9)]);
    assert_ranges(Provider::Bracket, "aaa(aaa)bbb(b|b)ccc(ccc)", &[(1, 13, 1, 15), (1, 12, 1, 16)]);
    assert_ranges(Provider::Bracket, "(aaa(aaa)bbb(b|b)ccc(ccc))", &[(1, 14, 1, 16), (1, 13, 1, 17), (1, 2, 1, 25), (1, 1, 1, 26)]);
}

#[test]
fn bracket_with_leading_and_trailing() {
    assert_ranges(Provider::Bracket, "for(a of b){\n  foo(|);\n}", &[(2, 7, 2, 7), (2, 6, 2, 8), (1, 13, 3, 1), (1, 12, 3, 2), (1, 1, 3, 2), (1, 1, 3, 2)]);
    assert_ranges(Provider::Bracket, "for(a of b)\n{\n  foo(|);\n}", &[(3, 7, 3, 7), (3, 6, 3, 8), (2, 2, 4, 1), (2, 1, 4, 2), (1, 1, 4, 2), (1, 1, 4, 2)]);
}

#[test]
fn in_word_ranges() {
    let subwords = || Provider::Word { select_subwords: true };
    assert_ranges(subwords(), "f|ooBar", &[(1, 1, 1, 4), (1, 1, 1, 7), (1, 1, 1, 7)]);
    assert_ranges(subwords(), "f|oo_Ba", &[(1, 1, 1, 4), (1, 1, 1, 7), (1, 1, 1, 7)]);
    assert_ranges(subwords(), "f|oo-Ba", &[(1, 1, 1, 4), (1, 1, 1, 7), (1, 1, 1, 7)]);
}

#[test]
fn in_word_ranges_with_select_subwords_false() {
    let words = || Provider::Word { select_subwords: false };
    assert_ranges(words(), "f|ooBar", &[(1, 1, 1, 7), (1, 1, 1, 7)]);
    assert_ranges(words(), "f|oo_Ba", &[(1, 1, 1, 7), (1, 1, 1, 7)]);
    assert_ranges(words(), "f|oo-Ba", &[(1, 1, 1, 7), (1, 1, 1, 7)]);
}

#[test]
fn issue_67493_default_selection_selects_the_current_word_or_hump_first() {
    let subwords = || Provider::Word { select_subwords: true };
    assert_ranges(subwords(), "Abs|tractSmartSelect", &[(1, 1, 1, 9), (1, 1, 1, 20), (1, 1, 1, 20)]);
    assert_ranges(subwords(), "AbstractSma|rtSelect", &[(1, 9, 1, 14), (1, 1, 1, 20), (1, 1, 1, 20)]);
    assert_ranges(subwords(), "Abstrac-Sma|rt-elect", &[(1, 9, 1, 14), (1, 1, 1, 20), (1, 1, 1, 20)]);
    assert_ranges(subwords(), "Abstrac_Sma|rt_elect", &[(1, 9, 1, 14), (1, 1, 1, 20), (1, 1, 1, 20)]);
    assert_ranges(subwords(), "Abstrac_Sma|rt-elect", &[(1, 9, 1, 14), (1, 1, 1, 20), (1, 1, 1, 20)]);
    assert_ranges(subwords(), "Abstrac_Sma|rtSelect", &[(1, 9, 1, 14), (1, 1, 1, 20), (1, 1, 1, 20)]);
}

/// Upstream registers a provider with these ranges; without extension providers the step after the
/// providers is tested on its own.
#[test]
fn issue_73850_only_add_line_ranges_if_they_are_contained_by_the_next_range() {
    let lines: Vec<String> = ["type T = {", "\tx: number", "}"].map(String::from).to_vec();
    let ranges = spans(&[(1, 10, 1, 11), (1, 10, 3, 2), (1, 1, 3, 2)]);
    assert_eq!(with_whitespace_ranges(&lines, &ranges), ranges);
}

#[test]
fn issue_90589_expand_selection_in_words_with_underscores() {
    let subwords = || Provider::Word { select_subwords: true };
    assert_ranges(subwords(), "Hel|lo_World", &[(1, 1, 1, 6), (1, 1, 1, 12), (1, 1, 1, 12)]);
    assert_ranges(subwords(), "Hello_Wo|rld", &[(1, 7, 1, 12), (1, 1, 1, 12), (1, 1, 1, 12)]);
    assert_ranges(subwords(), "Hello|_World", &[(1, 1, 1, 6), (1, 1, 1, 12), (1, 1, 1, 12)]);
    assert_ranges(subwords(), "Hello_|World", &[(1, 7, 1, 12), (1, 1, 1, 12), (1, 1, 1, 12)]);
    assert_ranges(subwords(), "Hello|-World", &[(1, 1, 1, 6), (1, 1, 1, 12), (1, 1, 1, 12)]);
    assert_ranges(subwords(), "Hello-|World", &[(1, 7, 1, 12), (1, 1, 1, 12), (1, 1, 1, 12)]);
    assert_ranges(subwords(), "Hello|World", &[(1, 6, 1, 11), (1, 1, 1, 11), (1, 1, 1, 11)]);
}

/// `SmartSelectController.run`: expand steps out past equal ranges, shrink steps back, a cursor move starts over.
#[test]
fn expand_and_shrink_walk_the_ranges() {
    let (mut editor, languages) = mock_js("foo(barBaz)");
    editor.set_selections(vec![Selection::caret(Position::new(0, 5))]);
    let selected = |editor: &OpenEditor| (editor.selection().anchor.column, editor.selection().head.column);
    let mut expected = vec![(4, 7), (4, 10), (3, 11), (0, 11)];
    for &step in &expected {
        editor.smart_select(true, &languages, OPTIONS);
        assert_eq!(selected(&editor), step);
    }
    editor.smart_select(true, &languages, OPTIONS);
    assert_eq!(selected(&editor), (0, 11), "the last range stays");
    expected.pop();
    for &step in expected.iter().rev() {
        editor.smart_select(false, &languages, OPTIONS);
        assert_eq!(selected(&editor), step);
    }
    editor.smart_select(false, &languages, OPTIONS);
    assert_eq!(selected(&editor), (5, 5), "back to the cursor");
    editor.set_selections(vec![Selection::caret(Position::new(0, 1))]);
    editor.smart_select(true, &languages, OPTIONS);
    assert_eq!(selected(&editor), (0, 3), "a moved cursor starts over");
}
