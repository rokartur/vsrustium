use super::*;

#[test]
fn hsva_and_rgba_convert_as_color_ts() {
    let red = PickerColor::from_rgba([255, 0, 0], 1.);
    assert_eq!(red.hsva, [0., 1., 1., 1.]);
    let picked = PickerColor::from_hsva(120., 0.5, 0.5, 0.4567);
    assert_eq!((picked.rgb, picked.alpha), ([64, 128, 64], 0.457));
    // A gray keeps the hue it was picked with, as `Color` keeps its `HSVA`.
    assert_eq!(PickerColor::from_hsva(200., 0., 0.5, 1.).hsva[0], 200.);
    assert_eq!(PickerColor::from_rgba([0, 0, 255], 1.).hsva, [240., 1., 1., 1.]);
}

#[test]
fn the_presentation_is_guessed_from_the_original_text() {
    let color = PickerColor::from_rgba([255, 0, 0], 1.);
    let mut picker = HoverColorPicker { model_id: 1, start: Position::default(), end: Position::default(), handle: None, original: color, color, presentations: Vec::new(), index: 0, drag: None, updated_by_picker: false, standalone: false, bounds: Rc::default() };
    picker.set_presentations(default_color_presentations(color.channels()).into_iter().map(|label| json!({ "label": label })).collect());
    picker.guess_presentation("#FF0000");
    assert_eq!(picker.presentation_label(), "#ff0000");
    picker.guess_presentation("hsl(1, 2%, 3%)");
    assert_eq!(picker.presentation_label(), "hsl(0, 100%, 50%)");
    picker.set_presentations(vec![json!({ "label": "only" })]);
    assert_eq!(picker.index, 0);
}
