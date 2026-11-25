use super::*;
use crate::editor::word_wrap::{BREAK_AFTER, BREAK_BEFORE, Classifier, WrapOptions, WrappingIndent, line_breaks};

#[test]
fn injections_move_the_later_columns_right() {
    // "ab|cd" with a 3 column hint before `c`.
    let layout = LineLayout::new("abcd", 4, [(2, 3.)]);
    assert_eq!((layout.x(1), layout.x(2), layout.char_x(2), layout.x(3)), (1., 2., 5., 6.));
    // A cursor at the hint draws before it, a range starting there after it, one ending there before it.
    assert_eq!(layout.range(2, 2), (2., 2.));
    assert_eq!(layout.range(2, 4), (5., 7.));
    assert_eq!(layout.range(0, 2), (0., 2.));
    assert_eq!(layout.width(), 7.);
}

#[test]
fn an_injection_at_the_line_end_widens_the_line() {
    let layout = LineLayout::new("ab", 4, [(2, 1.5)]);
    assert_eq!((layout.x(2), layout.width()), (2., 3.5));
}

#[test]
fn a_click_on_an_injection_goes_to_its_column() {
    let layout = LineLayout::new("abcd", 4, [(2, 3.)]);
    assert_eq!(layout.column_at(1.4), 1);
    assert_eq!(layout.column_at(1.6), 2);
    assert_eq!(layout.column_at(3.), 2);
    assert_eq!(layout.column_at(5.4), 2);
    assert_eq!(layout.column_at(5.6), 3);
    assert_eq!(layout.column_at(20.), 4);
    assert_eq!(layout.model_visible(4.), 2.);
    // `columnFromVisibleColumn` takes the left column on a tie.
    assert_eq!(LineLayout::new("\ta", 4, []).column_at(2.), 0);
}

#[test]
fn tab_stops_leave_the_injections_out() {
    let layout = LineLayout::new("a\tb", 4, [(1, 2.)]);
    assert_eq!((layout.char_x(1), layout.char_x(2)), (3., 6.));
}

#[test]
fn an_injection_moves_with_the_text_typed_before_it() {
    let mut editor = crate::editor::edit::tests::editor("let x = f(1);", "\n");
    let hint = |column| InjectedText { position: Position::new(0, column), columns: 3., owner: InjectionOwner::InlayHint, id: 0, at_range_start: true };
    editor.set_injected_text(InjectionOwner::InlayHint, vec![hint(10)]);
    editor.set_selections(vec![crate::editor::Selection::caret(Position::new(0, 0))]);
    editor.type_text("  ");
    assert_eq!(editor.injected_text_on(0)[0].position, Position::new(0, 12));
    // Typed at its own column, the text goes after an injection at its range's start.
    editor.set_selections(vec![crate::editor::Selection::caret(Position::new(0, 12))]);
    editor.type_text("x");
    assert_eq!(editor.injected_text_on(0)[0].position, Position::new(0, 12));
    assert_eq!(editor.line_layout(0).char_x(12), 15.);
    editor.set_injected_text(InjectionOwner::InlayHint, Vec::new());
    assert!(editor.injected_text_on(0).is_empty());
}

#[test]
fn wrapping_counts_the_injected_width_and_breaks_before_it() {
    let options = WrapOptions {
        column: 10,
        viewport_wrapping: false,
        tab_size: 4,
        indent: WrappingIndent::None,
        keep_all: false,
        wrap_on_escaped_line_feeds: false,
        columns_for_full_width_char: 2.,
        two_cell_full_width: false,
        break_before: BREAK_BEFORE.into(),
        break_after: BREAK_AFTER.into(),
    };
    let classifier = Classifier::new(BREAK_BEFORE, BREAK_AFTER);
    assert_eq!(line_breaks(&classifier, "aaaa bbbb", &options, &[]), None);
    // A 4 column hint before `bbbb` makes the line 13 columns: the hint goes to the second view line with it.
    let breaks = line_breaks(&classifier, "aaaa bbbb", &options, &[(5, 4.)]).unwrap();
    assert_eq!(breaks.offsets, [5, 9]);
}
