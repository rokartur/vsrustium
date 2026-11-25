//! Workspace extension recommendations: workspaceExtensionsConfig.ts reads `extensions.json` of each folder and the
//! workspace file's `extensions` key, workspaceRecommendations.ts validates the ids, and
//! extensionIgnoredRecommendationsService.ts drops the unwanted and globally ignored ones.
//! Port decision: a folder's `extensions.json` is read from `.vscode/` and `.vsrustium/`, `.vsrustium/` winning per key.

use std::path::Path;

use config::json::{self, ParseOptions};
use config::service::FOLDER_CONFIG_DIRS;
use serde_json::Value;

/// extensionIgnoredRecommendationsService.ts `ignoredRecommendationsStorageKey` (profile storage).
pub const IGNORED_RECOMMENDATIONS_KEY: &str = "extensionsAssistant/ignored_recommendations";
/// extensionRecommendationNotificationService.ts `donotShowWorkspaceRecommendationsStorageKey` (workspace storage).
pub const DONOT_SHOW_WORKSPACE_RECOMMENDATIONS_KEY: &str = "extensionsAssistant/workspaceRecommendationsIgnore";

/// `IExtensionsConfigContent`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExtensionsConfig {
    pub recommendations: Option<Vec<String>>,
    pub unwanted_recommendations: Option<Vec<String>>,
}

/// `resolveWorkspaceFolderExtensionConfig`: `<folder>/.vscode/extensions.json`, then `.vsrustium/extensions.json` per key.
pub fn read_folder_config(folder: &Path) -> ExtensionsConfig {
    let mut config = ExtensionsConfig::default();
    for dir in FOLDER_CONFIG_DIRS {
        let Some(value) = read_json(&folder.join(dir).join("extensions.json")) else { continue };
        let read = config_from(&value);
        if read.recommendations.is_some() {
            config.recommendations = read.recommendations;
        }
        if read.unwanted_recommendations.is_some() {
            config.unwanted_recommendations = read.unwanted_recommendations;
        }
    }
    config
}

/// `resolveWorkspaceExtensionConfig`: the `.code-workspace` file's `extensions` object.
pub fn read_workspace_file_config(file: &Path) -> Option<ExtensionsConfig> {
    read_json(file)?.get("extensions").map(config_from)
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    json::parse(&text, &mut Vec::new(), ParseOptions { allow_trailing_comma: true, allow_empty_content: true, ..ParseOptions::default() })
}

/// `parseExtensionConfig`: only string entries of the two arrays count.
fn config_from(value: &Value) -> ExtensionsConfig {
    let strings = |key: &str| value.get(key).and_then(Value::as_array).map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect());
    ExtensionsConfig { recommendations: strings("recommendations"), unwanted_recommendations: strings("unwantedRecommendations") }
}

/// What `WorkspaceRecommendations.fetch` keeps: valid recommendations, the unwanted ids, and the warning for bad ids.
#[derive(Debug, Default, PartialEq)]
pub struct WorkspaceRecommendations {
    pub recommendations: Vec<String>,
    pub unwanted: Vec<String>,
    pub warning: Option<String>,
}

pub fn workspace_recommendations(configs: &[ExtensionsConfig]) -> WorkspaceRecommendations {
    let pattern = regex::Regex::new(r"^([a-z0-9A-Z][a-z0-9-A-Z]*)\.([a-z0-9A-Z][a-z0-9-A-Z]*)$").expect("EXTENSION_IDENTIFIER_PATTERN");
    let mut invalid: Vec<&str> = Vec::new();
    for id in configs.iter().flat_map(|config| config.recommendations.iter().flatten()) {
        if !pattern.is_match(id) && !invalid.contains(&id.as_str()) {
            invalid.push(id);
        }
    }
    let mut result = WorkspaceRecommendations::default();
    for config in configs {
        result.unwanted.extend(config.unwanted_recommendations.iter().flatten().filter(|id| !invalid.contains(&id.as_str())).map(|id| id.to_lowercase()));
        result.recommendations.extend(config.recommendations.iter().flatten().filter(|id| !invalid.contains(&id.as_str())).cloned());
    }
    if !invalid.is_empty() {
        let message: String = invalid.iter().map(|id| format!("{id} (bad format) Expected: <provider>.<name>\n")).collect();
        result.warning = Some(format!("The {} extension(s) below, in workspace recommendations have issues:\n{message}", invalid.len()));
    }
    result
}

/// `ExtensionRecommendationsService.getWorkspaceRecommendations`: distinct lowercase ids that are neither unwanted
/// by the workspace nor ignored globally.
pub fn allowed_recommendations(workspace: &WorkspaceRecommendations, globally_ignored: &[String]) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    for id in &workspace.recommendations {
        let id = id.to_lowercase();
        if !result.contains(&id) && !workspace.unwanted.contains(&id) && !globally_ignored.contains(&id) {
            result.push(id);
        }
    }
    result
}

/// The globally ignored ids as stored (a JSON array of ids).
pub fn parse_ignored(stored: Option<&str>) -> Vec<String> {
    stored.and_then(|text| serde_json::from_str::<Vec<String>>(text).ok()).unwrap_or_default().iter().map(|id| id.to_lowercase()).collect()
}

/// extensionRecommendationNotificationService.ts `promptRecommendationsNotification`: the message names the one
/// extension and its publisher, else up to two publishers.
pub fn prompt_message(extensions: &[(String, String)]) -> String {
    let what = match extensions {
        [(name, publisher)] => format!("'{name}' extension from {publisher}"),
        _ => {
            let mut publishers: Vec<&str> = Vec::new();
            for (_, publisher) in extensions {
                if !publishers.contains(&publisher.as_str()) {
                    publishers.push(publisher);
                }
            }
            match publishers.as_slice() {
                [one] => format!("extensions from {one}"),
                [first, second] => format!("extensions from {first} and {second}"),
                [first, second, ..] => format!("extensions from {first}, {second} and others"),
                [] => String::new(),
            }
        }
    };
    format!("Do you want to install the recommended {what} for this repository?")
}

#[cfg(test)]
mod tests;
