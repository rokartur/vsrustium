//! history.test.ts, `History Navigator`.

use super::HistoryNavigator;

fn navigator(elements: &[&str], limit: usize) -> HistoryNavigator {
    HistoryNavigator::new(elements.iter().map(|element| element.to_string()).collect(), limit)
}

fn to_array(navigator: &mut HistoryNavigator) -> Vec<String> {
    let mut result = Vec::new();
    navigator.first();
    if navigator.current().is_some() {
        loop {
            result.push(navigator.current().unwrap());
            if navigator.next().is_none() {
                break;
            }
        }
    }
    result
}

#[test]
fn create_reduces_the_input_to_limit() {
    assert_eq!(to_array(&mut navigator(&["1", "2", "3", "4"], 2)), ["3", "4"]);
}

#[test]
fn create_sets_the_position_after_last() {
    let mut test = navigator(&["1", "2", "3", "4"], 100);
    assert_eq!(test.current(), None);
    assert!(test.is_nowhere() && !test.is_first() && !test.is_last());
    assert_eq!(test.next(), None);
    assert_eq!(test.previous().as_deref(), Some("4"));
    assert!(!test.is_nowhere() && !test.is_first() && test.is_last());
}

#[test]
fn first_last_next_and_previous() {
    let mut test = navigator(&["1", "2", "3", "4"], 3);
    assert_eq!(test.first().as_deref(), Some("2"));
    assert!(test.is_first() && !test.is_last());
    assert_eq!(test.next().as_deref(), Some("3"));
    assert_eq!(test.next().as_deref(), Some("4"));
    assert_eq!(test.next(), None);
    assert!(!test.is_last(), "stepping past the last element is no longer last");

    let mut test = navigator(&["1", "2", "3", "4"], 3);
    assert_eq!(test.previous().as_deref(), Some("4"));
    assert_eq!(test.previous().as_deref(), Some("3"));
    assert_eq!(test.previous().as_deref(), Some("2"));
    assert_eq!(test.previous(), None);
    assert!(test.is_first(), "previous on the first element stays on it");
}

#[test]
fn add_reduces_moves_and_resets() {
    let mut test = navigator(&["1", "2", "3", "4"], 2);
    test.add("5");
    assert_eq!(to_array(&mut test), ["4", "5"]);

    let mut test = navigator(&["1", "2", "3", "4"], 5);
    test.add("2");
    assert_eq!(to_array(&mut test), ["1", "3", "4", "2"]);

    let mut test = navigator(&["1", "2", "3", "4"], 3);
    test.first();
    test.add("5");
    assert_eq!(test.previous().as_deref(), Some("5"));
    assert!(test.is_last());
    assert_eq!(test.next(), None);
    assert!(!test.is_last());
}

#[test]
fn clear() {
    let mut test = navigator(&["a", "b", "c"], 10);
    assert_eq!(test.previous().as_deref(), Some("c"));
    test.clear();
    assert_eq!(test.current(), None);
    assert!(test.is_nowhere());
}

#[test]
fn up_and_down_walk_the_history_like_the_input_box() {
    let mut test = navigator(&["a", "b"], 100);
    assert_eq!(test.show_previous_value("typed").as_deref(), Some("b"), "the typed value joins the history first");
    assert_eq!(test.elements(), ["a", "b", "typed"]);
    assert_eq!(test.show_previous_value("b").as_deref(), Some("a"));
    assert_eq!(test.show_previous_value("a").as_deref(), Some("a"), "the first element stays");
    assert_eq!(test.show_next_value("a"), "b");
    assert_eq!(test.show_next_value("b"), "typed");
    assert_eq!(test.show_next_value("typed"), "");
}
