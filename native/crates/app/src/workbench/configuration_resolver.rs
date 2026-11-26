//! baseConfigurationResolverService.ts: the workbench's context for `${...}` variables, what the config crate's
//! `variables` module resolves against.

use std::collections::HashMap;

use config::configuration::Overrides;
use config::variables::{self, Context, VariableError};
use futures::channel::oneshot;
use gpui::{AsyncWindowContext, WeakEntity};
use serde_json::{Map, Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::debug::configuration::{Interactive, interactive_variables, replace_variables};
use crate::editor::utf16_column;
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Entry, Item, Kind, QuickInput};

/// `LAST_INPUT_STORAGE_KEY` and `LAST_INPUT_CACHE_SIZE`: the last value of each prompt, per workspace.
const LAST_INPUT_KEY: &str = "configResolveInputLru";
const LAST_INPUT_SIZE: usize = 5;

impl Workbench {
    /// searchActionsBase.ts and searchEditorActions.ts: each string argument through `resolveAsync` with the last
    /// active workspace root; other values stay as they are.
    // Upstream's `resolveAsync` leaves `${command:}` and `${input:}` as written too; only tasks and launch prompt.
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
    pub(super) fn last_active_workspace_root(&self) -> Option<String> {
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

impl Workbench {
    /// `resolveInputs(folder, 'tasks')`: the `inputs` of the folder's tasks.json, else of the merged `tasks` value.
    pub(super) fn task_inputs(&self, folder: Option<&std::path::Path>) -> Option<Vec<Value>> {
        let settings = self.settings.lock().unwrap();
        let overrides = Overrides { resource: folder, override_identifier: None };
        let inspected = settings.configuration.inspect("tasks", &overrides);
        let folder_inputs = inspected.workspace_folder.value.and_then(|tasks| tasks["inputs"].as_array().cloned());
        folder_inputs.or_else(|| settings.get_value(Some("tasks"), &overrides).and_then(|tasks| tasks["inputs"].as_array().cloned()))
    }
}

/// `_collectTaskVariables`: the `${command:...}` and `${input:...}` of a shell or process task's command, args and
/// options, or of a custom execution's definition, as the array `resolveWithInteraction` gets; `None` without any.
pub(super) fn task_interactive_variables(task: &crate::tasks::configuration::Task) -> Option<Value> {
    let source = match &task.execution {
        Some(execution) if execution["customExecution"] == "customExecution" => {
            let mut definition = task.definition.clone();
            if let Some(members) = definition.as_object_mut() {
                members.remove("_key");
                members.remove("type");
            }
            definition
        }
        Some(execution) => execution.clone(),
        None => return None,
    };
    let variables = interactive_variables(&source);
    (!variables.is_empty()).then(|| Value::Array(variables.iter().map(|variable| Value::String(variable.variable())).collect()))
}

/// The task with its interactive variables replaced in its execution and definition.
pub(super) fn task_with_variables(mut task: crate::tasks::configuration::Task, resolved: &[(String, String)]) -> crate::tasks::configuration::Task {
    if let Some(execution) = &mut task.execution {
        replace_variables(execution, resolved);
    }
    replace_variables(&mut task.definition, resolved);
    task
}

/// `resolveWithInteraction(folder, config, section, variableToCommandMap)`: each `${command:...}` runs its command
/// (named through `commands`, a debugger's `variables`) with the configuration resolved so far, each `${input:...}`
/// asks as its `inputs` entry says. The variables with their values, or `None` when the user cancels a prompt.
pub(super) async fn resolve_with_interaction(this: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, config: &Value, section: &str, inputs: Option<&[Value]>, commands: &Map<String, Value>) -> Result<Option<Vec<(String, String)>>, String> {
    let mut resolved: Vec<(String, String)> = Vec::new();
    for variable in interactive_variables(config) {
        let value = match &variable {
            Interactive::Command(name) => {
                let command = commands.get(name).and_then(Value::as_str).unwrap_or(name).to_owned();
                let mut so_far = config.clone();
                replace_variables(&mut so_far, &resolved);
                match run_command(this, cx, &command, vec![so_far]).await? {
                    Value::Null => None,
                    Value::String(text) => Some(text),
                    _ => return Err(format!("Cannot substitute command variable '{command}' because command did not return a result of type string.")),
                }
            }
            Interactive::Input(name) => {
                let Some(inputs) = inputs else { return Err(format!("Variable '{name}' must be defined in an 'inputs' section of the debug or task configuration.")) };
                show_user_input(this, cx, section, name, inputs, commands).await?
            }
        };
        // A cancelled prompt (or a command without a value) skips the whole flow.
        let Some(value) = value else { return Ok(None) };
        resolved.push((variable.variable(), value));
    }
    Ok(Some(resolved))
}

/// `resolveWithInteractionReplace`: the configuration with its interactive variables replaced.
pub(super) async fn resolve_with_interaction_replace(this: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, mut config: Value, section: &str, inputs: Option<&[Value]>, commands: &Map<String, Value>) -> Result<Option<Value>, String> {
    let Some(resolved) = resolve_with_interaction(this, cx, &config, section, inputs, commands).await? else { return Ok(None) };
    replace_variables(&mut config, &resolved);
    Ok(Some(config))
}

/// `showUserInput`: a `promptString` input box, a `pickString` pick or a `command` input, the last value picked or
/// typed (`configResolveInputLru`) offered first.
async fn show_user_input(this: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, section: &str, variable: &str, inputs: &[Value], commands: &Map<String, Value>) -> Result<Option<String>, String> {
    let Some(info) = inputs.iter().rev().find(|input| input["id"] == variable) else {
        return Err(format!("Undefined input variable '{variable}' encountered. Remove or define '{variable}' to continue."));
    };
    let kind = info["type"].as_str().unwrap_or_default();
    let missing = |attribute: &str| format!("Input variable '{variable}' is of type '{kind}' and must include '{attribute}'.");
    let key = format!("{section}.{variable}");
    let previous = this.update(cx, |workbench, _| read_input_lru(&workbench.workspace_storage.lock().unwrap()).into_iter().find(|(stored, _)| *stored == key).map(|(_, value)| value)).map_err(|_| "Canceled".to_owned())?;
    let top = commands.get(&format!("input:{variable}")).and_then(Value::as_str).map(str::to_owned).or(previous).or_else(|| info["default"].as_str().map(str::to_owned));
    match kind {
        "promptString" => {
            let Some(description) = info["description"].as_str() else { return Err(missing("description")) };
            let password = info["password"] == true;
            let mut input = QuickInput::new(0, Kind::InputBox);
            input.prompt = Some(description.to_owned());
            input.ignore_focus_out = true;
            input.password = password;
            if let Some(value) = top {
                input.set_value(value);
            }
            let answer = ask(this, cx, input).await?;
            if let Some(value) = &answer
                && !password
            {
                store_input(this, cx, &key, value);
            }
            Ok(answer)
        }
        "pickString" => {
            let Some(description) = info["description"].as_str() else { return Err(missing("description")) };
            let Some(options) = info["options"].as_array() else { return Err(missing("options")) };
            let mut picks: Vec<(String, Item)> = Vec::new();
            for option in options {
                let (value, label) = match option {
                    Value::String(value) => (value.clone(), None),
                    _ => match option["value"].as_str() {
                        Some(value) => (value.to_owned(), option["label"].as_str()),
                        None => return Err(missing("value")),
                    },
                };
                let label = label.map_or_else(|| value.clone(), |label| format!("{label}: {value}"));
                let mut item = Item { label, ..Item::default() };
                if info["default"].as_str() == Some(value.as_str()) {
                    item.description = Some("(Default)".into());
                    picks.insert(0, (value, item));
                } else if top.as_deref() == Some(value.as_str()) {
                    picks.insert(0, (value, item));
                } else {
                    picks.push((value, item));
                }
            }
            let mut input = QuickInput::new(0, Kind::QuickPick);
            input.placeholder = Some(description.to_owned());
            input.match_on_detail = true;
            input.ignore_focus_out = true;
            input.set_items(picks.iter().enumerate().map(|(handle, (_, item))| Entry::Item(Item { handle: handle as i64, ..item.clone() })).collect());
            let picked = ask(this, cx, input).await?.and_then(|handle| handle.parse::<usize>().ok()).map(|index| picks[index].0.clone());
            if let Some(value) = &picked {
                store_input(this, cx, &key, value);
            }
            Ok(picked)
        }
        "command" => {
            let Some(command) = info["command"].as_str() else { return Err(missing("command")) };
            let args = if info["args"].is_null() { Vec::new() } else { vec![info["args"].clone()] };
            match run_command(this, cx, command, args).await? {
                Value::Null => Ok(None),
                Value::String(text) => Ok(Some(text)),
                _ => Err(format!("Cannot substitute input variable '{variable}' because command '{command}' did not return a result of type string.")),
            }
        }
        _ => Err(format!("Input variable '{variable}' can only be of type 'promptString', 'pickString', or 'command'.")),
    }
}

/// `quickInputService.input` or `pick`: the typed value, or the picked item's handle.
async fn ask(this: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, input: QuickInput) -> Result<Option<String>, String> {
    let (sender, receiver) = oneshot::channel();
    this.update_in(cx, |workbench, window, cx| {
        let id = workbench.quick_inputs.add(input, Owner::Debug(Some(sender)));
        workbench.show_quick_input(id, window, cx);
        workbench.flush_quick_input_events(window, cx);
    })
    .map_err(|_| "Canceled".to_owned())?;
    Ok(receiver.await.ok().flatten())
}

async fn run_command(this: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, command: &str, args: Vec<Value>) -> Result<Value, String> {
    let outcome = this.update_in(cx, |workbench, window, cx| workbench.execute_command(command, args, window, cx)).map_err(|_| "Canceled".to_owned())?;
    let result = match outcome {
        Outcome::Ready(result) => result,
        Outcome::Pending(receiver) => receiver.await.map_err(|_| "Canceled".to_owned())?,
    };
    result.map_err(|error| error["message"].as_str().or(error.as_str()).unwrap_or_default().to_owned())
}

/// `readInputLru`: `LRUCache.toJSON`'s `[key, value]` pairs, oldest first.
fn read_input_lru(storage: &storage::Storage) -> Vec<(String, String)> {
    storage.get(LAST_INPUT_KEY).and_then(|contents| serde_json::from_str(contents).ok()).unwrap_or_default()
}

/// `storeInputLru(lru.set(key, value))`: the pair moves to the newest end, the oldest past five go.
fn store_input(this: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, key: &str, value: &str) {
    let _ = this.update(cx, |workbench, _| {
        let mut storage = workbench.workspace_storage.lock().unwrap();
        let lru = with_input(read_input_lru(&storage), key, value);
        storage.set(LAST_INPUT_KEY, json!(lru).to_string());
    });
}

fn with_input(mut lru: Vec<(String, String)>, key: &str, value: &str) -> Vec<(String, String)> {
    lru.retain(|(stored, _)| stored != key);
    lru.push((key.to_owned(), value.to_owned()));
    let excess = lru.len().saturating_sub(LAST_INPUT_SIZE);
    lru.drain(..excess);
    lru
}

#[cfg(test)]
mod tests;
