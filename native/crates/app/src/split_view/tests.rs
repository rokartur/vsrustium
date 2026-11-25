use super::{SashState, SplitView, View};

fn views(sizes: &[f32]) -> Vec<View> {
    sizes.iter().map(|&size| View { size, minimum: 20., maximum: f32::INFINITY }).collect()
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

    let mut fixed = SplitView::new(200., vec![View { size: 40., minimum: 40., maximum: 40. }, View { size: 160., minimum: 20., maximum: f32::INFINITY }]);
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
