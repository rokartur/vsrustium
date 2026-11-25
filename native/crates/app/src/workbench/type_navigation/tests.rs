use std::path::PathBuf;

use super::type_navigation_target;
use crate::explorer::Row;

fn rows(names: &[&str]) -> Vec<Row> {
    names
        .iter()
        .map(|name| Row { path: PathBuf::from(name), name: name.to_string(), depth: 1, is_dir: false, has_nests: false, expanded: false, is_new: false, compressed: Vec::new() })
        .collect()
}

/// listWidget.ts `TypeNavigationController.onInput`: a new word starts after the focused row and wraps, a longer
/// word keeps matching from the focused row, case is ignored, compact rows match their joined folders.
#[test]
fn typed_words_focus_the_next_row_with_that_prefix() {
    let list = rows(&["alpha", "beta", "Bravo", "charlie"]);
    assert_eq!(type_navigation_target(&list, None, "b", true), Some(1));
    assert_eq!(type_navigation_target(&list, Some(1), "b", true), Some(2), "the same letter again moves on");
    assert_eq!(type_navigation_target(&list, Some(1), "br", false), Some(2), "a longer word keeps from the focused row");
    assert_eq!(type_navigation_target(&list, Some(1), "be", false), Some(1));
    assert_eq!(type_navigation_target(&list, Some(3), "a", true), Some(0), "wraps");
    assert_eq!(type_navigation_target(&list, Some(0), "z", true), None);
    let mut compact = rows(&["c"]);
    compact[0].compressed = vec![(PathBuf::from("a"), "a".into()), (PathBuf::from("a/b"), "b".into()), (PathBuf::from("a/b/c"), "c".into())];
    assert_eq!(type_navigation_target(&compact, None, "a/b", true), Some(0));
}
