//! macLinuxKeyboardMapper.test.ts, the macOS suites, over the upstream layout fixtures.

use std::path::Path;

use serde_json::{Map, Value};

use super::*;
use crate::keybindings::parse_keybinding;

fn fixture(name: &str) -> String {
    let path = Path::new(env!("VSCODE_SRC")).join("src/vs/workbench/services/keybinding/test/node").join(name);
    std::fs::read_to_string(path).unwrap().replace("\r\n", "\n")
}

/// `readRawMapping`: the fixture's `define({ Code: { name: 'text', nameIsDeadKey: false }, ... })` object literal.
pub(crate) fn raw_mapping(name: &str) -> Value {
    let mut mapping = Map::new();
    let mut current: Option<(String, Map<String, Value>)> = None;
    for line in fixture(&format!("{name}.js")).lines() {
        let line = line.trim();
        if let Some(code) = line.strip_suffix(": {") {
            current = Some((code.to_string(), Map::new()));
        } else if line.starts_with('}') {
            if let Some((code, entry)) = current.take() {
                mapping.insert(code, Value::Object(entry));
            }
        } else if let (Some((_, entry)), Some((key, value))) = (current.as_mut(), line.trim_end_matches(',').split_once(": ")) {
            let value = match value.strip_prefix('\'').and_then(|value| value.strip_suffix('\'')) {
                Some(text) => Value::from(text.replace("\\'", "'").replace("\\\\", "\\")),
                None => Value::from(value == "true"),
            };
            entry.insert(key.to_string(), value);
        }
    }
    Value::Object(mapping)
}

fn mapper(name: &str, is_us_standard: bool) -> KeyboardMapper {
    KeyboardMapper::new(is_us_standard, &raw_mapping(name), false)
}

/// `IResolvedKeybinding`: label, aria label, electron accelerator, user settings label, WYSIWYG, dispatch parts,
/// single modifier dispatch parts.
type Resolved = (Option<String>, Option<String>, Option<String>, Option<String>, bool, Vec<Option<String>>, Vec<Option<&'static str>>);

fn resolved_of(mapper: &KeyboardMapper, chords: &[ScanCodeChord]) -> Resolved {
    (
        mapper.label(chords),
        mapper.aria_label(chords),
        mapper.electron_accelerator(chords),
        mapper.user_settings_label(chords),
        mapper.is_wysiwyg_keybinding(chords),
        chords.iter().map(|chord| mapper.dispatch(chord)).collect(),
        chords.iter().map(|chord| mapper.single_modifier_dispatch(chord)).collect(),
    )
}

fn resolved(label: &str, aria: &str, electron: Option<&str>, user_settings: &str, wysiwyg: bool, dispatch: &[Option<&str>]) -> Resolved {
    let dispatch: Vec<Option<String>> = dispatch.iter().map(|part| part.map(str::to_string)).collect();
    let single = vec![None; dispatch.len()];
    (Some(label.into()), Some(aria.into()), electron.map(str::to_string), Some(user_settings.into()), wysiwyg, dispatch, single)
}

fn resolve_keybinding(mapper: &KeyboardMapper, keybinding: &str) -> Vec<Resolved> {
    mapper.resolve_keybinding(&parse_keybinding(keybinding)).iter().map(|chords| resolved_of(mapper, chords)).collect()
}

fn resolve_event(mapper: &KeyboardMapper, meta: bool, code: &str) -> Resolved {
    let code = SCAN_CODE_INFO[scan_code(code)].0;
    resolved_of(mapper, &[mapper.resolve_keyboard_event(KeyEvent { code, ctrl: false, shift: false, alt: false, meta }, None, false)])
}

/// `_assertKeybindingTranslation`: the scan code combos of a key code chord as user settings labels of scan codes.
fn translation(mapper: &KeyboardMapper, keybinding: &str) -> Vec<String> {
    let chord = parse_keybinding(keybinding)[0];
    let chords = mapper.key_code_chord_to_scan_code_chords(&chord);
    chords.iter().map(|chord| with_modifiers(chord.modifiers(), ["ctrl", "shift", "alt", "cmd"], "+", SCAN_CODE_INFO[chord.scan_code].0)).collect()
}

#[test]
fn mac_de_ch_mapping() {
    assert_eq!(mapper("mac_de_ch", false).dump_debug_info(), fixture("mac_de_ch.txt"));
}

#[test]
fn mac_en_us_mapping() {
    assert_eq!(mapper("mac_en_us", true).dump_debug_info(), fixture("mac_en_us.txt"));
}

#[test]
fn mac_zh_hant_mapping() {
    assert_eq!(mapper("mac_zh_hant", false).dump_debug_info(), fixture("mac_zh_hant.txt"));
}

#[test]
fn mac_zh_hant2_mapping() {
    assert_eq!(mapper("mac_zh_hant2", false).dump_debug_info(), fixture("mac_zh_hant2.txt"));
}

#[test]
fn mac_de_ch_kb_to_hw() {
    let mapper = mapper("mac_de_ch", false);
    assert_eq!(translation(&mapper, "cmd+1"), ["cmd+Digit1"]);
    assert_eq!(translation(&mapper, "cmd+b"), ["cmd+KeyB"]);
    assert_eq!(translation(&mapper, "shift+cmd+b"), ["shift+cmd+KeyB"]);
    assert_eq!(translation(&mapper, "ctrl+shift+alt+cmd+b"), ["ctrl+shift+alt+cmd+KeyB"]);
    assert_eq!(translation(&mapper, "cmd+z"), ["cmd+KeyY"]);
    assert_eq!(translation(&mapper, "cmd+y"), ["cmd+KeyZ"]);
    assert_eq!(translation(&mapper, "cmd+/"), ["shift+cmd+Digit7"]);
}

#[test]
fn mac_de_ch_resolve() {
    let mapper = mapper("mac_de_ch", false);
    let one = |keybinding| resolve_keybinding(&mapper, keybinding);
    assert_eq!(one("cmd+a"), [resolved("⌘A", "Command+A", Some("Cmd+A"), "cmd+a", true, &[Some("meta+[KeyA]")])]);
    assert_eq!(one("cmd+b"), [resolved("⌘B", "Command+B", Some("Cmd+B"), "cmd+b", true, &[Some("meta+[KeyB]")])]);
    assert_eq!(one("cmd+z"), [resolved("⌘Z", "Command+Z", Some("Cmd+Z"), "cmd+z", true, &[Some("meta+[KeyY]")])]);
    assert_eq!(resolve_event(&mapper, true, "KeyY"), resolved("⌘Z", "Command+Z", Some("Cmd+Z"), "cmd+z", true, &[Some("meta+[KeyY]")]));
    assert_eq!(one("cmd+]"), [resolved("⌃⌥⌘6", "Control+Option+Command+6", Some("Ctrl+Alt+Cmd+6"), "ctrl+alt+cmd+6", true, &[Some("ctrl+alt+meta+[Digit6]")])]);
    assert_eq!(resolve_event(&mapper, true, "BracketRight"), resolved("⌘¨", "Command+¨", None, "cmd+[BracketRight]", false, &[Some("meta+[BracketRight]")]));
    assert_eq!(one("shift+]"), [resolved("⌃⌥9", "Control+Option+9", Some("Ctrl+Alt+9"), "ctrl+alt+9", true, &[Some("ctrl+alt+[Digit9]")])]);
    assert_eq!(one("cmd+/"), [resolved("⇧⌘7", "Shift+Command+7", Some("Shift+Cmd+7"), "shift+cmd+7", true, &[Some("shift+meta+[Digit7]")])]);
    assert_eq!(one("cmd+shift+/"), [resolved("⇧⌘'", "Shift+Command+'", None, "shift+cmd+[Minus]", false, &[Some("shift+meta+[Minus]")])]);
    assert_eq!(
        one("cmd+k cmd+\\"),
        [resolved("⌘K ⌃⇧⌥⌘7", "Command+K Control+Shift+Option+Command+7", None, "cmd+k ctrl+shift+alt+cmd+7", true, &[Some("meta+[KeyK]"), Some("ctrl+shift+alt+meta+[Digit7]")])]
    );
    assert_eq!(one("cmd+k cmd+="), [resolved("⌘K ⇧⌘0", "Command+K Shift+Command+0", None, "cmd+k shift+cmd+0", true, &[Some("meta+[KeyK]"), Some("shift+meta+[Digit0]")])]);
    assert_eq!(one("cmd+down"), [resolved("⌘↓", "Command+DownArrow", Some("Cmd+Down"), "cmd+down", true, &[Some("meta+[ArrowDown]")])]);
    assert_eq!(one("cmd+numpad0"), [resolved("⌘NumPad0", "Command+NumPad0", None, "cmd+numpad0", true, &[Some("meta+[Numpad0]")])]);
    assert_eq!(one("cmd+home"), [resolved("⌘Home", "Command+Home", Some("Cmd+Home"), "cmd+home", true, &[Some("meta+[Home]")])]);
    assert_eq!(resolve_event(&mapper, true, "Home"), resolved("⌘Home", "Command+Home", Some("Cmd+Home"), "cmd+home", true, &[Some("meta+[Home]")]));
    assert_eq!(one("cmd+[comma] cmd+/"), [resolved("⌘, ⇧⌘7", "Command+, Shift+Command+7", None, "cmd+[Comma] shift+cmd+7", false, &[Some("meta+[Comma]"), Some("shift+meta+[Digit7]")])]);
    assert_single_modifier_meta(&mapper);
}

/// 'resolveKeyboardEvent Single Modifier MetaLeft+' and 'MetaRight+'.
fn assert_single_modifier_meta(mapper: &KeyboardMapper) {
    for code in ["MetaLeft", "MetaRight"] {
        let expected = (Some("⌘".into()), Some("Command".into()), None, Some("cmd".into()), true, vec![None], vec![Some("meta")]);
        assert_eq!(resolve_event(mapper, true, code), expected, "{code}");
    }
}

#[test]
fn mac_en_us_resolve() {
    let mapper = mapper("mac_en_us", true);
    assert_eq!(
        resolve_keybinding(&mapper, "cmd+[comma] cmd+/"),
        [resolved("⌘, ⌘/", "Command+, Command+/", None, "cmd+, cmd+/", true, &[Some("meta+[Comma]"), Some("meta+[Slash]")])]
    );
    assert_single_modifier_meta(&mapper);
}

#[test]
fn mac_en_us_alt_gr_maps_to_ctrl_alt() {
    let mapper = KeyboardMapper::new(true, &raw_mapping("mac_en_us"), true);
    let chord = mapper.resolve_keyboard_event(KeyEvent { code: "KeyZ", ctrl: false, shift: false, alt: false, meta: false }, None, true);
    assert_eq!(resolved_of(&mapper, &[chord]), resolved("⌃⌥Z", "Control+Option+Z", Some("Ctrl+Alt+Z"), "ctrl+alt+z", true, &[Some("ctrl+alt+[KeyZ]")]));
}

/// 'issue #28237 resolveKeybinding Cmd+C'.
#[test]
fn mac_zh_hant_cmd_c() {
    let mapper = mapper("mac_zh_hant", false);
    assert_eq!(resolve_keybinding(&mapper, "cmd+c"), [resolved("⌘C", "Command+C", Some("Cmd+C"), "cmd+c", true, &[Some("meta+[KeyC]")])]);
}

/// nativeKeyboardLayout.ts `createKeyboardMapper`: key code dispatch for `keyboard.dispatch: keyCode`, an empty
/// key map and Dvorak - QWERTY ⌘.
#[test]
fn for_layout_falls_back_to_key_codes() {
    let raw = raw_mapping("mac_en_us");
    let us = serde_json::json!({ "id": "com.apple.keylayout.US" });
    assert!(for_layout(&us, &raw, false, false).is_some_and(|mapper| mapper.is_us_standard));
    assert!(for_layout(&us, &raw, true, false).is_none());
    assert!(for_layout(&us, &serde_json::json!({}), false, false).is_none());
    assert!(for_layout(&serde_json::json!({ "id": "com.apple.keylayout.DVORAK-QWERTYCMD" }), &raw, false, false).is_none());
}
