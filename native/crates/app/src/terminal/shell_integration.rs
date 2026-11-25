//! shellIntegrationAddon.ts and commandDetectionCapability.ts: the OSC 633 (VS Code), 133 (FinalTerm), 1337 (iTerm)
//! and 7 sequences a shell integration script prints, turned into command and cwd events in stream order.

/// `TerminalShellExecutionCommandLineConfidence`.
pub const CONFIDENCE_LOW: u8 = 0;
pub const CONFIDENCE_MEDIUM: u8 = 1;
pub const CONFIDENCE_HIGH: u8 = 2;

/// xterm.js caps an OSC payload at 10 MB (`PAYLOAD_LIMIT` of OscParser.ts).
const PAYLOAD_LIMIT: usize = 10_000_000;

/// A command as the capability promotes it (`ITerminalCommand`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Command {
    pub command_line: String,
    pub confidence: u8,
    pub is_trusted: bool,
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ShellEvent {
    /// `onDidAddCommandDetectionCapability`: the first command detection sequence arrived.
    CommandDetectionAdded,
    /// `onCommandExecuted`; a command line still empty is read from the buffer by the instance.
    CommandExecuted(Command),
    /// `onCommandFinished`.
    CommandFinished(Command),
    /// `CwdDetection.onDidChangeCwd`.
    Cwd(String),
    /// Output of the executing command (`$shellExecutionData`), added by the instance between the sequences.
    ExecutionData(String),
}

/// A position in the buffer counted from the first line of the scrollback, so it survives scrolling.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BufferPoint {
    pub line: i64,
    pub column: usize,
}

/// One OSC sequence found in a chunk: where it starts and ends in the chunk and its payload (`Ps;Pt`).
#[derive(Debug, PartialEq)]
pub struct Sequence {
    pub start: usize,
    pub end: usize,
    pub payload: String,
}

#[derive(Default)]
enum ScanState {
    #[default]
    Ground,
    Escape,
    Osc(String),
    OscEscape(String),
}

#[derive(Default)]
pub struct ShellIntegration {
    /// `shellIntegrationNonce`: an E or P sequence carrying it comes from the injected script.
    pub nonce: String,
    state: ScanState,
    /// Where the open sequence began in the current chunk; 0 when it began in an earlier one.
    sequence_start: usize,
    pub has_command_detection: bool,
    /// The partial command (`PartialTerminalCommand`).
    prompt_start: Option<BufferPoint>,
    command_start: Option<BufferPoint>,
    command_line: Option<(String, bool)>,
    /// The line read from the buffer at execution when no E sequence set one, with its confidence.
    buffer_command_line: Option<(String, u8)>,
    executed: bool,
    cwd: Option<String>,
}

impl ShellIntegration {
    pub fn new(nonce: String) -> Self {
        ShellIntegration { nonce, ..Default::default() }
    }

    /// The OSC sequences that end in `text`, in order; a sequence cut at the chunk's end continues in the next.
    pub fn scan(&mut self, text: &str) -> Vec<Sequence> {
        let mut found = Vec::new();
        for (index, char) in text.char_indices() {
            self.state = match (std::mem::take(&mut self.state), char) {
                (ScanState::Ground, '\x1b') => ScanState::Escape,
                (ScanState::Ground, '\u{9d}') => {
                    self.sequence_start = index;
                    ScanState::Osc(String::new())
                }
                (ScanState::Ground, _) => ScanState::Ground,
                (ScanState::Escape, ']') => {
                    self.sequence_start = index.saturating_sub(1);
                    ScanState::Osc(String::new())
                }
                (ScanState::Escape, '\x1b') => ScanState::Escape,
                (ScanState::Escape, _) => ScanState::Ground,
                (ScanState::Osc(payload) | ScanState::OscEscape(payload), '\x07' | '\u{9c}') => {
                    found.push(Sequence { start: self.sequence_start, end: index + char.len_utf8(), payload });
                    ScanState::Ground
                }
                (ScanState::OscEscape(payload), '\\') => {
                    found.push(Sequence { start: self.sequence_start, end: index + 1, payload });
                    ScanState::Ground
                }
                // CAN and SUB abort the sequence; another ESC starts a new one.
                (ScanState::Osc(_) | ScanState::OscEscape(_), '\x18' | '\x1a') => ScanState::Ground,
                (ScanState::OscEscape(_), ']') => {
                    self.sequence_start = index.saturating_sub(1);
                    ScanState::Osc(String::new())
                }
                (ScanState::OscEscape(_), _) => ScanState::Ground,
                (ScanState::Osc(payload), '\x1b') => ScanState::OscEscape(payload),
                (ScanState::Osc(mut payload), char) => {
                    if payload.len() < PAYLOAD_LIMIT {
                        payload.push(char);
                    }
                    ScanState::Osc(payload)
                }
            };
        }
        self.sequence_start = 0;
        found
    }

    /// Handles one sequence's payload with the cursor where the parser left it.
    pub fn handle(&mut self, payload: &str, cursor: BufferPoint) -> Vec<ShellEvent> {
        let (ps, pt) = payload.split_once(';').unwrap_or((payload, ""));
        match ps {
            "633" => self.handle_vscode(pt, cursor),
            "133" => self.handle_final_term(pt, cursor),
            "1337" => match pt.split(';').next().unwrap_or_default().split_once('=') {
                // The iTerm2 protocol has no nonce and no escapes.
                Some(("CurrentDir", value)) => self.update_cwd(value),
                _ => Vec::new(),
            },
            "7" => {
                let uri = pt.split(';').next().unwrap_or_default();
                // `^file:\/\/.*\/`, then the URI's path.
                match uri.strip_prefix("file://").and_then(|rest| rest.find('/').map(|slash| &rest[slash..])) {
                    Some(path) if !path.is_empty() => self.update_cwd(&percent_decode(path)),
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    fn handle_vscode(&mut self, data: &str, cursor: BufferPoint) -> Vec<ShellEvent> {
        let (command, args) = match data.split_once(';') {
            Some((command, rest)) => (command, rest.split(';').collect::<Vec<_>>()),
            None => (data, Vec::new()),
        };
        let arg = |index: usize| args.get(index).copied();
        match command {
            "A" => self.prompt_start(cursor),
            "B" => self.command_start(cursor),
            "C" => self.command_executed(),
            "D" => self.command_finished(arg(0).and_then(|code| code.parse().ok())),
            "E" => {
                let events = self.ensure_command_detection();
                let line = arg(0).map(deserialize_message).unwrap_or_default();
                self.command_line = Some((line, arg(1) == Some(self.nonce.as_str())));
                events
            }
            "P" => {
                let value = deserialize_message(arg(0).unwrap_or_default());
                match value.split_once('=') {
                    Some(("Cwd", cwd)) => self.update_cwd(cwd),
                    Some(("IsWindows" | "HasRichCommandDetection" | "ContinuationPrompt" | "Prompt", _)) => self.ensure_command_detection(),
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    fn handle_final_term(&mut self, data: &str, cursor: BufferPoint) -> Vec<ShellEvent> {
        let mut parts = data.split(';');
        let command = parts.next().unwrap_or_default();
        let args: Vec<&str> = parts.collect();
        match command {
            "A" => self.prompt_start(cursor),
            "B" => self.command_start(cursor),
            "C" => self.command_executed(),
            "D" => self.command_finished(if args.len() == 1 { args[0].parse().ok() } else { None }),
            _ => Vec::new(),
        }
    }

    fn ensure_command_detection(&mut self) -> Vec<ShellEvent> {
        if self.has_command_detection {
            return Vec::new();
        }
        self.has_command_detection = true;
        vec![ShellEvent::CommandDetectionAdded]
    }

    fn prompt_start(&mut self, cursor: BufferPoint) -> Vec<ShellEvent> {
        let events = self.ensure_command_detection();
        self.prompt_start = Some(cursor);
        events
    }

    fn command_start(&mut self, cursor: BufferPoint) -> Vec<ShellEvent> {
        let events = self.ensure_command_detection();
        self.command_start = Some(cursor);
        self.executed = false;
        events
    }

    fn command_executed(&mut self) -> Vec<ShellEvent> {
        let mut events = self.ensure_command_detection();
        if self.executed {
            return events;
        }
        self.executed = true;
        events.push(ShellEvent::CommandExecuted(self.current_command(None)));
        events
    }

    /// The instance read the command line from the buffer (`extractCommandLine`); the finish reports it too.
    pub fn set_buffer_command_line(&mut self, line: String, confidence: u8) -> Command {
        self.buffer_command_line = Some((line, confidence));
        self.current_command(None)
    }

    pub fn executing(&self) -> bool {
        self.executed
    }

    /// `promptInputModel.state === PromptInputState.Input`: a command start was seen and nothing runs.
    pub fn prompt_ready(&self) -> bool {
        self.command_start.is_some() && !self.executed
    }

    /// `CwdDetection.getCwd`.
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    /// `handleCommandFinished`: an execution that was not announced is announced first; without a command start
    /// nothing finishes.
    fn command_finished(&mut self, exit_code: Option<i32>) -> Vec<ShellEvent> {
        let mut events = self.ensure_command_detection();
        if !self.executed {
            events.extend(self.command_executed());
        }
        if self.command_start.is_some() {
            events.push(ShellEvent::CommandFinished(self.current_command(exit_code)));
        }
        self.prompt_start = None;
        self.command_start = None;
        self.command_line = None;
        self.buffer_command_line = None;
        self.executed = false;
        events
    }

    fn current_command(&self, exit_code: Option<i32>) -> Command {
        let (command_line, confidence, is_trusted) = match &self.command_line {
            Some((line, trusted)) => (line.clone(), CONFIDENCE_HIGH, *trusted),
            None => match &self.buffer_command_line {
                Some((line, confidence)) => (line.clone(), *confidence, false),
                None => (String::new(), CONFIDENCE_LOW, false),
            },
        };
        Command { command_line, confidence, is_trusted, cwd: self.cwd.clone(), exit_code }
    }

    /// Where the typed command line starts and whether a buffer read can be `medium` confidence (a prompt
    /// start was seen and the command does not start at the left-most column).
    pub fn command_start_point(&self) -> Option<(BufferPoint, bool)> {
        if self.command_line.is_some() {
            return None;
        }
        let start = self.command_start?;
        Some((start, self.prompt_start.is_some() && start.column > 0))
    }

    fn update_cwd(&mut self, value: &str) -> Vec<ShellEvent> {
        // terminalEnvironment.ts `sanitizeCwd`: quotes around the path go.
        let quoted = value.len() >= 2 && (value.starts_with('\'') || value.starts_with('"')) && (value.ends_with('\'') || value.ends_with('"'));
        let cwd = if quoted { &value[1..value.len() - 1] } else { value };
        self.cwd = Some(cwd.to_owned());
        vec![ShellEvent::Cwd(cwd.to_owned())]
    }
}

/// `deserializeVSCodeOscMessage`: `\\` is a backslash and `\xAB` a character.
pub fn deserialize_message(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(index) = rest.find('\\') {
        out.push_str(&rest[..index]);
        let after = &rest[index + 1..];
        if let Some(stripped) = after.strip_prefix('\\') {
            out.push('\\');
            rest = stripped;
            continue;
        }
        let hex = after.strip_prefix(['x', 'X']).and_then(|hex| hex.get(..2)).filter(|hex| hex.chars().all(|char| char.is_ascii_hexdigit()));
        match hex {
            Some(hex) => {
                out.push(char::from(u8::from_str_radix(hex, 16).expect("two hex digits")));
                rest = &after[3..];
            }
            None => {
                out.push('\\');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn percent_decode(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = (bytes[index] == b'%').then(|| path.get(index + 1..index + 3)).flatten().and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match hex {
            Some(byte) => {
                out.push(byte);
                index += 3;
            }
            None => {
                out.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: BufferPoint = BufferPoint { line: 0, column: 2 };

    fn run(integration: &mut ShellIntegration, text: &str) -> Vec<ShellEvent> {
        let mut events = Vec::new();
        for sequence in integration.scan(text) {
            events.extend(integration.handle(&sequence.payload, AT));
        }
        events
    }

    #[test]
    fn sequences_are_found_across_chunks_with_both_terminators() {
        let mut integration = ShellIntegration::new("n".into());
        assert!(integration.scan("out\x1b]633;E;ec").is_empty());
        let found = integration.scan("ho\x07rest\x1b]633;C\x1b\\");
        assert_eq!(found, vec![Sequence { start: 0, end: 3, payload: "633;E;echo".into() }, Sequence { start: 7, end: 16, payload: "633;C".into() }]);
    }

    #[test]
    fn a_trusted_command_line_runs_and_finishes_with_its_exit_code() {
        let mut integration = ShellIntegration::new("nonce".into());
        let events = run(&mut integration, "\x1b]633;A\x07$ \x1b]633;B\x07echo hello\r\n\x1b]633;E;echo hello;nonce\x07\x1b]633;C\x07hello\r\n\x1b]633;D;0\x07");
        let command = Command { command_line: "echo hello".into(), confidence: CONFIDENCE_HIGH, is_trusted: true, cwd: None, exit_code: None };
        assert_eq!(events, vec![ShellEvent::CommandDetectionAdded, ShellEvent::CommandExecuted(command.clone()), ShellEvent::CommandFinished(Command { exit_code: Some(0), ..command })]);
    }

    #[test]
    fn a_wrong_nonce_is_untrusted_and_a_finish_without_execution_announces_it() {
        let mut integration = ShellIntegration::new("nonce".into());
        let events = run(&mut integration, "\x1b]633;B\x07\x1b]633;E;ls;other\x07\x1b]633;D;1\x07");
        let command = Command { command_line: "ls".into(), confidence: CONFIDENCE_HIGH, is_trusted: false, cwd: None, exit_code: None };
        assert_eq!(events[1..], [ShellEvent::CommandExecuted(command.clone()), ShellEvent::CommandFinished(Command { exit_code: Some(1), ..command })]);
    }

    #[test]
    fn cwd_comes_from_633_p_1337_and_osc_7() {
        let mut integration = ShellIntegration::new(String::new());
        assert_eq!(run(&mut integration, "\x1b]633;P;Cwd=/a\\x3bb\x07"), vec![ShellEvent::Cwd("/a;b".into())]);
        assert_eq!(run(&mut integration, "\x1b]1337;CurrentDir='/c'\x07"), vec![ShellEvent::Cwd("/c".into())]);
        assert_eq!(run(&mut integration, "\x1b]7;file://host/d%20e\x07"), vec![ShellEvent::Cwd("/d e".into())]);
    }

    #[test]
    fn messages_unescape_backslashes_and_hex() {
        assert_eq!(deserialize_message(r"a\\b\x3bc\x0Ad\q"), "a\\b;c\nd\\q");
    }
}
