//! Settings import (port feature, Artur; no upstream counterpart): settings, keybindings, snippets and the extension
//! list of a VS Code, Cursor or VSCodium user data folder, written into the current profile. Settings and
//! extensions of what the port removed by decision (AI, sign-in, Microsoft services, telemetry, remote) are left out.

use std::path::{Path, PathBuf};

use config::configuration::Configuration;
use config::json::{self, ParseOptions, Segment};
use config::json_edit::{apply_edits, set_property};
use config::service::{formatting_options, write_user_data_file};
use serde_json::{Map, Value};

/// An editor whose user data can be imported.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub name: &'static str,
    /// `<user data>/User`.
    pub user_dir: PathBuf,
    /// The extensions folder with its `extensions.json`.
    pub extensions_dir: PathBuf,
}

/// The editors the import looks for in their macOS locations under `home`.
pub fn sources(home: &Path) -> Vec<Source> {
    let support = home.join("Library/Application Support");
    let source = |name, folder: &str, extensions: &str| Source { name, user_dir: support.join(folder).join("User"), extensions_dir: home.join(extensions).join("extensions") };
    let all = [source("VS Code", "Code", ".vscode"), source("Cursor", "Cursor", ".cursor"), source("VSCodium", "VSCodium", ".vscode-oss")];
    all.into_iter().filter(|source| source.user_dir.is_dir() || source.extensions_dir.is_dir()).collect()
}

/// What one source holds that the port takes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Import {
    pub settings: Map<String, Value>,
    /// The `keybindings.json` entries.
    pub keybindings: Vec<Value>,
    /// Snippet files by their name in the snippets folder.
    pub snippets: Vec<(String, PathBuf)>,
    /// Gallery extension ids, lowercased.
    pub extensions: Vec<String>,
}

/// Setting prefixes of removed features.
const REMOVED_SETTINGS: [&str; 16] = [
    "chat.",
    "inlineChat.",
    "github.copilot",
    "copilot.",
    "mcp",
    "telemetry.",
    "remote.",
    "remoteTunnels.",
    "settingsSync.",
    "accounts.",
    "workbench.experimental.chat",
    "cursor.",
    "cursorai.",
    "aiFeatures.",
    "notebook.experimental.generate",
    "workbench.settings.showAISearchToggle",
];

/// Extension id prefixes of removed features: AI, remote, Live Share and the editors' own extensions.
const REMOVED_EXTENSIONS: [&str; 10] = [
    "github.copilot",
    "ms-vscode-remote.",
    "ms-vscode.remote-",
    "ms-vsliveshare.",
    "ms-azuretools.vscode-azure-github-copilot",
    "visualstudioexptteam.",
    "anysphere.",
    "ms-toolsai.vscode-ai",
    "ms-vscode.vscode-speech",
    "github.remotehub",
];

pub fn is_removed_setting(key: &str) -> bool {
    REMOVED_SETTINGS.iter().any(|prefix| key.starts_with(prefix))
}

pub fn is_removed_extension(id: &str) -> bool {
    REMOVED_EXTENSIONS.iter().any(|prefix| id.starts_with(prefix))
}

/// Reads `source`. Language override blocks keep only their settings the port has.
pub fn read(source: &Source) -> Import {
    let mut import = Import::default();
    if let Some(Value::Object(settings)) = parse(&source.user_dir.join("settings.json")) {
        for (key, value) in settings {
            if is_removed_setting(&key) {
                continue;
            }
            let value = match value {
                Value::Object(block) if key.starts_with('[') => Value::Object(block.into_iter().filter(|(key, _)| !is_removed_setting(key)).collect()),
                value => value,
            };
            import.settings.insert(key, value);
        }
    }
    if let Some(Value::Array(keybindings)) = parse(&source.user_dir.join("keybindings.json")) {
        let removed = |entry: &Value| entry["command"].as_str().is_some_and(|command| command.trim_start_matches('-').starts_with("github.copilot") || command.contains(".chat.") || command.starts_with("inlineChat."));
        import.keybindings = keybindings.into_iter().filter(|entry| !removed(entry)).collect();
    }
    for entry in std::fs::read_dir(source.user_dir.join("snippets")).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|extension| extension == "json" || extension == "code-snippets") {
            import.snippets.push((entry.file_name().to_string_lossy().into_owned(), path));
        }
    }
    import.snippets.sort();
    import.extensions = extension_ids(&source.extensions_dir).into_iter().filter(|id| !is_removed_extension(id)).collect();
    import
}

/// The ids of `extensions.json`, else of the folder names (`<publisher>.<name>-<version>`).
fn extension_ids(folder: &Path) -> Vec<String> {
    let mut ids: Vec<String> = match parse(&folder.join("extensions.json")) {
        Some(Value::Array(entries)) => entries.iter().filter_map(|entry| entry["identifier"]["id"].as_str().map(str::to_lowercase)).collect(),
        _ => std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().join("package.json").is_file())
            .filter_map(|entry| entry.file_name().to_string_lossy().rsplit_once('-').map(|(id, _)| id.to_lowercase()))
            .collect(),
    };
    ids.sort();
    ids.dedup();
    ids
}

/// What `write` changed, for the report.
#[derive(Debug, Default, PartialEq)]
pub struct Written {
    pub settings: usize,
    pub keybindings: usize,
    pub snippets: usize,
}

/// The profile files the import writes.
pub struct Target<'a> {
    pub settings: &'a Path,
    pub keybindings: &'a Path,
    pub snippets_home: &'a Path,
}

/// Writes `import` into the profile: each setting set in its settings.json (comments and other settings kept),
/// the keybindings not there yet appended, the snippet files the profile does not have copied.
pub fn write(import: &Import, target: &Target, configuration: &Configuration) -> Result<Written, String> {
    let mut written = Written::default();
    if !import.settings.is_empty() {
        let mut text = std::fs::read_to_string(target.settings).unwrap_or_else(|_| "{}".into());
        let options = formatting_options(&text, configuration, target.settings);
        for (key, value) in &import.settings {
            let edits = set_property(&text, &[Segment::Key(key.clone())], Some(value), &options);
            text = apply_edits(&text, &edits);
            written.settings += 1;
        }
        save(target.settings, &text)?;
    }
    if !import.keybindings.is_empty() {
        let mut text = std::fs::read_to_string(target.keybindings).unwrap_or_else(|_| "[]".into());
        let existing = match json::parse(&text, &mut Vec::new(), parse_options()) {
            Some(Value::Array(existing)) => existing,
            _ => Vec::new(),
        };
        let options = formatting_options(&text, configuration, target.keybindings);
        for entry in import.keybindings.iter().filter(|entry| !existing.contains(entry)) {
            // jsonEdit.ts appends with index -1; the port's index is clamped to the array's length.
            let edits = set_property(&text, &[Segment::Index(usize::MAX)], Some(entry), &options);
            text = apply_edits(&text, &edits);
            written.keybindings += 1;
        }
        save(target.keybindings, &text)?;
    }
    for (name, path) in &import.snippets {
        let destination = target.snippets_home.join(name);
        if destination.exists() {
            continue;
        }
        let content = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        write_user_data_file(&destination, &content).map_err(|error| format!("{}: {error}", destination.display()))?;
        written.snippets += 1;
    }
    Ok(written)
}

fn parse(path: &Path) -> Option<Value> {
    json::parse(&std::fs::read_to_string(path).ok()?, &mut Vec::new(), parse_options())
}

fn parse_options() -> ParseOptions {
    ParseOptions { allow_trailing_comma: true, allow_empty_content: true, ..ParseOptions::default() }
}

fn save(path: &Path, text: &str) -> Result<(), String> {
    write_user_data_file(path, text.as_bytes()).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests;
