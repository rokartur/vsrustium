//! problemMatcher.ts: problem patterns and matchers (the built-in ones, `contributes.problemPatterns` and
//! `contributes.problemMatchers`, and the ones a tasks.json writes inline) and the line matchers that turn
//! task output into markers. Regular expressions run on Oniguruma, whose syntax is close to JavaScript's.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

/// `MarkerSeverity`.
pub const SEVERITY_ERROR: u8 = 8;
pub const SEVERITY_WARNING: u8 = 4;
pub const SEVERITY_INFO: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FileLocation {
    Absolute,
    Relative,
    AutoDetect,
    Search,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ApplyTo {
    AllDocuments,
    OpenDocuments,
    ClosedDocuments,
}

#[derive(Clone)]
pub struct Pattern {
    pub regexp: Arc<onig::Regex>,
    /// `ProblemLocationKind.File`: the match is about the whole file.
    pub file_kind: bool,
    /// `kind: "location"` was given: a match without a line is about the whole file.
    pub location_kind: bool,
    pub file: Option<usize>,
    pub location: Option<usize>,
    pub line: Option<usize>,
    pub character: Option<usize>,
    pub end_line: Option<usize>,
    pub end_character: Option<usize>,
    pub severity: Option<usize>,
    pub code: Option<usize>,
    pub message: Option<usize>,
    pub loop_: bool,
}

#[derive(Clone)]
pub struct WatchingPattern {
    pub regexp: Arc<onig::Regex>,
    pub file: usize,
}

#[derive(Clone)]
pub struct Watching {
    pub active_on_start: bool,
    pub begins: WatchingPattern,
    pub ends: WatchingPattern,
}

#[derive(Clone)]
pub struct ProblemMatcher {
    pub owner: String,
    pub source: Option<String>,
    pub apply_to: ApplyTo,
    pub file_location: FileLocation,
    /// `filePrefix`, with `${workspaceFolder}` already resolved by the task system.
    pub file_prefix: Option<String>,
    /// `Search` arguments: the folders to search and the ones to leave out.
    pub search_include: Vec<String>,
    pub search_exclude: Vec<String>,
    pub patterns: Vec<Pattern>,
    pub severity: Option<u8>,
    pub watching: Option<Watching>,
    /// `INamedProblemMatcher`.
    pub name: Option<String>,
    pub label: Option<String>,
}

/// The problem patterns and named matchers (`ProblemPatternRegistry`, `ProblemMatcherRegistry`).
#[derive(Default, Clone)]
pub struct Registry {
    patterns: HashMap<String, Vec<Pattern>>,
    matchers: HashMap<String, ProblemMatcher>,
}

impl Registry {
    /// The built-in patterns and matchers, then those the extensions contribute, in extension order.
    pub fn new(contributions: &[&Value]) -> Self {
        let mut registry = Registry::default();
        registry.fill_defaults();
        for contributes in contributions {
            for pattern in contributes["problemPatterns"].as_array().into_iter().flatten() {
                let Some(name) = pattern["name"].as_str() else { continue };
                let patterns = match &pattern["patterns"] {
                    Value::Array(patterns) => parse_patterns(patterns),
                    _ => parse_pattern(pattern, true).map(|pattern| vec![pattern]),
                };
                if let Some(patterns) = patterns {
                    registry.patterns.insert(name.to_owned(), patterns);
                }
            }
        }
        for contributes in contributions {
            for matcher in contributes["problemMatchers"].as_array().into_iter().flatten() {
                if let Some(parsed) = registry.parse(matcher)
                    && let Some(name) = parsed.name.clone()
                {
                    registry.matchers.insert(name, parsed);
                }
            }
        }
        registry
    }

    pub fn get(&self, name: &str) -> Option<&ProblemMatcher> {
        self.matchers.get(name)
    }

    /// `ProblemMatcherParser.parse`: an object matcher, possibly based on a named one (`base`).
    pub fn parse(&self, description: &Value) -> Option<ProblemMatcher> {
        let owner = description["owner"].as_str().map(str::to_owned).unwrap_or_else(exthost::main_thread_storage::generate_uuid);
        let source = description["source"].as_str().map(str::to_owned);
        let apply_to = match description["applyTo"].as_str() {
            Some("openDocuments") => ApplyTo::OpenDocuments,
            Some("closedDocuments") => ApplyTo::ClosedDocuments,
            _ => ApplyTo::AllDocuments,
        };
        let (file_location, file_prefix, search) = parse_file_location(&description["fileLocation"]);
        let patterns = match &description["pattern"] {
            Value::String(name) => name.strip_prefix('$').and_then(|name| self.patterns.get(name)).cloned(),
            Value::Array(patterns) => parse_patterns(patterns),
            Value::Object(_) => parse_pattern(&description["pattern"], true).map(|pattern| vec![pattern]),
            _ => None,
        };
        let severity = description["severity"].as_str().map(|severity| severity_from_value(severity).unwrap_or(SEVERITY_ERROR));
        let mut result = if let Some(base) = description["base"].as_str().and_then(|base| base.strip_prefix('$')) {
            let mut result = self.matchers.get(base)?.clone();
            if description["owner"].is_string() {
                result.owner = owner;
            }
            if source.is_some() {
                result.source = source;
            }
            if let Some(file_location) = file_location.filter(|_| !description["fileLocation"].is_null()) {
                result.file_location = file_location;
                result.file_prefix = file_prefix;
                (result.search_include, result.search_exclude) = search;
            }
            if let Some(patterns) = patterns.filter(|_| !description["pattern"].is_null()) {
                result.patterns = patterns;
            }
            if severity.is_some() {
                result.severity = severity;
            }
            if description["applyTo"].is_string() {
                result.apply_to = apply_to;
            }
            result
        } else {
            let (file_location, patterns) = (file_location?, patterns?);
            ProblemMatcher {
                owner,
                source,
                apply_to,
                file_location,
                file_prefix,
                search_include: search.0,
                search_exclude: search.1,
                patterns,
                severity,
                watching: None,
                name: None,
                label: None,
            }
        };
        if let Some(watching) = parse_watching(description) {
            result.watching = Some(watching);
        }
        if let Some(name) = description["name"].as_str() {
            result.name = Some(name.to_owned());
            result.label = Some(description["label"].as_str().unwrap_or(name).to_owned());
        }
        Some(result)
    }

    fn add_pattern(&mut self, name: &str, patterns: Vec<Pattern>) {
        self.patterns.insert(name.to_owned(), patterns);
    }

    fn fill_defaults(&mut self) {
        let single = |regexp: &str, fields: &[(&str, usize)]| vec![pattern(regexp, fields, false)];
        self.add_pattern("msCompile", single(r"^\s*(?:\s*\d+>)?(\S.*?)(?:\((\d+|\d+,\d+|\d+,\d+,\d+,\d+)\))?\s*:\s+(?:(\S+)\s+)?((?:fatal +)?error|warning|info)\s+(\w+\d+)?\s*:\s*(.*)$", &[("file", 1), ("location", 2), ("severity", 4), ("code", 5), ("message", 6)]));
        self.add_pattern("gulp-tsc", single(r"^([^\s].*)\((\d+|\d+,\d+|\d+,\d+,\d+,\d+)\):\s+(\d+)\s+(.*)$", &[("file", 1), ("location", 2), ("code", 3), ("message", 4)]));
        self.add_pattern("cpp", single(r"^(\S.*)\((\d+|\d+,\d+|\d+,\d+,\d+,\d+)\):\s+(error|warning|info)\s+(C\d+)\s*:\s*(.*)$", &[("file", 1), ("location", 2), ("severity", 3), ("code", 4), ("message", 5)]));
        self.add_pattern("csc", single(r"^(\S.*)\((\d+|\d+,\d+|\d+,\d+,\d+,\d+)\):\s+(error|warning|info)\s+(CS\d+)\s*:\s*(.*)$", &[("file", 1), ("location", 2), ("severity", 3), ("code", 4), ("message", 5)]));
        self.add_pattern("vb", single(r"^(\S.*)\((\d+|\d+,\d+|\d+,\d+,\d+,\d+)\):\s+(error|warning|info)\s+(BC\d+)\s*:\s*(.*)$", &[("file", 1), ("location", 2), ("severity", 3), ("code", 4), ("message", 5)]));
        self.add_pattern("lessCompile", single(r"^\s*(.*) in file (.*) line no. (\d+)$", &[("message", 1), ("file", 2), ("line", 3)]));
        self.add_pattern("jshint", single(r"^(.*):\s+line\s+(\d+),\s+col\s+(\d+),\s(.+?)(?:\s+\((\w)(\d+)\))?$", &[("file", 1), ("line", 2), ("character", 3), ("message", 4), ("severity", 5), ("code", 6)]));
        self.add_pattern(
            "jshint-stylish",
            vec![pattern(r"^(.+)$", &[("file", 1)], false), pattern(r"^\s+line\s+(\d+)\s+col\s+(\d+)\s+(.+?)(?:\s+\((\w)(\d+)\))?$", &[("line", 1), ("character", 2), ("message", 3), ("severity", 4), ("code", 5)], true)],
        );
        self.add_pattern("eslint-compact", single(r"^(.+):\sline\s(\d+),\scol\s(\d+),\s(Error|Warning|Info)\s-\s(.+)\s\((.+)\)$", &[("file", 1), ("line", 2), ("character", 3), ("severity", 4), ("message", 5), ("code", 6)]));
        self.add_pattern(
            "eslint-stylish",
            vec![pattern(r"^((?:[a-zA-Z]:)*[./\\]+.*?)$", &[("file", 1)], false), pattern(r"^\s+(\d+):(\d+)\s+(error|warning|info)\s+(.+?)(?:\s\s+(.*))?$", &[("line", 1), ("character", 2), ("severity", 3), ("message", 4), ("code", 5)], true)],
        );
        self.add_pattern("go", single(r"^([^:]*: )?((.:)?[^:]*):(\d+)(:(\d+))?: (.*)$", &[("file", 2), ("line", 4), ("character", 6), ("message", 7)]));
        let defaults = [
            ("msCompile", "Microsoft compiler problems", "msCompile", "cpp", ApplyTo::AllDocuments, FileLocation::Absolute, None, None),
            ("lessCompile", "Less problems", "lessCompile", "less", ApplyTo::AllDocuments, FileLocation::Absolute, None, Some(SEVERITY_ERROR)),
            ("gulp-tsc", "Gulp TSC Problems", "typescript", "ts", ApplyTo::ClosedDocuments, FileLocation::Relative, Some("${workspaceFolder}"), None),
            ("jshint", "JSHint problems", "jshint", "jshint", ApplyTo::AllDocuments, FileLocation::Absolute, None, None),
            ("jshint-stylish", "JSHint stylish problems", "jshint", "jshint", ApplyTo::AllDocuments, FileLocation::Absolute, None, None),
            ("eslint-compact", "ESLint compact problems", "eslint", "eslint", ApplyTo::AllDocuments, FileLocation::Absolute, Some("${workspaceFolder}"), None),
            ("eslint-stylish", "ESLint stylish problems", "eslint", "eslint", ApplyTo::AllDocuments, FileLocation::Absolute, None, None),
            ("go", "Go problems", "go", "go", ApplyTo::AllDocuments, FileLocation::Relative, Some("${workspaceFolder}"), None),
        ];
        for (name, label, owner, source, apply_to, file_location, file_prefix, severity) in defaults {
            let matcher = ProblemMatcher {
                owner: owner.into(),
                source: Some(source.into()),
                apply_to,
                file_location,
                file_prefix: file_prefix.map(str::to_owned),
                search_include: Vec::new(),
                search_exclude: Vec::new(),
                patterns: self.patterns[name].clone(),
                severity,
                watching: None,
                name: Some(name.into()),
                label: Some(label.into()),
            };
            self.matchers.insert(name.into(), matcher);
        }
    }
}

fn pattern(regexp: &str, fields: &[(&str, usize)], loop_: bool) -> Pattern {
    let mut pattern = Pattern {
        regexp: Arc::new(onig::Regex::new(regexp).expect("built-in pattern compiles")),
        file_kind: false,
        location_kind: true,
        file: None,
        location: None,
        line: None,
        character: None,
        end_line: None,
        end_character: None,
        severity: None,
        code: None,
        message: None,
        loop_,
    };
    for (field, index) in fields {
        set_field(&mut pattern, field, *index);
    }
    pattern
}

fn set_field(pattern: &mut Pattern, field: &str, index: usize) {
    let slot = match field {
        "file" => &mut pattern.file,
        "location" => &mut pattern.location,
        "line" => &mut pattern.line,
        "character" | "column" => &mut pattern.character,
        "endLine" => &mut pattern.end_line,
        "endCharacter" | "endColumn" => &mut pattern.end_character,
        "severity" => &mut pattern.severity,
        "code" => &mut pattern.code,
        "message" => &mut pattern.message,
        _ => return,
    };
    *slot = Some(index);
}

/// `doCreateSingleProblemPattern`; `set_defaults` fills file 1, line 2, column 3 and message 0.
fn parse_pattern(value: &Value, set_defaults: bool) -> Option<Pattern> {
    let regexp = onig::Regex::new(value["regexp"].as_str()?).ok()?;
    let mut pattern = Pattern {
        regexp: Arc::new(regexp),
        file_kind: value["kind"].as_str().is_some_and(|kind| kind.eq_ignore_ascii_case("file")),
        location_kind: value["kind"].as_str().is_some_and(|kind| kind.eq_ignore_ascii_case("location")),
        file: None,
        location: None,
        line: None,
        character: None,
        end_line: None,
        end_character: None,
        severity: None,
        code: None,
        message: None,
        loop_: value["loop"] == true,
    };
    for field in ["file", "location", "line", "column", "endLine", "endColumn", "severity", "code", "message"] {
        if let Some(index) = value[field].as_u64() {
            set_field(&mut pattern, field, index as usize);
        }
    }
    if set_defaults {
        pattern.file = pattern.file.or(Some(1));
        pattern.message = pattern.message.or(Some(0));
        if pattern.location.is_none() && !pattern.file_kind {
            pattern.line = pattern.line.or(Some(2));
            pattern.character = pattern.character.or(Some(3));
        }
    }
    Some(pattern)
}

/// `createMultiLineProblemPattern`: no defaults; only the last pattern may loop.
fn parse_patterns(values: &[Value]) -> Option<Vec<Pattern>> {
    let mut patterns = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let mut pattern = parse_pattern(value, false)?;
        if index + 1 < values.len() {
            pattern.loop_ = false;
        }
        patterns.push(pattern);
    }
    if values.len() == 1 {
        return parse_pattern(&values[0], true).map(|pattern| vec![pattern]);
    }
    (!patterns.is_empty()).then_some(patterns)
}

fn parse_file_location(value: &Value) -> (Option<FileLocation>, Option<String>, (Vec<String>, Vec<String>)) {
    let kind = |name: &str| match name.to_lowercase().as_str() {
        "absolute" => Some(FileLocation::Absolute),
        "relative" => Some(FileLocation::Relative),
        "autodetect" => Some(FileLocation::AutoDetect),
        "search" => Some(FileLocation::Search),
        _ => None,
    };
    let folder = || vec!["${workspaceFolder}".to_owned()];
    match value {
        Value::Null => (Some(FileLocation::Relative), Some("${workspaceFolder}".into()), Default::default()),
        Value::String(name) => match kind(name) {
            Some(location @ (FileLocation::Relative | FileLocation::AutoDetect)) => (Some(location), Some("${workspaceFolder}".into()), Default::default()),
            Some(FileLocation::Search) => (Some(FileLocation::Search), None, (folder(), Vec::new())),
            location => (location, None, Default::default()),
        },
        Value::Array(values) => match (values.first().and_then(Value::as_str).and_then(kind), values.get(1)) {
            (Some(FileLocation::Absolute), None) => (Some(FileLocation::Absolute), None, Default::default()),
            (Some(location @ (FileLocation::Relative | FileLocation::AutoDetect)), Some(Value::String(prefix))) if !prefix.is_empty() => (Some(location), Some(prefix.clone()), Default::default()),
            (Some(FileLocation::Search), args) => {
                let list = |key: &str| args.and_then(|args| args[key].as_array()).map(|values| values.iter().filter_map(|value| value.as_str().map(str::to_owned)).collect());
                (Some(FileLocation::Search), None, (list("include").unwrap_or_else(folder), list("exclude").unwrap_or_default()))
            }
            _ => (None, None, Default::default()),
        },
        _ => (None, None, Default::default()),
    }
}

/// `addWatchingMatcher`: `background` (or the older `watching`) with both patterns.
fn parse_watching(description: &Value) -> Option<Watching> {
    let watching_pattern = |value: &Value| -> Option<WatchingPattern> {
        let (regexp, file) = match value {
            Value::String(regexp) => (regexp.as_str(), 1),
            Value::Object(_) => (value["regexp"].as_str()?, value["file"].as_u64().unwrap_or(1) as usize),
            _ => return None,
        };
        Some(WatchingPattern { regexp: Arc::new(onig::Regex::new(regexp).ok()?), file })
    };
    if let (Some(begins), Some(ends)) = (description["watchedTaskBeginsRegExp"].as_str(), description["watchedTaskEndsRegExp"].as_str()) {
        let compile = |regexp: &str| onig::Regex::new(regexp).ok().map(|regexp| WatchingPattern { regexp: Arc::new(regexp), file: 1 });
        return Some(Watching { active_on_start: false, begins: compile(begins)?, ends: compile(ends)? });
    }
    let monitor = if description["background"].is_object() { &description["background"] } else { &description["watching"] };
    Some(Watching { active_on_start: monitor["activeOnStart"] == true, begins: watching_pattern(&monitor["beginsPattern"])?, ends: watching_pattern(&monitor["endsPattern"])? })
}

/// `Severity.fromValue` with the problem matcher's single letter and hint/note forms.
fn severity_from_value(value: &str) -> Option<u8> {
    match value.to_lowercase().as_str() {
        "error" | "e" => Some(SEVERITY_ERROR),
        "warning" | "warn" | "w" => Some(SEVERITY_WARNING),
        "info" | "i" | "hint" | "note" => Some(SEVERITY_INFO),
        _ => None,
    }
}

/// `IProblemData`.
#[derive(Clone, Default)]
struct ProblemData {
    file_kind: bool,
    file: Option<String>,
    location: Option<String>,
    line: Option<String>,
    character: Option<String>,
    end_line: Option<String>,
    end_character: Option<String>,
    message: Option<String>,
    severity: Option<String>,
    code: Option<String>,
}

/// A marker the matcher found: its owner, the file and the `IMarkerData`.
#[derive(Clone, Debug, PartialEq)]
pub struct ProblemMatch {
    pub owner: String,
    pub apply_to: ApplyTo,
    pub resource: PathBuf,
    pub marker: Value,
}

/// `ILineMatcher` (`SingleLineMatcher` or `MultiLineMatcher` by the pattern count).
pub struct LineMatcher {
    pub matcher: ProblemMatcher,
    data: Option<ProblemData>,
}

pub struct HandleResult {
    pub found: Option<ProblemMatch>,
    pub continues: bool,
}

impl LineMatcher {
    pub fn new(matcher: ProblemMatcher) -> Self {
        LineMatcher { matcher, data: None }
    }

    pub fn match_length(&self) -> usize {
        self.matcher.patterns.len()
    }

    pub fn handle(&mut self, lines: &[String], start: usize) -> HandleResult {
        let patterns = self.matcher.patterns.clone();
        let mut data = ProblemData { file_kind: patterns[0].file_kind, ..Default::default() };
        let continues = patterns.len() > 1 && patterns[patterns.len() - 1].loop_;
        for (index, pattern) in patterns.iter().enumerate() {
            let Some(captures) = pattern.regexp.captures(&lines[start + index]) else {
                return HandleResult { found: None, continues: false };
            };
            // The looping last pattern fills a copy; the data before it starts each following line.
            if continues && index + 1 == patterns.len() {
                self.data = Some(data.clone());
            }
            fill(&mut data, pattern, &captures);
        }
        if patterns.len() == 1 && patterns[0].location_kind && data.location.is_none() && data.line.is_none() && data.file.is_some() {
            data.file_kind = true;
        }
        HandleResult { found: self.marker_match(&data), continues }
    }

    /// The looping last pattern on the next line; `None` ends the loop.
    pub fn next(&mut self, line: &str) -> Option<ProblemMatch> {
        let pattern = self.matcher.patterns.last()?.clone();
        let Some(captures) = pattern.regexp.captures(line) else {
            self.data = None;
            return None;
        };
        let mut data = self.data.clone()?;
        fill(&mut data, &pattern, &captures);
        self.marker_match(&data)
    }

    fn marker_match(&self, data: &ProblemData) -> Option<ProblemMatch> {
        let (file, message) = (data.file.as_ref()?, data.message.as_ref()?);
        let (start_line, start_column, end_line, end_column) = location(data)?;
        let severity = data.severity.as_deref().and_then(severity_from_value).or(self.matcher.severity).unwrap_or(SEVERITY_ERROR);
        let mut marker = json!({
            "severity": severity,
            "startLineNumber": start_line,
            "startColumn": start_column,
            "endLineNumber": end_line,
            "endColumn": end_column,
            "message": message,
        });
        if let Some(code) = &data.code {
            marker["code"] = json!(code);
        }
        if let Some(source) = &self.matcher.source {
            marker["source"] = json!(source);
        }
        Some(ProblemMatch { owner: self.matcher.owner.clone(), apply_to: self.matcher.apply_to, resource: resource(file, &self.matcher), marker })
    }
}

/// `fillProblemData`: each property set once, except the message, to which a later pattern appends a line.
fn fill(data: &mut ProblemData, pattern: &Pattern, captures: &onig::Captures) {
    let group = |index: Option<usize>, trim: bool| -> Option<String> {
        let value = captures.at(index?)?;
        Some(if trim { value.trim().to_owned() } else { value.to_owned() })
    };
    let set = |slot: &mut Option<String>, value: Option<String>| {
        if slot.is_none() {
            *slot = value;
        }
    };
    set(&mut data.file, group(pattern.file, true));
    match (&mut data.message, group(pattern.message, true)) {
        (Some(message), Some(more)) => {
            message.push('\n');
            message.push_str(&more);
        }
        (message, more) => *message = more,
    }
    set(&mut data.code, group(pattern.code, true));
    set(&mut data.severity, group(pattern.severity, true));
    set(&mut data.location, group(pattern.location, true));
    set(&mut data.line, group(pattern.line, false));
    set(&mut data.character, group(pattern.character, false));
    set(&mut data.end_line, group(pattern.end_line, false));
    set(&mut data.end_character, group(pattern.end_character, false));
}

/// `getLocation` and `createLocation`: a line without a column spans the whole line.
fn location(data: &ProblemData) -> Option<(u64, u64, u64, u64)> {
    if data.file_kind {
        return Some((0, 0, 0, 0));
    }
    let number = |value: &str| value.trim().parse::<u64>().ok();
    let (start_line, start_column, end_line, end_column) = match &data.location {
        Some(location) => {
            let parts: Vec<&str> = location.split(',').collect();
            let start = number(parts[0])?;
            let column = parts.get(1).and_then(|part| number(part));
            match parts.len() > 3 {
                true => (start, column, number(parts[2]), number(parts[3])),
                false => (start, column, None, None),
            }
        }
        None => {
            let line = data.line.as_deref().filter(|line| !line.is_empty())?;
            (number(line)?, data.character.as_deref().and_then(number), data.end_line.as_deref().and_then(number), data.end_character.as_deref().and_then(number))
        }
    };
    Some(match (start_column, end_column) {
        (Some(start), Some(end)) => (start_line, start, end_line.filter(|line| *line != 0).unwrap_or(start_line), end),
        (Some(start), None) => (start_line, start, start_line, start),
        _ => (start_line, 1, start_line, (1u64 << 31) - 1),
    })
}

/// `getResource`: the file against `filePrefix` per `fileLocation`, normalized.
pub fn resource(file: &str, matcher: &ProblemMatcher) -> PathBuf {
    let relative = || matcher.file_prefix.as_ref().map(|prefix| Path::new(prefix).join(file));
    let path = match matcher.file_location {
        FileLocation::Absolute => PathBuf::from(file),
        FileLocation::Relative => relative().unwrap_or_else(|| PathBuf::from(file)),
        FileLocation::AutoDetect => relative().filter(|path| path.exists()).unwrap_or_else(|| PathBuf::from(file)),
        FileLocation::Search => matcher.search_include.iter().find_map(|dir| search(Path::new(dir), file, &matcher.search_exclude)).unwrap_or_else(|| PathBuf::from(file)),
    };
    normalize(&Path::new("/").join(path))
}

/// `searchForFileLocation`: the first file under `dir` whose path ends with `file`, files before subfolders.
fn search(dir: &Path, file: &str, exclude: &[String]) -> Option<PathBuf> {
    if exclude.iter().any(|excluded| Path::new(excluded) == dir) {
        return None;
    }
    let mut subdirs = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => subdirs.push(path),
            Ok(kind) if kind.is_file() && path.to_string_lossy().ends_with(file) => return Some(path),
            _ => {}
        }
    }
    subdirs.into_iter().find_map(|subdir| search(&subdir, file, exclude))
}

fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            other => result.push(other),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tsc() -> Value {
        json!({
            "name": "tsc", "owner": "typescript", "source": "ts", "applyTo": "closedDocuments", "fileLocation": "relative",
            "pattern": "$tsc",
            "background": { "activeOnStart": true, "beginsPattern": "Starting compilation", "endsPattern": "Found \\d+ errors?" }
        })
    }

    fn contributions() -> Value {
        json!({
            "problemPatterns": [{ "name": "tsc", "regexp": "^([^\\s].*)[\\(:](\\d+[,:]\\d+)(?:\\):\\s+|\\s+-\\s+)(error|warning|info)\\s+TS(\\d+)\\s*:\\s*(.*)$", "file": 1, "location": 2, "severity": 3, "code": 4, "message": 5 }],
            "problemMatchers": [tsc()]
        })
    }

    fn run(matcher: &ProblemMatcher, lines: &[&str]) -> Vec<ProblemMatch> {
        let mut line_matcher = LineMatcher::new(matcher.clone());
        let mut found = Vec::new();
        let length = line_matcher.match_length();
        let lines: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
        let mut index = 0;
        while index + length <= lines.len() {
            let result = line_matcher.handle(&lines, index);
            index += length;
            found.extend(result.found);
            if result.continues {
                while index < lines.len() {
                    let Some(next) = line_matcher.next(&lines[index]) else { break };
                    found.push(next);
                    index += 1;
                }
            }
        }
        found
    }

    /// problemMatcher.test.ts: a contributed pattern with a location group, relative to the workspace folder.
    #[test]
    fn a_tsc_error_becomes_a_marker_of_the_file() {
        let contributions = contributions();
        let registry = Registry::new(&[&contributions]);
        let mut matcher = registry.get("tsc").unwrap().clone();
        matcher.file_prefix = Some("/work".into());
        let found = run(&matcher, &["src/a.ts(3,5): error TS2322: Type 'x' is not assignable."]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].resource, PathBuf::from("/work/src/a.ts"));
        assert_eq!(found[0].owner, "typescript");
        assert_eq!(found[0].marker, json!({ "severity": 8, "startLineNumber": 3, "startColumn": 5, "endLineNumber": 3, "endColumn": 5, "message": "Type 'x' is not assignable.", "code": "2322", "source": "ts" }));
        assert!(matcher.watching.as_ref().unwrap().active_on_start);
    }

    #[test]
    fn multi_line_patterns_loop_over_the_file_s_problems() {
        let registry = Registry::new(&[]);
        let matcher = registry.parse(&json!({ "base": "$eslint-stylish", "owner": "lint" })).unwrap();
        let found = run(&matcher, &["/a/b.js", "  1:2  error  Missing semicolon  semi", "  4:1  warning  Unused  no-unused"]);
        assert_eq!(found.len(), 2);
        assert_eq!((found[1].marker["severity"].clone(), found[1].marker["startLineNumber"].clone(), found[1].owner.as_str()), (json!(4), json!(4), "lint"));
        assert_eq!(found[0].marker["code"], "semi");
    }

    #[test]
    fn an_inline_matcher_gets_the_default_groups_and_a_whole_line_range() {
        let registry = Registry::new(&[]);
        let matcher = registry.parse(&json!({ "owner": "x", "fileLocation": ["relative", "/root"], "pattern": { "regexp": "^(.*):(\\d+): (.*)$", "message": 3, "column": 9 } })).unwrap();
        let found = run(&matcher, &["../c.txt:7: bad"]);
        assert_eq!(found[0].resource, PathBuf::from("/c.txt"));
        assert_eq!(found[0].marker["endColumn"], json!((1u64 << 31) - 1));
        assert_eq!(found[0].marker["message"], "bad");
    }
}
