//! defaultDocumentColorsComputer.test.ts, and the presentations of defaultDocumentColorProvider.ts.

use super::*;

fn colors(text: &str) -> Vec<ColorInformation> {
    compute_default_document_colors(&text.split('\n').map(String::from).collect::<Vec<_>>())
}

#[test]
fn hex_colors_in_strings_are_found() {
    let found = colors("const color = '#ff0000';");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].color, [1., 0., 0., 1.]);
    assert_eq!((found[0].start, found[0].end), (Position::new(0, 15), Position::new(0, 22)));
    assert_eq!(colors("const color = \"#00ff00\";")[0].color, [0., 1., 0., 1.]);
    let three = colors("const colors = ['#ff0000', '#00ff00', '#0000ff'];");
    assert_eq!(three.iter().map(|color| color.color).collect::<Vec<_>>(), [[1., 0., 0., 1.], [0., 1., 0., 1.], [0., 0., 1., 1.]]);
}

#[test]
fn existing_functionality_still_works() {
    for text in ["const color = ' #ff0000';", "#ff0000", "  #ff0000"] {
        assert_eq!(colors(text).len(), 1, "{text}");
    }
    assert_eq!(colors("const color = '#ff0000ff';")[0].color, [1., 0., 0., 1.]);
    // Not after a word char: no color.
    assert!(colors("a#ff0000").is_empty());
}

#[test]
fn hsl_and_hsla_with_decimals_and_level_4_syntax_are_found() {
    for text in [
        "hsl(253, 100.00%, 47.10%)",
        "hsl(253, 100%, 47.10%)",
        "hsl(0, 83.60%, 47.80%)",
        "hsl(253.5, 100%, 50%)",
        "hsl(360.0, 50%, 50%)",
        "hsl(100.5, 50.5%, 50.5%)",
        "hsl(0.5, 50%, 50%)",
        "hsl(359.9, 100%, 50%)",
        "hsla(253.5, 100%, 50%, 0.5)",
        "hsla(360.0, 50.5%, 50.5%, 1)",
        "hsla(0.5, 50%, 50%, 0.25)",
        "hsl(253 100% 50%)",
        "hsl(253.5 100% 50%)",
        "hsla(253 100% 50% / 0.5)",
        "hsla(253.5 100% 50% / 0.5)",
        "hsla(253 100% 50% / 1)",
    ] {
        assert_eq!(colors(&format!("const color = {text};")).len(), 1, "{text}");
    }
}

#[test]
fn rgb_and_rgba_in_both_syntaxes_are_found() {
    for text in ["rgb(255 0 0)", "rgb(128 128 128)", "rgba(255 0 0 / 0.5)", "rgba(128 128 128 / 0.8)", "rgba(255 0 0 / 1)", "rgb(255, 0, 0)", "rgba(255, 0, 0, 0.5)"] {
        assert_eq!(colors(&format!("const color = {text};")).len(), 1, "{text}");
    }
    assert_eq!(colors("x\n  rgba(255, 0, 0, 0.5)")[0].start, Position::new(1, 2));
    assert_eq!(colors("rgba(255, 0, 0, 0.5)")[0].color, [1., 0., 0., 0.5]);
    assert!(colors("rgb(256, 0, 0)").is_empty());
}

#[test]
fn presentations_are_rgb_hsl_and_hex() {
    assert_eq!(default_color_presentations([1., 0., 0., 1.]), ["rgb(255, 0, 0)", "hsl(0, 100%, 50%)", "#ff0000"]);
    assert_eq!(default_color_presentations([1., 0., 0., 0.5]), ["rgba(255, 0, 0, 0.5)", "hsla(0, 100%, 50%, 0.50)", "#ff000080"]);
}
