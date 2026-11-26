//! Registry defaults of the notebook keys (notebookEditorWidget.ts), as `registry_default` columns: dark, light,
//! high contrast, high contrast light.

use super::ColorValue::{self, *};

pub(super) fn notebook_default(key: &str) -> Option<[ColorValue; 4]> {
    const CELL_BORDER: ColorValue = Transparent(&Key("list.inactiveSelectionBackground"), 1.);
    let same = |value: ColorValue| [value, value, value, value];
    Some(match key {
        "notebook.cellBorderColor" => [CELL_BORDER, CELL_BORDER, Key("panel.border"), Key("panel.border")],
        "notebook.focusedEditorBorder" | "notebook.focusedCellBorder" | "notebook.cellInsertionIndicator" => same(Key("focusBorder")),
        "notebook.inactiveFocusedCellBorder" => same(Key("notebook.cellBorderColor")),
        "notebookStatusSuccessIcon.foreground" | "notebookEditorOverviewRuler.runningCellForeground" => same(Key("debugIcon.startForeground")),
        "notebookStatusErrorIcon.foreground" => same(Key("errorForeground")),
        "notebookStatusRunningIcon.foreground" => same(Key("foreground")),
        "notebook.cellToolbarSeparator" => [Hex("#80808059"), Hex("#80808059"), Key("contrastBorder"), Key("contrastBorder")],
        "notebook.selectedCellBackground" => [Key("list.inactiveSelectionBackground"), Key("list.inactiveSelectionBackground"), None, None],
        "notebook.selectedCellBorder" => [Key("notebook.cellBorderColor"), Key("notebook.cellBorderColor"), Key("contrastBorder"), Key("contrastBorder")],
        "notebook.inactiveSelectedCellBorder" => [None, None, Key("focusBorder"), Key("focusBorder")],
        "notebook.cellStatusBarItemHoverBackground" => [Hex("#FFFFFF26"), Hex("#00000014"), Hex("#FFFFFF26"), Hex("#00000014")],
        "notebook.symbolHighlightBackground" => [Hex("#ffffff0b"), Hex("#fdff0033"), None, None],
        "notebook.cellEditorBackground" => [Key("sideBar.background"), Key("sideBar.background"), None, None],
        // `editorPane.background` defaults to the editor background.
        "notebook.editorBackground" => [Key("editor.background"), Key("editor.background"), None, None],
        _ => return Option::None,
    })
}
