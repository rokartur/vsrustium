use super::{Accepted, ActionList, Item, Options};

fn action(id: &str) -> Item<String> {
    Item { id: Some(id.to_owned()), ..Item::action(id.to_owned(), id) }
}

fn disabled(id: &str) -> Item<String> {
    Item { disabled: true, ..action(id) }
}

fn separator(label: Option<&str>) -> Item<String> {
    Item::separator(label)
}

fn in_section(item: Item<String>, section: &str) -> Item<String> {
    Item { section: Some(section.to_owned()), ..item }
}

fn section_toggle(section: &str) -> Item<String> {
    Item { is_section_toggle: true, ..in_section(action(&format!("toggle-{section}")), section) }
}

fn list(items: Vec<Item<String>>) -> ActionList<String> {
    ActionList::new(items, Options::default())
}

fn with_filter(items: Vec<Item<String>>) -> ActionList<String> {
    ActionList::new(items, Options { show_filter: true, ..Options::default() })
}

fn rows(list: &ActionList<String>) -> Vec<&str> {
    list.visible.iter().map(|&index| list.all[index].label.as_deref().unwrap_or("")).collect()
}

fn focused(list: &ActionList<String>) -> Option<&str> {
    list.focused_item().and_then(|item| item.id.as_deref())
}

/// actionList.test.ts 'arrow navigation skips disabled rows and separators', on the list itself.
#[test]
fn arrow_navigation_skips_disabled_rows_and_separators() {
    let mut list = list(vec![action("alpha"), separator(Some("Models")), disabled("beta"), action("gamma")]);
    list.focus();
    let mut states = Vec::new();
    for down in [true, false, false, true] {
        if down { list.focus_next() } else { list.focus_previous() }
        states.push(focused(&list).unwrap().to_owned());
    }
    assert_eq!(states, ["gamma", "alpha", "gamma", "alpha"]);
}

/// The filter sits before the first row: wrapping past either end lands in it.
#[test]
fn wrapping_moves_through_the_filter() {
    let mut list = with_filter(vec![action("first"), action("second")]);
    list.focus();
    assert_eq!((focused(&list), list.filter_focused), (Some("first"), false));
    list.focus_previous();
    assert_eq!((focused(&list), list.filter_focused), (None, true));
    list.focus_previous();
    assert_eq!((focused(&list), list.filter_focused), (Some("second"), false));
    list.focus_next();
    assert_eq!((focused(&list), list.filter_focused), (None, true));
    list.focus_next();
    assert_eq!((focused(&list), list.filter_focused), (Some("first"), false));
}

/// 'typing can refine the query after navigation and Enter accepts the new result'.
#[test]
fn typing_refines_the_query_and_enter_accepts_the_new_result() {
    let mut list = ActionList::new(vec![action("first"), action("second"), action("third")], Options { show_filter: true, focus_filter_on_open: true, ..Options::default() });
    list.focus();
    list.focus_next();
    list.filter_focused = true;
    list.set_filter("thi".into());
    assert_eq!(rows(&list), ["third"]);
    assert_eq!(list.accept_selected(false, false), Accepted::Select { item: "third".into(), preview: false });
}

/// 'empty and disabled results cannot accept a stale selection'.
#[test]
fn empty_and_disabled_results_cannot_accept_a_stale_selection() {
    let mut list = ActionList::new(vec![action("first"), disabled("disabled")], Options { show_filter: true, focus_filter_on_open: true, ..Options::default() });
    list.focus();
    for query in ["missing", "disabled"] {
        list.set_filter(query.into());
        list.focus_next();
        list.focus_previous();
        assert_eq!(list.accept_selected(false, false), Accepted::Nothing);
        assert_eq!(focused(&list), None);
    }
}

/// 'keeps titled separator above first filtered match'.
#[test]
fn keeps_the_titled_separator_above_the_first_filtered_match() {
    let mut list = list(vec![separator(Some("Provider A")), action("alpha"), separator(Some("Provider B")), action("beta")]);
    list.set_filter("alpha".into());
    assert_eq!(rows(&list), ["Provider A", "alpha"]);
}

/// 'keeps only titled separators for sections with filtered matches'.
#[test]
fn keeps_only_titled_separators_for_sections_with_filtered_matches() {
    let mut list = list(vec![
        separator(Some("Provider A")),
        action("alpha"),
        separator(Some("Provider B")),
        action("beta"),
        separator(Some("Provider C")),
        action("gamma"),
    ]);
    list.set_filter("beta".into());
    assert_eq!(rows(&list), ["Provider B", "beta"]);
}

/// 'matches nonvisual filter text without rendering it'.
#[test]
fn matches_nonvisual_filter_text() {
    let mut list = list(vec![
        Item { filter_text: Some("owner-one/shared".into()), ..action("shared") },
        Item { filter_text: Some("owner-two/shared".into()), ..action("shared") },
    ]);
    list.set_filter("owner-one/shared".into());
    assert_eq!(rows(&list), ["shared"]);
}

/// Unlabeled leading separators and separators with no action after them go, unfiltered too.
#[test]
fn drops_orphaned_separators() {
    let list = list(vec![separator(None), action("a"), separator(None), separator(Some("Empty")), separator(None), action("b"), separator(None)]);
    assert_eq!(rows(&list), ["a", "", "b"]);
}

/// 'focuses the configured initial item when opened'.
#[test]
fn focuses_the_configured_initial_item() {
    let mut list = ActionList::new(vec![action("first"), action("active"), action("last")], Options { initial_focus_item_id: Some("active".into()), ..Options::default() });
    list.focus();
    assert_eq!(focused(&list), Some("active"));
}

/// 'skips a disabled configured and checked item when choosing initial focus'.
#[test]
fn skips_a_disabled_configured_and_checked_item() {
    let mut list = ActionList::new(
        vec![action("first"), Item { checked: true, ..disabled("disabled") }, action("last")],
        Options { initial_focus_item_id: Some("disabled".into()), ..Options::default() },
    );
    list.focus();
    assert_eq!(focused(&list), Some("first"));
}

/// Without an initial id the checked row takes focus.
#[test]
fn focuses_the_checked_item() {
    let mut list = list(vec![action("first"), Item { checked: true, ..action("checked") }]);
    list.focus();
    assert_eq!(focused(&list), Some("checked"));
}

/// 'consumes initial focus before later filtering and refocusing'.
#[test]
fn consumes_initial_focus_before_later_filtering() {
    let mut list = ActionList::new(vec![action("match-first"), action("match-initial"), action("other")], Options { initial_focus_item_id: Some("match-initial".into()), ..Options::default() });
    list.focus();
    list.focus_previous();
    list.set_filter("match".into());
    list.focus();
    assert_eq!(focused(&list), Some("match-first"));
}

#[test]
fn collapsing_a_section_hides_its_rows_and_focuses_its_toggle() {
    let mut list = ActionList::new(
        vec![action("top"), section_toggle("more"), in_section(action("inner"), "more")],
        Options { collapsed_by_default: ["more".to_owned()].into(), ..Options::default() },
    );
    assert_eq!(rows(&list), ["top", "toggle-more"]);
    list.focus();
    list.focus_next();
    assert_eq!(list.accept_selected(false, false), Accepted::Nothing);
    assert_eq!(rows(&list), ["top", "toggle-more", "inner"]);
    list.focus_next();
    assert!(list.collapse_focused_section());
    assert_eq!((rows(&list), focused(&list)), (vec!["top", "toggle-more"], Some("toggle-more")));
    assert!(list.expand_focused_section());
    assert!(!list.expand_focused_section());
    assert!(list.toggle_focused_section());
    assert_eq!(rows(&list), ["top", "toggle-more"]);
}

/// `clearFilter` reports whether there was a filter to clear.
#[test]
fn clear_filter_restores_every_row() {
    let mut list = with_filter(vec![action("alpha"), action("beta")]);
    assert!(!list.clear_filter());
    list.set_filter("beta".into());
    assert!(list.clear_filter());
    assert_eq!(rows(&list), ["alpha", "beta"]);
}

#[test]
fn disabled_rows_are_not_accepted_on_click() {
    let mut list = list(vec![disabled("off"), action("on")]);
    assert_eq!(list.accept_row(0, false, true), Accepted::Nothing);
    assert_eq!(list.accept_row(1, true, true), Accepted::Select { item: "on".into(), preview: true });
    assert_eq!(list.accept_row(1, true, false), Accepted::Select { item: "on".into(), preview: false });
}

/// `updateItems` keeps the focused row by id.
#[test]
fn update_items_keeps_the_focused_row() {
    let mut list = list(vec![action("a"), action("b")]);
    list.focus();
    list.focus_next();
    list.update_items(vec![action("new"), action("a"), action("b")], None);
    assert_eq!(focused(&list), Some("b"));
    list.update_items(vec![action("x"), action("y")], Some("y"));
    assert_eq!(focused(&list), Some("y"));
}

/// 'max visible items caps the height at the rows through that many actions, recomputed after filtering'.
#[test]
fn max_visible_items_caps_the_height() {
    let mut items = vec![action("first"), separator(None), Item { detail: Some("Second line".into()), ..action("detailed") }, separator(Some("Group"))];
    items.extend((0..20).map(|i| action(&format!("item-{i}"))));
    let mut list = ActionList::new(items, Options { show_filter: true, max_visible_items: Some(3), ..Options::default() });
    let mut above = None;
    let initial = list.compute_height(20., 20., 600., 0., &mut above);
    list.set_filter("item-1".into());
    let filtered = list.compute_height(20., 20., 600., 0., &mut above);
    // 24px action + 8px separator + 48px detail action + 24px labeled separator + 24px action,
    // then the labeled separator kept as a section header above three 24px matches.
    assert_eq!((initial, filtered), (128., 96.));
}

/// 'leaves room for action widget chrome when clamping dynamic height'.
#[test]
fn leaves_room_for_the_widget_chrome() {
    let list = with_filter((0..50).map(|i| action(&format!("item-{i}"))).collect());
    let mut above = None;
    let chrome = 10.;
    let height = list.compute_height(150., 20., 300., chrome, &mut above);
    assert_eq!(above, Some(true));
    assert!(height + super::FILTER_HEIGHT + chrome <= 150.);
}

/// `computeMaxWidth`: 50 rows or more take 380px; the option bounds win over the measured rows.
#[test]
fn compute_width_clamps_the_widest_row() {
    let big = list((0..50).map(|i| action(&format!("item-{i}"))).collect());
    assert_eq!(big.compute_width(0., |_| 1000.), 380.);
    let small = ActionList::new(vec![action("a"), action("bb")], Options { min_width: Some(100.), max_width: Some(300.), ..Options::default() });
    assert_eq!(small.compute_width(0., |item| item.label.as_ref().unwrap().len() as f32 * 200.), 300.);
    assert_eq!(small.compute_width(0., |_| 20.), 100.);
    assert_eq!(small.compute_width(400., |_| 20.), 400.);
}
