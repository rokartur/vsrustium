//! The output channels of the window: output.ts `IOutputChannelRegistry` with outputServices.ts `OutputService`,
//! and the logger registry of log.ts `AbstractLoggerService` that logs.contribution.ts `LogOutputChannels` turns
//! into log channels. A channel's text is its file from the channel's start offset (outputChannelModel.ts
//! `FileContentProvider`): the extension host writes the files, the workbench reads them.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

pub mod filter;
pub mod links;
pub mod log_levels;
pub mod logger;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod upstream_tests;

/// `Schemas.outputChannel`.
pub const OUTPUT_SCHEME: &str = "output";
/// output.ts `OUTPUT_MODE_ID` and `LOG_MODE_ID`, the languages of plain and log channels.
pub const OUTPUT_LANGUAGE: &str = "Log";
pub const LOG_LANGUAGE: &str = "log";
/// logConstants.ts `windowLogId`.
pub const WINDOW_LOG_ID: &str = "rendererLog";
/// log.ts `DEFAULT_LOG_LEVEL`, `LogLevel.Info`.
pub const DEFAULT_LOG_LEVEL: u8 = 3;
/// log.ts `LogLevel` from `Off` (0) to `Error` (5), with `LogLevelToString` and `LogLevelToLocalizedString`.
pub const LOG_LEVELS: [(u8, &str, &str); 6] = [(1, "trace", "Trace"), (2, "debug", "Debug"), (3, "info", "Info"), (4, "warn", "Warning"), (5, "error", "Error"), (0, "off", "Off")];

/// log.ts `LogLevelToString`.
pub fn log_level_name(level: u8) -> &'static str {
    LOG_LEVELS.iter().find(|(known, _, _)| *known == level).expect("a log level").1
}

pub struct OutputChannel {
    pub id: String,
    pub label: String,
    pub file: PathBuf,
    pub log: bool,
    pub language_id: Option<String>,
    pub extension_id: Option<String>,
    start: u64,
    end: u64,
}

/// What reading a channel's file added to its text.
#[derive(Debug, PartialEq)]
pub enum Content {
    Append(String),
    /// The file shrank or the channel was reset: the text is this from now on.
    Reset(String),
}

/// `ILoggerResource`, as `$registerLogger` sends it.
#[derive(Clone, Debug, PartialEq)]
pub struct Logger {
    pub id: String,
    pub name: Option<String>,
    pub file: PathBuf,
    pub hidden: bool,
    pub extension_id: Option<String>,
    /// The level the user set for this logger; `None` follows the default level.
    pub log_level: Option<u8>,
}

pub struct Output {
    /// In registration order, as `getChannels` lists them.
    pub channels: Vec<OutputChannel>,
    pub active: Option<String>,
    pub loggers: Vec<Logger>,
    /// `MainThreadOutputService._extensionIdPool`.
    extension_id_pool: HashMap<String, u32>,
    /// outputServices.ts `OutputViewFilters`, shared by every channel the view shows.
    pub filters: filter::Filters,
    /// log.ts `AbstractLoggerService.logLevel`: the level of every logger without its own.
    pub log_level: u8,
    /// The default log levels of the environment: argv.json `log-level` as it was at startup.
    pub startup_log_levels: log_levels::DefaultLogLevels,
    /// environmentService.argvResource.
    pub argv_file: PathBuf,
    /// The loggers the workbench writes, created by `$createLogger`, and whether they log at every level.
    written_loggers: Vec<(logger::LogFile, bool)>,
}

impl Default for Output {
    fn default() -> Self {
        Output {
            channels: Vec::new(),
            active: None,
            loggers: Vec::new(),
            extension_id_pool: HashMap::new(),
            filters: filter::Filters::default(),
            log_level: DEFAULT_LOG_LEVEL,
            startup_log_levels: log_levels::DefaultLogLevels::default(),
            argv_file: PathBuf::new(),
            written_loggers: Vec::new(),
        }
    }
}

impl OutputChannel {
    pub fn new(id: String, label: String, file: PathBuf, log: bool) -> Self {
        OutputChannel { id, label, file, log, language_id: None, extension_id: None, start: 0, end: 0 }
    }

    /// The language of the channel's model (`createOutputChannelModel`).
    pub fn language(&self) -> &str {
        self.language_id.as_deref().unwrap_or(if self.log { LOG_LANGUAGE } else { OUTPUT_LANGUAGE })
    }

    /// `FileContentProvider.reset(till)`, or `resetToEnd` without `till`: the text starts there.
    pub fn reset(&mut self, till: Option<u64>) {
        self.start = till.unwrap_or(self.end);
        self.end = self.start;
    }

    /// The whole text, read again from the start offset (`loadModel`, `replaceContent`).
    pub fn read_all(&mut self) -> String {
        self.end = self.start;
        match self.read_appended() {
            Content::Append(text) | Content::Reset(text) => text,
        }
    }

    /// `FileContentProvider.doWatch` and `getContent`: what the file gained since the last read; a file shorter than
    /// what was read restarts the channel at 0. A file not written yet reads as empty.
    pub fn read_appended(&mut self) -> Content {
        let Ok(mut file) = std::fs::File::open(&self.file) else { return Content::Append(String::new()) };
        let size = file.metadata().map_or(0, |metadata| metadata.len());
        let reset = self.end > size;
        if reset {
            self.reset(Some(0));
        }
        let mut bytes = Vec::new();
        if file.seek(SeekFrom::Start(self.end)).is_ok() {
            let _ = file.take(size - self.end).read_to_end(&mut bytes);
        }
        self.end += bytes.len() as u64;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if reset { Content::Reset(text) } else { Content::Append(text) }
    }
}

impl Output {
    pub fn channel(&self, id: &str) -> Option<&OutputChannel> {
        self.channels.iter().find(|channel| channel.id == id)
    }

    pub fn channel_mut(&mut self, id: &str) -> Option<&mut OutputChannel> {
        self.channels.iter_mut().find(|channel| channel.id == id)
    }

    pub fn active_channel(&self) -> Option<&OutputChannel> {
        self.channel(self.active.as_deref()?)
    }

    /// `registerChannel` with `onDidRegisterChannel`: a known id is kept; the first channel becomes the active one.
    pub fn register_channel(&mut self, channel: OutputChannel) {
        if self.channel(&channel.id).is_some() {
            return;
        }
        if self.active.is_none() {
            self.active = Some(channel.id.clone());
        }
        self.channels.push(channel);
    }

    /// `removeChannel`: the active channel moves to the first one left, `None` when none is.
    pub fn remove_channel(&mut self, id: &str) -> Option<OutputChannel> {
        let index = self.channels.iter().position(|channel| channel.id == id)?;
        let removed = self.channels.remove(index);
        if self.active.as_deref() == Some(id) {
            self.active = self.channels.first().map(|channel| channel.id.clone());
        }
        Some(removed)
    }

    /// `MainThreadOutputService.$register`: a plain channel of an extension, its id numbered per extension.
    pub fn register_extension_channel(&mut self, label: &str, file: PathBuf, language_id: Option<String>, extension_id: &str) -> String {
        let counter = self.extension_id_pool.entry(extension_id.to_owned()).or_default();
        *counter += 1;
        let id = format!("extension-output-{extension_id}-#{counter}-{label}");
        let channel = OutputChannel { language_id, extension_id: Some(extension_id.to_owned()), ..OutputChannel::new(id.clone(), label.to_owned(), file, false) };
        self.register_channel(channel);
        id
    }

    /// The logger a log channel shows; plain channels have none.
    pub fn channel_logger(&self, id: &str) -> Option<&Logger> {
        let channel = self.channel(id).filter(|channel| channel.log)?;
        self.loggers.iter().find(|logger| logger.file == channel.file)
    }

    /// `OutputService.getLogLevel`: the level of the channel's logger, else the global level.
    pub fn channel_log_level(&self, id: &str) -> Option<u8> {
        self.channel_logger(id).map(|logger| logger.log_level.unwrap_or(self.log_level))
    }

    /// `OutputService.setLogLevel` and `LoggerService.setLogLevel(resource, level)`: the logger's file when its level
    /// changed; the global level is stored as none.
    pub fn set_channel_log_level(&mut self, id: &str, level: u8) -> Option<PathBuf> {
        let file = self.channel_logger(id)?.file.clone();
        let global = self.log_level;
        let logger = self.loggers.iter_mut().find(|logger| logger.file == file).expect("the channel's logger");
        if logger.log_level == Some(level) {
            return None;
        }
        logger.log_level = (level != global).then_some(level);
        Some(file)
    }

    /// defaultLogLevels.ts `defaultLogLevels`: argv.json's levels, the startup ones where it has none.
    pub fn default_log_levels(&self) -> log_levels::DefaultLogLevels {
        let argv = log_levels::read_argv(&self.argv_file);
        if argv == log_levels::DefaultLogLevels::default() {
            return self.startup_log_levels.clone();
        }
        log_levels::DefaultLogLevels { default: argv.default.or(self.startup_log_levels.default), extensions: argv.extensions }
    }

    /// `registerLogger` and `LogOutputChannels.onDidAddLoggers`: a visible logger is a log channel.
    pub fn register_logger(&mut self, logger: Logger) {
        self.loggers.retain(|known| known.file != logger.file);
        if !logger.hidden {
            self.register_log_channel(&logger);
        }
        self.loggers.push(logger);
    }

    /// `deregisterLogger` and `onDidRemoveLoggers`.
    pub fn deregister_logger(&mut self, file: &Path) {
        let Some(index) = self.loggers.iter().position(|logger| logger.file == file) else { return };
        let logger = self.loggers.remove(index);
        self.remove_channel(&logger.id);
    }

    /// `setVisibility` and `onDidChangeVisibility`.
    pub fn set_logger_visibility(&mut self, file: &Path, visible: bool) {
        let Some(logger) = self.loggers.iter_mut().find(|logger| logger.file == file) else { return };
        logger.hidden = !visible;
        let logger = logger.clone();
        match visible {
            true => self.register_log_channel(&logger),
            false => drop(self.remove_channel(&logger.id)),
        }
    }

    // shortcut: a logger's `group` and `when` are ignored, every logger is its own channel; the port's host registers no grouped or conditional logger.
    fn register_log_channel(&mut self, logger: &Logger) {
        let label = logger.name.clone().unwrap_or_else(|| logger.id.clone());
        let channel = OutputChannel { extension_id: logger.extension_id.clone(), ..OutputChannel::new(logger.id.clone(), label, logger.file.clone(), true) };
        self.register_channel(channel);
    }

    /// logService.ts: the Window log, `renderer.log` of the window's logs folder.
    pub fn register_window_log(&mut self, file: PathBuf) {
        let logger = Logger { id: WINDOW_LOG_ID.into(), name: Some("Window".into()), file, hidden: false, extension_id: None, log_level: None };
        self.register_logger(logger);
    }

    /// The Window log (`NativeLogService`'s file logger, a rotating spdlog file of the main process): one line when
    /// the logger's level lets `level` through.
    pub fn log_window(&self, level: u8, message: &str) {
        let Some(logger) = self.loggers.iter().find(|logger| logger.id == WINDOW_LOG_ID) else { return };
        if self.lets_through(logger, level) {
            logger::LogFile { path: logger.file.clone(), rotating: true, formatted: true }.append(level, message);
        }
    }

    /// `AbstractLogger.checkLogLevel`: the logger's own level, else the global one; `Off` (0) lets nothing through.
    fn lets_through(&self, logger: &Logger, level: u8) -> bool {
        let logger_level = logger.log_level.unwrap_or(self.log_level);
        logger_level != 0 && level >= logger_level
    }

    /// mainThreadLogService.ts `$createLogger`: `createLogger(resource, options)` for a logger the host does not write
    /// itself; it is registered as `registerLogger` does, with `options.id` or the resource's hash as id.
    pub fn create_logger(&mut self, file: PathBuf, uri: &str, options: &Value) {
        let always = options["logLevel"] == "always";
        // log.ts `createLogger`: 'always' is Trace and lets every level through.
        let log_level = if always { Some(1) } else { options["logLevel"].as_u64().map(|level| level as u8) };
        let id = options["id"].as_str().map_or_else(|| js_hex(crate::main_thread_status_bar::string_hash(uri)), str::to_owned);
        let logger = Logger { id, name: options["name"].as_str().map(str::to_owned), file: file.clone(), hidden: options["hidden"] == true, extension_id: options["extensionId"].as_str().map(str::to_owned), log_level };
        self.register_logger(logger);
        let log_file = logger::LogFile { path: file, rotating: options["donotRotate"] != true, formatted: options["donotUseFormatters"] != true };
        self.written_loggers.retain(|(known, _)| known.path != log_file.path);
        self.written_loggers.push((log_file, always));
    }

    /// mainThreadLogService.ts `$log`: each message through the created logger's level.
    pub fn log(&self, file: &Path, messages: &[(u8, String)]) -> Result<(), String> {
        let Some((log_file, always)) = self.written_loggers.iter().find(|(log_file, _)| log_file.path == file) else {
            return Err("Create the logger before logging".into());
        };
        let logger = self.loggers.iter().find(|logger| logger.file == file);
        for (level, message) in messages {
            if *always || logger.is_some_and(|logger| self.lets_through(logger, *level)) {
                log_file.append(*level, message);
            }
        }
        Ok(())
    }

    /// mainThreadLogService.ts `$flush`: lines are written as they come, so only the check is left.
    pub fn flush_logger(&self, file: &Path) -> Result<(), String> {
        match self.written_loggers.iter().any(|(log_file, _)| log_file.path == file) {
            true => Ok(()),
            false => Err("Create the logger before flushing".into()),
        }
    }
}

/// `Number.prototype.toString(16)` of a 32-bit hash.
fn js_hex(value: i32) -> String {
    match value < 0 {
        true => format!("-{:x}", value.unsigned_abs()),
        false => format!("{value:x}"),
    }
}

/// `ILoggerResource` from its JSON (`UriDto<ILoggerResource>`).
pub fn logger_from_json(value: &Value) -> Logger {
    let text = |key: &str| value[key].as_str().map(str::to_owned);
    Logger {
        id: text("id").unwrap_or_default(),
        name: text("name"),
        file: PathBuf::from(value["resource"]["path"].as_str().unwrap_or_default()),
        hidden: value["hidden"] == true,
        extension_id: text("extensionId"),
        log_level: value["logLevel"].as_u64().map(|level| level as u8),
    }
}
