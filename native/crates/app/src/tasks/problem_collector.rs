//! problemCollectors.ts: feeds task output lines to the line matchers and reports markers per owner and file,
//! cleaning the files of an owner the run no longer reports (`StartStopProblemCollector`), and for background
//! tasks per cycle between the begins and ends patterns (`WatchingProblemCollector`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use serde_json::Value;

use super::problem_matcher::{ApplyTo, LineMatcher, ProblemMatch, ProblemMatcher};

/// What the collector asks the marker service to do.
#[derive(Debug, PartialEq)]
pub enum MarkerChange {
    /// `changeOne(owner, resource, markers)`.
    Set { owner: String, resource: PathBuf, markers: Vec<Value> },
    /// `remove(owner, resources)`.
    Remove { owner: String, resources: Vec<PathBuf> },
}

/// `ProblemCollectorEventKind` of a background task.
#[derive(Debug, PartialEq)]
pub enum CollectorEvent {
    BackgroundProcessingBegins,
    BackgroundProcessingEnds,
}

/// What the marker service and the editors know, for cleaning and `applyTo`.
pub trait Markers {
    /// The files that have markers of `owner`.
    fn resources_of(&self, owner: &str) -> Vec<PathBuf>;
    fn is_open(&self, resource: &PathBuf) -> bool;
}

pub struct ProblemCollector {
    matchers: Vec<LineMatcher>,
    active: Option<usize>,
    buffer: Vec<String>,
    buffer_length: usize,
    apply_to_by_owner: HashMap<String, ApplyTo>,
    resources_to_clean: BTreeMap<String, BTreeSet<PathBuf>>,
    /// Markers by owner and file, keyed as `makeKeyOptionalMessage` without the message.
    markers: BTreeMap<String, BTreeMap<PathBuf, Vec<(String, Value)>>>,
    delivered: HashMap<String, HashMap<PathBuf, usize>>,
    current: Option<(String, PathBuf)>,
    pub number_of_matches: usize,
    pub max_severity: Option<u8>,
    /// Background matchers (by index) between their begins and ends patterns.
    active_background: BTreeSet<usize>,
    has_started: bool,
    /// Output not ended by a line break yet.
    partial_line: String,
}

impl ProblemCollector {
    /// The `StartStopProblemCollector` constructor records the files of every owner to clean; a watching collector
    /// does that in `about_to_start` and at each begin.
    pub fn new(problem_matchers: Vec<ProblemMatcher>, markers: &dyn Markers) -> Self {
        let mut apply_to_by_owner: HashMap<String, ApplyTo> = HashMap::new();
        for matcher in &problem_matchers {
            apply_to_by_owner
                .entry(matcher.owner.clone())
                .and_modify(|current| {
                    if *current != matcher.apply_to {
                        *current = ApplyTo::AllDocuments;
                    }
                })
                .or_insert(matcher.apply_to);
        }
        let buffer_length = problem_matchers.iter().map(|matcher| matcher.patterns.len()).max().unwrap_or(1);
        let mut collector = ProblemCollector {
            matchers: problem_matchers.into_iter().map(LineMatcher::new).collect(),
            active: None,
            buffer: Vec::new(),
            buffer_length,
            apply_to_by_owner,
            resources_to_clean: BTreeMap::new(),
            markers: BTreeMap::new(),
            delivered: HashMap::new(),
            current: None,
            number_of_matches: 0,
            max_severity: None,
            active_background: BTreeSet::new(),
            has_started: false,
            partial_line: String::new(),
        };
        if !collector.is_watching() {
            let owners: Vec<String> = collector.apply_to_by_owner.keys().cloned().collect();
            for owner in owners {
                collector.record_resources_to_clean(&owner, markers);
            }
        }
        collector
    }

    pub fn is_watching(&self) -> bool {
        self.matchers.iter().any(|matcher| matcher.matcher.watching.is_some())
    }

    /// `aboutToStart`: background matchers active on start begin at once.
    pub fn about_to_start(&mut self, markers: &dyn Markers) -> Vec<CollectorEvent> {
        let mut events = Vec::new();
        for index in 0..self.matchers.len() {
            let matcher = &self.matchers[index].matcher;
            if matcher.watching.as_ref().is_some_and(|watching| watching.active_on_start) {
                let owner = matcher.owner.clone();
                self.active_background.insert(index);
                events.push(CollectorEvent::BackgroundProcessingBegins);
                self.record_resources_to_clean(&owner, markers);
            }
        }
        events
    }

    /// Splits terminal output into lines (`LineDecoder` over the data with escapes removed) and processes each.
    pub fn process_output(&mut self, data: &str, markers: &dyn Markers, changes: &mut Vec<MarkerChange>) -> Vec<CollectorEvent> {
        let text = strip_ansi(data);
        self.partial_line.push_str(&text);
        let mut events = Vec::new();
        while let Some(index) = self.partial_line.find(['\n', '\r']) {
            let line: String = self.partial_line[..index].to_owned();
            let mut rest = &self.partial_line[index + 1..];
            if self.partial_line.as_bytes()[index] == b'\r' && rest.starts_with('\n') {
                rest = &rest[1..];
            }
            self.partial_line = rest.to_owned();
            events.extend(self.process_line(&line, markers, changes));
        }
        events
    }

    pub fn process_line(&mut self, line: &str, markers: &dyn Markers, changes: &mut Vec<MarkerChange>) -> Vec<CollectorEvent> {
        let mut events = Vec::new();
        if self.is_watching() {
            if self.try_begin(line, markers, changes, &mut events) || self.try_finish(line, markers, changes, &mut events) {
                return events;
            }
        } else if !self.has_started {
            self.has_started = true;
            events.push(CollectorEvent::BackgroundProcessingBegins);
        }
        let Some(found) = self.try_find_marker(line) else { return events };
        self.remove_resource_to_clean(&found.owner, &found.resource);
        if !self.should_apply(&found, markers) {
            return events;
        }
        self.record_marker(&found);
        let key = (found.owner.clone(), found.resource.clone());
        if self.current.as_ref() != Some(&key) {
            if let Some((owner, resource)) = self.current.take() {
                self.deliver(&owner, &resource, changes);
            }
            self.current = Some(key);
        }
        events
    }

    /// `done`: the last partial line, then every marker, then the files of the previous run the run did not report.
    pub fn done(&mut self, markers: &dyn Markers, changes: &mut Vec<MarkerChange>) {
        if !self.partial_line.is_empty() {
            let line = std::mem::take(&mut self.partial_line);
            self.process_line(&line, markers, changes);
        }
        if self.is_watching() {
            let owners: Vec<String> = self.apply_to_by_owner.keys().cloned().collect();
            for owner in owners {
                self.record_resources_to_clean(&owner, markers);
            }
        }
        let owners: Vec<(String, Vec<PathBuf>)> = self.markers.iter().map(|(owner, files)| (owner.clone(), files.keys().cloned().collect())).collect();
        for (owner, files) in owners {
            for file in files {
                self.deliver(&owner, &file, changes);
            }
        }
        let to_clean = std::mem::take(&mut self.resources_to_clean);
        for (owner, resources) in to_clean {
            self.clean(&owner, resources, markers, changes);
        }
    }

    fn try_find_marker(&mut self, line: &str) -> Option<ProblemMatch> {
        if let Some(active) = self.active {
            if let Some(found) = self.matchers[active].next(line) {
                self.capture(&found);
                return Some(found);
            }
            self.buffer.clear();
            self.active = None;
        }
        if self.buffer.len() >= self.buffer_length {
            self.buffer.remove(0);
        }
        self.buffer.push(line.to_owned());
        let found = self.try_matchers();
        if found.is_some() {
            self.buffer.clear();
        }
        found
    }

    /// `tryMatchers`: from the oldest buffered line, matchers whose pattern count fits the lines left.
    fn try_matchers(&mut self) -> Option<ProblemMatch> {
        self.active = None;
        let length = self.buffer.len();
        for start in 0..length {
            for index in 0..self.matchers.len() {
                if self.matchers[index].match_length() != length - start {
                    continue;
                }
                let result = self.matchers[index].handle(&self.buffer, start);
                if let Some(found) = result.found {
                    self.capture(&found);
                    if result.continues {
                        self.active = Some(index);
                    }
                    return Some(found);
                }
            }
        }
        None
    }

    fn capture(&mut self, found: &ProblemMatch) {
        self.number_of_matches += 1;
        let severity = found.marker["severity"].as_u64().unwrap_or(0) as u8;
        if self.max_severity.is_none_or(|max| severity > max) {
            self.max_severity = Some(severity);
        }
    }

    fn should_apply(&self, found: &ProblemMatch, markers: &dyn Markers) -> bool {
        match found.apply_to {
            ApplyTo::AllDocuments => true,
            ApplyTo::OpenDocuments => markers.is_open(&found.resource),
            ApplyTo::ClosedDocuments => !markers.is_open(&found.resource),
        }
    }

    fn record_marker(&mut self, found: &ProblemMatch) {
        let marker = &found.marker;
        let key = format!("{}¦{}¦{}¦{}¦{}¦{}¦{}", marker["source"], marker["code"], marker["severity"], marker["startLineNumber"], marker["startColumn"], marker["endLineNumber"], marker["endColumn"]);
        let list = self.markers.entry(found.owner.clone()).or_default().entry(found.resource.clone()).or_default();
        if !list.iter().any(|(known, _)| *known == key) {
            list.push((key, marker.clone()));
        }
    }

    fn deliver(&mut self, owner: &str, resource: &PathBuf, changes: &mut Vec<MarkerChange>) {
        let Some(list) = self.markers.get(owner).and_then(|files| files.get(resource)) else { return };
        let delivered = self.delivered.entry(owner.to_owned()).or_default();
        if delivered.get(resource) != Some(&list.len()) {
            delivered.insert(resource.clone(), list.len());
            changes.push(MarkerChange::Set { owner: owner.to_owned(), resource: resource.clone(), markers: list.iter().map(|(_, marker)| marker.clone()).collect() });
        }
    }

    fn record_resources_to_clean(&mut self, owner: &str, markers: &dyn Markers) {
        self.resources_to_clean.entry(owner.to_owned()).or_default().extend(markers.resources_of(owner));
    }

    fn remove_resource_to_clean(&mut self, owner: &str, resource: &PathBuf) {
        if let Some(set) = self.resources_to_clean.get_mut(owner) {
            set.remove(resource);
        }
    }

    fn clean(&self, owner: &str, resources: BTreeSet<PathBuf>, markers: &dyn Markers, changes: &mut Vec<MarkerChange>) {
        let apply_to = self.apply_to_by_owner.get(owner).copied().unwrap_or(ApplyTo::AllDocuments);
        let resources: Vec<PathBuf> = resources
            .into_iter()
            .filter(|resource| match apply_to {
                ApplyTo::AllDocuments => true,
                ApplyTo::OpenDocuments => markers.is_open(resource),
                ApplyTo::ClosedDocuments => !markers.is_open(resource),
            })
            .collect();
        if !resources.is_empty() {
            changes.push(MarkerChange::Remove { owner: owner.to_owned(), resources });
        }
    }

    fn clean_marker_caches(&mut self) {
        self.number_of_matches = 0;
        self.max_severity = None;
        self.markers.clear();
        self.delivered.clear();
    }

    fn reset_current_resource(&mut self, changes: &mut Vec<MarkerChange>) {
        if let Some((owner, resource)) = self.current.take() {
            self.deliver(&owner, &resource, changes);
        }
    }

    fn try_begin(&mut self, line: &str, markers: &dyn Markers, changes: &mut Vec<MarkerChange>, events: &mut Vec<CollectorEvent>) -> bool {
        let mut result = false;
        for index in 0..self.matchers.len() {
            let matcher = self.matchers[index].matcher.clone();
            let Some(watching) = &matcher.watching else { continue };
            let Some(captures) = watching.begins.regexp.captures(line) else { continue };
            if !self.active_background.insert(index) {
                continue;
            }
            result = true;
            events.push(CollectorEvent::BackgroundProcessingBegins);
            self.clean_marker_caches();
            self.reset_current_resource(changes);
            match captures.at(watching.begins.file) {
                Some(file) if watching.begins.file > 0 => {
                    let resource = super::problem_matcher::resource(file, &matcher);
                    self.resources_to_clean.entry(matcher.owner.clone()).or_default().insert(resource);
                }
                _ => self.record_resources_to_clean(&matcher.owner, markers),
            }
        }
        result
    }

    fn try_finish(&mut self, line: &str, markers: &dyn Markers, changes: &mut Vec<MarkerChange>, events: &mut Vec<CollectorEvent>) -> bool {
        let mut result = false;
        for index in 0..self.matchers.len() {
            let Some(watching) = &self.matchers[index].matcher.watching else { continue };
            if watching.ends.regexp.find(line).is_none() {
                continue;
            }
            let owner = self.matchers[index].matcher.owner.clone();
            if self.active_background.contains(&index)
                && let Some((owner, resource)) = self.current.clone()
            {
                self.deliver(&owner, &resource, changes);
            }
            if self.active_background.remove(&index) {
                self.reset_current_resource(changes);
                events.push(CollectorEvent::BackgroundProcessingEnds);
                result = true;
                if let Some(resources) = self.resources_to_clean.remove(&owner) {
                    self.clean(&owner, resources, markers, changes);
                }
                self.clean_marker_caches();
            }
        }
        result
    }
}

/// `removeAnsiEscapeCodes`: CSI and OSC sequences and other escapes out of the output.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(char) = chars.next() {
        if char != '\x1b' {
            out.push(char);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(next) = chars.next() {
                    if next == '\x07' || (next == '\x1b' && chars.peek() == Some(&'\\')) {
                        if next == '\x1b' {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::problem_matcher::Registry;
    use super::*;

    struct Service {
        existing: Vec<PathBuf>,
    }

    impl Markers for Service {
        fn resources_of(&self, _: &str) -> Vec<PathBuf> {
            self.existing.clone()
        }
        fn is_open(&self, _: &PathBuf) -> bool {
            false
        }
    }

    fn matcher(registry: &Registry, background: bool) -> ProblemMatcher {
        let mut description = json!({ "owner": "go", "fileLocation": "absolute", "pattern": { "regexp": "^(.*):(\\d+): (.*)$", "message": 3 } });
        if background {
            description["background"] = json!({ "beginsPattern": "^start$", "endsPattern": "^end$" });
        }
        registry.parse(&description).unwrap()
    }

    #[test]
    fn a_run_reports_its_files_and_cleans_the_others() {
        let registry = Registry::new(&[]);
        let service = Service { existing: vec![PathBuf::from("/old.go"), PathBuf::from("/a.go")] };
        let mut collector = ProblemCollector::new(vec![matcher(&registry, false)], &service);
        let mut changes = Vec::new();
        collector.process_output("\x1b[31m/a.go:1: x\x1b[0m\r\n/a.go:2: y\n/b.go:3: z", &service, &mut changes);
        assert!(changes.is_empty(), "the file is delivered once another file starts, and the last line has no break yet");
        collector.done(&service, &mut changes);
        let MarkerChange::Set { resource, markers, .. } = &changes[0] else { panic!() };
        assert_eq!((resource.to_str().unwrap(), markers.len()), ("/a.go", 2));
        assert_eq!(changes[1], MarkerChange::Set { owner: "go".into(), resource: "/b.go".into(), markers: vec![json!({ "severity": 8, "startLineNumber": 3, "startColumn": 1, "endLineNumber": 3, "endColumn": (1u64 << 31) - 1, "message": "z" })] });
        assert_eq!(changes[2], MarkerChange::Remove { owner: "go".into(), resources: vec!["/old.go".into()] });
        assert_eq!(collector.number_of_matches, 3);
    }

    #[test]
    fn a_background_cycle_begins_and_ends_with_its_patterns() {
        let registry = Registry::new(&[]);
        let service = Service { existing: vec![PathBuf::from("/old.go")] };
        let mut collector = ProblemCollector::new(vec![matcher(&registry, true)], &service);
        let mut changes = Vec::new();
        let events = collector.process_output("start\n/a.go:1: x\nend\n", &service, &mut changes);
        assert_eq!(events, [CollectorEvent::BackgroundProcessingBegins, CollectorEvent::BackgroundProcessingEnds]);
        assert!(matches!(&changes[0], MarkerChange::Set { resource, .. } if resource.to_str() == Some("/a.go")));
        assert_eq!(changes[1], MarkerChange::Remove { owner: "go".into(), resources: vec!["/old.go".into()] });
    }
}
