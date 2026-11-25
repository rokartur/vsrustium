use std::path::PathBuf;

use super::{explorer_label, type_navigation_target};
use crate::explorer::Row;

fn labels(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

/// listWidget.ts `TypeNavigationController.onInput`: a new word starts after the focused row and wraps, a longer
/// word keeps matching from the focused row, case is ignored.
#[test]
fn typed_words_focus_the_next_row_with_that_prefix() {
    let list = labels(&["alpha", "beta", "Bravo", "charlie"]);
    assert_eq!(type_navigation_target(&list, None, "b", true), Some(1));
    assert_eq!(type_navigation_target(&list, Some(1), "b", true), Some(2), "the same letter again moves on");
    assert_eq!(type_navigation_target(&list, Some(1), "br", false), Some(2), "a longer word keeps from the focused row");
    assert_eq!(type_navigation_target(&list, Some(1), "be", false), Some(1));
    assert_eq!(type_navigation_target(&list, Some(3), "a", true), Some(0), "wraps");
    assert_eq!(type_navigation_target(&list, Some(0), "z", true), None);
}

/// Compact Explorer rows match their joined folders.
#[test]
fn typed_words_match_a_compact_rows_folders() {
    let compressed = vec![(PathBuf::from("a"), "a".into()), (PathBuf::from("a/b"), "b".into()), (PathBuf::from("a/b/c"), "c".into())];
    let row = Row { path: PathBuf::from("a/b/c"), name: "c".into(), depth: 1, is_dir: true, has_nests: false, expanded: false, is_new: false, compressed };
    assert_eq!(type_navigation_target(&[explorer_label(&row)], None, "a/b", true), Some(0));
}
