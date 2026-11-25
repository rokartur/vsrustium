//! baseConfigurationResolverService.ts: the workbench's context for `${...}` variables, what the config crate's
//! `variables` module resolves against.

use std::collections::HashMap;

use config::configuration::Overrides;
use config::variables::{self, Context, VariableError};
use serde_json::Value;

use super::Workbench;
use crate::editor::utf16_column;

impl Workbench {
    /// searchActionsBase.ts and searchEditorActions.ts: each string argument through `resolveAsync` with the last
    /// active workspace root; other values stay as they are.
    // shortcut: `${command:}` and `${input:}` stay as written; their prompts come with tasks and launch (milestones 15, 16).
    pub(super) fn resolve_string_arguments(&self, mut args: Value) -> Result<Value, VariableError> {
        let Some(entries) = args.as_object_mut() else { return Ok(args) };
        let context = self.resolver_variable_context();
        let root = self.last_active_workspace_root();
        for value in entries.values_mut().filter(|value| value.is_string()) {
            *value = variables::resolve(root.as_deref(), value, &context)?;
        }
        Ok(args)
    }

    /// `historyService.getLastActiveWorkspaceRoot`: the folder of the active file, else the first folder.
    fn last_active_workspace_root(&self) -> Option<String> {
        let workspace = self.workspace()?;
        let file = self.editors.active().filter(|editor| editor.document().scheme == "file").map(|editor| editor.document().path.clone());
        let folder = file.and_then(|file| workspace.folders.iter().filter(|folder| file.starts_with(&folder.path)).max_by_key(|folder| folder.path.as_os_str().len()));
        Some(folder.or(workspace.folders.first())?.path.to_string_lossy().into_owned())
    }

    fn resolver_variable_context(&self) -> Context {
        let folders = self.workspace().map(|workspace| workspace.folders.iter().map(|folder| (folder.name.clone(), folder.path.to_string_lossy().into_owned())).collect()).unwrap_or_default();
        let settings = self.settings.clone();
        let config = move |folder: Option<&str>, section: &str| {
            let resource = folder.map(std::path::Path::new);
            settings.lock().unwrap().get_value(Some(section), &Overrides { resource, override_identifier: None })
        };
        let locations: HashMap<String, String> = self.extensions.iter().map(|extension| (extension.id.to_lowercase(), extension.location.to_string_lossy().into_owned())).collect();
        let extension_location = move |id: &str| locations.get(&id.to_lowercase()).cloned();

        let editor = self.editors.active();
        let file = editor.filter(|editor| matches!(editor.document().scheme.as_str(), "file" | "vscode-userdata")).map(|editor| editor.document().path.clone());
        let workspace_folder_for_file = file.as_ref().and_then(|file| {
            let workspace = self.workspace()?;
            let folder = workspace.folders.iter().filter(|folder| file.starts_with(&folder.path)).max_by_key(|folder| folder.path.as_os_str().len())?;
            Some(folder.path.to_string_lossy().into_owned())
        });
        let selection = editor.map(|editor| (editor, editor.selection()));
        Context {
            folders,
            config: Box::new(config),
            extension_location: Box::new(extension_location),
            file_path: file.map(|file| file.to_string_lossy().into_owned()),
            workspace_folder_for_file,
            selected_text: selection.map(|(editor, selection)| crate::editor::text_between(&editor.document().lines, selection.start(), selection.end())),
            line_number: selection.map(|(_, selection)| (selection.head.line + 1).to_string()),
            column_number: selection.map(|(editor, selection)| (utf16_column(&editor.document().lines[selection.head.line], selection.head.column) + 1).to_string()),
            exec_path: std::env::current_exe().ok().map(|path| path.to_string_lossy().into_owned()),
            app_root: Some(self.app_root.to_string_lossy().into_owned()),
            env: std::env::vars().collect(),
            user_home: std::env::var("HOME").ok(),
            cwd: std::env::current_dir().map(|path| path.to_string_lossy().into_owned()).unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests;
