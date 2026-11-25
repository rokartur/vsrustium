use super::{Priority, SashState, SplitView, View};

fn views(sizes: &[f32]) -> Vec<View> {
    sizes.iter().map(|&size| View::new(size, 20., f32::INFINITY)).collect()
}

#[test]
fn can_resize_views() {
    let mut split_view = SplitView::new(200., views(&[160., 20., 20.]));
    split_view.layout(200.);
    assert_eq!(split_view.sizes(), [160., 20., 20.]);

    split_view.resize_view(1, 40.);
    assert_eq!(split_view.sizes(), [140., 40., 20.]);

    split_view.resize_view(0, 70.);
    assert_eq!(split_view.sizes(), [70., 40., 90.]);

    split_view.resize_view(2, 40.);
    assert_eq!(split_view.sizes(), [70., 90., 40.]);
}

#[test]
fn proportional_layout() {
    let mut split_view = SplitView::new(200., views(&[100., 100.]));
    split_view.layout(200.);
    assert_eq!(split_view.sizes(), [100., 100.]);

    split_view.layout(100.);
    assert_eq!(split_view.sizes(), [50., 50.]);
}

#[test]
fn sashes_are_properly_enabled_and_disabled() {
    let mut split_view = SplitView::new(200., views(&[66., 67., 67.]));
    split_view.layout(200.);
    assert_eq!(split_view.sash_states(), [SashState::Enabled, SashState::Enabled]);

    split_view.layout(60.);
    assert_eq!(split_view.sash_states(), [SashState::Disabled, SashState::Disabled]);

    split_view.layout(20.);
    assert_eq!(split_view.sash_states(), [SashState::Disabled, SashState::Disabled]);

    split_view.layout(200.);
    assert_eq!(split_view.sash_states(), [SashState::Enabled, SashState::Enabled]);

    let mut fixed = SplitView::new(200., vec![View::new(40., 40., 40.), View::new(160., 20., f32::INFINITY)]);
    fixed.layout(200.);
    assert_eq!(fixed.sash_states(), [SashState::Disabled]);
}

#[test]
fn a_sash_drag_moves_the_boundary_within_the_bounds_and_keeps_the_new_proportions() {
    let mut split_view = SplitView::new(200., views(&[100., 100.]));
    split_view.layout(200.);
    split_view.start_sash_drag(0, 100., false);
    split_view.drag_sash(130., false);
    assert_eq!(split_view.sizes(), [130., 70.]);

    split_view.drag_sash(250., false);
    assert_eq!(split_view.sizes(), [180., 20.]);
    split_view.end_sash_drag();
    assert_eq!(split_view.sash_states(), [SashState::AtMaximum]);

    split_view.layout(100.);
    assert_eq!(split_view.sizes(), [80., 20.]);
}

#[test]
fn an_alt_sash_drag_resizes_the_view_past_the_sash_on_both_sides() {
    let mut split_view = SplitView::new(200., views(&[60., 80., 60.]));
    split_view.layout(200.);
    split_view.start_sash_drag(0, 60., true);
    split_view.drag_sash(70., true);
    assert_eq!(split_view.sizes(), [70., 60., 70.]);

    split_view.drag_sash(80., false);
    assert_eq!(split_view.sizes(), [80., 50., 70.]);
}

fn prioritized(sizes: &[f32], priorities: &[Priority]) -> SplitView {
    let views = sizes.iter().zip(priorities).map(|(&size, &priority)| View { priority, ..View::new(size, 20., f32::INFINITY) });
    let mut split_view = SplitView::with_priorities(sizes.iter().sum(), views.collect());
    split_view.layout(sizes.iter().sum());
    split_view
}

#[test]
fn disable_proportional_layout() {
    let mut split_view = prioritized(&[100., 100.], &[Priority::Normal; 2]);
    split_view.layout(100.);
    assert_eq!(split_view.sizes(), [80., 20.]);
}

#[test]
fn high_layout_priority() {
    let mut split_view = prioritized(&[66., 68., 66.], &[Priority::Normal, Priority::High, Priority::Normal]);
    for (size, sizes) in [(180., [66., 48., 66.]), (124., [66., 20., 38.]), (60., [20., 20., 20.]), (200., [20., 160., 20.])] {
        split_view.layout(size);
        assert_eq!(split_view.sizes(), sizes, "at {size}");
    }
}

#[test]
fn low_layout_priority() {
    let mut split_view = prioritized(&[66., 68., 66.], &[Priority::Normal, Priority::Normal, Priority::Low]);
    for (size, sizes) in [(180., [66., 48., 66.]), (132., [46., 20., 66.]), (60., [20., 20., 20.]), (200., [20., 160., 20.])] {
        split_view.layout(size);
        assert_eq!(split_view.sizes(), sizes, "at {size}");
    }
}

#[test]
fn a_toggled_view_gives_its_room_to_the_high_priority_view_and_takes_it_back() {
    let editor = View { priority: Priority::High, ..View::new(120., 20., f32::INFINITY) };
    let side_bar = View { priority: Priority::Low, ..View::new(70., 20., f32::INFINITY) };
    let mut split_view = SplitView::with_priorities(200., vec![editor, side_bar, View::new(10., 10., 10.)]);
    split_view.layout(200.);
    split_view.set_view_visible(1, false);
    assert_eq!(split_view.sizes(), [190., 0., 10.]);
    split_view.set_view_visible(1, true);
    assert_eq!(split_view.sizes(), [120., 70., 10.]);
}

#[test]
fn a_sash_drag_past_half_the_minimum_snaps_the_view_hidden_and_back() {
    let side_bar = View { snap: true, priority: Priority::Low, ..View::new(100., 60., f32::INFINITY) };
    let editor = View { priority: Priority::High, ..View::new(200., 50., f32::INFINITY) };
    let mut split_view = SplitView::with_priorities(310., vec![View::new(10., 10., 10.), side_bar, editor]);
    split_view.layout(310.);
    split_view.start_sash_drag(1, 110., false);
    split_view.drag_sash(65., false);
    assert_eq!(split_view.sizes(), [10., 60., 240.]);
    split_view.drag_sash(15., false);
    assert_eq!((split_view.sizes(), split_view.is_view_visible(1)), (vec![10., 0., 300.], false));
    split_view.drag_sash(90., false);
    assert_eq!((split_view.sizes(), split_view.is_view_visible(1)), (vec![10., 80., 220.], true));
    split_view.drag_sash(10., false);
    split_view.end_sash_drag();
    assert_eq!(split_view.sash_states(), [SashState::Disabled, SashState::AtMinimum]);
}
