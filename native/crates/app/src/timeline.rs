//! The timeline model: timelineService.ts providers and timelinePane.ts `TimelineAggregate` per source, with the
//! pane's target (a file or folder, or the workspace), its request bookkeeping (`loadTimelineForSource`,
//! `handleRequest`) and its merged, paged rows (`getItems`).

use std::collections::{HashMap, HashSet};

use serde_json::Value;

/// localHistoryTimeline.ts `LocalHistoryTimeline.id`, the source the empty Local History message names.
pub const LOCAL_HISTORY_SOURCE: &str = "timeline.localHistory";

/// timeline.ts `TimelineProviderDescriptor`; a `WorkspaceTimelineProviderDescriptor` has no schemes.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineProvider {
    pub id: String,
    pub label: String,
    pub schemes: Vec<String>,
}

impl TimelineProvider {
    /// The `$registerTimelineProvider` descriptor `{ id, label, scheme }`, or `$registerWorkspaceTimelineProvider`'s
    /// `{ id, label }`.
    pub fn from_descriptor(descriptor: &Value) -> Option<TimelineProvider> {
        let id = descriptor["id"].as_str()?.to_string();
        let label = descriptor["label"].as_str().unwrap_or(&id).to_string();
        let schemes = match &descriptor["scheme"] {
            Value::String(scheme) => vec![scheme.clone()],
            Value::Array(schemes) => schemes.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            _ => Vec::new(),
        };
        Some(TimelineProvider { id, label, schemes })
    }

    /// timelineService.ts `getTimeline`: a provider answers for its schemes only, `*` for every scheme.
    fn handles(&self, scheme: &str) -> bool {
        self.schemes.iter().any(|own| own == "*" || own == scheme)
    }
}

/// timeline.ts `TimelineItem` as `ExtHostTimeline._convertItem` sends it.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineItem {
    pub handle: String,
    /// The provider the item came from, set by timelineService.ts `getTimeline`.
    pub source: String,
    pub id: Option<String>,
    pub label: String,
    /// The provider gave no label, so `label` is the resource's base name (`getLabelResource`).
    pub label_from_resource: bool,
    pub description: Option<String>,
    pub tooltip: Option<String>,
    pub timestamp: i64,
    pub theme_icon: Option<(String, Option<String>)>,
    pub command: Option<Value>,
    pub context_value: Option<String>,
    /// The resource's content as of this item, which `compareSelected` diffs.
    pub resource_uri: Option<Value>,
}

impl TimelineItem {
    fn from_value(item: &Value) -> Option<TimelineItem> {
        let string = |value: &Value| value.as_str().map(str::to_string);
        let label = string(&item["label"]).filter(|label| !label.is_empty());
        let resource_uri = item["resourceUri"].is_object().then(|| item["resourceUri"].clone());
        Some(TimelineItem {
            handle: string(&item["handle"])?,
            source: string(&item["source"]).unwrap_or_default(),
            id: string(&item["id"]),
            label_from_resource: label.is_none() && resource_uri.is_some(),
            label: label.or_else(|| resource_uri.as_ref().map(uri_basename)).unwrap_or_default(),
            description: string(&item["description"]),
            tooltip: string(&item["tooltip"]).or_else(|| string(&item["tooltip"]["value"])),
            timestamp: item["timestamp"].as_f64()? as i64,
            theme_icon: string(&item["themeIcon"]["id"]).map(|id| (id, string(&item["themeIcon"]["color"]["id"]))),
            command: item["command"]["id"].is_string().then(|| item["command"].clone()),
            context_value: string(&item["contextValue"]),
            resource_uri,
        })
    }
}

/// timeline.ts `Timeline`: one source's answer, newest first as timelineService.ts `getTimeline` sorts it.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineResult {
    pub items: Vec<TimelineItem>,
    pub cursor: Option<String>,
}

impl TimelineResult {
    /// The `$getTimeline` answer `{ source, items, cursor }` (`paging.cursor` in older protocols); `None` for undefined.
    pub fn from_value(timeline: &Value) -> Option<TimelineResult> {
        let mut items: Vec<TimelineItem> = timeline["items"].as_array()?.iter().filter_map(TimelineItem::from_value).collect();
        items.sort_by_key(|item| std::cmp::Reverse(item.timestamp));
        let cursor = timeline["cursor"].as_str().or(timeline["paging"]["cursor"].as_str()).map(str::to_string);
        Some(TimelineResult { items, cursor })
    }
}

/// timelinePane.ts `TimelineAggregate`: the items of one source loaded so far.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineAggregate {
    pub source: String,
    pub items: Vec<TimelineItem>,
    pub cursor: Option<String>,
    pub stale: bool,
}

impl TimelineAggregate {
    fn more(&self) -> bool {
        self.cursor.is_some()
    }

    /// `TimelineAggregate.add`: drops the items the page repeats, then keeps them newest first.
    fn add(&mut self, timeline: TimelineResult) -> bool {
        let mut updated = false;
        if !timeline.items.is_empty() && !self.items.is_empty() {
            updated = true;
            let ids: HashSet<&str> = timeline.items.iter().filter_map(|item| item.id.as_deref()).collect();
            let timestamps: HashSet<i64> = timeline.items.iter().filter(|item| item.id.is_none()).map(|item| item.timestamp).collect();
            self.items.retain(|item| !(item.id.as_deref().is_some_and(|id| ids.contains(id)) || timestamps.contains(&item.timestamp)));
            let newest = self.items.first().map_or(0, |item| item.timestamp);
            if timeline.items.last().map_or(0, |item| item.timestamp) >= newest {
                self.items.splice(0..0, timeline.items);
            } else {
                self.items.extend(timeline.items);
            }
        } else if !timeline.items.is_empty() {
            updated = true;
            self.items.extend(timeline.items);
        }
        self.cursor = timeline.cursor;
        if updated {
            self.items.sort_by_key(|item| std::cmp::Reverse(item.timestamp));
        }
        updated
    }
}

/// One `$getTimeline` in flight (`TimelineRequest`): its options, the resource it asks about (`None` for the
/// workspace timeline), and an id so a replaced one is ignored.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineRequest {
    pub id: u64,
    pub source: String,
    pub uri: Option<Value>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
    pub reset: bool,
}

/// A row of the pane's tree: an item with its relative time, or the `LoadMoreCommand`.
#[derive(Debug, PartialEq)]
pub enum Row<'a> {
    /// `duplicate` is `hideRelativeTime`: the row above shows the same relative time.
    Item { item: &'a TimelineItem, relative_time: String, duplicate: bool },
    LoadMore { loading: bool },
}

/// The registered providers and the timelines of the shown target.
#[derive(Default)]
pub struct Timeline {
    pub providers: Vec<TimelineProvider>,
    /// timelineService.ts `workspaceProviders`.
    pub workspace_providers: Vec<TimelineProvider>,
    /// The file or folder whose timeline is shown; `None` with `workspace` for the workspace timeline.
    pub uri: Option<Value>,
    /// timelinePane.ts `workspace`: the workspace timeline is shown.
    pub workspace: bool,
    /// `timelinesBySource`, in insertion order as the pane's Map iterates it.
    pub timelines: Vec<TimelineAggregate>,
    pub pending: HashMap<String, TimelineRequest>,
    /// `_maxItemCount`.
    pub max_item_count: usize,
    next_request: u64,
}

impl Timeline {
    /// timelineService.ts `registerTimelineProvider`: a provider with the same id is replaced.
    pub fn register(&mut self, provider: TimelineProvider) {
        self.providers.retain(|other| other.id != provider.id);
        self.providers.push(provider);
    }

    /// timelineService.ts `registerWorkspaceTimelineProvider`: a provider with the same id is replaced.
    pub fn register_workspace(&mut self, provider: TimelineProvider) {
        self.workspace_providers.retain(|other| other.id != provider.id);
        self.workspace_providers.push(provider);
    }

    /// `unregisterTimelineProvider` and the pane's `onProvidersChanged` removal: its request and items go too.
    /// Returns whether a workspace provider keeps the source (`fireProviderRemoved` adds it back).
    pub fn unregister(&mut self, id: &str) -> bool {
        self.providers.retain(|provider| provider.id != id);
        self.remove_source(id)
    }

    /// `unregisterWorkspaceTimelineProvider`; returns whether a file provider keeps the source.
    pub fn unregister_workspace(&mut self, id: &str) -> bool {
        self.workspace_providers.retain(|provider| provider.id != id);
        self.remove_source(id)
    }

    fn remove_source(&mut self, id: &str) -> bool {
        self.pending.remove(id);
        self.timelines.retain(|timeline| timeline.source != id);
        self.sources().iter().any(|source| source.id == id)
    }

    /// timelineService.ts `getSources`: file then workspace providers, the first of an id naming it.
    pub fn sources(&self) -> Vec<&TimelineProvider> {
        let mut sources: Vec<&TimelineProvider> = Vec::new();
        for provider in self.providers.iter().chain(&self.workspace_providers) {
            if sources.iter().all(|source| source.id != provider.id) {
                sources.push(provider);
            }
        }
        sources
    }

    /// `hasTarget`.
    pub fn has_target(&self) -> bool {
        self.uri.is_some() || self.workspace
    }

    /// `getTargetLabel`: "Workspace", the resource's base name, or empty without a target.
    pub fn target_label(&self) -> String {
        match &self.uri {
            Some(uri) => uri_basename(uri),
            None if self.workspace => "Workspace".into(),
            None => String::new(),
        }
    }

    pub fn timeline(&self, source: &str) -> Option<&TimelineAggregate> {
        self.timelines.iter().find(|timeline| timeline.source == source)
    }

    /// An item of the loaded timelines by its handle.
    pub fn item(&self, handle: &str) -> Option<&TimelineItem> {
        self.timelines.iter().flat_map(|timeline| &timeline.items).find(|item| item.handle == handle)
    }

    /// timelinePane.ts `clear`.
    pub fn clear(&mut self, cancel_pending: bool, page_size: usize) {
        self.max_item_count = page_size;
        self.timelines.clear();
        if cancel_pending {
            self.pending.clear();
        }
    }

    /// `loadTimelineForSource`: the request to send, recorded as pending, or `None` when nothing needs asking.
    pub fn request(&mut self, source: &str, reset: bool, limit: Option<usize>, page_size: usize, excluded: &HashSet<String>) -> Option<TimelineRequest> {
        if excluded.contains(source) {
            return None;
        }
        let timeline = self.timeline(source);
        let (mut cursor, mut limit) = match limit {
            Some(limit) => (None, Some(limit)),
            None => {
                if !reset && timeline.is_some_and(|timeline| !timeline.items.is_empty() && !timeline.more()) {
                    return None;
                }
                (timeline.filter(|_| !reset).and_then(|timeline| timeline.cursor.clone()), Some(page_size))
            }
        };
        if let Some(pending) = self.pending.get(source) {
            cursor = pending.cursor.clone();
            limit = limit.zip(pending.limit).map(|(limit, pending)| limit + pending);
        }
        // timelineService.ts `getTimeline`: no provider for the target, no request.
        let provided = match &self.uri {
            Some(uri) => self.providers.iter().any(|provider| provider.id == source && provider.handles(uri["scheme"].as_str().unwrap_or_default())),
            None => self.workspace && self.workspace_providers.iter().any(|provider| provider.id == source),
        };
        if !provided {
            return None;
        }
        self.next_request += 1;
        let request = TimelineRequest { id: self.next_request, source: source.to_string(), uri: self.uri.clone(), cursor, limit, reset };
        self.pending.insert(source.to_string(), request.clone());
        Some(request)
    }

    /// `reloadTimeline`: asks again for at least every item it has, so the visible rows stay.
    pub fn reload(&mut self, source: &str, page_size: usize, excluded: &HashSet<String>) -> Option<TimelineRequest> {
        let count = self.timeline(source).map_or(0, |timeline| timeline.items.len());
        self.timelines.retain(|timeline| timeline.source != source);
        self.request(source, true, Some(count.max(page_size)), page_size, excluded)
    }

    /// `handleRequest`: an answer to a replaced or cancelled request, or for another target, is dropped. Returns
    /// whether the items changed.
    pub fn respond(&mut self, request: &TimelineRequest, answer: Option<TimelineResult>) -> bool {
        if self.pending.get(&request.source).map(|pending| pending.id) != Some(request.id) {
            return false;
        }
        self.pending.remove(&request.source);
        let Some(mut answer) = answer else { return false };
        if request.uri != self.uri {
            return false;
        }
        for item in &mut answer.items {
            item.source = request.source.clone();
        }
        match self.timelines.iter_mut().find(|timeline| timeline.source == request.source) {
            Some(timeline) => timeline.add(answer),
            None => {
                self.timelines.push(TimelineAggregate { source: request.source.clone(), items: answer.items, cursor: answer.cursor, stale: false });
                true
            }
        }
    }

    /// `getItems`: one source's items as they are; several sources merged newest first, cut where a source with
    /// more pages could still add newer ones; then "Load more" while there are more items or requests.
    pub fn rows(&self, excluded: &HashSet<String>, now_ms: i64) -> Vec<Row<'_>> {
        let mut items: Vec<&TimelineItem> = Vec::new();
        let mut more = false;
        if !self.has_target() {
            return Vec::new();
        }
        if let [timeline] = self.timelines.as_slice() {
            if excluded.contains(&timeline.source) {
                return Vec::new();
            }
            more = timeline.more();
            if timeline.items.len() > self.max_item_count {
                more = true;
            }
            items.extend(timeline.items.iter().take(self.max_item_count));
        } else {
            let shown: Vec<&TimelineAggregate> = self.timelines.iter().filter(|timeline| !excluded.contains(&timeline.source) && !timeline.stale).collect();
            let mut most_recent_end = 0;
            for timeline in &shown {
                if timeline.more() {
                    more = true;
                    if let Some(last) = timeline.items.get(self.max_item_count.min(timeline.items.len().saturating_sub(1))) {
                        most_recent_end = most_recent_end.max(last.timestamp);
                    }
                }
            }
            let mut next = vec![0; shown.len()];
            loop {
                let mut newest: Option<usize> = None;
                for (index, timeline) in shown.iter().enumerate() {
                    let Some(item) = timeline.items.get(next[index]) else { continue };
                    if newest.is_none_or(|newest| item.timestamp >= shown[newest].items[next[newest]].timestamp) {
                        newest = Some(index);
                    }
                }
                let Some(index) = newest else { break };
                let item = &shown[index].items[next[index]];
                if item.timestamp >= most_recent_end {
                    if items.len() == self.max_item_count {
                        more = true;
                        break;
                    }
                    items.push(item);
                }
                next[index] += 1;
            }
        }
        let mut rows = Vec::new();
        let mut last_relative_time: Option<String> = None;
        for item in &items {
            let relative_time = from_now(item.timestamp, now_ms);
            let duplicate = last_relative_time.as_ref() == Some(&relative_time);
            last_relative_time = Some(relative_time.clone());
            rows.push(Row::Item { item, relative_time, duplicate });
        }
        if !items.is_empty() && (more || !self.pending.is_empty()) {
            rows.push(Row::LoadMore { loading: !self.pending.is_empty() });
        }
        rows
    }

    /// `refresh` and `setLoadingUriMessage`: the message over the rows; `local_history` is
    /// `workbench.localHistory.enabled`, `scm_providers` `scm.providerCount`.
    pub fn message(&self, excluded: &HashSet<String>, local_history: bool, scm_providers: usize) -> Option<String> {
        if !self.has_target() {
            return Some("The active editor cannot provide timeline information.".into());
        }
        if self.rows(excluded, 0).iter().any(|row| matches!(row, Row::Item { .. })) {
            return None;
        }
        if !self.pending.is_empty() {
            return Some(match self.workspace {
                true => "Loading workspace timeline...".into(),
                false => format!("Loading timeline for {}...", self.target_label()),
            });
        }
        let sources = self.sources();
        let mut message = if !sources.is_empty() && sources.iter().all(|source| excluded.contains(&source.id)) {
            "All timeline sources have been filtered out."
        } else if !self.workspace && local_history && !excluded.contains(LOCAL_HISTORY_SOURCE) {
            "Local History will track recent changes as you save them unless the file has been excluded or is too large."
        } else if sources.iter().any(|source| excluded.contains(&source.id)) {
            "No filtered timeline information was provided."
        } else {
            "No timeline information was provided."
        }
        .to_string();
        if scm_providers == 0 {
            message.push_str(" Source Control has not been configured.");
        }
        Some(message)
    }
}

/// resources.ts `basenameOrAuthority` of a URI value.
pub fn uri_basename(uri: &Value) -> String {
    let path = uri["path"].as_str().unwrap_or_default();
    match path.trim_end_matches('/').rsplit('/').next().filter(|name| !name.is_empty()) {
        Some(name) => name.to_string(),
        None => uri["authority"].as_str().unwrap_or_default().to_string(),
    }
}

/// date.ts `fromNow(date)` without the "ago" label, in its short words, as the pane shows `relativeTime`.
pub fn from_now(timestamp_ms: i64, now_ms: i64) -> String {
    const MINUTE: f64 = 60.;
    const HOUR: f64 = MINUTE * 60.;
    const DAY: f64 = HOUR * 24.;
    const WEEK: f64 = DAY * 7.;
    const MONTH: f64 = DAY * 30.;
    const YEAR: f64 = DAY * 365.;
    let seconds = ((now_ms - timestamp_ms) as f64 / 1000.).round();
    if seconds < -30. {
        return format!("in {}", from_now(now_ms + (seconds * 1000.) as i64, now_ms));
    }
    if seconds < 30. {
        return "now".into();
    }
    let (value, singular, plural) = match seconds {
        s if s < MINUTE => (s, "sec", "secs"),
        s if s < HOUR => ((s / MINUTE).round(), "min", "mins"),
        s if s < DAY => ((s / HOUR).round(), "hr", "hrs"),
        s if s < WEEK => ((s / DAY).round(), "day", "days"),
        s if s < MONTH => ((s / WEEK).round(), "wk", "wks"),
        s if s < YEAR => ((s / MONTH).round(), "mo", "mos"),
        s => ((s / YEAR).round(), "yr", "yrs"),
    };
    format!("{value} {}", if value == 1. { singular } else { plural })
}

/// date.ts `fromNow(date, true)`: the relative time with " ago", "now" as it is.
pub fn from_now_ago(timestamp_ms: i64, now_ms: i64) -> String {
    let relative = from_now(timestamp_ms, now_ms);
    match relative.as_str() {
        "now" => relative,
        _ if relative.starts_with("in ") => relative,
        _ => format!("{relative} ago"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn item(source: &str, timestamp: i64) -> Value {
        json!({ "handle": format!("{source}|{timestamp}"), "source": source, "label": format!("{source} {timestamp}"), "timestamp": timestamp })
    }

    fn model(sources: &[&str]) -> Timeline {
        let mut model = Timeline { uri: Some(json!({ "scheme": "file", "path": "/a.txt" })), max_item_count: 50, ..Timeline::default() };
        for source in sources {
            model.register(TimelineProvider::from_descriptor(&json!({ "id": source, "label": source, "scheme": ["file", "git"] })).unwrap());
        }
        model
    }

    fn answer(model: &mut Timeline, source: &str, items: Vec<Value>, cursor: Option<&str>) -> bool {
        let request = model.request(source, false, None, 50, &HashSet::new()).unwrap();
        model.respond(&request, TimelineResult::from_value(&json!({ "source": source, "items": items, "cursor": cursor })))
    }

    fn labels(model: &Timeline) -> Vec<String> {
        model
            .rows(&HashSet::new(), 0)
            .iter()
            .map(|row| match row {
                Row::Item { item, .. } => item.label.clone(),
                Row::LoadMore { loading } => format!("load more {loading}"),
            })
            .collect()
    }

    #[test]
    fn merges_sources_newest_first_and_stops_where_a_paged_source_ends() {
        let mut model = model(&["git", "local"]);
        answer(&mut model, "git", vec![item("git", 300), item("git", 100)], Some("next"));
        answer(&mut model, "local", vec![item("local", 200), item("local", 50)], None);
        assert_eq!(labels(&model), ["git 300", "local 200", "git 100", "load more false"]);
    }

    #[test]
    fn a_next_page_appends_and_drops_repeated_items() {
        let mut model = model(&["git"]);
        answer(&mut model, "git", vec![json!({ "handle": "git|a", "id": "a", "label": "a", "timestamp": 30 }), json!({ "handle": "git|b", "id": "b", "label": "b", "timestamp": 20 })], Some("b"));
        let request = model.request("git", false, None, 50, &HashSet::new()).unwrap();
        assert_eq!(request.cursor.as_deref(), Some("b"));
        let page = json!({ "items": [{ "handle": "git|b", "id": "b", "label": "b", "timestamp": 20 }, { "handle": "git|c", "id": "c", "label": "c", "timestamp": 10 }] });
        assert!(model.respond(&request, TimelineResult::from_value(&page)));
        assert_eq!(labels(&model), ["a", "b", "c"]);
        assert_eq!(model.request("git", false, None, 50, &HashSet::new()), None, "no cursor, nothing more to ask");
    }

    #[test]
    fn page_size_cuts_the_rows_and_load_more_shows() {
        let mut model = model(&["git"]);
        model.max_item_count = 2;
        answer(&mut model, "git", vec![item("git", 3), item("git", 2), item("git", 1)], None);
        assert_eq!(labels(&model), ["git 3", "git 2", "load more false"]);
    }

    #[test]
    fn a_replaced_request_and_a_foreign_scheme_are_ignored() {
        let mut model = model(&["git"]);
        let first = model.request("git", true, None, 50, &HashSet::new()).unwrap();
        let second = model.request("git", true, None, 50, &HashSet::new()).unwrap();
        assert_eq!(second.limit, Some(100), "a request in flight adds its limit");
        assert!(!model.respond(&first, TimelineResult::from_value(&json!({ "items": [item("git", 1)] }))));
        assert!(model.respond(&second, TimelineResult::from_value(&json!({ "items": [item("git", 1)] }))));
        model.uri = Some(json!({ "scheme": "untitled", "path": "Untitled-1" }));
        assert_eq!(model.request("git", true, None, 50, &HashSet::new()), None);
    }

    #[test]
    fn an_answer_for_another_target_is_dropped() {
        let mut model = model(&["git"]);
        let request = model.request("git", true, None, 50, &HashSet::new()).unwrap();
        model.uri = Some(json!({ "scheme": "file", "path": "/b.txt" }));
        assert!(!model.respond(&request, TimelineResult::from_value(&json!({ "items": [item("git", 1)] }))));
        assert!(model.timelines.is_empty());
    }

    #[test]
    fn the_workspace_timeline_asks_workspace_providers_only() {
        let mut model = model(&["git"]);
        model.uri = None;
        model.workspace = true;
        assert_eq!(model.request("git", true, None, 50, &HashSet::new()), None, "a file provider has no workspace timeline");
        model.register_workspace(TimelineProvider::from_descriptor(&json!({ "id": "git", "label": "Git History" })).unwrap());
        assert_eq!(model.sources().iter().map(|source| source.label.as_str()).collect::<Vec<_>>(), ["git"], "the file provider names the shared source");
        let request = model.request("git", true, None, 50, &HashSet::new()).unwrap();
        assert_eq!(request.uri, None);
        assert_eq!(model.message(&HashSet::new(), true, 1).as_deref(), Some("Loading workspace timeline..."));
        assert!(model.respond(&request, TimelineResult::from_value(&json!({ "items": [item("git", 1)] }))));
        assert_eq!(labels(&model), ["git 1"]);
        assert_eq!(model.item("git|1").map(|item| item.source.as_str()), Some("git"));
        assert!(model.unregister("git"), "the workspace provider keeps the source");
        assert!(!model.unregister_workspace("git"));
        assert!(model.timelines.is_empty());
    }

    #[test]
    fn messages_follow_the_pane() {
        let mut model = model(&["git"]);
        let excluded = HashSet::new();
        model.request("git", true, None, 50, &excluded);
        assert_eq!(model.message(&excluded, false, 1).as_deref(), Some("Loading timeline for a.txt..."));
        model.pending.clear();
        assert_eq!(model.message(&excluded, false, 1).as_deref(), Some("No timeline information was provided."));
        assert_eq!(model.message(&excluded, true, 1).as_deref(), Some("Local History will track recent changes as you save them unless the file has been excluded or is too large."));
        assert_eq!(model.message(&HashSet::from(["git".to_string()]), true, 0).as_deref(), Some("All timeline sources have been filtered out. Source Control has not been configured."));
        model.uri = None;
        assert_eq!(model.message(&excluded, false, 1).as_deref(), Some("The active editor cannot provide timeline information."));
    }

    #[test]
    fn an_item_without_a_label_is_named_after_its_resource() {
        let timeline = TimelineResult::from_value(&json!({ "items": [{ "handle": "h", "timestamp": 1, "resourceUri": { "scheme": "git", "path": "/w/a.txt" } }] })).unwrap();
        assert_eq!(timeline.items[0].label, "a.txt");
        assert!(timeline.items[0].label_from_resource);
    }

    #[test]
    fn relative_times_use_the_short_words() {
        let now = 1_000_000_000_000;
        let ago = |seconds: i64| from_now(now - seconds * 1000, now);
        assert_eq!([ago(10), ago(45), ago(60), ago(150), ago(3600 * 5), ago(86400), ago(86400 * 14), ago(86400 * 60), ago(86400 * 365)], ["now", "45 secs", "1 min", "3 mins", "5 hrs", "1 day", "2 wks", "2 mos", "1 yr"]);
        assert_eq!(ago(-120), "in 2 mins");
        assert_eq!([from_now_ago(now - 150_000, now), from_now_ago(now, now)], ["3 mins ago", "now"]);
    }

    #[test]
    fn a_repeated_relative_time_is_marked_duplicate() {
        let mut model = model(&["git"]);
        answer(&mut model, "git", vec![item("git", 0), item("git", -1000)], None);
        let duplicates: Vec<bool> = model.rows(&HashSet::new(), 86_400_000 * 3).iter().map(|row| matches!(row, Row::Item { duplicate: true, .. })).collect();
        assert_eq!(duplicates, [false, true]);
    }
}
