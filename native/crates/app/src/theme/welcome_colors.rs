//! Registry defaults of gettingStartedColors.ts, walkThroughUtils.ts and the block quote colors of baseColors.ts, as `registry_default` columns: dark, light,
//! high contrast, high contrast light.

use super::ColorValue::{self, *};

pub(super) fn welcome_default(key: &str) -> Option<[ColorValue; 4]> {
    Some(match key {
        "welcomePage.tileBackground" => [Key("editorWidget.background"), Key("editorWidget.background"), Hex("#000"), Key("editorWidget.background")],
        "welcomePage.tileHoverBackground" => [Lighten(&Key("editorWidget.background"), 0.2), Darken(&Key("editorWidget.background"), 0.1), None, None],
        "welcomePage.tileBorder" => [Hex("#ffffff1a"), Hex("#0000001a"), Key("contrastBorder"), Key("contrastBorder")],
        "welcomePage.progress.background" => [Key("input.background"), Key("input.background"), Key("input.background"), Key("input.background")],
        "welcomePage.progress.foreground" => [Key("textLink.foreground"), Key("textLink.foreground"), Key("textLink.foreground"), Key("textLink.foreground")],
        "walkthrough.stepTitle.foreground" => [Hex("#ffffff"), Hex("#000000"), None, None],
        "walkThrough.embeddedEditorBackground" => [Hex("#00000066"), Hex("#f4f4f4"), None, None],
        "textBlockQuote.background" => [Hex("#222222"), Hex("#f2f2f2"), None, Hex("#F2F2F2")],
        "textBlockQuote.border" => [Hex("#007acc80"), Hex("#007acc80"), Hex("#ffffff"), Hex("#292929")],
        _ => return Option::None,
    })
}
