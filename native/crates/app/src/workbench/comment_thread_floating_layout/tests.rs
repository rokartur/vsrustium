//! commentThreadFloatingWidget.test.ts.

use super::*;

fn rect(top: f32, left: f32, width: f32, height: f32) -> Rect {
    Rect { top, left, width, height }
}

fn obstacle(top: f32, left: f32, width: f32, height: f32, code_top: f32) -> Obstacle {
    Obstacle { rect: rect(top, left, width, height), code_top }
}

fn point(x: f32, y: f32) -> Point {
    Point { x, y }
}

/// The SVG path upstream's `getRowsOutlinePath` returns.
fn svg_path(outline: &[OutlineCorner]) -> String {
    if outline.is_empty() {
        return String::new();
    }
    let mut commands = Vec::new();
    for (i, corner) in outline.iter().enumerate() {
        let command = if i == 0 { "M" } else { "L" };
        match corner {
            OutlineCorner::Sharp(p) => commands.push(format!("{command} {} {}", p.x, p.y)),
            OutlineCorner::Rounded { start, corner, end } => commands.push(format!("{command} {} {} Q {} {} {} {}", start.x, start.y, corner.x, corner.y, end.x, end.y)),
        }
    }
    commands.push("Z".into());
    commands.join(" ")
}

#[test]
fn connector_connects_the_closest_point_of_the_code_to_the_anchor() {
    let code = [rect(100., 50., 200., 20.)];
    assert_eq!(connector_points(&code, point(400., 40.)), Some((point(250., 100.), point(400., 40.))));
    assert_eq!(connector_points(&code, point(400., 110.)), Some((point(250., 110.), point(400., 110.))));
    assert_eq!(connector_points(&code, point(400., 300.)), Some((point(250., 120.), point(400., 300.))));
    assert_eq!(connector_points(&code, point(120., 160.)), Some((point(120., 120.), point(120., 160.))));
    assert_eq!(connector_points(&code, point(10., 10.)), Some((point(50., 100.), point(10., 10.))));
}

#[test]
fn connector_connects_the_closest_line_of_code_that_spans_several_lines() {
    let lines = [rect(100., 50., 200., 20.), rect(120., 50., 80., 20.)];
    assert_eq!(connector_points(&lines, point(200., 135.)), Some((point(200., 120.), point(200., 135.))));
    assert_eq!(connector_points(&lines, point(100., 160.)), Some((point(100., 140.), point(100., 160.))));
    assert_eq!(connector_points(&lines, point(100., 130.)), None);
}

#[test]
fn connector_does_not_connect_an_anchor_within_or_on_the_code() {
    let code = [rect(100., 50., 200., 20.)];
    assert_eq!(connector_points(&code, point(100., 110.)), None);
    assert_eq!(connector_points(&code, point(250., 105.)), None);
    assert_eq!(connector_points(&[], point(100., 110.)), None);
}

#[test]
fn rows_outline_outlines_the_end_of_each_row() {
    let rows = [rect(0., 0., 100., 20.), rect(20., 0., 60., 20.), rect(40., 0., 60., 20.)];
    assert_eq!(svg_path(&rows_outline(&[], 4.)), "");
    assert_eq!(svg_path(&rows_outline(&rows, 0.)), "M 100 0 L 100 20 L 60 20 L 60 60 L 0 60 L 0 0 Z");
    assert_eq!(svg_path(&rows_outline(&rows[..1], 4.)), "M 96 0 Q 100 0 100 4 L 100 16 Q 100 20 96 20 L 4 20 Q 0 20 0 16 L 0 4 Q 0 0 4 0 Z");
}

#[test]
fn centered_top_centers_on_the_code_without_going_above_the_document() {
    assert_eq!(centered_top(500., 700., 100.), 550.);
    assert_eq!(centered_top(500., 520., 200.), 410.);
    assert_eq!(centered_top(20., 40., 200.), 0.);
}

fn request() -> PlacementRequest {
    PlacementRequest { min_left: 60., max_right: 1000., width: 400., height: 200., code: Some(rect(500., 60., 300., 40.)), obstacles: Vec::new(), scrollable_height: 5000., beside_code: false, lines_right: None }
}

fn placed(top: f32, left: f32, centered: bool) -> Placement {
    Placement { top, left, centered }
}

#[test]
fn placement_centers_the_widget_vertically_on_the_code_at_the_right_edge_of_the_editor() {
    assert_eq!(floating_widget_placement(&request()), placed(420., 592., true));
}

#[test]
fn placement_goes_below_the_code_at_the_right_edge_when_there_is_no_room_next_to_it() {
    let long_code = PlacementRequest { code: Some(rect(500., 60., 560., 40.)), ..request() };
    let indented_code = PlacementRequest { code: Some(rect(500., 700., 280., 20.)), ..request() };
    let narrow_editor = PlacementRequest { max_right: 500., ..request() };
    assert_eq!(floating_widget_placement(&long_code), placed(548., 592., false));
    assert_eq!(floating_widget_placement(&indented_code), placed(528., 592., false));
    assert_eq!(floating_widget_placement(&narrow_editor), placed(548., 92., false));
}

#[test]
fn placement_is_at_the_top_of_the_document_for_file_comments() {
    assert_eq!(floating_widget_placement(&PlacementRequest { code: None, ..request() }), placed(0., 592., true));
}

#[test]
fn placement_moves_the_widget_below_the_widgets_it_would_cover() {
    let obstacles = vec![obstacle(300., 600., 300., 150., 360.), obstacle(460., 500., 300., 100., 480.), obstacle(0., 600., 300., 100., 20.)];
    assert_eq!(floating_widget_placement(&PlacementRequest { obstacles, ..request() }), placed(568., 592., false));
}

#[test]
fn placement_puts_a_new_comment_right_after_the_end_of_the_code_when_there_is_room() {
    let room = PlacementRequest { beside_code: true, ..request() };
    let no_room = PlacementRequest { beside_code: true, code: Some(rect(500., 60., 560., 40.)), ..request() };
    let beside_other_widget = PlacementRequest { beside_code: true, obstacles: vec![obstacle(480., 400., 300., 100., 440.)], ..request() };
    assert_eq!(floating_widget_placement(&room), placed(500., 384., false));
    assert_eq!(floating_widget_placement(&no_room), placed(548., 592., false));
    assert_eq!(floating_widget_placement(&beside_other_widget), placed(588., 384., false));
}

#[test]
fn placement_puts_an_existing_comment_right_after_the_lines_next_to_the_code_when_there_is_room() {
    let with = |lines_right: f32| floating_widget_placement(&PlacementRequest { lines_right: Some(lines_right), ..request() });
    assert_eq!(with(400.), placed(420., 424., true));
    assert_eq!(with(200.), placed(420., 384., true));
    assert_eq!(with(700.), placed(420., 592., true));
}

#[test]
fn placement_moves_to_the_closest_free_position_above_or_below_the_widgets_it_would_cover() {
    let closer_above = PlacementRequest { obstacles: vec![obstacle(380., 600., 300., 300., 600.)], ..request() };
    // The document ends at 1000 and can't be scrolled further, so there is no room below the other widget.
    let below_out_of_reach = PlacementRequest { height: 360., code: Some(rect(940., 60., 300., 20.)), obstacles: vec![obstacle(632., 592., 400., 360., 960.)], scrollable_height: 1000., ..request() };
    let no_free_position = PlacementRequest { obstacles: vec![obstacle(0., 600., 300., 400., 520.)], scrollable_height: 400., ..request() };
    assert_eq!(floating_widget_placement(&closer_above), placed(172., 592., false));
    assert_eq!(floating_widget_placement(&below_out_of_reach), placed(264., 592., false));
    assert_eq!(floating_widget_placement(&no_free_position), placed(192., 592., false));
}

#[test]
fn placement_keeps_the_widgets_in_the_order_of_their_code() {
    let with = |obstacles: Vec<Obstacle>| floating_widget_placement(&PlacementRequest { obstacles, ..request() });
    assert_eq!(with(vec![obstacle(340., 600., 300., 200., 600.)]), placed(132., 592., false));
    assert_eq!(with(vec![obstacle(480., 600., 300., 300., 400.)]), placed(788., 592., false));
    assert_eq!(with(vec![obstacle(100., 600., 300., 400., 600.)]), placed(508., 592., false));
    // A widget moved above this one's code doesn't keep this one from staying above the others.
    assert_eq!(with(vec![obstacle(380., 600., 300., 100., 560.), obstacle(0., 600., 300., 100., 600.)]), placed(172., 592., false));
}

#[test]
fn in_the_way_of_a_widget_it_covers_or_is_out_of_code_order_with() {
    let earlier = obstacle(400., 600., 300., 200., 300.);
    assert!(is_in_the_way_of(obstacle(500., 650., 300., 200., 600.), earlier));
    assert!(is_in_the_way_of(obstacle(100., 650., 300., 200., 600.), earlier));
    assert!(!is_in_the_way_of(obstacle(700., 650., 300., 200., 600.), earlier));
    assert!(!is_in_the_way_of(obstacle(100., 100., 300., 200., 600.), earlier));
    assert!(!is_in_the_way_of(obstacle(100., 650., 300., 200., 300.), earlier));
}

#[test]
fn width_beside_code_is_as_preferred_or_as_wide_as_the_room_after_the_code() {
    let code = rect(500., 60., 300., 20.);
    assert_eq!(width_beside_code(code, 200., 400., 240., 1000., &[]), Some(400.));
    assert_eq!(width_beside_code(Rect { width: 500., ..code }, 200., 560., 240., 1000., &[]), Some(408.));
    assert_eq!(width_beside_code(Rect { width: 100., ..code }, 200., 280., 240., 500., &[]), Some(280.));
    assert_eq!(width_beside_code(Rect { width: 700., ..code }, 200., 400., 240., 1000., &[]), None);
    assert_eq!(width_beside_code(Rect { width: 500., ..code }, 200., 560., 450., 1000., &[]), None);
}

#[test]
fn width_beside_code_leaves_room_for_the_widgets_beside_the_code() {
    let code = rect(500., 60., 300., 20.);
    assert_eq!(width_beside_code(code, 200., 400., 240., 1000., &[rect(400., 750., 240., 150.)]), Some(358.));
    assert_eq!(width_beside_code(code, 200., 400., 240., 1000., &[rect(650., 650., 340., 100.)]), None);
    assert_eq!(width_beside_code(code, 200., 400., 240., 1000., &[rect(300., 400., 300., 200.), rect(700., 400., 300., 100.)]), Some(400.));
    assert_eq!(width_beside_code(code, 200., 400., 240., 1000., &[rect(500., 300., 200., 100.)]), None);
}

fn beside_lines(rights: &[f32], bottom: Option<f32>, code_right: f32, max_right: f32, natural_width: f32) -> Option<BesideLines> {
    let lines: Vec<Rect> = rights.iter().enumerate().map(|(i, right)| rect(i as f32 * 20., 60., right - 60., 20.)).collect();
    placement_beside_lines(&lines, 0., bottom.unwrap_or(rights.len() as f32 * 20.), code_right, max_right, 300., natural_width)
}

fn after(lines_right: f32, width: f32) -> Option<BesideLines> {
    Some(BesideLines { lines_right, width })
}

#[test]
fn beside_lines_goes_after_the_lines_that_leave_room_as_wide_as_its_content_up_to_a_readable_width() {
    assert_eq!(beside_lines(&[100., 300., 200.], None, 60., 1000., 450.), after(300., 450.));
    assert_eq!(beside_lines(&[100., 400., 200.], None, 60., 1000., 900.), after(400., 568.));
    assert_eq!(beside_lines(&[100., 300.], None, 60., 1400., 900.), after(300., 640.));
    assert_eq!(beside_lines(&[100., 300.], None, 400., 1000., 450.), after(400., 450.));
    assert_eq!(beside_lines(&[100., 600., 200.], None, 60., 1000., 450.), after(600., 368.));
    assert_eq!(beside_lines(&[100., 300., 200., 700.], Some(60.), 60., 1000., 450.), after(300., 450.));
}

#[test]
fn beside_lines_hides_a_couple_of_lines_that_leave_no_room_at_its_default_width() {
    assert_eq!(beside_lines(&[100., 700., 200.], None, 60., 1000., 450.), after(200., 300.));
    assert_eq!(beside_lines(&[100., 700., 300., 1400.], None, 60., 1000., 450.), after(300., 300.));
    assert_eq!(beside_lines(&[700., 1400., 300., 800.], None, 60., 1000., 450.), None);
    assert_eq!(beside_lines(&[100., 300.], None, 700., 1000., 450.), None);
}

fn span(top: f32, bottom: f32) -> Span {
    Span { top, bottom }
}

#[test]
fn reveal_scrolls_only_when_the_code_or_the_widget_is_out_of_view() {
    let (code, widget) = (span(1000., 1020.), span(830., 1190.));
    assert_eq!(reveal_scroll_top(code, widget, 700., 600.), None);
    assert_eq!(reveal_scroll_top(code, widget, 0., 600.), Some(710.));
}

#[test]
fn reveal_shows_the_whole_widget_when_it_does_not_fit_with_the_code() {
    // The widget is centered on code taller than the viewport.
    let tall_code = span(1000., 3000.);
    let centered_widget = span(1820., 2180.);
    assert_eq!(reveal_scroll_top(tall_code, centered_widget, 0., 600.), Some(1588.));
    assert_eq!(reveal_scroll_top(tall_code, centered_widget, 1700., 600.), None);
    assert_eq!(reveal_scroll_top(span(1000., 1020.), span(2000., 2360.), 0., 600.), Some(1768.));
    assert_eq!(reveal_scroll_top(tall_code, span(1500., 2200.), 0., 600.), Some(1492.));
}

const CODE: Span = Span { top: 1000., bottom: 1020. };
const COLLAPSED_REPLY: ContentHeights = ContentHeights { fixed: 100., comments: 2000. };
const EXPANDED_REPLY: ContentHeights = ContentHeights { fixed: 180., comments: 2000. };
const SCROLLABLE_HEIGHT: f32 = 5000.;

fn model() -> SizeModel {
    let mut model = SizeModel::default();
    model.capture_editor_size(Size { width: 1000., height: 600. });
    model
}

fn widths(minimum: f32, natural: f32) -> ContentWidths {
    ContentWidths { minimum, natural }
}

fn heights(fixed: f32, comments: f32) -> ContentHeights {
    ContentHeights { fixed, comments }
}

#[test]
fn size_can_be_as_wide_as_the_editor_and_shows_as_wide_as_its_content_unless_writing() {
    let model = model();
    assert_eq!(model.width_range(widths(100., 180.)), SizeRange { min: 240., max: 1000. });
    assert_eq!(model.width_range(widths(300., 700.)), SizeRange { min: 300., max: 1000. });
    assert_eq!(model.default_width(widths(100., 180.), 400., false), 240.);
    assert_eq!(model.default_width(widths(300., 700.), 400., false), 400.);
    assert_eq!(model.default_width(widths(100., 180.), 400., true), 400.);
    assert_eq!(model.default_width(widths(300., 1400.), 1200., false), 1000.);
}

#[test]
fn size_shows_the_title_reply_and_some_comments_no_taller_than_content_or_editor() {
    let model = model();
    assert_eq!(model.height_range(heights(100., 30.)), SizeRange { min: 130., max: 130. });
    assert_eq!(model.height_range(heights(100., 300.)), SizeRange { min: 140., max: 400. });
    assert_eq!(model.height_range(COLLAPSED_REPLY), SizeRange { min: 140., max: 584. });
}

#[test]
fn size_fits_its_content_up_to_part_of_the_editor_unless_the_user_chose_a_height() {
    let mut model = model();
    model.update_growth(COLLAPSED_REPLY);
    assert_eq!(model.height(COLLAPSED_REPLY, None), 360.);
    assert_eq!(model.height(heights(100., 100.), None), 200.);
    assert_eq!(model.height(COLLAPSED_REPLY, Some(500.)), 500.);
    assert_eq!(model.height(COLLAPSED_REPLY, Some(900.)), 584.);
}

#[test]
fn size_keeps_some_comments_when_too_short_for_the_rest() {
    let model = model();
    assert_eq!(model.comments_height(COLLAPSED_REPLY, 360.), 260.);
    assert_eq!(model.comments_height(heights(180., 2000.), 134.), 40.);
    assert_eq!(model.comments_height(heights(180., 20.), 134.), 20.);
}

#[test]
fn size_grows_down_by_as_much_as_its_reply_grows() {
    let mut model = model();
    model.update_growth(COLLAPSED_REPLY);
    let collapsed_height = model.height(COLLAPSED_REPLY, None);
    assert_eq!((collapsed_height, model.top(None, CODE, collapsed_height, SCROLLABLE_HEIGHT)), (360., 830.));
    assert!(model.update_growth(EXPANDED_REPLY));
    let expanded_height = model.height(EXPANDED_REPLY, None);
    assert_eq!((expanded_height, model.top(None, CODE, expanded_height, SCROLLABLE_HEIGHT), model.height(EXPANDED_REPLY, Some(300.))), (440., 830., 380.));
    // A height chosen while the reply is expanded includes the reply.
    model.reset_growth();
    model.update_growth(EXPANDED_REPLY);
    assert_eq!(model.height(EXPANDED_REPLY, Some(300.)), 300.);
}

#[test]
fn size_moves_up_to_keep_its_grown_reply_in_the_editor_until_it_shrinks() {
    let mut model = model();
    model.update_growth(COLLAPSED_REPLY);
    model.update_growth(EXPANDED_REPLY);
    let mut shifts = Vec::new();
    // The widget's top is 400px below the editor's top, so the expanded reply runs past its bottom.
    model.update_upward_shift(Some(0.), CODE, 440., 600., 600.);
    shifts.push(model.upward_shift());
    let shifted_top = model.top(Some(0.), CODE, 440., SCROLLABLE_HEIGHT);
    // It doesn't move back down while the reply is grown, such as when the editor scrolls.
    model.update_upward_shift(Some(0.), CODE, 440., 900., 600.);
    shifts.push(model.upward_shift());
    model.update_growth(COLLAPSED_REPLY);
    model.update_upward_shift(Some(0.), CODE, 360., 600., 600.);
    shifts.push(model.upward_shift());
    assert_eq!((shifts, shifted_top), (vec![248., 248., 0.], 752.));
}

#[test]
fn size_does_not_move_up_past_the_top_of_the_editor() {
    let mut model = model();
    model.update_growth(COLLAPSED_REPLY);
    model.update_growth(EXPANDED_REPLY);
    // The editor got shorter than the widget after it showed; its top is 100px below the editor's top.
    model.update_upward_shift(Some(0.), CODE, 440., 900., 400.);
    assert_eq!(model.upward_shift(), 92.);
}

#[test]
fn size_stays_within_the_scrollable_area() {
    let mut model = model();
    model.update_growth(COLLAPSED_REPLY);
    // The code is the last line of a document that can't scroll past its last line.
    assert_eq!(model.top(Some(28.), CODE, 360., 1020.), 652.);
    assert_eq!(model.top(None, CODE, 360., 1020.), 652.);
    assert_eq!(model.top(Some(28.), CODE, 360., 300.), 0.);
}

#[test]
fn size_is_limited_by_the_editor_size_it_captured() {
    let mut model = model();
    let max_size = |model: &SizeModel| (model.width_range(widths(300., 1400.)).max, model.height_range(COLLAPSED_REPLY).max);
    let captured = max_size(&model);
    model.grow_editor_size(Size { width: 800., height: 900. });
    let grown = max_size(&model);
    model.capture_editor_size(Size { width: 800., height: 400. });
    assert_eq!((captured, grown, max_size(&model)), ((1000., 584.), (1000., 884.), (800., 384.)));
}
