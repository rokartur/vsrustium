//! The test model of workbench/contrib/testing/common: the test items the controllers publish
//! (mainThreadTestCollection.ts, testTypes.ts `TestsDiffOp`), the run profiles (testProfileService.ts) and the
//! results of the runs (testResult.ts `LiveTestResult`, testingStates.ts, getComputedState.ts).

use std::collections::HashMap;

use serde_json::{Value, json};

#[cfg(test)]
mod tests;

/// testTypes.ts `TestResultState`, in its wire values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ResultState {
    #[default]
    Unset,
    Queued,
    Running,
    Passed,
    Failed,
    Skipped,
    Errored,
}

impl ResultState {
    pub fn from_value(value: &Value) -> Self {
        match value.as_u64() {
            Some(1) => ResultState::Queued,
            Some(2) => ResultState::Running,
            Some(3) => ResultState::Passed,
            Some(4) => ResultState::Failed,
            Some(5) => ResultState::Skipped,
            Some(6) => ResultState::Errored,
            _ => ResultState::Unset,
        }
    }

    pub fn to_value(self) -> u8 {
        self as u8
    }

    /// testingStates.ts `statePriority`.
    pub fn priority(self) -> u8 {
        match self {
            ResultState::Running => 6,
            ResultState::Errored => 5,
            ResultState::Failed => 4,
            ResultState::Queued => 3,
            ResultState::Passed => 2,
            ResultState::Skipped => 1,
            ResultState::Unset => 0,
        }
    }

    /// `terminalStatePriorities`: a terminal state never goes back to a lower one.
    fn terminal_priority(self) -> Option<u8> {
        match self {
            ResultState::Passed => Some(0),
            ResultState::Skipped => Some(1),
            ResultState::Failed => Some(2),
            ResultState::Errored => Some(3),
            _ => None,
        }
    }

    pub fn max(self, other: ResultState) -> ResultState {
        if other.priority() > self.priority() { other } else { self }
    }
}

/// testId.ts: the path of ids from the controller down, joined by `\0`.
pub fn parent_id(id: &str) -> Option<&str> {
    id.rfind('\0').map(|index| &id[..index])
}

pub fn root_id(id: &str) -> &str {
    id.split('\0').next().unwrap_or(id)
}

/// testTypes.ts `TestItemExpandState`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExpandState {
    #[default]
    NotExpandable,
    Expandable,
    BusyExpanding,
    Expanded,
}

impl ExpandState {
    fn from_value(value: &Value) -> Self {
        match value.as_u64() {
            Some(1) => ExpandState::Expandable,
            Some(2) => ExpandState::BusyExpanding,
            Some(3) => ExpandState::Expanded,
            _ => ExpandState::NotExpandable,
        }
    }
}

/// An `InternalTestItem`: the serialized `ITestItem` and its expand state.
#[derive(Clone, Debug, PartialEq)]
pub struct TestItem {
    pub id: String,
    pub expand: ExpandState,
    /// `ITestItem.Serialized`: label, tags, busy, uri, range, description, error, sortText.
    pub item: Value,
}

impl TestItem {
    pub fn label(&self) -> &str {
        self.item["label"].as_str().unwrap_or_default()
    }

    pub fn controller_id(&self) -> &str {
        root_id(&self.id)
    }

    /// The `file` path of the item's uri.
    pub fn path(&self) -> Option<&str> {
        let uri = &self.item["uri"];
        (uri["scheme"] == "file").then(|| uri["path"].as_str()).flatten()
    }

    /// The 1-based start line of the item's range.
    pub fn start_line(&self) -> Option<u32> {
        self.item["range"]["startLineNumber"].as_u64().map(|line| line as u32)
    }

    fn sort_key(&self) -> String {
        self.item["sortText"].as_str().unwrap_or(self.label()).to_owned()
    }
}

/// The items every controller published, by id, with each one's children (mainThreadTestCollection.ts).
#[derive(Default)]
pub struct TestCollection {
    pub items: HashMap<String, TestItem>,
    pub children: HashMap<String, Vec<String>>,
    pub roots: Vec<String>,
    /// `IncrementPendingExtHosts`: hosts still discovering tests.
    pub pending_hosts: i64,
}

impl TestCollection {
    /// `apply` of a published diff; answers the ids the explorer shows expanded at once that can expand: the
    /// roots and their children (index.ts: the top level starts expanded, and a single root shows its children there).
    pub fn apply(&mut self, diff: &[Value]) -> Vec<String> {
        let mut expandable_roots = Vec::new();
        for op in diff {
            match op["op"].as_u64() {
                Some(0) => {
                    let id = op["item"]["item"]["extId"].as_str().unwrap_or_default().to_owned();
                    let item = TestItem { id: id.clone(), expand: ExpandState::from_value(&op["item"]["expand"]), item: op["item"]["item"].clone() };
                    let parent = parent_id(&id);
                    match parent {
                        Some(parent) => self.children.entry(parent.to_owned()).or_default().push(id.clone()),
                        None => self.roots.push(id.clone()),
                    }
                    if item.expand == ExpandState::Expandable && parent.is_none_or(|parent| parent_id(parent).is_none()) {
                        expandable_roots.push(id.clone());
                    }
                    self.items.insert(id, item);
                }
                Some(1) => {
                    let patch = &op["item"];
                    let Some(item) = self.items.get_mut(patch["extId"].as_str().unwrap_or_default()) else { continue };
                    if !patch["expand"].is_null() {
                        item.expand = ExpandState::from_value(&patch["expand"]);
                        // `canResolveChildren` set after the add arrives as an update.
                        if item.expand == ExpandState::Expandable && parent_id(&item.id).is_none_or(|parent| parent_id(parent).is_none()) {
                            expandable_roots.push(item.id.clone());
                        }
                    }
                    if let (Some(fields), Some(target)) = (patch["item"].as_object(), item.item.as_object_mut()) {
                        for (key, value) in fields {
                            target.insert(key.clone(), value.clone());
                        }
                    }
                }
                Some(3) => self.remove(op["itemId"].as_str().unwrap_or_default()),
                Some(4) => self.pending_hosts += op["amount"].as_i64().unwrap_or_default(),
                _ => {}
            }
        }
        for children in self.children.values_mut() {
            children.sort_by_key(|id| self.items.get(id).map(TestItem::sort_key).unwrap_or_default());
        }
        expandable_roots
    }

    fn remove(&mut self, id: &str) {
        for child in self.children.remove(id).unwrap_or_default() {
            self.remove(&child);
        }
        self.items.remove(id);
        self.roots.retain(|root| root != id);
        if let Some(siblings) = parent_id(id).and_then(|parent| self.children.get_mut(parent)) {
            siblings.retain(|sibling| sibling != id);
        }
    }

    pub fn children_of(&self, id: &str) -> &[String] {
        self.children.get(id).map_or(&[], Vec::as_slice)
    }

    /// `getReviverDiff`: every item as an Add, parents first, for a host that subscribes.
    pub fn reviver_diff(&self) -> Vec<Value> {
        let mut diff = Vec::new();
        let mut queue: Vec<&String> = self.roots.iter().collect();
        while let Some(id) = queue.pop() {
            let item = &self.items[id];
            diff.push(json!({ "op": 0, "item": { "expand": item.expand as u8, "item": item.item } }));
            queue.extend(self.children_of(id));
        }
        diff
    }

    /// The tests of a file whose range is known, with their 1-based start line.
    pub fn tests_in_file<'a>(&'a self, path: &'a str) -> impl Iterator<Item = &'a TestItem> {
        self.items.values().filter(move |item| item.path() == Some(path) && item.start_line().is_some())
    }
}

/// testTypes.ts `TestRunProfileBitset`.
pub const GROUP_RUN: u64 = 1 << 1;
pub const GROUP_DEBUG: u64 = 1 << 2;
pub const GROUP_COVERAGE: u64 = 1 << 3;

/// An `ITestRunProfile`.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub controller_id: String,
    pub profile_id: i64,
    pub label: String,
    pub group: u64,
    pub is_default: bool,
    pub tag: Option<String>,
}

impl Profile {
    pub fn from_value(value: &Value) -> Self {
        Profile {
            controller_id: value["controllerId"].as_str().unwrap_or_default().to_owned(),
            profile_id: value["profileId"].as_i64().unwrap_or_default(),
            label: value["label"].as_str().unwrap_or_default().to_owned(),
            group: value["group"].as_u64().unwrap_or_default(),
            is_default: value["isDefault"] == true,
            tag: value["tag"].as_str().map(str::to_owned),
        }
    }

    pub fn update(&mut self, patch: &Value) {
        if let Some(label) = patch["label"].as_str() {
            self.label = label.to_owned();
        }
        if let Some(group) = patch["group"].as_u64() {
            self.group = group;
        }
        if let Some(is_default) = patch["isDefault"].as_bool() {
            self.is_default = is_default;
        }
        if patch.get("tag").is_some() {
            self.tag = patch["tag"].as_str().map(str::to_owned);
        }
    }

    /// testProfileService.ts `canUseProfileWithTest`: an untagged profile, or one whose tag the test has.
    pub fn can_run(&self, test: &TestItem) -> bool {
        let Some(tag) = &self.tag else { return true };
        let namespaced = format!("{}\0{tag}", self.controller_id);
        test.item["tags"].as_array().is_some_and(|tags| tags.iter().any(|candidate| candidate.as_str() == Some(&namespaced)))
    }
}

/// `getDefaultProfileForTest`: the controller's default profile of the group that can run the test, else its
/// first profile of the group that can.
pub fn default_profile<'a>(profiles: &'a [Profile], group: u64, test: &TestItem) -> Option<&'a Profile> {
    let usable = || profiles.iter().filter(|profile| profile.group & group != 0 && profile.controller_id == test.controller_id() && profile.can_run(test));
    usable().find(|profile| profile.is_default).or_else(|| usable().next())
}

/// One test's state in one task of a run (`ITestTaskState`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskState {
    pub state: ResultState,
    pub duration: Option<f64>,
    /// `ITestMessage.Serialized`: error messages and output markers.
    pub messages: Vec<Value>,
}

/// A test of a run (`TestResultItem`): its item when the run started, its parent and its state per task.
#[derive(Clone, Debug, PartialEq)]
pub struct ResultItem {
    pub item: Value,
    pub expand: u8,
    pub parent: Option<String>,
    pub children: Vec<String>,
    pub tasks: Vec<TaskState>,
    pub own_state: ResultState,
    pub own_duration: Option<f64>,
}

impl ResultItem {
    pub fn label(&self) -> &str {
        self.item["label"].as_str().unwrap_or_default()
    }
}

/// `ITestRunTask` with its output (`TaskRawOutput`).
#[derive(Clone, Debug, PartialEq)]
pub struct RunTask {
    pub id: String,
    pub name: String,
    pub controller_id: String,
    pub running: bool,
    pub output: String,
}

/// A `LiveTestResult`.
#[derive(Clone, Debug, PartialEq)]
pub struct TestResult {
    pub id: String,
    /// `ResolvedTestRunRequest`.
    pub request: Value,
    pub tasks: Vec<RunTask>,
    pub items: HashMap<String, ResultItem>,
    pub roots: Vec<String>,
    pub completed_at: Option<u64>,
}

impl TestResult {
    pub fn new(id: String, request: Value) -> Self {
        TestResult { id, request, tasks: Vec::new(), items: HashMap::new(), roots: Vec::new(), completed_at: None }
    }

    /// testResult.ts `name`: the first included test's label, "+ n more" for the rest.
    pub fn name(&self) -> String {
        let ids: Vec<&str> = self.request["targets"].as_array().into_iter().flatten().flat_map(|target| target["testIds"].as_array().into_iter().flatten()).filter_map(Value::as_str).collect();
        let Some(first) = ids.first() else { return "Test Run".into() };
        let label = self.items.get(*first).map(ResultItem::label).unwrap_or(first);
        match ids.len() {
            1 => label.to_owned(),
            count => format!("{label} +{} more", count - 1),
        }
    }

    pub fn add_task(&mut self, task: &Value) {
        self.tasks.push(RunTask {
            id: task["id"].as_str().unwrap_or_default().to_owned(),
            name: task["name"].as_str().unwrap_or_default().to_owned(),
            controller_id: task["ctrlId"].as_str().unwrap_or_default().to_owned(),
            running: task["running"] != false,
            output: String::new(),
        });
        for item in self.items.values_mut() {
            item.tasks.push(TaskState::default());
        }
    }

    fn task_index(&self, task_id: &str) -> Option<usize> {
        self.tasks.iter().position(|task| task.id == task_id)
    }

    /// `addTestChainToRun`: the chain from a root (or a known test) down to the test.
    pub fn add_chain(&mut self, chain: &[Value]) {
        let mut parent: Option<String> = None;
        for item in chain {
            let id = item["extId"].as_str().unwrap_or_default().to_owned();
            if !self.items.contains_key(&id) {
                let tasks = vec![TaskState::default(); self.tasks.len()];
                self.items.insert(id.clone(), ResultItem { item: item.clone(), expand: 0, parent: parent.clone(), children: Vec::new(), tasks, own_state: ResultState::Unset, own_duration: None });
                match &parent {
                    Some(parent) => self.items.get_mut(parent).expect("added before its children").children.push(id.clone()),
                    None => self.roots.push(id.clone()),
                }
            }
            parent = Some(id);
        }
    }

    /// `updateState`: a terminal state never goes back to a lower terminal or a non-terminal one.
    pub fn update_state(&mut self, test_id: &str, task_id: &str, state: ResultState, duration: Option<f64>) {
        let Some(index) = self.task_index(task_id) else { return };
        let Some(item) = self.items.get_mut(test_id) else { return };
        if let Some(old) = item.tasks[index].state.terminal_priority()
            && state.terminal_priority().is_none_or(|new| new < old)
        {
            return;
        }
        item.tasks[index].state = state;
        if let Some(duration) = duration {
            item.tasks[index].duration = Some(duration);
            item.own_duration = Some(item.own_duration.unwrap_or(0.).max(duration));
        }
        item.own_state = item.tasks.iter().fold(ResultState::Unset, |max, task| max.max(task.state));
    }

    pub fn append_message(&mut self, test_id: &str, task_id: &str, message: Value) {
        let Some(index) = self.task_index(task_id) else { return };
        if let Some(item) = self.items.get_mut(test_id) {
            item.tasks[index].messages.push(message);
        }
    }

    /// `appendOutput`: the task's output, and an output message on the test that wrote it.
    pub fn append_output(&mut self, task_id: &str, output: &str, location: Option<Value>, test_id: Option<&str>) {
        let Some(index) = self.task_index(task_id) else { return };
        let task = &mut self.tasks[index];
        let offset = task.output.len();
        task.output.push_str(output);
        let Some(item) = test_id.and_then(|id| self.items.get_mut(id)) else { return };
        item.tasks[index].messages.push(json!({ "type": 1, "message": output, "offset": offset, "length": output.len(), "location": location }));
    }

    /// `markTaskComplete`: queued and running tests of the task are skipped.
    pub fn mark_task_complete(&mut self, task_id: &str) {
        let Some(index) = self.task_index(task_id) else { return };
        self.tasks[index].running = false;
        let unfinished: Vec<String> = self.items.iter().filter(|(_, item)| matches!(item.tasks[index].state, ResultState::Queued | ResultState::Running)).map(|(id, _)| id.clone()).collect();
        for id in unfinished {
            self.update_state(&id, task_id, ResultState::Skipped, None);
        }
    }

    pub fn mark_complete(&mut self, now: u64) {
        let running: Vec<String> = self.tasks.iter().filter(|task| task.running).map(|task| task.id.clone()).collect();
        for task in running {
            self.mark_task_complete(&task);
        }
        self.completed_at = Some(now);
    }

    /// getComputedState.ts: the item's own state raised by its children's.
    pub fn computed_state(&self, id: &str) -> ResultState {
        let Some(item) = self.items.get(id) else { return ResultState::Unset };
        item.children.iter().fold(item.own_state, |state, child| state.max(self.computed_state(child)))
    }

    /// `counts` of the tests' own states.
    pub fn count(&self, state: ResultState) -> usize {
        self.items.values().filter(|item| item.own_state == state).count()
    }

    /// `toJSONWithMessages` (`ISerializedTestResults`), for `$publishTestResults`.
    pub fn to_json(&self) -> Value {
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|(id, item)| {
                let tasks: Vec<Value> = item.tasks.iter().map(|task| json!({ "state": task.state.to_value(), "duration": task.duration, "messages": task.messages })).collect();
                json!({ "expand": item.expand, "item": item.item, "tasks": tasks, "ownComputedState": item.own_state.to_value(), "computedState": self.computed_state(id).to_value() })
            })
            .collect();
        let tasks: Vec<Value> = self.tasks.iter().map(|task| json!({ "id": task.id, "name": task.name, "ctrlId": task.controller_id, "hasCoverage": false })).collect();
        json!({ "id": self.id, "completedAt": self.completed_at, "items": items, "tasks": tasks, "name": self.name(), "request": self.request })
    }
}
