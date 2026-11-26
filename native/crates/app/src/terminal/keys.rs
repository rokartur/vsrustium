//! xterm.js `evaluateKeyboardEvent` (common/input/Keyboard.ts) for macOS with `macOptionIsMeta` off: the bytes a key
//! press sends to the shell, or a scroll the terminal does itself.

use gpui::Keystroke;

const ESC: &str = "\x1b";

#[derive(Debug, PartialEq)]
pub enum KeyResult {
    Send(String),
    PageUp,
    PageDown,
    SelectAll,
    /// The key sends nothing: a command key, or a modifier combination xterm.js ignores.
    None,
}

pub fn evaluate(keystroke: &Keystroke, application_cursor: bool, mac_option_is_meta: bool) -> KeyResult {
    let m = &keystroke.modifiers;
    // CoreBrowserTerminal.ts `_isThirdLevelShift`: Option without Ctrl or Cmd on a key past keyCode 47 is left to
    // keypress, which types the key's character; a key without one (F1 to F12) sends nothing and reaches the workbench.
    let below_key_code_48 = matches!(keystroke.key.as_str(), "backspace" | "tab" | "enter" | "escape" | "space" | "pageup" | "pagedown" | "end" | "home" | "left" | "up" | "right" | "down" | "insert" | "delete");
    if !mac_option_is_meta && m.alt && !m.control && !m.platform && !below_key_code_48 {
        return match keystroke.key_char.as_deref() {
            Some(text) if !text.is_empty() => KeyResult::Send(text.into()),
            _ => KeyResult::None,
        };
    }
    let modifiers = m.shift as u8 | (m.alt as u8) << 1 | (m.control as u8) << 2 | (m.platform as u8) << 3;
    let csi = |final_byte: &str| -> String {
        match modifiers {
            0 => format!("{ESC}[{final_byte}"),
            _ => format!("{ESC}[1;{}{final_byte}", modifiers + 1),
        }
    };
    let cursor = |final_byte: &str| -> KeyResult {
        if m.platform {
            return KeyResult::None;
        }
        KeyResult::Send(match (modifiers, application_cursor) {
            (0, true) => format!("{ESC}O{final_byte}"),
            _ => csi(final_byte),
        })
    };
    let home_end = |final_byte: &str| -> KeyResult {
        KeyResult::Send(match (modifiers, application_cursor) {
            (0, true) => format!("{ESC}O{final_byte}"),
            _ => csi(final_byte),
        })
    };
    let tilde = |number: u8| -> KeyResult {
        KeyResult::Send(match modifiers {
            0 => format!("{ESC}[{number}~"),
            _ => format!("{ESC}[{number};{}~", modifiers + 1),
        })
    };
    let function = |final_byte: &str| -> KeyResult {
        KeyResult::Send(match modifiers {
            0 => format!("{ESC}O{final_byte}"),
            _ => format!("{ESC}[1;{}{final_byte}", modifiers + 1),
        })
    };
    match keystroke.key.as_str() {
        "backspace" => {
            let key = if m.control { "\x08" } else { "\x7f" };
            KeyResult::Send(if m.alt { format!("{ESC}{key}") } else { key.into() })
        }
        "tab" if m.shift => KeyResult::Send(format!("{ESC}[Z")),
        "tab" => KeyResult::Send("\t".into()),
        "enter" => KeyResult::Send(if m.alt { format!("{ESC}\r") } else { "\r".into() }),
        "escape" => KeyResult::Send(if m.alt { format!("{ESC}{ESC}") } else { ESC.into() }),
        "left" => cursor("D"),
        "right" => cursor("C"),
        "up" => cursor("A"),
        "down" => cursor("B"),
        "insert" if !m.shift && !m.control => KeyResult::Send(format!("{ESC}[2~")),
        "insert" => KeyResult::None,
        "delete" => tilde(3),
        "home" => home_end("H"),
        "end" => home_end("F"),
        "pageup" if m.shift => KeyResult::PageUp,
        "pageup" if m.control => tilde(5),
        "pageup" => KeyResult::Send(format!("{ESC}[5~")),
        "pagedown" if m.shift => KeyResult::PageDown,
        "pagedown" if m.control => tilde(6),
        "pagedown" => KeyResult::Send(format!("{ESC}[6~")),
        "f1" => function("P"),
        "f2" => function("Q"),
        "f3" => function("R"),
        "f4" => function("S"),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        key => evaluate_character(keystroke, key, mac_option_is_meta),
    }
}

/// The `default:` branch: control characters, meta prefixes and printable keys.
fn evaluate_character(keystroke: &Keystroke, key: &str, mac_option_is_meta: bool) -> KeyResult {
    let m = &keystroke.modifiers;
    let single = key.chars().next().filter(|_| key.chars().count() == 1);
    if m.control && !m.shift && !m.alt && !m.platform {
        let byte = match (key, single) {
            (_, Some(letter @ 'a'..='z')) => letter as u8 - b'a' + 1,
            ("space", _) => 0,
            (_, Some(digit @ '3'..='7')) => digit as u8 - b'3' + 27,
            (_, Some('8')) => 0x7f,
            (_, Some('/')) => 0x1f,
            (_, Some('[')) => 0x1b,
            (_, Some('\\')) => 0x1c,
            (_, Some(']')) => 0x1d,
            _ => return KeyResult::None,
        };
        return KeyResult::Send((byte as char).to_string());
    }
    if mac_option_is_meta && m.alt && !m.platform {
        if key == "space" {
            return KeyResult::Send(format!("{ESC}{}", if m.control { "\0" } else { " " }));
        }
        return match single {
            Some(letter @ 'a'..='z') if m.control => KeyResult::Send(format!("{ESC}{}", (letter as u8 - b'a' + 1) as char)),
            Some(letter @ 'a'..='z') if m.shift => KeyResult::Send(format!("{ESC}{}", letter.to_ascii_uppercase())),
            Some(character) => KeyResult::Send(format!("{ESC}{}", if m.shift { shifted(character) } else { character })),
            None => KeyResult::None,
        };
    }
    if m.platform && !m.alt && !m.control && !m.shift {
        return if key == "a" { KeyResult::SelectAll } else { KeyResult::None };
    }
    if m.control && m.shift {
        return match key {
            "-" => KeyResult::Send("\x1f".into()),
            "2" => KeyResult::Send("\0".into()),
            "6" => KeyResult::Send("\x1e".into()),
            _ => KeyResult::None,
        };
    }
    if m.control || m.platform {
        return KeyResult::None;
    }
    // The keypress/input path: the character the key (with Shift or Option) types.
    match keystroke.key_char.as_deref() {
        Some(text) if !text.is_empty() => KeyResult::Send(text.into()),
        _ if key == "space" => KeyResult::Send(" ".into()),
        _ => KeyResult::None,
    }
}

/// `KEYCODE_KEY_MAPPINGS`: the US layout's shifted character of a digit or punctuation key.
fn shifted(character: char) -> char {
    let (plain, shift) = ("0123456789;=,-./`[\\]'", ")!@#$%^&*(:+<_>?~{|}\"");
    plain.chars().position(|c| c == character).and_then(|index| shift.chars().nth(index)).unwrap_or(character)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> KeyResult {
        let mut keystroke = Keystroke::parse(text).unwrap();
        if keystroke.key_char.is_none() && keystroke.key.chars().count() == 1 && !keystroke.modifiers.control && !keystroke.modifiers.platform {
            let character = if keystroke.modifiers.shift { keystroke.key.to_uppercase() } else { keystroke.key.clone() };
            keystroke.key_char = Some(character);
        }
        evaluate(&keystroke, false, false)
    }

    fn send(text: &str) -> KeyResult {
        KeyResult::Send(text.into())
    }

    #[test]
    fn keys_send_xterm_sequences() {
        assert_eq!(key("a"), send("a"));
        assert_eq!(key("shift-a"), send("A"));
        assert_eq!(key("ctrl-c"), send("\x03"));
        assert_eq!(key("ctrl-space"), send("\0"));
        assert_eq!(key("ctrl-["), send("\x1b"));
        assert_eq!(key("enter"), send("\r"));
        assert_eq!(key("backspace"), send("\x7f"));
        assert_eq!(key("ctrl-backspace"), send("\x08"));
        assert_eq!(key("alt-backspace"), send("\x1b\x7f"));
        assert_eq!(key("shift-tab"), send("\x1b[Z"));
        assert_eq!(key("up"), send("\x1b[A"));
        assert_eq!(key("shift-up"), send("\x1b[1;2A"));
        assert_eq!(key("cmd-up"), KeyResult::None);
        assert_eq!(evaluate(&Keystroke::parse("up").unwrap(), true, false), send("\x1bOA"));
        assert_eq!(key("delete"), send("\x1b[3~"));
        assert_eq!(key("f1"), send("\x1bOP"));
        assert_eq!(key("shift-f5"), send("\x1b[15;2~"));
        assert_eq!(key("shift-pageup"), KeyResult::PageUp);
        assert_eq!(key("cmd-a"), KeyResult::SelectAll);
        assert_eq!(key("cmd-x"), KeyResult::None);
        assert_eq!(key("alt-f2"), KeyResult::None);
        assert_eq!(key("alt-up"), send("\x1b[1;3A"));
        assert_eq!(evaluate(&Keystroke::parse("alt-f2").unwrap(), false, true), send("\x1b[1;3Q"));
    }
}
