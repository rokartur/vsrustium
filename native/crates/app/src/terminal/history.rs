//! Terminal command and directory history: history.ts `TerminalPersistedHistory` and the shell history file
//! readers behind `getShellFileHistory`.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// `Constants.DefaultHistoryLimit`, used when `terminal.integrated.shellIntegration.history` is not a number.
pub const DEFAULT_HISTORY_LIMIT: usize = 100;
/// `StorageKeys.Entries` for `getCommandHistory`.
pub const ENTRIES_KEY_COMMANDS: &str = "terminal.history.entries.commands";
/// `StorageKeys.Entries` for `getDirectoryHistory`.
pub const ENTRIES_KEY_DIRS: &str = "terminal.history.entries.dirs";
/// `StorageKeys.Timestamp` for `getCommandHistory`.
pub const TIMESTAMP_KEY_COMMANDS: &str = "terminal.history.timestamp.commands";
/// `StorageKeys.Timestamp` for `getDirectoryHistory`.
pub const TIMESTAMP_KEY_DIRS: &str = "terminal.history.timestamp.dirs";

/// `TerminalPersistedHistory`: an LRU of entries, most recent last. Every change sets `timestamp`; the caller
/// then stores `to_json()` under the entries key and `timestamp()` under the timestamp key (`_saveState`).
pub struct TerminalPersistedHistory<T> {
    entries: Vec<(String, T)>,
    limit: usize,
    timestamp: u64,
}

#[derive(Serialize)]
struct SavedEntry<'a, T> {
    key: &'a str,
    value: &'a T,
}

#[derive(Serialize)]
struct SavedCache<'a, T> {
    entries: Vec<SavedEntry<'a, T>>,
}

#[derive(Deserialize)]
struct LoadedEntry<T> {
    key: String,
    value: T,
}

#[derive(Deserialize)]
struct LoadedCache<T> {
    entries: Vec<LoadedEntry<T>>,
}

impl<T: Serialize + DeserializeOwned> TerminalPersistedHistory<T> {
    /// `_loadState`: `json` is the stored entries (missing, empty or invalid loads none) and `timestamp` the stored
    /// timestamp (0 when missing).
    pub fn load(json: Option<&str>, timestamp: u64, limit: usize) -> Self {
        let mut history = TerminalPersistedHistory { entries: Vec::new(), limit, timestamp };
        let Some(raw) = json.filter(|raw| !raw.is_empty()) else { return history };
        let Ok(Some(serialized)) = serde_json::from_str::<Option<LoadedCache<T>>>(raw) else { return history };
        for entry in serialized.entries {
            history.set(entry.key, entry.value);
        }
        history
    }

    /// `_saveState`'s entries value, `{"entries":[{"key":..,"value":..}]}`.
    pub fn to_json(&self) -> String {
        let entries = self.entries.iter().map(|(key, value)| SavedEntry { key, value }).collect();
        serde_json::to_string(&SavedCache { entries }).expect("history values serialize to JSON")
    }
}

impl<T> TerminalPersistedHistory<T> {
    /// `entries`, oldest first.
    pub fn entries(&self) -> &[(String, T)] {
        &self.entries
    }

    /// `add`: inserts or moves `key` to the most recent end.
    pub fn add(&mut self, key: &str, value: T) {
        self.set(key.to_owned(), value);
        self.touch();
    }

    /// `remove`.
    pub fn remove(&mut self, key: &str) {
        self.entries.retain(|(entry, _)| entry != key);
        self.touch();
    }

    /// `clear`.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.touch();
    }

    /// The `onDidChangeConfiguration` listener: a new `terminal.integrated.shellIntegration.history` drops the
    /// oldest entries over it.
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
        self.trim();
    }

    /// The timestamp of the last save, or of the loaded state.
    pub fn timestamp(&self) -> u64 {
        self.timestamp
    }

    /// The `onDidChangeValue` listener: another window saved when the stored timestamp differs; the caller then
    /// reloads with `load`.
    pub fn is_stale(&self, stored_timestamp: u64) -> bool {
        stored_timestamp != self.timestamp
    }

    fn set(&mut self, key: String, value: T) {
        self.entries.retain(|(entry, _)| *entry != key);
        self.entries.push((key, value));
        self.trim();
    }

    fn trim(&mut self) {
        let excess = self.entries.len().saturating_sub(self.limit);
        self.entries.drain(..excess);
    }

    fn touch(&mut self) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock after 1970");
        self.timestamp = now.as_millis().try_into().expect("milliseconds fit u64");
    }
}

/// `IShellFileHistoryEntry`.
#[derive(Debug, PartialEq)]
pub struct ShellFileHistory {
    pub source_label: &'static str,
    pub source_resource: PathBuf,
    pub commands: Vec<String>,
}

/// `getShellFileHistory` without its per-window cache, which the caller keeps; `shell_type` as `terminal::shell_type`
/// names it. `home` is `$HOME`, `env` reads other variables.
pub fn fetch_shell_file_history(shell_type: &str, home: &Path, env: &dyn Fn(&str) -> Option<String>) -> io::Result<Option<ShellFileHistory>> {
    match shell_type {
        "bash" => fetch_bash_history(home),
        "pwsh" => fetch_pwsh_history(home),
        "zsh" => fetch_zsh_history(home),
        "fish" => fetch_fish_history(home, env),
        "python" => fetch_python_history(home),
        _ => Ok(None),
    }
}

/// `fetchBashHistory`. Not ported: the Windows (local or remote) branch, which returns undefined.
pub fn fetch_bash_history(home: &Path) -> io::Result<Option<ShellFileHistory>> {
    fetch_file(home, ".bash_history", "~/.bash_history", parse_bash_history)
}

/// `fetchZshHistory`. Not ported: the Windows (local or remote) branch, which returns undefined.
pub fn fetch_zsh_history(home: &Path) -> io::Result<Option<ShellFileHistory>> {
    fetch_file(home, ".zsh_history", "~/.zsh_history", parse_zsh_history)
}

/// `fetchPythonHistory`.
pub fn fetch_python_history(home: &Path) -> io::Result<Option<ShellFileHistory>> {
    fetch_file(home, ".python_history", "~/.python_history", parse_python_history)
}

/// `fetchPwshHistory`. Not ported: the Windows branch reading `$APPDATA\Microsoft\Windows\PowerShell\PSReadLine`.
pub fn fetch_pwsh_history(home: &Path) -> io::Result<Option<ShellFileHistory>> {
    let file = ".local/share/powershell/PSReadline/ConsoleHost_history.txt";
    fetch_file(home, file, "~/.local/share/powershell/PSReadline/ConsoleHost_history.txt", parse_pwsh_history)
}

/// `fetchFishHistory`: `$XDG_DATA_HOME/fish/fish_history` when the variable is set, else
/// `~/.local/share/fish/fish_history`. Not ported: the Windows (local or remote) branch, which returns undefined.
pub fn fetch_fish_history(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> io::Result<Option<ShellFileHistory>> {
    match env("XDG_DATA_HOME").filter(|data_home| !data_home.is_empty()) {
        Some(data_home) => fetch_file(Path::new(&data_home), "fish/fish_history", "$XDG_DATA_HOME/fish/fish_history", parse_fish_history),
        None => fetch_file(home, ".local/share/fish/fish_history", "~/.local/share/fish/fish_history", parse_fish_history),
    }
}

/// `fetchFileContents`: a missing file is `None`, other read errors are returned. Not ported: the
/// `vscode-remote` resource of a remote connection.
fn fetch_file(folder: &Path, file: &str, source_label: &'static str, parse: fn(&str) -> Vec<String>) -> io::Result<Option<ShellFileHistory>> {
    if folder.as_os_str().is_empty() {
        return Ok(None);
    }
    let source_resource = folder.join(file);
    let content = match std::fs::read(&source_resource) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(Some(ShellFileHistory { source_label, source_resource, commands: parse(&content) }))
}

/// `fetchBashHistory`'s parser: a line ending inside `'` or `"` quotes continues on the next line.
pub fn parse_bash_history(content: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut pending: Option<String> = None;
    let mut wrap_char = None;
    for line in content.split('\n') {
        let command = match pending.take() {
            Some(previous) => format!("{previous}\n{line}"),
            None => line.to_owned(),
        };
        for c in line.chars() {
            match wrap_char {
                Some(wrap) if c == wrap => wrap_char = None,
                Some(_) => {}
                None if c == '\'' || c == '"' => wrap_char = Some(c),
                None => {}
            }
        }
        if wrap_char.is_some() {
            pending = Some(command);
        } else if !command.is_empty() {
            commands.push(command.trim().to_owned());
        }
    }
    unique(commands)
}

/// `fetchZshHistory`'s parser: `: <start>:<elapsed>;` starts each command in extended history, otherwise a
/// newline not escaped with `\` does.
pub fn parse_zsh_history(content: &str) -> Vec<String> {
    let extended = regex::Regex::new(r"^:\s[0-9]+:[0-9]+;").unwrap().is_match(content);
    let lines: Vec<&str> = if extended { regex::Regex::new(r":\s[0-9]+:[0-9]+;").unwrap().split(content).collect() } else { split_unescaped_newlines(content) };
    let commands = lines.iter().map(|line| line.replace("\\\n", "\n").trim().to_owned()).filter(|command| !command.is_empty());
    unique(commands)
}

/// `content.split(/(?<!\\)\n/)`.
fn split_unescaped_newlines(content: &str) -> Vec<&str> {
    let bytes = content.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' && (index == 0 || bytes[index - 1] != b'\\') {
            parts.push(&content[start..index]);
            start = index + 1;
        }
    }
    parts.push(&content[start..]);
    parts
}

/// `fetchPythonHistory`'s parser: one command per line.
pub fn parse_python_history(content: &str) -> Vec<String> {
    unique(content.split('\n').map(str::trim).filter(|line| !line.is_empty()).map(str::to_owned))
}

/// `fetchPwshHistory`'s parser: a line ending in an odd number of backticks continues on the next line, without
/// that backtick.
pub fn parse_pwsh_history(content: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut pending: Option<String> = None;
    for line in content.split('\n') {
        let mut command = match pending.take() {
            Some(previous) => format!("{previous}\n{line}"),
            None => line.to_owned(),
        };
        let wrapped = line.ends_with('`') && line.chars().filter(|c| *c == '`').count() % 2 == 1;
        if wrapped {
            command.pop();
            pending = Some(command);
            continue;
        }
        let sanitized = command.trim();
        if !sanitized.is_empty() {
            commands.push(sanitized.to_owned());
        }
    }
    unique(commands)
}

/// `fetchFishHistory`'s parser: the `- cmd:` records of fish's pseudo-YAML, `\n` escapes made line breaks.
pub fn parse_fish_history(content: &str) -> Vec<String> {
    let commands = content
        .split('\n')
        .filter_map(|line| line.strip_prefix("- cmd:"))
        .map(|cmd| sanitize_fish_history_cmd(cmd.trim_start()).trim().to_owned())
        .filter(|command| !command.is_empty());
    unique(commands)
}

/// `sanitizeFishHistoryCmd`: `\n` after an even number of backslashes becomes a line break.
pub fn sanitize_fish_history_cmd(cmd: &str) -> String {
    // One pass reaches the fixed point of upstream's repeated `/(^|[^\\])((?:\\\\)*)(\\n)/g` replace.
    let mut result = String::with_capacity(cmd.len());
    let mut backslashes = 0;
    for c in cmd.chars() {
        if c == 'n' && backslashes % 2 == 1 {
            result.pop();
            result.push('\n');
            backslashes = 0;
            continue;
        }
        backslashes = if c == '\\' { backslashes + 1 } else { 0 };
        result.push(c);
    }
    result
}

/// The `Set` each parser collects into: first occurrence order, duplicates dropped.
fn unique(commands: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    commands.into_iter().filter(|command| seen.insert(command.clone())).collect()
}

#[cfg(test)]
mod tests;
