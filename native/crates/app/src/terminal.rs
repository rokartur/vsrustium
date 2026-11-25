//! The integrated terminal's model: one `TerminalInstance` per terminal (terminalInstance.ts), its shell in a
//! PTY (terminalProcess.ts over node-pty) and its screen in alacritty_terminal standing in for xterm.js.

pub mod colors;
pub mod keys;
pub mod mouse;
pub mod profiles;
pub mod pty;
pub mod skip_shell;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{ClearMode, Handler, Processor, Rgb};
use futures::channel::mpsc::{UnboundedReceiver, unbounded};

use self::pty::{Pty, PtyEvent};

struct GridSize {
    cols: u16,
    rows: u16,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }

    fn screen_lines(&self) -> usize {
        self.rows as usize
    }

    fn columns(&self) -> usize {
        self.cols as usize
    }
}

/// What a terminal launches (terminal.ts `IShellLaunchConfig`, resolved).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShellLaunchConfig {
    /// The terminal's static name; the process name shows when it is `None`.
    pub name: Option<String>,
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub icon: Option<String>,
    pub color: Option<String>,
}

/// The events alacritty_terminal raises while it parses, handled after each chunk.
#[derive(Clone, Default)]
pub struct Listener(Rc<RefCell<Vec<Event>>>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        self.0.borrow_mut().push(event);
    }
}

pub struct TerminalInstance {
    /// `instanceId`, unique in the window, counting from 1.
    pub id: u32,
    pub launch: ShellLaunchConfig,
    pub term: Term<Listener>,
    parser: Processor,
    events: Listener,
    pty: Option<Pty>,
    pub pid: Option<i32>,
    /// The name the user gave with Rename (`TitleEventSource.Api` from the rename command).
    pub user_title: Option<String>,
    /// `${process}`: the foreground process of the PTY.
    pub process_title: String,
    /// `${sequence}`: the title the process set with OSC 0 or 2.
    pub sequence_title: Option<String>,
    /// `${cwd}` of the shell.
    pub cwd: Option<String>,
    /// `exitCode` once the process exited; `Some(None)` when a signal ended it.
    pub exit_code: Option<Option<i32>>,
    pub spawn_error: Option<String>,
    /// The cell size in pixels, for `CSI 14 t`.
    pub cell_size: (f32, f32),
    /// Text the process asked to put on the clipboard (OSC 52).
    pub clipboard_requests: Vec<String>,
    /// The bell rang since it was last read.
    pub bell: bool,
    launched: Instant,
    /// `ProcessState.KilledByUser`: the kill command ended the process.
    pub killed_by_user: bool,
    /// What `Term` was last configured with: `set_options` resets its history size and damage, so it runs on change only.
    config: Config,
}

/// terminalProcessManager.ts `ErrorLaunchThresholdDuration`: an exit this soon after launch is a failed launch.
const ERROR_LAUNCH_THRESHOLD: Duration = Duration::from_millis(500);

impl TerminalInstance {
    pub fn new(id: u32, launch: ShellLaunchConfig, cols: u16, rows: u16, config: Config) -> (Self, Option<UnboundedReceiver<PtyEvent>>) {
        let events = Listener::default();
        let term = Term::new(config.clone(), &GridSize { cols, rows }, events.clone());
        let process_title = launch.executable.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let mut instance = TerminalInstance {
            id,
            launch,
            term,
            parser: Processor::new(),
            events,
            pty: None,
            pid: None,
            user_title: None,
            process_title,
            sequence_title: None,
            cwd: None,
            exit_code: None,
            spawn_error: None,
            cell_size: (0., 0.),
            clipboard_requests: Vec::new(),
            bell: false,
            launched: Instant::now(),
            killed_by_user: false,
            config,
        };
        let (sender, receiver) = unbounded();
        let launch = &instance.launch;
        match Pty::spawn(&launch.executable, &launch.args, &launch.cwd, &launch.env, cols, rows, sender) {
            Ok(pty) => {
                instance.pid = Some(pty.pid);
                instance.pty = Some(pty);
                (instance, Some(receiver))
            }
            Err(error) => {
                // terminalProcess.ts `_validateCwd` and `_validateExecutable` messages before the spawn fails.
                let message = format!("The terminal process failed to launch: A native exception occurred during launch ({error}).");
                instance.spawn_error = Some(message);
                (instance, None)
            }
        }
    }

    /// The tab title, terminal.integrated.tabs.title `${process}` unless a name was given.
    pub fn title(&self) -> String {
        if let Some(title) = &self.user_title {
            return title.clone();
        }
        if let Some(name) = &self.launch.name {
            return name.clone();
        }
        self.process_title.clone()
    }

    /// Feeds the process's output to the parser and acts on what it raised.
    pub fn process(&mut self, bytes: &[u8], colors: &colors::TerminalColors) {
        self.parser.advance(&mut self.term, bytes);
        let events = std::mem::take(&mut *self.events.0.borrow_mut());
        for event in events {
            match event {
                Event::PtyWrite(text) => self.write_to_pty(text.as_bytes()),
                Event::Title(title) => self.sequence_title = Some(title),
                Event::ResetTitle => self.sequence_title = None,
                Event::Bell => self.bell = true,
                Event::ClipboardStore(_, text) => self.clipboard_requests.push(text),
                Event::ColorRequest(index, format) => {
                    let color = match index {
                        0..256 => colors.palette[index],
                        256 => colors.foreground,
                        257 => colors.background,
                        _ => colors.cursor,
                    };
                    let byte = |channel: f32| (channel * 255.).round() as u8;
                    let reply = format(Rgb { r: byte(color.r), g: byte(color.g), b: byte(color.b) });
                    self.write_to_pty(reply.as_bytes());
                }
                Event::TextAreaSizeRequest(format) => {
                    let size = WindowSize { num_lines: self.term.screen_lines() as u16, num_cols: self.term.columns() as u16, cell_width: self.cell_size.0 as u16, cell_height: self.cell_size.1 as u16 };
                    self.write_to_pty(format(size).as_bytes());
                }
                _ => {}
            }
        }
    }

    /// Reads the foreground process and the cwd the shell is in, for `${process}` and `${cwd}`. Returns whether
    /// either changed.
    pub fn refresh_process(&mut self) -> bool {
        let Some(pty) = &self.pty else { return false };
        let (process, cwd) = (pty.foreground_process(), pty.cwd());
        let changed = process.as_ref().is_some_and(|process| *process != self.process_title) || cwd != self.cwd;
        if let Some(process) = process {
            self.process_title = process;
        }
        self.cwd = cwd;
        changed
    }

    pub fn exited(&mut self, code: Option<i32>) {
        self.exit_code = Some(code);
        self.pty = None;
    }

    /// `sendText` and typed input: the terminal follows the input (xterm.js `scrollOnUserInput`).
    pub fn input(&mut self, data: &str) {
        self.term.scroll_display(Scroll::Bottom);
        self.term.selection = None;
        self.write_to_pty(data.as_bytes());
    }

    /// xterm.js `paste`: line breaks become carriage returns, wrapped in bracketed paste mode markers when the
    /// process asked for them.
    pub fn paste(&mut self, text: &str) {
        let data = paste_data(text, self.term.mode().contains(TermMode::BRACKETED_PASTE));
        self.input(&data);
    }

    fn write_to_pty(&mut self, data: &[u8]) {
        if let Some(pty) = &mut self.pty {
            pty.write(data);
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if self.term.columns() == cols as usize && self.term.screen_lines() == rows as usize {
            return;
        }
        self.term.resize(GridSize { cols, rows });
        if let Some(pty) = &self.pty {
            pty.resize(cols, rows);
        }
    }

    /// `kill`: SIGHUP to the shell (node-pty `kill()`).
    pub fn kill(&mut self) {
        self.killed_by_user = true;
        self.pty = None;
    }

    /// terminalInstance.ts `parseExitResult`: the message of a non-zero exit, worded for a failed launch when the
    /// process ended within `ErrorLaunchThresholdDuration`.
    pub fn exit_message(&self, code: i32) -> Option<String> {
        if code == 0 {
            return None;
        }
        let mut command_line = self.launch.executable.to_string_lossy().into_owned();
        // `args.map(a => ` '${a}'`).join()`: Array.join's default separator is a comma.
        command_line += &self.launch.args.iter().map(|arg| format!(" '{arg}'")).collect::<Vec<_>>().join(",");
        Some(match self.failed_during_launch() {
            true => format!("The terminal process \"{command_line}\" failed to launch (exit code: {code})."),
            false => format!("The terminal process \"{command_line}\" terminated with exit code: {code}."),
        })
    }

    /// `ProcessState.KilledDuringLaunch`.
    pub fn failed_during_launch(&self) -> bool {
        !self.killed_by_user && self.launched.elapsed() < ERROR_LAUNCH_THRESHOLD
    }

    /// xterm.js `clear`: the cursor's line becomes the buffer's first and the scrollback goes.
    pub fn clear(&mut self) {
        let cursor = self.term.grid().cursor.point;
        self.term.scroll_display(Scroll::Bottom);
        self.term.scroll_up(cursor.line.0.max(0) as usize);
        self.term.clear_screen(ClearMode::Saved);
        self.term.goto(0, cursor.column.0);
        self.term.selection = None;
    }

    /// The settings xterm.js reads live (`scrollback`, `wordSeparators`, `cursorStyle`) changed.
    pub fn configure(&mut self, config: Config) {
        if config != self.config {
            self.term.set_options(config.clone());
            self.config = config;
        }
    }

    pub fn scroll(&mut self, scroll: Scroll) {
        self.term.scroll_display(scroll);
    }

    /// Starts a selection at a cell of the viewport (line 0 is the viewport's first line).
    pub fn start_selection(&mut self, kind: SelectionType, line: i32, column: usize, side: Side) {
        let point = self.viewport_point(line, column);
        self.term.selection = Some(Selection::new(kind, point, side));
    }

    pub fn extend_selection(&mut self, line: i32, column: usize, side: Side) {
        let point = self.viewport_point(line, column);
        if let Some(selection) = &mut self.term.selection {
            selection.update(point, side);
        }
    }

    pub fn select_all(&mut self) {
        let top = Point::new(Line(-(self.term.history_size() as i32)), Column(0));
        let bottom = Point::new(Line(self.term.screen_lines() as i32 - 1), Column(self.term.columns() - 1));
        let mut selection = Selection::new(SelectionType::Simple, top, Side::Left);
        selection.update(bottom, Side::Right);
        self.term.selection = Some(selection);
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.selection_to_string().filter(|text| !text.is_empty())
    }

    fn viewport_point(&self, line: i32, column: usize) -> Point {
        let offset = self.term.grid().display_offset() as i32;
        let last_line = self.term.screen_lines() as i32 - 1;
        Point::new(Line(line.clamp(0, last_line) - offset), Column(column.min(self.term.columns() - 1)))
    }

    /// Writes `text` as if the shell printed it, for messages the terminal shows itself.
    pub fn write_message(&mut self, text: &str, colors: &colors::TerminalColors) {
        self.process(text.replace('\n', "\r\n").as_bytes(), colors);
    }
}

fn paste_data(text: &str, bracketed: bool) -> String {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if !bracketed {
        return text;
    }
    // xterm Clipboard.ts: ESC becomes U+241B, so pasted text cannot end the bracket and run as typed input.
    format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "\u{241b}"))
}

/// terminalEnvironment.ts `createTerminalEnvironment` for a local terminal: the window's environment, the
/// `terminal.integrated.env.osx` and profile overrides (a null value unsets), then `addTerminalEnvironmentKeys`.
pub fn create_environment(base: impl Iterator<Item = (String, String)>, overrides: &[HashMap<String, Option<String>>], version: &str, locale: &str, detect_locale: &str) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = base.collect();
    for (from, to) in [("VSCODE_NODE_OPTIONS", "NODE_OPTIONS"), ("VSCODE_NODE_REPL_EXTERNAL_MODULE", "NODE_REPL_EXTERNAL_MODULE")] {
        if let Some(value) = env.remove(from) {
            env.insert(to.into(), value);
        }
    }
    env.retain(|key, _| key == "VSCODE_IPC_HOOK_CLI" || !sanitized_away(key));
    for layer in overrides {
        for (key, value) in layer {
            match value {
                Some(value) => env.insert(key.clone(), value.clone()),
                None => env.remove(key),
            };
        }
    }
    env.insert("TERM_PROGRAM".into(), "vscode".into());
    env.insert("TERM_PROGRAM_VERSION".into(), version.into());
    let set_lang = match detect_locale {
        "on" => true,
        "off" => false,
        _ => env.get("LANG").is_none_or(|lang| !lang.ends_with(".UTF-8") && !lang.ends_with(".utf8") && !lang.contains(".euc")),
    };
    if set_lang {
        env.insert("LANG".into(), lang_variable(locale));
    }
    env.insert("COLORTERM".into(), "truecolor".into());
    // node-pty sets TERM to the terminal name terminalProcess.ts passes.
    env.insert("TERM".into(), "xterm-256color".into());
    env
}

/// processes.ts `sanitizeProcessEnvironment`: the variables Electron, VS Code, Snap and GDK set for themselves.
fn sanitized_away(key: &str) -> bool {
    let kept_vscode = ["PORTABLE", "SHELL_LOGIN", "ENV_REPLACE", "ENV_APPEND", "ENV_PREPEND"];
    let has_rest = |prefix: &str| key.strip_prefix(prefix).is_some_and(|rest| !rest.is_empty());
    if has_rest("ELECTRON_") || has_rest("GDK_PIXBUF_") || key == "SNAP" || key.starts_with("SNAP_") {
        return true;
    }
    key.strip_prefix("VSCODE_").is_some_and(|rest| !rest.is_empty() && !kept_vscode.iter().any(|kept| rest.starts_with(kept)))
}

/// `getLangEnvVariable`: a language gets its most prominent region (`en` becomes `en_US.UTF-8`).
fn lang_variable(locale: &str) -> String {
    let mut parts: Vec<String> = locale.split('-').filter(|part| !part.is_empty()).map(str::to_owned).collect();
    match parts.len() {
        0 => return "en_US.UTF-8".into(),
        1 => {
            let variants = [
                ("af", "ZA"), ("am", "ET"), ("be", "BY"), ("bg", "BG"), ("ca", "ES"), ("cs", "CZ"), ("da", "DK"), ("de", "DE"), ("el", "GR"), ("en", "US"),
                ("es", "ES"), ("et", "EE"), ("eu", "ES"), ("fi", "FI"), ("fr", "FR"), ("he", "IL"), ("hr", "HR"), ("hu", "HU"), ("hy", "AM"), ("is", "IS"),
                ("it", "IT"), ("ja", "JP"), ("kk", "KZ"), ("ko", "KR"), ("lt", "LT"), ("nl", "NL"), ("no", "NO"), ("pl", "PL"), ("pt", "BR"), ("ro", "RO"),
                ("ru", "RU"), ("sk", "SK"), ("sl", "SI"), ("sr", "YU"), ("sv", "SE"), ("tr", "TR"), ("uk", "UA"), ("zh", "CN"),
            ];
            if let Some((_, region)) = variants.iter().find(|(language, _)| *language == parts[0]) {
                parts.push(region.to_string());
            }
        }
        _ => parts[1] = parts[1].to_uppercase(),
    }
    format!("{}.UTF-8", parts.join("_"))
}

#[cfg(test)]
mod tests;
