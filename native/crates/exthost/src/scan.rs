//! Builds `IExtensionDescription`s from extension folders the way the built-in scanner does:
//! package.json with its `%key%` strings replaced from package.nls.json.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

#[derive(Clone)]
pub struct ScannedExtension {
    pub id: String,
    pub location: PathBuf,
    pub description: Value,
}

impl ScannedExtension {
    pub fn has_code(&self) -> bool {
        self.description.get("main").is_some()
    }
}

/// Built-in extension folders the port does not ship: sign-in, AI, and Microsoft tunnels (port/notes.md).
pub const REMOVED_BUILTINS: [&str; 5] = ["copilot", "github-authentication", "microsoft-authentication", "prompt-basics", "tunnel-forwarding"];
/// Upstream's product leaves these out too (build/lib/extensions.ts excludedExtensions); api-tests load theirs as development extensions.
const TEST_BUILTINS: [&str; 4] = ["vscode-api-tests", "vscode-colorize-tests", "vscode-colorize-perf-tests", "vscode-test-resolver"];

/// Scans every direct child folder of `dir` that holds a package.json, except `REMOVED_BUILTINS` and `TEST_BUILTINS`.
pub fn scan_builtin_extensions(dir: &Path) -> anyhow::Result<Vec<ScannedExtension>> {
    let mut folders: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.join("package.json").is_file())
        .filter(|path| !path.file_name().is_some_and(|name| REMOVED_BUILTINS.iter().chain(&TEST_BUILTINS).any(|skipped| name == *skipped)))
        .collect();
    folders.sort();
    folders.iter().map(|folder| scan_extension(folder, true)).collect()
}

/// extensionsScannerService.ts `scanDevSystemExtensions`: product.json `builtInExtensions` (js-debug and its
/// companions) from `.build/builtInExtensions`, where port/tools/fetch-builtin-extensions puts them.
pub fn scan_dev_system_extensions(app_root: &Path) -> anyhow::Result<Vec<ScannedExtension>> {
    let product: Value = serde_json::from_slice(&std::fs::read(app_root.join("product.json"))?)?;
    let names = product["builtInExtensions"].as_array().into_iter().flatten().filter_map(|extension| extension["name"].as_str());
    let folders = names.map(|name| app_root.join(".build/builtInExtensions").join(name)).filter(|folder| folder.join("package.json").is_file());
    folders.map(|folder| scan_extension(&folder, true)).collect()
}

pub fn scan_extension(folder: &Path, is_builtin: bool) -> anyhow::Result<ScannedExtension> {
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(folder.join("package.json"))?)?;
    if let Ok(nls) = std::fs::read(folder.join("package.nls.json")) {
        let messages: Map<String, Value> = serde_json::from_slice(&nls)?;
        replace_nls_placeholders(&mut manifest, &messages);
    }
    let publisher = manifest["publisher"].as_str().unwrap_or("undefined_publisher");
    let name = manifest["name"].as_str().ok_or_else(|| anyhow::anyhow!("{} has no name", folder.display()))?;
    let id = format!("{publisher}.{name}");

    let object = manifest.as_object_mut().expect("package.json is an object");
    object.insert("id".into(), json!(id));
    object.insert("identifier".into(), extension_identifier(&id));
    object.insert("isBuiltin".into(), json!(is_builtin));
    object.insert("isUserBuiltin".into(), json!(false));
    object.insert("isUnderDevelopment".into(), json!(false));
    object.insert("extensionLocation".into(), file_uri(folder));
    object.insert("targetPlatform".into(), json!("undefined"));
    object.insert("preRelease".into(), json!(false));

    Ok(ScannedExtension { id, location: folder.to_path_buf(), description: manifest })
}

/// `ExtensionIdentifier` as it serializes: `{ value, _lower }`.
pub fn extension_identifier(id: &str) -> Value {
    json!({ "value": id, "_lower": id.to_lowercase() })
}

/// `UriComponents` of a file path, marked as a URI for revival on the other side.
pub fn file_uri(path: &Path) -> Value {
    json!({ "$mid": 1, "scheme": "file", "authority": "", "path": path.to_string_lossy(), "query": "", "fragment": "" })
}

/// Replaces string values of the form `%key%` with the message from package.nls.json,
/// which is either a string or `{ "message": string, "comment": [...] }`.
fn replace_nls_placeholders(value: &mut Value, messages: &Map<String, Value>) {
    match value {
        Value::String(text) => {
            if let Some(key) = text.strip_prefix('%').and_then(|rest| rest.strip_suffix('%')) {
                let message = match messages.get(key) {
                    Some(Value::String(message)) => Some(message.clone()),
                    Some(Value::Object(entry)) => entry.get("message").and_then(Value::as_str).map(str::to_owned),
                    _ => None,
                };
                if let Some(message) = message {
                    *text = message;
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| replace_nls_placeholders(item, messages)),
        Value::Object(fields) => fields.values_mut().for_each(|field| replace_nls_placeholders(field, messages)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_string_and_object_messages() {
        let messages = json!({ "a": "Alpha", "b": { "message": "Beta", "comment": ["x"] } });
        let mut manifest = json!({ "displayName": "%a%", "contributes": { "commands": [{ "title": "%b%" }, { "title": "%missing%" }] } });
        replace_nls_placeholders(&mut manifest, messages.as_object().unwrap());
        assert_eq!(manifest, json!({ "displayName": "Alpha", "contributes": { "commands": [{ "title": "Beta" }, { "title": "%missing%" }] } }));
    }

    #[test]
    fn skips_removed_builtins() {
        let dir = std::env::temp_dir().join(format!("scan-removed-{}", std::process::id()));
        for name in ["copilot", "git", "vscode-api-tests"] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
            std::fs::write(dir.join(name).join("package.json"), format!(r#"{{"name":"{name}","publisher":"vscode"}}"#)).unwrap();
        }
        let ids: Vec<String> = scan_builtin_extensions(&dir).unwrap().into_iter().map(|e| e.id).collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(ids, ["vscode.git"]);
    }
}
