use super::*;

/// objectTree.test.ts `shows the default sticky node after its source row starts scrolling out`: 20px rows, a
/// 100px view, one sticky row at most.
#[test]
fn a_parent_sticks_once_its_row_starts_scrolling_out() {
    let depths = [1, 2, 2, 2, 2, 2, 2];
    assert_eq!(sticky_rows(&depths, 20., 0., 100., 1), []);
    assert_eq!(sticky_rows(&depths, 20., 1., 100., 1), [StickyRow { index: 0, position: 0. }]);
}

/// objectTree.test.ts `shrinks a sticky node to its final pixel before the next root`.
#[test]
fn a_sticky_row_is_pushed_up_by_the_end_of_its_children() {
    let depths = [1, 2, 2, 1, 1, 1, 1, 1];
    assert_eq!(sticky_rows(&depths, 20., 59., 100., 1), [StickyRow { index: 0, position: -19. }]);
    assert_eq!(sticky_rows(&depths, 20., 60., 100., 1), []);
}

#[test]
fn nested_parents_stick_up_to_the_item_count_and_40_percent_of_the_view() {
    // a/ b/ c/ with ten files in c, 22px rows.
    let depths = [1, 2, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4];
    let indices = |rows: Vec<StickyRow>| rows.iter().map(|row| row.index).collect::<Vec<_>>();
    assert_eq!(indices(sticky_rows(&depths, 22., 100., 400., 7)), [0, 1, 2]);
    assert_eq!(indices(sticky_rows(&depths, 22., 100., 400., 2)), [0, 1]);
    // 40% of a 120px view holds two 22px rows.
    assert_eq!(indices(sticky_rows(&depths, 22., 100., 120., 7)), [0, 1]);
    assert_eq!(position_below_widget(&depths, 5, 22., 7), 66.);
    assert_eq!(position_below_widget(&depths, 5, 22., 2), 44.);
}

/// `_onDidChangeActiveNodes`: an expanded focused folder makes its own guides active, a file its parent's.
#[test]
fn indent_guides_are_active_under_the_focused_rows_parent() {
    // 0 a/, 1 a/b/, 2 a/b/x, 3 a/y, 4 z
    let depths = [1, 2, 3, 2, 1];
    assert_eq!(active_guide_parents(&depths, &[2]), [1]);
    assert_eq!(active_guide_parents(&depths, &[1]), [1]);
    assert_eq!(active_guide_parents(&depths, &[4]), Vec::<usize>::new());
    assert_eq!(indent_guides(&depths, 2, &[1]), [false, true]);
    assert_eq!(indent_guides(&depths, 3, &[0]), [true]);
    assert_eq!(indent_guides(&depths, 4, &[0]), Vec::<bool>::new());
}

#[test]
fn the_wheel_scrolls_the_predominant_axis_by_the_sensitivities() {
    assert_eq!(wheel_scroll_delta(0., -1., false, 1., 5.), (0., 50.));
    assert_eq!(wheel_scroll_delta(0.2, -1., false, 2., 5.), (0., 100.));
    assert_eq!(wheel_scroll_delta(0., -1., true, 1., 5.), (0., 250.));
    assert_eq!(wheel_scroll_delta(0.004, 0.001, false, 1., 5.), (-1., 0.), "a slow scroll still moves a pixel");
}

#[test]
fn a_paged_track_press_moves_one_view() {
    assert_eq!(paged_scroll_position(10., 50., 300., 200.), 100.);
    assert_eq!(paged_scroll_position(90., 50., 300., 200.), 500.);
}

#[test]
fn smooth_scrolling_eases_out_and_skips_long_jumps() {
    assert_eq!(smooth_scroll_position(0., 100., 400., 0.), 0.);
    assert_eq!(smooth_scroll_position(0., 100., 400., 0.5), 87.5);
    assert_eq!(smooth_scroll_position(0., 100., 400., 1.), 100.);
    // 2000px over a 400px view: 300px in the first third, then from 1700px.
    assert_eq!(smooth_scroll_position(0., 2000., 400., 0.33), 1700.);
    assert!((smooth_scroll_position(0., 2000., 400., 0.165) - 262.5).abs() < 0.01);
}
