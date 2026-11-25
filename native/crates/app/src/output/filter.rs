//! The Output view's filters (outputServices.ts `OutputViewFilters`) and the log entries they apply to
//! (outputChannelModel.ts `parseLogEntryAt`, outputView.ts `FilterController`).

use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;

static LOG_ENTRY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3})\s(\[(info|trace|debug|error|warning)\])\s(\[(.*?)\])?").unwrap());

/// `LogLevel` of an entry, by its `[level]` header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warning,
    Error,
}

/// `ILogEntry`: 0-based model lines, the header line first.
#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    pub lines: Range<usize>,
    /// `new Date(header).getTime()`: the header's local time in milliseconds since the epoch.
    pub timestamp: i64,
    pub level: Level,
    pub category: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Filters {
    pub text: String,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub trace: bool,
    pub debug: bool,
    pub info: bool,
    pub warning: bool,
    pub error: bool,
    /// `,channel:category,` of each hidden category.
    pub categories: String,
}

impl Default for Filters {
    fn default() -> Self {
        Filters { text: String::new(), include: Vec::new(), exclude: Vec::new(), trace: true, debug: true, info: true, warning: true, error: true, categories: ",".into() }
    }
}

impl Filters {
    /// `set text`: comma separated patterns; `!` excludes. A quoted pattern keeps its quotes and its commas.
    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_owned();
        self.include.clear();
        self.exclude.clear();
        for pattern in split_by_comma_respecting_quotes(text) {
            let trimmed = pattern.trim();
            match trimmed.strip_prefix('!') {
                Some(negative) if !negative.trim().is_empty() => self.exclude.push(negative.trim().to_owned()),
                Some(_) => {}
                None if !trimmed.is_empty() => self.include.push(trimmed.to_owned()),
                None => {}
            }
        }
    }

    pub fn level_shown(&self, level: Level) -> bool {
        match level {
            Level::Trace => self.trace,
            Level::Debug => self.debug,
            Level::Info => self.info,
            Level::Warning => self.warning,
            Level::Error => self.error,
        }
    }

    pub fn level_mut(&mut self, level: Level) -> &mut bool {
        match level {
            Level::Trace => &mut self.trace,
            Level::Debug => &mut self.debug,
            Level::Info => &mut self.info,
            Level::Warning => &mut self.warning,
            Level::Error => &mut self.error,
        }
    }

    pub fn toggle_category(&mut self, category: &str) {
        match self.has_category(category) {
            true => self.categories = self.categories.replace(&format!(",{category},"), ","),
            false => self.categories = format!("{}{category},", self.categories),
        }
    }

    pub fn has_category(&self, category: &str) -> bool {
        category != "," && self.categories.contains(&format!(",{category},"))
    }

    /// The text the Output view shows: `text` without the lines the filters hide.
    // shortcut: hidden lines leave the view's model instead of becoming hidden areas (`setHiddenAreas`); the port's
    // view model hides folded regions only. Upgrade when the editor gets hidden areas.
    pub fn shown_text(&self, text: &str, channel: &str) -> String {
        let lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        let hidden = self.hidden_lines(&lines, &parse_log_entries(&lines), channel);
        if hidden.is_empty() {
            return text.to_owned();
        }
        let mut shown = vec![true; lines.len()];
        for range in hidden {
            shown[range].fill(false);
        }
        let lines: Vec<&str> = lines.iter().zip(shown).filter(|(_, shown)| *shown).map(|(line, _)| line.as_str()).collect();
        lines.join("\n")
    }

    /// `OutputViewPane.checkMoreFilters`: the filter button shows as checked.
    pub fn more_filters(&self, channel: Option<&str>) -> bool {
        let hidden_category = channel.is_some_and(|channel| self.categories.contains(&format!(",{channel}:")));
        !self.trace || !self.debug || !self.info || !self.warning || !self.error || hidden_category
    }

    /// `FilterController.compute`: the model lines the filters hide, ascending; with log entries an entry hides
    /// whole, else each line on its own. `shouldShowLine`: no exclude pattern matches, and an include pattern does
    /// when there are any.
    pub fn hidden_lines(&self, lines: &[String], entries: &[LogEntry], channel: &str) -> Vec<Range<usize>> {
        let mut hidden = Vec::new();
        let patterns = !self.include.is_empty() || !self.exclude.is_empty();
        let include: Vec<Regex> = self.include.iter().map(|pattern| plain_text(pattern)).collect();
        let exclude: Vec<Regex> = self.exclude.iter().map(|pattern| plain_text(pattern)).collect();
        let shows = |lines: &[String]| {
            let matches = |pattern: &Regex| lines.iter().any(|line| pattern.is_match(line));
            !exclude.iter().any(matches) && (include.is_empty() || include.iter().any(matches))
        };
        if !entries.is_empty() {
            for entry in entries {
                let category_hidden = entry.category.as_ref().is_some_and(|category| self.has_category(&format!("{channel}:{category}")));
                if !self.level_shown(entry.level) || category_hidden || (patterns && !shows(&lines[entry.lines.clone()])) {
                    hidden.push(entry.lines.clone());
                }
            }
            return hidden;
        }
        if !patterns {
            return hidden;
        }
        for (line, text) in lines.iter().enumerate() {
            if !shows(std::slice::from_ref(text)) {
                hidden.push(line..line + 1);
            }
        }
        hidden
    }
}

/// `shouldShowLine` patterns match as plain text, ignoring case (`findMatches(pattern, range, false, false)`).
fn plain_text(pattern: &str) -> Regex {
    Regex::new(&format!("(?i){}", regex::escape(pattern))).unwrap()
}

fn split_by_comma_respecting_quotes(text: &str) -> Vec<String> {
    let mut patterns = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for char in text.chars() {
        match char {
            '"' => {
                in_quotes = !in_quotes;
                current.push(char);
            }
            ',' if !in_quotes => {
                if !current.is_empty() {
                    patterns.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(char),
        }
    }
    if !current.is_empty() {
        patterns.push(current);
    }
    patterns
}

/// `parseLogEntries` over a whole text: an entry runs from its header line to the line before the next header; the
/// text's last empty line belongs to none. A text whose first line is no header has no entries.
/// `new Date('2023-10-15 14:30:45.123').getTime()`: a date and time without a zone is local time.
fn local_timestamp(text: &str) -> i64 {
    let naive = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.3f").expect("LOG_ENTRY matched a date and time");
    naive.and_local_timezone(chrono::Local).earliest().map_or_else(|| naive.and_utc().timestamp_millis(), |local| local.timestamp_millis())
}

pub fn parse_log_entries(lines: &[String]) -> Vec<LogEntry> {
    let header = |line: usize| LOG_ENTRY.captures(&lines[line]);
    let mut entries = Vec::new();
    if lines.is_empty() || header(0).is_none() {
        return entries;
    }
    let last = lines.len() - 1;
    let mut line = 0;
    while line < lines.len() {
        let Some(captures) = header(line) else {
            line += 1;
            continue;
        };
        let level = match &captures[3] {
            "trace" => Level::Trace,
            "debug" => Level::Debug,
            "info" => Level::Info,
            "warning" => Level::Warning,
            _ => Level::Error,
        };
        let category = captures.get(5).map(|category| category.as_str().to_owned());
        let mut end = line;
        while end < last && header(end + 1).is_none() && !(end + 1 == last && lines[last].is_empty()) {
            end += 1;
        }
        entries.push(LogEntry { lines: line..end + 1, timestamp: local_timestamp(&captures[1]), level, category });
        line = end + 1;
    }
    entries
}
