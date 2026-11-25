//! The integrated terminal's model: one `TerminalInstance` per terminal (terminalInstance.ts), its shell in a
//! PTY (terminalProcess.ts over node-pty) and its screen in alacritty_terminal standing in for xterm.js.

pub mod auto_replies;
pub mod colors;
pub mod completion_item;
pub mod completion_model;
pub mod completion_service;
pub mod keys;
pub mod link_parsing;
pub mod mouse;
pub mod profiles;
pub mod prompt_input;
pub mod environment_collection;
pub mod history;
pub mod sticky_scroll;
pub mod marks;
pub mod injection;
pub mod pty;
pub mod osc99;
pub mod search;
pub mod serialize_html;
pub mod shell_integration;
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
use self::shell_integration::{BufferPoint, ShellEvent, ShellIntegration};

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
    /// The extension host's id of a terminal it created (`extHostTerminalId`).
    pub ext_host_terminal_id: Option<String>,
    pub hide_from_user: bool,
    /// `customPtyImplementation`: the process is an extension's `Pseudoterminal`, no PTY is spawned.
    pub extension_pty: bool,
    /// `isFeatureTerminal` (tasks): shell integration is not injected and `executeCommand` is not offered.
    pub is_feature_terminal: bool,
    /// `shellIntegrationNonce`.
    pub nonce: String,
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
    /// When the instance was created; the resize overlay waits a second after it.
    pub started: Instant,
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
    /// OSC 99 notification sequences, the data after `99;`, for the workbench's handler.
    pub osc99_sequences: Vec<String>,
    pub osc99: osc99::Osc99,
    /// OSC 52 `?` queries, each formatting the clipboard text into its reply.
    pub clipboard_loads: Vec<std::sync::Arc<dyn Fn(&str) -> String + Sync + Send>>,
    /// The bell rang since it was last read.
    pub bell: bool,
    /// `terminal.integrated.autoReplies` as last read, and its responders.
    auto_reply_setting: serde_json::Value,
    auto_responders: Vec<auto_replies::AutoResponder>,
    launched: Instant,
    /// `ProcessState.KilledByUser`: the kill command ended the process.
    pub killed_by_user: bool,
    /// What `Term` was last configured with: `set_options` resets its history size and damage, so it runs on change only.
    config: Config,
    pub shell_integration: ShellIntegration,
    /// The bytes of a UTF-8 character the last chunk cut off.
    utf8_tail: Vec<u8>,
    /// Where the input of an extension pty goes (`$acceptProcessInput`); set for `launch.extension_pty`.
    pub process_input: Option<Box<dyn FnMut(&str)>>,
    /// `onDidInputData`: typed, pasted or sent text reached the process.
    pub on_input: Option<Box<dyn FnMut()>>,
    /// terminalProcess.ts `shellType`, from the foreground process name.
    pub shell_type: Option<&'static str>,
    /// commandDetectionCapability.ts `promptInputModel`, fed by the B, C, D and P sequences.
    pub prompt_input: prompt_input::PromptInputModel,
    /// commandDetectionCapability.ts `commands`: the finished commands of this session, oldest first.
    pub commands: Vec<FinishedCommand>,
    /// cwdDetectionCapability.ts `cwds`: each cwd once, the last one entered last.
    pub cwds: Vec<String>,
    /// The marks and output of the command being entered or run.
    current: FinishedCommand,
}

/// `ITerminalCommand`: lines are absolute buffer lines, as `BufferPoint` counts them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FinishedCommand {
    pub command: String,
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
    /// Milliseconds since the Unix epoch when it finished.
    pub timestamp: u64,
    pub output: String,
    /// The A (prompt start), B (command start), C (executed) and D (finished) lines.
    pub prompt_line: Option<i64>,
    pub command_line: Option<i64>,
    pub executed_line: Option<i64>,
    pub end_line: Option<i64>,
    /// The cursor column at C (`executedX`).
    pub executed_column: Option<usize>,
}

/// What one chunk of process output did: its text (`onData`) and the shell integration events in stream order.
pub struct Processed {
    pub text: String,
    pub shell_events: Vec<ShellEvent>,
    /// The prompt input changed (`onDidChangeInput`).
    pub input_changed: bool,
}

/// terminalProcessManager.ts `ErrorLaunchThresholdDuration`: an exit this soon after launch is a failed launch.
const ERROR_LAUNCH_THRESHOLD: Duration = Duration::from_millis(500);

impl TerminalInstance {
    pub fn new(id: u32, launch: ShellLaunchConfig, cols: u16, rows: u16, config: Config) -> (Self, Option<UnboundedReceiver<PtyEvent>>) {
        let events = Listener::default();
        let term = Term::new(config.clone(), &GridSize { cols, rows }, events.clone());
        let process_title = launch.executable.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let nonce = launch.nonce.clone();
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
            osc99_sequences: Vec::new(),
            osc99: osc99::Osc99::default(),
            clipboard_loads: Vec::new(),
            bell: false,
            auto_reply_setting: serde_json::Value::Null,
            auto_responders: Vec::new(),
            launched: Instant::now(),
            killed_by_user: false,
            config,
            shell_integration: ShellIntegration::new(nonce),
            utf8_tail: Vec::new(),
            process_input: None,
            started: Instant::now(),
            on_input: None,
            shell_type: None,
            prompt_input: prompt_input::PromptInputModel::default(),
            commands: Vec::new(),
            cwds: Vec::new(),
            current: FinishedCommand::default(),
        };
        if instance.launch.extension_pty {
            return (instance, None);
        }
        let (sender, receiver) = unbounded();
        let launch = &instance.launch;
        match Pty::spawn(&launch.executable, &launch.args, &launch.cwd, &launch.env, cols, rows, sender) {
            Ok(pty) => {
                instance.pid = Some(pty.pid);
                instance.pty = Some(pty);
                instance.shell_type = shell_type(&instance.process_title);
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

    /// Feeds the process's output to the parser and acts on what it raised; shell integration sequences are
    /// handled where they occur in the stream, so the command output between them is known.
    pub fn process(&mut self, bytes: &[u8], colors: &colors::TerminalColors) -> Processed {
        let text = self.decode(bytes);
        let mut shell_events = Vec::new();
        let mut fed = 0;
        let mut input_changed = false;
        for sequence in self.shell_integration.scan(&text) {
            if self.shell_integration.executing() && sequence.start > fed {
                self.push_shell_event(&mut shell_events, ShellEvent::ExecutionData(text[fed..sequence.start].to_owned()));
            }
            self.feed(&text[fed..sequence.end], colors);
            fed = sequence.end;
            if let Some(data) = sequence.payload.strip_prefix("99;") {
                self.osc99_sequences.push(data.to_owned());
            }
            let cursor = self.buffer_cursor();
            let command_start = self.shell_integration.command_start_point();
            input_changed |= self.prompt_sequence(&sequence.payload, cursor);
            for event in self.shell_integration.handle(&sequence.payload, cursor) {
                let event = match event {
                    ShellEvent::CommandExecuted(command) if command.command_line.is_empty() => match command_start {
                        Some((start, single_line_after_prompt)) => {
                            let line = self.buffer_text(start, cursor);
                            let confidence = if single_line_after_prompt && !line.contains('\n') { shell_integration::CONFIDENCE_MEDIUM } else { shell_integration::CONFIDENCE_LOW };
                            ShellEvent::CommandExecuted(self.shell_integration.set_buffer_command_line(line, confidence))
                        }
                        None => ShellEvent::CommandExecuted(command),
                    },
                    event => event,
                };
                self.push_shell_event(&mut shell_events, event);
            }
        }
        if self.shell_integration.executing() && fed < text.len() {
            self.push_shell_event(&mut shell_events, ShellEvent::ExecutionData(text[fed..].to_owned()));
        }
        self.feed(&text[fed..], colors);
        input_changed |= self.prompt_input.sync(&self.term);
        Processed { text, shell_events, input_changed }
    }

    /// Keeps `commands` and `cwds` as the command and cwd detection capabilities do, then queues the event.
    fn push_shell_event(&mut self, events: &mut Vec<ShellEvent>, event: ShellEvent) {
        match &event {
            ShellEvent::ExecutionData(data) => self.current.output.push_str(data),
            ShellEvent::CommandFinished(command) => {
                let mut finished = std::mem::take(&mut self.current);
                finished.command = command.command_line.clone();
                finished.cwd = command.cwd.clone();
                finished.exit_code = command.exit_code;
                finished.timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as u64);
                self.commands.push(finished);
            }
            ShellEvent::Cwd(cwd) => {
                self.cwds.retain(|seen| seen != cwd);
                self.cwds.push(cwd.clone());
            }
            ShellEvent::CommandExecuted(command) => self.current.command = command.command_line.clone(),
            ShellEvent::CommandDetectionAdded => {}
        }
        events.push(event);
    }

    /// The prompt input model's side of shellIntegrationAddon.ts: B starts the input, C and D end it, `P;Prompt=`
    /// and `P;ContinuationPrompt=` tell it the prompt. Returns whether the input changed.
    fn prompt_sequence(&mut self, payload: &str, cursor: BufferPoint) -> bool {
        let (ps, pt) = payload.split_once(';').unwrap_or((payload, ""));
        if ps != "633" && ps != "133" {
            return false;
        }
        let mut args = pt.split(';');
        match args.next() {
            Some("A") => {
                self.current = FinishedCommand { prompt_line: Some(cursor.line), ..FinishedCommand::default() };
                false
            }
            Some("B") => {
                self.current.command_line = Some(cursor.line);
                self.prompt_input.command_start(&self.term, cursor)
            }
            Some("C") => {
                self.current.executed_line = Some(cursor.line);
                self.current.executed_column = Some(cursor.column);
                self.prompt_input.command_executed();
                true
            }
            Some("D") => {
                self.current.end_line = Some(cursor.line);
                self.prompt_input.command_finished();
                true
            }
            Some("P") if ps == "633" => {
                let value = shell_integration::deserialize_message(args.next().unwrap_or_default());
                match value.split_once('=') {
                    // `_updatePromptTerminator`: the prompt's last line without SGR sequences.
                    Some(("Prompt", prompt)) => {
                        let prompt = strip_sgr(prompt);
                        let last_line = prompt.rsplit('\n').next().unwrap_or_default();
                        self.prompt_input.set_last_prompt_line(&self.term, last_line)
                    }
                    Some(("ContinuationPrompt", prompt)) => self.prompt_input.set_continuation_prompt(&self.term, &strip_prompt_escapes(prompt)),
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn feed(&mut self, text: &str, colors: &colors::TerminalColors) {
        self.parser.advance(&mut self.term, text.as_bytes());
        let events = std::mem::take(&mut *self.events.0.borrow_mut());
        for event in events {
            match event {
                Event::PtyWrite(text) => self.write_to_pty(text.as_bytes()),
                Event::Title(title) => self.sequence_title = Some(title),
                Event::ResetTitle => self.sequence_title = None,
                Event::Bell => self.bell = true,
                Event::ClipboardStore(_, text) => self.clipboard_requests.push(text),
                Event::ClipboardLoad(_, format) => self.clipboard_loads.push(format),
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

    /// The chunk as text; a character cut at its end waits for the next chunk (the PTY reads in 64 KiB pieces).
    fn decode(&mut self, bytes: &[u8]) -> String {
        let mut data = std::mem::take(&mut self.utf8_tail);
        data.extend_from_slice(bytes);
        let complete = match std::str::from_utf8(&data) {
            Ok(_) => data.len(),
            // An incomplete sequence at the end (`error_len` is `None`) is kept; invalid bytes become U+FFFD.
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => data.len(),
        };
        self.utf8_tail = data.split_off(complete);
        String::from_utf8_lossy(&data).into_owned()
    }

    fn buffer_cursor(&self) -> BufferPoint {
        let cursor = self.term.grid().cursor.point;
        BufferPoint { line: self.term.history_size() as i64 + cursor.line.0 as i64, column: cursor.column.0 }
    }

    /// `extractCommandLine`: the text from the command start to `end`, wrapped lines joined, trailing blanks gone.
    fn buffer_text(&self, start: BufferPoint, end: BufferPoint) -> String {
        let history = self.term.history_size() as i64;
        let to_point = |point: BufferPoint| Point::new(Line((point.line - history).max(-history) as i32), Column(point.column.min(self.term.columns() - 1)));
        let (start, end) = (to_point(start), to_point(end));
        if end <= start {
            return String::new();
        }
        let mut text = self.term.bounds_to_string(start, Point::new(end.line, Column(end.column.0.saturating_sub(1))));
        text.truncate(text.trim_end().len());
        text
    }

    /// Reads the foreground process and the cwd the shell is in, for `${process}` and `${cwd}`. Returns whether
    /// either changed.
    pub fn refresh_process(&mut self) -> bool {
        let Some(pty) = &self.pty else { return false };
        let (process, cwd) = (pty.foreground_process(), pty.cwd());
        let changed = process.as_ref().is_some_and(|process| *process != self.process_title) || cwd != self.cwd;
        if let Some(process) = process {
            self.shell_type = shell_type(&process);
            self.prompt_input.set_is_fish(self.shell_type == Some("fish"));
            self.process_title = process;
        }
        self.cwd = cwd;
        changed
    }

    pub fn exited(&mut self, code: Option<i32>) {
        self.exit_code = Some(code);
        self.pty = None;
    }

    /// `TerminalAutoResponder`: the process output answered by the replies of `setting`; a changed setting
    /// starts new responders.
    pub fn answer_auto_replies(&mut self, setting: &serde_json::Value, data: &str) {
        if self.auto_reply_setting != *setting {
            self.auto_responders = auto_replies::responders(setting);
            self.auto_reply_setting = setting.clone();
        }
        let now = Instant::now();
        let replies: Vec<String> = self.auto_responders.iter_mut().filter_map(|responder| responder.feed(data, now)).collect();
        for reply in replies {
            self.write_to_pty(reply.as_bytes());
        }
    }

    /// `sendText` and typed input: the terminal follows the input (xterm.js `scrollOnUserInput`).
    pub fn input(&mut self, data: &str) {
        self.term.scroll_display(Scroll::Bottom);
        self.term.selection = None;
        self.prompt_input.user_input(data);
        self.write_to_pty(data.as_bytes());
        if let Some(on_input) = &mut self.on_input {
            on_input();
        }
    }

    /// terminalInstance.ts `sendText`: line breaks become carriage returns and an executed line ends with one.
    pub fn send_text(&mut self, text: &str, should_execute: bool) {
        let mut text = text.replace("\r\n", "\r").replace('\n', "\r");
        if should_execute && !text.ends_with('\r') {
            text.push('\r');
        }
        self.input(&text);
    }

    /// xterm.js `paste`: line breaks become carriage returns, wrapped in bracketed paste mode markers when the
    /// process asked for them.
    pub fn paste(&mut self, text: &str) {
        let data = paste_data(text, self.term.mode().contains(TermMode::BRACKETED_PASTE));
        self.input(&data);
    }

    /// Answers the OSC 52 queries with the clipboard's text.
    pub fn answer_clipboard_loads(&mut self, text: &str) {
        for format in std::mem::take(&mut self.clipboard_loads) {
            self.write_to_pty(format(text).as_bytes());
        }
    }

    fn write_to_pty(&mut self, data: &[u8]) {
        if let Some(pty) = &mut self.pty {
            pty.write(data);
        } else if let Some(process_input) = &mut self.process_input {
            process_input(&String::from_utf8_lossy(data));
        }
    }

    /// Returns whether the size changed.
    pub fn resize(&mut self, cols: u16, rows: u16) -> bool {
        if self.term.columns() == cols as usize && self.term.screen_lines() == rows as usize {
            return false;
        }
        self.term.resize(GridSize { cols, rows });
        if let Some(pty) = &self.pty {
            pty.resize(cols, rows);
        }
        true
    }

    /// `sendSignal`: node-pty `kill(signal)` on the shell process.
    pub fn send_signal(&self, signal: i32) {
        if let Some(pty) = &self.pty {
            pty.kill(signal);
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
        // An extension pty has no executable: the message names no command line.
        if self.launch.extension_pty {
            return Some(match self.failed_during_launch() {
                true => format!("The terminal process failed to launch (exit code: {code})."),
                false => format!("The terminal process terminated with exit code: {code}."),
            });
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

    /// `serializeAsHTML({ onlySelection: true })`.
    pub fn selection_html(&self, colors: &colors::TerminalColors, font_family: &str, font_size: f32) -> Option<String> {
        let range = self.term.selection.as_ref()?.to_range(&self.term)?;
        let history = self.term.history_size() as i64;
        let start = (range.start.line.0 as i64 + history, range.start.column.0);
        let end = (range.end.line.0 as i64 + history, range.end.column.0 + 1);
        Some(self.serialize_html(start, end, &colors.palette, font_family, font_size))
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

/// terminalProcess.ts `posixShellTypeMap` and `generalShellTypeMap` (AI agent CLIs left out) over the process name.
pub fn shell_type(process: &str) -> Option<&'static str> {
    // `_updateTitle` drops a leading `-` of a login shell and the path.
    let name = process.trim_start_matches('-').rsplit('/').next().unwrap_or_default();
    let types = [
        ("bash", "bash"), ("csh", "csh"), ("fish", "fish"), ("ksh", "ksh"), ("sh", "sh"), ("zsh", "zsh"), ("pwsh", "pwsh"), ("powershell", "pwsh"),
        ("python", "python"), ("julia", "julia"), ("nu", "nu"), ("node", "node"), ("xonsh", "xonsh"),
    ];
    types.iter().find(|(process, _)| *process == name).map(|(_, kind)| *kind)
}

/// shellIntegrationAddon.ts `value.replace(/\x1b\[[0-9;]*m/g, '')`.
fn strip_sgr(text: &str) -> String {
    let sgr = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    sgr.replace_all(text, "").into_owned()
}

/// strings.ts `removeAnsiEscapeCodesFromPrompt`: escapes, then bash's `\[...\]` non-printing markers.
fn strip_prompt_escapes(text: &str) -> String {
    let non_printable = regex::Regex::new(r"\\\[.*?\\\]").unwrap();
    non_printable.replace_all(&crate::tasks::problem_collector::strip_ansi(text), "").into_owned()
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
pub(crate) fn sanitized_away(key: &str) -> bool {
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
