//! Registry defaults of the debug keys: debugColors.ts, breakpointEditorContribution.ts,
//! callStackEditorContribution.ts, statusbarColorProvider.ts and testing/browser/theme.ts, as `registry_default` columns: dark, light,
//! high contrast, high contrast light.

use super::ColorValue::{self, *};

pub(super) fn debug_default(key: &str) -> Option<[ColorValue; 4]> {
    const TOOLBAR_BLUE: [ColorValue; 4] = [Hex("#75BEFF"), Hex("#007ACC"), Hex("#75BEFF"), Hex("#007ACC")];
    const RED: [ColorValue; 4] = [Hex("#F48771"), Hex("#A1260D"), Hex("#F48771"), Hex("#A1260D")];
    const GREEN: [ColorValue; 4] = [Hex("#89D185"), Hex("#388A34"), Hex("#89D185"), Hex("#388A34")];
    Some(match key {
        "debugToolBar.background" => [Hex("#333333"), Hex("#F3F3F3"), Hex("#000000"), Hex("#FFFFFF")],
        "debugIcon.startForeground" | "debugIcon.restartForeground" => GREEN,
        "debugIcon.pauseForeground" | "debugIcon.stepOverForeground" | "debugIcon.stepIntoForeground" | "debugIcon.stepOutForeground" | "debugIcon.continueForeground" | "debugIcon.stepBackForeground" | "debugIcon.reverseContinueForeground" => TOOLBAR_BLUE,
        "debugIcon.stopForeground" | "debugIcon.disconnectForeground" => RED,
        "debugTokenExpression.name" => [Hex("#c586c0"), Hex("#9b46b0"), Key("foreground"), Key("foreground")],
        "debugTokenExpression.type" => [Hex("#4A90E2"), Hex("#4A90E2"), Key("foreground"), Key("foreground")],
        "debugTokenExpression.value" => [Hex("#cccccc99"), Hex("#6c6c6ccc"), Key("foreground"), Key("foreground")],
        "debugTokenExpression.string" => [Hex("#ce9178"), Hex("#a31515"), Hex("#f48771"), Hex("#a31515")],
        "debugTokenExpression.boolean" => [Hex("#4e94ce"), Hex("#0000ff"), Hex("#75bdfe"), Hex("#0000ff")],
        "debugTokenExpression.number" => [Hex("#b5cea8"), Hex("#098658"), Hex("#89d185"), Hex("#098658")],
        "debugTokenExpression.error" => [Hex("#f48771"), Hex("#e51400"), Hex("#f48771"), Hex("#e51400")],
        "debugView.exceptionLabelForeground" => [Key("foreground"), Hex("#FFF"), Key("foreground"), Key("foreground")],
        "debugView.exceptionLabelBackground" => [Hex("#6C2022"), Hex("#A31515"), Hex("#6C2022"), Hex("#A31515")],
        "debugView.stateLabelForeground" | "debugConsole.sourceForeground" | "debugConsoleInput.foreground" | "debugConsoleInputIcon.foreground" => [Key("foreground"), Key("foreground"), Key("foreground"), Key("foreground")],
        "debugView.stateLabelBackground" => [Hex("#88888844"), Hex("#88888844"), Hex("#88888844"), Hex("#88888844")],
        "debugView.valueChangedHighlight" => [Hex("#569CD6"), Hex("#569CD6"), Hex("#569CD6"), Hex("#569CD6")],
        "debugConsole.infoForeground" => [Key("editorInfo.foreground"), Key("editorInfo.foreground"), Key("foreground"), Key("foreground")],
        "debugConsole.warningForeground" => [Key("editorWarning.foreground"), Key("editorWarning.foreground"), Hex("#008000"), Key("editorWarning.foreground")],
        "debugConsole.errorForeground" => [Key("errorForeground"), Key("errorForeground"), Key("errorForeground"), Key("errorForeground")],
        "debugIcon.breakpointForeground" => [Hex("#E51400"), Hex("#E51400"), Hex("#E51400"), Hex("#E51400")],
        "debugIcon.breakpointDisabledForeground" | "debugIcon.breakpointUnverifiedForeground" => [Hex("#848484"), Hex("#848484"), Hex("#848484"), Hex("#848484")],
        "debugIcon.breakpointCurrentStackframeForeground" => [Hex("#FFCC00"), Hex("#BE8700"), Hex("#FFCC00"), Hex("#BE8700")],
        "debugIcon.breakpointStackframeForeground" => [Hex("#89D185"), Hex("#89D185"), Hex("#89D185"), Hex("#89D185")],
        "editor.stackFrameHighlightBackground" => [Hex("#ffff0033"), Hex("#ffff6673"), Hex("#ffff0033"), Hex("#ffff6673")],
        "editor.focusedStackFrameHighlightBackground" => [Hex("#7abd7a4d"), Hex("#cee7ce73"), Hex("#7abd7a4d"), Hex("#cee7ce73")],
        "statusBar.debuggingBackground" => [Hex("#CC6633"), Hex("#CC6633"), Hex("#BA592C"), Hex("#B5200D")],
        "statusBar.debuggingForeground" => [Key("statusBar.foreground"), Key("statusBar.foreground"), Key("statusBar.foreground"), Hex("#FFFFFF")],
        "testing.iconFailed" | "testing.iconErrored" => [Key("list.errorForeground"), Key("list.errorForeground"), Key("list.errorForeground"), Key("list.errorForeground")],
        "testing.iconPassed" | "testing.runAction" => [Hex("#73c991"), Hex("#73c991"), Hex("#73c991"), Hex("#007100")],
        "testing.iconQueued" => [Key("list.warningForeground"), Key("list.warningForeground"), Key("list.warningForeground"), Key("list.warningForeground")],
        "testing.iconUnset" | "testing.iconSkipped" => [Hex("#848484"), Hex("#848484"), Hex("#848484"), Hex("#848484")],
        "debugExceptionWidget.border" => [Hex("#a31515"), Hex("#a31515"), Hex("#a31515"), Hex("#a31515")],
        "debugExceptionWidget.background" => [Hex("#420b0d"), Hex("#f1dfde"), Hex("#420b0d"), Hex("#f1dfde")],
        "list.errorForeground" => [Hex("#F88070"), Hex("#B01011"), ColorValue::None, ColorValue::None],
        "list.warningForeground" => [Hex("#CCA700"), Hex("#855F00"), ColorValue::None, ColorValue::None],
        _ => return Option::None,
    })
}
