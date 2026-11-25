//! MacLinuxKeyboardMapper of workbench/services/keybinding/common/macLinuxKeyboardMapper.ts, the macOS branch: the
//! keyboard layout's characters per scan code (`IMacLinuxKeyboardMapping`, as keyboard_layout.rs reads it) turned
//! into scan code combos <-> key code combos, so a keybinding of a key code dispatches on the scan code that types
//! it on this layout, with the labels that layout shows.

#[cfg(test)]
pub(crate) mod tests;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use serde_json::Value;

use crate::keybindings::key_codes::KEY_CODES;
use crate::keybindings::scan_codes::SCAN_CODE_INFO;
use crate::keybindings::{Chord, electron_key, with_modifiers};
use crate::keyboard_layout::KeyEvent;

/// `ScanCodeChord`: `scan_code` indexes `SCAN_CODE_INFO`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScanCodeChord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
    pub scan_code: usize,
}

impl ScanCodeChord {
    fn is_duplicate_modifier_case(&self) -> bool {
        let name = SCAN_CODE_INFO[self.scan_code].0;
        (self.ctrl && matches!(name, "ControlLeft" | "ControlRight"))
            || (self.shift && matches!(name, "ShiftLeft" | "ShiftRight"))
            || (self.alt && matches!(name, "AltLeft" | "AltRight"))
            || (self.meta && matches!(name, "MetaLeft" | "MetaRight"))
    }

    fn modifiers(&self) -> [bool; 4] {
        [self.ctrl, self.shift, self.alt, self.meta]
    }
}

/// `ScanCodeUtils.toEnum`: 0 (`ScanCode.None`) for an unknown name.
pub fn scan_code(name: &str) -> usize {
    SCAN_CODE_INFO.iter().position(|(code, _)| *code == name).unwrap_or(0)
}

/// `IMMUTABLE_CODE_TO_KEY_CODE`: the key code a scan code has on every layout.
fn immutable_key_code(scan_code: usize) -> Option<usize> {
    usize::try_from(SCAN_CODE_INFO[scan_code].1).ok()
}

/// The `KeyCode` whose `KeyCodeUtils.toString` is `name`.
fn key_code(name: &str) -> usize {
    KEY_CODES.iter().position(|(label, ..)| *label == name).expect("a KeyCode label")
}

fn is_modifier_key(key: usize) -> bool {
    matches!(KEY_CODES[key].0, "Ctrl" | "Shift" | "Alt" | "Meta")
}

/// A scan code or key code with Ctrl, Shift and Alt, encoded as `_encode` does: Meta never changes the mapping.
fn encode(ctrl: bool, shift: bool, alt: bool, principal: usize) -> u32 {
    u32::from(ctrl) | u32::from(shift) << 1 | u32::from(alt) << 2 | (principal as u32) << 3
}

fn decode(encoded: u32) -> (bool, bool, bool, usize) {
    (encoded & 1 != 0, encoded & 2 != 0, encoded & 4 != 0, (encoded >> 3) as usize)
}

fn combo_string(encoded: u32, name: &str) -> String {
    let (ctrl, shift, alt, _) = decode(encoded);
    format!("{}{}{}{name}", if ctrl { "Ctrl+" } else { "" }, if shift { "Shift+" } else { "" }, if alt { "Alt+" } else { "" })
}

/// The four characters of one key (`IMacLinuxKeyMapping`).
#[derive(Clone, Debug, Default)]
struct KeyMapping {
    value: String,
    with_shift: String,
    with_alt_gr: String,
    with_shift_alt_gr: String,
}

impl KeyMapping {
    fn of(raw: &Value) -> KeyMapping {
        let text = |name: &str| raw[name].as_str().unwrap_or_default().to_string();
        KeyMapping { value: text("value"), with_shift: text("withShift"), with_alt_gr: text("withAltGr"), with_shift_alt_gr: text("withShiftAltGr") }
    }
}

/// The mapper of the current keyboard layout; none until the workbench reads the layout, and in tests, where the
/// US key code labels and dispatch of `FallbackKeyboardMapper` apply.
static CURRENT: RwLock<Option<Arc<KeyboardMapper>>> = RwLock::new(None);

pub fn current() -> Option<Arc<KeyboardMapper>> {
    CURRENT.read().expect("the keyboard mapper lock is never poisoned").clone()
}

pub fn install(mapper: Option<KeyboardMapper>) {
    *CURRENT.write().expect("the keyboard mapper lock is never poisoned") = mapper.map(Arc::new);
}

/// The layout info and settings the installed mapper was made for.
static INSTALLED_FOR: Mutex<Option<(Value, bool, bool)>> = Mutex::new(None);

/// Installs the mapper of the current input source unless the installed one was made for it with the same
/// settings; returns whether it changed.
pub fn refresh(dispatch_key_code: bool, map_alt_gr_to_ctrl_alt: bool) -> bool {
    let signature = (crate::keyboard_layout::current_layout_info(), dispatch_key_code, map_alt_gr_to_ctrl_alt);
    let mut installed_for = INSTALLED_FOR.lock().expect("the keyboard mapper lock is never poisoned");
    if installed_for.as_ref() == Some(&signature) {
        return false;
    }
    install(for_layout(&signature.0, &crate::keyboard_layout::current_raw_mapping(), dispatch_key_code, map_alt_gr_to_ctrl_alt));
    *installed_for = Some(signature);
    true
}

/// nativeKeyboardLayout.ts `getKeyboardMapper` and `createKeyboardMapper` on macOS: key code dispatch (no mapper)
/// when `keyboard.dispatch` is `keyCode`, the layout has no key map (Japanese and Chinese input sources) or is
/// Dvorak - QWERTY ⌘.
pub fn for_layout(layout_info: &Value, raw_mapping: &Value, dispatch_key_code: bool, map_alt_gr_to_ctrl_alt: bool) -> Option<KeyboardMapper> {
    if dispatch_key_code || raw_mapping.as_object().is_none_or(|mapping| mapping.is_empty()) || layout_info["id"] == "com.apple.keylayout.DVORAK-QWERTYCMD" {
        return None;
    }
    Some(KeyboardMapper::new(layout_info["id"] == "com.apple.keylayout.US", raw_mapping, map_alt_gr_to_ctrl_alt))
}

/// The chord of a key down: with a mapper, the scan code chord of the native event's DOM `code` and modifiers
/// (`resolveKeyboardEvent`), standing for the key code it types first; else the key code chord of GPUI's key name.
pub fn event_chord(keystroke: &gpui::Keystroke) -> Chord {
    let fallback = Chord::from_keystroke(keystroke);
    let Some(mapper) = current() else { return fallback };
    let Some(event) = crate::keyboard_layout::current_key_event() else { return fallback };
    let chord = mapper.resolve_keyboard_event(event, Some(fallback.key), false);
    let key = mapper.key_code_of(&chord).unwrap_or(fallback.key);
    Chord { ctrl: chord.ctrl, shift: chord.shift, alt: chord.alt, meta: chord.meta, key, scan_code: Some(chord.scan_code) }
}

pub struct KeyboardMapper {
    is_us_standard: bool,
    map_alt_gr_to_ctrl_alt: bool,
    /// The layout's characters per scan code, for `dump_debug_info`.
    code_info: HashMap<usize, KeyMapping>,
    /// `ScanCodeKeyCodeMapper`: scan code combo => key code combos and the inverse, most preferred first.
    scan_code_to_key_code: HashMap<u32, Vec<u32>>,
    key_code_to_scan_code: HashMap<u32, Vec<u32>>,
    scan_code_to_label: Vec<Option<String>>,
    scan_code_to_dispatch: Vec<Option<String>>,
}

impl KeyboardMapper {
    pub fn new(is_us_standard: bool, raw_mappings: &Value, map_alt_gr_to_ctrl_alt: bool) -> KeyboardMapper {
        let count = SCAN_CODE_INFO.len();
        let mut mapper = KeyboardMapper {
            is_us_standard,
            map_alt_gr_to_ctrl_alt,
            code_info: HashMap::new(),
            scan_code_to_key_code: HashMap::new(),
            key_code_to_scan_code: HashMap::new(),
            scan_code_to_label: vec![None; count],
            scan_code_to_dispatch: vec![None; count],
        };
        for (scan_code, &(name, _)) in SCAN_CODE_INFO.iter().enumerate() {
            let Some(key) = immutable_key_code(scan_code) else { continue };
            mapper.register_all_combos(scan_code, key);
            mapper.scan_code_to_label[scan_code] = Some(KEY_CODES[key].0.to_string());
            if key != 0 && !is_modifier_key(key) {
                mapper.scan_code_to_dispatch[scan_code] = Some(format!("[{name}]"));
            }
        }

        let raw_mappings = raw_mappings.as_object().cloned().unwrap_or_default();
        let layout_codes: Vec<(usize, &Value)> = raw_mappings
            .iter()
            .map(|(name, raw)| (scan_code(name), raw))
            .filter(|(scan_code, _)| *scan_code != 0 && immutable_key_code(*scan_code).is_none())
            .collect();

        // Some layouts (Cyrillic, Greek) type no Latin letters; their letters stay on the US positions.
        let mut produces_latin_letter = [false; 26];
        for (_, raw) in &layout_codes {
            if let Some(letter) = char::from_u32(char_code(raw["value"].as_str().unwrap_or_default())).filter(char::is_ascii_lowercase) {
                produces_latin_letter[(letter as u8 - b'a') as usize] = true;
            }
        }

        let mut mappings: Vec<(usize, [u32; 4])> = Vec::new();
        for (scan_code, raw) in layout_codes {
            let original = KeyMapping::of(raw);
            let name = SCAN_CODE_INFO[scan_code].0;
            let missing_letter = name.strip_prefix("Key").filter(|letter| letter.len() == 1).map(|letter| letter.as_bytes()[0] - b'A').filter(|index| !produces_latin_letter[*index as usize]);
            let mapping = match missing_letter {
                Some(index) => {
                    let letter = (b'a' + index) as char;
                    KeyMapping { value: letter.to_string(), with_shift: letter.to_ascii_uppercase().to_string(), ..KeyMapping::default() }
                }
                None => original.clone(),
            };
            mapper.code_info.insert(scan_code, original);
            let codes = [&mapping.value, &mapping.with_shift, &mapping.with_alt_gr, &mapping.with_shift_alt_gr].map(|text| char_code(text));
            mappings.push((scan_code, codes));
            mapper.scan_code_to_dispatch[scan_code] = Some(format!("[{name}]"));
            let value = codes[0];
            mapper.scan_code_to_label[scan_code] = match char::from_u32(value) {
                Some(letter) if letter.is_ascii_lowercase() => Some(letter.to_ascii_uppercase().to_string()),
                Some(character) if value != 0 => Some(character.to_string()),
                _ => None,
            };
        }

        // Ctrl+Shift+Alt (AltGr+Shift on Linux) first, as its characters are the least preferred.
        for &(scan_code, [value, with_shift, with_alt_gr, with_shift_alt_gr]) in mappings.iter().rev() {
            if with_shift_alt_gr == with_alt_gr || with_shift_alt_gr == with_shift || with_shift_alt_gr == value {
                continue;
            }
            let Some((key, shift)) = char_code_to_kb(with_shift_alt_gr) else { continue };
            mapper.register_if_unknown(encode(true, true, true, scan_code), encode(false, shift, false, key), key);
        }
        for &(scan_code, [value, with_shift, with_alt_gr, _]) in mappings.iter().rev() {
            if with_alt_gr == with_shift || with_alt_gr == value {
                continue;
            }
            let Some((key, shift)) = char_code_to_kb(with_alt_gr) else { continue };
            mapper.register_if_unknown(encode(true, false, true, scan_code), encode(false, shift, false, key), key);
        }
        for &(scan_code, [value, with_shift, ..]) in mappings.iter().rev() {
            if with_shift == value {
                continue;
            }
            let Some((key, shift)) = char_code_to_kb(with_shift) else { continue };
            let pairs: &[(u8, u8)] = match shift {
                true => &[(0b010, 0b010), (0b110, 0b110), (0b011, 0b011), (0b111, 0b111)],
                false => &[(0b010, 0b000), (0b010, 0b010), (0b110, 0b100), (0b110, 0b110), (0b011, 0b001), (0b011, 0b011), (0b111, 0b101), (0b111, 0b111)],
            };
            mapper.register_pairs(scan_code, key, pairs);
        }
        for &(scan_code, [value, ..]) in mappings.iter().rev() {
            let Some((key, shift)) = char_code_to_kb(value) else { continue };
            let pairs: &[(u8, u8)] = match shift {
                true => &[(0b000, 0b010), (0b100, 0b110), (0b001, 0b011), (0b101, 0b111)],
                false => &[(0b000, 0b000), (0b100, 0b100), (0b010, 0b010), (0b110, 0b110), (0b001, 0b001), (0b101, 0b101), (0b011, 0b011), (0b111, 0b111)],
            };
            mapper.register_pairs(scan_code, key, pairs);
        }
        // Digits keep their key codes on every modifier: Ctrl+1 is Ctrl+1 even where 1 needs Shift.
        for digit in 1..=10 {
            let name = format!("Digit{}", digit % 10);
            mapper.register_all_combos(scan_code(&name), key_code(&(digit % 10).to_string()));
        }
        mapper.move_to_end(scan_code("IntlHash"));
        mapper.move_to_end(scan_code("IntlBackslash"));
        mapper
    }

    /// `_registerIfUnknown` for (scan code modifiers, key code modifiers) bit pairs: bit 0 Ctrl, 1 Shift, 2 Alt.
    fn register_pairs(&mut self, scan_code: usize, key: usize, pairs: &[(u8, u8)]) {
        for &(hardware, keyboard) in pairs {
            let bits = |mods: u8| (mods & 1 != 0, mods & 2 != 0, mods & 4 != 0);
            let ((ctrl, shift, alt), (kb_ctrl, kb_shift, kb_alt)) = (bits(hardware), bits(keyboard));
            self.register_if_unknown(encode(ctrl, shift, alt, scan_code), encode(kb_ctrl, kb_shift, kb_alt, key), key);
        }
    }

    fn register_all_combos(&mut self, scan_code: usize, key: usize) {
        for mods in 0..8u8 {
            self.register_pairs(scan_code, key, &[(mods, mods)]);
        }
    }

    /// `registerIfUnknown`: a scan code combo takes several letter or digit key codes, else only its first one.
    fn register_if_unknown(&mut self, scan_code_combo: u32, key_code_combo: u32, key: usize) {
        if key == 0 {
            return;
        }
        let letter_or_digit = matches!(KEY_CODES[key].0.as_bytes(), [b'A'..=b'Z' | b'0'..=b'9']);
        let existing = self.scan_code_to_key_code.entry(scan_code_combo).or_default();
        if letter_or_digit && existing.contains(&key_code_combo) || !letter_or_digit && !existing.is_empty() {
            return;
        }
        existing.insert(0, key_code_combo);
        self.key_code_to_scan_code.entry(key_code_combo).or_default().insert(0, scan_code_combo);
    }

    /// `_moveToEnd`: IntlHash and IntlBackslash are rare keys, so they are never the preferred scan code.
    fn move_to_end(&mut self, scan_code: usize) {
        for mods in 0..8 {
            let Some(key_code_combos) = self.scan_code_to_key_code.get(&((scan_code as u32) << 3 | mods)) else { continue };
            for key_code_combo in key_code_combos {
                let scan_code_combos = self.key_code_to_scan_code.get_mut(key_code_combo).expect("the inverse map has every key code combo");
                let len = scan_code_combos.len();
                if len == 1 {
                    continue;
                }
                for j in 0..len {
                    let entry = scan_code_combos[j];
                    if (entry >> 3) as usize == scan_code {
                        scan_code_combos[j..].rotate_left(1);
                    }
                }
            }
        }
    }

    fn lookup_scan_code_combo(&self, scan_code_combo: u32) -> &[u32] {
        self.scan_code_to_key_code.get(&scan_code_combo).map_or(&[], Vec::as_slice)
    }

    fn lookup_key_code_combo(&self, key_code_combo: u32) -> &[u32] {
        self.key_code_to_scan_code.get(&key_code_combo).map_or(&[], Vec::as_slice)
    }

    /// `guessStableKeyCode`: the key code a scan code types with and without Shift, when both agree.
    fn guess_stable_key_code(&self, scan_code: usize) -> Option<usize> {
        let name = SCAN_CODE_INFO[scan_code].0;
        if let Some(digit) = name.strip_prefix("Digit") {
            return Some(key_code(digit));
        }
        let ([unshifted], [shifted]) = (self.lookup_scan_code_combo(encode(false, false, false, scan_code)), self.lookup_scan_code_combo(encode(false, true, false, scan_code))) else {
            return None;
        };
        let ((_, shift_1, _, key_1), (_, shift_2, _, key_2)) = (decode(*unshifted), decode(*shifted));
        (key_1 == key_2 && shift_1 != shift_2).then_some(key_1)
    }

    /// `keyCodeChordToScanCodeChord`: every scan code combo that types the chord on this layout.
    pub fn key_code_chord_to_scan_code_chords(&self, chord: &Chord) -> Vec<ScanCodeChord> {
        if KEY_CODES[chord.key].0 == "Enter" {
            return vec![ScanCodeChord { ctrl: chord.ctrl, shift: chord.shift, alt: chord.alt, meta: chord.meta, scan_code: scan_code("Enter") }];
        }
        let combos = self.lookup_key_code_combo(encode(chord.ctrl, chord.shift, chord.alt, chord.key));
        combos
            .iter()
            .map(|combo| {
                let (ctrl, shift, alt, scan_code) = decode(*combo);
                ScanCodeChord { ctrl, shift, alt, meta: chord.meta, scan_code }
            })
            .collect()
    }

    /// `_resolveChord`: a `[code]` chord as written, a key code chord through the layout.
    pub fn resolve_chord(&self, chord: &Chord) -> Vec<ScanCodeChord> {
        match chord.scan_code {
            Some(scan_code) => vec![ScanCodeChord { ctrl: chord.ctrl, shift: chord.shift, alt: chord.alt, meta: chord.meta, scan_code }],
            None => self.key_code_chord_to_scan_code_chords(chord),
        }
    }

    /// `resolveKeybinding`: one scan code keybinding per combination of the chords' scan code combos.
    pub fn resolve_keybinding(&self, chords: &[Chord]) -> Vec<Vec<ScanCodeChord>> {
        let mut resolved: Vec<Vec<ScanCodeChord>> = vec![Vec::new()];
        for chord in chords {
            let options = self.resolve_chord(chord);
            resolved = resolved.iter().flat_map(|previous| options.iter().map(move |option| [previous.as_slice(), &[*option]].concat())).collect();
        }
        if chords.is_empty() { Vec::new() } else { resolved }
    }

    /// `resolveKeyboardEvent` of a key down with its DOM `code` and, for the keys that move with NumLock,
    /// its key code.
    pub fn resolve_keyboard_event(&self, event: KeyEvent, key: Option<usize>, alt_graph: bool) -> ScanCodeChord {
        let KeyEvent { code, ctrl, shift, alt, meta } = event;
        let mut code = scan_code(code);
        if SCAN_CODE_INFO[code].0 == "NumpadEnter" {
            code = scan_code("Enter");
        }
        // `IMMUTABLE_KEY_CODE_TO_CODE`: the last immutable scan code of the key code wins.
        let immutable_scan_code = |key: usize| (1..SCAN_CODE_INFO.len()).rev().find(|scan_code| immutable_key_code(*scan_code) == Some(key));
        if let Some(key) = key {
            let moves = matches!(KEY_CODES[key].0, "LeftArrow" | "UpArrow" | "RightArrow" | "DownArrow" | "Del" | "Insert" | "Home" | "End" | "PageDown" | "PageUp" | "Backspace");
            let numpad = matches!(SCAN_CODE_INFO[code].0, "Numpad1" | "Numpad2" | "Numpad3" | "Numpad4" | "Numpad5" | "Numpad6" | "Numpad7" | "Numpad8" | "Numpad9" | "Numpad0" | "NumpadDecimal");
            if (moves || numpad)
                && let Some(scan_code) = immutable_scan_code(key)
            {
                code = scan_code;
            }
        }
        let alt_gr = self.map_alt_gr_to_ctrl_alt && alt_graph;
        ScanCodeChord { ctrl: ctrl || alt_gr, shift, alt: alt || alt_gr, meta, scan_code: code }
    }

    /// The key code a scan code combo types first (`lookupScanCodeCombo`), else its immutable one.
    pub fn key_code_of(&self, chord: &ScanCodeChord) -> Option<usize> {
        let combos = self.lookup_scan_code_combo(encode(chord.ctrl, chord.shift, chord.alt, chord.scan_code));
        combos.first().map(|combo| decode(*combo).3).or_else(|| immutable_key_code(chord.scan_code))
    }

    fn key_label(&self, chord: &ScanCodeChord) -> Option<String> {
        if chord.is_duplicate_modifier_case() {
            return Some(String::new());
        }
        self.scan_code_to_label[chord.scan_code].clone()
    }

    /// `getUILabelForScanCodeChord`: arrows as arrows on macOS.
    fn ui_key_label(&self, chord: &ScanCodeChord) -> Option<String> {
        let arrow = match SCAN_CODE_INFO[chord.scan_code].0 {
            "ArrowLeft" => "←",
            "ArrowUp" => "↑",
            "ArrowRight" => "→",
            "ArrowDown" => "↓",
            _ => return self.key_label(chord),
        };
        Some(if chord.is_duplicate_modifier_case() { String::new() } else { arrow.to_string() })
    }

    /// `getUserSettingsLabelForScanCodeChord`: the US key code's name where the layout keeps it, else `[code]`.
    fn user_settings_key_label(&self, chord: &ScanCodeChord) -> Option<String> {
        if chord.is_duplicate_modifier_case() {
            return Some(String::new());
        }
        if let Some(key) = immutable_key_code(chord.scan_code) {
            return Some(KEY_CODES[key].1.to_lowercase());
        }
        if let Some(key) = self.guess_stable_key_code(chord.scan_code) {
            let reverse = self.key_code_chord_to_scan_code_chords(&Chord { ctrl: chord.ctrl, shift: chord.shift, alt: chord.alt, meta: chord.meta, key, scan_code: None });
            if reverse.iter().any(|reverse| reverse.scan_code == chord.scan_code) {
                return Some(KEY_CODES[key].1.to_lowercase());
            }
        }
        self.scan_code_to_dispatch[chord.scan_code].clone()
    }

    fn electron_key_label(&self, chord: &ScanCodeChord) -> Option<String> {
        let key = immutable_key_code(chord.scan_code).or_else(|| self.guess_stable_key_code(chord.scan_code))?;
        electron_key(key).map(str::to_string)
    }

    fn is_wysiwyg(&self, chord: &ScanCodeChord) -> bool {
        if immutable_key_code(chord.scan_code).is_some() {
            return true;
        }
        match (self.key_label(chord), self.user_settings_key_label(chord)) {
            (None, None) => true,
            (Some(a), Some(b)) => a.to_lowercase() == b.to_lowercase(),
            _ => false,
        }
    }

    /// `getDispatchStrForScanCodeChord`: `ctrl+shift+alt+meta+[Code]`, none for a lone modifier or unknown key.
    pub fn dispatch(&self, chord: &ScanCodeChord) -> Option<String> {
        let code = self.scan_code_to_dispatch[chord.scan_code].as_deref()?;
        Some(format!("{}{code}", with_modifiers(chord.modifiers(), ["ctrl+", "shift+", "alt+", "meta+"], "", "")))
    }

    /// `_getSingleModifierChordDispatch`; the port has no single modifier keybindings (`shift shift`) yet.
    #[cfg(test)]
    pub fn single_modifier_dispatch(&self, chord: &ScanCodeChord) -> Option<&'static str> {
        let name = SCAN_CODE_INFO[chord.scan_code].0;
        match (chord.ctrl, chord.shift, chord.alt, chord.meta) {
            (_, false, false, false) if matches!(name, "ControlLeft" | "ControlRight") => Some("ctrl"),
            (false, false, _, false) if matches!(name, "AltLeft" | "AltRight") => Some("alt"),
            (false, _, false, false) if matches!(name, "ShiftLeft" | "ShiftRight") => Some("shift"),
            (false, false, false, _) if matches!(name, "MetaLeft" | "MetaRight") => Some("meta"),
            _ => None,
        }
    }

    pub fn label(&self, chords: &[ScanCodeChord]) -> Option<String> {
        to_label(chords, ["\u{2303}", "⇧", "⌥", "⌘"], "", |chord| self.ui_key_label(chord))
    }

    pub fn aria_label(&self, chords: &[ScanCodeChord]) -> Option<String> {
        to_label(chords, ["Control", "Shift", "Option", "Command"], "+", |chord| self.key_label(chord))
    }

    pub fn user_settings_label(&self, chords: &[ScanCodeChord]) -> Option<String> {
        to_label(chords, ["ctrl", "shift", "alt", "cmd"], "+", |chord| self.user_settings_key_label(chord))
    }

    /// `getElectronAccelerator`: none for chords and a lone modifier.
    pub fn electron_accelerator(&self, chords: &[ScanCodeChord]) -> Option<String> {
        let [chord] = chords else { return None };
        if chord.is_duplicate_modifier_case() {
            return None;
        }
        to_label(chords, ["Ctrl", "Shift", "Alt", "Cmd"], "+", |chord| self.electron_key_label(chord))
    }

    pub fn is_wysiwyg_keybinding(&self, chords: &[ScanCodeChord]) -> bool {
        chords.iter().all(|chord| self.is_wysiwyg(chord))
    }

    /// The key caps `KeybindingLabel` draws for one chord: each modifier, then the UI key label.
    pub fn key_caps(&self, chord: &ScanCodeChord) -> Vec<String> {
        let mut caps: Vec<String> = ["\u{2303}", "⇧", "⌥", "⌘"].into_iter().zip(chord.modifiers()).filter(|(_, pressed)| *pressed).map(|(symbol, _)| symbol.to_string()).collect();
        if let Some(key) = self.ui_key_label(chord).filter(|key| !key.is_empty()) {
            caps.push(key);
        }
        caps
    }

    /// `dumpDebugInfo`: per layout dependent scan code (and ArrowUp and Numpad0 as samples), each Ctrl, Shift and
    /// Alt combination with the character it types, its key code combos and the resolved labels.
    pub fn dump_debug_info(&self) -> String {
        let rule = "-".repeat(208);
        let mut result = vec![format!("isUSStandard: {}", self.is_us_standard), rule.clone()];
        let mut count = 0;
        for (scan_code, &(name, _)) in SCAN_CODE_INFO.iter().enumerate() {
            if immutable_key_code(scan_code).is_some() && !matches!(name, "ArrowUp" | "Numpad0") {
                continue;
            }
            if count % 4 == 0 {
                result.push("|       HW Code combination      |  Key  |    KeyCode combination    | Pri |          UI label         |         User settings          |    Electron accelerator   |       Dispatching string       | WYSIWYG |".into());
                result.push(rule.clone());
            }
            count += 1;
            let mapping = self.code_info.get(&scan_code);
            for mods in 0..8u32 {
                let (ctrl, shift, alt, _) = decode(mods);
                let combo = encode(ctrl, shift, alt, scan_code);
                let chord = self.resolve_keyboard_event(KeyEvent { code: name, ctrl, shift, alt, meta: false }, None, false);
                let chords = [chord];
                let out_combo = combo_string(combo, name);
                let out_key = produced_char(mapping, ctrl, shift, alt);
                let ui_label = self.aria_label(&chords).map(|label| label.replacen("Control+", "Ctrl+", 1));
                let user_settings = self.user_settings_label(&chords);
                let electron = self.electron_accelerator(&chords);
                let dispatch = self.dispatch(&chord);
                let wysiwyg = if self.is_wysiwyg_keybinding(&chords) { "       " } else { "   NO  " };
                let key_code_combos = self.lookup_scan_code_combo(combo);
                if key_code_combos.is_empty() {
                    result.push(format!(
                        "| {} | {out_key} | {} | {} | {} | {} | {} | {} | {wysiwyg} |",
                        pad(Some(&out_combo), 30),
                        pad(Some(""), 25),
                        pad(Some(""), 3),
                        pad(ui_label.as_deref(), 25),
                        pad(user_settings.as_deref(), 30),
                        pad(electron.as_deref(), 25),
                        pad(dispatch.as_deref(), 30)
                    ));
                    continue;
                }
                for (index, key_code_combo) in key_code_combos.iter().enumerate() {
                    let scan_code_combos = self.lookup_key_code_combo(*key_code_combo);
                    let priority = match scan_code_combos.len() {
                        1 => String::new(),
                        _ => scan_code_combos.iter().position(|other| *other == combo).map_or(-1, |position| position as i64 + 1).to_string(),
                    };
                    let keybinding = combo_string(*key_code_combo, KEY_CODES[decode(*key_code_combo).3].0);
                    if index == 0 {
                        result.push(format!(
                            "| {} | {out_key} | {} | {} | {} | {} | {} | {} | {wysiwyg} |",
                            pad(Some(&out_combo), 30),
                            pad(Some(&keybinding), 25),
                            pad(Some(&priority), 3),
                            pad(ui_label.as_deref(), 25),
                            pad(user_settings.as_deref(), 30),
                            pad(electron.as_deref(), 25),
                            pad(dispatch.as_deref(), 30)
                        ));
                    } else {
                        result.push(format!(
                            "| {} |       | {} | {} | {} | {} | {} | {} |         |",
                            pad(Some(""), 30),
                            pad(Some(&keybinding), 25),
                            pad(Some(&priority), 3),
                            pad(Some(""), 25),
                            pad(Some(""), 30),
                            pad(Some(""), 25),
                            pad(Some(""), 30)
                        ));
                    }
                }
            }
            result.push(rule.clone());
        }
        result.join("\n")
    }
}

/// `ModifierLabelProvider.toLabel`: chords joined by a space, none when a key cannot be expressed.
fn to_label(chords: &[ScanCodeChord], labels: [&str; 4], separator: &str, key_label: impl Fn(&ScanCodeChord) -> Option<String>) -> Option<String> {
    let mut parts = Vec::new();
    for chord in chords {
        parts.push(with_modifiers(chord.modifiers(), labels, separator, &key_label(chord)?));
    }
    Some(parts.join(" "))
}

/// `_leftPad`, counting UTF-16 code units as JavaScript does; `null` for a missing value.
fn pad(text: Option<&str>, width: usize) -> String {
    let text = text.unwrap_or("null");
    let len = text.encode_utf16().count();
    format!("{}{text}", " ".repeat(width.saturating_sub(len)))
}

/// `ScanCodeCombo.getProducedChar`.
fn produced_char(mapping: Option<&KeyMapping>, ctrl: bool, shift: bool, alt: bool) -> String {
    let text = match mapping {
        None => "",
        Some(mapping) if ctrl && shift && alt => &mapping.with_shift_alt_gr,
        Some(mapping) if ctrl && alt => &mapping.with_alt_gr,
        Some(mapping) if shift => &mapping.with_shift,
        Some(mapping) => &mapping.value,
    };
    match char_code(text) {
        0 => " --- ".into(),
        code @ 0x0300..=0x036F => format!("U+{code:x}"),
        code => format!("  {}  ", char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER)),
    }
}

/// `getCharCode`: the first character, combining diacritics as their spacing forms.
fn char_code(text: &str) -> u32 {
    let Some(first) = text.encode_utf16().next() else { return 0 };
    match first {
        0x0300 => 0x0060,
        0x0301 => 0x00B4,
        0x0302 => 0x005E,
        0x0303 => 0x02DC,
        0x0304 => 0x00AF,
        0x0305 => 0x203E,
        0x0306 => 0x02D8,
        0x0307 => 0x02D9,
        0x0308 => 0x00A8,
        0x030A => 0x02DA,
        0x030B => 0x02DD,
        code => u32::from(code),
    }
}

/// `_charCodeToKb`: the US key code (and whether it needs Shift) of a character, CJK punctuation as its ASCII twin.
fn char_code_to_kb(code: u32) -> Option<(usize, bool)> {
    let code = match code {
        0x3002 => '.' as u32,
        0x300C | 0x3010 => '[' as u32,
        0x300D | 0x3011 => ']' as u32,
        0xFF1B => ';' as u32,
        0xFF0C => ',' as u32,
        code => code,
    };
    let character = char::from_u32(code)?;
    if character.is_ascii_alphabetic() {
        return Some((key_code(&character.to_ascii_uppercase().to_string()), character.is_ascii_uppercase()));
    }
    const PAIRS: [(char, char, &str); 11] = [
        (';', ':', ";"),
        ('=', '+', "="),
        (',', '<', ","),
        ('-', '_', "-"),
        ('.', '>', "."),
        ('/', '?', "/"),
        ('`', '~', "`"),
        ('[', '{', "["),
        ('\\', '|', "\\"),
        (']', '}', "]"),
        ('\'', '"', "'"),
    ];
    PAIRS.iter().find_map(|(unshifted, shifted, key)| match character {
        c if c == *unshifted => Some((key_code(key), false)),
        c if c == *shifted => Some((key_code(key), true)),
        _ => None,
    })
}
