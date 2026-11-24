//! mainThreadStorage.ts with the ExtensionStorageService calls it makes: each extension's `globalState` is one
//! JSON string under its id in the global `state.vscdb`, its `workspaceState` the same in the workspace's.
//! Left out: `$acceptValue` for shared keys another window changed (the port has one window) and the
//! storage migration between extension ids.

use std::io::Read;
use std::path::Path;
use std::time::SystemTime;

use config::workspace::Workspace;
use serde_json::{Value, json};
use storage::Storage;

use crate::rpc::{Arg, Reply};
use crate::session::{SessionPaths, workspace_id};
use crate::utc::utc_string;

pub struct MainThreadStorage {
    /// StorageScope.PROFILE, which is the application storage in the default profile.
    profile: Storage,
    workspace: Storage,
}

impl MainThreadStorage {
    /// Opens `User/globalStorage/state.vscdb` and `User/workspaceStorage/<id>/state.vscdb`, creating the
    /// folders. A window without a folder keeps its workspace state in memory.
    pub fn open(paths: &SessionPaths) -> std::io::Result<Self> {
        let global = paths.user_data.join("User/globalStorage");
        std::fs::create_dir_all(&global)?;
        let workspace = match paths.workspace {
            Some(workspace) => Storage::open(&workspace_storage(paths.user_data, workspace)?),
            None => Storage::in_memory(),
        };
        Ok(MainThreadStorage { profile: Storage::open(&global.join("state.vscdb")), workspace })
    }

    pub fn in_memory() -> Self {
        MainThreadStorage { profile: Storage::in_memory(), workspace: Storage::in_memory() }
    }

    /// `telemetryInfo` as the browser workbench builds it (workbenchCommonProperties.ts): machine id and first
    /// session date kept in application storage, a new session id per window. The port sends no telemetry;
    /// extensions read these through `vscode.env`.
    pub fn telemetry_info(&mut self) -> Value {
        let machine_id = self.get_or_store("telemetry.machineId", generate_uuid);
        let first_session_date = self.get_or_store("telemetry.firstSessionDate", || utc_string(SystemTime::now()));
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).expect("the clock is after 1970").as_millis();
        json!({
            "sessionId": format!("{}{now}", generate_uuid()),
            "machineId": machine_id,
            "sqmId": "",
            "devDeviceId": machine_id,
            "firstSessionDate": first_session_date,
        })
    }

    fn get_or_store(&mut self, key: &str, create: impl FnOnce() -> String) -> String {
        if let Some(value) = self.profile.get(key) {
            return value.to_owned();
        }
        let value = create();
        self.profile.set(key, value.clone());
        value
    }

    pub fn serve(&mut self, method: &str, args: &[Arg]) -> Reply {
        let arg = |index: usize| match args.get(index) {
            Some(Arg::Json(value)) => value,
            _ => &Value::Null,
        };
        let shared = arg(0).as_bool() == Some(true);
        let storage = if shared { &mut self.profile } else { &mut self.workspace };
        match method {
            "$initializeExtensionStorage" => {
                let extension_id = arg(1).as_str().expect("an extension id");
                storage.get(extension_id).map_or(Reply::Empty, |raw| Reply::Json(Value::String(raw.to_owned())))
            }
            "$setValue" => {
                let extension_id = arg(1).as_str().expect("an extension id");
                match arg(2) {
                    Value::Null => storage.delete(extension_id),
                    state => storage.set(extension_id, state.to_string()),
                }
                Reply::Empty
            }
            "$registerExtensionStorageKeysToSync" => {
                let extension = arg(0);
                let id = extension["id"].as_str().expect("an extension id").to_lowercase();
                let key = format!("extensionKeys/{id}@{}", extension["version"].as_str().unwrap_or_default());
                self.profile.set(&key, arg(1).to_string());
                Reply::Empty
            }
            _ => Reply::Empty,
        }
    }
}

/// `generateUuid()`: a version 4 UUID.
fn generate_uuid() -> String {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut random| random.read_exact(&mut bytes)).expect("/dev/urandom is readable");
    bytes[6] = bytes[6] & 0x0f | 0x40;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// Upstream also writes `workspace.json` next to the database for its cleanup of stale workspace storage,
/// which the port does not have.
fn workspace_storage(user_data: &Path, workspace: &Workspace) -> std::io::Result<std::path::PathBuf> {
    let dir = user_data.join("User/workspaceStorage").join(workspace_id(workspace));
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("state.vscdb"))
}
