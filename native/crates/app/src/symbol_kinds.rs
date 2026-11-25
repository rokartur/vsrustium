//! languages.ts `SymbolKinds.toIcon` and the symbolIcons.css color of each icon, for the views that draw
//! document symbols.

/// `(SVG file of the kind's codicon glyph, color key of the codicon's class)` per `SymbolKind`, in enum order.
const KINDS: [(&str, &str); 26] = [
    ("symbol-file", "symbolIcon.fileForeground"),
    ("symbol-namespace", "symbolIcon.moduleForeground"),
    ("symbol-namespace", "symbolIcon.namespaceForeground"),
    ("symbol-namespace", "symbolIcon.packageForeground"),
    ("symbol-class", "symbolIcon.classForeground"),
    ("symbol-method", "symbolIcon.methodForeground"),
    ("symbol-property", "symbolIcon.propertyForeground"),
    ("symbol-field", "symbolIcon.fieldForeground"),
    ("symbol-method", "symbolIcon.constructorForeground"),
    ("symbol-enum", "symbolIcon.enumeratorForeground"),
    ("symbol-interface", "symbolIcon.interfaceForeground"),
    ("symbol-method", "symbolIcon.functionForeground"),
    ("symbol-variable", "symbolIcon.variableForeground"),
    ("symbol-constant", "symbolIcon.constantForeground"),
    ("symbol-string", "symbolIcon.stringForeground"),
    ("symbol-numeric", "symbolIcon.numberForeground"),
    ("symbol-boolean", "symbolIcon.booleanForeground"),
    ("symbol-array", "symbolIcon.arrayForeground"),
    ("symbol-namespace", "symbolIcon.objectForeground"),
    ("symbol-key", "symbolIcon.keyForeground"),
    ("symbol-boolean", "symbolIcon.nullForeground"),
    ("symbol-enum-member", "symbolIcon.enumeratorMemberForeground"),
    ("symbol-structure", "symbolIcon.structForeground"),
    ("symbol-event", "symbolIcon.eventForeground"),
    ("symbol-operator", "symbolIcon.operatorForeground"),
    ("symbol-parameter", "symbolIcon.typeParameterForeground"),
];

/// The SVG of a kind's codicon; an unknown kind gets `symbol-property`, as `toIcon` falls back to it.
pub fn icon_file(kind: u64) -> &'static str {
    KINDS.get(kind as usize).map_or("symbol-property", |kind| kind.0)
}

pub fn color_key(kind: u64) -> &'static str {
    KINDS.get(kind as usize).map_or("symbolIcon.propertyForeground", |kind| kind.1)
}
