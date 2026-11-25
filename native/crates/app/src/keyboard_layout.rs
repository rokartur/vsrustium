//! The keyboard layout data of platform/keyboardLayout (IKeyboardLayoutInfo, IMacLinuxKeyboardMapping) as
//! native-keymap's keyboard_mac.mm reads it: the current input source and, per DOM key code, the characters of
//! UCKeyTranslate without modifiers, with Shift, with Option (AltGr) and with Shift+Option.

use std::ffi::c_void;

use objc2_foundation::{NSArray, NSString};
use serde_json::{Map, Value, json};

/// The DOM codes with a macOS virtual key code, in the order of chromium's dom_code_data.inc that native-keymap
/// walks (and so the order of the mapping's keys).
const MAC_KEY_CODES: [(&str, u16); 119] = [
    ("KeyA", 0), ("KeyB", 11), ("KeyC", 8), ("KeyD", 2), ("KeyE", 14), ("KeyF", 3), ("KeyG", 5),
    ("KeyH", 4), ("KeyI", 34), ("KeyJ", 38), ("KeyK", 40), ("KeyL", 37), ("KeyM", 46), ("KeyN", 45),
    ("KeyO", 31), ("KeyP", 35), ("KeyQ", 12), ("KeyR", 15), ("KeyS", 1), ("KeyT", 17), ("KeyU", 32),
    ("KeyV", 9), ("KeyW", 13), ("KeyX", 7), ("KeyY", 16), ("KeyZ", 6), ("Digit1", 18), ("Digit2", 19),
    ("Digit3", 20), ("Digit4", 21), ("Digit5", 23), ("Digit6", 22), ("Digit7", 26), ("Digit8", 28), ("Digit9", 25),
    ("Digit0", 29), ("Enter", 36), ("Escape", 53), ("Backspace", 51), ("Tab", 48), ("Space", 49), ("Minus", 27),
    ("Equal", 24), ("BracketLeft", 33), ("BracketRight", 30), ("Backslash", 42), ("Semicolon", 41), ("Quote", 39), ("Backquote", 50),
    ("Comma", 43), ("Period", 47), ("Slash", 44), ("CapsLock", 57), ("F1", 122), ("F2", 120), ("F3", 99),
    ("F4", 118), ("F5", 96), ("F6", 97), ("F7", 98), ("F8", 100), ("F9", 101), ("F10", 109),
    ("F11", 103), ("F12", 111), ("Insert", 114), ("Home", 115), ("PageUp", 116), ("Delete", 117), ("End", 119),
    ("PageDown", 121), ("ArrowRight", 124), ("ArrowLeft", 123), ("ArrowDown", 125), ("ArrowUp", 126), ("NumLock", 71), ("NumpadDivide", 75),
    ("NumpadMultiply", 67), ("NumpadSubtract", 78), ("NumpadAdd", 69), ("NumpadEnter", 76), ("Numpad1", 83), ("Numpad2", 84), ("Numpad3", 85),
    ("Numpad4", 86), ("Numpad5", 87), ("Numpad6", 88), ("Numpad7", 89), ("Numpad8", 91), ("Numpad9", 92), ("Numpad0", 82),
    ("NumpadDecimal", 65), ("IntlBackslash", 10), ("ContextMenu", 110), ("NumpadEqual", 81), ("F13", 105), ("F14", 107), ("F15", 113),
    ("F16", 106), ("F17", 64), ("F18", 79), ("F19", 80), ("F20", 90), ("AudioVolumeMute", 74), ("AudioVolumeUp", 72),
    ("AudioVolumeDown", 73), ("NumpadComma", 95), ("IntlRo", 94), ("IntlYen", 93), ("Lang1", 104), ("Lang2", 102), ("ControlLeft", 59),
    ("ShiftLeft", 56), ("AltLeft", 58), ("MetaLeft", 55), ("ControlRight", 62), ("ShiftRight", 60), ("AltRight", 61), ("MetaRight", 54),
];

/// UCKeyTranslate's modifier key state: Carbon's `shiftKey` (1 << 9) and `optionKey` (1 << 11), shifted right by 8.
const SHIFT: u32 = 2;
const OPTION: u32 = 8;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> *const c_void;
    fn TISCopyCurrentKeyboardLayoutInputSource() -> *const c_void;
    fn TISGetInputSourceProperty(source: *const c_void, key: *const c_void) -> *const c_void;
    fn UCKeyTranslate(
        layout: *const c_void,
        virtual_key_code: u16,
        key_action: u16,
        modifier_key_state: u32,
        keyboard_type: u32,
        options: u32,
        dead_key_state: *mut u32,
        max_length: usize,
        actual_length: *mut usize,
        unicode: *mut u16,
    ) -> i32;
    fn LMGetKbdLast() -> u8;
    static kTISPropertyUnicodeKeyLayoutData: *const c_void;
    static kTISPropertyInputSourceID: *const c_void;
    static kTISPropertyLocalizedName: *const c_void;
    static kTISPropertyInputSourceLanguages: *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFRelease(object: *const c_void);
}

/// `getCurrentKeyboardLayout()`: `{ id, localizedName, lang }` of the current input source (IMacKeyboardLayoutInfo).
pub fn current_layout_info() -> Value {
    let mut info = Map::new();
    unsafe {
        let source = TISCopyCurrentKeyboardInputSource();
        let string = |key| (TISGetInputSourceProperty(source, key) as *const NSString).as_ref().map(|value| Value::from(value.to_string()));
        if let Some(id) = string(kTISPropertyInputSourceID) {
            info.insert("id".into(), id);
        }
        if let Some(name) = string(kTISPropertyLocalizedName) {
            info.insert("localizedName".into(), name);
        }
        let languages = (TISGetInputSourceProperty(source, kTISPropertyInputSourceLanguages) as *const NSArray<NSString>).as_ref();
        if let Some(lang) = languages.and_then(|languages| languages.firstObject()) {
            info.insert("lang".into(), Value::from(lang.to_string()));
        }
        CFRelease(source);
    }
    Value::Object(info)
}

/// `getKeyMap()`: per DOM code, the four characters and whether each is a dead key; `{}` when the input source
/// has no Unicode layout data.
pub fn current_raw_mapping() -> Value {
    let mut mapping = Map::new();
    unsafe {
        let mut source = TISCopyCurrentKeyboardInputSource();
        let mut data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
        // Japanese input sources have no layout data; their keyboard layout source has (native-keymap does the same).
        if data.is_null() {
            CFRelease(source);
            source = TISCopyCurrentKeyboardLayoutInputSource();
            data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
        }
        if !data.is_null() {
            let layout = CFDataGetBytePtr(data) as *const c_void;
            for (code, key_code) in MAC_KEY_CODES {
                let mut entry = Map::new();
                for (name, modifiers) in [("value", 0), ("withShift", SHIFT), ("withAltGr", OPTION), ("withShiftAltGr", SHIFT | OPTION)] {
                    let (is_dead_key, text) = key_code_to_text(layout, key_code, modifiers);
                    entry.insert(name.into(), Value::from(text));
                    entry.insert(format!("{name}IsDeadKey"), Value::from(is_dead_key));
                }
                mapping.insert(code.into(), Value::Object(entry));
            }
        }
        CFRelease(source);
    }
    Value::Object(mapping)
}

/// `ConvertKeyCodeToText`: a dead key gives the character of pressing it twice. native-keymap passes
/// `kUCKeyTranslateNoDeadKeysBit` (0) as the options, so dead keys are processed.
unsafe fn key_code_to_text(layout: *const c_void, key_code: u16, modifiers: u32) -> (bool, String) {
    let keyboard_type = unsafe { LMGetKbdLast() } as u32;
    let mut dead_key_state = 0;
    let mut length = 0;
    let mut unit = 0u16;
    let mut status = unsafe { UCKeyTranslate(layout, key_code, 0, modifiers, keyboard_type, 0, &mut dead_key_state, 1, &mut length, &mut unit) };
    let mut is_dead_key = false;
    if status == 0 && length == 0 && dead_key_state != 0 {
        is_dead_key = true;
        status = unsafe { UCKeyTranslate(layout, key_code, 0, modifiers, keyboard_type, 0, &mut dead_key_state, 1, &mut length, &mut unit) };
    }
    match char::from_u32(unit as u32) {
        Some(character) if status == 0 && length == 1 && !is_control(character) => (is_dead_key, character.to_string()),
        _ => (false, String::new()),
    }
}

/// `std::iscntrl` of macOS's libc on a UTF-16 unit: C0, DEL and C1.
fn is_control(character: char) -> bool {
    matches!(character, '\0'..='\x1f' | '\x7f'..='\u{9f}')
}

/// keybindingService.ts `_dumpDebugInfoJSON`, the text of Developer: Inspect Key Mappings (JSON).
pub fn dump_debug_info_json() -> String {
    let info = json!({ "layout": current_layout_info(), "rawMapping": current_raw_mapping() });
    let mut text = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut text, serde_json::ser::PrettyFormatter::with_indent(b"\t"));
    serde::Serialize::serialize(&info, &mut serializer).expect("JSON values serialize");
    String::from_utf8(text).expect("serde_json writes UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_mapping_has_every_dom_code_with_its_four_characters() {
        let mapping = current_raw_mapping();
        let mapping = mapping.as_object().expect("an object");
        assert_eq!(mapping.keys().map(String::as_str).collect::<Vec<_>>(), MAC_KEY_CODES.map(|(code, _)| code));
        assert_eq!(mapping["ShiftLeft"], json!({ "value": "", "valueIsDeadKey": false, "withShift": "", "withShiftIsDeadKey": false, "withAltGr": "", "withAltGrIsDeadKey": false, "withShiftAltGr": "", "withShiftAltGrIsDeadKey": false }));
        assert_eq!(mapping["Space"]["value"], " ");
    }

    #[test]
    fn control_characters_are_left_out_as_iscntrl() {
        assert!(is_control('\r') && is_control('\x7f') && is_control('\u{85}'));
        assert!(!is_control(' ') && !is_control('a') && !is_control('\u{a0}'));
    }
}
