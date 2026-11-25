use super::*;

fn options() -> UnicodeHighlighterOptions {
    UnicodeHighlighterOptions { ambiguous_characters: true, invisible_characters: true, ..Default::default() }
}

fn ranges(result: &UnicodeHighlights) -> Vec<String> {
    result.ranges.iter().map(|(start, end)| format!("[{},{} -> {},{}]", start.line + 1, start.column + 1, end.line + 1, end.column + 1)).collect()
}

// unicodeTextModelHighlighter.test.ts
#[test]
fn compute_unicode_highlights_168068() {
    let lines = ["", "\tFor\u{a0}å\u{a0}gi\u{a0}et\u{a0}eksempel", ""].map(String::from);
    let result = compute_unicode_highlights(&lines, 0, &options());
    assert_eq!(ranges(&result), ["[2,5 -> 2,6]", "[2,7 -> 2,8]", "[2,10 -> 2,11]", "[2,13 -> 2,14]"]);
    assert_eq!((result.ambiguous_character_count, result.invisible_character_count, result.non_basic_ascii_character_count, result.has_more), (0, 4, 0, false));
}

#[test]
fn ambiguous_characters_depend_on_the_word_and_the_locales() {
    // Cyrillic а in an ASCII word looks like a.
    let result = compute_unicode_highlights(&["let vаlue = 1;".into()], 0, &options());
    assert_eq!((ranges(&result), result.ambiguous_character_count), (vec!["[1,6 -> 1,7]".to_string()], 1));
    // A word of Cyrillic letters only is not mixed with ASCII.
    assert!(compute_unicode_highlights(&["привет мир".into()], 0, &options()).ranges.is_empty());
    // Russian makes the Cyrillic look-alikes common.
    let russian = UnicodeHighlighterOptions { allowed_locales: vec!["ru".into()], ..options() };
    assert!(compute_unicode_highlights(&["let vаlue = 1;".into()], 0, &russian).ranges.is_empty());
    let allowed = UnicodeHighlighterOptions { allowed_code_points: vec!['а'], ..options() };
    assert!(compute_unicode_highlights(&["let vаlue = 1;".into()], 0, &allowed).ranges.is_empty());
}

#[test]
fn non_basic_ascii_marks_every_other_character_and_stops_at_the_cap() {
    let all = UnicodeHighlighterOptions { non_basic_ascii: true, ..options() };
    let result = compute_unicode_highlights(&["é\tx\u{200b}".into()], 0, &all);
    assert_eq!((ranges(&result), result.non_basic_ascii_character_count), (vec!["[1,1 -> 1,2]".to_string(), "[1,4 -> 1,5]".to_string()], 2));
    let result = compute_unicode_highlights(&["\u{200b}".repeat(1001)], 0, &options());
    assert_eq!((result.ranges.len(), result.invisible_character_count, result.has_more), (1000, 1001, true));
}
