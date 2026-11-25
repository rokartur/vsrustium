//! workbench/services/log/common/defaultLogLevels.ts: the default log levels, argv.json `log-level` entries of the
//! form `<level>` or `<extension id>=<level>`. main.ts turns them into `--log` arguments at startup.

use std::path::{Path, PathBuf};

use config::json::{self, ParseOptions, Segment};
use config::json_edit::{FormattingOptions, apply_edits, set_property};
use serde_json::Value;

use super::{DEFAULT_LOG_LEVEL, LOG_LEVELS, log_level_name};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DefaultLogLevels {
    pub default: Option<u8>,
    /// Lowercase extension ids.
    pub extensions: Vec<(String, u8)>,
}

impl DefaultLogLevels {
    /// `getDefaultLogLevel(extensionId)`.
    pub fn level(&self, extension_id: Option<&str>) -> u8 {
        let extension = extension_id.and_then(|id| self.extensions.iter().find(|(known, _)| known.eq_ignore_ascii_case(id)));
        extension.map(|&(_, level)| level).or(self.default).unwrap_or(DEFAULT_LOG_LEVEL)
    }
}

/// environmentService.argvResource: `<home>/<product dataFolderName>/argv.json`.
pub fn argv_file(home: &Path) -> PathBuf {
    home.join(".vsrustium/argv.json")
}

/// log.ts `parseLogLevel`.
pub fn parse_log_level(value: &str) -> Option<u8> {
    match value {
        "critical" => Some(5),
        _ => LOG_LEVELS.iter().find(|(_, name, _)| *name == value).map(|&(level, _, _)| level),
    }
}

/// `_parseLogLevelsFromArgv`: the entries of argv.json `log-level`, a string or an array; later entries win.
pub fn read_argv(file: &Path) -> DefaultLogLevels {
    let Ok(content) = std::fs::read_to_string(file) else { return DefaultLogLevels::default() };
    let argv = json::parse(&content, &mut Vec::new(), ParseOptions::default()).unwrap_or(Value::Null);
    let entries: Vec<&str> = match &argv["log-level"] {
        Value::String(entry) => vec![entry.as_str()],
        Value::Array(entries) => entries.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let mut levels = DefaultLogLevels::default();
    for entry in entries {
        match extension_entry(entry) {
            Some((extension, level)) => {
                let Some(level) = parse_log_level(level) else { continue };
                let extension = extension.to_lowercase();
                levels.extensions.retain(|(known, _)| *known != extension);
                levels.extensions.push((extension, level));
            }
            None => levels.default = parse_log_level(entry).or(levels.default),
        }
    }
    levels
}

/// `_writeLogLevelsToArgv`: `log-level` set to the entries, removed when there are none.
pub fn write_argv(file: &Path, levels: &DefaultLogLevels, options: impl FnOnce(&str) -> FormattingOptions) -> std::io::Result<()> {
    let mut entries: Vec<Value> = levels.default.map(|level| Value::from(log_level_name(level))).into_iter().collect();
    entries.extend(levels.extensions.iter().map(|(extension, level)| Value::from(format!("{extension}={}", log_level_name(*level)))));
    let value = (!entries.is_empty()).then_some(Value::Array(entries));
    let content = std::fs::read_to_string(file).unwrap_or_default();
    let options = options(&content);
    let edits = set_property(&content, &[Segment::Key("log-level".into())], value.as_ref(), &options);
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder)?;
    }
    std::fs::write(file, apply_edits(&content, &edits))
}

/// environmentService.ts `EXTENSION_IDENTIFIER_WITH_LOG_REGEX`, `^([^.]+\..+)[:=](.+)$`.
fn extension_entry(entry: &str) -> Option<(&str, &str)> {
    let separator = entry[..entry.len() - entry.chars().last()?.len_utf8()].rfind([':', '='])?;
    let (extension, level) = (&entry[..separator], &entry[separator + 1..]);
    let dot = extension.find('.')?;
    (dot > 0 && dot + 1 < extension.len()).then_some((extension, level))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_argv_log_levels() {
        let dir = std::env::temp_dir().join(format!("argv-log-levels-{}", std::process::id()));
        let file = dir.join("argv.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, "{\n\t// comment\n\t\"log-level\": [\"debug\", \"Pub.Ext=trace\", \"x=off\", \"bad\"]\n}\n").unwrap();
        let levels = read_argv(&file);
        assert_eq!(levels, DefaultLogLevels { default: Some(2), extensions: vec![("pub.ext".into(), 1)] });
        assert_eq!(levels.level(Some("PUB.EXT")), 1);
        assert_eq!(levels.level(Some("other.ext")), 2);

        let tabs = |_: &str| FormattingOptions { tab_size: 4, insert_spaces: false, eol: "\n".into() };
        write_argv(&file, &DefaultLogLevels { default: Some(4), extensions: vec![("pub.ext".into(), 0)] }, tabs).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\n\t// comment\n\t\"log-level\": [\n\t\t\"warn\",\n\t\t\"pub.ext=off\"\n\t]\n}\n");
        write_argv(&file, &DefaultLogLevels::default(), tabs).unwrap();
        assert_eq!(read_argv(&file), DefaultLogLevels::default());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
