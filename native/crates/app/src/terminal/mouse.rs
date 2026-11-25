//! Mouse reports to the process (xterm.js CoreMouseService with its DEFAULT and SGR encodings): the buttons,
//! wheel and motion a program asked for with DECSET 1000, 1002, 1003 and 1006.

use alacritty_terminal::term::TermMode;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    /// A move with no button pressed.
    None,
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MouseAction {
    Down,
    Up,
    Move,
}

/// Whether the program receives this event at all (`CoreMouseService.triggerMouseEvent` protocol restrictions).
pub fn wants(mode: TermMode, button: MouseButton, action: MouseAction) -> bool {
    match action {
        // DECSET 1000 reports presses, releases and the wheel; 1002 adds drags; 1003 every move.
        MouseAction::Move if button == MouseButton::None => mode.contains(TermMode::MOUSE_MOTION),
        MouseAction::Move => mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION),
        _ => mode.intersects(TermMode::MOUSE_MODE),
    }
}

/// The bytes of a report at the 0-based `column` and `row`, `None` when DEFAULT encoding cannot hold the position.
pub fn report(mode: TermMode, button: MouseButton, action: MouseAction, column: usize, row: usize, shift: bool, alt: bool, control: bool) -> Option<String> {
    let mut code: u32 = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::None => 3,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
    };
    if action == MouseAction::Move {
        code += 32;
    }
    code += (shift as u32) << 2 | (alt as u32) << 3 | (control as u32) << 4;
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if action == MouseAction::Up && !matches!(button, MouseButton::WheelUp | MouseButton::WheelDown) { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{end}", column + 1, row + 1));
    }
    // DEFAULT encoding: a release is button 3, and coordinates past 223 do not fit a byte.
    if action == MouseAction::Up {
        code = 3 | (code & !3);
    }
    let encode = |value: usize| -> Option<char> {
        let value = value as u32 + 1 + 32;
        if mode.contains(TermMode::UTF8_MOUSE) { (value < 2048).then(|| char::from_u32(value)).flatten() } else { (value <= 255).then(|| char::from_u32(value)).flatten() }
    };
    Some(format!("\x1b[M{}{}{}", char::from_u32(code + 32)?, encode(column)?, encode(row)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_and_default_reports() {
        let sgr = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(report(sgr, MouseButton::Left, MouseAction::Down, 4, 2, false, false, false).as_deref(), Some("\x1b[<0;5;3M"));
        assert_eq!(report(sgr, MouseButton::Left, MouseAction::Up, 4, 2, false, false, false).as_deref(), Some("\x1b[<0;5;3m"));
        assert_eq!(report(sgr, MouseButton::WheelDown, MouseAction::Down, 0, 0, false, false, true).as_deref(), Some("\x1b[<81;1;1M"));
        let default = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(report(default, MouseButton::Left, MouseAction::Down, 0, 0, false, false, false).as_deref(), Some("\x1b[M !!"));
        assert_eq!(report(default, MouseButton::Right, MouseAction::Up, 1, 1, false, false, false).as_deref(), Some("\x1b[M#\"\""));
        assert_eq!(report(default, MouseButton::Left, MouseAction::Down, 300, 0, false, false, false), None);
        assert!(!wants(default, MouseButton::Left, MouseAction::Move));
        assert!(wants(TermMode::MOUSE_DRAG, MouseButton::Left, MouseAction::Move));
        assert!(!wants(TermMode::MOUSE_DRAG, MouseButton::None, MouseAction::Move));
    }
}
