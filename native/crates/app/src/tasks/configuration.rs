//! taskConfiguration.ts for the tasks.json shapes the port reads (version 2.0.0): custom `shell` and
//! `process` tasks and the configurations of contributed task types, with tasks.ts's `TaskDTO.from`
//! (mainThreadTask.ts) for what the extension host gets back from `fetchTasks`.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::definitions::{TaskDefinition, create_task_identifier};

/// `Tasks.CUSTOMIZED_TASK_TYPE`.
pub const CUSTOMIZED_TASK_TYPE: &str = "$customized";

/// One task the system knows: a tasks.json entry or one an extension provided.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Task {
    /// `_id`.
    pub id: String,
    /// The label the user sees and `runTask` matches: `name` for a configured task, `source: name` for a
    /// contributed one.
    pub label: String,
    /// `configurationProperties.name`.
    pub name: String,
    /// The keyed identifier (`definition`): `{ _key, type, ... }`.
    pub definition: Value,
    /// `ITaskDTO.execution`: shell, process or custom; `None` for a configuration of a contributed type.
    pub execution: Option<Value>,
    /// `ITaskSourceDTO` as the host gets it.
    pub source: Value,
    pub is_background: bool,
    pub group: Option<Value>,
    pub detail: Option<String>,
    /// `hide`: left out of the run pickers.
    pub hide: bool,
    pub presentation: Value,
    pub problem_matchers: Vec<Value>,
    pub has_defined_matchers: bool,
    pub depends_on: Vec<Value>,
    pub depends_in_sequence: bool,
    pub run_options: Value,
    pub workspace_folder: Option<PathBuf>,
    /// A tasks.json entry for a contributed type (`ConfiguringTask`), merged with the provided task when run.
    pub configures: bool,
}

impl Task {
    /// `TaskDTO.from`.
    pub fn to_dto(&self) -> Value {
        let mut dto = json!({
            "_id": self.id,
            "name": self.name,
            "definition": self.definition,
            "source": self.source,
            "execution": self.execution,
            "presentationOptions": if self.execution.is_some() { self.presentation.clone() } else { Value::Null },
            "isBackground": self.is_background,
            "problemMatchers": self.problem_matchers.iter().filter(|matcher| matcher.is_string()).collect::<Vec<_>>(),
            "hasDefinedMatchers": self.has_defined_matchers,
            "runOptions": self.run_options,
        });
        if let Some(group) = &self.group {
            dto["group"] = group.clone();
        }
        if let Some(detail) = &self.detail {
            dto["detail"] = json!(detail);
        }
        dto
    }

    /// `TaskDTO.to`: a task an extension provided or passed to `executeTask`.
    pub fn from_dto(dto: &Value, definition: Option<&TaskDefinition>, workspace_folder: Option<&Path>) -> Option<Task> {
        let name = dto["name"].as_str()?;
        let execution = dto["execution"].clone();
        let custom = execution["customExecution"] == "customExecution";
        if !custom && execution["commandLine"].is_null() && execution["command"].is_null() && execution["process"].is_null() {
            return None;
        }
        let mut definition_value = dto["definition"].clone();
        if let Some(object) = definition_value.as_object_mut() {
            object.remove("_key");
        }
        let identifier = create_task_identifier(&definition_value, definition).ok()?;
        let source = &dto["source"];
        let id = match (custom, dto["_id"].as_str().filter(|id| !id.is_empty())) {
            (true, Some(id)) => id.to_owned(),
            _ => format!("{}.{}", source["extensionId"].as_str().unwrap_or_default(), identifier["_key"].as_str().unwrap_or_default()),
        };
        Some(Task {
            id,
            label: format!("{}: {name}", source["label"].as_str().unwrap_or_default()),
            name: name.to_owned(),
            definition: identifier,
            execution: Some(execution),
            source: source.clone(),
            is_background: dto["isBackground"] == true,
            group: Some(dto["group"].clone()).filter(|group| !group.is_null()),
            detail: dto["detail"].as_str().map(str::to_owned),
            hide: dto["hide"] == true,
            presentation: dto["presentationOptions"].clone(),
            problem_matchers: dto["problemMatchers"].as_array().cloned().unwrap_or_default(),
            has_defined_matchers: dto["hasDefinedMatchers"] == true,
            depends_on: Vec::new(),
            depends_in_sequence: false,
            run_options: dto["runOptions"].clone(),
            workspace_folder: workspace_folder.map(Path::to_path_buf),
            configures: false,
        })
    }

    /// `createCustomTask`: a configuration's properties over the provided task it configures.
    pub fn customize(&self, configuration: &Task) -> Task {
        let mut task = self.clone();
        task.id = configuration.id.clone();
        task.name = configuration.name.clone();
        task.label = configuration.label.clone();
        task.source = configuration.source.clone();
        if configuration.group.is_some() {
            task.group = configuration.group.clone();
        }
        task.is_background = configuration.is_background || self.is_background;
        if !configuration.depends_on.is_empty() {
            task.depends_on = configuration.depends_on.clone();
            task.depends_in_sequence = configuration.depends_in_sequence;
        }
        if configuration.has_defined_matchers {
            task.problem_matchers = configuration.problem_matchers.clone();
            task.has_defined_matchers = true;
        }
        if configuration.detail.is_some() {
            task.detail = configuration.detail.clone();
        }
        task.hide = configuration.hide;
        if !configuration.run_options.is_null() {
            task.run_options = configuration.run_options.clone();
        }
        if !configuration.presentation.is_null() {
            task.presentation = configuration.presentation.clone();
        }
        task.configures = false;
        task
    }

    /// `ConfiguringTask` matching: same type and the configuration's key equals the provided task's.
    pub fn configures_task(&self, provided: &Task) -> bool {
        self.configures && self.definition["_key"] == provided.definition["_key"]
    }
}

/// `TaskConfigSource.TasksJson` entries of one folder (`parse` of taskConfiguration.ts for version 2.0.0):
/// `uuids` gives each custom task's id, stable across reads by label.
pub fn parse_tasks_json(config: &Value, folder: &Path, folder_uri: &Value, definitions: &[TaskDefinition], uuid: &mut dyn FnMut(&str) -> String) -> (Vec<Task>, Vec<String>) {
    let mut tasks = Vec::new();
    let mut errors = Vec::new();
    let globals = config.as_object().cloned().unwrap_or_default();
    for external in config["tasks"].as_array().into_iter().flatten() {
        let task_type = external["type"].as_str();
        let custom = matches!(task_type, None | Some("shell" | "process"));
        let source = json!({ "label": "Workspace", "extensionId": "$core", "scope": folder_uri });
        let common = common_properties(external, &globals);
        if custom {
            let Some(label) = external["label"].as_str().or(external["taskName"].as_str()) else {
                errors.push(format!("Error: a task must provide a label property. The task will be ignored.\n{}\n", pretty(external)));
                continue;
            };
            let Some(execution) = command_execution(external, task_type.unwrap_or("process")) else {
                if external["dependsOn"].is_null() {
                    errors.push(format!("Error: the task '{label}' doesn't define a command. The task will be ignored. Its definition is:\n{}", pretty(external)));
                    continue;
                }
                let task = Task { execution: None, ..custom_task(label, uuid(label), source, folder, common) };
                tasks.push(task);
                continue;
            };
            tasks.push(Task { execution: Some(execution), ..custom_task(label, uuid(label), source, folder, common) });
            continue;
        }
        let task_type = task_type.expect("custom tasks handled above");
        let Some(definition) = definitions.iter().find(|definition| definition.task_type == task_type) else {
            errors.push(format!("Error: there is no registered task type '{task_type}'. Did you miss installing an extension that provides a corresponding task provider?"));
            continue;
        };
        let identifier = match create_task_identifier(external, Some(definition)) {
            Ok(identifier) => identifier,
            Err(property) => {
                errors.push(format!("Error: the task identifier '{}' is missing the required property '{property}'. The task identifier will be ignored.", external));
                continue;
            }
        };
        // `_label`: the name, else the type and the first required property that has a value.
        let label = external["label"].as_str().map(str::to_owned).unwrap_or_else(|| {
            let value = definition.required.iter().find_map(|required| external[required.as_str()].as_str().filter(|value| !value.is_empty()));
            value.map_or_else(|| task_type.to_owned(), |value| format!("{task_type}: {value}"))
        });
        let id = format!("{}.{}", definition.extension_id, identifier["_key"].as_str().unwrap_or_default());
        tasks.push(Task {
            id,
            label: label.clone(),
            name: label,
            definition: identifier,
            execution: None,
            source,
            configures: true,
            ..common
        });
    }
    (tasks, errors)
}

fn custom_task(label: &str, id: String, source: Value, folder: &Path, common: Task) -> Task {
    Task {
        id,
        label: label.to_owned(),
        name: label.to_owned(),
        definition: json!({ "type": CUSTOMIZED_TASK_TYPE, "_key": format!("{CUSTOMIZED_TASK_TYPE},{label}") }),
        source,
        workspace_folder: Some(folder.to_path_buf()),
        ..common
    }
}

/// The configuration properties every kind of entry has (`ConfigurationProperties.from`), with the globals
/// filled in where the task leaves them (`fillGlobals`).
fn common_properties(external: &Value, globals: &Map<String, Value>) -> Task {
    let group = match &external["group"] {
        Value::String(kind) => Some(json!({ "_id": kind, "isDefault": false })),
        Value::Object(group) => group.get("kind").and_then(Value::as_str).map(|kind| json!({ "_id": kind, "isDefault": group.get("isDefault").filter(|value| value.is_string() || value.is_boolean()).cloned().unwrap_or(json!(false)) })),
        _ => None,
    };
    let matchers = if external.get("problemMatcher").is_some() { &external["problemMatcher"] } else { globals.get("problemMatcher").unwrap_or(&Value::Null) };
    let problem_matchers = match matchers {
        Value::Array(matchers) => matchers.clone(),
        Value::Null => Vec::new(),
        matcher => vec![matcher.clone()],
    };
    let depends_on = match &external["dependsOn"] {
        Value::Array(items) => items.clone(),
        Value::Null => Vec::new(),
        item => vec![item.clone()],
    };
    Task {
        id: String::new(),
        label: String::new(),
        name: String::new(),
        definition: Value::Null,
        execution: None,
        source: Value::Null,
        is_background: external["isBackground"] == true,
        group,
        detail: external["detail"].as_str().map(str::to_owned),
        hide: external["hide"] == true,
        presentation: if external["presentation"].is_object() { external["presentation"].clone() } else { globals.get("presentation").cloned().unwrap_or(Value::Null) },
        has_defined_matchers: !matchers.is_null(),
        problem_matchers,
        depends_on,
        depends_in_sequence: external["dependsOrder"] == "sequence",
        run_options: external["runOptions"].clone(),
        workspace_folder: None,
        configures: false,
    }
}

/// `CommandConfiguration.from` for a custom task: a `shell` command line (or command and args) or a `process`.
fn command_execution(external: &Value, task_type: &str) -> Option<Value> {
    let command = &external["command"];
    if command.is_null() {
        return None;
    }
    let args = external["args"].as_array().cloned().unwrap_or_default();
    let mut options = external["options"].clone();
    if let Some(shell) = options.get("shell").cloned() {
        options["executable"] = shell["executable"].clone();
        options["shellArgs"] = shell["args"].clone();
        options.as_object_mut().expect("options is an object").remove("shell");
    }
    let options = Some(options).filter(|options| !options.is_null());
    Some(match task_type {
        "process" => json!({ "process": command, "args": args.iter().map(|arg| arg.as_str().map_or_else(|| arg["value"].clone(), |text| json!(text))).collect::<Vec<_>>(), "options": options }),
        _ if args.is_empty() && command.is_string() => json!({ "commandLine": command, "options": options }),
        _ => json!({ "command": command, "args": args, "options": options }),
    })
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::definitions::definitions;
    use super::*;

    #[test]
    fn tasks_json_gives_custom_tasks_and_configurations() {
        let contributes = json!({ "taskDefinitions": [{ "type": "npm", "required": ["script"], "properties": { "script": { "type": "string" }, "path": { "type": "string" } } }] });
        let definitions = definitions(&[("vscode.npm", &contributes)]);
        let config = json!({
            "version": "2.0.0",
            "problemMatcher": "$tsc",
            "tasks": [
                { "label": "echo", "type": "shell", "command": "echo hi", "group": { "kind": "build", "isDefault": true } },
                { "label": "list", "type": "process", "command": "ls", "args": ["-l"], "problemMatcher": [] },
                { "type": "npm", "script": "build", "dependsOn": "echo" },
                { "type": "grunt", "task": "x" },
                { "type": "shell" }
            ]
        });
        let folder_uri = json!({ "$mid": 1, "scheme": "file", "path": "/w" });
        let mut next = 0;
        let (tasks, errors) = parse_tasks_json(&config, Path::new("/w"), &folder_uri, &definitions, &mut |_| {
            next += 1;
            format!("id{next}")
        });
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].execution, Some(json!({ "commandLine": "echo hi", "options": null })));
        assert_eq!(tasks[0].group, Some(json!({ "_id": "build", "isDefault": true })));
        assert_eq!(tasks[0].problem_matchers, [json!("$tsc")]);
        assert_eq!(tasks[1].execution, Some(json!({ "process": "ls", "args": ["-l"], "options": null })));
        assert!(tasks[1].problem_matchers.is_empty() && tasks[1].has_defined_matchers);
        assert!(tasks[2].configures);
        assert_eq!((tasks[2].id.as_str(), tasks[2].label.as_str()), ("vscode.npm.script,build,type,npm,", "npm: build"));
        assert_eq!(tasks[2].depends_on, [json!("echo")]);
        assert_eq!(tasks[2].to_dto()["source"]["extensionId"], "$core");
        assert_eq!(errors.len(), 2);
        assert!(errors[0].starts_with("Error: there is no registered task type 'grunt'"));
        assert!(errors[1].starts_with("Error: a task must provide a label property."));
    }

    #[test]
    fn a_provided_task_dto_gets_its_extension_keyed_id() {
        let dto = json!({
            "_id": "ignored", "name": "build", "definition": { "type": "npm", "script": "build" },
            "source": { "label": "npm", "extensionId": "vscode.npm" }, "execution": { "commandLine": "npm run build" }, "problemMatchers": []
        });
        let task = Task::from_dto(&dto, None, None).unwrap();
        assert_eq!((task.id.as_str(), task.label.as_str()), ("vscode.npm.script,build,type,npm,", "npm: build"));
        let custom = json!({ "_id": "uuid-1", "name": "c", "definition": { "type": "t" }, "source": { "label": "x" }, "execution": { "customExecution": "customExecution" }, "problemMatchers": [] });
        assert_eq!(Task::from_dto(&custom, None, None).unwrap().id, "uuid-1");
    }

    #[test]
    fn a_configuration_carries_its_run_options_onto_the_provided_task() {
        let provided = Task { definition: json!({ "type": "t", "_key": "k" }), run_options: json!({}), ..Task::default() };
        let configuration = Task { definition: json!({ "type": "t", "_key": "k" }), configures: true, run_options: json!({ "instanceLimit": 2 }), ..Task::default() };
        assert!(configuration.configures_task(&provided));
        assert_eq!(provided.customize(&configuration).run_options, json!({ "instanceLimit": 2 }));
    }
}
