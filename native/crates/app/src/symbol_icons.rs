//! symbolIcons.ts and languages.ts `SymbolKinds`: the codicon of each `SymbolKind`, the SVG that draws it and the
//! `symbolIcon.*Foreground` color symbolIcons.css gives it.

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

/// The SVG of a codicon: codicons' mapping.json draws aliases with one glyph (`symbol-function` is `symbol-method`).
pub fn svg_name(codicon: &str) -> &str {
    match codicon {
        "symbol-folder" => "folder",
        "symbol-module" | "symbol-package" | "symbol-object" => "symbol-namespace",
        "symbol-function" | "symbol-constructor" => "symbol-method",
        "symbol-null" => "symbol-boolean",
        "symbol-number" => "symbol-numeric",
        "symbol-struct" => "symbol-structure",
        "symbol-type-parameter" => "symbol-parameter",
        "symbol-text" => "symbol-key",
        "symbol-reference" => "go-to-file",
        "symbol-value" => "symbol-enum",
        "symbol-unit" => "symbol-ruler",
        _ => codicon,
    }
}

/// symbolIcons.css: the theme color of a symbol codicon.
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

/// symbolIcons.ts `registerColor`: a `symbolIcon.*Foreground` key's dark, light, hcDark and hcLight hex, or `None`
/// for the ones that default to `foreground`. Keys of other registries are `None` outside.
pub fn color_default(key: &str) -> Option<Option<[&'static str; 4]>> {
    const ORANGE: [&str; 4] = ["#EE9D28", "#D67E00", "#EE9D28", "#D67E00"];
    const PURPLE: [&str; 4] = ["#B180D7", "#652D90", "#B180D7", "#652D90"];
    const BLUE: [&str; 4] = ["#75BEFF", "#007ACC", "#75BEFF", "#007ACC"];
    let name = key.strip_prefix("symbolIcon.")?.strip_suffix("Foreground")?;
    Some(match name {
        "class" | "enumerator" | "event" => Some(ORANGE),
        "constructor" | "function" | "method" => Some(PURPLE),
        "enumeratorMember" | "field" | "interface" | "variable" => Some(BLUE),
        "array" | "boolean" | "color" | "constant" | "file" | "folder" | "key" | "keyword" | "module" | "namespace" | "null" | "number" | "object" | "operator" | "package" | "property" | "reference" | "snippet" | "string" | "struct" | "text" | "typeParameter" | "unit" => None,
        _ => return None,
    })
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
        assert_eq!(color_default("symbolIcon.functionForeground"), Some(Some(["#B180D7", "#652D90", "#B180D7", "#652D90"])));
        assert_eq!(color_default("symbolIcon.stringForeground"), Some(None));
        assert_eq!(color_default("editor.foreground"), None);
    }
}
