//! The kernels of mainThreadNotebookKernels.ts with notebookKernelServiceImpl.ts (`getMatchingKernel`, the notebook
//! bindings) and the cell executions of notebookExecutionStateServiceImpl.ts.

use std::collections::HashMap;

use serde_json::{Value, json};

/// `NotebookCellExecutionState`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExecutionState {
    Unconfirmed,
    Pending,
    Executing,
}

#[derive(Clone, Debug)]
pub struct Kernel {
    pub handle: u64,
    /// `${extensionId}/${controllerId}`.
    pub id: String,
    pub notebook_type: String,
    pub label: String,
    pub description: Option<String>,
    pub detail: Option<String>,
    /// Empty: every registered language.
    pub supported_languages: Vec<String>,
    pub supports_interrupt: bool,
    /// `preloadUris`: the controller's `NotebookRendererScript` URIs, which its notebooks' webviews load.
    pub preloads: Vec<Value>,
    /// `localResourceRoot`: the extension's folder, which the webviews may load from while the kernel is theirs.
    pub extension_location: Value,
    pub has_variable_provider: bool,
    /// `$updateNotebookPriority` per notebook URI string.
    priorities: HashMap<String, i64>,
}

#[derive(Clone, Debug)]
pub struct Execution {
    pub notebook: String,
    pub cell_handle: u64,
    pub state: ExecutionState,
}

/// A change of a notebook's bound kernel, for `$acceptNotebookAssociation`.
#[derive(Debug, PartialEq)]
pub struct Association {
    pub kernel_handle: u64,
    pub notebook: Value,
    pub selected: bool,
}

#[derive(Default)]
pub struct Kernels {
    pub kernels: Vec<Kernel>,
    /// `_notebookBindings`: `${viewType}/${uri}` to a kernel id. A binding outlives its kernel and binds again when a
    /// kernel of that id comes back.
    bindings: HashMap<String, (String, Value)>,
    /// Cell executions by `$createExecution` handle.
    pub executions: HashMap<u64, Execution>,
    /// Cells with a running execution: (notebook URI string, cell handle).
    pub running: Vec<(String, u64)>,
    /// `variableRequestMap`: the variables `$receiveVariable` brought per running `$provideVariables` request id.
    pub variable_requests: HashMap<String, Vec<Value>>,
    pub variable_request_index: u64,
}

fn binding_key(view_type: &str, notebook: &str) -> String {
    format!("{view_type}/{notebook}")
}

impl Kernels {
    /// `$addKernel`; notebooks bound to a kernel of its id bind to it (`_tryAutoBindNotebook`).
    pub fn add(&mut self, handle: u64, data: &Value) -> Vec<Association> {
        let text = |key: &str| data[key].as_str().map(str::to_owned);
        let kernel = Kernel {
            handle,
            id: text("id").unwrap_or_default(),
            notebook_type: text("notebookType").unwrap_or_default(),
            label: text("label").unwrap_or_default(),
            description: text("description"),
            detail: text("detail"),
            supported_languages: languages(&data["supportedLanguages"]),
            supports_interrupt: data["supportsInterrupt"] == true,
            preloads: data["preloads"].as_array().into_iter().flatten().map(|preload| preload["uri"].clone()).collect(),
            extension_location: data["extensionLocation"].clone(),
            has_variable_provider: data["hasVariableProvider"] == true,
            priorities: HashMap::new(),
        };
        let associations = self.bindings.values().filter(|(id, _)| *id == kernel.id).map(|(_, notebook)| Association { kernel_handle: handle, notebook: notebook.clone(), selected: true }).collect();
        self.kernels.push(kernel);
        associations
    }

    /// `_tryAutoBindNotebook` of an added notebook: a kernel it is bound to (preselected) hears of it.
    pub fn notebook_added(&self, view_type: &str, notebook: &str) -> Vec<Association> {
        let Some((id, uri)) = self.bindings.get(&binding_key(view_type, notebook)) else { return Vec::new() };
        let (all, _) = self.matching(view_type, notebook);
        all.iter().filter(|kernel| kernel.id == *id).map(|kernel| Association { kernel_handle: kernel.handle, notebook: uri.clone(), selected: true }).collect()
    }

    /// `$updateKernel`.
    pub fn update(&mut self, handle: u64, data: &Value) {
        let Some(kernel) = self.kernels.iter_mut().find(|kernel| kernel.handle == handle) else { return };
        if let Some(label) = data["label"].as_str() {
            kernel.label = label.to_owned();
        }
        if !data["description"].is_null() {
            kernel.description = data["description"].as_str().map(str::to_owned);
        }
        if !data["detail"].is_null() {
            kernel.detail = data["detail"].as_str().map(str::to_owned);
        }
        if data.get("supportedLanguages").is_some_and(|languages| !languages.is_null()) {
            kernel.supported_languages = languages(&data["supportedLanguages"]);
        }
        if let Some(interrupt) = data["supportsInterrupt"].as_bool() {
            kernel.supports_interrupt = interrupt;
        }
        if let Some(has_variable_provider) = data["hasVariableProvider"].as_bool() {
            kernel.has_variable_provider = has_variable_provider;
        }
    }

    /// `$removeKernel`: its notebooks hear they are no longer bound; the bindings stay.
    pub fn remove(&mut self, handle: u64) -> Vec<Association> {
        let Some(index) = self.kernels.iter().position(|kernel| kernel.handle == handle) else { return Vec::new() };
        let kernel = self.kernels.remove(index);
        self.bindings.values().filter(|(id, _)| *id == kernel.id).map(|(_, notebook)| Association { kernel_handle: handle, notebook: notebook.clone(), selected: false }).collect()
    }

    pub fn update_priority(&mut self, handle: u64, notebook: &str, value: Option<i64>) {
        let Some(kernel) = self.kernels.iter_mut().find(|kernel| kernel.handle == handle) else { return };
        match value {
            Some(value) => _ = kernel.priorities.insert(notebook.to_owned(), value),
            None => _ = kernel.priorities.remove(notebook),
        }
    }

    /// `getMatchingKernel`: the kernels for the notebook, by affinity, then type match, then label; and the bound one.
    pub fn matching(&self, view_type: &str, notebook: &str) -> (Vec<&Kernel>, Option<&Kernel>) {
        let score = |kernel: &Kernel| match kernel.notebook_type.as_str() {
            "*" => 5,
            notebook_type if notebook_type == view_type => 10,
            _ => 0,
        };
        let affinity = |kernel: &Kernel| kernel.priorities.get(notebook).copied().unwrap_or(1);
        let mut all: Vec<&Kernel> = self.kernels.iter().filter(|kernel| score(kernel) > 0).collect();
        all.sort_by(|a, b| affinity(b).cmp(&affinity(a)).then(score(a).cmp(&score(b))).then(a.label.cmp(&b.label)));
        let selected = self.bindings.get(&binding_key(view_type, notebook)).and_then(|(id, _)| all.iter().copied().find(|kernel| kernel.id == *id));
        (all, selected)
    }

    /// notebookKernelHistoryServiceImpl.ts `getKernels(..).selected`: the bound kernel, else the only one.
    pub fn resolve(&self, view_type: &str, notebook: &str) -> Option<&Kernel> {
        let (all, selected) = self.matching(view_type, notebook);
        selected.or(match all[..] {
            [only] => Some(only),
            _ => None,
        })
    }

    /// `selectKernelForNotebook`: the old kernel and the new one hear of the change.
    pub fn select(&mut self, kernel_id: Option<&str>, view_type: &str, notebook_uri: &Value) -> Vec<Association> {
        let notebook = super::uri_string(notebook_uri);
        let key = binding_key(view_type, &notebook);
        let old = self.bindings.get(&key).map(|(id, _)| id.clone());
        if old.as_deref() == kernel_id {
            return Vec::new();
        }
        match kernel_id {
            Some(id) => _ = self.bindings.insert(key, (id.to_owned(), notebook_uri.clone())),
            None => _ = self.bindings.remove(&key),
        }
        let mut associations = Vec::new();
        for kernel in &self.kernels {
            if old.as_deref() == Some(kernel.id.as_str()) {
                associations.push(Association { kernel_handle: kernel.handle, notebook: notebook_uri.clone(), selected: false });
            } else if kernel_id == Some(kernel.id.as_str()) {
                associations.push(Association { kernel_handle: kernel.handle, notebook: notebook_uri.clone(), selected: true });
            }
        }
        associations
    }

    /// `createCellExecution` for a cell that has none: unconfirmed until the kernel confirms it.
    pub fn start_cell(&mut self, notebook: &str, cell_handle: u64) -> bool {
        if self.running.iter().any(|(uri, handle)| uri == notebook && *handle == cell_handle) {
            return false;
        }
        self.running.push((notebook.to_owned(), cell_handle));
        true
    }

    /// The cell's execution ended.
    pub fn finish_cell(&mut self, notebook: &str, cell_handle: u64) {
        self.running.retain(|(uri, handle)| !(uri == notebook && *handle == cell_handle));
    }

    pub fn state(&self, notebook: &str, cell_handle: u64) -> Option<ExecutionState> {
        let running = self.running.iter().any(|(uri, handle)| uri == notebook && *handle == cell_handle);
        let execution = self.executions.values().find(|execution| execution.notebook == notebook && execution.cell_handle == cell_handle);
        running.then(|| execution.map_or(ExecutionState::Unconfirmed, |execution| execution.state))
    }
}

fn languages(value: &Value) -> Vec<String> {
    value.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
}

/// `$acceptNotebookAssociation` arguments.
pub fn association_args(association: &Association) -> Vec<Value> {
    vec![json!(association.kernel_handle), association.notebook.clone(), json!(association.selected)]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn uri(path: &str) -> Value {
        json!({ "scheme": "file", "authority": "", "path": path, "query": "", "fragment": "" })
    }

    #[test]
    fn a_binding_outlives_its_kernel_and_binds_the_next_of_its_id() {
        let mut kernels = Kernels::default();
        kernels.add(0, &json!({ "id": "ext/main", "notebookType": "nb", "label": "Main" }));
        kernels.add(1, &json!({ "id": "ext/other", "notebookType": "nb", "label": "Other" }));
        assert!(kernels.resolve("nb", "file:///a.nb").is_none(), "two kernels, none bound");
        let associations = kernels.select(Some("ext/main"), "nb", &uri("/a.nb"));
        assert_eq!(associations, [Association { kernel_handle: 0, notebook: uri("/a.nb"), selected: true }]);
        assert_eq!(kernels.resolve("nb", "file:///a.nb").map(|kernel| kernel.handle), Some(0));
        assert_eq!(kernels.remove(0), [Association { kernel_handle: 0, notebook: uri("/a.nb"), selected: false }]);
        assert_eq!(kernels.resolve("nb", "file:///a.nb").map(|kernel| kernel.handle), Some(1), "the only kernel left");
        assert_eq!(kernels.add(2, &json!({ "id": "ext/main", "notebookType": "nb", "label": "Main" })), [Association { kernel_handle: 2, notebook: uri("/a.nb"), selected: true }]);
        let associations = kernels.select(Some("ext/other"), "nb", &uri("/a.nb"));
        assert_eq!(associations.len(), 2);
    }

    #[test]
    fn other_notebook_types_do_not_match() {
        let mut kernels = Kernels::default();
        kernels.add(0, &json!({ "id": "ext/main", "notebookType": "other", "label": "Main" }));
        assert!(kernels.resolve("nb", "file:///a.nb").is_none());
        kernels.add(1, &json!({ "id": "ext/any", "notebookType": "*", "label": "Any" }));
        assert_eq!(kernels.resolve("nb", "file:///a.nb").map(|kernel| kernel.handle), Some(1));
    }
}
