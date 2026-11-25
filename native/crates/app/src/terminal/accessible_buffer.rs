//! bufferContentTracker.ts and terminalAccessibleBufferProvider.ts: the buffer as the accessible view's lines, a
//! wrapped row joined to the row it continues, blank lines left out, and the editor line of each command.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;

use super::TerminalInstance;

/// The accessible view's content: its lines, and per command (`_getCommandsWithEditorLine`) its 1-based line,
/// command line and exit code.
#[derive(Debug, Default, PartialEq)]
pub struct AccessibleBuffer {
    pub lines: Vec<String>,
    pub commands: Vec<(usize, String, Option<i32>)>,
}

impl TerminalInstance {
    pub fn accessible_buffer(&self) -> AccessibleBuffer {
        let history = self.term.history_size() as i64;
        let total = history + self.term.screen_lines() as i64;
        let mut lines = Vec::new();
        // `bufferToEditorLineMapping`: a buffer line maps to the editor line its text lands on.
        let mut editor_line_of = Vec::with_capacity(total as usize);
        let mut current = String::new();
        for line in 0..total {
            editor_line_of.push(lines.len());
            let row = &self.term.grid()[Line((line - history) as i32)];
            let wrapped = line + 1 < total && row[Column(self.term.columns() - 1)].flags.contains(Flags::WRAPLINE);
            let text: String = (0..self.term.columns()).map(|column| row[Column(column)].c).collect();
            // `translateToString(!isWrapped)`: a row that continues keeps its trailing blanks.
            current += if wrapped { &text } else { text.trim_end_matches([' ', '\0']) };
            if !current.is_empty() && !wrapped {
                lines.push(std::mem::take(&mut current));
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
        let mut commands = Vec::new();
        let finished = self.commands.iter().map(|command| (command.prompt_line.or(command.command_line), command.command.clone(), command.exit_code));
        let running = self.current.command_line.map(|line| (Some(line), self.current.command.clone(), None));
        for (line, command, exit_code) in finished.chain(running) {
            let Some(&editor_line) = line.and_then(|line| editor_line_of.get(usize::try_from(line).ok()?)) else { continue };
            commands.push((editor_line + 1, command, exit_code));
        }
        AccessibleBuffer { lines, commands }
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::term::Config;

    use super::super::{ShellLaunchConfig, TerminalInstance, colors};

    #[test]
    fn blank_rows_drop_out_and_wrapped_rows_join() {
        let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let colors = colors::TerminalColors::new(&crate::theme::load_theme(&[themes], "Dark Modern").unwrap());
        let launch = ShellLaunchConfig { extension_pty: true, cwd: std::env::temp_dir(), ..Default::default() };
        let (mut instance, _) = TerminalInstance::new(1, launch, 10, 4, Config { scrolling_history: 100, ..Config::default() });
        let command = |name: &str, out: &str| format!("\x1b]633;A\x07$ \x1b]633;B\x07{name}\r\n\x1b]633;C\x07{out}\r\n\r\n\x1b]633;D;1\x07");
        instance.process(format!("{}{}\x1b]633;A\x07$ ", command("a", "0123456789abc"), command("b", "x")).as_bytes(), &colors);
        let buffer = instance.accessible_buffer();
        assert_eq!(buffer.lines, ["$ a", "0123456789abc", "$ b", "x", "$"]);
        assert_eq!(buffer.commands, [(1, "a".into(), Some(1)), (3, "b".into(), Some(1))]);
    }
}
