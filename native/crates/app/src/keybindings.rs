//! Keybindings: the keybinding parser and US layout labels (base/common/keybindingParser.ts, keybindings.ts,
//! keybindingLabels.ts, platform/keybinding/common/usLayoutResolvedKeybinding.ts), the resolver
//! (keybindingResolver.ts) and the default, extension and user keybindings it is built from
//! (keybindingsRegistry.ts, workbench/services/keybinding/browser/keybindingService.ts).

mod key_codes;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::Path;

use exthost::scan::ScannedExtension;
use serde_json::Value;

use crate::contextkey::{Context, Expr, implies};
use key_codes::{KEY_CODES, SCAN_CODES};

/// One key press with its modifiers, as `KeyCodeChord`; `key` indexes `KEY_CODES`, 0 is `KeyCode.Unknown`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
    pub key: usize,
}

impl Chord {
    /// The chord GPUI reports for a key down; GPUI's key names are VS Code's user settings labels.
    pub fn from_keystroke(keystroke: &gpui::Keystroke) -> Chord {
        let modifiers = keystroke.modifiers;
        // GPUI reports Cmd+Shift+. as Cmd+> without Shift; keybindings name the unshifted US key, as upstream's key codes do.
        let shifted = US_SHIFTED_SYMBOLS.chars().position(|symbol| keystroke.key == symbol.to_string());
        let (key, shift) = match shifted {
            Some(index) => (US_UNSHIFTED_SYMBOLS.chars().nth(index).unwrap().to_string(), true),
            None => (keystroke.key.clone(), modifiers.shift),
        };
        Chord { ctrl: modifiers.control, shift, alt: modifiers.alt, meta: modifiers.platform, key: key_from_user_settings(&key) }
    }

    /// `isModifierKey`: such a chord cannot be dispatched in the keydown phase.
    pub fn is_modifier_key(&self) -> bool {
        matches!(KEY_CODES[self.key].0, "unknown" | "Ctrl" | "Meta" | "Alt" | "Shift")
    }

    fn is_duplicate_modifier_case(&self) -> bool {
        let key = KEY_CODES[self.key].0;
        (self.ctrl && key == "Ctrl") || (self.shift && key == "Shift") || (self.alt && key == "Alt") || (self.meta && key == "Meta")
    }

    /// The macOS UI label: modifier symbols in Ctrl-Shift-Alt-Cmd order, arrows as arrows.
    pub fn label(&self) -> String {
        let key = match KEY_CODES[self.key].0 {
            _ if self.is_duplicate_modifier_case() => "",
            "LeftArrow" => "←",
            "UpArrow" => "↑",
            "RightArrow" => "→",
            "DownArrow" => "↓",
            key => key,
        };
        self.with_modifiers(["\u{2303}", "⇧", "⌥", "⌘"], "", key)
    }

    /// The boxes `KeybindingLabel.renderChord` draws: one per modifier, then the key.
    pub fn key_caps(&self) -> Vec<String> {
        let mut caps = Vec::new();
        for (symbol, pressed) in [("\u{2303}", self.ctrl), ("⇧", self.shift), ("⌥", self.alt), ("⌘", self.meta)] {
            if pressed {
                caps.push(symbol.to_string());
            }
        }
        let key = Chord { ctrl: false, shift: false, alt: false, meta: false, key: self.key }.label();
        if !key.is_empty() && !self.is_duplicate_modifier_case() {
            caps.push(key);
        }
        caps
    }

    pub fn aria_label(&self) -> String {
        let key = if self.is_duplicate_modifier_case() { "" } else { KEY_CODES[self.key].0 };
        self.with_modifiers(["Control", "Shift", "Option", "Command"], "+", key)
    }

    /// `keyAriaLabel` of a `ResolvedChord`.
    pub fn key_aria_label(&self) -> &'static str {
        KEY_CODES[self.key].0
    }

    pub fn user_settings_label(&self) -> String {
        let key = if self.is_duplicate_modifier_case() { String::new() } else { KEY_CODES[self.key].1.to_lowercase() };
        self.with_modifiers(["ctrl", "shift", "alt", "cmd"], "+", &key)
    }

    /// The source `gpui::KeyBinding` parses, `shift-cmd-s`; `None` where Electron has no accelerator either.
    pub fn gpui_keystroke(&self) -> Option<String> {
        if self.is_modifier_key() {
            return None;
        }
        Some(self.with_modifiers(["ctrl", "shift", "alt", "cmd"], "-", &KEY_CODES[self.key].1.to_lowercase()))
    }

    /// `_simpleAsString`.
    fn with_modifiers(&self, labels: [&str; 4], separator: &str, key: &str) -> String {
        let pressed = [self.ctrl, self.shift, self.alt, self.meta];
        let mut parts: Vec<&str> = labels.iter().zip(pressed).filter(|(_, pressed)| *pressed).map(|(label, _)| *label).collect();
        if !key.is_empty() {
            parts.push(key);
        }
        parts.join(separator)
    }
}

const US_SHIFTED_SYMBOLS: &str = "~!@#$%^&*()_+{}|:\"<>?";
const US_UNSHIFTED_SYMBOLS: &str = "`1234567890-=[]\\;',./";

/// `KeyCodeUtils.fromUserSettings`: the US labels first, then the general ones; a later key code wins a shared label.
fn key_from_user_settings(label: &str) -> usize {
    let label = label.to_lowercase();
    let us = KEY_CODES.iter().rposition(|key| key.1.to_lowercase() == label);
    us.or_else(|| KEY_CODES.iter().rposition(|key| key.2.to_lowercase() == label)).unwrap_or(0)
}

/// `KeybindingParser.parseKeybinding` resolved on a US layout: `[code]` scan codes become key codes, and a chord
/// whose scan code has no key code leaves no chords at all (`toEmptyArrayIfContainsNull`).
pub fn parse_keybinding(input: &str) -> Vec<Chord> {
    let mut input = input.to_string();
    let mut chords = Vec::new();
    while !input.is_empty() {
        let (chord, remains) = parse_chord(&input);
        let Some(chord) = chord else { return Vec::new() };
        chords.push(chord);
        input = remains;
    }
    chords
}

fn parse_chord(input: &str) -> (Option<Chord>, String) {
    let mut input = input.to_lowercase().trim().to_string();
    let mut chord = Chord { ctrl: false, shift: false, alt: false, meta: false, key: 0 };
    loop {
        let modifier = ["ctrl", "shift", "alt", "meta", "win", "cmd"].into_iter().find(|name| {
            input.strip_prefix(name).is_some_and(|rest| rest.starts_with('+') || rest.starts_with('-'))
        });
        let Some(modifier) = modifier else { break };
        match modifier {
            "ctrl" => chord.ctrl = true,
            "shift" => chord.shift = true,
            "alt" => chord.alt = true,
            _ => chord.meta = true,
        }
        input = input[modifier.len() + 1..].to_string();
    }
    let (key, remains) = match input.find(' ') {
        Some(space) if space > 0 => (input[..space].to_string(), input[space..].to_string()),
        _ => (input.clone(), String::new()),
    };
    if let Some(scan_code) = key.strip_prefix('[').and_then(|key| key.strip_suffix(']')) {
        let key = SCAN_CODES.iter().find(|(name, _)| *name == scan_code).map(|(_, key)| *key);
        return (key.map(|key| Chord { key, ..chord }), remains);
    }
    chord.key = key_from_user_settings(&key);
    (Some(chord), remains)
}

/// Labels of a whole keybinding, chords separated by a space (`ModifierLabelProvider.toLabel`).
pub fn label(chords: &[Chord]) -> String {
    chords.iter().map(Chord::label).collect::<Vec<_>>().join(" ")
}

pub fn aria_label(chords: &[Chord]) -> String {
    chords.iter().map(Chord::aria_label).collect::<Vec<_>>().join(" ")
}

pub fn user_settings_label(chords: &[Chord]) -> String {
    chords.iter().map(Chord::user_settings_label).collect::<Vec<_>>().join(" ")
}

/// `getElectronAccelerator`: none for chords, a lone duplicate modifier or a numpad key (Electron draws those as
/// the plain key), else `toElectronAccelerator`'s key name after the modifiers.
pub fn electron_accelerator(chords: &[Chord]) -> Option<String> {
    let [chord] = chords else { return None };
    if chord.is_duplicate_modifier_case() || (NUMPAD_0..=NUMPAD_DIVIDE).contains(&chord.key) {
        return None;
    }
    let (name, user_settings_name, _) = KEY_CODES[chord.key];
    let key = match name {
        "LeftArrow" | "UpArrow" | "RightArrow" | "DownArrow" | "Del" => user_settings_name,
        name => name,
    };
    Some(chord.with_modifiers(["Ctrl", "Shift", "Alt", "Cmd"], "+", key))
}

const NUMPAD_0: usize = 98;
const NUMPAD_DIVIDE: usize = 113;

/// The scan code of the key's US position, as `SCAN_CODES` names it (`keya`, `arrowup`).
pub fn us_scan_code(key: usize) -> Option<&'static str> {
    SCAN_CODES.iter().find(|(_, scan_key)| *scan_key == key).map(|(name, _)| *name)
}

/// `ResolvedKeybindingItem`. An empty `command` unbinds the key; a leading `-` removes a default keybinding.
#[derive(Clone, Debug)]
pub struct Item {
    pub keybinding: Vec<Chord>,
    /// The chords dispatch matches on; empty when any chord is a lone modifier or unknown.
    pub chords: Vec<Chord>,
    pub bubble: bool,
    pub command: String,
    pub args: Option<Value>,
    pub when: Option<Expr>,
    pub is_default: bool,
    /// The extension that contributed a default keybinding (`extensionId`).
    pub extension: Option<ExtensionSource>,
    /// `"systemWide": true` of a keybindings.json entry.
    pub system_wide: bool,
}

/// The source of an extension's keybinding: its identifier and the `displayName` the Keyboard Shortcuts editor shows.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtensionSource {
    pub id: String,
    pub display_name: Option<String>,
}

impl ExtensionSource {
    pub fn of(extension: &ScannedExtension) -> ExtensionSource {
        ExtensionSource { id: extension.id.clone(), display_name: extension.description["displayName"].as_str().map(str::to_string) }
    }
}

impl Item {
    pub fn new(keybinding: Vec<Chord>, command: &str, args: Option<Value>, when: Option<Expr>, is_default: bool) -> Item {
        let bubble = command.starts_with('^');
        let command = command.strip_prefix('^').unwrap_or(command).to_string();
        let chords = if keybinding.iter().any(Chord::is_modifier_key) { Vec::new() } else { keybinding.clone() };
        Item { keybinding, chords, bubble, command, args, when, is_default, extension: None, system_wide: false }
    }
}

#[derive(Debug, PartialEq)]
pub enum Resolution {
    NoMatchingKb,
    MoreChordsNeeded,
    /// An empty command means the key is bound to nothing.
    KbFound { command: String, args: Option<Value>, bubble: bool },
}

/// `KeybindingResolver`: the keybindings after removals, indexed by their first chord.
pub struct Resolver {
    keybindings: Vec<Item>,
    map: HashMap<Chord, Vec<usize>>,
    lookup: HashMap<String, Vec<usize>>,
}

impl Resolver {
    pub fn new(defaults: Vec<Item>, overrides: Vec<Item>) -> Resolver {
        let keybindings = Resolver::handle_removals(defaults.into_iter().chain(overrides).collect());
        let mut resolver = Resolver { keybindings, map: HashMap::new(), lookup: HashMap::new() };
        for index in 0..resolver.keybindings.len() {
            let item = &resolver.keybindings[index];
            if item.chords.is_empty() || item.when.as_ref().and_then(Expr::substitute_constants).is_some_and(|when| when.is_false()) {
                continue;
            }
            resolver.add_key_press(index);
        }
        resolver
    }

    fn is_targeted_for_removal(default: &Item, keypress: &[Chord], when: Option<&Expr>) -> bool {
        if keypress.iter().enumerate().any(|(i, chord)| default.chords.get(i) != Some(chord)) {
            return false;
        }
        if let Some(when) = when.filter(|when| !when.is_true()) {
            let Some(default_when) = &default.when else { return false };
            let default_when = default_when.substitute_constants();
            let removal_when = when.substitute_constants();
            if !Resolver::when_is_entirely_included(default_when.as_ref(), removal_when.as_ref()) {
                return false;
            }
        }
        true
    }

    /// Drops `-command` rules and the default keybindings they target.
    pub fn handle_removals(rules: Vec<Item>) -> Vec<Item> {
        let mut removals: HashMap<&str, Vec<&Item>> = HashMap::new();
        for rule in &rules {
            if let Some(command) = rule.command.strip_prefix('-') {
                removals.entry(command).or_default().push(rule);
            }
        }
        if removals.is_empty() {
            return rules;
        }
        let kept: Vec<bool> = rules
            .iter()
            .map(|rule| {
                if rule.command.starts_with('-') {
                    return false;
                }
                match removals.get(rule.command.as_str()) {
                    Some(targeting) if rule.is_default && !rule.command.is_empty() => {
                        !targeting.iter().any(|removal| Resolver::is_targeted_for_removal(rule, &removal.chords, removal.when.as_ref()))
                    }
                    _ => true,
                }
            })
            .collect();
        rules.into_iter().zip(kept).filter(|(_, kept)| *kept).map(|(rule, _)| rule).collect()
    }

    fn add_key_press(&mut self, index: usize) {
        let item = &self.keybindings[index];
        let conflicts = self.map.entry(item.chords[0]).or_default();
        for &conflict_index in conflicts.iter().rev() {
            let conflict = &self.keybindings[conflict_index];
            if conflict.command == item.command {
                continue;
            }
            let is_shorter_prefix = (1..conflict.chords.len().min(item.chords.len())).all(|i| conflict.chords[i] == item.chords[i]);
            if is_shorter_prefix && Resolver::when_is_entirely_included(conflict.when.as_ref(), item.when.as_ref()) {
                if let Some(entries) = self.lookup.get_mut(&conflict.command) {
                    entries.retain(|entry| *entry != conflict_index);
                }
            }
        }
        conflicts.push(index);
        if !item.command.is_empty() {
            self.lookup.entry(item.command.clone()).or_default().push(index);
        }
    }

    /// True when `a` implies `b`; a missing or `true` `b` includes everything, and only such a `b` includes `a` = `true`.
    pub fn when_is_entirely_included(a: Option<&Expr>, b: Option<&Expr>) -> bool {
        if b.is_none_or(Expr::is_true) {
            return true;
        }
        match a {
            None => false,
            Some(a) if a.is_true() => false,
            Some(a) => implies(a, b.expect("checked above")),
        }
    }

    /// `getKeybindings`: every rule in precedence order, the removals applied.
    pub fn keybindings(&self) -> &[Item] {
        &self.keybindings
    }

    /// Keybindings of a command, the one that wins first.
    #[cfg(test)]
    pub fn lookup_keybindings(&self, command: &str) -> Vec<&Item> {
        self.lookup.get(command).map(|entries| entries.iter().rev().map(|&index| &self.keybindings[index]).collect()).unwrap_or_default()
    }

    /// The keybinding a command shows: the last whose `when` holds, else the last one.
    pub fn lookup_primary_keybinding(&self, command: &str, context: &dyn Context) -> Option<&Item> {
        let entries = self.lookup.get(command)?;
        let last = &self.keybindings[*entries.last()?];
        if entries.len() == 1 {
            return Some(last);
        }
        let matching = entries.iter().rev().map(|&index| &self.keybindings[index]).find(|item| item.when.as_ref().is_none_or(|when| when.evaluate(context)));
        Some(matching.unwrap_or(last))
    }

    pub fn resolve(&self, context: &dyn Context, current_chords: &[Chord], keypress: Chord) -> Resolution {
        let pressed: Vec<Chord> = current_chords.iter().copied().chain([keypress]).collect();
        let Some(candidates) = self.map.get(&pressed[0]) else { return Resolution::NoMatchingKb };
        let candidates = candidates.iter().map(|&index| &self.keybindings[index]).filter(|candidate| {
            pressed.len() == 1 || (pressed.len() <= candidate.chords.len() && (1..pressed.len()).all(|i| candidate.chords[i] == pressed[i]))
        });
        let found = candidates.collect::<Vec<_>>().into_iter().rev().find(|item| item.when.as_ref().is_none_or(|when| when.evaluate(context)));
        match found {
            None => Resolution::NoMatchingKb,
            Some(item) if pressed.len() < item.chords.len() => Resolution::MoreChordsNeeded,
            Some(item) => Resolution::KbFound { command: item.command.clone(), args: item.args.clone(), bubble: item.bubble },
        }
    }
}

/// `KeybindingEditorDecorationsRenderer._userSettingsFuzzyEquals`: the same keybinding after trimming and
/// lowercasing, where a `[code]` chord equals only the same scan code, never the key it types.
pub fn user_settings_fuzzy_equals(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().to_lowercase(), b.trim().to_lowercase());
    if a == b {
        return true;
    }
    let scan_codes = |input: &str| input.split_whitespace().map(|chord| chord.contains('[')).collect::<Vec<_>>();
    parse_keybinding(&a) == parse_keybinding(&b) && scan_codes(&a) == scan_codes(&b)
}

/// Whether a key of the layout types `key`. macLinuxKeyboardMapper.ts always maps the Latin letters and the digits,
/// and the immutable keys; the OEM and ABNT key codes exist only where a key's characters (`raw_mapping`'s value,
/// withShift, withAltGr, withShiftAltGr) are their US symbols. The port has no scan code mapper (port/notes.md
/// milestone 9), so this asks per key code, not per modifier combination as `lookupKeyCodeCombo` does.
pub fn layout_types_key(key: usize, raw_mapping: &Value) -> bool {
    const OEM_1: usize = 85;
    const OEM_102: usize = 97;
    const ABNT_C1: usize = 120;
    const ABNT_C2: usize = 121;
    if key == 0 {
        return false;
    }
    if !(OEM_1..=OEM_102).contains(&key) && !(ABNT_C1..=ABNT_C2).contains(&key) {
        return true;
    }
    let Some(mapping) = raw_mapping.as_object() else { return false };
    let layout_keys = mapping.iter().filter(|(code, _)| LAYOUT_DEPENDENT_CODES.contains(&code.as_str()));
    let characters = layout_keys.flat_map(|(_, entry)| ["value", "withShift", "withAltGr", "withShiftAltGr"].map(|name| entry[name].as_str().unwrap_or_default().to_string()));
    characters.filter_map(|text| text.chars().next()).any(|character| us_key_of_char(character) == key)
}

/// The scan codes `IMMUTABLE_CODE_TO_KEY_CODE` leaves to the layout, besides the letters and digits.
const LAYOUT_DEPENDENT_CODES: [&str; 15] =
    ["Minus", "Equal", "BracketLeft", "BracketRight", "Backslash", "IntlHash", "Semicolon", "Quote", "Backquote", "Comma", "Period", "Slash", "IntlBackslash", "IntlRo", "IntlYen"];

/// `CHAR_CODE_TO_KEY_CODE` of a US symbol, shifted or not; 0 for any other character.
fn us_key_of_char(character: char) -> usize {
    let unshifted = match US_SHIFTED_SYMBOLS.chars().position(|symbol| symbol == character) {
        Some(index) => US_UNSHIFTED_SYMBOLS.chars().nth(index).expect("the symbol rows have the same length"),
        None => character,
    };
    if !US_UNSHIFTED_SYMBOLS.contains(unshifted) {
        return 0;
    }
    key_from_user_settings(&unshifted.to_string())
}

/// preferences.contribution.ts gives `editor.action.defineKeybinding` the condition
/// `resource == <the profile's keybindings.json>`; the reference's exports froze the exporting profile's temp file in
/// it, so the port puts its own user keybindings.json URI in its place.
pub fn with_keybindings_resource(when: Option<Expr>, resource: &str) -> Option<Expr> {
    const FILE: &str = "/User/keybindings.json'";
    let serialized = when?.serialize();
    let Some(start) = serialized.find("'vscode-userdata:") else { return Expr::deserialize(&serialized) };
    let Some(length) = serialized[start..].find(FILE) else { return Expr::deserialize(&serialized) };
    let end = start + length + FILE.len();
    Expr::deserialize(&format!("{}'{resource}'{}", &serialized[..start], &serialized[end..]))
}

/// One `{ key, command, when, args }` rule of keybindings.json or the default table.
fn rule_item(rule: &Value, key: &str, is_default: bool) -> Option<Item> {
    let command = rule.get("command")?.as_str()?;
    let when = rule.get("when").and_then(Value::as_str).and_then(Expr::deserialize);
    let system_wide = rule.get("systemWide") == Some(&Value::Bool(true));
    Some(Item { system_wide, ..Item::new(parse_keybinding(key), command, rule.get("args").cloned(), when, is_default) })
}

/// The macOS default keybindings of the workbench: the reference's own `--export-default-keybindings` output
/// without the built-in extensions' rows, which come from `contributes.keybindings` here. Rows without a
/// weight sort below every extension rule (weights under 300); the rest carry their `KeybindingWeight`.
const DEFAULT_KEYBINDINGS: &str = include_str!("keybindings/default-macos.json");

struct Weighted {
    weight: u64,
    order: usize,
    item: Item,
}

/// `KeybindingsRegistry.getDefaultKeybindings()` plus the extensions' `contributes.keybindings`, sorted as upstream's
/// `sorter`: by weight, then command, then registration order.
pub fn default_keybindings(extensions: &[ScannedExtension]) -> Vec<Item> {
    let rows: Vec<Value> = serde_json::from_str(DEFAULT_KEYBINDINGS).expect("the bundled default keybindings are JSON");
    let mut items = Vec::new();
    let mut weighted = Vec::new();
    for row in &rows {
        let item = rule_item(row, row["key"].as_str().expect("every default row has a key"), true).expect("every default row has a command");
        match row.get("weight").and_then(Value::as_u64) {
            None => items.push(item),
            Some(weight) => weighted.push(Weighted { weight, order: weighted.len(), item }),
        }
    }
    let enablements = command_enablements(extensions);
    for extension in extensions {
        weighted.extend(extension_keybindings(extension, &enablements).into_iter().map(|(weight, item)| Weighted { weight, order: 0, item }));
    }
    weighted.sort_by(|a, b| a.weight.cmp(&b.weight).then_with(|| a.item.command.cmp(&b.item.command)).then(a.order.cmp(&b.order)));
    items.extend(weighted.into_iter().map(|weighted| weighted.item));
    items
}

/// `WorkbenchKeybindingService._asCommandRule`: the macOS key (`mac`, else `key`), a weight of 300 (built-in)
/// or 400 (external) plus the rule's 1-based index, and the command's `enablement` joined to its `when`.
fn extension_keybindings(extension: &ScannedExtension, enablements: &HashMap<&str, Expr>) -> Vec<(u64, Item)> {
    let rules = match &extension.description["contributes"]["keybindings"] {
        Value::Array(rules) => rules.iter().collect(),
        rule @ Value::Object(_) => vec![rule],
        _ => return Vec::new(),
    };
    let base = if extension.description["isBuiltin"] == true { 300 } else { 400 };
    let source = ExtensionSource::of(extension);
    let mut items = Vec::new();
    for (index, rule) in rules.into_iter().enumerate() {
        let Some(key) = rule.get("mac").or_else(|| rule.get("key")).and_then(Value::as_str) else { continue };
        let Some(command) = rule.get("command").and_then(Value::as_str) else { continue };
        let when = rule.get("when").and_then(Value::as_str).and_then(Expr::deserialize);
        let when = match (when, enablements.get(command).cloned()) {
            (Some(when), Some(enablement)) => Expr::and(vec![Some(enablement), Some(when)]),
            (when, enablement) => when.or(enablement),
        };
        let item = Item::new(parse_keybinding(key), command, rule.get("args").cloned(), when, true);
        items.push((base + index as u64 + 1, Item { extension: Some(source.clone()), ..item }));
    }
    items
}

/// The `enablement` of every contributed command, the precondition `MenuRegistry.getCommand` gives.
fn command_enablements(extensions: &[ScannedExtension]) -> HashMap<&str, Expr> {
    let mut enablements = HashMap::new();
    for extension in extensions {
        let commands = match &extension.description["contributes"]["commands"] {
            Value::Array(commands) => commands.iter().collect(),
            command => vec![command],
        };
        for command in commands {
            let (Some(id), Some(enablement)) = (command["command"].as_str(), command["enablement"].as_str().and_then(Expr::deserialize)) else { continue };
            enablements.insert(id, enablement);
        }
    }
    enablements
}

/// The user's keybindings.json (`KeybindingIO.readUserKeybindingItem`); a missing or broken file adds nothing.
pub fn user_keybindings(path: &Path) -> Vec<Item> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(Value::Array(rules)) = serde_json::from_str::<Value>(&crate::theme::strip_jsonc(&text)) else {
        eprintln!("keybindings: {} is not a JSON array", path.display());
        return Vec::new();
    };
    rules.iter().filter_map(|rule| rule_item(rule, rule.get("key").and_then(Value::as_str).unwrap_or(""), false)).collect()
}
