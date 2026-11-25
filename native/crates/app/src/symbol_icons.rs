//! symbolIcons.ts and symbolIcons.css: the codicon and color of each `CompletionItemKind` and `SymbolKind`, and
//! the registry defaults of the suggest widget's colors (suggestWidget.ts).

use crate::theme::ColorValue::{self, Hex, Key, Transparent};

/// `CompletionItemKinds.toIcon`: the codicon's SVG of a completion kind and the color key its CSS class takes.
pub fn completion_kind_icon(kind: u64) -> (&'static str, Option<&'static str>) {
    let name = match kind {
        0 => "symbol-method",
        1 => "symbol-function",
        2 => "symbol-constructor",
        3 => "symbol-field",
        4 => "symbol-variable",
        5 => "symbol-class",
        6 => "symbol-struct",
        7 => "symbol-interface",
        8 => "symbol-module",
        10 => "symbol-event",
        11 => "symbol-operator",
        12 => "symbol-unit",
        13 => "symbol-value",
        14 => "symbol-constant",
        15 => "symbol-enum",
        16 => "symbol-enum-member",
        17 => "symbol-keyword",
        18 => "symbol-text",
        19 => "symbol-color",
        20 => "symbol-file",
        21 => "symbol-reference",
        22 => "symbol-customcolor",
        23 => "symbol-folder",
        24 => "symbol-type-parameter",
        25 => "account",
        26 => "issues",
        27 => "tools",
        28 => "symbol-snippet",
        _ => "symbol-property",
    };
    (svg_name(name), color_key(name))
}

/// codiconsLibrary.ts registers aliases under the code point of another icon; the SVG carries that icon's name.
pub fn svg_name(codicon: &'static str) -> &'static str {
    match codicon {
        "symbol-function" | "symbol-constructor" => "symbol-method",
        "symbol-struct" => "symbol-structure",
        "symbol-module" | "symbol-object" | "symbol-package" => "symbol-namespace",
        "symbol-unit" => "symbol-ruler",
        "symbol-value" => "symbol-enum",
        "symbol-text" => "symbol-key",
        "symbol-reference" => "go-to-file",
        "symbol-customcolor" => "add",
        "symbol-folder" => "folder",
        "symbol-type-parameter" => "symbol-parameter",
        "symbol-null" => "symbol-boolean",
        "symbol-number" => "symbol-numeric",
        "wrench" | "wrench-subaction" => "symbol-property",
        name => name,
    }
}

/// `SymbolKinds.toIcon`: the codicon of a `SymbolKind` value, `symbol-property` for an unknown one.
pub fn codicon(kind: u64) -> &'static str {
    const BY_KIND: [&str; 26] = [
        "symbol-file",
        "symbol-module",
        "symbol-namespace",
        "symbol-package",
        "symbol-class",
        "symbol-method",
        "symbol-property",
        "symbol-field",
        "symbol-constructor",
        "symbol-enum",
        "symbol-interface",
        "symbol-function",
        "symbol-variable",
        "symbol-constant",
        "symbol-string",
        "symbol-number",
        "symbol-boolean",
        "symbol-array",
        "symbol-object",
        "symbol-key",
        "symbol-null",
        "symbol-enum-member",
        "symbol-struct",
        "symbol-event",
        "symbol-operator",
        "symbol-type-parameter",
    ];
    BY_KIND.get(kind as usize).copied().unwrap_or("symbol-property")
}

/// symbolIcons.css: the `symbolIcon.*Foreground` key of a `.codicon-symbol-*` class.
pub fn color_key(codicon: &str) -> Option<&'static str> {
    Some(match codicon {
        "symbol-array" => "symbolIcon.arrayForeground",
        "symbol-boolean" => "symbolIcon.booleanForeground",
        "symbol-class" => "symbolIcon.classForeground",
        "symbol-method" => "symbolIcon.methodForeground",
        "symbol-color" => "symbolIcon.colorForeground",
        "symbol-constant" => "symbolIcon.constantForeground",
        "symbol-constructor" => "symbolIcon.constructorForeground",
        "symbol-value" | "symbol-enum" => "symbolIcon.enumeratorForeground",
        "symbol-enum-member" => "symbolIcon.enumeratorMemberForeground",
        "symbol-event" => "symbolIcon.eventForeground",
        "symbol-field" => "symbolIcon.fieldForeground",
        "symbol-file" => "symbolIcon.fileForeground",
        "symbol-folder" => "symbolIcon.folderForeground",
        "symbol-function" => "symbolIcon.functionForeground",
        "symbol-interface" => "symbolIcon.interfaceForeground",
        "symbol-key" => "symbolIcon.keyForeground",
        "symbol-keyword" => "symbolIcon.keywordForeground",
        "symbol-module" => "symbolIcon.moduleForeground",
        "symbol-namespace" => "symbolIcon.namespaceForeground",
        "symbol-null" => "symbolIcon.nullForeground",
        "symbol-number" => "symbolIcon.numberForeground",
        "symbol-object" => "symbolIcon.objectForeground",
        "symbol-operator" => "symbolIcon.operatorForeground",
        "symbol-package" => "symbolIcon.packageForeground",
        "symbol-property" => "symbolIcon.propertyForeground",
        "symbol-reference" => "symbolIcon.referenceForeground",
        "symbol-snippet" => "symbolIcon.snippetForeground",
        "symbol-string" => "symbolIcon.stringForeground",
        "symbol-struct" => "symbolIcon.structForeground",
        "symbol-text" => "symbolIcon.textForeground",
        "symbol-type-parameter" => "symbolIcon.typeParameterForeground",
        "symbol-unit" => "symbolIcon.unitForeground",
        "symbol-variable" => "symbolIcon.variableForeground",
        _ => return None,
    })
}

/// Registry defaults as dark, light, high contrast dark and high contrast light.
pub(crate) fn color_default(key: &str) -> Option<[ColorValue; 4]> {
    const ORANGE: [ColorValue; 4] = [Hex("#EE9D28"), Hex("#D67E00"), Hex("#EE9D28"), Hex("#D67E00")];
    const PURPLE: [ColorValue; 4] = [Hex("#B180D7"), Hex("#652D90"), Hex("#B180D7"), Hex("#652D90")];
    const BLUE: [ColorValue; 4] = [Hex("#75BEFF"), Hex("#007ACC"), Hex("#75BEFF"), Hex("#007ACC")];
    const SUGGEST_FOREGROUND: ColorValue = Key("editorSuggestWidget.foreground");
    let same = |value: ColorValue| Some([value, value, value, value]);
    match key {
        "symbolIcon.classForeground" | "symbolIcon.enumeratorForeground" | "symbolIcon.eventForeground" => Some(ORANGE),
        "symbolIcon.constructorForeground" | "symbolIcon.functionForeground" | "symbolIcon.methodForeground" => Some(PURPLE),
        "symbolIcon.enumeratorMemberForeground" | "symbolIcon.fieldForeground" | "symbolIcon.interfaceForeground" | "symbolIcon.variableForeground" => Some(BLUE),
        _ if key.starts_with("symbolIcon.") => same(Key("foreground")),
        "editorSuggestWidget.background" => same(Key("editorWidget.background")),
        "editorSuggestWidget.border" => same(Key("editorWidget.border")),
        "editorSuggestWidget.foreground" => same(Key("editor.foreground")),
        "editorSuggestWidget.selectedForeground" => Some([Key("quickInputList.focusForeground"), Key("quickInputList.focusForeground"), Key("editorSuggestWidget.background"), Key("editorSuggestWidget.background")]),
        "editorSuggestWidget.selectedIconForeground" => Some([Key("quickInputList.focusIconForeground"), Key("quickInputList.focusIconForeground"), Key("editorSuggestWidget.background"), Key("editorSuggestWidget.background")]),
        "editorSuggestWidget.selectedBackground" => Some([Key("quickInputList.focusBackground"), Key("quickInputList.focusBackground"), Key("editorSuggestWidget.foreground"), Key("editorSuggestWidget.foreground")]),
        "editorSuggestWidget.focusOutline" => same(Key("contrastActiveBorder")),
        "editorSuggestWidget.highlightForeground" => same(Key("list.highlightForeground")),
        "editorSuggestWidget.focusHighlightForeground" => Some([Key("list.focusHighlightForeground"), Key("list.focusHighlightForeground"), Key("editorSuggestWidget.selectedForeground"), Key("editorSuggestWidget.selectedForeground")]),
        "editorSuggestWidgetStatus.foreground" => same(Transparent(&SUGGEST_FOREGROUND, 0.5)),
        "quickInputList.focusIconForeground" => same(Key("list.activeSelectionIconForeground")),
        // editorColors.ts, for snippetSession.css.
        "editor.snippetTabstopHighlightBackground" => Some([Hex("#7C7C7C4D"), Hex("#0A326433"), Hex("#7C7C7C4D"), Hex("#0A326433")]),
        "editor.snippetFinalTabstopHighlightBorder" => Some([Hex("#525252"), Hex("#0A326480"), Hex("#525252"), Hex("#292929")]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_a_drawn_svg_and_a_registered_color() {
        for kind in 0..26 {
            let codicon = codicon(kind);
            assert!(color_default(color_key(codicon).unwrap()).is_some(), "{codicon}");
        }
        assert_eq!(codicon(99), "symbol-property");
        assert_eq!(svg_name(codicon(11)), "symbol-method");
        assert_eq!(color_default("symbolIcon.functionForeground").map(|values| values[0]), Some(Hex("#B180D7")));
        assert_eq!(color_default("symbolIcon.stringForeground").map(|values| values[0]), Some(Key("foreground")));
        assert_eq!(color_default("editor.foreground"), None);
    }
}
