//! Keybindings: the keybinding parser and US layout labels (base/common/keybindingParser.ts, keybindings.ts,
//! keybindingLabels.ts, platform/keybinding/common/usLayoutResolvedKeybinding.ts), the resolver
//! (keybindingResolver.ts) and the default, extension and user keybindings it is built from
//! (keybindingsRegistry.ts, workbench/services/keybinding/browser/keybindingService.ts).

pub(crate) mod key_codes;
pub(crate) mod scan_codes;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::Path;

use exthost::scan::ScannedExtension;
use serde_json::Value;

use crate::contextkey::{Context, Expr, implies};
use key_codes::{KEY_CODES, SCAN_CODES};
use scan_codes::SCAN_CODE_INFO;

use crate::keyboard_mapper::{self, KeyboardMapper, ScanCodeChord};

/// One key press with its modifiers: a `KeyCodeChord` (`key` indexes `KEY_CODES`, 0 is `KeyCode.Unknown`), or a
/// `ScanCodeChord` when `scan_code` (an index of `SCAN_CODE_INFO`) is set, where `key` is the US key code it stands
/// for without a keyboard mapper. The labels come from the current layout's keyboard mapper when there is one.
#[derive(Clone, Copy, Debug)]
pub struct Chord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
    pub key: usize,
    pub scan_code: Option<usize>,
}

/// A scan code chord equals only the same scan code, a key code chord only the same key code.
impl PartialEq for Chord {
    fn eq(&self, other: &Chord) -> bool {
        self.identity() == other.identity()
    }
}

impl Eq for Chord {}

impl std::hash::Hash for Chord {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.identity().hash(state);
    }
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
        Chord { ctrl: modifiers.control, shift, alt: modifiers.alt, meta: modifiers.platform, key: key_from_user_settings(&key), scan_code: None }
    }

    fn identity(&self) -> ([bool; 4], Option<usize>, usize) {
        ([self.ctrl, self.shift, self.alt, self.meta], self.scan_code, if self.scan_code.is_some() { 0 } else { self.key })
    }

    /// `isModifierKey`: such a chord cannot be dispatched in the keydown phase.
    pub fn is_modifier_key(&self) -> bool {
        matches!(KEY_CODES[self.key].0, "unknown" | "Ctrl" | "Meta" | "Alt" | "Shift")
    }

    fn is_duplicate_modifier_case(&self) -> bool {
        let key = KEY_CODES[self.key].0;
        (self.ctrl && key == "Ctrl") || (self.shift && key == "Shift") || (self.alt && key == "Alt") || (self.meta && key == "Meta")
    }

    /// The chord as the current layout types it (`resolveKeybinding`'s first result), when a mapper is installed.
    fn mapped(&self) -> Option<(std::sync::Arc<KeyboardMapper>, ScanCodeChord)> {
        let mapper = keyboard_mapper::current()?;
        let chord = mapper.resolve_chord(self).into_iter().next()?;
        Some((mapper, chord))
    }

    /// The macOS UI label: modifier symbols in Ctrl-Shift-Alt-Cmd order, arrows as arrows.
    pub fn label(&self) -> String {
        if let Some(label) = self.mapped().and_then(|(mapper, chord)| mapper.label(&[chord])) {
            return label;
        }
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
        if let Some((mapper, chord)) = self.mapped().filter(|(mapper, chord)| mapper.label(&[*chord]).is_some()) {
            return mapper.key_caps(&chord);
        }
        let mut caps = Vec::new();
        for (symbol, pressed) in [("\u{2303}", self.ctrl), ("⇧", self.shift), ("⌥", self.alt), ("⌘", self.meta)] {
            if pressed {
                caps.push(symbol.to_string());
            }
        }
        let key = Chord { ctrl: false, shift: false, alt: false, meta: false, key: self.key, scan_code: None }.label();
        if !key.is_empty() && !self.is_duplicate_modifier_case() {
            caps.push(key);
        }
        caps
    }

    pub fn aria_label(&self) -> String {
        if let Some(label) = self.mapped().and_then(|(mapper, chord)| mapper.aria_label(&[chord])) {
            return label;
        }
        let key = if self.is_duplicate_modifier_case() { "" } else { KEY_CODES[self.key].0 };
        self.with_modifiers(["Control", "Shift", "Option", "Command"], "+", key)
    }

    /// `keyAriaLabel` of a `ResolvedChord`.
    pub fn key_aria_label(&self) -> String {
        let modifierless = |chord: ScanCodeChord| ScanCodeChord { ctrl: false, shift: false, alt: false, meta: false, ..chord };
        if let Some(label) = self.mapped().and_then(|(mapper, chord)| mapper.aria_label(&[modifierless(chord)])) {
            return label;
        }
        KEY_CODES[self.key].0.to_string()
    }

    pub fn user_settings_label(&self) -> String {
        if let Some(label) = self.mapped().and_then(|(mapper, chord)| mapper.user_settings_label(&[chord])) {
            return label;
        }
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

    /// `getDispatchChords` of one chord: the scan code dispatch string of the layout's keyboard mapper, else
    /// `USLayoutResolvedKeybinding`'s `ctrl+shift+alt+meta+<KeyCode>`; none for a lone modifier.
    pub fn dispatch(&self) -> Option<String> {
        if let Some(mapper) = keyboard_mapper::current() {
            return mapper.resolve_chord(self).first().and_then(|chord| mapper.dispatch(chord));
        }
        if self.is_modifier_key() {
            return None;
        }
        Some(format!("{}{}", self.with_modifiers(["ctrl+", "shift+", "alt+", "meta+"], "", ""), KEY_CODES[self.key].0))
    }

    fn with_modifiers(&self, labels: [&str; 4], separator: &str, key: &str) -> String {
        with_modifiers([self.ctrl, self.shift, self.alt, self.meta], labels, separator, key)
    }
}

/// `_simpleAsString`: the pressed modifiers' labels in Ctrl, Shift, Alt, Meta order, then the key.
pub fn with_modifiers(pressed: [bool; 4], labels: [&str; 4], separator: &str, key: &str) -> String {
    let mut parts: Vec<&str> = labels.iter().zip(pressed).filter(|(_, pressed)| *pressed).map(|(label, _)| *label).collect();
    if !key.is_empty() {
        parts.push(key);
    }
    parts.join(separator)
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
    let mut chord = Chord { ctrl: false, shift: false, alt: false, meta: false, key: 0, scan_code: None };
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
        let scan_code = SCAN_CODE_INFO.iter().position(|(name, _)| name.to_lowercase() == scan_code);
        return (key.map(|key| Chord { key, scan_code, ..chord }), remains);
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
/// the plain key), else `toElectronAccelerator`'s key name after the modifiers; on the layout's keyboard mapper
/// when there is one.
pub fn electron_accelerator(chords: &[Chord]) -> Option<String> {
    if let Some(mapper) = keyboard_mapper::current() {
        return mapper.resolve_keybinding(chords).first().and_then(|first| mapper.electron_accelerator(first));
    }
    let [chord] = chords else { return None };
    if chord.is_duplicate_modifier_case() {
        return None;
    }
    Some(chord.with_modifiers(["Ctrl", "Shift", "Alt", "Cmd"], "+", electron_key(chord.key)?))
}

/// `KeyCodeUtils.toElectronAccelerator`: none for numpad keys, Electron's names for the arrows and Delete.
pub fn electron_key(key: usize) -> Option<&'static str> {
    if (NUMPAD_0..=NUMPAD_DIVIDE).contains(&key) {
        return None;
    }
    let (name, user_settings_name, _) = KEY_CODES[key];
    Some(match name {
        "LeftArrow" | "UpArrow" | "RightArrow" | "DownArrow" | "Del" => user_settings_name,
        name => name,
    })
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
    /// `_sourceKey`: the `key` string of a keybindings.json entry.
    pub source_key: Option<String>,
}

/// The source of an extension's keybinding: its identifier and the `displayName` the Keyboard Shortcuts editor shows.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtensionSource {
    pub id: String,
    pub display_name: Option<String>,
    pub is_builtin: bool,
}

impl ExtensionSource {
    pub fn of(extension: &ScannedExtension) -> ExtensionSource {
        ExtensionSource { id: extension.id.clone(), display_name: extension.description["displayName"].as_str().map(str::to_string), is_builtin: extension.description["isBuiltin"] == true }
    }
}

impl Item {
    pub fn new(keybinding: Vec<Chord>, command: &str, args: Option<Value>, when: Option<Expr>, is_default: bool) -> Item {
        let bubble = command.starts_with('^');
        let command = command.strip_prefix('^').unwrap_or(command).to_string();
        let chords = if keybinding.iter().any(Chord::is_modifier_key) { Vec::new() } else { keybinding.clone() };
        Item { keybinding, chords, bubble, command, args, when, is_default, extension: None, system_wide: false, source_key: None }
    }

    /// keybindingResolver.ts `printSourceExplanation`.
    fn source_explanation(&self) -> String {
        match &self.extension {
            Some(extension) if extension.is_builtin => format!("built-in extension {}", extension.id),
            Some(extension) => format!("user extension {}", extension.id),
            None if self.is_default => "built-in".into(),
            None => "user".into(),
        }
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
    /// The dispatch strings of each keybinding's chords (`ResolvedKeybindingItem.chords`).
    dispatch: Vec<Vec<String>>,
    map: HashMap<String, Vec<usize>>,
    lookup: HashMap<String, Vec<usize>>,
}

impl Resolver {
    /// With a keyboard mapper installed, each rule first becomes one rule per scan code keybinding that types it on
    /// the layout (`_resolveKeybindingItems`); a rule the layout cannot type keeps its command without keys.
    pub fn new(defaults: Vec<Item>, overrides: Vec<Item>) -> Resolver {
        let rules: Vec<Item> = defaults.into_iter().chain(overrides).collect();
        let rules = match keyboard_mapper::current() {
            Some(mapper) => rules.into_iter().flat_map(|rule| resolve_item(&mapper, rule)).collect(),
            None => rules,
        };
        let keybindings = Resolver::handle_removals(rules);
        let dispatch = keybindings.iter().map(|item| dispatch_chords(&item.chords)).collect();
        let mut resolver = Resolver { keybindings, dispatch, map: HashMap::new(), lookup: HashMap::new() };
        for index in 0..resolver.keybindings.len() {
            let item = &resolver.keybindings[index];
            if resolver.dispatch[index].is_empty() || item.when.as_ref().and_then(Expr::substitute_constants).is_some_and(|when| when.is_false()) {
                continue;
            }
            resolver.add_key_press(index);
        }
        resolver
    }

    fn is_targeted_for_removal(default: &Item, keypress: &[String], when: Option<&Expr>) -> bool {
        let default_chords = dispatch_chords(&default.chords);
        if keypress.iter().enumerate().any(|(i, chord)| default_chords.get(i) != Some(chord)) {
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
                        !targeting.iter().any(|removal| Resolver::is_targeted_for_removal(rule, &dispatch_chords(&removal.chords), removal.when.as_ref()))
                    }
                    _ => true,
                }
            })
            .collect();
        rules.into_iter().zip(kept).filter(|(_, kept)| *kept).map(|(rule, _)| rule).collect()
    }

    fn add_key_press(&mut self, index: usize) {
        let item = &self.keybindings[index];
        let chords = &self.dispatch[index];
        let conflicts = self.map.entry(chords[0].clone()).or_default();
        for &conflict_index in conflicts.iter().rev() {
            let conflict = &self.keybindings[conflict_index];
            if conflict.command == item.command {
                continue;
            }
            let conflict_chords = &self.dispatch[conflict_index];
            let is_shorter_prefix = (1..conflict_chords.len().min(chords.len())).all(|i| conflict_chords[i] == chords[i]);
            if is_shorter_prefix
                && Resolver::when_is_entirely_included(conflict.when.as_ref(), item.when.as_ref())
                && let Some(entries) = self.lookup.get_mut(&conflict.command)
            {
                entries.retain(|entry| *entry != conflict_index);
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

    /// abstractKeybindingService.ts `appendKeybinding`: `label (⌘K)` when the command has a keybinding.
    pub fn append_keybinding(&self, label: &str, command: &str, context: &dyn Context) -> String {
        match self.lookup_primary_keybinding(command, context) {
            Some(item) if !command.is_empty() => format!("{label} ({})", self::label(&item.keybinding)),
            _ => label.to_string(),
        }
    }

    #[cfg(test)]
    pub fn resolve(&self, context: &dyn Context, current_chords: &[Chord], keypress: Chord) -> Resolution {
        self.resolve_logged(context, current_chords, keypress, &mut Vec::new())
    }

    /// `resolve`, pushing the lines of Keyboard Shortcuts Troubleshooting to `log`.
    pub fn resolve_logged(&self, context: &dyn Context, current_chords: &[Chord], keypress: Chord, log: &mut Vec<String>) -> Resolution {
        let Some(pressed) = current_chords.iter().chain([&keypress]).map(Chord::dispatch).collect::<Option<Vec<String>>>() else {
            return Resolution::NoMatchingKb;
        };
        log.push(format!("| Resolving {}", pressed.join(",")));
        let Some(candidates) = self.map.get(&pressed[0]) else {
            log.push("\\ No keybinding entries.".into());
            return Resolution::NoMatchingKb;
        };
        let candidates: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&index| {
                let chords = &self.dispatch[index];
                pressed.len() == 1 || (pressed.len() <= chords.len() && (1..pressed.len()).all(|i| chords[i] == pressed[i]))
            })
            .collect();
        let entries = candidates.len();
        let Some(index) = candidates.into_iter().rev().find(|&index| self.keybindings[index].when.as_ref().is_none_or(|when| when.evaluate(context))) else {
            log.push(format!("\\ From {entries} keybinding entries, no when clauses matched the context."));
            return Resolution::NoMatchingKb;
        };
        let item = &self.keybindings[index];
        let explanation = format!("when: {}, source: {}", item.when.as_ref().map_or("no when condition".into(), Expr::serialize), item.source_explanation());
        let chord_count = self.dispatch[index].len();
        if pressed.len() < chord_count {
            log.push(format!("\\ From {entries} keybinding entries, awaiting {} more chord(s), {explanation}.", chord_count - pressed.len()));
            return Resolution::MoreChordsNeeded;
        }
        log.push(format!("\\ From {entries} keybinding entries, matched {}, {explanation}.", item.command));
        Resolution::KbFound { command: item.command.clone(), args: item.args.clone(), bubble: item.bubble }
    }
}

/// keybindingService.ts `_dumpResolveKeybindingDebugInfo`: each distinct default and user keybinding with the
/// dispatch strings it resolves to on the current layout.
pub fn dump_resolve_keybinding_debug_info(defaults: &[Item], overrides: &[Item]) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut result = vec!["Default Resolved Keybindings (unique only):".to_string()];
    for item in defaults.iter().filter(|item| !item.keybinding.is_empty()) {
        // `_printKeybinding`: user settings modifiers with `KeyCodeUtils.toString` or the scan code's name.
        let input = item.keybinding.iter().map(|chord| {
            let key = chord.scan_code.map_or(KEY_CODES[chord.key].0, |scan_code| SCAN_CODE_INFO[scan_code].0);
            chord.with_modifiers(["ctrl", "shift", "alt", "cmd"], "+", key)
        });
        let input = input.collect::<Vec<_>>().join(" ");
        if seen.insert(input.clone()) {
            print_resolved_keybindings(&mut result, &input, &item.keybinding);
        }
    }
    result.push("User Resolved Keybindings (unique only):".into());
    for item in overrides.iter().filter(|item| !item.keybinding.is_empty()) {
        let input = item.source_key.clone().unwrap_or_else(|| "Impossible: missing source key, but has keybinding".into());
        if seen.insert(input.clone()) {
            print_resolved_keybindings(&mut result, &input, &item.keybinding);
        }
    }
    result.join("\n")
}

/// `_printResolvedKeybindings`: every row starts with the input, as upstream's `isFirst` is never cleared.
fn print_resolved_keybindings(output: &mut Vec<String>, input: &str, keybinding: &[Chord]) {
    let pad = |text: &str| format!("{text:>35}");
    let first_row = format!("{} => ", pad(input));
    let resolved: Vec<String> = match keyboard_mapper::current() {
        Some(mapper) => mapper.resolve_keybinding(keybinding).iter().map(|chords| chords.iter().map(|chord| mapper.dispatch(chord).unwrap_or("[null]".into())).collect::<Vec<_>>().join(" ")).collect(),
        None => vec![keybinding.iter().map(|chord| chord.dispatch().unwrap_or("[null]".into())).collect::<Vec<_>>().join(" ")],
    };
    if resolved.is_empty() {
        output.push(format!("{first_row}{}", pad("[NO BINDING]")));
    }
    for dispatch in resolved {
        output.push(format!("{first_row}{}", pad(&dispatch)));
    }
}

/// `toEmptyArrayIfContainsNull(getDispatchChords())`.
fn dispatch_chords(chords: &[Chord]) -> Vec<String> {
    chords.iter().map(Chord::dispatch).collect::<Option<Vec<_>>>().unwrap_or_default()
}

/// One rule per scan code keybinding of `rule`'s keys on the mapper's layout; the chords keep their key code for
/// the code paths without a mapper.
fn resolve_item(mapper: &KeyboardMapper, rule: Item) -> Vec<Item> {
    if rule.keybinding.is_empty() {
        return vec![rule];
    }
    let resolved = mapper.resolve_keybinding(&rule.keybinding);
    if resolved.is_empty() {
        return vec![Item { keybinding: Vec::new(), chords: Vec::new(), ..rule }];
    }
    resolved
        .into_iter()
        .map(|scan_chords| {
            let keybinding: Vec<Chord> = rule
                .keybinding
                .iter()
                .zip(scan_chords)
                .map(|(chord, scan)| Chord { ctrl: scan.ctrl, shift: scan.shift, alt: scan.alt, meta: scan.meta, key: chord.key, scan_code: Some(scan.scan_code) })
                .collect();
            let chords = if keybinding.iter().any(Chord::is_modifier_key) { Vec::new() } else { keybinding.clone() };
            Item { keybinding, chords, ..rule.clone() }
        })
        .collect()
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
    let source_key = (!is_default).then(|| key.to_string());
    Some(Item { system_wide, source_key, ..Item::new(parse_keybinding(key), command, rule.get("args").cloned(), when, is_default) })
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

/// keybindingsExport.contribution.ts `_getDefaultKeybindingsContentForOS` on macOS: the core rows in registry order, then
/// each built-in extension's rules in their own order, unsorted.
pub fn exported_default_keybindings(extensions: &[ScannedExtension]) -> Vec<Item> {
    let rows: Vec<Value> = serde_json::from_str(DEFAULT_KEYBINDINGS).expect("the bundled default keybindings are JSON");
    let mut items: Vec<Item> = rows.iter().map(|row| rule_item(row, row["key"].as_str().expect("every default row has a key"), true).expect("every default row has a command")).collect();
    let enablements = command_enablements(extensions);
    for extension in extensions.iter().filter(|extension| extension.description["isBuiltin"] == true) {
        items.extend(extension_keybindings(extension, &enablements).into_iter().map(|(_, item)| item));
    }
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
