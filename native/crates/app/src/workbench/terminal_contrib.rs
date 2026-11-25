//! The small terminalContrib actions: sendSequence, sendSignal and zoom (font size commands and Cmd+wheel).

use gpui::{Context, ScrollDelta, Window};
use serde_json::json;

use super::Workbench;
use super::terminal_view::TerminalInput;
use crate::commands::Outcome;
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Entry, Event, Item, Kind, QuickInput};

/// terminal.ts `defaultTerminalFontSize` on macOS.
const DEFAULT_FONT_SIZE: f64 = 12.;
const MANUAL_SIGNAL: &str = "Manually enter signal";
/// terminal.sendSignal.contribution.ts: the signals the picker offers, with their descriptions.
const SIGNALS: [(&str, &str); 9] = [
    ("SIGINT", "Interrupt process (Ctrl+C)"),
    ("SIGTERM", "Terminate process gracefully"),
    ("SIGKILL", "Force kill process"),
    ("SIGSTOP", "Stop process"),
    ("SIGCONT", "Continue process"),
    ("SIGHUP", "Hangup"),
    ("SIGQUIT", "Quit process"),
    ("SIGUSR1", "User-defined signal 1"),
    ("SIGUSR2", "User-defined signal 2"),
];

impl Workbench {
    pub(super) fn register_terminal_contrib_actions(&mut self) {
        self.commands.register("workbench.action.terminal.sendSequence", |workbench, args, window, cx| {
            match args.first().and_then(|arg| arg["text"].as_str()).filter(|text| !text.is_empty()) {
                Some(text) => workbench.send_sequence(text),
                None => workbench.show_send_sequence_input(window, cx),
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.sendSignal", |workbench, args, window, cx| {
            match args.first().and_then(|arg| arg["signal"].as_str()).filter(|signal| !signal.is_empty()) {
                Some(signal) => workbench.send_signal(signal),
                None => workbench.show_signal_picker(window, cx),
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.fontZoomIn", |workbench, _, _, _| workbench.zoom_terminal_font(1.));
        self.commands.register("workbench.action.terminal.fontZoomOut", |workbench, _, _, _| workbench.zoom_terminal_font(-1.));
        self.commands.register("workbench.action.terminal.fontZoomReset", |workbench, _, _, _| {
            workbench.update_setting("terminal.integrated.fontSize", json!(DEFAULT_FONT_SIZE));
            Outcome::done()
        });
    }

    /// `FontZoomIn` and `FontZoomOut`: the font size setting by one, clamped to 6..100 (`clampTerminalFontSize`).
    fn zoom_terminal_font(&mut self, delta: f64) -> Outcome {
        if let Some(size) = self.setting("terminal.integrated.fontSize").as_f64() {
            self.update_setting("terminal.integrated.fontSize", json!(clamp_font_size(size + delta)));
        }
        Outcome::done()
    }

    /// `TerminalMouseWheelZoomContribution` with Cmd or Ctrl held: a wheel notch is one point, a trackpad gesture
    /// one point per 5 pixels.
    pub(super) fn terminal_wheel_zoom(&mut self, delta: ScrollDelta) {
        let steps = match delta {
            ScrollDelta::Lines(lines) => lines.y.signum(),
            ScrollDelta::Pixels(pixels) => {
                let total = self.terminal_view.zoom_remainder + f32::from(pixels.y) / 5.;
                self.terminal_view.zoom_remainder = total.fract();
                total.trunc()
            }
        };
        if steps != 0. {
            let _ = self.zoom_terminal_font(f64::from(steps));
        }
    }

    /// `terminalSendSequenceCommand`: the text with its variables resolved against the first folder.
    fn send_sequence(&mut self, text: &str) {
        let resolved = match self.resolve_variables(&json!(text), None) {
            Ok(resolved) => resolved.as_str().unwrap_or(text).to_owned(),
            Err(message) => return self.notify_error(message),
        };
        if let Some(instance) = self.terminal_view.active_instance_mut() {
            instance.send_text(&resolved, false);
        }
    }

    fn show_send_sequence_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut input = QuickInput::new(0, Kind::InputBox);
        input.placeholder = Some("Enter sequence to send (supports \\n, \\r, \\xAB)".into());
        input.prompt = Some("Enter sequence to send to the terminal".into());
        let id = self.quick_inputs.add(input, Owner::Terminal(TerminalInput::SendSequence));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    fn show_signal_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut entries: Vec<Entry> = SIGNALS.iter().enumerate().map(|(index, (label, description))| Entry::Item(Item { handle: index as i64, label: (*label).into(), description: Some((*description).into()), ..Item::default() })).collect();
        entries.push(Entry::Separator(None));
        entries.push(Entry::Item(Item { handle: SIGNALS.len() as i64, label: MANUAL_SIGNAL.into(), ..Item::default() }));
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select signal to send to terminal process".into());
        input.set_items(entries);
        let id = self.quick_inputs.add(input, Owner::Terminal(TerminalInput::Signal));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    /// `instance.sendSignal`: node-pty `kill(signal)` on the shell process.
    fn send_signal(&mut self, signal: &str) {
        let Some(number) = signal_number(signal) else {
            self.notify_error(format!("Unknown signal: {signal}"));
            return;
        };
        if let Some(instance) = self.terminal_view.active_instance_mut() {
            instance.send_signal(number);
        }
    }

    pub(super) fn terminal_contrib_input_event(&mut self, id: i64, event: Event, input: TerminalInput, window: &mut Window, cx: &mut Context<Self>) {
        let Event::Accept(_) = event else { return };
        let Some(quick_input) = self.quick_inputs.get(id) else { return };
        let value = quick_input.input_box.value.clone();
        let picked = quick_input.selected_handles().first().copied();
        self.hide_quick_input_by_id(id, window, cx);
        match input {
            TerminalInput::SendSequence if !value.is_empty() => self.send_sequence(&unescape_sequence(&value)),
            TerminalInput::Signal => match picked.map(|handle| handle as usize) {
                Some(index) if index < SIGNALS.len() => self.send_signal(SIGNALS[index].0),
                Some(_) => {
                    let mut input = QuickInput::new(0, Kind::InputBox);
                    input.prompt = Some("Enter signal name (e.g., SIGTERM, SIGKILL)".into());
                    let id = self.quick_inputs.add(input, Owner::Terminal(TerminalInput::ManualSignal));
                    self.show_quick_input(id, window, cx);
                    self.flush_quick_input_events(window, cx);
                }
                None => {}
            },
            TerminalInput::ManualSignal if !value.is_empty() => self.send_signal(&value),
            _ => {}
        }
    }
}

fn clamp_font_size(size: f64) -> f64 {
    size.clamp(6., 100.)
}

/// The typed sequence: `\n`, `\r` and `\xNN` become the characters they name.
fn unescape_sequence(text: &str) -> String {
    let text = text.replace("\\n", "\n").replace("\\r", "\r");
    let mut result = String::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find("\\x") {
        let hex = rest.get(start + 2..start + 4).filter(|hex| hex.chars().all(|char| char.is_ascii_hexdigit()));
        let Some(hex) = hex else {
            result.push_str(&rest[..start + 2]);
            rest = &rest[start + 2..];
            continue;
        };
        result.push_str(&rest[..start]);
        result.push(char::from(u8::from_str_radix(hex, 16).expect("two hex digits")));
        rest = &rest[start + 4..];
    }
    result.push_str(rest);
    result
}

/// Node's `os.constants.signals` names (with or without `SIG`) or a number.
fn signal_number(signal: &str) -> Option<i32> {
    if let Ok(number) = signal.parse() {
        return Some(number);
    }
    let name = signal.trim().to_ascii_uppercase();
    let name = name.strip_prefix("SIG").unwrap_or(&name);
    Some(match name {
        "HUP" => libc::SIGHUP,
        "INT" => libc::SIGINT,
        "QUIT" => libc::SIGQUIT,
        "KILL" => libc::SIGKILL,
        "TERM" => libc::SIGTERM,
        "STOP" => libc::SIGSTOP,
        "CONT" => libc::SIGCONT,
        "TSTP" => libc::SIGTSTP,
        "USR1" => libc::SIGUSR1,
        "USR2" => libc::SIGUSR2,
        "WINCH" => libc::SIGWINCH,
        "ALRM" => libc::SIGALRM,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequences_unescape_newlines_and_hex() {
        assert_eq!(unescape_sequence("a\\nb\\r\\x1b[A\\xZZ"), "a\nb\r\x1b[A\\xZZ");
    }

    #[test]
    fn signals_and_font_sizes() {
        assert_eq!(signal_number("SIGTERM"), Some(libc::SIGTERM));
        assert_eq!(signal_number("kill"), Some(libc::SIGKILL));
        assert_eq!(signal_number("9"), Some(9));
        assert_eq!(signal_number("SIGNOPE"), None);
        assert_eq!(clamp_font_size(120.), 100.);
        assert_eq!(clamp_font_size(5.), 6.);
    }
}
