use super::*;

/// The reference's port/fixtures/notebooks/sample.ipynb in Dark Modern (window points below the notebook
/// toolbar): the markdown preview measures 143, the first code cell has 2 lines and 54 of outputs, its box starts 6
/// below the cell, its outputs below its status bar, and the next cell 24 below them.
#[test]
fn lays_out_cells_as_the_reference_notebook() {
    let cells = [
        CellInput::Preview { height: Some(143.), collapsed: false },
        CellInput::Editor { lines: 2, outputs_height: 54., markup: false },
        CellInput::Editor { lines: 1, outputs_height: 36., markup: false },
        CellInput::Editor { lines: 1, outputs_height: 0., markup: false },
    ];
    let (layouts, height) = layout(&cells, 18.);
    let tops: Vec<f32> = layouts.iter().map(|cell| cell.top).collect();
    assert_eq!(tops, [18., 179., 337., 459.]);
    assert_eq!((layouts[1].editor_top, layouts[1].editor_height, layouts[1].output_top), (6., 52., 80.));
    assert_eq!(height, 459. + 6. + 34. + 22. + 18. + 6.);
}

#[test]
fn an_unmeasured_preview_takes_upstreams_first_height() {
    let (layouts, _) = layout(&[CellInput::Preview { height: None, collapsed: false }], 18.);
    assert_eq!(layouts[0].height, 118.);
}

/// markupCellViewModel.ts: a collapsed header adds the fold hint, the cells of its section take no space.
#[test]
fn a_collapsed_section_shows_its_hint_and_hides_its_cells() {
    let cells = [CellInput::Preview { height: Some(40.), collapsed: true }, CellInput::Hidden, CellInput::Editor { lines: 1, outputs_height: 0., markup: false }];
    let (layouts, _) = layout(&cells, 18.);
    assert_eq!(layouts[0].height, 40. + 18. + 22.);
    assert_eq!((layouts[1].top, layouts[1].height), (18. + 80., 0.));
    assert_eq!(layouts[2].top, 18. + 80.);
}
