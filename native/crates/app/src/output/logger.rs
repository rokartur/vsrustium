//! What platform/log writes and how: spdlogLog.ts's rotating file logger with its line pattern, log.ts `format`,
//! `createUnexpectedErrorHandler` and errorMessage.ts `toErrorMessage`, and remoteConsoleUtil.ts, which turns an
//! extension host console entry into a log line.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

/// spdlogLog.ts `_createSpdLogLogger`: 30 MB over the files a logger keeps, 6 when it rotates.
const LOG_BYTES: u64 = 30 * 1024 * 1024;
const ROTATING_FILES: u64 = 6;

/// errorMessage.ts `error.defaultMessage`.
const DEFAULT_ERROR_MESSAGE: &str = "An unknown error occurred. Please consult the log for more details.";

/// spdlog's `%l` for log.ts `LogLevel` 1 (trace) to 5 (error).
fn level_name(level: u8) -> &'static str {
    ["", "trace", "debug", "info", "warning", "error"][level as usize]
}

/// A log file as `SpdLogLogger` writes it: `rotating` unless `donotRotate`, `formatted` (spdlog's
/// `%Y-%m-%d %H:%M:%S.%e [%l] %v` pattern) unless `donotUseFormatters`.
#[derive(Clone, Debug, PartialEq)]
pub struct LogFile {
    pub path: PathBuf,
    pub rotating: bool,
    pub formatted: bool,
}

impl LogFile {
    /// One line appended. A rotating file stays under 5 MB: the full file moves to `<name>.1.<ext>`, the older ones
    /// one number up, the sixth dropped (spdlog's `rotating_file_sink`); one that does not rotate keeps 30 MB.
    pub fn append(&self, level: u8, message: &str) {
        let line = match self.formatted {
            true => format!("{} [{}] {message}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"), level_name(level)),
            false => format!("{message}\n"),
        };
        let file = &self.path;
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(file.parent().expect("a log file is in a folder"))?;
            let files = if self.rotating { ROTATING_FILES } else { 1 };
            let size = std::fs::metadata(file).map_or(0, |metadata| metadata.len());
            if size > 0 && size + line.len() as u64 > LOG_BYTES / files {
                rotate(file, files)?;
            }
            std::fs::OpenOptions::new().create(true).append(true).open(file)?.write_all(line.as_bytes())
        };
        if let Err(error) = write() {
            eprintln!("cannot write the log {}: {error}", file.display());
        }
    }
}

/// `rotating_file_sink::rotate_`.
fn rotate(file: &Path, files: u64) -> std::io::Result<()> {
    for index in (1..=files).rev() {
        let source = rotated_name(file, index - 1);
        if source.exists() {
            std::fs::rename(&source, rotated_name(file, index))?;
        }
    }
    Ok(())
}

/// spdlog's `calc_filename`: `renderer.log` as `renderer.<index>.log`, the file itself at 0.
pub fn rotated_name(file: &Path, index: u64) -> PathBuf {
    if index == 0 {
        return file.to_path_buf();
    }
    let stem = file.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let name = match file.extension() {
        Some(extension) => format!("{stem}.{index}.{}", extension.to_string_lossy()),
        None => format!("{stem}.{index}"),
    };
    file.with_file_name(name)
}

/// log.ts `format`: the arguments joined by spaces, objects as JSON. A serialized error is its stack with
/// `verbose`, else its message, as an `Error` argument is.
pub fn format(args: &[Value], verbose: bool) -> String {
    let parts: Vec<String> = args
        .iter()
        .map(|arg| match arg {
            Value::String(text) => text.clone(),
            arg if arg["$isError"] == true => to_error_message(arg, verbose),
            arg => arg.to_string(),
        })
        .collect();
    parts.join(" ")
}

/// errorMessage.ts `toErrorMessage`.
pub fn to_error_message(error: &Value, verbose: bool) -> String {
    match error {
        Value::Null | Value::Bool(false) => DEFAULT_ERROR_MESSAGE.into(),
        Value::String(text) if text.is_empty() => DEFAULT_ERROR_MESSAGE.into(),
        Value::String(text) => text.clone(),
        Value::Array(errors) => {
            let errors: Vec<&Value> = errors.iter().filter(|error| is_truthy(error)).collect();
            let message = to_error_message(errors.first().copied().unwrap_or(&Value::Null), verbose);
            match errors.len() > 1 {
                true => format!("{message} ({} errors in total)", errors.len()),
                false => message,
            }
        }
        error => {
            let detail = &error["detail"];
            if is_truthy(detail) {
                if is_truthy(&detail["error"]) {
                    return exception_message(&detail["error"], verbose);
                }
                if is_truthy(&detail["exception"]) {
                    return exception_message(&detail["exception"], verbose);
                }
            }
            if is_truthy(&error["stack"]) {
                return exception_message(error, verbose);
            }
            match &error["message"] {
                message if is_truthy(message) => text_of(message),
                _ => DEFAULT_ERROR_MESSAGE.into(),
            }
        }
    }
}

/// `exceptionToErrorMessage`: with `verbose` and a stack, `<message>: <stack>`.
fn exception_message(exception: &Value, verbose: bool) -> String {
    let stack = match &exception["stack"] {
        Value::Array(lines) => lines.iter().map(text_of).collect::<Vec<_>>().join("\n"),
        stack if is_truthy(stack) => text_of(stack),
        _ => String::new(),
    };
    match verbose && !stack.is_empty() {
        true => format!("{}: {stack}", system_error_message(exception)),
        false => system_error_message(exception),
    }
}

/// `detectSystemErrorMessage`.
fn system_error_message(exception: &Value) -> String {
    let message = text_of(&exception["message"]);
    if exception["code"] == "ERR_UNC_HOST_NOT_ALLOWED" {
        return format!("{message}. Please update the 'security.allowedUNCHosts' setting if you want to allow this host.");
    }
    if exception["code"].is_string() && exception["errno"].is_number() && exception["syscall"].is_string() {
        return format!("A system error occurred ({message})");
    }
    match is_truthy(&exception["message"]) {
        true => message,
        false => DEFAULT_ERROR_MESSAGE.into(),
    }
}

fn text_of(value: &Value) -> String {
    value.as_str().map_or_else(|| value.to_string(), str::to_owned)
}

/// JavaScript truthiness of a JSON value.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// log.ts `createUnexpectedErrorHandler`: the line an unexpected error logs, the same message within 1000 ms of
/// the last one logged only once.
#[derive(Default)]
pub struct UnexpectedErrorHandler {
    previous: Option<(String, Instant)>,
}

impl UnexpectedErrorHandler {
    /// The text `logger.error` writes for `error`, or `None` for an empty message or a duplicate. An error with a
    /// stack logs its stack (`AbstractMessageLogger.error`), anything else its verbose message.
    pub fn handle(&mut self, error: &Value, now: Instant) -> Option<String> {
        let message = to_error_message(error, true);
        if message.is_empty() {
            return None;
        }
        if self.previous.as_ref().is_some_and(|(previous, at)| *previous == message && now.duration_since(*at) <= Duration::from_millis(1000)) {
            return None;
        }
        self.previous = Some((message.clone(), now));
        let stack = error["stack"].as_str().filter(|stack| error["$isError"] == true && !stack.is_empty());
        Some(stack.map_or(message, str::to_owned))
    }
}

/// remoteConsoleUtil.ts `logRemoteEntry` (every entry, in an extension test run from the command line) or
/// `logRemoteEntryIfError` (errors only): the level and text the log service gets, or `None`.
pub fn console_log_line(entry: &Value, every_entry: bool) -> Option<(u8, String)> {
    let mut args = match entry["arguments"].as_str().map(serde_json::from_str::<Value>) {
        Some(Ok(Value::Array(args))) => args,
        _ => vec![Value::from("Unable to log remote console arguments"), entry["arguments"].clone()],
    };
    if args.last().is_some_and(|last| is_truthy(&last["__$stack"])) {
        args.pop();
    }
    let Some(Value::String(first)) = args.first() else { return None };
    // logRemoteEntry labels nothing; logRemoteEntryIfError labels the errors it logs.
    let first = if every_entry { first.clone() } else { format!("[Extension Host] {first}") };
    let text = format(&[&[Value::from(first)], &args[1..]].concat(), false);
    let level = match entry["severity"].as_str() {
        Some("error") => 5,
        Some("warn") if every_entry => 4,
        Some("log" | "info") | None if every_entry => 3,
        _ => return None,
    };
    Some((level, text))
}

#[cfg(test)]
mod tests;
