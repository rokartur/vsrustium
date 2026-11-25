use super::{Grid, Part, PartSizes, Sash};
use crate::split_view::SashState;

/// The reference window at 1024x768: a 35px title bar, a 28px status bar and every part showing.
fn reference_grid() -> Grid {
    Grid::new(1024., 705., PartSizes::defaults(1024., 768.), |_| true)
}

#[test]
fn the_default_grid_matches_the_reference_window() {
    let grid = reference_grid();
    assert_eq!(grid.row_sizes(), vec![48., 256., 464., 256.]);
    assert_eq!(grid.column_sizes(), vec![449., 256.]);
    let sashes = grid.sashes();
    assert_eq!(sashes[0], (Sash::Row(0), SashState::Disabled, [48., 0., 4., 705.]));
    assert_eq!(sashes[1].2, [304., 0., 4., 705.]);
    assert_eq!(sashes[2].2, [768., 0., 4., 705.]);
    assert_eq!(sashes[3].2, [306., 449., 462., 4.]);
}

#[test]
fn a_hidden_side_bar_gives_its_width_to_the_editor_and_comes_back_at_it() {
    let mut grid = reference_grid();
    grid.set_visible(Part::SideBar, false);
    assert_eq!(grid.row_sizes(), vec![48., 0., 720., 256.]);
    assert_eq!(grid.size(Part::SideBar), 256.);
    grid.set_visible(Part::SideBar, true);
    assert_eq!(grid.row_sizes(), vec![48., 256., 464., 256.]);
}

#[test]
fn a_hidden_panel_gives_its_height_to_the_editor() {
    let mut grid = Grid::new(1024., 705., PartSizes::defaults(1024., 768.), |part| part != Part::Panel);
    assert_eq!(grid.column_sizes(), vec![705., 0.]);
    grid.set_visible(Part::Panel, true);
    assert_eq!(grid.column_sizes(), vec![449., 256.]);
}

#[test]
fn a_window_resize_goes_to_the_editor_first() {
    let mut grid = reference_grid();
    grid.layout(1124., 805.);
    assert_eq!(grid.row_sizes(), vec![48., 256., 564., 256.]);
    assert_eq!(grid.column_sizes(), vec![549., 256.]);
}

#[test]
fn dragging_the_side_bar_sash_below_half_its_minimum_hides_it() {
    let mut grid = reference_grid();
    grid.start_sash_drag(Sash::Row(1), 304., false);
    grid.drag_sash(304. - 256. + 84., false);
    assert!(!grid.is_visible(Part::SideBar));
    grid.end_sash_drag();
    assert_eq!(grid.row_sizes()[1], 0.);
}

#[test]
fn the_panel_keeps_the_editor_column_at_least_its_minimum_width() {
    let mut grid = reference_grid();
    grid.start_sash_drag(Sash::Row(1), 304., false);
    grid.drag_sash(2000., false);
    assert_eq!(grid.row_sizes()[2], 300.);
}
