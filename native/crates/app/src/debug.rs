//! The breakpoint and watch model of workbench/contrib/debug/common/debugModel.ts (`DebugModel` without the
//! sessions), its storage (debugStorage.ts) and the breakpoint DTOs mainThreadDebugService.ts exchanges with the
//! extension host. Sessions, threads and the debug adapter protocol live in `debug/session.rs`.

pub mod configuration;
pub mod session;
#[cfg(test)]
mod tests;

use serde_json::{Map, Value, json};

/// debugStorage.ts keys, all in workspace storage.
pub const BREAKPOINTS_KEY: &str = "debug.breakpoint";
pub const FUNCTION_BREAKPOINTS_KEY: &str = "debug.functionbreakpoint";
pub const EXCEPTION_BREAKPOINTS_KEY: &str = "debug.exceptionbreakpoint";
pub const DATA_BREAKPOINTS_KEY: &str = "debug.databreakpoint";
pub const WATCH_EXPRESSIONS_KEY: &str = "debug.watchexpressions";
/// debugService.ts `DEBUG_BREAKPOINTS_ACTIVATED_KEY`.
pub const BREAKPOINTS_ACTIVATED_KEY: &str = "debug.breakpointactivated";

/// What one session's adapter said about a breakpoint (`IBreakpointSessionData`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionData {
    pub session_id: String,
    pub id: Option<i64>,
    pub verified: bool,
    pub message: Option<String>,
    pub source: Value,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub end_line: Option<u32>,
    pub end_column: Option<u32>,
    pub supports_log_points: bool,
    pub supports_conditional: bool,
    pub supports_hit_conditional: bool,
    pub supports_function: bool,
}

impl SessionData {
    /// debugModel.ts `toBreakpointSessionData`: a DAP `Breakpoint` with the session's capabilities.
    pub fn from_dap(breakpoint: &Value, capabilities: &Value) -> SessionData {
        let number = |key: &str| breakpoint[key].as_u64().map(|value| value as u32);
        SessionData {
            session_id: String::new(),
            id: breakpoint["id"].as_i64(),
            verified: breakpoint["verified"] == true,
            message: breakpoint["message"].as_str().map(str::to_owned),
            source: breakpoint["source"].clone(),
            line: number("line"),
            column: number("column"),
            end_line: number("endLine"),
            end_column: number("endColumn"),
            supports_log_points: capabilities["supportsLogPoints"] == true,
            supports_conditional: capabilities["supportsConditionalBreakpoints"] == true,
            supports_hit_conditional: capabilities["supportsHitConditionalBreakpoints"] == true,
            supports_function: capabilities["supportsFunctionBreakpoints"] == true,
        }
    }
}

/// `BaseBreakpoint`: the options every kind shares and the data of each session that knows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BaseBreakpoint {
    pub id: String,
    pub enabled: bool,
    pub condition: Option<String>,
    pub hit_condition: Option<String>,
    pub log_message: Option<String>,
    pub mode: Option<String>,
    pub mode_label: Option<String>,
    session_data: Vec<SessionData>,
    /// `this.data`: the one verified location, or the first session's data when none verified.
    data: Option<SessionData>,
}

impl BaseBreakpoint {
    pub fn new(id: String, options: &Value) -> Self {
        let text = |key: &str| options[key].as_str().map(str::to_owned);
        BaseBreakpoint {
            id,
            enabled: options["enabled"].as_bool().unwrap_or(true),
            condition: text("condition"),
            hit_condition: text("hitCondition"),
            log_message: text("logMessage"),
            mode: text("mode"),
            mode_label: text("modeLabel"),
            session_data: Vec::new(),
            data: None,
        }
    }

    /// `setSessionData`: two sessions verifying different locations leave no single location.
    pub fn set_session_data(&mut self, session_id: &str, data: Option<SessionData>) {
        self.session_data.retain(|existing| existing.session_id != session_id);
        if let Some(mut data) = data {
            data.session_id = session_id.to_owned();
            self.session_data.push(data);
        }
        let mut verified: Vec<&SessionData> = Vec::new();
        for data in self.session_data.iter().filter(|data| data.verified) {
            if !verified.iter().any(|seen| (seen.line, seen.column) == (data.line, data.column)) {
                verified.push(data);
            }
        }
        self.data = match verified.len() {
            0 => self.session_data.first().cloned(),
            1 => Some(verified[0].clone()),
            _ => None,
        };
    }

    pub fn message(&self) -> Option<&str> {
        self.data.as_ref()?.message.as_deref()
    }

    pub fn verified(&self) -> bool {
        self.data.as_ref().is_none_or(|data| data.verified)
    }

    pub fn id_from_adapter(&self, session_id: &str) -> Option<i64> {
        self.session_data.iter().find(|data| data.session_id == session_id)?.id
    }

    /// `getDebugProtocolBreakpoint`.
    pub fn debug_protocol_breakpoint(&self, session_id: &str) -> Option<Value> {
        let data = self.session_data.iter().find(|data| data.session_id == session_id)?;
        let mut breakpoint = json!({ "id": data.id, "verified": data.verified, "message": data.message, "source": data.source, "line": data.line, "column": data.column, "endLine": data.end_line, "endColumn": data.end_column });
        strip_nulls(&mut breakpoint);
        Some(breakpoint)
    }

    fn to_json(&self) -> Map<String, Value> {
        let mut object = Map::new();
        object.insert("id".into(), json!(self.id));
        object.insert("enabled".into(), json!(self.enabled));
        for (key, value) in [("condition", &self.condition), ("hitCondition", &self.hit_condition), ("logMessage", &self.log_message), ("mode", &self.mode), ("modeLabel", &self.mode_label)] {
            if let Some(value) = value {
                object.insert(key.into(), json!(value));
            }
        }
        object
    }
}

/// A source breakpoint (`Breakpoint`).
#[derive(Clone, Debug, PartialEq)]
pub struct Breakpoint {
    pub base: BaseBreakpoint,
    /// `UriComponents` of the resource the user set it in.
    pub uri: Value,
    line: u32,
    column: Option<u32>,
    adapter_data: Value,
    pub triggered_by: Option<String>,
    /// `sessionsDidTrigger`: sessions where the breakpoint it waits for was hit.
    pub sessions_did_trigger: Vec<String>,
}

impl Breakpoint {
    pub fn id(&self) -> &str {
        &self.base.id
    }

    /// `originalUri` as a string.
    pub fn uri_string(&self) -> String {
        uri_string(&self.uri)
    }

    /// `lineNumber`: the adapter's line once verified.
    pub fn line_number(&self) -> u32 {
        match &self.base.data {
            Some(data) if self.base.verified() => data.line.unwrap_or(self.line),
            _ => self.line,
        }
    }

    pub fn column(&self) -> Option<u32> {
        match &self.base.data {
            Some(data) if self.base.verified() && data.column.is_some() => data.column,
            _ => self.column,
        }
    }

    /// `sessionAgnosticData`: where the user set it.
    pub fn session_agnostic_line(&self) -> (u32, Option<u32>) {
        (self.line, self.column)
    }

    /// `verified`: an adapter verified it and the file has no unsaved edits.
    pub fn verified(&self, dirty: bool) -> bool {
        match &self.base.data {
            Some(data) => data.verified && !dirty,
            None => true,
        }
    }

    pub fn pending(&self) -> bool {
        self.base.data.is_none() && self.triggered_by.is_some()
    }

    /// `message`: a modified file's breakpoints are unverified until the session restarts.
    pub fn message(&self, dirty: bool) -> Option<String> {
        match dirty {
            true => Some("Unverified breakpoint. File is modified, please restart debug session.".into()),
            false => self.base.message().map(str::to_owned),
        }
    }

    pub fn supported(&self) -> bool {
        let Some(data) = &self.base.data else { return true };
        !(self.base.log_message.is_some() && !data.supports_log_points
            || self.base.condition.is_some() && !data.supports_conditional
            || self.base.hit_condition.is_some() && !data.supports_hit_conditional)
    }

    /// `toDAP`: a `SourceBreakpoint` at the line the user set.
    pub fn to_dap(&self) -> Value {
        let mut breakpoint = json!({ "line": self.line, "column": self.column, "condition": self.base.condition, "hitCondition": self.base.hit_condition, "logMessage": self.base.log_message, "mode": self.base.mode });
        strip_nulls(&mut breakpoint);
        breakpoint
    }

    pub fn adapter_data(&self) -> &Value {
        match &self.base.data {
            Some(data) if !data.source["adapterData"].is_null() => &data.source["adapterData"],
            _ => &self.adapter_data,
        }
    }

    pub fn set_session_data(&mut self, session_id: &str, data: Option<SessionData>) {
        self.base.set_session_data(session_id, data);
        if self.adapter_data.is_null() {
            self.adapter_data = self.adapter_data().clone();
        }
    }

    /// `update`: only the keys present in `data` change.
    pub fn update(&mut self, data: &Map<String, Value>) {
        let text = |value: &Value| value.as_str().map(str::to_owned);
        if let Some(line) = data.get("lineNumber").and_then(Value::as_u64) {
            self.line = line as u32;
        }
        if let Some(column) = data.get("column") {
            self.column = column.as_u64().map(|column| column as u32);
        }
        if let Some(condition) = data.get("condition") {
            self.base.condition = text(condition);
        }
        if let Some(hit_condition) = data.get("hitCondition") {
            self.base.hit_condition = text(hit_condition);
        }
        if let Some(log_message) = data.get("logMessage") {
            self.base.log_message = text(log_message);
        }
        if let Some(mode) = data.get("mode") {
            self.base.mode = text(mode);
            self.base.mode_label = data.get("modeLabel").and_then(text);
        }
        if let Some(triggered_by) = data.get("triggeredBy") {
            self.triggered_by = text(triggered_by);
            self.sessions_did_trigger.clear();
        }
    }

    fn to_json(&self) -> Value {
        let mut object = self.base.to_json();
        object.insert("uri".into(), uri_to_json(&self.uri));
        object.insert("lineNumber".into(), json!(self.line));
        if let Some(column) = self.column {
            object.insert("column".into(), json!(column));
        }
        if !self.adapter_data().is_null() {
            object.insert("adapterData".into(), self.adapter_data().clone());
        }
        if let Some(triggered_by) = &self.triggered_by {
            object.insert("triggeredBy".into(), json!(triggered_by));
        }
        Value::Object(object)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionBreakpoint {
    pub base: BaseBreakpoint,
    pub name: String,
}

impl FunctionBreakpoint {
    pub fn to_dap(&self) -> Value {
        let mut breakpoint = json!({ "name": self.name, "condition": self.base.condition, "hitCondition": self.base.hit_condition });
        strip_nulls(&mut breakpoint);
        breakpoint
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DataBreakpoint {
    pub base: BaseBreakpoint,
    pub description: String,
    pub data_id: String,
    pub can_persist: bool,
    pub access_type: String,
    pub access_types: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExceptionBreakpoint {
    pub base: BaseBreakpoint,
    pub filter: String,
    pub label: String,
    pub supports_condition: bool,
    pub description: Option<String>,
    pub condition_description: Option<String>,
    /// `fallback`: shown before any session reported its filters.
    pub fallback: bool,
    pub supported_sessions: Vec<String>,
}

impl ExceptionBreakpoint {
    /// `matches`: the same filter as an adapter's `ExceptionBreakpointsFilter`.
    fn matches(&self, filter: &Value) -> bool {
        self.filter == filter["filter"].as_str().unwrap_or_default()
            && self.label == filter["label"].as_str().unwrap_or_default()
            && self.supports_condition == (filter["supportsCondition"] == true)
            && self.condition_description.as_deref() == filter["conditionDescription"].as_str()
            && self.description.as_deref() == filter["description"].as_str()
    }

    pub fn is_supported_session(&self, session_id: Option<&str>) -> bool {
        match session_id {
            Some(session_id) => self.supported_sessions.iter().any(|id| id == session_id),
            None => self.fallback,
        }
    }
}

/// A watch expression (`Expression` as debugStorage.ts keeps it).
#[derive(Clone, Debug, PartialEq)]
pub struct WatchExpression {
    pub id: String,
    pub name: String,
}

/// `IBreakpointsChangeEvent`: the ids of what changed, for the extension host's delta.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BreakpointsChange {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
    /// A change only the sessions see (verification), which the host does not hear about.
    pub session_only: bool,
}

impl BreakpointsChange {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// debugModel.ts `DebugModel`, breakpoints and watch expressions.
#[derive(Clone, Debug, PartialEq)]
pub struct DebugModel {
    pub breakpoints: Vec<Breakpoint>,
    pub function_breakpoints: Vec<FunctionBreakpoint>,
    pub exception_breakpoints: Vec<ExceptionBreakpoint>,
    pub data_breakpoints: Vec<DataBreakpoint>,
    pub breakpoints_activated: bool,
    pub watch_expressions: Vec<WatchExpression>,
}

impl Default for DebugModel {
    fn default() -> Self {
        DebugModel { breakpoints: Vec::new(), function_breakpoints: Vec::new(), exception_breakpoints: Vec::new(), data_breakpoints: Vec::new(), breakpoints_activated: true, watch_expressions: Vec::new() }
    }
}

/// `IBreakpointData`: a breakpoint to add at a line of a resource.
#[derive(Clone, Debug, Default)]
pub struct BreakpointData {
    pub id: Option<String>,
    pub line: u32,
    pub column: Option<u32>,
    pub enabled: bool,
    pub condition: Option<String>,
    pub hit_condition: Option<String>,
    pub log_message: Option<String>,
    pub mode: Option<String>,
    pub mode_label: Option<String>,
    pub triggered_by: Option<String>,
}

impl BreakpointData {
    pub fn at(line: u32) -> Self {
        BreakpointData { line, enabled: true, ..Default::default() }
    }
}

impl DebugModel {
    /// `getBreakpoints({ originalUri })`.
    pub fn breakpoints_in(&self, uri: &str) -> impl Iterator<Item = &Breakpoint> {
        self.breakpoints.iter().filter(move |breakpoint| breakpoint.uri_string() == uri)
    }

    pub fn breakpoint(&self, id: &str) -> Option<&Breakpoint> {
        self.breakpoints.iter().find(|breakpoint| breakpoint.id() == id)
    }

    pub fn breakpoint_mut(&mut self, id: &str) -> Option<&mut Breakpoint> {
        self.breakpoints.iter_mut().find(|breakpoint| breakpoint.id() == id)
    }

    /// `addBreakpoints`: adding activates the breakpoints, then they sort and duplicates go.
    pub fn add_breakpoints(&mut self, uri: &Value, data: Vec<BreakpointData>) -> BreakpointsChange {
        let mut added = Vec::new();
        for data in data {
            let id = data.id.unwrap_or_else(generate_uuid);
            let options = json!({ "enabled": data.enabled, "condition": data.condition, "hitCondition": data.hit_condition, "logMessage": data.log_message, "mode": data.mode, "modeLabel": data.mode_label });
            added.push(id.clone());
            self.breakpoints.push(Breakpoint {
                base: BaseBreakpoint::new(id, &options),
                uri: normalize_uri(uri),
                line: data.line,
                column: data.column,
                adapter_data: Value::Null,
                triggered_by: data.triggered_by,
                sessions_did_trigger: Vec::new(),
            });
        }
        self.breakpoints_activated = true;
        self.sort_and_dedup();
        BreakpointsChange { added, ..Default::default() }
    }

    pub fn remove_breakpoints(&mut self, ids: &[String]) -> BreakpointsChange {
        let removed: Vec<String> = self.breakpoints.iter().filter(|breakpoint| ids.iter().any(|id| id == breakpoint.id())).map(|breakpoint| breakpoint.id().to_owned()).collect();
        self.breakpoints.retain(|breakpoint| !removed.iter().any(|id| id == breakpoint.id()));
        BreakpointsChange { removed, ..Default::default() }
    }

    /// `updateBreakpoints`: data keyed by breakpoint id.
    pub fn update_breakpoints(&mut self, data: &[(String, Map<String, Value>)]) -> BreakpointsChange {
        let mut changed = Vec::new();
        for (id, update) in data {
            if let Some(breakpoint) = self.breakpoint_mut(id) {
                breakpoint.update(update);
                changed.push(id.clone());
            }
        }
        self.sort_and_dedup();
        BreakpointsChange { changed, ..Default::default() }
    }

    /// `setBreakpointSessionData`: what a session's adapter answered, by breakpoint id; `None` forgets the session.
    pub fn set_breakpoint_session_data(&mut self, session_id: &str, capabilities: &Value, data: Option<&[(String, Value)]>) -> BreakpointsChange {
        let find = |id: &str| data.map(|data| data.iter().find(|(known, _)| known == id).map(|(_, breakpoint)| SessionData::from_dap(breakpoint, capabilities)));
        for breakpoint in &mut self.breakpoints {
            match find(breakpoint.id()) {
                None => breakpoint.set_session_data(session_id, None),
                Some(Some(found)) => breakpoint.set_session_data(session_id, Some(found)),
                Some(None) => {}
            }
        }
        for breakpoint in &mut self.function_breakpoints {
            match find(&breakpoint.base.id) {
                None => breakpoint.base.set_session_data(session_id, None),
                Some(Some(found)) => breakpoint.base.set_session_data(session_id, Some(found)),
                Some(None) => {}
            }
        }
        for breakpoint in &mut self.data_breakpoints {
            match find(&breakpoint.base.id) {
                None => breakpoint.base.set_session_data(session_id, None),
                Some(Some(found)) => breakpoint.base.set_session_data(session_id, Some(found)),
                Some(None) => {}
            }
        }
        for breakpoint in &mut self.exception_breakpoints {
            match find(&breakpoint.base.id) {
                None => breakpoint.base.set_session_data(session_id, None),
                Some(Some(found)) => breakpoint.base.set_session_data(session_id, Some(found)),
                Some(None) => {}
            }
        }
        BreakpointsChange { session_only: true, ..Default::default() }
    }

    /// `sortAndDeDup`: by file name, then line and column, one breakpoint per location.
    fn sort_and_dedup(&mut self) {
        self.breakpoints.sort_by(|first, second| {
            let (first_uri, second_uri) = (first.uri_string(), second.uri_string());
            if first_uri != second_uri {
                return basename(&first.uri).cmp(basename(&second.uri));
            }
            match first.line_number().cmp(&second.line_number()) {
                std::cmp::Ordering::Equal => match (first.column(), second.column()) {
                    (Some(first), Some(second)) => first.cmp(&second),
                    _ => std::cmp::Ordering::Greater,
                },
                order => order,
            }
        });
        let mut seen = std::collections::HashSet::new();
        self.breakpoints.retain(|breakpoint| seen.insert(format!("{}:{}:{:?}", breakpoint.uri_string(), breakpoint.line_number(), breakpoint.column())));
    }

    /// `setEnablement`: enabling also activates the breakpoints.
    pub fn set_enablement(&mut self, id: &str, enable: bool) -> BreakpointsChange {
        let mut changed = Vec::new();
        for base in self.bases_mut().filter(|base| base.id == id) {
            if base.enabled != enable {
                base.enabled = enable;
                changed.push(id.to_owned());
            }
        }
        if enable {
            self.breakpoints_activated = true;
        }
        BreakpointsChange { changed, ..Default::default() }
    }

    /// `enableOrDisableAllBreakpoints`: exception breakpoints keep their state.
    pub fn enable_or_disable_all(&mut self, enable: bool) -> BreakpointsChange {
        let mut changed = Vec::new();
        let sources = self.breakpoints.iter_mut().map(|breakpoint| &mut breakpoint.base);
        let functions = self.function_breakpoints.iter_mut().map(|breakpoint| &mut breakpoint.base);
        let data = self.data_breakpoints.iter_mut().map(|breakpoint| &mut breakpoint.base);
        for base in sources.chain(functions).chain(data) {
            if base.enabled != enable {
                base.enabled = enable;
                changed.push(base.id.clone());
            }
        }
        if enable {
            self.breakpoints_activated = true;
        }
        BreakpointsChange { changed, ..Default::default() }
    }

    fn bases_mut(&mut self) -> impl Iterator<Item = &mut BaseBreakpoint> {
        let sources = self.breakpoints.iter_mut().map(|breakpoint| &mut breakpoint.base);
        let functions = self.function_breakpoints.iter_mut().map(|breakpoint| &mut breakpoint.base);
        let data = self.data_breakpoints.iter_mut().map(|breakpoint| &mut breakpoint.base);
        let exceptions = self.exception_breakpoints.iter_mut().map(|breakpoint| &mut breakpoint.base);
        sources.chain(functions).chain(data).chain(exceptions)
    }

    pub fn add_function_breakpoint(&mut self, name: &str, options: &Value, id: Option<String>) -> BreakpointsChange {
        let id = id.unwrap_or_else(generate_uuid);
        self.function_breakpoints.push(FunctionBreakpoint { base: BaseBreakpoint::new(id.clone(), options), name: name.to_owned() });
        BreakpointsChange { added: vec![id], ..Default::default() }
    }

    /// `updateFunctionBreakpoint`: `name`, `condition` and `hitCondition` when given.
    pub fn update_function_breakpoint(&mut self, id: &str, update: &Map<String, Value>) -> BreakpointsChange {
        let Some(breakpoint) = self.function_breakpoints.iter_mut().find(|breakpoint| breakpoint.base.id == id) else { return BreakpointsChange::default() };
        if let Some(name) = update.get("name").and_then(Value::as_str) {
            breakpoint.name = name.to_owned();
        }
        if let Some(condition) = update.get("condition") {
            breakpoint.base.condition = condition.as_str().map(str::to_owned);
        }
        if let Some(hit_condition) = update.get("hitCondition") {
            breakpoint.base.hit_condition = hit_condition.as_str().map(str::to_owned);
        }
        BreakpointsChange { changed: vec![id.to_owned()], ..Default::default() }
    }

    /// `removeFunctionBreakpoints`: all of them without an id.
    pub fn remove_function_breakpoints(&mut self, id: Option<&str>) -> BreakpointsChange {
        let removed: Vec<String> = self.function_breakpoints.iter().filter(|breakpoint| id.is_none_or(|id| breakpoint.base.id == id)).map(|breakpoint| breakpoint.base.id.clone()).collect();
        self.function_breakpoints.retain(|breakpoint| !removed.contains(&breakpoint.base.id));
        BreakpointsChange { removed, ..Default::default() }
    }

    pub fn add_data_breakpoint(&mut self, breakpoint: DataBreakpoint) -> BreakpointsChange {
        let id = breakpoint.base.id.clone();
        self.data_breakpoints.push(breakpoint);
        BreakpointsChange { added: vec![id], ..Default::default() }
    }

    pub fn remove_data_breakpoints(&mut self, id: Option<&str>) -> BreakpointsChange {
        let removed: Vec<String> = self.data_breakpoints.iter().filter(|breakpoint| id.is_none_or(|id| breakpoint.base.id == id)).map(|breakpoint| breakpoint.base.id.clone()).collect();
        self.data_breakpoints.retain(|breakpoint| !removed.contains(&breakpoint.base.id));
        BreakpointsChange { removed, ..Default::default() }
    }

    /// `setExceptionBreakpointsForSession`: a filter no breakpoint matches yet becomes one, enabled when it is
    /// the adapter's default.
    pub fn set_exception_breakpoints_for_session(&mut self, session_id: &str, filters: &[Value]) -> bool {
        let mut added = false;
        for filter in filters {
            let index = match self.exception_breakpoints.iter().rposition(|breakpoint| breakpoint.matches(filter)) {
                Some(index) => index,
                None => {
                    added = true;
                    let options = json!({ "enabled": filter["default"] == true });
                    self.exception_breakpoints.push(ExceptionBreakpoint {
                        base: BaseBreakpoint::new(generate_uuid(), &options),
                        filter: filter["filter"].as_str().unwrap_or_default().to_owned(),
                        label: filter["label"].as_str().unwrap_or_default().to_owned(),
                        supports_condition: filter["supportsCondition"] == true,
                        description: filter["description"].as_str().map(str::to_owned),
                        condition_description: filter["conditionDescription"].as_str().map(str::to_owned),
                        fallback: false,
                        supported_sessions: Vec::new(),
                    });
                    self.exception_breakpoints.len() - 1
                }
            };
            let breakpoint = &mut self.exception_breakpoints[index];
            if !breakpoint.supported_sessions.iter().any(|id| id == session_id) {
                breakpoint.supported_sessions.push(session_id.to_owned());
            }
        }
        added
    }

    /// `removeExceptionBreakpointsForSession`.
    pub fn remove_exception_breakpoints_for_session(&mut self, session_id: &str) {
        for breakpoint in &mut self.exception_breakpoints {
            breakpoint.supported_sessions.retain(|id| id != session_id);
        }
    }

    /// `setExceptionBreakpointFallbackSession`: the filters of that session show while no session runs.
    pub fn set_exception_breakpoint_fallback_session(&mut self, session_id: &str) {
        for breakpoint in &mut self.exception_breakpoints {
            breakpoint.fallback = breakpoint.supported_sessions.iter().any(|id| id == session_id);
        }
    }

    /// `getExceptionBreakpointsForSession`.
    pub fn exception_breakpoints_for(&self, session_id: Option<&str>) -> impl Iterator<Item = &ExceptionBreakpoint> {
        self.exception_breakpoints.iter().filter(move |breakpoint| breakpoint.is_supported_session(session_id))
    }

    pub fn add_watch_expression(&mut self, name: &str) -> String {
        let id = generate_uuid();
        self.watch_expressions.push(WatchExpression { id: id.clone(), name: name.to_owned() });
        id
    }

    pub fn rename_watch_expression(&mut self, id: &str, name: &str) {
        if let Some(expression) = self.watch_expressions.iter_mut().find(|expression| expression.id == id) {
            expression.name = name.to_owned();
        }
    }

    /// `removeWatchExpressions`: all of them without an id.
    pub fn remove_watch_expressions(&mut self, id: Option<&str>) {
        self.watch_expressions.retain(|expression| id.is_some_and(|id| expression.id != id));
    }

    /// mainThreadDebugService.ts `convertToDto` of the breakpoints with these ids, sources at their verified line.
    pub fn dtos(&self, ids: &[String]) -> Vec<Value> {
        let mut dtos = Vec::new();
        for breakpoint in self.breakpoints.iter().filter(|breakpoint| ids.iter().any(|id| id == breakpoint.id())) {
            dtos.push(json!({
                "type": "source", "id": breakpoint.id(), "enabled": breakpoint.base.enabled, "condition": breakpoint.base.condition,
                "hitCondition": breakpoint.base.hit_condition, "logMessage": breakpoint.base.log_message, "uri": breakpoint.uri,
                "line": breakpoint.line_number().saturating_sub(1), "character": breakpoint.column().map_or(0, |column| column.saturating_sub(1)),
            }));
        }
        for breakpoint in self.function_breakpoints.iter().filter(|breakpoint| ids.contains(&breakpoint.base.id)) {
            let base = &breakpoint.base;
            dtos.push(json!({ "type": "function", "id": base.id, "enabled": base.enabled, "condition": base.condition, "hitCondition": base.hit_condition, "logMessage": base.log_message, "functionName": breakpoint.name }));
        }
        for breakpoint in self.data_breakpoints.iter().filter(|breakpoint| ids.contains(&breakpoint.base.id)) {
            let base = &breakpoint.base;
            dtos.push(json!({
                "type": "data", "id": base.id, "dataId": breakpoint.data_id, "enabled": base.enabled, "condition": base.condition, "hitCondition": base.hit_condition,
                "logMessage": base.log_message, "accessType": breakpoint.access_type, "label": breakpoint.description, "canPersist": breakpoint.can_persist,
            }));
        }
        for dto in &mut dtos {
            strip_nulls(dto);
        }
        dtos
    }

    /// Every breakpoint id the extension host can know: sources, functions and data.
    pub fn all_ids(&self) -> Vec<String> {
        let sources = self.breakpoints.iter().map(|breakpoint| breakpoint.id().to_owned());
        let functions = self.function_breakpoints.iter().map(|breakpoint| breakpoint.base.id.clone());
        let data = self.data_breakpoints.iter().map(|breakpoint| breakpoint.base.id.clone());
        sources.chain(functions).chain(data).collect()
    }

    /// debugStorage.ts `storeBreakpoints` and `storeWatchExpressions`: each key with its JSON, `None` to remove it.
    pub fn storage_entries(&self) -> Vec<(&'static str, Option<String>)> {
        let list = |values: Vec<Value>| (!values.is_empty()).then(|| Value::Array(values).to_string());
        let functions = self.function_breakpoints.iter().map(|breakpoint| {
            let mut object = breakpoint.base.to_json();
            object.insert("name".into(), json!(breakpoint.name));
            Value::Object(object)
        });
        let data = self.data_breakpoints.iter().filter(|breakpoint| breakpoint.can_persist).map(|breakpoint| {
            let mut object = breakpoint.base.to_json();
            object.insert("description".into(), json!(breakpoint.description));
            object.insert("src".into(), json!({ "type": 0, "dataId": breakpoint.data_id }));
            object.insert("canPersist".into(), json!(true));
            object.insert("accessType".into(), json!(breakpoint.access_type));
            if let Some(access_types) = &breakpoint.access_types {
                object.insert("accessTypes".into(), access_types.clone());
            }
            Value::Object(object)
        });
        let exceptions = self.exception_breakpoints.iter().map(|breakpoint| {
            let mut object = breakpoint.base.to_json();
            object.insert("filter".into(), json!(breakpoint.filter));
            object.insert("label".into(), json!(breakpoint.label));
            object.insert("supportsCondition".into(), json!(breakpoint.supports_condition));
            for (key, value) in [("description", &breakpoint.description), ("conditionDescription", &breakpoint.condition_description)] {
                if let Some(value) = value {
                    object.insert(key.into(), json!(value));
                }
            }
            object.insert("fallback".into(), json!(breakpoint.fallback));
            Value::Object(object)
        });
        let watch = self.watch_expressions.iter().map(|expression| json!({ "name": expression.name, "id": expression.id }));
        vec![
            (BREAKPOINTS_KEY, list(self.breakpoints.iter().map(Breakpoint::to_json).collect())),
            (FUNCTION_BREAKPOINTS_KEY, list(functions.collect())),
            (DATA_BREAKPOINTS_KEY, list(data.collect())),
            (EXCEPTION_BREAKPOINTS_KEY, list(exceptions.collect())),
            (WATCH_EXPRESSIONS_KEY, list(watch.collect())),
            // debugService.ts stores the key only while the breakpoints are deactivated.
            (BREAKPOINTS_ACTIVATED_KEY, (!self.breakpoints_activated).then(|| "false".into())),
        ]
    }

    /// debugStorage.ts `load*`: a key that does not parse loads as empty, as upstream logs and goes on.
    pub fn from_storage(get: impl Fn(&str) -> Option<String>) -> DebugModel {
        let list = |key: &str| -> Vec<Value> { get(key).and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default() };
        let text = |value: &Value, key: &str| value[key].as_str().map(str::to_owned);
        let id = |value: &Value| text(value, "id").unwrap_or_else(generate_uuid);
        let breakpoints = list(BREAKPOINTS_KEY)
            .iter()
            .map(|value| Breakpoint {
                base: BaseBreakpoint::new(id(value), value),
                uri: normalize_uri(&value["uri"]),
                line: value["lineNumber"].as_u64().unwrap_or(1) as u32,
                column: value["column"].as_u64().map(|column| column as u32),
                adapter_data: value["adapterData"].clone(),
                triggered_by: text(value, "triggeredBy"),
                sessions_did_trigger: Vec::new(),
            })
            .collect();
        let function_breakpoints = list(FUNCTION_BREAKPOINTS_KEY).iter().map(|value| FunctionBreakpoint { base: BaseBreakpoint::new(id(value), value), name: text(value, "name").unwrap_or_default() }).collect();
        let data_breakpoints = list(DATA_BREAKPOINTS_KEY)
            .iter()
            .map(|value| DataBreakpoint {
                base: BaseBreakpoint::new(id(value), value),
                description: text(value, "description").unwrap_or_default(),
                data_id: value["src"]["dataId"].as_str().or(value["src"]["address"].as_str()).unwrap_or_default().to_owned(),
                can_persist: value["canPersist"] == true,
                access_type: text(value, "accessType").unwrap_or_else(|| "write".into()),
                access_types: value.get("accessTypes").cloned(),
            })
            .collect();
        let exception_breakpoints = list(EXCEPTION_BREAKPOINTS_KEY)
            .iter()
            .map(|value| ExceptionBreakpoint {
                base: BaseBreakpoint::new(id(value), value),
                filter: text(value, "filter").unwrap_or_default(),
                label: text(value, "label").unwrap_or_default(),
                supports_condition: value["supportsCondition"] == true,
                description: text(value, "description"),
                condition_description: text(value, "conditionDescription"),
                fallback: value["fallback"] == true,
                supported_sessions: Vec::new(),
            })
            .collect();
        let watch_expressions = list(WATCH_EXPRESSIONS_KEY).iter().map(|value| WatchExpression { id: id(value), name: text(value, "name").unwrap_or_default() }).collect();
        let breakpoints_activated = get(BREAKPOINTS_ACTIVATED_KEY).is_none_or(|value| value != "false");
        DebugModel { breakpoints, function_breakpoints, exception_breakpoints, data_breakpoints, breakpoints_activated, watch_expressions }
    }
}

/// A breakpoint's shared part from `IBaseBreakpointOptions`.
pub fn base_breakpoint(id: String, options: &Value) -> BaseBreakpoint {
    BaseBreakpoint::new(id, options)
}

/// The folder path of a `file` `UriComponents` the host passes as a workspace folder.
pub fn configuration_folder(uri: &Value) -> Option<std::path::PathBuf> {
    (uri["scheme"] == "file").then(|| uri["path"].as_str().map(std::path::PathBuf::from)).flatten()
}

/// `URI.parse` of a file URI string; other schemes keep the text after the scheme as their path.
pub fn uri_from_string(uri: &str) -> Value {
    match crate::editor_history::file_path(uri) {
        Some(path) => json!({ "$mid": 1, "scheme": "file", "authority": "", "path": path.to_string_lossy(), "query": "", "fragment": "" }),
        None => {
            let (scheme, path) = uri.split_once(':').unwrap_or(("file", uri));
            json!({ "$mid": 1, "scheme": scheme, "authority": "", "path": path, "query": "", "fragment": "" })
        }
    }
}

/// `UriComponents` with every part present, as the host revives them.
pub fn normalize_uri(uri: &Value) -> Value {
    let part = |key: &str| uri[key].as_str().unwrap_or_default();
    json!({ "$mid": 1, "scheme": part("scheme"), "authority": part("authority"), "path": part("path"), "query": part("query"), "fragment": part("fragment") })
}

/// `URI.toString()` of `UriComponents`.
pub fn uri_string(uri: &Value) -> String {
    let part = |key: &str| uri[key].as_str().unwrap_or_default();
    let mut text = crate::editor::uri_string(part("scheme"), std::path::Path::new(part("path")));
    if !part("authority").is_empty() {
        text = text.replacen(':', &format!("://{}", part("authority")), 1);
    }
    if !part("query").is_empty() {
        text.push('?');
        text.push_str(part("query"));
    }
    if !part("fragment").is_empty() {
        text.push('#');
        text.push_str(part("fragment"));
    }
    text
}

/// `URI.toJSON()` as storage keeps it: the parts, `fsPath` and `external`.
fn uri_to_json(uri: &Value) -> Value {
    let mut object = Map::new();
    object.insert("$mid".into(), json!(1));
    if uri["scheme"] == "file" {
        object.insert("fsPath".into(), uri["path"].clone());
    }
    object.insert("external".into(), json!(uri_string(uri)));
    for key in ["path", "scheme", "authority", "query", "fragment"] {
        if uri[key].as_str().is_some_and(|value| !value.is_empty()) {
            object.insert(key.into(), uri[key].clone());
        }
    }
    Value::Object(object)
}

/// `resources.basenameOrAuthority`.
fn basename(uri: &Value) -> &str {
    let path = uri["path"].as_str().unwrap_or_default();
    match path.rsplit('/').next() {
        Some(name) if !name.is_empty() => name,
        _ => uri["authority"].as_str().unwrap_or_default(),
    }
}

/// JSON with `undefined` members left out, as `JSON.stringify` drops them.
pub fn strip_nulls(value: &mut Value) {
    if let Value::Object(object) = value {
        object.retain(|_, member| !member.is_null());
    }
}

/// `generateUuid()`: a version 4 UUID.
pub fn generate_uuid() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut random| random.read_exact(&mut bytes)).expect("/dev/urandom is readable");
    bytes[6] = bytes[6] & 0x0f | 0x40;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}
