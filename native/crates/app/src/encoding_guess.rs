//! `files.autoGuessEncoding` (encoding.ts `detectEncodingFromBuffer` with `autoGuessEncoding`): a file without a BOM
//! or UTF-16 zero bytes is guessed by upstream's jschardet in a Bun worker (encoding_guess/worker.mjs), started on
//! the first guess and kept for the process.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};

use config::configuration::Overrides;
use config::service::Settings;
use files::encoding;
use serde_json::{json, Value};

/// `AUTO_ENCODING_GUESS_MAX_BYTES`: jschardet reads at most this many bytes.
const GUESS_MAX_BYTES: usize = 512 * 128;

static APP_ROOT: OnceLock<PathBuf> = OnceLock::new();
static WORKER: Mutex<Option<Worker>> = Mutex::new(None);

struct Worker {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

pub fn set_app_root(root: &Path) {
    let _ = APP_ROOT.set(root.to_path_buf());
}

/// The encoding `detectEncodingFromBuffer` reports for a resource: its BOM or UTF-16 zero bytes, else jschardet's
/// guess when `auto_guess` (the read option) or `files.autoGuessEncoding` is on for it and the bytes do not seem binary.
pub fn detect(bytes: &[u8], settings: &Settings, path: Option<&Path>, auto_guess: bool) -> Option<String> {
    let overrides = Overrides { resource: path, ..Overrides::default() };
    let auto_guess = auto_guess || settings.get_value(Some("files.autoGuessEncoding"), &overrides) == Some(Value::Bool(true));
    let candidates = settings.get_value(Some("files.candidateGuessEncodings"), &overrides);
    detect_from_buffer(bytes, auto_guess, candidates)
}

/// `detectEncodingFromBuffer`.
fn detect_from_buffer(bytes: &[u8], auto_guess: bool, candidates: Option<Value>) -> Option<String> {
    let detected = encoding::detect(bytes);
    if !auto_guess || detected.encoding.is_some() || detected.seems_binary {
        return detected.encoding.map(str::to_string);
    }
    guess(&bytes[..bytes.len().min(GUESS_MAX_BYTES)], candidates.filter(|value| value.as_array().is_some_and(|list| !list.is_empty())))
}

/// One request to the worker; a worker that cannot start or answer gives no guess and is started again next time.
fn guess(bytes: &[u8], candidates: Option<Value>) -> Option<String> {
    let mut worker = WORKER.lock().unwrap();
    if worker.is_none() {
        *worker = start_worker(APP_ROOT.get()?).ok();
    }
    let running = worker.as_mut()?;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let request = json!({ "bytes": hex, "candidates": candidates }).to_string() + "\n";
    let mut answer = String::new();
    if running.stdin.write_all(request.as_bytes()).is_err() || running.stdout.read_line(&mut answer).unwrap_or(0) == 0 {
        *worker = None;
        return None;
    }
    serde_json::from_str::<Value>(&answer).ok()?["encoding"].as_str().map(str::to_string)
}

fn start_worker(app_root: &Path) -> std::io::Result<Worker> {
    let mut child = Command::new(exthost::Runtime::Bun.executable())
        .args(["--eval", include_str!("encoding_guess/worker.mjs")])
        .env("VSCODE_NODE_MODULES", app_root.join("node_modules"))
        .env("DO_NOT_TRACK", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(Worker { stdin: child.stdin.take().expect("piped"), stdout: BufReader::new(child.stdout.take().expect("piped")) })
}

#[cfg(test)]
mod tests;
