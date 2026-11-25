//! performance.ts marks of the port (`code/...`, one process, so all under `renderer`) and of the extension host
//! (`$setPerformanceMarks`), with mainThreadExtensionService.ts's activation times; Developer: Startup Performance
//! (workbench/perf_view.rs) reads them. Global as upstream's `vs/base/common/performance` is.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// A mark's name and its time in milliseconds since the epoch, as `PerformanceMark.startTime`.
pub type Mark = (String, f64);

/// `ActivationTimes` and the `ExtensionActivationReason` of one extension.
pub struct Activation {
    pub id: String,
    pub startup: bool,
    pub code_loading: f64,
    pub activate_call: f64,
    pub activate_resolved: f64,
    pub event: String,
    pub by: String,
}

static MARKS: Mutex<Vec<Mark>> = Mutex::new(Vec::new());
static EXT_HOST_MARKS: Mutex<Vec<Mark>> = Mutex::new(Vec::new());
static ACTIVATIONS: Mutex<Vec<Activation>> = Mutex::new(Vec::new());

pub fn mark(name: &str) {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("the clock is after 1970").as_secs_f64() * 1000.;
    MARKS.lock().unwrap().push((name.to_owned(), now.round()));
}

/// `ITimerService.getPerformanceMarks`: each source with its marks.
pub fn performance_marks() -> Vec<(&'static str, Vec<Mark>)> {
    let mut sources = vec![("renderer", MARKS.lock().unwrap().clone())];
    let ext_host = EXT_HOST_MARKS.lock().unwrap().clone();
    if !ext_host.is_empty() {
        sources.push(("localExtHost", ext_host));
    }
    sources
}

/// The duration between two port marks, when both happened.
pub fn between(from: &str, to: &str) -> Option<f64> {
    let marks = MARKS.lock().unwrap();
    let time = |name: &str| marks.iter().find(|(mark, _)| mark == name).map(|(_, time)| *time);
    Some(time(to)? - time(from)?)
}

/// `$setPerformanceMarks`: `[{ name, startTime }]`.
pub fn set_ext_host_marks(marks: &Value) {
    let marks = marks.as_array().into_iter().flatten().map(|mark| (mark["name"].as_str().unwrap_or_default().to_owned(), mark["startTime"].as_f64().unwrap_or_default()));
    *EXT_HOST_MARKS.lock().unwrap() = marks.collect();
}

/// `$onDidActivateExtension(extensionId, codeLoadingTime, activateCallTime, activateResolvedTime, activationReason)`.
pub fn extension_activated(args: &[Value]) {
    let reason = &args[4];
    ACTIVATIONS.lock().unwrap().push(Activation {
        id: args[0]["value"].as_str().unwrap_or_default().to_owned(),
        startup: reason["startup"] == true,
        code_loading: args[1].as_f64().unwrap_or_default(),
        activate_call: args[2].as_f64().unwrap_or_default(),
        activate_resolved: args[3].as_f64().unwrap_or_default(),
        event: reason["activationEvent"].as_str().unwrap_or_default().to_owned(),
        by: reason["extensionId"]["value"].as_str().unwrap_or_default().to_owned(),
    });
}

/// perfviewEditor.ts `_addExtensionsTable` rows: eager activations first.
pub fn activations() -> Vec<Vec<String>> {
    let activations = ACTIVATIONS.lock().unwrap();
    let row = |activation: &Activation| {
        let numbers = [activation.code_loading, activation.activate_call, activation.activate_resolved].map(|time| time.to_string());
        [vec![activation.id.clone(), activation.startup.to_string()], numbers.to_vec(), vec![activation.event.clone(), activation.by.clone()]].concat()
    };
    let eager = activations.iter().filter(|activation| activation.startup).map(row);
    let normal = activations.iter().filter(|activation| !activation.startup).map(row);
    eager.chain(normal).collect()
}
