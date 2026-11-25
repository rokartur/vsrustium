//! `SettingsFileSystemProvider` (workbench/contrib/preferences/common/settingsFilesystemProvider.ts): the read-only
//! `vscode` scheme serving the JSON schemas of the contribution registry and their file associations.
// shortcut: `vscode://defaultsettings/...` content is built in the app crate, so only `openTextDocument` reaches it
// (host_requests.rs); `workspace.fs` reads of it fail until that content moves below the app crate.

use std::time::{SystemTime, UNIX_EPOCH};

use config::json_schema::{KEYBINDINGS_SCHEMA_ID, REFERENCED_SCHEMAS, compressed_content, keybindings_schema, settings_schemas};
use config::service::Settings;
use files::{FILE, FILE_NOT_FOUND, PROVIDER_READONLY, READONLY, Resource, Stat};

/// `FileSystemProviderCapabilities.Readonly + FileReadWrite`.
pub const CAPABILITIES: u32 = PROVIDER_READONLY + 2;

const SCHEMA_ASSOCIATIONS: &str = "vscode://schemas-associations/schemas-associations.json";

pub fn read_file(settings: &Settings, resource: &Resource) -> files::Result<Vec<u8>> {
    let content = match resource.authority.as_str() {
        // `getSchemaContent`: an unregistered schema is the empty schema.
        "schemas" => match referenced_schema(resource) {
            Some(content) => content.to_string(),
            None if uri_string(resource) == KEYBINDINGS_SCHEMA_ID => keybindings_schema(&settings.registry).to_string(),
            None => schema(settings, resource).map_or_else(|| "{}".to_string(), |schema| compressed_content(&schema)),
        },
        // Only chat registers schema associations, and the port has no chat.
        "schemas-associations" => r#"{"schemas":[]}"#.to_string(),
        _ => return Err(not_found(resource)),
    };
    Ok(content.into_bytes())
}

pub fn stat(settings: &Settings, resource: &Resource) -> files::Result<Stat> {
    let uri = uri_string(resource);
    if referenced_schema(resource).is_none() && schema(settings, resource).is_none() && uri != SCHEMA_ASSOCIATIONS && uri != KEYBINDINGS_SCHEMA_ID {
        return Err(not_found(resource));
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("after 1970").as_millis() as u64;
    Ok(Stat { file_type: FILE, ctime: now, mtime: now, size: 0, permissions: Some(READONLY) })
}

fn schema(settings: &Settings, resource: &Resource) -> Option<serde_json::Value> {
    let uri = uri_string(resource);
    let in_workspace = settings.workspace.as_ref().is_some_and(|workspace| workspace.file.is_some());
    settings_schemas(&settings.registry, in_workspace).into_iter().find(|(id, _)| *id == uri).map(|(_, schema)| schema)
}

fn referenced_schema(resource: &Resource) -> Option<&'static str> {
    let uri = uri_string(resource);
    REFERENCED_SCHEMAS.iter().find(|(id, _)| *id == uri).map(|(_, content)| *content)
}

fn uri_string(resource: &Resource) -> String {
    format!("{}://{}{}", resource.scheme, resource.authority, resource.path)
}

fn not_found(resource: &Resource) -> files::FileError {
    files::provider_error(FILE_NOT_FOUND, format!("{} not found", uri_string(resource)))
}
