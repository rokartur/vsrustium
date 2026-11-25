//! The problem markers: platform/markers/common/markerService.ts.

use std::sync::Arc;

use serde_json::Value;

/// `MarkerSeverity`.
pub const HINT: u8 = 1;
pub const INFO: u8 = 2;
pub const WARNING: u8 = 4;
pub const ERROR: u8 = 8;

/// `MarkerTag`.
pub const UNNECESSARY: u8 = 1;
pub const DEPRECATED: u8 = 2;

/// `IMarker`: 1-based lines and UTF-16 columns, as the owner reported them.
#[derive(Debug, PartialEq)]
pub struct Marker {
    pub owner: String,
    /// `URI.toString()` of the resource.
    pub resource: String,
    pub severity: u8,
    pub message: String,
    pub source: Option<String>,
    /// A string, or `{ value, target }`.
    pub code: Value,
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub tags: Vec<u8>,
    pub related_information: Value,
    pub model_version_id: Option<u64>,
    pub origin: Option<String>,
}

impl Marker {
    /// `_toMarker`: `None` without a message; positions below 1 and ends before starts are sanitized.
    pub fn new(owner: &str, resource: &str, data: &Value) -> Option<Marker> {
        let message = data["message"].as_str().filter(|message| !message.is_empty())?;
        let number = |key: &str| data[key].as_f64().unwrap_or(0.);
        let start_line = if number("startLineNumber") > 0. { number("startLineNumber") } else { 1. };
        let start_column = if number("startColumn") > 0. { number("startColumn") } else { 1. };
        let end_line = if number("endLineNumber") >= start_line { number("endLineNumber") } else { start_line };
        let end_column = if number("endColumn") > 0. { number("endColumn") } else { start_column };
        // `Number.MAX_VALUE` end columns saturate; `validateRange` clamps them to the line.
        Some(Marker {
            owner: owner.to_string(),
            resource: resource.to_string(),
            severity: data["severity"].as_u64().unwrap_or(0) as u8,
            message: message.to_string(),
            source: data["source"].as_str().map(String::from),
            code: data["code"].clone(),
            start_line: start_line as usize,
            start_column: start_column as usize,
            end_line: end_line as usize,
            end_column: end_column as usize,
            tags: data["tags"].as_array().into_iter().flatten().filter_map(|tag| tag.as_u64()).map(|tag| tag as u8).collect(),
            related_information: data["relatedInformation"].clone(),
            model_version_id: data["modelVersionId"].as_u64(),
            origin: data["origin"].as_str().map(String::from),
        })
    }

    /// The `IMarkerData` of the marker, as `$acceptMarkersChange` sends it.
    pub fn to_data(&self) -> Value {
        let mut data = serde_json::json!({
            "severity": self.severity,
            "message": self.message,
            "startLineNumber": self.start_line,
            "startColumn": self.start_column,
            "endLineNumber": self.end_line,
            "endColumn": self.end_column,
        });
        let optional = [("source", self.source.clone().map(Value::from)), ("code", Some(self.code.clone())), ("relatedInformation", Some(self.related_information.clone())), ("modelVersionId", self.model_version_id.map(Value::from)), ("origin", self.origin.clone().map(Value::from))];
        for (key, value) in optional {
            if let Some(value) = value.filter(|value| !value.is_null()) {
                data[key] = value;
            }
        }
        if !self.tags.is_empty() {
            data["tags"] = self.tags.clone().into();
        }
        data
    }

    pub fn has_tag(&self, tag: u8) -> bool {
        self.tags.contains(&tag)
    }
}

/// `MarkerStatistics`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MarkerStatistics {
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub unknowns: usize,
}

/// `IMarkerReadOptions`.
#[derive(Default)]
pub struct ReadOptions<'a> {
    pub owner: Option<&'a str>,
    pub resource: Option<&'a str>,
    pub severities: Option<u8>,
    pub take: Option<usize>,
}

struct OwnerMarkers {
    owner: String,
    markers: Vec<Arc<Marker>>,
    is_truncated: bool,
}

/// `MarkerService`, by resource then owner, each in insertion order as `DoubleResourceMap` keeps them.
/// No `installResourceFilter`: only chat editing sessions install one, and the port has no chat.
#[derive(Default)]
pub struct MarkerService {
    by_resource: Vec<(String, Vec<OwnerMarkers>)>,
}

impl MarkerService {
    /// `changeOne`; returns the changed resources, what `onMarkerChanged` fires with.
    pub fn change_one(&mut self, owner: &str, resource: &str, data: &[Value], is_truncated: bool) -> Vec<String> {
        if data.is_empty() {
            return match self.delete(resource, owner) {
                true => vec![resource.to_string()],
                false => Vec::new(),
            };
        }
        let markers = data.iter().filter_map(|data| Marker::new(owner, resource, data)).map(Arc::new).collect();
        self.set(resource, OwnerMarkers { owner: owner.to_string(), markers, is_truncated });
        vec![resource.to_string()]
    }

    /// `changeAll`: replaces every marker of `owner` with `data`, `(resource, IMarkerData)` pairs.
    pub fn change_all(&mut self, owner: &str, data: &[(String, Value)]) -> Vec<String> {
        let mut changes = Vec::new();
        for (resource, owners) in &self.by_resource {
            if owners.iter().any(|entry| entry.owner == owner && !entry.markers.is_empty()) {
                changes.push(resource.clone());
            }
        }
        for resource in changes.clone() {
            self.delete(&resource, owner);
        }
        let mut groups: Vec<(String, Vec<Arc<Marker>>)> = Vec::new();
        for (resource, data) in data {
            let Some(marker) = Marker::new(owner, resource, data) else { continue };
            match groups.iter_mut().find(|(key, _)| key == resource) {
                Some((_, markers)) => markers.push(Arc::new(marker)),
                None => {
                    groups.push((resource.clone(), vec![Arc::new(marker)]));
                    changes.push(resource.clone());
                }
            }
        }
        for (resource, markers) in groups {
            self.set(&resource, OwnerMarkers { owner: owner.to_string(), markers, is_truncated: false });
        }
        changes
    }

    pub fn read(&self, options: ReadOptions) -> Vec<Arc<Marker>> {
        let mut result = Vec::new();
        for (resource, owners) in &self.by_resource {
            if options.resource.is_some_and(|wanted| wanted != resource) {
                continue;
            }
            for entry in owners {
                if options.owner.is_some_and(|wanted| wanted != entry.owner) {
                    continue;
                }
                for marker in &entry.markers {
                    if options.take.is_some_and(|take| take > 0 && result.len() == take) {
                        return result;
                    }
                    if options.severities.is_none_or(|severities| severities & marker.severity == marker.severity) {
                        result.push(marker.clone());
                    }
                }
            }
        }
        result
    }

    #[allow(dead_code, reason = "the Problems panel (markersView.ts) reads it; it waits for milestone 11's panel part")]
    pub fn is_source_truncated(&self, resource: &str) -> bool {
        self.owners(resource).iter().any(|entry| entry.is_truncated)
    }

    /// `getStatistics`; markers of `unsupportedSchemas` do not count.
    pub fn statistics(&self) -> MarkerStatistics {
        let mut stats = MarkerStatistics::default();
        for (resource, owners) in &self.by_resource {
            let scheme = resource.split(':').next().unwrap_or_default();
            if UNSUPPORTED_SCHEMES.contains(&scheme) {
                continue;
            }
            for marker in owners.iter().flat_map(|entry| &entry.markers) {
                match marker.severity {
                    ERROR => stats.errors += 1,
                    WARNING => stats.warnings += 1,
                    INFO => stats.infos += 1,
                    _ => stats.unknowns += 1,
                }
            }
        }
        stats
    }

    fn owners(&self, resource: &str) -> &[OwnerMarkers] {
        self.by_resource.iter().find(|(key, _)| key == resource).map(|(_, owners)| owners.as_slice()).unwrap_or_default()
    }

    fn set(&mut self, resource: &str, markers: OwnerMarkers) {
        let index = match self.by_resource.iter().position(|(key, _)| key == resource) {
            Some(index) => index,
            None => {
                self.by_resource.push((resource.to_string(), Vec::new()));
                self.by_resource.len() - 1
            }
        };
        let owners = &mut self.by_resource[index].1;
        match owners.iter_mut().find(|entry| entry.owner == markers.owner) {
            Some(entry) => *entry = markers,
            None => owners.push(markers),
        }
    }

    /// Removes the markers of `owner` on `resource`, and the resource once no owner is left.
    fn delete(&mut self, resource: &str, owner: &str) -> bool {
        let Some(index) = self.by_resource.iter().position(|(key, _)| key == resource) else { return false };
        let owners = &mut self.by_resource[index].1;
        let Some(position) = owners.iter().position(|entry| entry.owner == owner) else { return false };
        owners.remove(position);
        if owners.is_empty() {
            self.by_resource.remove(index);
        }
        true
    }
}

/// markerService.ts `unsupportedSchemas`.
const UNSUPPORTED_SCHEMES: [&str; 6] = ["inmemory", "vscode-scm", "walkThrough", "walkThroughSnippet", "vscode-chat-code-block", "vscode-copilot-chat-code-block"];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn random_marker_data(severity: u8) -> Value {
        json!({ "severity": severity, "message": format!("{:x}", rand_message()), "startLineNumber": 1, "startColumn": 1, "endLineNumber": 1, "endColumn": 1 })
    }

    fn rand_message() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    fn data() -> Value {
        random_marker_data(ERROR)
    }

    fn count(service: &MarkerService, options: ReadOptions) -> usize {
        service.read(options).len()
    }

    fn owner(owner: &str) -> ReadOptions<'_> {
        ReadOptions { owner: Some(owner), ..Default::default() }
    }

    fn resource(resource: &str) -> ReadOptions<'_> {
        ReadOptions { resource: Some(resource), ..Default::default() }
    }

    fn severities(severities: u8) -> ReadOptions<'static> {
        ReadOptions { severities: Some(severities), ..Default::default() }
    }

    #[test]
    fn query() {
        let mut service = MarkerService::default();
        let file = "file:///c/test/file.cs";
        service.change_all("far", &[(file.into(), random_marker_data(ERROR))]);
        assert_eq!(count(&service, ReadOptions::default()), 1);
        assert_eq!(count(&service, owner("far")), 1);
        assert_eq!(count(&service, resource(file)), 1);
        assert_eq!(count(&service, ReadOptions { owner: Some("far"), resource: Some(file), ..Default::default() }), 1);

        service.change_all("boo", &[(file.into(), random_marker_data(WARNING))]);
        assert_eq!(count(&service, ReadOptions::default()), 2);
        assert_eq!(count(&service, owner("far")), 1);
        assert_eq!(count(&service, owner("boo")), 1);
        assert_eq!(count(&service, severities(ERROR)), 1);
        assert_eq!(count(&service, severities(WARNING)), 1);
        assert_eq!(count(&service, severities(HINT)), 0);
        assert_eq!(count(&service, severities(ERROR | WARNING)), 2);
    }

    #[test]
    fn change_one_override() {
        let mut service = MarkerService::default();
        let path = "file:///path/only.cs";
        service.change_one("far", path, &[data()], false);
        assert_eq!(count(&service, ReadOptions::default()), 1);
        assert_eq!(count(&service, owner("far")), 1);

        service.change_one("boo", path, &[data()], false);
        assert_eq!(count(&service, ReadOptions::default()), 2);
        assert_eq!(count(&service, owner("far")), 1);
        assert_eq!(count(&service, owner("boo")), 1);

        service.change_one("far", path, &[data(), data()], false);
        assert_eq!(count(&service, owner("far")), 2);
        assert_eq!(count(&service, owner("boo")), 1);
    }

    #[test]
    fn change_one_and_all_clear() {
        let mut service = MarkerService::default();
        let path = "file:///path/only.cs";
        service.change_one("far", path, &[data()], false);
        service.change_one("boo", path, &[data()], false);
        assert_eq!(count(&service, ReadOptions::default()), 2);

        service.change_one("far", path, &[], false);
        assert_eq!(count(&service, owner("far")), 0);
        assert_eq!(count(&service, owner("boo")), 1);
        assert_eq!(count(&service, ReadOptions::default()), 1);

        service.change_all("boo", &[]);
        assert_eq!(count(&service, owner("boo")), 0);
        assert_eq!(count(&service, ReadOptions::default()), 0);
    }

    #[test]
    fn change_one_and_all_prune_empty_indexes_and_preserve_remaining_associations() {
        for change_one in [true, false] {
            let mut service = MarkerService::default();
            let (shared, other) = ("file:///path/shared.cs", "file:///path/other.cs");
            service.change_one("first", shared, &[data()], false);
            service.change_one("first", other, &[data()], false);
            service.change_one("second", shared, &[data()], false);
            let clear = |service: &mut MarkerService, owner: &str, resource: &str| match change_one {
                true => drop(service.change_one(owner, resource, &[], false)),
                false => drop(service.change_all(owner, &[])),
            };
            let snapshot = |service: &MarkerService| {
                let mut markers: Vec<String> = service.read(ReadOptions::default()).iter().map(|marker| format!("{}:{}", marker.owner, marker.resource)).collect();
                markers.sort();
                (service.by_resource.len(), markers)
            };
            clear(&mut service, "first", other);
            let after_first_clear = snapshot(&service);
            clear(&mut service, "first", shared);
            let after_owner_clear = snapshot(&service);
            clear(&mut service, "second", shared);
            let expected_first = match change_one {
                true => vec!["first:file:///path/shared.cs".to_string(), "second:file:///path/shared.cs".to_string()],
                false => vec!["second:file:///path/shared.cs".to_string()],
            };
            assert_eq!(after_first_clear, (1, expected_first));
            assert_eq!(after_owner_clear, (1, vec!["second:file:///path/shared.cs".to_string()]));
            assert_eq!(snapshot(&service), (0, Vec::new()));
        }
    }

    #[test]
    fn change_all_sends_event_for_cleared() {
        let mut service = MarkerService::default();
        service.change_all("far", &[("file:///d/path".into(), data()), ("file:///d/path".into(), data())]);
        assert_eq!(count(&service, owner("far")), 2);
        assert_eq!(service.change_all("far", &[]), ["file:///d/path"]);
        assert_eq!(count(&service, owner("far")), 0);
    }

    #[test]
    fn change_all_merges() {
        let mut service = MarkerService::default();
        service.change_all("far", &[("file:///c/test/file.cs".into(), data()), ("file:///c/test/file.cs".into(), data())]);
        assert_eq!(count(&service, owner("far")), 2);
    }

    #[test]
    fn change_all_must_not_break_integrity_issue_12635() {
        let mut service = MarkerService::default();
        service.change_all("far", &[("scheme:path1".into(), data()), ("scheme:path2".into(), data())]);
        service.change_all("boo", &[("scheme:path1".into(), data())]);
        service.change_all("far", &[("scheme:path1".into(), data()), ("scheme:path2".into(), data())]);
        assert_eq!(count(&service, owner("far")), 2);
        assert_eq!(count(&service, resource("scheme:path1")), 2);
    }

    #[test]
    fn invalid_marker_data() {
        let mut service = MarkerService::default();
        let mut marker = data();
        marker["message"] = Value::Null;
        service.change_one("far", "some:uri/path", &[marker.clone()], false);
        assert_eq!(count(&service, owner("far")), 0);
        marker["message"] = json!("null");
        service.change_one("far", "some:uri/path", &[marker], false);
        assert_eq!(count(&service, owner("far")), 1);
    }

    #[test]
    fn map_map_remove_returns_bad_values_issue_13548() {
        let mut service = MarkerService::default();
        service.change_one("o", "some:uri/1", &[data()], false);
        assert!(service.change_one("o", "some:uri/2", &[], false).is_empty());
    }

    #[test]
    fn error_code_of_zero_in_markers_get_removed_31275() {
        let mut service = MarkerService::default();
        let marker = json!({ "code": "0", "startLineNumber": 1, "startColumn": 2, "endLineNumber": 1, "endColumn": 5, "message": "test", "severity": 0, "source": "me" });
        service.change_one("far", "some:thing", &[marker], false);
        let markers = service.read(resource("some:thing"));
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].code, "0");
    }

    #[test]
    fn model_version_id_is_preserved_on_marker_when_present_in_marker_data() {
        let mut service = MarkerService::default();
        let file = "file:///path/file.ts";
        let mut with_version = data();
        with_version["modelVersionId"] = json!(42);
        service.change_one("owner", file, &[with_version], false);
        assert_eq!(service.read(resource(file))[0].model_version_id, Some(42));
        service.change_one("owner", file, &[data()], false);
        assert_eq!(service.read(resource(file))[0].model_version_id, None);
    }

    #[test]
    fn tracks_diagnostic_truncation_per_resource_across_owners() {
        let mut service = MarkerService::default();
        let (file, other) = ("file:///path/file.ts", "file:///path/other.ts");
        service.change_one("first", file, &[data()], true);
        service.change_one("second", file, &[data()], false);
        service.change_one("first", file, &[data()], false);
        service.change_one("first", other, &[data()], true);
        let after_change_all = [service.is_source_truncated(file), service.is_source_truncated(other)];
        service.change_all("first", &[]);
        assert_eq!(after_change_all, [false, true]);
        assert_eq!([service.is_source_truncated(file), service.is_source_truncated(other)], [false, false]);
    }

    #[test]
    fn sanitizes_positions_and_counts_statistics() {
        let mut service = MarkerService::default();
        let marker = json!({ "severity": WARNING, "message": "m", "startLineNumber": 0, "startColumn": -3, "endLineNumber": 0, "endColumn": 0 });
        service.change_one("o", "file:///a", &[marker, random_marker_data(ERROR), random_marker_data(INFO)], false);
        service.change_one("o", "inmemory://model/1", &[data()], false);
        let read = service.read(resource("file:///a"));
        assert_eq!((read[0].start_line, read[0].start_column, read[0].end_line, read[0].end_column), (1, 1, 1, 1));
        assert_eq!(service.statistics(), MarkerStatistics { errors: 1, warnings: 1, infos: 1, unknowns: 0 });
    }
}
