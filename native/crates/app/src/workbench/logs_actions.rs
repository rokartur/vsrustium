//! workbench/contrib/logs: Set Log Level..., Set Default Log Level, Show Window Log, Open Logs Folder and Open
//! Extension Logs Folder (logs.contribution.ts, logsActions.ts), with output.contribution.ts Show Logs... and Open
//! Log..., and logsDataCleaner.ts removing the folders of old sessions.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::output_actions::{OutputPick, OutputPicks};
use crate::commands::Outcome;
use crate::output::log_levels::{self, DefaultLogLevels};
use crate::output::{LOG_LEVELS, OutputChannel, WINDOW_LOG_ID};
use crate::quick_input::{Button, Entry, Item};

impl Workbench {
    pub(super) fn register_logs_actions(&mut self) {
        self.commands.register("workbench.action.setLogLevel", |workbench, _, window, cx| {
            workbench.show_set_log_level_picker(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.setDefaultLogLevel", |workbench, args, _, _| {
            let Some(level) = args.first().and_then(Value::as_u64).filter(|level| *level <= 5) else { return Outcome::error("a log level from 0 to 5") };
            let extension_id = args.get(1).and_then(Value::as_str).map(str::to_owned);
            match workbench.set_default_log_level(level as u8, extension_id.as_deref()) {
                Ok(()) => Outcome::done(),
                Err(error) => Outcome::error(&format!("cannot write {}: {error}", workbench.output.argv_file.display())),
            }
        });
        self.commands.register("workbench.action.output.activeOutputLogLevelDefault", |workbench, _, _, _| {
            let Some(channel) = workbench.output.active_channel() else { return Outcome::done() };
            let (id, extension_id) = (channel.id.clone(), channel.extension_id.clone());
            let Some(level) = workbench.output.channel_log_level(&id) else { return Outcome::done() };
            match workbench.set_default_log_level(level, extension_id.as_deref()) {
                Ok(()) => Outcome::done(),
                Err(error) => Outcome::error(&format!("cannot write {}: {error}", workbench.output.argv_file.display())),
            }
        });
        self.commands.register("workbench.action.showWindowLog", |workbench, _, window, cx| {
            workbench.show_output_channel(WINDOW_LOG_ID, false, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.openLogsFolder", |workbench, _, _, cx| {
            if let Some(logs_home) = workbench.window_logs_folder().and_then(|folder| folder.parent().map(Path::to_path_buf)) {
                // shortcut: the port has no main process writing main.log; its session folder is revealed instead.
                let main_log = logs_home.join("main.log");
                cx.reveal_path(if main_log.exists() { &main_log } else { &logs_home });
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.openExtensionLogsFolder", |workbench, _, _, cx| {
            let Some(folder) = workbench.window_logs_folder().map(|folder| folder.join("exthost")) else { return Outcome::done() };
            let mut children: Vec<PathBuf> = std::fs::read_dir(&folder).into_iter().flatten().flatten().map(|entry| entry.path()).collect();
            children.sort();
            if let Some(first) = children.first() {
                cx.reveal_path(first);
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.showLogs", |workbench, _, window, cx| {
            workbench.show_logs_picker(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.openLogFile", |workbench, args, window, cx| {
            workbench.open_log_file(args.first().and_then(Value::as_str), window, cx);
            Outcome::done()
        });
    }

    /// The startup log levels from argv.json (main.ts `--log` arguments), and logsDataCleaner.ts ten seconds later.
    pub fn start_logs(&mut self, argv_file: PathBuf, logs_home: PathBuf, cx: &mut Context<Self>) {
        self.output.startup_log_levels = log_levels::read_argv(&argv_file);
        self.output.argv_file = argv_file;
        let level = self.output.startup_log_levels.level(None);
        if level != self.output.log_level {
            self.set_log_level(level);
        }
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(Duration::from_secs(10)).await;
            cx.background_executor().spawn(async move { remove_old_log_sessions(&logs_home) }).await;
        })
        .detach();
    }

    /// The folder of the Window log, environmentService.windowLogsPath.
    fn window_logs_folder(&self) -> Option<PathBuf> {
        let window = self.output.loggers.iter().find(|logger| logger.id == WINDOW_LOG_ID)?;
        window.file.parent().map(Path::to_path_buf)
    }

    /// `LoggerService.setLogLevel(level)`: every logger without its own level follows, and `MainThreadLogger` tells
    /// the host.
    pub(super) fn set_log_level(&mut self, level: u8) {
        self.output.log_level = level;
        self.host.send("ExtHostLogLevelServiceShape", "$setLogLevel", vec![json!(level)]);
    }

    /// defaultLogLevels.ts `setDefaultLogLevel`: argv.json gets the level; the loggers still at the old default move
    /// to the new one.
    pub(super) fn set_default_log_level(&mut self, level: u8, extension_id: Option<&str>) -> std::io::Result<()> {
        let argv = log_levels::read_argv(&self.output.argv_file);
        let mut levels = DefaultLogLevels { default: argv.default.or(self.output.startup_log_levels.default), extensions: argv.extensions.clone() };
        let current = levels.level(extension_id);
        let mut written = argv;
        match extension_id.map(str::to_lowercase) {
            Some(extension) => {
                written.extensions.retain(|(known, _)| *known != extension);
                written.extensions.push((extension, level));
            }
            None => written.default = Some(level),
        }
        let configuration = self.settings.lock().unwrap().configuration.clone();
        log_levels::write_argv(&self.output.argv_file, &written, |content| config::service::formatting_options(content, &configuration, &self.output.argv_file))?;
        levels.extensions = written.extensions;
        let Some(extension_id) = extension_id else {
            if self.output.log_level == current {
                self.set_log_level(level);
            }
            return Ok(());
        };
        let global = self.output.log_level;
        for logger in &mut self.output.loggers {
            let matches = logger.extension_id.as_deref().is_some_and(|id| id.eq_ignore_ascii_case(extension_id));
            if !matches || logger.log_level.unwrap_or(global) != current || current == level {
                continue;
            }
            logger.log_level = (level != global).then_some(level);
            self.host.send("ExtHostLogLevelServiceShape", "$setLogLevel", vec![json!(level), exthost::scan::file_uri(&logger.file)]);
        }
        Ok(())
    }

    /// logsActions.ts `SetLogLevelAction.selectLogLevelOrChannel`: the global levels, then the log channels.
    fn show_set_log_level_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let defaults = self.output.default_log_levels();
        let global = self.output.log_level;
        let mut extension_logs: Vec<(String, Option<String>, String)> = Vec::new();
        let mut logs = Vec::new();
        for channel in self.output.channels.iter().filter(|channel| channel.log) {
            let level = self.output.channel_log_level(&channel.id).unwrap_or(global);
            let row = (channel.label.clone(), (level != global).then(|| level_label(level, None)), channel.id.clone());
            match channel.extension_id {
                Some(_) => extension_logs.push(row),
                None => logs.push(row),
            }
        }
        let mut picks = OutputPicks::default();
        picks.entries.push(Entry::Separator(Some("All".into())));
        for (item, level) in level_items(defaults.level(None), global, true) {
            picks.push(item, OutputPick::Level(level));
        }
        for (separator, mut rows) in [("Extension Logs", extension_logs), ("Logs", logs)] {
            if separator == "Extension Logs" && rows.is_empty() {
                continue;
            }
            picks.entries.push(Entry::Separator(Some(separator.into())));
            rows.sort_by_key(|(label, _, _)| label.to_lowercase());
            for (label, description, id) in rows {
                picks.push(Item { label, description, ..Item::default() }, OutputPick::LogChannel(id));
            }
        }
        self.show_output_picker("Set Log Level", picks, &[], window, cx);
    }

    /// logsActions.ts `SetLogLevelAction.setLogLevelForChannel`: the levels of one log channel, the global level active.
    pub(super) fn show_channel_log_level_picker(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(channel) = self.output.channel(id) else { return };
        let default = self.output.default_log_levels().level(channel.extension_id.as_deref());
        let current = self.output.channel_log_level(id).unwrap_or(default);
        let placeholder = format!(" {}: Select log level", channel.label);
        let mut picks = OutputPicks::default();
        let mut active = Vec::new();
        for (item, level) in level_items(default, current, channel.extension_id.is_some()) {
            if level == self.output.log_level {
                active.push(picks.picks.len() as i64);
            }
            picks.push(item, OutputPick::ChannelLevel(id.to_owned(), level));
        }
        self.show_output_picker(&placeholder, picks, &active, window, cx);
    }

    /// output.contribution.ts `registerShowLogsAction`: the port's logs, then the extensions'.
    fn show_logs_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (extension_logs, logs): (Vec<&OutputChannel>, Vec<&OutputChannel>) = self.output.channels.iter().filter(|channel| channel.log).partition(|channel| channel.extension_id.is_some());
        let mut picks = OutputPicks::default();
        for channel in &logs {
            picks.push(Item { label: channel.label.clone(), ..Item::default() }, OutputPick::Show(channel.id.clone()));
        }
        if !extension_logs.is_empty() && !logs.is_empty() {
            picks.entries.push(Entry::Separator(Some("Extension Logs".into())));
        }
        for channel in &extension_logs {
            picks.push(Item { label: channel.label.clone(), ..Item::default() }, OutputPick::Show(channel.id.clone()));
        }
        self.show_output_picker("Select Log", picks, &[], window, cx);
    }

    /// output.contribution.ts `registerOpenLogFileAction`: the log of the id argument, else a pick of the extension
    /// logs, then the others. Upstream lists the others only after extension logs, so without one the pick is empty.
    fn open_log_file(&mut self, id: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(channel) = id.and_then(|id| self.output.channel(id)).filter(|channel| channel.log) {
            let file = channel.file.clone();
            self.open_file(file, true, window, cx);
            return;
        }
        let (mut extension_logs, mut logs): (Vec<&OutputChannel>, Vec<&OutputChannel>) = self.output.channels.iter().filter(|channel| channel.log).partition(|channel| channel.extension_id.is_some());
        extension_logs.sort_by_key(|channel| channel.label.to_lowercase());
        logs.sort_by_key(|channel| channel.label.to_lowercase());
        let mut picks = OutputPicks::default();
        for channel in &extension_logs {
            picks.push(Item { label: channel.label.clone(), ..Item::default() }, OutputPick::OpenFile(channel.id.clone()));
        }
        if !extension_logs.is_empty() && !logs.is_empty() {
            picks.entries.push(Entry::Separator(None));
            for channel in &logs {
                picks.push(Item { label: channel.label.clone(), ..Item::default() }, OutputPick::OpenFile(channel.id.clone()));
            }
        }
        self.show_output_picker("Select Log File", picks, &[], window, cx);
    }
}

/// logsActions.ts `getLogLevelEntries`: the levels, the current one checked, the default one described, and a Set as
/// Default Log Level button on the others when the default can be set.
fn level_items(default: u8, current: u8, can_set_default: bool) -> Vec<(Item, u8)> {
    let mut items = Vec::new();
    for &(level, _, _) in &LOG_LEVELS {
        let button = Button { codicon: Some("check-all".into()), tooltip: Some("Set as Default Log Level".into()), ..Button::default() };
        let buttons = if can_set_default && level != default { vec![button] } else { Vec::new() };
        let description = (level == default).then(|| "Default".to_owned());
        items.push((Item { label: level_label(level, Some(current)), description, buttons, ..Item::default() }, level));
    }
    items
}

/// logsActions.ts `getLabel`.
fn level_label(level: u8, current: Option<u8>) -> String {
    let label = LOG_LEVELS.iter().find(|(known, _, _)| *known == level).expect("a log level").2;
    if current == Some(level) { format!("$(check) {label}") } else { label.to_owned() }
}

/// logsDataCleaner.ts (sharedProcess): ten seconds after startup, the session folders of `<userData>/logs` but the
/// current one and the nine newest are removed.
pub fn remove_old_log_sessions(logs_home: &Path) {
    let Some((root, current)) = logs_home.parent().zip(logs_home.file_name()) else { return };
    let mut sessions: Vec<_> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name())
        .filter(|name| name != current && is_session_name(&name.to_string_lossy()))
        .collect();
    sessions.sort();
    let removed = sessions.len().saturating_sub(9);
    for session in &sessions[..removed] {
        if let Err(error) = std::fs::remove_dir_all(root.join(session)) {
            eprintln!("[logs cleanup] cannot remove {}: {error}", root.join(session).display());
        }
    }
}

/// `/^\d{8}T\d{6}$/`.
fn is_session_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 15 && bytes[8] == b'T' && bytes.iter().enumerate().all(|(i, byte)| i == 8 || byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    #[test]
    fn old_log_sessions_are_removed() {
        let root = std::env::temp_dir().join(format!("code-gpui-logs-{}", std::process::id()));
        let sessions: Vec<String> = (10..22).map(|day| format!("202601{day}T120000")).collect();
        for name in sessions.iter().chain([&"backup".to_owned()]) {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        // The current session is the oldest name here and stays anyway.
        super::remove_old_log_sessions(&root.join(&sessions[0]));
        let mut left: Vec<String> = std::fs::read_dir(&root).unwrap().map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        let mut expected: Vec<String> = sessions[3..].to_vec();
        expected.extend([sessions[0].clone(), "backup".into()]);
        expected.sort();
        assert_eq!(left, expected);
        std::fs::remove_dir_all(root).unwrap();
    }
}
