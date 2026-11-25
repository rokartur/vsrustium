use super::*;

#[test]
fn labels_are_cut_at_the_maximum_length_per_line() {
    let mut total = 0;
    let first = rendered_parts(&json!({ "label": "abcdef" }), &mut total, 8, None);
    assert_eq!(first[0].text, "abcdef");
    let second = rendered_parts(&json!({ "label": [{ "label": "gh" }, { "label": "ij" }] }), &mut total, 8, None);
    // The running total passes 8 in the second part: it loses the excess and ends in an ellipsis, the rest goes.
    assert_eq!(second.iter().map(|part| (part.text.as_str(), part.index)).collect::<Vec<_>>(), [("gh", 0), ("\u{2026}", 1)]);
}

#[test]
fn spaces_become_no_break_spaces() {
    let parts = rendered_parts(&json!({ "label": "a b\tc" }), &mut 0, 0, None);
    assert_eq!(parts[0].text, "a\u{a0}b\u{a0}c");
}

#[test]
fn a_fixed_length_cuts_or_pads_the_label() {
    let cut = rendered_parts(&json!({ "label": "string" }), &mut 0, 0, Some(3));
    assert_eq!(cut[0].text, "st\u{2026}");
    let padded = rendered_parts(&json!({ "label": "x" }), &mut 0, 0, Some(3));
    assert_eq!(padded.iter().map(|part| part.text.as_str()).collect::<String>(), "x\u{200a}\u{200a}");
}

#[test]
fn a_hint_after_a_word_attaches_after_it() {
    let lines = vec!["let value = f(arg);".to_owned()];
    let type_hint = hint_item(&lines, 1, json!({ "label": ": number", "position": { "lineNumber": 1, "column": 10 } }));
    assert_eq!((type_hint.position, type_hint.after), (Position::new(0, 9), true));
    let parameter = hint_item(&lines, 1, json!({ "label": "x:", "position": { "lineNumber": 1, "column": 15 } }));
    assert_eq!((parameter.position, parameter.after), (Position::new(0, 14), false));
}
