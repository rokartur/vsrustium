use super::{Alignment, Arrangement, CARD_MARGIN, Grid, Part, PartSizes, Position, Sash, card_margins};
use crate::split_view::SashState;

fn arrangement(side_bar: Position, panel: Position, alignment: Alignment) -> Arrangement {
    Arrangement { side_bar, panel, alignment }
}

/// The reference window at 1024x768: a 35px title bar, a 28px status bar and every part showing.
fn reference_grid(arrangement: Arrangement) -> Grid {
    Grid::new(1024., 705., PartSizes::defaults(1024., 768., arrangement.panel), arrangement, |_| true)
}

fn bounds(grid: &Grid, part: Part) -> [f32; 4] {
    grid.part_bounds(part).unwrap()
}

#[test]
fn the_default_grid_matches_the_reference_window() {
    let grid = reference_grid(Arrangement::default());
    assert_eq!(bounds(&grid, Part::ActivityBar), [0., 0., 48., 705.]);
    assert_eq!(bounds(&grid, Part::SideBar), [48., 0., 256., 705.]);
    assert_eq!(bounds(&grid, Part::Editor), [304., 0., 464., 449.]);
    assert_eq!(bounds(&grid, Part::Panel), [304., 449., 464., 256.]);
    assert_eq!(bounds(&grid, Part::AuxiliaryBar), [768., 0., 256., 705.]);
    let sashes = grid.sashes();
    assert_eq!(sashes[0], (Sash { branch: 0, index: 0, vertical: true }, SashState::Disabled, [48., 0., 4., 705.]));
    assert_eq!(sashes[1].2, [304., 0., 4., 705.]);
    assert_eq!(sashes[2].2, [768., 0., 4., 705.]);
    assert_eq!(sashes[3], (Sash { branch: 1, index: 0, vertical: false }, sashes[3].1, [306., 449., 462., 4.]));
}

#[test]
fn a_hidden_side_bar_gives_its_width_to_the_editor_and_comes_back_at_it() {
    let mut grid = reference_grid(Arrangement::default());
    grid.set_visible(Part::SideBar, false);
    assert_eq!(bounds(&grid, Part::Editor), [48., 0., 720., 449.]);
    assert_eq!(grid.size(Part::SideBar), 256.);
    grid.set_visible(Part::SideBar, true);
    assert_eq!(bounds(&grid, Part::Editor), [304., 0., 464., 449.]);
}

#[test]
fn a_hidden_panel_gives_its_height_to_the_editor() {
    let mut grid = Grid::new(1024., 705., PartSizes::defaults(1024., 768., Position::Bottom), Arrangement::default(), |part| part != Part::Panel);
    assert_eq!(bounds(&grid, Part::Editor), [304., 0., 464., 705.]);
    grid.set_visible(Part::Panel, true);
    assert_eq!(bounds(&grid, Part::Editor), [304., 0., 464., 449.]);
}

#[test]
fn every_arrangement_lays_out_with_any_part_hidden() {
    let alignments = [Alignment::Left, Alignment::Center, Alignment::Right, Alignment::Justify];
    for side_bar in [Position::Left, Position::Right] {
        for panel in [Position::Bottom, Position::Top, Position::Left, Position::Right] {
            for alignment in alignments {
                for hidden in [None, Some(Part::SideBar), Some(Part::AuxiliaryBar), Some(Part::Panel)] {
                    let grid = Grid::new(1024., 705., PartSizes::defaults(1024., 768., panel), arrangement(side_bar, panel, alignment), |part| Some(part) != hidden);
                    assert!(grid.part_bounds(Part::Editor).is_some());
                    grid.sashes();
                }
            }
        }
    }
}
#[test]
fn a_window_resize_goes_to_the_editor_first() {
    let mut grid = reference_grid(Arrangement::default());
    grid.layout(1124., 805.);
    assert_eq!(bounds(&grid, Part::Editor), [304., 0., 564., 549.]);
    assert_eq!(bounds(&grid, Part::Panel)[3], 256.);
}

#[test]
fn dragging_the_side_bar_sash_below_half_its_minimum_hides_it() {
    let mut grid = reference_grid(Arrangement::default());
    grid.start_sash_drag(Sash { branch: 0, index: 1, vertical: true }, 304., false);
    grid.drag_sash(304. - 256. + 84., false);
    assert!(!grid.is_visible(Part::SideBar));
    grid.end_sash_drag();
    assert_eq!(grid.part_bounds(Part::SideBar), None);
}

#[test]
fn the_panel_keeps_the_editor_column_at_least_its_minimum_width() {
    let mut grid = reference_grid(Arrangement::default());
    grid.start_sash_drag(Sash { branch: 0, index: 1, vertical: true }, 304., false);
    grid.drag_sash(2000., false);
    assert_eq!(bounds(&grid, Part::Editor)[2], 300.);
}

#[test]
fn a_right_side_bar_mirrors_the_row() {
    let grid = reference_grid(arrangement(Position::Right, Position::Bottom, Alignment::Center));
    assert_eq!(bounds(&grid, Part::AuxiliaryBar), [0., 0., 256., 705.]);
    assert_eq!(bounds(&grid, Part::Editor), [256., 0., 464., 449.]);
    assert_eq!(bounds(&grid, Part::SideBar), [720., 0., 256., 705.]);
    assert_eq!(bounds(&grid, Part::ActivityBar), [976., 0., 48., 705.]);
}

#[test]
fn a_right_rail_without_the_side_bar_takes_the_leading_gap() {
    let mut grid = reference_grid(arrangement(Position::Right, Position::Bottom, Alignment::Center));
    grid.set_visible(Part::SideBar, false);
    assert_eq!(bounds(&grid, Part::ActivityBar), [972., 0., 52., 705.]);
    grid.set_visible(Part::SideBar, true);
    assert_eq!(bounds(&grid, Part::ActivityBar), [976., 0., 48., 705.]);
}

#[test]
fn a_side_panel_sits_next_to_the_editor_at_a_quarter_of_the_width() {
    let wide = |panel| Grid::new(1400., 705., PartSizes::defaults(1400., 768., panel), arrangement(Position::Left, panel, Alignment::Center), |_| true);
    let grid = wide(Position::Right);
    assert_eq!(bounds(&grid, Part::Editor), [348., 0., 402., 705.]);
    assert_eq!(bounds(&grid, Part::Panel), [750., 0., 350., 705.]);
    assert_eq!(bounds(&wide(Position::Left), Part::Panel), [348., 0., 350., 705.]);
}

#[test]
fn a_justified_panel_spans_under_both_side_bars() {
    let grid = reference_grid(arrangement(Position::Left, Position::Bottom, Alignment::Justify));
    assert_eq!(bounds(&grid, Part::SideBar), [48., 0., 256., 449.]);
    assert_eq!(bounds(&grid, Part::Editor), [304., 0., 464., 449.]);
    assert_eq!(bounds(&grid, Part::AuxiliaryBar), [768., 0., 256., 449.]);
    assert_eq!(bounds(&grid, Part::Panel), [48., 449., 976., 256.]);
    // The editor row's sashes follow the root's and the column's.
    assert_eq!(grid.sashes().iter().filter(|(sash, ..)| sash.branch == 2).count(), 2);
}

#[test]
fn left_and_right_alignment_span_the_panel_under_one_side_bar() {
    let grid = reference_grid(arrangement(Position::Left, Position::Top, Alignment::Left));
    assert_eq!(bounds(&grid, Part::Panel), [48., 0., 720., 256.]);
    assert_eq!(bounds(&grid, Part::SideBar), [48., 256., 256., 449.]);
    assert_eq!(bounds(&grid, Part::AuxiliaryBar), [768., 0., 256., 705.]);
    let grid = reference_grid(arrangement(Position::Left, Position::Bottom, Alignment::Right));
    assert_eq!(bounds(&grid, Part::Panel), [304., 449., 720., 256.]);
    assert_eq!(bounds(&grid, Part::SideBar), [48., 0., 256., 705.]);
}

#[test]
fn a_hidden_editor_maximizes_the_panel() {
    let mut grid = reference_grid(Arrangement::default());
    grid.set_visible(Part::Editor, false);
    assert_eq!(bounds(&grid, Part::Panel), [304., 0., 464., 705.]);
}

/// layoutService.test.ts `getFloatingPaneCompositeHorizontalMargins` and `getFloatingPaneCompositeVerticalMargins`
/// for the default density, here from the parts' places.
#[test]
fn card_margins_follow_the_floating_layout() {
    let margins = |grid: &Grid, part: Part| card_margins(part, bounds(grid, part), &grid.bounds(), grid.arrangement(), 1024., 705., false);
    let grid = reference_grid(Arrangement::default());
    assert_eq!(margins(&grid, Part::ActivityBar), [CARD_MARGIN, 0., 0., CARD_MARGIN]);
    assert_eq!(margins(&grid, Part::SideBar), [0., 0., 0., CARD_MARGIN]);
    assert_eq!(margins(&grid, Part::Editor), [CARD_MARGIN, 0., 0., 0.]);
    assert_eq!(margins(&grid, Part::Panel), [CARD_MARGIN, CARD_MARGIN, 0., CARD_MARGIN]);
    assert_eq!(margins(&grid, Part::AuxiliaryBar), [CARD_MARGIN, 0., CARD_MARGIN, CARD_MARGIN]);
    let grid = reference_grid(arrangement(Position::Right, Position::Bottom, Alignment::Justify));
    assert_eq!(margins(&grid, Part::SideBar), [CARD_MARGIN, 0., 0., 0.]);
    assert_eq!(margins(&grid, Part::ActivityBar), [0., 0., CARD_MARGIN, CARD_MARGIN]);
    assert_eq!(margins(&grid, Part::Panel), [CARD_MARGIN, CARD_MARGIN, CARD_MARGIN, CARD_MARGIN]);
    let grid = reference_grid(arrangement(Position::Left, Position::Top, Alignment::Center));
    assert_eq!(margins(&grid, Part::Editor), [CARD_MARGIN, CARD_MARGIN, 0., CARD_MARGIN]);
    assert_eq!(margins(&grid, Part::Panel), [CARD_MARGIN, 0., 0., 0.]);
}

/// activitybarPart.ts with `window.density.layout: compact`: a 40px rail, and sashes centered on the seams.
#[test]
fn compact_density_narrows_the_rail_and_centers_the_sashes() {
    let mut grid = Grid::new(1024., 707., PartSizes::defaults(1024., 768., Position::Bottom), Arrangement::default(), |_| true);
    grid.set_compact(true);
    assert_eq!(bounds(&grid, Part::ActivityBar), [0., 0., 40., 707.]);
    let [x, ..] = bounds(&grid, Part::Editor);
    let sashes = grid.sashes();
    assert_eq!(sashes[1].2, [x - 2., 0., 4., 707.]);
    let [_, y, width, _] = bounds(&grid, Part::Panel);
    assert_eq!(sashes[3].2, [x, y - 2., width, 4.]);
    let mut grid = Grid::new(1024., 707., PartSizes::defaults(1024., 768., Position::Bottom), arrangement(Position::Right, Position::Bottom, Alignment::Center), |part| part != Part::SideBar);
    grid.set_compact(true);
    assert_eq!(bounds(&grid, Part::ActivityBar)[2], 40., "compact density never adds the right rail's leading gap");
    grid.set_compact(false);
    assert_eq!(bounds(&grid, Part::ActivityBar)[2], 52.);
}
