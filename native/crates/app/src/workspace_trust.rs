//! workspaceTrust.ts `WorkspaceTrustManagementService` for `file` workspaces: the trusted folders in the shared
//! storage (`content.trust.model.key`), the trust of the window's folders and workspace file, and the
//! `workspaceTrust` memento of an empty window.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use config::workspace::Workspace;
use futures::channel::oneshot;
use exthost::scan::ScannedExtension;
use serde_json::{Value, json};
use storage::Storage;

use crate::editor_history::{file_path, file_uri_string};

pub const STORAGE_KEY: &str = "content.trust.model.key";
const MEMENTO_KEY: &str = "memento/workspaceTrust";
/// workspace.contribution.ts `STARTUP_PROMPT_SHOWN_KEY` and `BANNER_RESTRICTED_MODE_DISMISSED_KEY`.
pub const STARTUP_PROMPT_SHOWN_KEY: &str = "workspace.trust.startupPrompt.shown";
pub const BANNER_DISMISSED_KEY: &str = "workbench.banner.restrictedMode.dismissed";

pub struct WorkspaceTrust {
    /// `isWorkspaceTrustEnabled`: the setting and `--disable-workspace-trust`.
    pub enabled: bool,
    /// The APPLICATION_SHARED storage, `sharedStorage/state.vscdb` of the shared data folder.
    shared: Arc<Mutex<Storage>>,
    /// `uriTrustInfo` as stored, trusted entries only; entries of other schemes are kept as they are.
    entries: Vec<Value>,
    /// `getWorkspaceUris`: the folders, then the workspace file.
    workspace_uris: Vec<PathBuf>,
    trusted: bool,
    /// WorkspaceTrustRequestService's pending `requestWorkspaceTrust` promise, shared by every caller.
    pub requests: Vec<oneshot::Sender<Result<Value, Value>>>,
    /// The startup prompt is open: the banner waits for its answer, as `updateWorkbenchIndicators` does.
    pub startup_prompt_open: bool,
}

/// What `calculateWorkspaceTrust` reads besides the trusted folders.
pub struct Startup<'a> {
    pub enabled: bool,
    pub workspace: Option<&'a Workspace>,
    /// `security.workspace.trust.emptyWindow`.
    pub empty_window_trusted: bool,
    /// The window's workspace storage, for the memento.
    pub workspace_storage: &'a mut Storage,
}

/// A window whose trust is never asked for, until main hands the workbench the real one.
impl Default for WorkspaceTrust {
    fn default() -> WorkspaceTrust {
        WorkspaceTrust { enabled: false, shared: Arc::new(Mutex::new(Storage::in_memory())), entries: Vec::new(), workspace_uris: Vec::new(), trusted: true, requests: Vec::new(), startup_prompt_open: false }
    }
}

impl WorkspaceTrust {
    pub fn new(shared: Arc<Mutex<Storage>>, startup: Startup) -> WorkspaceTrust {
        let entries = load_entries(&shared.lock().unwrap());
        let mut workspace_uris: Vec<PathBuf> = startup.workspace.iter().flat_map(|workspace| workspace.folders.iter().map(|folder| folder.path.clone())).collect();
        workspace_uris.extend(startup.workspace.and_then(|workspace| workspace.file.clone()));
        let mut trust = WorkspaceTrust { enabled: startup.enabled, shared, entries, workspace_uris, trusted: false, requests: Vec::new(), startup_prompt_open: false };
        trust.trusted = trust.calculate(startup.workspace_storage, startup.empty_window_trusted);
        if trust.enabled && trust.is_empty(startup.workspace_storage) && memento(startup.workspace_storage)["isEmptyWorkspaceTrusted"].is_null() {
            let trusted = trust.trusted;
            write_memento(startup.workspace_storage, "isEmptyWorkspaceTrusted", trusted);
        }
        trust
    }

    pub fn is_trusted(&self) -> bool {
        self.trusted
    }

    /// `isEmptyWorkspace`: an empty window, or one that accepted files from outside its folders.
    pub fn is_empty(&self, workspace_storage: &Storage) -> bool {
        self.workspace_uris.is_empty() || memento(workspace_storage)["acceptsOutOfWorkspaceFiles"] == true
    }

    /// `doGetUriTrustInfo`: the longest trusted folder that contains `path`, compared ignoring case as the macOS disk is.
    pub fn uri_trust_info(&self, path: &Path) -> (bool, PathBuf) {
        if !self.enabled {
            return (true, path.to_owned());
        }
        let key = comparison_key(path);
        self.trusted_paths()
            .filter(|trusted| key.starts_with(&comparison_key(trusted)))
            .max_by_key(|trusted| trusted.as_os_str().len())
            .map_or((false, path.to_owned()), |trusted| (true, trusted))
    }

    /// The trusted `file` folders, as the Workspace Trust editor lists them.
    pub fn trusted_paths(&self) -> impl Iterator<Item = PathBuf> + '_ {
        self.entries.iter().filter_map(entry_path)
    }

    /// `canSetParentFolderTrust`: a single folder window whose folder has a parent.
    pub fn parent_folder(&self) -> Option<&Path> {
        let [folder] = self.workspace_uris.as_slice() else { return None };
        folder.parent()
    }

    /// `setWorkspaceTrust`: the empty window's memento, else the trust of every workspace URI. True when the
    /// window's trust changed.
    pub fn set_workspace_trust(&mut self, trusted: bool, workspace_storage: &mut Storage, empty_window_trusted: bool) -> bool {
        if self.is_empty(workspace_storage) {
            return self.update(Some(trusted), workspace_storage, empty_window_trusted);
        }
        let uris = self.workspace_uris.clone();
        self.set_uris_trust(&uris, trusted, workspace_storage, empty_window_trusted)
    }

    /// `setParentFolderTrust`.
    pub fn set_parent_folder_trust(&mut self, trusted: bool, workspace_storage: &mut Storage, empty_window_trusted: bool) -> bool {
        let Some(parent) = self.parent_folder().map(Path::to_owned) else { return false };
        self.set_uris_trust(&[parent], trusted, workspace_storage, empty_window_trusted)
    }

    /// `doSetUrisTrust`, then `saveTrustInfo` when an entry was added or removed. True when the window's trust changed.
    pub fn set_uris_trust(&mut self, paths: &[PathBuf], trusted: bool, workspace_storage: &mut Storage, empty_window_trusted: bool) -> bool {
        let mut changed = false;
        for path in paths {
            let key = comparison_key(path);
            let same = |entry: &Value| entry_path(entry).is_some_and(|stored| comparison_key(&stored) == key);
            if trusted && !self.entries.iter().any(same) {
                self.entries.push(json!({ "uri": uri_json(path), "trusted": true }));
                changed = true;
            } else if !trusted {
                let before = self.entries.len();
                self.entries.retain(|entry| !same(entry));
                changed |= before != self.entries.len();
            }
        }
        if !changed {
            return false;
        }
        self.shared.lock().unwrap().set(STORAGE_KEY, json!({ "uriTrustInfo": self.entries }).to_string());
        self.update(None, workspace_storage, empty_window_trusted)
    }

    /// `updateWorkspaceTrust`: recalculates (or takes) the trust; the `isTrusted` setter writes the memento.
    fn update(&mut self, trusted: Option<bool>, workspace_storage: &mut Storage, empty_window_trusted: bool) -> bool {
        if !self.enabled {
            return false;
        }
        let trusted = trusted.unwrap_or_else(|| self.calculate(workspace_storage, empty_window_trusted));
        if trusted == self.trusted {
            return false;
        }
        self.trusted = trusted;
        if !trusted {
            write_memento(workspace_storage, "acceptsOutOfWorkspaceFiles", false);
        }
        if self.is_empty(workspace_storage) {
            write_memento(workspace_storage, "isEmptyWorkspaceTrusted", trusted);
        }
        true
    }

    /// `calculateWorkspaceTrust` without remote authorities and startup files.
    fn calculate(&self, workspace_storage: &Storage, empty_window_trusted: bool) -> bool {
        if !self.enabled {
            return true;
        }
        if self.is_empty(workspace_storage) {
            return memento(workspace_storage)["isEmptyWorkspaceTrusted"].as_bool().unwrap_or(empty_window_trusted);
        }
        self.workspace_uris.iter().all(|uri| self.uri_trust_info(uri).0)
    }
}

/// extensionEnablementService.ts `_isDisabledByWorkspaceTrust` in an untrusted window: an extension inside the
/// workspace, or one whose `getExtensionUntrustedWorkspaceSupportType` is `false`. `configured` is the
/// `extensions.supportUntrustedWorkspaces` setting; the product overrides none.
pub fn disabled_when_untrusted(extension: &ScannedExtension, workspace: Option<&Workspace>, configured: &Value) -> bool {
    if workspace.is_some_and(|workspace| workspace.folders.iter().any(|folder| extension.location.starts_with(&folder.path))) {
        return true;
    }
    let manifest = &extension.description;
    if manifest["main"].is_null() && manifest["browser"].is_null() {
        return false;
    }
    let configured = configured.as_object().into_iter().flatten().find(|(id, _)| id.eq_ignore_ascii_case(&extension.id)).map(|(_, value)| value);
    if let Some(configured) = configured.filter(|configured| configured["version"].is_null() || configured["version"] == manifest["version"]) {
        return configured["supported"] == false;
    }
    let supported = &manifest["capabilities"]["untrustedWorkspaces"]["supported"];
    supported.is_null() || *supported == false
}

/// `loadTrustInfo`: unparsable storage counts as no entries; untrusted entries are dropped.
fn load_entries(shared: &Storage) -> Vec<Value> {
    let stored: Value = shared.get(STORAGE_KEY).and_then(|value| serde_json::from_str(value).ok()).unwrap_or_default();
    let entries = stored["uriTrustInfo"].as_array().cloned().unwrap_or_default();
    entries.into_iter().filter(|entry| entry["trusted"] == true).collect()
}

fn entry_path(entry: &Value) -> Option<PathBuf> {
    let uri = &entry["uri"];
    match uri {
        Value::String(uri) => file_path(uri),
        uri if uri["scheme"] == "file" => uri["path"].as_str().map(PathBuf::from),
        _ => None,
    }
}

/// `JSON.stringify` of a `file` URI: `URI.toJSON`.
fn uri_json(path: &Path) -> Value {
    let path_string = path.to_string_lossy();
    json!({ "$mid": 1, "fsPath": path_string, "external": file_uri_string(path), "path": path_string, "scheme": "file" })
}

fn memento(workspace_storage: &Storage) -> Value {
    workspace_storage.get(MEMENTO_KEY).and_then(|value| serde_json::from_str(value).ok()).unwrap_or_else(|| json!({}))
}

fn write_memento(workspace_storage: &mut Storage, key: &str, value: bool) {
    let mut memento = memento(workspace_storage);
    memento[key] = json!(value);
    workspace_storage.set(MEMENTO_KEY, memento.to_string());
}

/// extUri `isEqualOrParent` with ignored path casing, as a prefix that ends with a separator.
fn comparison_key(path: &Path) -> String {
    let mut key = path.to_string_lossy().to_lowercase();
    if !key.ends_with('/') {
        key.push('/');
    }
    key
}
