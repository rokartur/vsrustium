use super::*;

fn link(start: (usize, usize), end: (usize, usize)) -> Link {
    Link { handle: 1, dto: json!({}), start, end }
}

fn ranges(links: &[Link]) -> Vec<((usize, usize), (usize, usize))> {
    links.iter().map(|link| (link.start, link.end)).collect()
}

#[test]
fn union_drops_the_earlier_lists_links_touching_new_ones() {
    let old = vec![link((1, 1), (1, 5)), link((3, 1), (3, 4))];
    let new = vec![link((1, 5), (1, 9)), link((2, 1), (2, 3)), link((4, 1), (4, 2))];
    assert_eq!(ranges(&union(old, new)), [((1, 5), (1, 9)), ((2, 1), (2, 3)), ((3, 1), (3, 4)), ((4, 1), (4, 2))]);
}

#[test]
fn links_never_grow_when_typing_at_their_edges() {
    // Typing "x" at the link's start moves it right, at its end leaves it.
    assert_eq!(tracked((1, 5), (1, 5), (1, 5), (1, 6), false), (1, 6));
    assert_eq!(tracked((1, 9), (1, 9), (1, 9), (1, 10), true), (1, 9));
    // A line break before the link moves it down.
    assert_eq!(tracked((1, 9), (1, 2), (1, 2), (2, 1), false), (2, 8));
    assert_eq!(end_after((1, 2), "a\nbc"), (2, 3));
}

#[test]
fn the_hover_message_names_the_gesture() {
    assert_eq!(hover_message(&json!({ "url": "https://a.b/c d" }), "cmd + click"), json!({ "value": "[Follow link](https://a.b/c%20d) (cmd + click)", "isTrusted": true }));
    assert_eq!(hover_message(&json!({ "url": "command:foo.bar?1" }), "cmd + click"), json!({ "value": "[Execute command](command:foo.bar?1 \"Execute command foo.bar\") (cmd + click)", "isTrusted": true }));
    assert_eq!(hover_message(&json!({ "tooltip": "Open [x]" }), "option + click"), json!({ "value": "Open&nbsp;\\[x\\]&nbsp;\\(option&nbsp;\\+&nbsp;click\\)" }));
}

#[test]
fn the_worker_finds_urls_with_utf16_columns() {
    let lines = vec!["é see https://x.y/z now".to_string()];
    assert_eq!(worker_links(&lines), json!({ "links": [{ "range": { "startLineNumber": 1, "startColumn": 7, "endLineNumber": 1, "endColumn": 20 }, "url": "https://x.y/z" }] }));
}
