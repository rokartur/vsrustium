use gpui::{Bounds, point, size};

use super::*;

#[test]
fn distance_to_rectangle_is_zero_inside_and_euclidean_outside() {
    let bounds = Bounds { origin: point(10., 10.), size: size(20., 10.) };
    assert_eq!(distance_to_rectangle(point(15., 15.), bounds), 0.);
    assert_eq!(distance_to_rectangle(point(5., 15.), bounds), 5.);
    assert_eq!(distance_to_rectangle(point(33., 24.), bounds), 5.);
}

#[test]
fn code_points_format_as_the_reference_writes_them() {
    assert_eq!(code_point_to_hex('\u{2013}'), "U+2013");
    assert_eq!(code_point_to_hex('\u{a0}'), "U+00a0");
    assert_eq!(format_code_point_markdown('\u{2013}'), "`U+2013` \"`\u{2013}`\"");
    assert_eq!(format_code_point_markdown('\u{200b}'), "`U+200b`");
}

#[test]
fn encode_uri_component_keeps_the_unreserved_marks() {
    assert_eq!(encode_uri_component("{\"a\":1}"), "%7B%22a%22%3A1%7D");
    assert_eq!(encode_uri_component("é-_.!~*'()"), "%C3%A9-_.!~*'()");
}
